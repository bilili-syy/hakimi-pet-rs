//! style.css 的 @keyframes 移植：全部是「进度 → 变换」的纯函数。
//! CSS 对每个属性单独取关键帧（某帧没写的属性不算该属性的关键帧），缓动函数作用在每一段上。

/// 一组 CSS 变换 + 透明度 + brightness 滤镜。顺序同 CSS：translate → rotate → scale
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xf {
    pub tx: f32,
    pub ty: f32,
    /// 角度（度），顺时针为正（和 CSS 一致）
    pub rot: f32,
    pub sx: f32,
    pub sy: f32,
    pub opacity: f32,
    pub bright: f32,
}

impl Xf {
    pub const ID: Xf = Xf { tx: 0.0, ty: 0.0, rot: 0.0, sx: 1.0, sy: 1.0, opacity: 1.0, bright: 1.0 };
}

impl Default for Xf {
    fn default() -> Self { Xf::ID }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code, clippy::enum_variant_names)]
pub enum Ease { Linear, Ease, EaseIn, EaseOut, EaseInOut }

/// cubic-bezier(x1, y1, x2, y2) 在 x=t 处的 y
fn bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let bx = |s: f32| 3.0 * (1.0 - s) * (1.0 - s) * s * x1 + 3.0 * (1.0 - s) * s * s * x2 + s * s * s;
    let by = |s: f32| 3.0 * (1.0 - s) * (1.0 - s) * s * y1 + 3.0 * (1.0 - s) * s * s * y2 + s * s * s;
    // x(s) 单调递增，二分即可
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..24 {
        let mid = (lo + hi) * 0.5;
        if bx(mid) < t { lo = mid } else { hi = mid }
    }
    by((lo + hi) * 0.5)
}

pub fn ease(e: Ease, t: f32) -> f32 {
    if !t.is_finite() { return 0.0; }
    match e {
        Ease::Linear => t.clamp(0.0, 1.0),
        Ease::Ease => bezier(0.25, 0.1, 0.25, 1.0, t),
        Ease::EaseIn => bezier(0.42, 0.0, 1.0, 1.0, t),
        Ease::EaseOut => bezier(0.0, 0.0, 0.58, 1.0, t),
        Ease::EaseInOut => bezier(0.42, 0.0, 0.58, 1.0, t),
    }
}

/// 单个属性的关键帧插值。keys 按 offset 升序，offset ∈ [0,1]
pub fn kf(p: f32, keys: &[(f32, f32)], e: Ease) -> f32 {
    if keys.is_empty() { return 0.0; }
    if p <= keys[0].0 { return keys[0].1; }
    for w in keys.windows(2) {
        let (a, va) = w[0];
        let (b, vb) = w[1];
        if p <= b {
            let local = if b > a { (p - a) / (b - a) } else { 1.0 };
            return va + (vb - va) * ease(e, local);
        }
    }
    keys[keys.len() - 1].1
}

/// 动画迭代进度：elapsed 毫秒、单次时长、次数（None = infinite）。播完返回 None
pub fn cycle(elapsed_ms: f32, dur_ms: f32, iterations: Option<u32>) -> Option<f32> {
    if !dur_ms.is_finite() || dur_ms <= 0.0 || !elapsed_ms.is_finite() { return None; }
    let e = elapsed_ms.max(0.0);
    if let Some(n) = iterations && e >= dur_ms * n as f32 { return None; }
    Some((e % dur_ms) / dur_ms)
}

const IO: Ease = Ease::EaseInOut;

