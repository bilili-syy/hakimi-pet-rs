//! 备忘录的行为部分：到点跳出来喊、点她=知道了、双击=稍后、聊天里加备忘/问待办、菜单动作。
//! 数据和定时在 core::memo / 平台层；这里只管她怎么反应。
use super::{Behavior, Bubble, Later, Shot, St, StateAct, lines, pick};
use crate::core::memo::{self as store, Dt, Now};
use crate::services::memo_parse::{self as mp, Parsed};
use crate::types::{ChatInputCmd, ChatReply, MemoAct, MemoEv, MemoHit, SpeakKind, WinCmd};
use super::fx::FxKind;

/// 提醒气泡留多久（没点她的话）
const ALERT_MS: u64 = 60_000;
/// 点一下之后等多久才算「知道了」（等等看是不是双击）
const ACK_DELAY: u64 = 550;

#[derive(Debug, Default)]
pub struct MemoUi {
    /// 正在响的条目
    alert: Vec<u64>,
    alert_until: u64,
    bubble_token: u64,
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

impl Behavior {
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
        self.memo.bubble_token = self.memo_say(&bubble, &voice, ALERT_MS);
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
            MemoAct::OpenFile => {}
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
