//! 语音回答（config.tts.chatVoiceReply）：AI 回复到了先别出字，保持「思考中」，等语音合成好，
//! 字和声音同一刻出来。回复按句切成一段段：每段的字在它的声音开始时才出现（下一段在上一段念的时候预先合成）。
//! 合成失败 / 某一段 15 秒还没好：剩下的字直接全显示，不出声。
//!
//! 句级流水线：MiMo clone 合成耗时跟字数走（实测短句 ~2 秒、55 字 4–9 秒），所以第一段尽量短（一句话，
//! 太长就在逗号处切），早点出声；后面的段在前一段念的时候合成。语气按情绪标签（Emotion::voice_mood，
//! 和状态 → 语气同一张表），不再用「思考中」的语气。放歌时也念（audio 两个声道混音，说话时音乐自动压低）。
//!
//! 只有几个钩子接在 mod.rs：`on_chat_reply` 里调 `cv_start`，Speech / AudioDone / Later::ChatVoiceTimeout 先给这里看，
//! `say_opts` 开头调 `cv_interrupted`（别的台词插进来时结束语音回答）。
use super::*;
use crate::services::emotion::Emotion;
use std::collections::VecDeque;
use crate::types::Audio;

/// 每句最多等多久合成（毫秒）
pub(super) const CV_WAIT_MS: u64 = 15_000;
/// 最多切几段（再多就并到最后一段）
const MAX_CHUNKS: usize = 4;
/// 第二段起：短于这个字数的句子和下一句合并（别一个「嗯！」也单独念，每次请求还有固定开销）
const MIN_CHUNK: usize = 12;
/// 整段不超过这么多字就不切（一次合成就很快）
const SHORT_ALL: usize = 16;
/// 第一段至少这么多字（「嗯！！」这种并到下一句）
const FIRST_MIN: usize = 6;
/// 第一段超过这么多字，就在逗号处切开，先念前半句
const FIRST_MAX: usize = 30;
/// 在逗号处切第一段时，前半句至少这么多字
const FIRST_CUT_MIN: usize = 8;

/// 进行中的语音回答
#[derive(Debug, Default)]
pub(super) struct ChatVoice {
    chunks: Vec<String>,
    /// 已经显示了几段
    shown: usize,
    /// 正在等合成结果的 token（对应第 shown 段）；0 = 没在等
    req: u64,
    /// 上一段还在念时预先合成好的下一段（音频, 合成用的 token）
    ready: Option<(Audio, u64)>,
    /// 正在念的那段的 token
    playing: Option<u64>,
    /// 下一段合成失败/超时：当前这段念完把剩下的字全显示
    failed: bool,
    /// 回复的情绪标签（决定念的语气）
    emo: Option<Emotion>,
    /// 回复到达的时刻（trace：回复 → 首音）
    started: u64,
}

fn clen(s: &str) -> usize { s.chars().count() }

/// 按句切：在 。！？!?～~… 和换行后断开（连着的标点算一起）。
/// 第一段是第一句（太短并下一句，太长在逗号处切），之后每段至少 MIN_CHUNK 字，最多 MAX_CHUNKS 段，拼起来等于原文
pub fn split_sentences(text: &str) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    if clen(text) <= SHORT_ALL {
        return vec![text.to_string()];
    }
    let is_end = |c: char| "。！？!?～~…\n".contains(c);
    let mut raw: VecDeque<String> = VecDeque::new();
    let mut cur = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        cur.push(c);
        if is_end(c) {
            while let Some(&n) = chars.peek() {
                if is_end(n) || n == '”' || n == '"' || n == '）' || n == ')' {
                    cur.push(n);
                    chars.next();
                } else {
                    break;
                }
            }
            raw.push_back(std::mem::take(&mut cur));
        }
    }
    if !cur.trim().is_empty() {
        raw.push_back(cur);
    }
    // 去掉换行两边的空白，丢空段
    let mut raw: VecDeque<String> = raw.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    // 第一段
    let mut first = String::new();
    while clen(&first) < FIRST_MIN {
        let Some(s) = raw.pop_front() else { break };
        first.push_str(&s);
    }
    if clen(&first) > FIRST_MAX
        && let Some(cut) = comma_cut(&first)
    {
        let rest = first.split_off(cut);
        raw.push_front(rest);
    }
    let mut out = vec![first];
    // 之后：太短的并到下一句
    let mut carry = String::new();
    for s in raw {
        carry.push_str(&s);
        if clen(&carry) >= MIN_CHUNK {
            out.push(std::mem::take(&mut carry));
        }
    }
    if !carry.is_empty() {
        match out.last_mut() {
            Some(l) => l.push_str(&carry),
            None => out.push(carry),
        }
    }
    while out.len() > MAX_CHUNKS {
        let last = out.pop().unwrap_or_default();
        if let Some(l) = out.last_mut() {
            l.push_str(&last);
        }
    }
    out
}