pub fn breathe(p: f32) -> Xf {
    Xf { sx: kf(p, &[(0.0, 1.0), (0.5, 1.02), (1.0, 1.0)], IO), sy: kf(p, &[(0.0, 1.0), (0.5, 0.98), (1.0, 1.0)], IO), ..Xf::ID }
}
pub fn breathe_slow(p: f32) -> Xf {
    Xf { sx: kf(p, &[(0.0, 1.0), (0.5, 1.03), (1.0, 1.0)], IO), sy: kf(p, &[(0.0, 1.0), (0.5, 0.97), (1.0, 1.0)], IO), ..Xf::ID }
}
pub fn wiggle(p: f32) -> Xf {
    Xf { rot: kf(p, &[(0.0, -6.0), (0.5, 6.0), (1.0, -6.0)], IO), ..Xf::ID }
}
pub fn bounce(p: f32) -> Xf {
    let e = Ease::EaseOut;
    Xf {
        ty: kf(p, &[(0.0, 0.0), (0.4, -18.0), (1.0, 0.0)], e),
        sx: kf(p, &[(0.0, 1.0), (0.4, 0.97), (1.0, 1.0)], e),
        sy: kf(p, &[(0.0, 1.0), (0.4, 1.04), (1.0, 1.0)], e),
        ..Xf::ID
    }
}
pub fn squash(p: f32) -> Xf {
    let e = Ease::EaseOut;
    Xf { sx: kf(p, &[(0.0, 1.15), (0.6, 0.95), (1.0, 1.0)], e), sy: kf(p, &[(0.0, 0.8), (0.6, 1.06), (1.0, 1.0)], e), ..Xf::ID }
}
pub fn shake(p: f32) -> Xf {
    Xf { tx: kf(p, &[(0.0, 0.0), (0.2, -6.0), (0.4, 6.0), (0.6, -6.0), (0.8, 6.0), (1.0, 0.0)], IO), ..Xf::ID }
}
pub fn walk(p: f32) -> Xf {
    Xf {
        ty: kf(p, &[(0.0, 0.0), (0.25, -7.0), (0.5, 0.0), (0.75, -7.0), (1.0, 0.0)], IO),
        rot: kf(p, &[(0.0, -3.0), (0.25, 0.0), (0.5, 3.0), (0.75, 0.0), (1.0, -3.0)], IO),
        ..Xf::ID
    }
}
/// 小游戏里扑逗猫棒：蹲一下再高高跳起
pub fn jump(p: f32) -> Xf {
    let e = Ease::EaseOut;
    Xf {
        ty: kf(p, &[(0.0, 0.0), (0.15, 4.0), (0.5, -46.0), (1.0, 0.0)], e),
        sx: kf(p, &[(0.0, 1.0), (0.15, 1.08), (0.5, 0.95), (1.0, 1.0)], e),
        sy: kf(p, &[(0.0, 1.0), (0.15, 0.9), (0.5, 1.06), (1.0, 1.0)], e),
        ..Xf::ID
    }
}
pub fn chase(p: f32) -> Xf {
    Xf {
        ty: kf(p, &[(0.0, 0.0), (0.25, -14.0), (0.5, 0.0), (0.75, -14.0), (1.0, 0.0)], IO),
        sx: kf(p, &[(0.0, 1.0), (0.25, 0.2), (0.5, -1.0), (0.75, -0.2), (1.0, 1.0)], IO),
        ..Xf::ID
    }
}

// ---- 尾巴 / 星星图层
pub fn tail_sway(p: f32) -> Xf { Xf { rot: kf(p, &[(0.0, -5.0), (0.5, 6.0), (1.0, -5.0)], IO), ..Xf::ID } }
pub fn tail_wag(p: f32) -> Xf { Xf { rot: kf(p, &[(0.0, -7.0), (0.5, 9.0), (1.0, -7.0)], IO), ..Xf::ID } }
pub fn tail_droop(p: f32) -> Xf { Xf { rot: kf(p, &[(0.0, 10.0), (0.5, 13.0), (1.0, 10.0)], IO), ..Xf::ID } }
pub fn star_bob(p: f32) -> Xf {
    let s = kf(p, &[(0.0, 1.0), (0.5, 1.05), (1.0, 1.0)], IO);
    Xf { rot: kf(p, &[(0.0, -5.0), (0.5, 5.0), (1.0, -5.0)], IO), sx: s, sy: s, bright: kf(p, &[(0.0, 1.0), (0.5, 1.15), (1.0, 1.0)], IO), ..Xf::ID }
}
pub fn star_spin(p: f32) -> Xf {
    let s = kf(p, &[(0.0, 1.05), (0.5, 1.12), (1.0, 1.05)], IO);
    Xf { rot: kf(p, &[(0.0, -9.0), (0.5, 9.0), (1.0, -9.0)], IO), sx: s, sy: s, bright: kf(p, &[(0.0, 1.0), (0.5, 1.25), (1.0, 1.0)], IO), ..Xf::ID }
}
pub fn star_droop(p: f32) -> Xf { Xf { rot: kf(p, &[(0.0, 14.0), (0.5, 18.0), (1.0, 14.0)], IO), ..Xf::ID } }

// ---- zzZ
pub fn zfloat(p: f32) -> Xf {
    let e = Ease::EaseOut;
    Xf {
        opacity: kf(p, &[(0.0, 0.0), (0.2, 1.0), (1.0, 0.0)], e),
        tx: kf(p, &[(0.0, 0.0), (1.0, 28.0)], e),
        ty: kf(p, &[(0.0, 0.0), (1.0, -60.0)], e),
        ..Xf::ID
    }
}

