//! 伪 Live2D 的行为层钩子：什么时候呼吸扭曲、眨眼、按音量张嘴，以及要多快的帧率。
//! 画法和纯数学都在 render::live。
use super::{Behavior, PetAnim, St};
use crate::render::Xf;
use crate::render::live::{self, BlinkSched, Kind, LiveView, MouthEnv};
use crate::types::Face;

/// 呼吸 / 头点 / 发梢的周期（毫秒）。互不整除，叠起来不机械
const BREATH_MS: f32 = 3600.0;
const SLEEP_BREATH_MS: f32 = 5600.0;
const BOB_MS: f32 = 4700.0;
const SWAY_MS: f32 = 3300.0;
/// 睡觉时呼吸更深
const SLEEP_DEPTH: f32 = 1.7;
/// 灵动扭曲开/关时强度渐变的时长（不再一帧跳变）
const DEPTH_RAMP_MS: f32 = 400.0;
/// 表情切换的交叉淡化时长
pub(super) const XFADE_MS: u64 = 140;
/// 整体动画（#pet 的 CSS 动画）切换时从上一帧的姿态过渡过去的时长
const PET_BLEND_MS: u64 = 180;

pub(super) struct LiveState {
    pub(super) blink: BlinkSched,
    mouth: MouthEnv,
    last: u64,
    /// 三个相位的随机起点（0..1）
    phase: [f32; 3],
    /// 测试用：强制按「省电」处理（真机看平台层探测的 live::low_power）
    pub(super) force_low_power: bool,
    /// 当前扭曲强度（向目标值渐变）
    depth: f32,
    /// 上一帧画的 (表情, 衣服)；表情变了就从旧的淡过去
    shown: Option<(Face, String)>,
    /// 交叉淡化：(旧表情, 开始时刻)
    fade: Option<(Face, u64)>,
    /// 上一帧输出的整体变换 + 那时的动画 (种类, 起点)
    pet_last: Xf,
    pet_key: Option<(PetAnim, u64)>,
    /// 整体动画切换的过渡：(从哪个姿态, 开始时刻)
    pet_from: Option<(Xf, u64)>,
}