/// 第一段太长时的切点（字节位置）：最后一个前半句在 FIRST_CUT_MIN..=FIRST_MAX 字之间的逗号后面
fn comma_cut(s: &str) -> Option<usize> {
    let mut best = None;
    for (n, (i, c)) in s.char_indices().enumerate() {
        let len = n + 1;
        if len > FIRST_MAX {
            break;
        }
        if len >= FIRST_CUT_MIN && "，、；,;：:".contains(c) && i + c.len_utf8() < s.len() {
            best = Some(i + c.len_utf8());
        }
    }
    best
}

/// 气泡停留时间：2.5 秒起，每字 +0.15 秒，最多 20 秒（和原来一样）
fn read_ms(text: &str) -> u64 {
    (2500 + text.encode_utf16().count() as u64 * 150).min(20_000)
}

impl Behavior {
    /// 这次回复要不要走「语音回答」
    pub(super) fn chat_voice_on(&self) -> bool {
        if self.routine.is_some() || self.sense.quiet || self.memo_ringing() {
            return false; // 跳舞、全屏安静、备忘响铃时不走语音回答
        }
        if !self.io {
            return self.fake_chat_voice;
        }
        let t = crate::core::skin::effective_tts(crate::core::config::get().tts);
        t.enabled && t.chat_voice_reply && t.speak != "off" && crate::services::tts::online_ok(&t)
    }

    /// 语音回答关着：聊天回复也不念（只是关掉这一项，不影响别的台词）
    pub(super) fn chat_voice_disabled_by_user(&self) -> bool {
        if !self.io {
            return self.fake_chat_voice_off;
        }
        !crate::core::config::get().tts.chat_voice_reply
    }

    /// AI 回复到了：保持思考，先合成第一段。emo = 回复的情绪标签（决定语气）
    pub(super) fn cv_start(&mut self, text: &str, emo: Option<Emotion>) {
        let chunks = split_sentences(text);
        if chunks.is_empty() {
            return;
        }
        self.cv = Some(ChatVoice { chunks, emo, started: self.now, ..Default::default() });
        self.cv_request();
    }

    /// 语音回答念的语气：情绪标签优先，没有再按当前状态
    pub(super) fn cv_mood(&self) -> VoiceMood {
        match self.cv.as_ref().and_then(|cv| cv.emo) {
            Some(e) => e.voice_mood(),
            None => self.voice_mood(),
        }
    }

    fn cv_wait_ms(&self) -> u64 {
        if self.io && crate::core::skin::effective_tts(crate::core::config::get().tts).mode == "sovits" {
            65_000
        } else {
            CV_WAIT_MS
        }
    }

    /// 给第 shown 段发合成请求，并挂一个超时
    fn cv_request(&mut self) {
        let Some(cv) = &self.cv else { return };
        let Some(text) = cv.chunks.get(cv.shown).cloned() else { return };
        let token = self.next_token();
        if let Some(cv) = &mut self.cv {
            cv.req = token;
        }
        let mood = self.cv_mood();
        let prefetch = self.cv.as_ref().is_some_and(|c| c.playing.is_some());
        self.call(format!("cvspeak:{text}"));
        if self.io {
            if prefetch {
                crate::services::tts::speak_prefetch(&self.tx, token, text, Some(mood), SpeakKind::Chat);
            } else {
                crate::services::tts::speak(&self.tx, token, text, Some(mood), SpeakKind::Chat);
            }
        }
        let wait = self.cv_wait_ms();
        self.later(wait, Later::ChatVoiceTimeout(token));
    }

    /// 这个合成结果是不是语音回答在等的
    pub(super) fn cv_wants(&self, token: u64) -> bool {
        token != 0 && self.cv.as_ref().is_some_and(|cv| cv.req == token)
    }

    /// 语音回答在等的合成结果到了
    pub(super) fn cv_on_speech(&mut self, token: u64, audio: Option<Audio>) {
        let Some(cv) = &mut self.cv else { return };
        if token == 0 || cv.req != token {
            return;
        }
        cv.req = 0;
        match audio {
            Some(a) if cv.playing.is_some() => cv.ready = Some((a, token)),
            Some(a) => self.cv_reveal(Some((a, token))),
            None if cv.playing.is_some() => cv.failed = true,
            None => self.cv_give_up(),
        }
    }

