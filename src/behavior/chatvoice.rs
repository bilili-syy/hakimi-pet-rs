//! 语音回答（config.tts.chatVoiceReply）：AI 回复到了先别出字，保持「思考中」，等语音合成好，
//! 字和声音同一刻出来。长回复按句切开：每句的字在它的声音开始时才出现（下一句在上一句念的时候预先合成）。
//! 合成失败 / 某一句 15 秒还没好：剩下的字直接全显示，不出声。
//!
//! 只有几个钩子接在 mod.rs：`on_chat_reply` 里调 `cv_start`，Speech / AudioDone / Later::ChatVoiceTimeout 先给这里看，
//! `say_opts` 开头调 `cv_interrupted`（别的台词插进来时结束语音回答）。
use super::*;
use crate::types::Audio;

/// 每句最多等多久合成（毫秒）
pub(super) const CV_WAIT_MS: u64 = 15_000;
/// 最多切几段（再多就并到最后一段）
const MAX_CHUNKS: usize = 3;
/// 短于这个字数的句子和下一句合并（别一个「嗯！」也单独念）
const MIN_CHUNK: usize = 12;
/// 整段不超过这么多字就不切。MiMo 每次合成要等好几秒（主要花在请求本身，跟字数关系不大），
/// 切太碎反而句与句之间要干等；人设要求回复 ≤60 字，所以平常一整段一起出，只有长回复才分句
const NO_SPLIT_UNDER: usize = 60;

/// 进行中的语音回答
#[derive(Debug, Default)]
pub(super) struct ChatVoice {
    chunks: Vec<String>,
    /// 已经显示了几段
    shown: usize,
    /// 正在等合成结果的 token（对应第 shown 段）；0 = 没在等
    req: u64,
    /// 上一段还在念时预先合成好的下一段
    ready: Option<Audio>,
    /// 正在念的那段的 token
    playing: Option<u64>,
    /// 下一段合成失败/超时：当前这段念完把剩下的字全显示
    failed: bool,
}

