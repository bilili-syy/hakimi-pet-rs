//! 情绪驱动表情 + 长期记忆的行为部分。
//! - 聊天回复：去掉开头的情绪标签（气泡和 TTS 只拿干净文字），说话期间换成对应表情 + 小动作，说完 2 秒回平时
//! - 聊天里「记住…/忘掉…/你记得我什么」
//! - 每几轮聊天在后台总结一次新事实（services::mind）
//! - 主动关心：每天最多 2 次，不打断聊天框和提醒
//!
//! mod.rs 里只有几行挂接调用。
use super::fx::FxKind;
use super::{Behavior, St, StateAct, Shot};
use crate::core::memo::{Dt, Now};
use crate::core::memory::{self as mstore, Book};
use crate::core::stats::Act;
use crate::services::emotion::{self, Emotion};
use crate::services::mind::{self, Cmd};
use crate::types::{ChatReply, MindAct, MindEv, SpeakKind};

/// 说完之后多久回到平时
pub const AFTER_MS: u64 = 2000;
/// 正在念：最长演多久（和气泡兜底一致）
const VOICE_MAX_MS: u64 = 60_000;
/// 每几轮成功的聊天总结一次新事实
pub const EXTRACT_EVERY: u32 = 4;
/// 主动关心：一天最多几次、两次至少隔多少分钟、启动多久之后才开始、每次 tick（20 秒）的概率
pub const CARE_PER_DAY: u32 = 2;
const CARE_GAP_MIN: i64 = 180;
const CARE_FIRST_MS: u64 = 10 * 60_000;
const CARE_CHANCE: f64 = 1.0 / 45.0;
/// 模型说「没什么好关心的」之后，多久再问
const CARE_RETRY_MS: u64 = 2 * 3_600_000;
/// 只在白天主动说（时）
const CARE_HOURS: std::ops::Range<u32> = 9..22;

#[derive(Debug, Default)]
pub struct MindUi {
    /// 正在演的情绪：(气泡的语音 token, 状态)
    emo: Option<(u64, St)>,
    /// 距上次总结又聊了几轮
    turns: u32,
    /// 正在等后台给关心的话
    care_wait: bool,
    /// 下一次可以问关心的时刻（行为层时钟）
    care_next: u64,
    /// 测试用：io=false 时的记忆、现在、概率
    pub test_book: Option<Book>,
    pub test_now: Option<Dt>,
    pub test_chance: Option<f64>,
}

pub fn emotion_state(e: Emotion) -> St {
    match e {
        Emotion::Happy => St::Happy,
        Emotion::Love => St::Love,
        Emotion::Shy => St::Shy,
        Emotion::Surprised => St::Surprised,
        Emotion::Cry => St::Cry,
        Emotion::Hiss => St::Hiss,
        Emotion::Think => St::Think,
        Emotion::Dizzy => St::Dizzy,
        Emotion::Stare => St::Stare,
        Emotion::Yawn => St::Yawn,
        Emotion::Idle => St::Idle,
    }
}

/// 不念的时候估一个「说话」时长：每字 0.23 秒，1.5–15 秒
pub fn speak_ms(chars: usize) -> u64 { (chars as u64 * 230).clamp(1500, 15_000) }

impl Behavior {
    fn mind_now(&self) -> Dt {
        if self.io { Now::get().dt } else { self.mind.test_now.unwrap_or_else(|| Dt::new(2026, 1, 1, 12, 0).unwrap_or(Dt::from_min(0))) }
    }

    /// 改记忆：真机走 core::memory（会写盘），测试用内存里的
    fn mind_book<R>(&mut self, f: impl FnOnce(&mut Book) -> (R, bool)) -> R {
        if self.io { mstore::edit(f) } else { f(self.mind.test_book.get_or_insert_with(Book::default)).0 }
    }

