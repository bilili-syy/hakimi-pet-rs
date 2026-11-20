//! 伪 Live2D 的行为层钩子：什么时候呼吸扭曲、眨眼、按音量张嘴，以及要多快的帧率。
//! 画法和纯数学都在 render::live。
use super::{Behavior, St};
use crate::render::live::{self, BlinkSched, Kind, LiveView, MouthEnv};

/// 呼吸 / 头点 / 发梢的周期（毫秒）。互不整除，叠起来不机械
const BREATH_MS: f32 = 3600.0;
const SLEEP_BREATH_MS: f32 = 5600.0;
const BOB_MS: f32 = 4700.0;
const SWAY_MS: f32 = 3300.0;
/// 睡觉时呼吸更深
const SLEEP_DEPTH: f32 = 1.7;

pub(super) struct LiveState {
    pub(super) blink: BlinkSched,
    mouth: MouthEnv,
    last: u64,
    /// 三个相位的随机起点（0..1）
    phase: [f32; 3],
    /// 测试用：强制按「省电」处理（真机看平台层探测的 live::low_power）
    pub(super) force_low_power: bool,
}

impl LiveState {
    pub(super) fn new(now: u64) -> Self {
        Self {
            blink: BlinkSched::new(now, fastrand::f64()),
            mouth: MouthEnv::default(),
            last: now,
            phase: [fastrand::f32(), fastrand::f32(), fastrand::f32()],
            force_low_power: false,
        }
    }
}

/// 周期 period 的相位，带一点缓慢的周期抖动（±4%），每一口气长短略有不同
fn phase(now: u64, period: f32, offset: f32) -> f32 {
    let t = now as f32;
    let jitter = 0.04 * period / std::f32::consts::TAU * (t / 23_000.0).sin();
    ((t + jitter * std::f32::consts::TAU) / period + offset).rem_euclid(1.0)
}

impl Behavior {
    fn live_low_power(&self) -> bool { self.live.force_low_power || live::low_power() }

    /// 呼吸扭曲：None = 关；Some(true) = 睡觉的深呼吸
    fn live_mode(&self) -> Option<bool> {
        if self.routine.is_some() || self.dragging || self.dragging_cls { return None; }
        match self.state {
            St::Sleep => (!self.live_low_power()).then_some(true),
            St::Idle | St::Sit | St::Think if !self.live_low_power() => Some(false),
            _ => None,
        }
    }

    /// 能不能眨眼：这个表情有闭眼贴片，而且不在被拎/跳舞/睡觉
    pub(super) fn blink_allowed(&self) -> bool {
        self.routine.is_none()
            && !matches!(self.state, St::Drag | St::Fall | St::Dance | St::Sleep)
            && live::available(self.face, &self.costume, Kind::Blink)
    }

    /// 正在念台词 / 聊天语音（不算唱歌、跳舞的伴奏）
    pub(super) fn speaking(&self) -> bool {
        self.voice_playing.is_some()
            || (self.io && self.song.is_none() && self.routine.is_none() && crate::services::audio::is_playing())
    }

    /// 每帧：算出这一帧的灵动参数
    pub(super) fn live_view(&mut self) -> LiveView {
        let now = self.now;
        let dt = now.saturating_sub(self.live.last).min(200) as f32;
        self.live.last = now;
        let mut v = LiveView::default();
        if let Some(sleep) = self.live_mode() {
            let [pb, pk, ps] = self.live.phase;
            v.depth = if sleep { SLEEP_DEPTH } else { 1.0 };
            v.sleep = sleep;
            v.breath = phase(now, if sleep { SLEEP_BREATH_MS } else { BREATH_MS }, pb);
            v.bob = phase(now, BOB_MS, pk);
            v.sway = phase(now, SWAY_MS, ps);
        }
        if self.blink_allowed() {
            v.blink = self.live.blink.amount(now);
        } else {
            self.live.blink.cancel();
        }
        let target = if self.speaking() && live::available(self.face, &self.costume, Kind::Mouth) {
            let rms = if self.io { crate::services::audio::level() } else { self.fake_level };
            live::rms_to_open(rms)
        } else {
            0.0
        };
        v.mouth = live::mouth_alpha(self.live.mouth.step(target, dt));
        v
    }

    /// 眨眼计时器到点
    pub(super) fn on_blink(&mut self) {
        let ok = self.blink_allowed();
        self.live.blink.fire(self.now, ok, fastrand::f64(), fastrand::f64());
        self.next_blink = self.live.blink.next;
    }