    /// 某段念完了
    pub(super) fn cv_on_audio_done(&mut self, token: u64) {
        let Some(cv) = &mut self.cv else { return };
        if cv.playing != Some(token) {
            return;
        }
        cv.playing = None;
        if let Some(a) = cv.ready.take() {
            return self.cv_reveal(Some(a));
        }
        if cv.shown >= cv.chunks.len() {
            // 全念完了：字再留一会儿
            self.cv = None;
            if let Some(b) = &mut self.bubble
                && b.token == token
            {
                b.hide_at = Some(self.now + 2500);
            }
            return;
        }
        if cv.failed {
            return self.cv_give_up();
        }
        // 下一段还在合成：字先留着，等它（超时会兜底）
        let wait = self.cv_wait_ms();
        if let Some(b) = &mut self.bubble
            && b.token == token
        {
            b.hide_at = Some(self.now + wait + 1000);
        }
    }

    /// 等合成超时
    pub(super) fn cv_timeout(&mut self, token: u64) {
        let Some(cv) = &mut self.cv else { return };
        if cv.req != token {
            return;
        }
        cv.req = 0;
        let playing = cv.playing.is_some();
        if playing {
            cv.failed = true;
        }
        self.call("cvtimeout".into());
        if !playing {
            self.cv_give_up();
        }
    }

    /// 别的台词要插进来了：已经开始念的语音回答到此为止（还没出字的不管，结果回来照样显示）
    pub(super) fn cv_interrupted(&mut self) {
        if self.cv.as_ref().is_some_and(|cv| cv.shown > 0) {
            self.cv = None;
            self.emotion_cv_clear();
        }
    }

    /// 用户点了气泡：若还有未展示的段则立刻推进/出全文字；若已全出则停止语音并收起气泡
    pub(super) fn cv_click_bubble(&mut self) -> bool {
        let Some(cv) = &self.cv else { return false };
        if cv.shown == 0 {
            return false;
        }
        if cv.shown < cv.chunks.len() {
            let ready = self.cv.as_mut().and_then(|c| c.ready.take());
            self.stop_voice();
            if let Some(c) = &mut self.cv {
                c.playing = None;
            }
            if let Some(a) = ready {
                self.cv_reveal(Some(a));
            } else {
                self.cv_give_up();
            }
        } else {
            self.stop_voice();
            self.cv = None;
            self.emotion_cv_done();
            self.dismiss_bubble();
        }
        true
    }

    /// 用户关掉了语音开关：把还没念完的文字直接出全，清空队列不再续播已预合成的段
    pub(super) fn cv_disable_tts(&mut self) {
        if let Some(c) = &mut self.cv {
            c.playing = None;
            c.ready = None;
            self.cv_give_up();
        }
    }

    /// 第一次出字时：结束「思考中」，和原来收到回复时一样
    fn cv_finish_thinking(&mut self) {
        self.waiting = false;
        self.cmds.push(WinCmd::ChatInput(ChatInputCmd::Enable(true)));
        self.act(Act::Chat);
        if !self.dragging && !matches!(self.state, St::Drag | St::Fall) {
            self.play(Shot::Bounce);
            self.set_state(St::Happy, None);
            self.hold_timer(2500, StateAct::ChatIdle);
            // 情绪表情（behavior/emotion.rs）：出字这一刻换表情，念完 2 秒后回平时
            self.emotion_cv_reveal();
        } else {
            self.emotion_cv_clear();
        }
    }

    /// 显示下一段的字（累加到气泡里），同时开始念它；然后预合成下一段
    fn cv_reveal(&mut self, audio: Option<(Audio, u64)>) {
        // 合成期间切到全屏或备忘响铃：不再出声或盖掉备忘气泡
        if self.sense.quiet || self.memo_ringing() {
            if self.memo_ringing() {
                if self.cv.as_ref().is_some_and(|cv| cv.shown == 0) {
                    self.waiting = false;
                    self.cmds.push(WinCmd::ChatInput(ChatInputCmd::Enable(true)));
                }
                self.cv = None;
                self.emotion_cv_clear();
                return;
            }
            self.cv_give_up();
            return;
        }
        let Some(cv) = &mut self.cv else { return };
        let first = cv.shown == 0;
        let started = cv.started;
        cv.shown = (cv.shown + 1).min(cv.chunks.len());
        let text = cv.chunks[..cv.shown].concat();
        let more = cv.shown < cv.chunks.len();
        if first {
            self.cv_finish_thinking();
        }
        let token = self.next_token();
        let dur = read_ms(&text);
        self.show_bubble(text, Some(self.now + 60_000), dur, token, true);
        if let Some((a, req)) = audio {
            self.call(format!("cvplay:{token}"));
            if first {
                self.call(format!("cv reply->first sound {}ms", self.now.saturating_sub(started)));
            }
            if self.io {
                crate::services::tts::lat_link(req, token);
                let vol = crate::core::config::get().tts.volume;
                crate::services::audio::play(token, a, vol);
            }
            self.voice_req = token;
            self.voice_playing = Some(token);
            if let Some(cv) = &mut self.cv {
                cv.playing = Some(token);
            }
        }
        if more {
            self.cv_request();
        }
    }