    // ============================================================ 情绪
    /// on_chat_reply 一开始：去掉情绪标签。失败提示不处理
    pub(super) fn emotion_split(&mut self, r: ChatReply) -> (ChatReply, Option<Emotion>) {
        if !r.ok {
            return (r, None);
        }
        let (e, text) = emotion::analyze(&r.text);
        self.call(format!("emotion:{}", e.label()));
        (ChatReply { text, ..r }, Some(e))
    }

    /// 开口之前：换表情 + 小动作（开心跳一下、害羞冒心、生气哈气……）。状态先定好，TTS 的语气也跟着走
    pub(super) fn emotion_face(&mut self, e: Option<Emotion>) {
        let Some(e) = e else { return };
        self.set_state(emotion_state(e), None);
        match e {
            Emotion::Happy => {
                self.play(Shot::Jump);
                self.fx(FxKind::Spark, 5);
            }
            Emotion::Love => {
                self.play(Shot::Bounce);
                self.fx(FxKind::Heart, 5);
            }
            Emotion::Shy => {
                self.play(Shot::Squash);
                self.fx(FxKind::Heart, 3);
            }
            Emotion::Surprised => {
                self.play(Shot::Bounce);
                self.fx(FxKind::Exclaim, 1);
            }
            Emotion::Cry => {
                self.play(Shot::Shake);
                self.fx(FxKind::Tear, 2);
            }
            Emotion::Hiss => {
                self.play(Shot::Shake);
                self.fx(FxKind::Anger, 1);
            }
            Emotion::Dizzy => self.fx(FxKind::Dizzy, 1),
            Emotion::Think | Emotion::Stare | Emotion::Yawn | Emotion::Idle => {}
        }
    }

    /// 开口之后：表情保持到说完，再过 2 秒回平时。
    /// 要念的话先照原来的 2.5 秒；语音一到（emotion_speech）就按真实念的时长来，语音来得晚就到时再换回这个表情
    pub(super) fn emotion_hold(&mut self, e: Option<Emotion>) {
        let Some(e) = e else { return };
        let st = emotion_state(e);
        let Some((token, n)) = self.bubble.as_ref().map(|b| (b.token, b.text.chars().count())) else { return };
        if self.state != st {
            return;
        }
        self.mind.emo = Some((token, st));
        let voiced = token != 0 && self.voice_req == token;
        if !voiced {
            self.hold_timer(speak_ms(n) + AFTER_MS, StateAct::ChatIdle);
        }
    }

    /// AppEvent::Speech 到了（在原来的处理之前调用）
    pub(super) fn emotion_speech(&mut self, token: u64, has_audio: bool) {
        let Some((t, st)) = self.mind.emo else { return };
        if t != token {
            return;
        }
        let will_play = has_audio
            && token == self.voice_req
            && self.bubble_fade.on
            && self.bubble.as_ref().is_some_and(|b| b.token == token);
        // 语音来晚了、表情已经回到平时：开口时再换回来（被别的动作改了状态就算了）
        if will_play && self.state == St::Idle && st != St::Idle && !self.dragging {
            self.set_state(st, None);
        }
        if self.state != st {
            self.mind.emo = None;
            return;
        }
        if will_play {
            self.hold_timer(VOICE_MAX_MS, StateAct::ChatIdle);
        } else {
            self.mind.emo = None;
            let n = self.bubble.as_ref().map(|b| b.text.chars().count()).unwrap_or(0);
            self.hold_timer(speak_ms(n) + AFTER_MS, StateAct::ChatIdle);
        }
    }

    /// AppEvent::AudioDone（念完了）：2 秒后回平时
    pub(super) fn emotion_audio_done(&mut self, token: u64) {
        let Some((t, st)) = self.mind.emo else { return };
        if t != token {
            return;
        }
        self.mind.emo = None;
        if self.state == st {
            self.hold_timer(AFTER_MS, StateAct::ChatIdle);
        }
    }

