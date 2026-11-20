//! 备忘录的行为部分：到点跳出来喊、点她=知道了、双击=稍后、聊天里加备忘/问待办、菜单动作。
//! 数据和定时在 core::memo / 平台层；这里只管她怎么反应。
use super::{Behavior, Bubble, Later, Shot, St, StateAct, lines, pick};
use crate::core::memo::{self as store, Dt, Now};
use crate::services::memo_parse::{self as mp, Parsed};
use crate::types::{ChatInputCmd, ChatReply, MemoAct, MemoEv, MemoHit, SpeakKind, VoiceMood, WinCmd};
use super::fx::FxKind;

/// 提醒气泡留多久（没点她的话）
const ALERT_MS: u64 = 90_000;
/// 没点她的话隔多久再追一次（叮咚 + 再念一遍），追 NAGS 次
const NAG_MS: u64 = 30_000;
const NAGS: u8 = 2;
/// 点一下之后等多久才算「知道了」（等等看是不是双击）
const ACK_DELAY: u64 = 550;

#[derive(Debug, Default)]
pub struct MemoUi {
    /// 正在响的条目
    alert: Vec<u64>,
    alert_until: u64,
    bubble_token: u64,
    /// 叮咚放完再念的那句
    after_chime: Option<String>,
    /// 追问时念的
    nag_voice: String,
    /// 测试用的「现在」（io=false 时用）
    pub test_now: Option<Dt>,
}

impl MemoUi {
    fn active(&self, now: u64) -> bool { !self.alert.is_empty() && now < self.alert_until }

    /// AudioDone 之后气泡还留多久：提醒气泡留到 ALERT_MS，别的照旧 1 秒
    pub fn bubble_tail(&self, token: u64, now: u64) -> u64 {
        if token != 0 && token == self.bubble_token && self.active(now) {
            self.alert_until.saturating_sub(now).max(1000)
        } else {
            1000
        }
    }
}

fn strip_should(t: &str) -> &str {
    ["该", "要", "去"].iter().fold(t, |t, p| t.strip_prefix(p).filter(|r| !r.is_empty()).unwrap_or(t))
}

fn names(hits: &[MemoHit]) -> String {
    let mut s = hits.iter().take(3).map(|h| h.text.as_str()).collect::<Vec<_>>().join("、");
    if hits.len() > 3 {
        s.push_str(&format!(" 等 {} 件事", hits.len()));
    }
    s
}

/// (气泡, 语音)
pub fn alert_lines(hits: &[MemoHit]) -> (String, String) {
    const HINT: &str = "\n点我=知道了 · 双击=10 分钟后再叫";
    if hits.len() == 1 && !hits[0].late {
        let t = strip_should(&hits[0].text);
        return (format!("⏰ 该{t}了喵！{HINT}"), format!("喵！到点啦，该{t}了"));
    }
    let n = names(hits);
    if hits.iter().any(|h| h.late) {
        (format!("⏰ 刚才错过了：{n}{HINT}"), format!("刚才错过了：{n}，现在补上喵"))
    } else {
        (format!("⏰ 到点啦：{n}{HINT}"), format!("到点啦：{n}，别忘了喵"))
    }
}

/// 没点她时追问念的话
pub fn nag_line(hits: &[MemoHit]) -> String {
    let n = names(hits);
    if hits.len() == 1 { format!("{}！还没去吗？快去快去喵", strip_should(&n)) } else { format!("{n}，还没弄吗？快去喵") }
}

/// 单条准点提醒要念的话和语气（平台层拿去提前合成，到点直接命中缓存；clone 现合成要好几秒）：到点那句 + 追问那句
pub fn memo_voices(text: &str) -> Vec<(String, Option<VoiceMood>)> {
    let hits = [MemoHit { id: 0, text: text.to_string(), late: false }];
    let (_, v) = alert_lines(&hits);
    // memo_due 先切到 Surprised 再念，语气跟着它
    let m = St::Surprised.voice_mood();
    vec![(v, m), (nag_line(&hits), m)]
}

