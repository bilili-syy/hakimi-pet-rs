//! 外部事件的反应（`hakimi-pet.exe --event`，见 platform/ipc.rs）：
//! 跑完了 → 星星眼 + 提醒；出错 → 发抖着急；在等你 → 问号戳你；info → 只冒个气泡。
//! 附带的 msg 只显示不念（可能是代码 / 路径，念出来既慢又费 TTS）。
//! 她正忙（聊天、被拎着、跳舞、提醒在响……）就先存着，忙完再说；存太久就算了。
use super::fx::FxKind;
use super::{Behavior, Shot, St, lines, pick};
use crate::types::{ExtEvent, ExtKind, SpeakKind};

/// 忙的时候最多攒几条（同一来源只留最新一条）
const MAX_PENDING: usize = 4;
/// 攒着的事件过期时间
const PENDING_TTL: u64 = 10 * 60_000;
/// 气泡里 msg 最多显示多少字
const SHOW_MSG: usize = 60;

#[derive(Debug, Default)]
pub struct ExtUi {
    pending: Vec<(ExtEvent, u64)>,
}

fn short(s: &str) -> String {
    if s.chars().count() <= SHOW_MSG { s.to_string() } else { s.chars().take(SHOW_MSG).collect::<String>() + "…" }
}

impl Behavior {
    fn ext_busy(&self) -> bool {
        self.chatting || self.waiting || self.dragging || self.routine.is_some() || self.memo_ringing()
            || matches!(self.state, St::Drag | St::Fall | St::Dance | St::Eat)
    }

    pub(super) fn on_external(&mut self, e: ExtEvent) {
        self.call(format!("external:{:?}:{}", e.kind, e.source));
        if self.ext_busy() {
            let p = &mut self.ext.pending;
            p.retain(|(x, _)| x.source != e.source);
            p.push((e, self.now));
            if p.len() > MAX_PENDING { p.remove(0); }
            return;
        }
        self.ext_show(e);
    }

    /// 每帧：不忙了就把攒着的说出来（一次一条，按先后）
    pub(super) fn external_frame(&mut self) {
        if self.ext.pending.is_empty() { return; }
        let now = self.now;
        self.ext.pending.retain(|(_, t)| now.saturating_sub(*t) < PENDING_TTL);
        if self.ext.pending.is_empty() || self.ext_busy() { return; }
        // 刚说完一条的气泡还挂着就等等
        if self.bubble.as_ref().is_some_and(|b| b.hide_at.is_some_and(|h| h > now)) { return; }
        let (e, _) = self.ext.pending.remove(0);
        self.ext_show(e);
    }

    fn ext_show(&mut self, e: ExtEvent) {
        let src = if e.source.is_empty() { "那边".to_string() } else { e.source.clone() };
        let (st, pool, fx, n) = match e.kind {
            ExtKind::Done => (St::Starry, Some(lines::EXT_DONE), FxKind::Spark, 6),
            ExtKind::Error => (St::Scared, Some(lines::EXT_ERROR), FxKind::Sweat, 3),
            ExtKind::Wait => (St::Confused, Some(lines::EXT_WAIT), FxKind::Exclaim, 1),
            ExtKind::Info => (St::Happy, None, FxKind::Note, 1),
        };
        let line = pool.map(|p| pick(p).replace("{}", &src));
        let text = match (line, e.msg.is_empty()) {
            (Some(l), true) => l,
            (Some(l), false) => format!("{l}\n{}", short(&e.msg)),
            (None, false) => short(&e.msg),
            (None, true) => return,
        };
        if self.state == St::Sleep { self.set_state(St::Idle, None); }
        self.play(Shot::Jump);
        self.fx(fx, n);
        self.react(st, 3200, None);
        // 只有固定台词才念；带了 msg 的只看不念
        let voice = pool.is_some() && e.msg.is_empty();
        self.say_opts(&text, if e.msg.is_empty() { 3000 } else { 5500 }, voice, SpeakKind::Line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stats::Stats;
    use crate::types::{AppEvent, EventSender, WinCmd};

    fn mk() -> Behavior {
        fastrand::seed(3);
        let (s, _) = Stats::load(None);
        Behavior::with(EventSender::new(|_| {}), s, false, 0.0, String::new(), Vec::new(), false)
    }
    fn ev(b: &mut Behavior, e: AppEvent, now: u64) {
        let mut out: Vec<WinCmd> = Vec::new();
        b.event_at(e, now, &mut out);
    }
    fn tick(b: &mut Behavior, now: u64) {
        let mut out = Vec::new();
        b.frame_at(now, &mut out);
    }
    fn ext(kind: ExtKind, source: &str, msg: &str) -> AppEvent {
        AppEvent::External(ExtEvent { kind, source: source.into(), msg: msg.into() })
    }

    #[test]
    fn reacts_by_kind() {
        let mut b = mk();
        tick(&mut b, 3000);
        b.set_state(St::Idle, None);
        ev(&mut b, ext(ExtKind::Done, "Claude Code", ""), 4000);
        assert_eq!(b.state, St::Starry);
        assert!(b.bubble.as_ref().unwrap().text.contains("Claude Code"));
        b.set_state(St::Idle, None);
        ev(&mut b, ext(ExtKind::Error, "", "cargo build failed"), 9000);
        assert_eq!(b.state, St::Scared);
        let t = &b.bubble.as_ref().unwrap().text;
        assert!(t.contains("那边") && t.contains("cargo build failed"));
        b.set_state(St::Idle, None);
        ev(&mut b, ext(ExtKind::Wait, "codex", ""), 15000);
        assert_eq!(b.state, St::Confused);
        b.set_state(St::Idle, None);
        ev(&mut b, ext(ExtKind::Info, "x", "喵".repeat(100).as_str()), 21000);
        assert_eq!(b.bubble.as_ref().unwrap().text.chars().count(), SHOW_MSG + 1);
    }

    #[test]
    fn deferred_while_busy() {
        let mut b = mk();
        tick(&mut b, 3000);
        b.set_state(St::Idle, None);
        b.chatting = true;
        ev(&mut b, ext(ExtKind::Done, "a", ""), 4000);
        ev(&mut b, ext(ExtKind::Error, "a", ""), 4100);
        assert_ne!(b.state, St::Starry);
        assert_eq!(b.ext.pending.len(), 1, "同一来源只留最新一条");
        b.chatting = false;
        b.bubble = None;
        tick(&mut b, 5000);
        assert_eq!(b.state, St::Scared);
        assert!(b.ext.pending.is_empty());
    }

    #[test]
    fn pending_expires() {
        let mut b = mk();
        tick(&mut b, 3000);
        b.chatting = true;
        ev(&mut b, ext(ExtKind::Done, "a", ""), 4000);
        b.chatting = false;
        b.bubble = None;
        b.set_state(St::Idle, None);
        tick(&mut b, 4000 + PENDING_TTL + 1);
        assert!(b.ext.pending.is_empty());
        assert_ne!(b.state, St::Starry);
    }
}