    /// 和聊天回复一样地说一句（换表情、气泡 + 聊天语音、说完回平时）
    fn mind_reply(&mut self, line: &str, e: Emotion) {
        self.emotion_face(Some(e));
        let ms = (2500 + line.encode_utf16().count() as u64 * 150).min(20_000);
        self.say_opts(line, ms, true, SpeakKind::Chat);
        self.hold_timer(2500, StateAct::ChatIdle);
        self.emotion_hold(Some(e));
    }

    // ============================================================ 记忆
    /// 回复之后：攒够几轮就后台总结一次
    pub(super) fn mind_after_reply(&mut self, ok: bool) {
        if !ok {
            return;
        }
        self.mind.turns += 1;
        if self.mind.turns < EXTRACT_EVERY {
            return;
        }
        self.mind.turns = 0;
        self.call("mind:extract".into());
        if self.io {
            mind::extract_in_background();
        }
    }

    /// 聊天输入先过一遍：「记住…」「忘掉…」「你记得我什么」自己处理，返回 true
    pub(super) fn mind_chat(&mut self, text: &str) -> bool {
        let Some(cmd) = mind::parse_command(text) else { return false };
        let now = self.mind_now();
        let (line, e) = match cmd {
            Cmd::Remember(f) => {
                let fact = f.clone();
                self.mind_book(move |b| {
                    let n = b.merge(&[fact], now, true);
                    ((), n > 0)
                });
                self.call("mind:remember".into());
                (mind::remember_line(&f), Emotion::Happy)
            }
            Cmd::Forget { query, sure } => {
                let q = query.clone();
                let gone = self.mind_book(move |b| {
                    let mut c = b.clone();
                    let g = c.forget(&q);
                    if g.is_empty() && !sure {
                        return (None, false);
                    }
                    *b = c;
                    (Some(g), true)
                });
                // 「忘记…」开头又对不上任何记忆：多半只是普通聊天
                let Some(gone) = gone else { return false };
                self.call(format!("mind:forget:{}", gone.len()));
                let e = if gone.is_empty() { Emotion::Think } else { Emotion::Cry };
                (mind::forget_line(&gone, &query), e)
            }
            Cmd::ForgetAll => (mind::FORGET_ALL_LINE.to_string(), Emotion::Cry),
            Cmd::Recall => {
                let (items, total) = self.mind_book(|b| ((b.recall(5), b.items.len()), false));
                self.call("mind:recall".into());
                let e = if items.is_empty() { Emotion::Shy } else { Emotion::Love };
                (mind::recall_line(&items, total), e)
            }
        };
        self.act(Act::Chat);
        self.mind_reply(&line, e);
        true
    }

    pub(super) fn on_mind_menu(&mut self, a: MindAct) {
        match a {
            // 平台层：打开文件 / 弹框确认（确认了才会再发 Clear）
            MindAct::OpenFile | MindAct::ClearAsk => {}
            MindAct::Clear => {
                self.yield_chat();
                if self.io {
                    mstore::clear();
                } else if let Some(b) = self.mind.test_book.as_mut() {
                    b.clear(0);
                }
                self.mind.turns = 0;
                self.mind.emo = None;
                self.call("mind:clear".into());
                self.react(St::Cry, 2600, Some("呜……全都忘掉了喵。那我们重新认识一下吧？".into()));
                self.fx(FxKind::Tear, 2);
            }
        }
    }

    // ============================================================ 主动关心
    /// 没在聊天、没在响提醒、手上没事、气泡也空着
    fn mind_free(&self) -> bool {
        !self.chatting
            && !self.waiting
            && !self.memo_ringing()
            && self.routine.is_none()
            && !self.bubble_fade.on
            && matches!(self.state, St::Idle | St::Sit | St::Walk)
    }