/// 提醒提示音：两声「叮——咚」（正弦 + 一点泛音，指数衰减），24 kHz 单声道 wav
pub fn chime_wav() -> Vec<u8> {
    const RATE: u32 = 24_000;
    let notes = [(1318.5_f32, 0.0_f32), (1046.5, 0.22)];
    let len = (RATE as f32 * 0.95) as usize;
    let mut pcm = Vec::with_capacity(len * 2);
    for i in 0..len {
        let t = i as f32 / RATE as f32;
        let mut s = 0.0;
        for (f, at) in notes {
            let dt = t - at;
            if dt < 0.0 { continue; }
            let env = (dt / 0.004).min(1.0) * (-dt * 5.5).exp();
            let w = std::f32::consts::TAU * f * dt;
            s += env * (w.sin() + 0.3 * (2.0 * w).sin() + 0.12 * (3.0 * w).sin());
        }
        let v = (s * 0.33).clamp(-1.0, 1.0);
        pcm.extend_from_slice(&((v * 32767.0) as i16).to_le_bytes());
    }
    crate::services::tts::pcm16_wav(RATE, &pcm)
}

impl Behavior {
    /// 提醒语音还在合成/在放：别的台词只换气泡，不掐它（之前被「不许拎哈基米」顶掉过）
    pub(super) fn memo_voice_busy(&self, token: u64) -> bool {
        let mt = self.memo.bubble_token;
        mt != 0 && token != mt && self.memo.active(self.now) && (self.voice_req == mt || self.voice_playing == Some(mt))
    }

    fn memo_now(&self) -> Dt {
        if self.io { Now::get().dt } else { self.memo.test_now.unwrap_or_else(|| Dt::new(2026, 1, 1, 12, 0).unwrap_or(Dt::from_min(0))) }
    }

    /// 气泡和语音分开给（气泡带操作提示，语音只念内容）。聊天类语音：只念聊天模式下也念
    fn memo_say(&mut self, bubble: &str, voice: &str, ms: u64) -> u64 {
        let token = self.next_token();
        self.bubble = Some(Bubble { text: bubble.to_string(), shown: self.now, hide_at: Some(self.now + ms), dur: ms, token });
        self.bubble_fade.set(true, self.now);
        self.voice(voice, SpeakKind::Chat, token);
        token
    }

    /// 这个 token 是提醒的（叮咚/提醒语音）
    pub(super) fn memo_token(&self, t: u64) -> bool { t != 0 && t == self.memo.bubble_token }

    /// 提醒还响着（没点、没过 ALERT_MS）：发呆换动作/打招呼/天气这些闲事先别来盖掉提醒气泡
    pub(super) fn memo_ringing(&self) -> bool { self.memo.active(self.now) }

    pub(super) fn on_memo(&mut self, ev: MemoEv) {
        match ev {
            MemoEv::Due(hits) => self.memo_due(hits),
            MemoEv::Reply { ok, text } => self.on_chat_reply(ChatReply { ok, text, need_key: false }),
        }
    }

    fn memo_due(&mut self, hits: Vec<MemoHit>) {
        if hits.is_empty() {
            return;
        }
        self.call(format!("memo:due:{}", hits.iter().map(|h| h.id.to_string()).collect::<Vec<_>>().join(",")));
        // 藏起来也要叫出来
        self.cmds.push(WinCmd::Show);
        for h in &hits {
            if !self.memo.alert.contains(&h.id) {
                self.memo.alert.push(h.id);
            }
        }
        if !matches!(self.state, St::Drag | St::Fall) {
            self.play(Shot::Bounce);
            self.set_state(St::Surprised, None);
            self.hold_timer(2500, StateAct::Set(St::Idle, Some((8000.0, 15000.0))));
        }
        self.fx(FxKind::Exclaim, 1);
        let (bubble, voice) = alert_lines(&hits);
        self.memo.alert_until = self.now + ALERT_MS;
        self.memo.nag_voice = nag_line(&hits);
        self.laters.retain(|l| !matches!(l.1, Later::MemoNag(_)));
        for n in 1..=NAGS {
            self.later(NAG_MS * n as u64, Later::MemoNag(n));
        }
        let token = self.next_token();
        self.bubble = Some(Bubble { text: bubble, shown: self.now, hide_at: Some(self.now + ALERT_MS), dur: ALERT_MS, token });
        self.bubble_fade.set(true, self.now);
        self.memo.bubble_token = token;
        self.memo_chime(voice);
    }

