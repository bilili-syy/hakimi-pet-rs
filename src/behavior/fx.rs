//! 特效粒子（renderer.js 的 fx()）：生成参数 + 按时间算出每帧的 FxSprite
use crate::render::{FxShape, FxSprite, Xf, anim};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FxKind { Heart, Note, Spark, Sweat, Anger, Tear, Honey, Dizzy, Exclaim }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Motion { Rise, Burst, Pop, Fall }

#[derive(Clone, Debug)]
struct Particle {
    shape: FxShape,
    color: u32,
    x: f32,
    y: f32,
    dx: f32,
    dy: f32,
    size: f32,
    motion: Motion,
    /// 动画开始（已含 animation-delay）
    start: u64,
    remove_at: u64,
}

#[derive(Clone, Debug)]
struct Orbit {
    start: u64,
    remove_at: u64,
}

const FX_COLORS: [u32; 4] = [0x4285f4, 0xea4335, 0xfbbc04, 0x34a853];
/// 💧 / 💢 原版是彩色 emoji，文字颜色不起作用
const DROP_BLUE: u32 = 0x4aa8ee;
const ANGER_RED: u32 = 0xe53935;

fn rand(a: f32, b: f32) -> f32 { a + fastrand::f32() * (b - a) }

#[derive(Default)]
pub struct Fx {
    parts: Vec<Particle>,
    orbits: Vec<Orbit>,
}

impl Fx {
    pub fn spawn(&mut self, kind: FxKind, n: usize, now: u64) {
        match kind {
            FxKind::Dizzy => {
                self.orbits.push(Orbit { start: now, remove_at: now + 3200 });
                return;
            }
            FxKind::Exclaim => {
                self.parts.push(Particle {
                    shape: FxShape::Exclaim, color: 0xea4335, x: 128.0, y: 0.0, dx: 0.0, dy: 0.0, size: 16.0,
                    motion: Motion::Pop, start: now, remove_at: now + 900,
                });
                return;
            }
            _ => {}
        }
        for i in 0..n {
            let shape = match kind {
                FxKind::Heart => FxShape::Heart,
                FxKind::Note => FxShape::Note((i % 3) as u8),
                FxKind::Sweat | FxKind::Tear => FxShape::Drop,
                FxKind::Anger => FxShape::Anger,
                _ => FxShape::Spark,
            };
            let motion = match kind {
                FxKind::Spark => Motion::Burst,
                FxKind::Tear => Motion::Fall,
                FxKind::Anger | FxKind::Sweat => Motion::Pop,
                _ => Motion::Rise,
            };
            let color = match kind {
                FxKind::Heart => if i % 2 == 1 { 0xea4335 } else { 0xff7eb6 },
                FxKind::Honey => 0xfbbc04,
                FxKind::Sweat | FxKind::Tear => DROP_BLUE,
                FxKind::Anger => ANGER_RED,
                _ => FX_COLORS[i % 4],
            };
            let (mut x, mut y) = (rand(55.0, 145.0), rand(30.0, 80.0));
            let (mut dx, mut dy) = (rand(-25.0, 25.0), 0.0);
            match kind {
                FxKind::Anger => (x, y) = (140.0, 18.0),
                FxKind::Sweat => (x, y) = (150.0, 40.0),
                FxKind::Tear => (x, y) = (if i % 2 == 1 { 72.0 } else { 128.0 }, 100.0),
                FxKind::Spark => {
                    (x, y) = (96.0, 50.0);
                    let a = i as f32 / n as f32 * std::f32::consts::TAU;
                    (dx, dy) = (a.cos() * 70.0, a.sin() * 60.0);
                }
                _ => {}
            }
            let size = match kind {
                FxKind::Heart | FxKind::Note => rand(14.0, 22.0),
                FxKind::Anger => 26.0,
                _ => 16.0,
            };
            let delay = if matches!(kind, FxKind::Anger | FxKind::Sweat) { 0 } else { i as u64 * 120 };
            self.parts.push(Particle { shape, color, x, y, dx, dy, size, motion, start: now + delay, remove_at: now + 2200 + i as u64 * 120 });
        }
    }

    pub fn is_empty(&self) -> bool { self.parts.is_empty() && self.orbits.is_empty() }

    #[cfg(test)]
    pub fn clear(&mut self) { self.parts.clear(); self.orbits.clear(); }

    /// 去掉过期的，返回这一帧要画的
    pub fn sprites(&mut self, now: u64) -> Vec<FxSprite> {
        self.parts.retain(|p| now < p.remove_at);
        self.orbits.retain(|o| now < o.remove_at);
        let mut out = Vec::new();
        for p in &self.parts {
            if now < p.start { continue; } // animation-delay 期间 .fx 是 opacity:0
            let t = (now - p.start) as f32;
            let (dur, xf) = match p.motion {
                Motion::Rise => (1800.0, anim::fx_rise((t / 1800.0).min(1.0), p.dx)),
                Motion::Burst => (900.0, anim::fx_burst((t / 900.0).min(1.0), p.dx, p.dy)),
                Motion::Pop => (1600.0, anim::fx_pop((t / 1600.0).min(1.0))),
                Motion::Fall => (1200.0, anim::fx_fall((t / 1200.0).min(1.0))),
            };
            if t >= dur { continue; } // forwards 停在最后一帧（全透明）
            out.push(FxSprite { shape: p.shape, x: p.x, y: p.y, size: p.size, xf, color: p.color });
        }
        for o in &self.orbits {
            for (i, color) in FX_COLORS[..3].iter().enumerate() {
                // rotate(θ) translateX(44px) rotate(-θ)：绕自身中心转圈，字形本身不转
                let t = (now - o.start + i as u64 * 330) % 1000;
                let a = t as f32 / 1000.0 * std::f32::consts::TAU;
                let xf = Xf { tx: a.cos() * 44.0, ty: a.sin() * 44.0, ..Xf::ID };
                out.push(FxSprite { shape: FxShape::Spark, x: 100.0, y: 42.0, size: 16.0, xf, color: *color });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn particles_expire() {
        let mut f = Fx::default();
        f.spawn(FxKind::Heart, 5, 0);
        f.spawn(FxKind::Dizzy, 1, 0);
        assert!(!f.is_empty());
        // 第 0 个立刻出现，后面的有延迟
        let s = f.sprites(10);
        assert_eq!(s.iter().filter(|s| s.shape == FxShape::Heart).count(), 1);
        assert_eq!(s.iter().filter(|s| s.shape == FxShape::Spark).count(), 3);
        assert!(f.sprites(1000).len() >= 5);
        f.sprites(5000);
        assert!(f.is_empty());
    }

    #[test]
    fn spark_bursts_in_a_ring() {
        let mut f = Fx::default();
        f.spawn(FxKind::Spark, 4, 0);
        let s = f.sprites(400); // 4 个都已开始
        assert_eq!(s.len(), 4);
        assert!(s.iter().all(|s| s.x == 96.0 && s.y == 50.0));
    }
}