    /// on_tick（20 秒一次）最后调用
    pub(super) fn tick_mind(&mut self) {
        if !self.io && self.mind.test_now.is_none() {
            return;
        }
        if self.mind.care_wait || self.now < CARE_FIRST_MS || self.now < self.mind.care_next || !self.mind_free() {
            return;
        }
        let now = self.mind_now();
        if !CARE_HOURS.contains(&now.h) {
            return;
        }
        let ok = self.mind_book(|b| (b.care_ok(now, CARE_PER_DAY, CARE_GAP_MIN) && mind::has_care_topic(b), false));
        if !ok || fastrand::f64() >= self.mind.test_chance.unwrap_or(CARE_CHANCE) {
            return;
        }
        self.mind.care_wait = true;
        self.call("mind:care".into());
        if self.io && !mind::care_in_background(&self.tx) {
            self.mind.care_wait = false;
        }
    }

    pub(super) fn on_mind(&mut self, ev: MindEv) {
        match ev {
            MindEv::Care(line) => {
                self.mind.care_wait = false;
                let Some(line) = line else {
                    self.mind.care_next = self.now + CARE_RETRY_MS;
                    return;
                };
                // 这会儿在忙（开着聊天框、提醒在响……）：这次就不说了，也不算次数
                if !self.mind_free() {
                    self.mind.care_next = self.now + 10 * 60_000;
                    return;
                }
                let now = self.mind_now();
                self.mind_book(|b| {
                    b.care_mark(now);
                    ((), true)
                });
                let (e, text) = emotion::analyze(&line);
                self.call(format!("mind:say:{text}"));
                self.mind_reply(&text, e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::memory::{Fact, Kind};
    use crate::core::stats::Stats;
    use crate::types::{AppEvent, Audio, EventSender, Face, MenuAction, WinCmd};

    fn mk() -> Behavior {
        fastrand::seed(7);
        let (s, _) = Stats::load(None);
        Behavior::with(EventSender::new(|_| {}), s, false, 0.0, String::new(), Vec::new(), false)
    }
    fn ev(b: &mut Behavior, e: AppEvent, now: u64) -> Vec<WinCmd> {
        let mut out = Vec::new();
        b.event_at(e, now, &mut out);
        out
    }
    fn tick(b: &mut Behavior, now: u64) {
        let mut out = Vec::new();
        b.frame_at(now, &mut out);
    }
    fn reply(text: &str) -> AppEvent { AppEvent::ChatReply(ChatReply { ok: true, text: text.into(), need_key: false }) }
    fn bubble(b: &Behavior) -> String { b.scene.bubble.as_ref().map(|x| x.text.clone()).unwrap_or_default() }
    fn token(b: &Behavior) -> u64 { b.bubble.as_ref().map(|x| x.token).unwrap_or(0) }

    #[test]
    fn tag_sets_face_and_is_not_spoken() {
        let mut b = mk();
        tick(&mut b, 3000);
        ev(&mut b, reply("[cry]呜呜，好难过喵"), 3100);
        assert_eq!(b.state, St::Cry);
        assert_eq!(b.scene.face, Face::Cry);
        assert_eq!(bubble(&b), "呜呜，好难过喵");
        assert!(b.calls.iter().any(|c| c == "speak:呜呜，好难过喵"));
        assert!(!b.calls.iter().any(|c| c.contains("[cry]")));
        // 语音来了、念了 6 秒：念的时候一直是哭脸
        let tk = token(&b);
        ev(&mut b, AppEvent::Speech { token: tk, audio: Some(Audio(vec![1, 2, 3])) }, 4000);
        tick(&mut b, 9000);
        assert_eq!(b.state, St::Cry);
        ev(&mut b, AppEvent::AudioDone { token: tk }, 10_000);
        tick(&mut b, 11_500);
        assert_eq!(b.state, St::Cry, "念完还要停 2 秒");
        tick(&mut b, 12_100);
        assert_eq!(b.state, St::Idle);
    }

    #[test]
    fn late_voice_brings_face_back() {
        let mut b = mk();
        tick(&mut b, 3000);
        ev(&mut b, reply("[love]最喜欢你了"), 3100);
        assert_eq!(b.state, St::Love);
        let tk = token(&b);
        // TTS 慢：2.5 秒先回平时
        tick(&mut b, 5700);
        assert_eq!(b.state, St::Idle);
        ev(&mut b, AppEvent::Speech { token: tk, audio: Some(Audio(vec![1])) }, 6000);
        assert_eq!(b.state, St::Love, "开口时换回表情");
        ev(&mut b, AppEvent::AudioDone { token: tk }, 9000);
        tick(&mut b, 11_100);
        assert_eq!(b.state, St::Idle);
    }

    #[test]
    fn no_voice_uses_estimate() {
        let mut b = mk();
        tick(&mut b, 3000);
        ev(&mut b, reply("【HISS】哼，不理你了"), 3100);
        assert_eq!(b.state, St::Hiss);
        assert_eq!(bubble(&b), "哼，不理你了");
        // TTS 关了：Speech 没有音频
        let tk = token(&b);
        ev(&mut b, AppEvent::Speech { token: tk, audio: None }, 3300);
        let end = 3300 + speak_ms("哼，不理你了".chars().count()) + AFTER_MS;
        tick(&mut b, end - 100);
        assert_eq!(b.state, St::Hiss);
        tick(&mut b, end + 50);
        assert_eq!(b.state, St::Idle);
    }

    #[test]
    fn untagged_reply_guesses_or_idles() {
        let mut b = mk();
        tick(&mut b, 3000);
        ev(&mut b, reply("哈哈，好开心喵"), 3100);
        assert_eq!(b.state, St::Happy);
        assert!(b.calls.iter().any(|c| c == "emotion:happy"));
        ev(&mut b, reply("[banana]今天星期五"), 20_000);
        assert_eq!(b.state, St::Idle);
        assert_eq!(bubble(&b), "今天星期五");
        // 失败提示不动
        ev(&mut b, AppEvent::ChatReply(ChatReply { ok: false, text: "[x]网络不好".into(), need_key: false }), 40_000);
        assert_eq!(bubble(&b), "[x]网络不好");
    }

    #[test]
    fn extracts_every_few_turns() {
        let mut b = mk();
        tick(&mut b, 3000);
        for i in 0..EXTRACT_EVERY as u64 * 2 {
            ev(&mut b, reply("[idle]嗯嗯"), 4000 + i * 5000);
        }
        assert_eq!(b.calls.iter().filter(|c| *c == "mind:extract").count(), 2);
    }

    #[test]
    fn remember_recall_forget_in_chat() {
        let mut b = mk();
        b.mind.test_now = Some(Dt::new(2026, 11, 20, 10, 0).unwrap());
        tick(&mut b, 3000);
        ev(&mut b, AppEvent::ChatSubmit("记住：我喜欢草莓".into()), 3100);
        assert!(bubble(&b).contains("记住啦：你喜欢草莓"));
        assert!(!b.calls.iter().any(|c| c.starts_with("chat:")), "不该交给 AI");
        let book = b.mind.test_book.clone().unwrap();
        assert_eq!(book.items[0].text, "主人喜欢草莓");
        assert_eq!(book.items[0].created, "2026-11-20 10:00");
        ev(&mut b, AppEvent::ChatSubmit("记住我叫小明".into()), 8000);
        ev(&mut b, AppEvent::ChatSubmit("你记得我什么？".into()), 15_000);
        assert!(bubble(&b).starts_with("哈基米记得："), "{}", bubble(&b));
        assert!(bubble(&b).contains("你喜欢草莓"));
        assert_eq!(b.state, St::Love);
        ev(&mut b, AppEvent::ChatSubmit("忘掉草莓".into()), 25_000);
        assert!(bubble(&b).contains("忘掉了"));
        assert_eq!(b.mind.test_book.as_ref().unwrap().items.len(), 1);
        // 「忘记…」开头但对不上：普通聊天
        ev(&mut b, AppEvent::ChatSubmit("忘记带伞了怎么办".into()), 35_000);
        assert!(b.calls.iter().any(|c| c == "chat:忘记带伞了怎么办"));
    }

    #[test]
    fn menu_clear() {
        let mut b = mk();
        b.mind.test_book = Some(Book::default());
        b.mind.test_book.as_mut().unwrap().merge(&[Fact::new(Kind::Like, "主人喜欢猫")], Dt::from_min(0), true);
        tick(&mut b, 3000);
        ev(&mut b, AppEvent::Menu(MenuAction::Mind(MindAct::ClearAsk)), 3100);
        assert_eq!(b.mind.test_book.as_ref().unwrap().items.len(), 1, "只是问一下，还没确认");
        ev(&mut b, AppEvent::Menu(MenuAction::Mind(MindAct::Clear)), 3200);
        assert!(b.mind.test_book.as_ref().unwrap().items.is_empty());
        assert_eq!(b.state, St::Cry);
    }

    fn care_ready() -> Behavior {
        let mut b = mk();
        tick(&mut b, CARE_FIRST_MS + 1000);
        b.mind.test_now = Some(Dt::new(2026, 11, 20, 10, 0).unwrap());
        b.mind.test_chance = Some(1.0);
        let mut book = Book::default();
        book.merge(&[Fact::new(Kind::Busy, "主人今天考高数")], Dt::new(2026, 11, 19, 9, 0).unwrap(), true);
        b.mind.test_book = Some(book);
        b.set_state(St::Idle, None);
        b.bubble_fade.set(false, b.now);
        b
    }

    #[test]
    fn care_twice_a_day_at_most() {
        let mut b = care_ready();
        b.tick_mind();
        assert!(b.calls.iter().any(|c| c == "mind:care"));
        let t = b.now + 1000;
        ev(&mut b, AppEvent::Mind(MindEv::Care(Some("[think]今天考试考得怎么样喵？".into()))), t);
        assert_eq!(bubble(&b), "今天考试考得怎么样喵？");
        assert_eq!(b.state, St::Think);
        assert_eq!(b.mind.test_book.as_ref().unwrap().care.count, 1);
        // 3 小时后第二次，再之后当天不再说
        b.mind.test_now = Some(Dt::new(2026, 11, 20, 13, 30).unwrap());
        b.mind.test_book.as_mut().unwrap().care_mark(Dt::new(2026, 11, 20, 13, 30).unwrap());
        b.mind.test_now = Some(Dt::new(2026, 11, 20, 18, 0).unwrap());
        b.set_state(St::Idle, None);
        b.bubble_fade.set(false, b.now);
        let n = b.calls.iter().filter(|c| *c == "mind:care").count();
        b.tick_mind();
        assert_eq!(b.calls.iter().filter(|c| *c == "mind:care").count(), n);
    }

    #[test]
    fn care_never_interrupts() {
        // 聊天框开着：不问
        let mut b = care_ready();
        b.chatting = true;
        b.tick_mind();
        assert!(!b.calls.iter().any(|c| c == "mind:care"));
        // 问的时候还空，回来时已经开始聊天了：不说、不计次数
        let mut b = care_ready();
        b.tick_mind();
        b.chatting = true;
        let t = b.now + 1000;
        ev(&mut b, AppEvent::Mind(MindEv::Care(Some("[idle]考试怎么样喵".into()))), t);
        assert!(!b.calls.iter().any(|c| c.starts_with("mind:say")));
        assert_eq!(b.mind.test_book.as_ref().unwrap().care.count, 0);
        // 晚上不说
        let mut b = care_ready();
        b.mind.test_now = Some(Dt::new(2026, 11, 20, 23, 0).unwrap());
        b.tick_mind();
        assert!(!b.calls.iter().any(|c| c == "mind:care"));
        // 模型说没什么好关心的：过两小时再问
        let mut b = care_ready();
        b.tick_mind();
        let t = b.now + 1000;
        ev(&mut b, AppEvent::Mind(MindEv::Care(None)), t);
        b.tick_mind();
        assert_eq!(b.calls.iter().filter(|c| *c == "mind:care").count(), 1);
    }
}