    /// 灵动效果要的帧间隔（不需要时返回 u32::MAX）
    pub(super) fn live_interval(&self) -> u32 {
        let now = self.now;
        let mut ms = u32::MAX;
        if self.live.blink.active(now) { ms = ms.min(live::BLINK_FRAME_MS); }
        if let Some(d) = self.live.blink.due(now) { ms = ms.min(d.saturating_sub(now).max(1) as u32); }
        if self.live_mode().is_some() || self.live.mouth.value > 0.0 || (self.speaking() && live::available(self.face, &self.costume, Kind::Mouth)) {
            ms = ms.min(live::LIVE_FRAME_MS);
        }
        ms
    }

    /// 灵动扭曲开着时，老的整体缩放式呼吸（CSS breathe）就不要了
    pub(super) fn live_replaces_breathe(&self) -> bool { self.live_mode().is_some() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stats::Stats;
    use crate::types::{AppEvent, EventSender, Face, WinCmd};

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

    fn tick(b: &mut Behavior, now: u64) -> (u32, Vec<WinCmd>) {
        let mut out = Vec::new();
        let ms = b.frame_at(now, &mut out);
        (ms, out)
    }

    #[test]
    fn phase_wraps_and_advances() {
        for t in [0u64, 1000, 99_999, 1_000_000] {
            let p = phase(t, BREATH_MS, 0.3);
            assert!((0.0..1.0).contains(&p));
        }
        let a = phase(1000, BREATH_MS, 0.0);
        let b = phase(1100, BREATH_MS, 0.0);
        assert!(b > a && (b - a) < 0.05);
    }

    #[test]
    fn idle_breathes_at_live_rate_and_low_power_falls_back() {
        let mut b = mk();
        let mut t = 10_000;
        tick(&mut b, t);
        b.state_timer = None;
        b.fx.clear();
        b.bubble = None;
        b.shot = None;
        let mut live_frames = 0;
        for _ in 0..30 {
            let (ms, _) = tick(&mut b, t);
            b.state_timer = None;
            assert!(ms <= live::LIVE_FRAME_MS);
            if ms == live::LIVE_FRAME_MS { live_frames += 1; }
            t += ms as u64;
        }
        assert!(live_frames > 15);
        assert!(b.scene.live.warping());
        // 老的整体缩放呼吸关掉了
        assert_eq!(b.scene.pet.sx, 1.0);
        // 扭曲的相位在走
        let p0 = b.scene.live.breath;
        tick(&mut b, t + 500);
        assert_ne!(b.scene.live.breath, p0);
        // 省电：退回 200ms 和老的呼吸
        b.live.force_low_power = true;
        tick(&mut b, t + 600);
        assert!(!b.scene.live.warping());
    }

    #[test]
    fn sleep_breathes_deeper_drag_and_dance_off() {
        let mut b = mk();
        tick(&mut b, 3000);
        b.set_state(St::Sleep, None);
        tick(&mut b, 3100);
        assert!(b.scene.live.sleep && b.scene.live.depth > 1.0);
        b.set_state(St::Drag, None);
        tick(&mut b, 3200);
        assert!(!b.scene.live.warping());
        b.set_state(St::Dance, None);
        tick(&mut b, 3300);
        assert!(!b.scene.live.warping());
    }

    #[test]
    fn blinks_with_overlay_not_face_swap() {
        let mut b = mk();
        let mut t = 3000;
        tick(&mut b, t);
        b.state_timer = None;
        let mut seen = 0.0f32;
        // 2~6 秒内一定眨一次；脸不换成 Face::Blink
        while t < 12_000 {
            let (ms, _) = tick(&mut b, t);
            b.state_timer = None;
            if b.state == St::Idle { assert_ne!(b.scene.face, Face::Blink); }
            seen = seen.max(b.scene.live.blink);
            t += ms as u64;
        }
        assert!(seen > 0.9, "never blinked ({seen})");
        // 闭眼的表情不眨
        b.set_state(St::Sleep, None);
        b.next_blink = t;
        tick(&mut b, t);
        tick(&mut b, t + 60);
        assert_eq!(b.scene.live.blink, 0.0);
    }

    #[test]
    fn mouth_follows_voice_level() {
        let mut b = mk();
        tick(&mut b, 3000);
        b.state_timer = None;
        assert_eq!(b.scene.live.mouth, 0.0);
        // 假装在念台词
        b.voice_playing = Some(1);
        b.fake_level = 0.2;
        tick(&mut b, 3055);
        tick(&mut b, 3110);
        assert!(b.scene.live.mouth > 0.9, "{}", b.scene.live.mouth);
        b.fake_level = 0.0;
        for i in 0..6 { tick(&mut b, 3165 + i * 55); }
        assert_eq!(b.scene.live.mouth, 0.0);
        // 声音停了（AudioDone）
        b.fake_level = 0.3;
        tick(&mut b, 3600);
        ev(&mut b, AppEvent::AudioDone { token: 1 }, 3650);
        for i in 0..6 { tick(&mut b, 3700 + i * 55); }
        assert_eq!(b.scene.live.mouth, 0.0);
    }
}