/// 按句切：在 。！？!?～~… 和换行后断开（连着的标点算一起），太短的并到下一句，最多 MAX_CHUNKS 段
pub fn split_sentences(text: &str) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    if text.chars().count() <= NO_SPLIT_UNDER {
        return vec![text.to_string()];
    }
    let is_end = |c: char| "。！？!?～~…\n".contains(c);
    let mut raw: Vec<String> = Vec::new();
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
            raw.push(std::mem::take(&mut cur));
        }
    }
    if !cur.trim().is_empty() {
        raw.push(cur);
    }
    // 去掉换行两边的空白，丢空段
    let raw: Vec<String> = raw.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    // 太短的并到下一句
    let mut out: Vec<String> = Vec::new();
    let mut carry = String::new();
    for s in raw {
        carry.push_str(&s);
        if carry.chars().count() >= MIN_CHUNK {
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

/// 气泡停留时间：2.5 秒起，每字 +0.15 秒，最多 20 秒（和原来一样）
fn read_ms(text: &str) -> u64 {
    (2500 + text.encode_utf16().count() as u64 * 150).min(20_000)
}

impl Behavior {
    /// 这次回复要不要走「语音回答」
    pub(super) fn chat_voice_on(&self) -> bool {
        if self.song.is_some() {
            return false; // 只有一个声道：放歌时不念
        }
        if !self.io {
            return self.fake_chat_voice;
        }
        let t = crate::core::config::get().tts;
        t.enabled && t.chat_voice_reply && t.speak != "off" && !t.api_key.trim().is_empty()
    }

    /// 语音回答关着：聊天回复也不念（只是关掉这一项，不影响别的台词）
    pub(super) fn chat_voice_disabled_by_user(&self) -> bool {
        if !self.io {
            return self.fake_chat_voice_off;
        }
        !crate::core::config::get().tts.chat_voice_reply
    }

    /// AI 回复到了：保持思考，先合成第一段
    pub(super) fn cv_start(&mut self, text: &str) {
        let chunks = split_sentences(text);
        if chunks.is_empty() {
            return;
        }
        self.cv = Some(ChatVoice { chunks, ..Default::default() });
        self.cv_request();
    }

    /// 给第 shown 段发合成请求，并挂一个超时
    fn cv_request(&mut self) {
        let Some(cv) = &self.cv else { return };
        let Some(text) = cv.chunks.get(cv.shown).cloned() else { return };
        let token = self.next_token();
        if let Some(cv) = &mut self.cv {
            cv.req = token;
        }
        let mood = self.voice_mood();
        self.call(format!("cvspeak:{text}"));
        if self.io {
            crate::services::tts::speak(&self.tx, token, text, Some(mood), SpeakKind::Chat);
        }
        self.later(CV_WAIT_MS, Later::ChatVoiceTimeout(token));
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
            Some(a) if cv.playing.is_some() => cv.ready = Some(a),
            Some(a) => self.cv_reveal(Some(a)),
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
        if let Some(b) = &mut self.bubble
            && b.token == token
        {
            b.hide_at = Some(self.now + CV_WAIT_MS + 1000);
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
        }
    }

    /// 第一次出字时：结束「思考中」，和原来收到回复时一样
    fn cv_finish_thinking(&mut self) {
        self.waiting = false;
        self.cmds.push(WinCmd::ChatInput(ChatInputCmd::Enable(true)));
        self.act(Act::Chat);
        self.play(Shot::Bounce);
        self.set_state(St::Happy, None);
        self.hold_timer(2500, StateAct::ChatIdle);
    }

    /// 显示下一段的字（累加到气泡里），同时开始念它；然后预合成下一段
    fn cv_reveal(&mut self, audio: Option<Audio>) {
        let Some(cv) = &mut self.cv else { return };
        let first = cv.shown == 0;
        cv.shown = (cv.shown + 1).min(cv.chunks.len());
        let text = cv.chunks[..cv.shown].concat();
        let more = cv.shown < cv.chunks.len();
        if first {
            self.cv_finish_thinking();
        }
        let token = self.next_token();
        let dur = read_ms(&text);
        self.bubble = Some(Bubble { text, shown: self.now, hide_at: Some(self.now + 60_000), dur, token });
        self.bubble_fade.set(true, self.now);
        if let Some(a) = audio {
            self.call(format!("cvplay:{token}"));
            if self.io {
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
        self.bubble = Some(Bubble { text, shown: self.now, hide_at: Some(hide), dur: ms, token: cv.playing.unwrap_or(token) });
        self.bubble_fade.set(true, self.now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split() {
        assert_eq!(split_sentences("  "), Vec::<String>::new());
        assert_eq!(split_sentences("晴天喵～"), vec!["晴天喵～"]);
        // 一般长度（≤60 字）整段不切
        let short = "今天天气很好喵！我们出去玩吧～记得带伞，下午可能会下雨哦。";
        assert_eq!(split_sentences(short), vec![short]);
        // 长回复按句切，短句并到下一句，连着的标点算一起
        let long = "嗯！！这个问题哈基米知道……答案是四十二喵。宇宙、生命以及一切的终极答案就是它！不过哈基米觉得，比起答案，和你一起找答案更开心喵～你还想知道什么呀？";
        assert!(long.chars().count() > 60);
        let s = split_sentences(long);
        assert!(s.len() >= 2 && s.len() <= 3, "{s:?}");
        assert!(s[0].starts_with("嗯！！这个问题哈基米知道……"));
        assert!(s.iter().all(|c| c.chars().count() >= MIN_CHUNK));
        assert_eq!(s.concat(), long);
        // 最多 3 段，拼起来不丢字
        let many = "一二三四五六七八九十一二。".repeat(8);
        let s = split_sentences(&many);
        assert_eq!(s.len(), 3);
        assert_eq!(s.concat(), many);
        // 没有句末标点的长段
        let flat = "没有标点".repeat(20);
        assert_eq!(split_sentences(&flat), vec![flat.clone()]);
    }
}