    /// 先「叮咚」，放完（AudioDone）再念；token 用提醒气泡的，别的台词就掐不掉它
    fn memo_chime(&mut self, voice: String) {
        let tok = self.memo.bubble_token;
        self.stop_voice();
        self.voice_req = 0;
        self.memo.after_chime = Some(voice);
        self.call("chime".into());
        if self.io {
            let vol = crate::core::config::get().tts.volume.max(0.5);
            crate::services::audio::play(tok, crate::types::Audio(chime_wav()), vol);
        }
        self.voice_playing = Some(tok);
    }

    /// AudioDone：叮咚放完了就接着念
    pub(super) fn memo_audio_done(&mut self, token: u64) {
        if token == 0 || token != self.memo.bubble_token || !self.memo.active(self.now) {
            return;
        }
        if let Some(v) = self.memo.after_chime.take() {
            self.voice(&v, SpeakKind::Chat, token);
        }
    }

    /// 没点她：隔一会儿再叮咚 + 追一句，蹦一下
    pub(super) fn memo_nag(&mut self, n: u8) {
        if self.memo.bubble_token == 0 || !self.memo.active(self.now) {
            return;
        }
        self.call(format!("memo:nag:{n}"));
        self.cmds.push(WinCmd::Show);
        if !matches!(self.state, St::Drag | St::Fall) {
            self.play(Shot::Jump);
        }
        self.fx(FxKind::Exclaim, 1);
        if let Some(b) = &mut self.bubble
            && b.token == self.memo.bubble_token
        {
            b.hide_at = Some(self.memo.alert_until);
        }
        let v = self.memo.nag_voice.clone();
        self.memo_chime(v);
    }

    /// 提醒响着时点她 = 知道了（稍等一下，看是不是双击）
    pub(super) fn memo_poke(&mut self) -> bool {
        if !self.memo.active(self.now) {
            self.memo.alert.clear();
            return false;
        }
        self.laters.retain(|l| l.1 != Later::MemoAck);
        self.later(ACK_DELAY, Later::MemoAck);
        self.play(Shot::Bounce);
        true
    }

    pub(super) fn memo_ack(&mut self) {
        let ids = std::mem::take(&mut self.memo.alert);
        if ids.is_empty() {
            return;
        }
        self.call(format!("memo:ack:{ids:?}"));
        self.memo.after_chime = None;
        if self.io {
            store::ack(&ids);
        }
        self.memo.bubble_token = 0;
        self.react(St::Happy, 1500, Some(pick(&["好喵，知道啦～", "收到喵！", "记住啦，去吧喵"])));
    }

    /// 提醒响着时双击 = 10 分钟后再叫
    pub(super) fn memo_double_click(&mut self) -> bool {
        if !self.memo.active(self.now) {
            return false;
        }
        self.laters.retain(|l| l.1 != Later::MemoAck);
        let ids = std::mem::take(&mut self.memo.alert);
        self.memo_snooze(ids);
        true
    }