// ---- 特效
pub fn fx_rise(p: f32, dx: f32) -> Xf {
    let e = Ease::EaseOut;
    let s = kf(p, &[(0.0, 0.6), (0.15, 1.1), (1.0, 0.9)], e);
    Xf {
        opacity: kf(p, &[(0.0, 0.0), (0.15, 1.0), (1.0, 0.0)], e),
        tx: kf(p, &[(0.0, 0.0), (0.15, 0.0), (1.0, dx)], e),
        ty: kf(p, &[(0.0, 0.0), (0.15, -8.0), (1.0, -70.0)], e),
        sx: s,
        sy: s,
        ..Xf::ID
    }
}
pub fn fx_burst(p: f32, dx: f32, dy: f32) -> Xf {
    let e = Ease::EaseOut;
    let s = kf(p, &[(0.0, 0.4), (1.0, 1.2)], e);
    Xf {
        opacity: kf(p, &[(0.0, 1.0), (1.0, 0.0)], e),
        tx: kf(p, &[(0.0, 0.0), (1.0, dx)], e),
        ty: kf(p, &[(0.0, 0.0), (1.0, dy)], e),
        rot: kf(p, &[(0.0, 0.0), (1.0, 90.0)], e),
        sx: s,
        sy: s,
        ..Xf::ID
    }
}
pub fn fx_pop(p: f32) -> Xf {
    let e = Ease::EaseOut;
    let s = kf(p, &[(0.0, 0.2), (0.2, 1.3), (0.35, 0.95), (0.5, 1.1), (1.0, 1.0)], e);
    Xf { opacity: kf(p, &[(0.0, 0.0), (0.2, 1.0), (0.8, 1.0), (1.0, 0.0)], e), sx: s, sy: s, ..Xf::ID }
}
pub fn fx_fall(p: f32) -> Xf {
    let e = Ease::EaseIn;
    Xf { opacity: kf(p, &[(0.0, 0.0), (0.2, 1.0), (1.0, 0.0)], e), ty: kf(p, &[(0.0, 0.0), (1.0, 50.0)], e), ..Xf::ID }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool { (a - b).abs() < 1e-3 }

    #[test]
    fn ease_endpoints() {
        for e in [Ease::Linear, Ease::Ease, Ease::EaseIn, Ease::EaseOut, Ease::EaseInOut] {
            assert!(close(ease(e, 0.0), 0.0));
            assert!(close(ease(e, 1.0), 1.0));
        }
        assert!(close(ease(Ease::EaseInOut, 0.5), 0.5));
        assert!(ease(Ease::EaseIn, 0.3) < 0.3);
        assert!(ease(Ease::EaseOut, 0.3) > 0.3);
    }

    #[test]
    fn keyframe_interp() {
        let k = [(0.0, 0.0), (0.5, 10.0), (1.0, 0.0)];
        assert!(close(kf(0.0, &k, Ease::Linear), 0.0));
        assert!(close(kf(0.25, &k, Ease::Linear), 5.0));
        assert!(close(kf(0.5, &k, Ease::Linear), 10.0));
        assert!(close(kf(0.75, &k, Ease::Linear), 5.0));
        assert!(close(kf(1.0, &k, Ease::Linear), 0.0));
        // 缓动按段计算：每段中点 ease-in-out 正好一半
        assert!(close(kf(0.25, &k, Ease::EaseInOut), 5.0));
        // 超出范围夹紧
        assert!(close(kf(-1.0, &k, Ease::Linear), 0.0));
        assert!(close(kf(2.0, &k, Ease::Linear), 0.0));
    }

    #[test]
    fn cycles() {
        assert_eq!(cycle(0.0, 400.0, Some(1)), Some(0.0));
        assert!(close(cycle(600.0, 400.0, None).unwrap(), 0.5));
        assert_eq!(cycle(400.0, 400.0, Some(1)), None);
        assert!(cycle(999.0, 250.0, Some(4)).is_some());
        assert_eq!(cycle(1000.0, 250.0, Some(4)), None);
    }

    #[test]
    fn keyframes_match_css() {
        assert!(close(bounce(0.4).ty, -18.0));
        assert!(close(bounce(0.4).sy, 1.04));
        assert!(close(squash(0.0).sx, 1.15));
        assert!(close(shake(0.2).tx, -6.0));
        assert!(close(shake(0.4).tx, 6.0));
        assert!(close(walk(0.25).ty, -7.0));
        assert!(close(chase(0.5).sx, -1.0));
        assert!(close(breathe(0.5).sy, 0.98));
        assert!(close(star_spin(0.5).bright, 1.25));
        assert!(close(zfloat(0.2).opacity, 1.0));
        assert!(close(zfloat(1.0).ty, -60.0));
        assert!(close(fx_burst(1.0, 70.0, -60.0).rot, 90.0));
        assert!(close(fx_rise(0.15, 20.0).ty, -8.0));
        assert!(close(fx_rise(1.0, 20.0).tx, 20.0));
    }
}