impl LiveState {
    pub(super) fn new(now: u64) -> Self {
        Self {
            blink: BlinkSched::new(now, fastrand::f64()),
            mouth: MouthEnv::default(),
            last: now,
            phase: [fastrand::f32(), fastrand::f32(), fastrand::f32()],
            force_low_power: false,
            depth: 0.0,
            shown: None,
            fade: None,
            pet_last: Xf::ID,
            pet_key: None,
            pet_from: None,
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
            // 站着的平静表情也呼吸（以前只有待机/坐/思考，其它表情退回 5fps 的整体缩放）
            St::Idle | St::Sit | St::Think | St::Hungry | St::Stare | St::Love | St::Shy | St::Happy | St::Surprised if !self.live_low_power() => Some(false),
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
        let mode = self.live_mode();
        // 强度渐变：开/关不再一帧跳 1~2px；被拎起来、跳舞时立刻关（那时有别的大动作）
        let target = match mode { Some(true) => SLEEP_DEPTH, Some(false) => 1.0, None => 0.0 };
        let hard_off = self.routine.is_some() || self.dragging || self.dragging_cls || matches!(self.state, St::Drag | St::Fall | St::Dance) || self.live_low_power();
        self.live.depth = if hard_off { target } else { ramp(self.live.depth, target, dt) };
        if self.live.depth > 0.001 {
            let [pb, pk, ps] = self.live.phase;
            let sleep = mode == Some(true) || (mode.is_none() && self.state == St::Sleep);
            v.depth = self.live.depth;
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
        // 表情切换：旧表情盖在上面淡出
        let cur = (self.face, self.costume.clone());
        if self.live.shown.as_ref() != Some(&cur) {
            self.live.fade = match &self.live.shown {
                Some((pf, pc)) if *pc == cur.1 && self.xfade_ok(*pf, cur.0) => Some((*pf, now)),
                _ => None,
            };
            self.live.shown = Some(cur);
        }
        if let Some((pf, t0)) = self.live.fade {
            let a = xfade_alpha(now.saturating_sub(t0));
            if a > 0.0 && !self.dragging_cls && self.routine.is_none() {
                v.prev_face = Some(pf);
                v.prev_alpha = a;
            } else {
                self.live.fade = None;
            }
        }
        v
    }

    /// 哪些表情切换要交叉淡化：姿势基本一样的站姿/坐姿表情之间。走路两帧、眨眼帧、被拎、跳舞、睡觉这些不淡（会出重影）
    fn xfade_ok(&self, from: Face, to: Face) -> bool {
        if self.state == St::Walk || self.routine.is_some() || self.dragging_cls { return false; }
        let group = |f: Face| match f {
            Face::Idle | Face::Happy | Face::Hungry | Face::Think | Face::Shy | Face::Surprised | Face::Love | Face::Stare | Face::Cry | Face::Hiss | Face::Honey | Face::Eat | Face::Yawn => Some(0),
            Face::Sit => Some(1),
            _ => None,
        };
        from != to && group(from).is_some() && group(from) == group(to)
    }

    /// 走路换帧（替代原来固定 160ms 一帧）：跟整体动画的上下颠同步——脚落地（颠到最低）时是迈开腿的 walk1/walk2，
    /// 最高点是收腿的 idle。正常走一步 250ms，喝了蜂蜜水快走 125ms，脚步和窗口移动的距离对得上，不再滑步
    pub(super) fn walk_tick(&mut self, now: u64) {
        // now = 这一轮计时器的「真」当前时刻（self.now 是到期时刻）：落后很多时直接跳到当前该显示的那帧，不逐帧补
        let now = now.max(self.now);
        let (start, dur) = match self.pet_anim {
            (PetAnim::Walk(fast), t) => (t, if fast { 250.0 } else { 500.0 }),
            _ => (now, 500.0),
        };
        let e = now.saturating_sub(start) as f32;
        let (i, next) = walk_frame_at(e, dur);
        const WALK_SEQ: [Face; 4] = [Face::Walk1, Face::Idle, Face::Walk2, Face::Idle];
        self.walk_frame = i;
        self.show(WALK_SEQ[i]);
        self.next_walk_frame = start + next.max(e + 1.0).ceil() as u64;
    }

    /// 整体动画切换时（呼吸 → 走路 → 呼吸、弹一下……）从上一帧的姿态平滑过渡，不再一帧跳过去；相位从新动画的起点算，连续
    pub(super) fn blend_pet(&mut self, pet: Xf) -> Xf {
        let now = self.now;
        let key = Some(self.pet_anim);
        if self.live.pet_key != key {
            // 被拎起来要立刻跟手；第一次也不用过渡
            self.live.pet_from = (self.live.pet_key.is_some() && !self.dragging_cls).then_some((self.live.pet_last, now));
            self.live.pet_key = key;
        }
        let out = match self.live.pet_from {
            Some((from, t0)) if now < t0 + PET_BLEND_MS => {
                let t = (now - t0) as f32 / PET_BLEND_MS as f32;
                lerp_xf(&from, &pet, 1.0 - (1.0 - t) * (1.0 - t))
            }
            _ => {
                self.live.pet_from = None;
                pet
            }
        };
        self.live.pet_last = out;
        out
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
        // 帧间隔常量归帧率那边管，这里只沿用；扭曲深度渐出的那几百毫秒也算活动帧
        if self.live_mode().is_some() || self.live.depth > 0.001 || self.live.mouth.value > 0.0
            || (self.speaking() && live::available(self.face, &self.costume, Kind::Mouth))
        {
            ms = ms.min(crate::core::config::frame_ms());
        }
        // 交叉淡化 / 整体动画过渡进行中
        if self.live.fade.is_some_and(|(_, t)| now < t + XFADE_MS) || self.live.pet_from.is_some_and(|(_, t)| now < t + PET_BLEND_MS) {
            ms = ms.min(live::BLINK_FRAME_MS);
        }
        ms
    }

    /// 灵动扭曲开着时，老的整体缩放式呼吸（CSS breathe）就不要了
    pub(super) fn live_replaces_breathe(&self) -> bool { self.live_mode().is_some() }
}

/// 向目标值线性渐变，DEPTH_RAMP_MS 走完 0→1
fn ramp(cur: f32, target: f32, dt: f32) -> f32 {
    let step = dt / DEPTH_RAMP_MS;
    if cur < target { (cur + step).min(target) } else { (cur - step * 1.7).max(target) }
}

/// 交叉淡化：旧表情的不透明度（开始后 t 毫秒），先快后慢
fn xfade_alpha(t: u64) -> f32 {
    if t >= XFADE_MS { return 0.0; }
    let x = t as f32 / XFADE_MS as f32;
    (1.0 - x) * (1.0 - x)
}

/// 走路动画开始后 e 毫秒（一个动画周期 dur = 两步）：(该显示第几帧, 下一次换帧的时刻)。
/// 帧序 walk1, idle, walk2, idle，以 walk1 落地（周期 0）为中心各占 1/8 周期两侧
fn walk_frame_at(e: f32, dur: f32) -> (usize, f32) {
    let q = dur / 4.0;
    let k = ((e + q / 2.0) / q).floor();
    ((k as i64).rem_euclid(4) as usize, (k + 1.0) * q - q / 2.0)
}

fn lerp_xf(a: &Xf, b: &Xf, t: f32) -> Xf {
    let l = |x: f32, y: f32| x + (y - x) * t;
    Xf { tx: l(a.tx, b.tx), ty: l(a.ty, b.ty), rot: l(a.rot, b.rot), sx: l(a.sx, b.sx), sy: l(a.sy, b.sy), opacity: l(a.opacity, b.opacity), bright: l(a.bright, b.bright) }
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
        // 强度渐变上去（不再一帧跳到位）
        tick(&mut b, 3100);
        let d0 = b.scene.live.depth;
        for i in 1..=30 { tick(&mut b, 3100 + i * 33); }
        assert!(b.scene.live.sleep && b.scene.live.depth > 1.0 && b.scene.live.depth > d0);
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
        // 默认水手服还没有张嘴贴片，用原装卫衣测
        b.costume = "hoodie".into();
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
    #[test]
    fn walk_frames_follow_the_bob() {
        // 500ms 一个周期两步：0/250ms 脚落地（迈腿），125/375ms 最高点（收腿）
        assert_eq!(walk_frame_at(0.0, 500.0).0, 0);
        assert_eq!(walk_frame_at(125.0, 500.0).0, 1);
        assert_eq!(walk_frame_at(250.0, 500.0).0, 2);
        assert_eq!(walk_frame_at(375.0, 500.0).0, 3);
        assert_eq!(walk_frame_at(500.0, 500.0).0, 0);
        // 下一次换帧在半格处
        assert_eq!(walk_frame_at(0.0, 500.0).1, 62.5);
        assert_eq!(walk_frame_at(100.0, 500.0).1, 187.5);
        // 和整体动画 anim::walk 对齐：迈腿帧时颠到最低（ty≈0），收腿帧时在最高点附近
        for e in (0..1000).step_by(10) {
            let p = (e as f32 / 500.0).fract();
            let ty = crate::render::anim::walk(p).ty;
            match walk_frame_at(e as f32, 500.0).0 {
                0 | 2 => assert!(ty > -3.6, "e={e} ty={ty}"),
                _ => assert!(ty < -3.4, "e={e} ty={ty}"),
            }
        }
    }

    #[test]
    fn walk_catches_up_without_storm() {
        let mut b = mk();
        tick(&mut b, 3000);
        b.go_walk();
        tick(&mut b, 3001);
        // 一下子过去 10 秒：直接跳到当前帧，不逐帧补（计时器循环只有 64 次）
        tick(&mut b, 13_000);
        assert!(b.next_walk_frame > 13_000 && b.next_walk_frame <= 13_000 + 125);
    }

    #[test]
    fn expression_change_crossfades() {
        let mut b = mk();
        let mut t = 3000;
        tick(&mut b, t);
        b.state_timer = None;
        b.set_state(St::Think, None);
        tick(&mut b, t + 10);
        assert_eq!(b.scene.live.prev_face, Some(Face::Idle));
        assert!(b.scene.live.prev_alpha > 0.8);
        // 淡化期间高帧率
        assert!(b.live_interval() <= live::BLINK_FRAME_MS);
        t += 10 + XFADE_MS + 5;
        tick(&mut b, t);
        assert_eq!(b.scene.live.prev_face, None);
        // 被拎起来：不淡（立刻换）
        b.set_state(St::Drag, None);
        tick(&mut b, t + 10);
        assert_eq!(b.scene.live.prev_face, None);
        // 换衣服：不淡
        b.set_state(St::Idle, None);
        tick(&mut b, t + 400);
        b.costume = "maid".into();
        tick(&mut b, t + 410);
        assert_eq!(b.scene.live.prev_face, None);
        assert!(xfade_alpha(0) == 1.0 && xfade_alpha(XFADE_MS) == 0.0 && xfade_alpha(XFADE_MS / 2) < 0.5);
    }

    #[test]
    fn pet_animation_switch_is_blended() {
        let mut b = mk();
        tick(&mut b, 3000);
        b.state_timer = None;
        let jump = Xf { ty: -20.0, rot: 6.0, ..Xf::ID };
        // 模拟：上一帧在某个姿态，然后整体动画换了
        b.live.pet_key = Some((PetAnim::Wiggle, 0));
        b.live.pet_last = jump;
        b.pet_anim = (PetAnim::Breathe, 3000);
        b.now = 3000;
        let o = b.blend_pet(Xf::ID);
        assert!((o.ty - jump.ty).abs() < 1e-3, "starts from last pose");
        b.now = 3090;
        let m = b.blend_pet(Xf::ID);
        assert!(m.ty < 0.0 && m.ty > jump.ty && m.rot > 0.0 && m.rot < 6.0);
        b.now = 3000 + PET_BLEND_MS + 1;
        assert_eq!(b.blend_pet(Xf::ID), Xf::ID);
    }

    #[test]
    fn depth_ramps_and_low_power_off() {
        assert!(ramp(0.0, 1.0, 100.0) > 0.2 && ramp(0.0, 1.0, 100.0) < 0.3);
        assert_eq!(ramp(0.9, 1.0, 100.0), 1.0);
        assert!(ramp(1.0, 0.0, 100.0) < 0.6);
        let mut b = mk();
        b.costume = "hoodie".into();
        tick(&mut b, 3000);
        b.state_timer = None;
        b.voice_playing = Some(1);
        b.fake_level = 0.2;
        tick(&mut b, 3033);
        assert_eq!(b.live_interval(), live::LIVE_FRAME_MS.min(b.live_interval()));
        b.live.force_low_power = true;
        tick(&mut b, 3066);
        // 省电：不扭曲
        assert!(!b.scene.live.warping());
    }
}
