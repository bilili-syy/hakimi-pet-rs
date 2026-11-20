//! 跳舞的关键帧（纯函数）：动作 × 进度 → 立绘 / 尾巴 / 星星的变换。
//! 行为层的舞谱（behavior/dance.rs）只说「第几拍做什么动作」，具体怎么动全在这里。
//! 每个动作在拍与拍的交界处首尾相接（奇偶拍左右镜像），换动作时由行为层做短暂混合。
use super::anim::{Ease, Xf, kf};
use std::f32::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    /// 左右摇摆：歪身子 + 轻微下沉（小猫摇摆主步）
    Sway,
    /// 原地蹦：每拍一跳，落地压扁
    Bounce,
    /// 转圈：scaleX 1 → -1 → 1 假装转身，顺带小跳（整个动作段转一圈）
    Spin,
    /// 下蹲：压扁再弹起
    Squat,
    /// 侧滑步：左右平移
    Slide,
    /// 扭扭：快速小角度抖动
    Shimmy,
    /// 定格：歪头放大，每拍轻轻一顿
    Pose,
    /// 蹦迪大跳：高跳 + 左右倾斜
    Jump,
}

/// 一帧舞姿（Xf 原点同 Scene：pet 绕盒子底边中点，tail/star 绕各自挂点）
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub pet: Xf,
    pub tail: Xf,
    pub star: Xf,
}

impl Pose {
    pub const ID: Pose = Pose { pet: Xf::ID, tail: Xf::ID, star: Xf::ID };
}

const IO: Ease = Ease::EaseInOut;
const OUT: Ease = Ease::EaseOut;

/// 线性混合两个变换（换动作时过渡用）
pub fn lerp_xf(a: &Xf, b: &Xf, t: f32) -> Xf {
    let t = t.clamp(0.0, 1.0);
    let l = |x: f32, y: f32| x + (y - x) * t;
    Xf {
        tx: l(a.tx, b.tx),
        ty: l(a.ty, b.ty),
        rot: l(a.rot, b.rot),
        sx: l(a.sx, b.sx),
        sy: l(a.sy, b.sy),
        opacity: l(a.opacity, b.opacity),
        bright: l(a.bright, b.bright),
    }
}

pub fn lerp_pose(a: &Pose, b: &Pose, t: f32) -> Pose {
    Pose { pet: lerp_xf(&a.pet, &b.pet, t), tail: lerp_xf(&a.tail, &b.tail, t), star: lerp_xf(&a.star, &b.star, t) }
}