    fn memo_snooze(&mut self, mut ids: Vec<u64>) {
        if self.io {
            for id in store::ringing() {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        self.memo.alert.clear();
        self.memo.bubble_token = 0;
        self.call(format!("memo:snooze:{ids:?}"));
        if ids.is_empty() {
            return;
        }
        if self.io {
            store::snooze(&ids);
        }
        self.play(Shot::Squash);
        self.say(&format!("好喵，{} 分钟后再叫你", store::SNOOZE_MIN), 2600);
    }

    pub(super) fn on_memo_menu(&mut self, a: MemoAct) {
        match a {
            MemoAct::Done(id) => {
                self.memo.alert.retain(|x| *x != id);
                if self.io {
                    store::complete(id);
                }
                self.call(format!("memo:done:{id}"));
                self.react(St::Happy, 1500, Some(pick(&["划掉啦，真棒喵！", "完成一件喵～"])));
            }
            MemoAct::Delete(id) => {
                self.memo.alert.retain(|x| *x != id);
                if self.io {
                    store::delete(id);
                }
                self.call(format!("memo:delete:{id}"));
                self.say("删掉啦喵", 2000);
            }
            MemoAct::Snooze => {
                let ids = std::mem::take(&mut self.memo.alert);
                self.laters.retain(|l| l.1 != Later::MemoAck);
                self.memo_snooze(ids);
            }
            MemoAct::ClearDone => {
                let n = if self.io { store::clear_done() } else { 0 };
                self.call("memo:clear".into());
                self.say(&if n > 0 { format!("清掉了 {n} 条做完的喵") } else { "没有做完的可以清喵".into() }, 2200);
            }
            // 平台层打开文件
            MemoAct::OpenFile | MemoAct::OpenWindow => {}
        }
    }

    /// 聊天输入先过一遍：是备忘/问待办就自己处理，返回 true；普通聊天返回 false
    pub(super) fn memo_chat(&mut self, text: &str) -> bool {
        let now = self.memo_now();
        match mp::parse(text, now) {
            Parsed::NotMemo => false,
            Parsed::Memo(d) => {
                self.call(format!("memo:add:{}|{}|{:?}", d.text, d.time.map(Dt::fmt).unwrap_or_default(), d.repeat));
                if self.io {
                    store::add(&d.text, d.time, d.repeat);
                }
                let line = mp::confirm_line(&d, now);
                self.memo_reply(&line);
                true
            }
            Parsed::Query => {
                let (timed, loose) = if self.io { store::today(now) } else { (vec![], vec![]) };
                let line = mp::today_line(&timed, &loose);
                self.call("memo:query".into());
                self.memo_reply(&line);
                true
            }
            Parsed::Unsure => {
                // 和普通聊天一样进入「思考中」，后台问 AI；不是备忘的话 AI 线程会转成普通聊天
                self.waiting = true;
                self.cmds.push(WinCmd::ChatInput(ChatInputCmd::Enable(false)));
                self.set_state(St::Think, None);
                self.say_opts(&pick(lines::THINK), 30_000, false, SpeakKind::Line);
                let d = &self.stats.data;
                let ctx = crate::services::ai::ChatCtx {
                    hunger: d.hunger.round() as u32,
                    mood: d.mood.round() as u32,
                    energy: d.energy.round() as u32,
                    level: self.stats.level().1.to_string(),
                    days: self.stats.days(),
                };
                self.call(format!("memo:ai:{text}"));
                if self.io {
                    mp::route_ai(&self.tx, text.to_string(), ctx);
                }
                true
            }
        }
    }

    fn memo_reply(&mut self, line: &str) {
        self.act(crate::core::stats::Act::Chat);
        self.play(Shot::Bounce);
        self.set_state(St::Happy, None);
        let ms = (2500 + line.encode_utf16().count() as u64 * 150).min(20_000);
        self.memo_say(line, line, ms);
        self.hold_timer(2500, StateAct::ChatIdle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stats::Stats;
    use crate::types::{AppEvent, Button, EventSender, MenuAction, MouseEv};

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
    fn click(b: &mut Behavior, now: u64) {
        ev(b, AppEvent::Mouse(MouseEv::Down { button: Button::Left, x: 120.0, y: 250.0, sx: 500, sy: 500 }), now);
        ev(b, AppEvent::Mouse(MouseEv::Up { button: Button::Left, x: 120.0, y: 250.0, sx: 500, sy: 500 }), now + 10);
    }

    fn hit(id: u64, text: &str, late: bool) -> MemoHit { MemoHit { id, text: text.into(), late } }

    #[test]
    fn lines_for_hits() {
        let (b, v) = alert_lines(&[hit(1, "开会", false)]);
        assert!(b.starts_with("⏰ 该开会了喵！"));
        assert_eq!(v, "喵！到点啦，该开会了");
        assert_eq!(alert_lines(&[hit(1, "该吃药", false)]).1, "喵！到点啦，该吃药了");
        let (_, v) = alert_lines(&[hit(1, "开会", true), hit(2, "喝药", true)]);
        assert_eq!(v, "刚才错过了：开会、喝药，现在补上喵");
        let many: Vec<_> = (1..=5).map(|i| hit(i, "x", false)).collect();
        assert!(alert_lines(&many).1.contains("等 5 件事"));
    }

    #[test]
    fn due_shows_her_and_click_acks() {
        let mut b = mk();
        tick(&mut b, 3000);
        let out = ev(&mut b, AppEvent::Memo(MemoEv::Due(vec![hit(7, "开会", false)])), 3100);
        assert!(out.contains(&WinCmd::Show));
        assert!(b.scene.bubble.as_ref().is_some_and(|x| x.text.contains("该开会了")));
        assert!(b.calls.iter().any(|c| c == "chime"));
        assert!(!b.calls.iter().any(|c| c == "speak:喵！到点啦，该开会了"));
        let tok = b.memo.bubble_token;
        // 别的台词掐不掉叮咚
        b.say("路过一句", 1000);
        ev(&mut b, AppEvent::AudioDone { token: tok }, 3900);
        assert!(b.calls.iter().any(|c| c == "speak:喵！到点啦，该开会了"));
        click(&mut b, 4000);
        assert!(!b.calls.iter().any(|c| c.starts_with("memo:ack")));
        tick(&mut b, 4600);
        assert!(b.calls.iter().any(|c| c == "memo:ack:[7]"));
        // 之后点她就是普通戳戳
        click(&mut b, 6000);
        assert_eq!(b.calls.iter().filter(|c| c.starts_with("memo:ack")).count(), 1);
    }

    #[test]
    fn alert_bubble_survives_idle_thinking() {
        let mut b = mk();
        tick(&mut b, 3000);
        ev(&mut b, AppEvent::Memo(MemoEv::Due(vec![hit(9, "喝水", false)])), 3100);
        let mut t = 3100;
        while t < 3100 + ALERT_MS - 2000 {
            t += 250;
            tick(&mut b, t);
            assert!(b.scene.bubble.as_ref().is_some_and(|x| x.text.contains("该喝水了")), "alert bubble replaced at {t}");
        }
        assert_eq!(b.state, St::Idle);
        assert!(!b.calls.iter().any(|c| c.starts_with("memo:ack")));
    }

    #[test]
    fn nags_until_acked() {
        let mut b = mk();
        tick(&mut b, 3000);
        ev(&mut b, AppEvent::Memo(MemoEv::Due(vec![hit(5, "该喝水", false)])), 3100);
        let mut t = 3100;
        while t < 3100 + NAG_MS + 500 { t += 250; tick(&mut b, t); }
        assert!(b.calls.iter().any(|c| c == "memo:nag:1"));
        assert_eq!(b.calls.iter().filter(|c| *c == "chime").count(), 2);
        let tok = b.memo.bubble_token;
        ev(&mut b, AppEvent::AudioDone { token: tok }, t + 900);
        assert!(b.calls.iter().any(|c| c == "speak:喝水！还没去吗？快去快去喵"));
        click(&mut b, t + 1500);
        tick(&mut b, t + 2200);
        assert!(b.calls.iter().any(|c| c == "memo:ack:[5]"));
        while t < 3100 + NAG_MS * 3 { t += 500; tick(&mut b, t); }
        assert!(!b.calls.iter().any(|c| c == "memo:nag:2"));
        assert!(chime_wav().len() > 40_000);
    }

    #[test]
    fn double_click_snoozes() {
        let mut b = mk();
        tick(&mut b, 3000);
        ev(&mut b, AppEvent::Memo(MemoEv::Due(vec![hit(3, "喝药", true)])), 3100);
        click(&mut b, 4000);
        click(&mut b, 4200);
        let out = ev(&mut b, AppEvent::Mouse(MouseEv::DoubleClick { x: 120.0, y: 250.0 }), 4220);
        assert!(!out.contains(&WinCmd::ChatInput(ChatInputCmd::Open)));
        assert!(b.calls.iter().any(|c| c == "memo:snooze:[3]"));
        tick(&mut b, 5000);
        assert!(!b.calls.iter().any(|c| c.starts_with("memo:ack")));
        // 没在响的时候双击还是开聊天
        let out = ev(&mut b, AppEvent::Mouse(MouseEv::DoubleClick { x: 120.0, y: 250.0 }), 6000);
        assert!(out.contains(&WinCmd::ChatInput(ChatInputCmd::Open)));
    }

    #[test]
    fn chat_adds_memo_and_queries() {
        let mut b = mk();
        b.memo.test_now = Dt::new(2026, 11, 20, 12, 41);
        tick(&mut b, 3000);
        ev(&mut b, AppEvent::ChatSubmit("提醒我 3 点开会".into()), 3100);
        assert!(b.calls.iter().any(|c| c == "memo:add:开会|2026-11-20 15:00|None"));
        assert!(!b.calls.iter().any(|c| c.starts_with("chat:")));
        assert!(b.scene.bubble.as_ref().is_some_and(|x| x.text == "好喵，下午 3 点叫你开会"));
        assert!(b.calls.iter().any(|c| c == "speak:好喵，下午 3 点叫你开会"));
        ev(&mut b, AppEvent::ChatSubmit("我有什么待办".into()), 9000);
        assert!(b.calls.iter().any(|c| c == "memo:query"));
        ev(&mut b, AppEvent::ChatSubmit("等会儿提醒我喝水".into()), 15000);
        assert!(b.calls.iter().any(|c| c == "memo:ai:等会儿提醒我喝水"));
        assert_eq!(b.state, St::Think);
        ev(&mut b, AppEvent::Memo(MemoEv::Reply { ok: true, text: "好喵，10 分钟后叫你喝水".into() }), 16000);
        assert!(b.scene.bubble.as_ref().is_some_and(|x| x.text == "好喵，10 分钟后叫你喝水"));
        // 普通聊天照旧走 AI
        ev(&mut b, AppEvent::ChatSubmit("你好呀".into()), 30000);
        assert!(b.calls.iter().any(|c| c == "chat:你好呀"));
    }

    #[test]
    fn menu_actions() {
        let mut b = mk();
        tick(&mut b, 3000);
        ev(&mut b, AppEvent::Memo(MemoEv::Due(vec![hit(4, "开会", false)])), 3100);
        ev(&mut b, AppEvent::Menu(MenuAction::Memo(MemoAct::Snooze)), 3200);
        assert!(b.calls.iter().any(|c| c == "memo:snooze:[4]"));
        ev(&mut b, AppEvent::Menu(MenuAction::Memo(MemoAct::Done(4))), 9000);
        assert!(b.calls.iter().any(|c| c == "memo:done:4"));
        ev(&mut b, AppEvent::Menu(MenuAction::Memo(MemoAct::Delete(4))), 12000);
        ev(&mut b, AppEvent::Menu(MenuAction::Memo(MemoAct::ClearDone)), 15000);
        assert!(b.calls.iter().any(|c| c == "memo:clear"));
    }
}