    /// 不念了：剩下的字一次性显示
    fn cv_give_up(&mut self) {
        let Some(cv) = self.cv.take() else { return };
        let first = cv.shown == 0;
        let text = cv.chunks.concat();
        self.call("cvgiveup".into());
        if first {
            self.cv_finish_thinking();
        }
        let token = self.next_token();
        let ms = read_ms(&text);
        let hide = if cv.playing.is_some() { self.now + 60_000 } else { self.now + ms };
        self.show_bubble(text, Some(hide), ms, cv.playing.unwrap_or(token), true);
        if !first && cv.playing.is_none() {
            self.emotion_cv_done();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split() {
        assert_eq!(split_sentences("  "), Vec::<String>::new());
        assert_eq!(split_sentences("晴天喵～"), vec!["晴天喵～"]);
        // 很短的整段不切
        let tiny = "好呀，我们出去玩吧喵！";
        assert_eq!(split_sentences(tiny), vec![tiny]);
        // 一般长度：第一句先念（早出声），后面的句子各成一段
        let short = "今天天气很好喵！我们出去玩吧～记得带伞，下午可能会下雨哦。";
        let s = split_sentences(short);
        assert_eq!(s, vec!["今天天气很好喵！", "我们出去玩吧～记得带伞，下午可能会下雨哦。"]);
        // 长回复：「嗯！！」太短并到下一句；之后每段 ≥ MIN_CHUNK；最多 4 段；拼起来不丢字
        let long = "嗯！！这个问题哈基米知道……答案是四十二喵。宇宙、生命以及一切的终极答案就是它！不过哈基米觉得，比起答案，和你一起找答案更开心喵～你还想知道什么呀？";
        let s = split_sentences(long);
        assert_eq!(s[0], "嗯！！这个问题哈基米知道……");
        assert!(s.len() >= 3 && s.len() <= MAX_CHUNKS, "{s:?}");
        assert!(s[1..].iter().all(|c| c.chars().count() >= MIN_CHUNK), "{s:?}");
        assert_eq!(s.concat(), long);
        // 最多 4 段
        let many = "一二三四五六七八九十一二。".repeat(8);
        let s = split_sentences(&many);
        assert_eq!(s.len(), MAX_CHUNKS);
        assert_eq!(s.concat(), many);
        // 第一句太长：在逗号处切，前半句先念
        let comma = "哈基米觉得今天特别适合出去晒太阳，因为窗外的阳光暖洋洋的，小鸟也在叫，真的好舒服呀喵～";
        let s = split_sentences(comma);
        assert!(s[0].chars().count() <= FIRST_MAX && s[0].ends_with('，'), "{s:?}");
        assert_eq!(s.concat(), comma);
        // 没有标点的长段：没法切，整段
        let flat = "没有标点".repeat(20);
        assert_eq!(split_sentences(&flat), vec![flat.clone()]);
    }

    #[test]
    fn emotion_mood_matches_state_mood() {
        // 情绪标签的语气和同名状态的语气一致（同一张表）
        for (e, st) in [
            (Emotion::Happy, St::Happy),
            (Emotion::Love, St::Love),
            (Emotion::Shy, St::Shy),
            (Emotion::Surprised, St::Surprised),
            (Emotion::Cry, St::Cry),
            (Emotion::Hiss, St::Hiss),
            (Emotion::Think, St::Think),
            (Emotion::Dizzy, St::Dizzy),
            (Emotion::Stare, St::Stare),
            (Emotion::Yawn, St::Yawn),
        ] {
            assert_eq!(st.voice_mood(), Some(e.voice_mood()), "{e:?}");
        }
        assert_eq!(Emotion::Idle.voice_mood(), VoiceMood::Calm);
    }
}