/// 舞姿。step_p：在当前动作段里的进度 0..1；beat_p：拍内进度 0..1；
/// odd：奇数拍（左右交替）；amp：力度（1 = 标准，跟着音量包络 0.7~1.25 变化）
pub fn pose(m: Move, step_p: f32, beat_p: f32, odd: bool, amp: f32) -> Pose {
    let side = if odd { -1.0 } else { 1.0 };
    let b = beat_p.clamp(0.0, 1.0);
    let pet = match m {
        Move::Sway => {
            let rot = side * kf(b, &[(0.0, -8.0), (1.0, 8.0)], IO) * amp;
            Xf { rot, tx: rot * 0.6, ty: kf(b, &[(0.0, 0.0), (0.5, -4.0 * amp), (1.0, 0.0)], IO), ..Xf::ID }
        }
        Move::Bounce => {
            let sy = kf(b, &[(0.0, 0.9), (0.2, 1.06), (0.4, 1.0), (0.7, 1.0), (0.85, 0.92), (1.0, 0.9)], OUT);
            Xf { ty: kf(b, &[(0.0, 0.0), (0.4, -16.0 * amp), (0.8, 0.0), (1.0, 0.0)], OUT), sx: 2.0 - sy, sy, ..Xf::ID }
        }
        Move::Spin => {
            let s = step_p.clamp(0.0, 1.0);
            Xf {
                sx: (2.0 * PI * s).cos(),
                ty: -12.0 * amp * (PI * s).sin(),
                rot: 4.0 * (2.0 * PI * s).sin(),
                ..Xf::ID
            }
        }
        Move::Squat => {
            let sy = kf(b, &[(0.0, 1.0), (0.5, 1.0 - 0.15 * amp), (1.0, 1.0)], IO);
            Xf { sx: 2.0 - sy, sy, ..Xf::ID }
        }
        Move::Slide => {
            let tx = side * kf(b, &[(0.0, -14.0), (1.0, 14.0)], IO) * amp;
            Xf { tx, rot: tx * 0.3, ty: kf(b, &[(0.0, 0.0), (0.5, -5.0), (1.0, 0.0)], IO), ..Xf::ID }
        }
        Move::Shimmy => {
            let rot = 5.0 * amp * (b * 4.0 * PI).sin();
            Xf { rot, ty: 3.0 * (b * 2.0 * PI).sin().abs(), sx: 1.02, sy: 0.97, ..Xf::ID }
        }
        Move::Pose => {
            let k = (1.0 - b) * (1.0 - b);
            let s = 1.04 + 0.03 * k * amp;
            Xf { rot: -6.0, sx: s, sy: s, ..Xf::ID }
        }
        Move::Jump => {
            let h = (PI * b).sin();
            let land = kf(b, &[(0.0, 0.86), (0.15, 1.06), (0.3, 1.0), (0.85, 1.0), (1.0, 0.86)], OUT);
            Xf { ty: -26.0 * amp * h, rot: side * 10.0 * h * amp, sx: 2.0 - land, sy: land, ..Xf::ID }
        }
    };
    // 尾巴每拍甩一下，星星每拍亮一下（踩点）
    let tail_amp = if matches!(m, Move::Jump | Move::Shimmy) { 1.4 } else { 1.0 };
    let tail = Xf { rot: kf(b, &[(0.0, -10.0), (0.5, 12.0), (1.0, -10.0)], IO) * tail_amp * amp.min(1.2), ..Xf::ID };
    let k = (1.0 - b) * (1.0 - b);
    let s = 1.0 + 0.18 * k * amp;
    let star = Xf { rot: side * 8.0 * (PI * b).sin(), sx: s, sy: s, bright: 1.0 + 0.45 * k, ..Xf::ID };
    Pose { pet, tail, star }
}

/// 朝向：1 正常、-1 镜像。alt = 每拍翻一次（拍头 0.15 拍内过渡，和原版 200ms 翻转差不多）
pub fn facing(alt: bool, fixed: f32, beat_index: i64, beat_p: f32) -> f32 {
    if !alt { return fixed; }
    let to = if beat_index.rem_euclid(2) == 0 { 1.0 } else { -1.0 };
    let p = (beat_p / 0.15).clamp(0.0, 1.0);
    -to + (to - -to) * super::anim::ease(Ease::Ease, p)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOVES: [Move; 8] = [Move::Sway, Move::Bounce, Move::Spin, Move::Squat, Move::Slide, Move::Shimmy, Move::Pose, Move::Jump];

    fn close(a: f32, b: f32, eps: f32) -> bool { (a - b).abs() < eps }

    #[test]
    fn beats_join_seamlessly() {
        // 拍尾（偶数拍）和下一拍拍头（奇数拍）姿态一致 → 不跳帧（Spin 看段进度，单独测）
        for m in MOVES {
            if m == Move::Spin || m == Move::Pose { continue; }
            for odd in [false, true] {
                let a = pose(m, 0.5, 1.0, odd, 1.0).pet;
                let b = pose(m, 0.5, 0.0, !odd, 1.0).pet;
                assert!(close(a.rot, b.rot, 1e-3) && close(a.ty, b.ty, 1e-3) && close(a.tx, b.tx, 1e-3), "{m:?} {a:?} {b:?}");
                assert!(close(a.sy, b.sy, 1e-3), "{m:?} sy");
                let (ta, tb) = (pose(m, 0.5, 1.0, odd, 1.0).tail, pose(m, 0.5, 0.0, !odd, 1.0).tail);
                assert!(close(ta.rot, tb.rot, 1e-3), "{m:?} tail");
            }
        }
    }

    #[test]
    fn spin_turns_around_once() {
        assert!(close(pose(Move::Spin, 0.0, 0.0, false, 1.0).pet.sx, 1.0, 1e-4));
        assert!(close(pose(Move::Spin, 0.5, 0.0, false, 1.0).pet.sx, -1.0, 1e-4));
        assert!(close(pose(Move::Spin, 1.0, 0.0, false, 1.0).pet.sx, 1.0, 1e-4));
        assert!(pose(Move::Spin, 0.5, 0.0, false, 1.0).pet.ty < -11.0);
    }

    #[test]
    fn keyframes_hit_their_marks() {
        assert!(close(pose(Move::Bounce, 0.0, 0.4, false, 1.0).pet.ty, -16.0, 1e-3));
        assert!(close(pose(Move::Bounce, 0.0, 0.4, false, 0.5).pet.ty, -8.0, 1e-3));
        assert!(close(pose(Move::Sway, 0.0, 1.0, false, 1.0).pet.rot, 8.0, 1e-3));
        assert!(close(pose(Move::Sway, 0.0, 1.0, true, 1.0).pet.rot, -8.0, 1e-3));
        assert!(close(pose(Move::Squat, 0.0, 0.5, false, 1.0).pet.sy, 0.85, 1e-3));
        assert!(close(pose(Move::Jump, 0.0, 0.5, false, 1.0).pet.ty, -26.0, 1e-3));
        // 星星拍头最亮，拍尾回落
        let s0 = pose(Move::Sway, 0.0, 0.0, false, 1.0).star;
        let s1 = pose(Move::Sway, 0.0, 0.99, false, 1.0).star;
        assert!(s0.bright > 1.4 && s1.bright < 1.01 && s0.sx > s1.sx);
    }

    #[test]
    fn facing_alternates_per_beat() {
        assert_eq!(facing(false, -1.0, 3, 0.5), -1.0);
        assert!(close(facing(true, 1.0, 0, 1.0), 1.0, 1e-4));
        assert!(close(facing(true, 1.0, 1, 1.0), -1.0, 1e-4));
        assert!(close(facing(true, 1.0, 1, 0.0), 1.0, 1e-4)); // 拍头从上一拍的朝向开始翻
        assert!(close(facing(true, 1.0, -1, 0.5), -1.0, 1e-4));
    }

    #[test]
    fn lerp_blends() {
        let a = Xf { tx: 0.0, rot: -10.0, ..Xf::ID };
        let b = Xf { tx: 10.0, rot: 10.0, sx: 2.0, ..Xf::ID };
        let m = lerp_xf(&a, &b, 0.5);
        assert!(close(m.tx, 5.0, 1e-5) && close(m.rot, 0.0, 1e-5) && close(m.sx, 1.5, 1e-5));
        assert_eq!(lerp_xf(&a, &b, 2.0), b);
    }

    /// 手动看舞姿：HAKIMI_SNAP=dir cargo test dance_snapshot -- --ignored
    #[test]
    #[ignore]
    fn dance_snapshot() {
        use crate::render::{FxShape, FxSprite, Renderer, Scene};
        use crate::types::Face;
        let dir = std::env::var("HAKIMI_SNAP").unwrap_or_else(|_| ".".into());
        let mut r = Renderer::new();
        let shots = [(Move::Sway, 0.0, 1.0, Face::Dance, 1.0), (Move::Spin, 0.35, 0.3, Face::Love, 1.0), (Move::Jump, 0.0, 0.5, Face::Happy, -1.0), (Move::Squat, 0.0, 0.5, Face::Surprised, 1.0)];
        for (i, (m, sp, bp, face, flip)) in shots.iter().enumerate() {
            let p = pose(*m, *sp, *bp, false, 1.0);
            let sc = Scene {
                face: *face,
                pet: p.pet,
                tail: p.tail,
                star: p.star,
                flip: *flip,
                fx: vec![
                    FxSprite { shape: FxShape::Note(1), x: 40.0, y: 60.0, size: 20.0, xf: Xf::ID, color: 0x4285f4 },
                    FxSprite { shape: FxShape::Spark, x: 150.0, y: 120.0, size: 10.0, xf: Xf::ID, color: 0xff5ea8 },
                ],
                ..Scene::default()
            };
            r.draw(&sc, 1.0);
            r.pixmap().unwrap().save_png(format!("{dir}/dance{i}.png")).unwrap();
        }
    }
}
