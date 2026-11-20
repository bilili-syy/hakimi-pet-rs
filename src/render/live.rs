//! 「伪 Live2D」：几乎不占内存的灵动感。
//! - 呼吸 / 二次运动：把整张立绘按行（头发尖按列）做小幅网格扭曲——胸口起伏、头轻点、发梢和衣摆轻摆，脚底不动；
//! - 眨眼：眼部小贴片（闭眼）按不透明度交叉淡入淡出，每个表情一张，眼睛本来就闭着/差太多的表情不眨；
//! - 口型：放语音时按音量（services::audio::level 的滚动 RMS）张嘴，张嘴贴片同样只盖嘴部。
//!
//! 贴片约定（和皮肤包共用）：一张贴片 = 和立绘同画布尺寸的透明 PNG，只有眼/嘴那一小块不透明。
//! 皮肤可选提供 "blink" / "mouthOpen" 两种；内置默认皮肤的贴片在 assets/sprites/live/<表情>_blink.png、
//! <表情>_mouthOpen.png。渲染层只通过 [`overlays_for`] 取贴片（皮肤加载那一路以后在这里接皮肤文件）。
//! 解码后立刻缩放到当前 DPI 并裁成最小外接矩形，缓存几张小图，原图不留。
use crate::types::Face;
use std::rc::Rc;
use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};

/// 活动时的帧间隔（≈60fps；原来 60ms≈16fps，呼吸/发梢摆动看着一顿一顿的。实测 12 核机器上整机 CPU 约 0.13%）。待机呼吸、口型都按这个节奏，眨眼那一下用 BLINK_FRAME_MS
/// 运行时按 config::frame_ms()（设置里 30/60/120，默认 60）；这里是默认值，测试用
#[allow(dead_code)]
pub const LIVE_FRAME_MS: u32 = 16;
/// 眨眼进行中的帧间隔（一下眨眼 ≈ 190ms，要多几帧才顺）
pub const BLINK_FRAME_MS: u32 = 33;

// ---------------------------------------------------------------- 场景里的灵动参数
/// 行为层每帧填，渲染层照着画。全 0 = 静态立绘（和以前一模一样）
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct LiveView {
    /// 扭曲强度：0 关，1 待机，>1 睡觉时更深的呼吸
    pub depth: f32,
    /// 睡觉的姿势（蜷成一团）：呼吸作用到整团身体，头发不摆
    pub sleep: bool,
    /// 呼吸相位 0..1
    pub breath: f32,
    /// 头轻点相位 0..1（和呼吸错开，周期也不同）
    pub bob: f32,
    /// 发梢/衣摆摆动相位 0..1
    pub sway: f32,
    /// 闭眼程度 0..1（眨眼贴片的不透明度）
    pub blink: f32,
    /// 张嘴程度 0..1（张嘴贴片的不透明度）
    pub mouth: f32,
    /// 表情交叉淡化：上一个表情（同一套衣服）盖在上面，不透明度 prev_alpha 从 1 淡到 0
    pub prev_face: Option<Face>,
    pub prev_alpha: f32,
}

impl LiveView {
    pub fn warping(&self) -> bool { self.depth > 0.001 }
}

// ---------------------------------------------------------------- 扭曲场（纯函数，单位：#pet 盒子的逻辑像素）
/// 吸气时头和肩整体抬起的高度
const BREATH_PX: f32 = 1.1;
/// 头轻点的幅度
const BOB_PX: f32 = 0.55;
/// 发梢左右摆的幅度
const HAIR_PX: f32 = 1.5;
/// 衣摆左右摆的幅度
const HEM_PX: f32 = 0.6;
/// 眼睛/嘴所在的高度（立绘画布的比例），贴片按这里的位移整体平移
pub const FACE_V: f32 = 0.40;

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// 呼吸曲线 0..1：吸气 40%（快一点），呼气 60%（慢慢放下）
pub fn breath_curve(p: f32) -> f32 {
    let p = p.rem_euclid(1.0);
    if p < 0.4 {
        0.5 - 0.5 * (std::f32::consts::PI * p / 0.4).cos()
    } else {
        0.5 + 0.5 * (std::f32::consts::PI * (p - 0.4) / 0.6).cos()
    }
}

fn wave(p: f32) -> f32 { (std::f32::consts::TAU * p).sin() }

/// 第 v 行（0 顶 → 1 底）向上抬的像素（逻辑像素，正 = 向上）
pub fn row_lift(l: &LiveView, v: f32) -> f32 {
    if !l.warping() { return 0.0; }
    let (body, head) = if l.sleep {
        // 蜷成一团：上半团随呼吸鼓起，底下贴地不动
        (1.0 - smooth(0.35, 0.92, v), 0.0)
    } else {
        // 头（含脸）整体抬；胸口到衣摆之间被拉伸 = 胸口起伏；腿和脚不动
        (1.0 - smooth(0.50, 0.76, v), 1.0 - smooth(0.48, 0.56, v))
    };
    l.depth * (BREATH_PX * breath_curve(l.breath) * body + BOB_PX * wave(l.bob) * head)
}

/// 第 v 行整体的水平位移（衣摆），逻辑像素，正 = 向右
pub fn row_dx(l: &LiveView, v: f32) -> f32 {
    if !l.warping() || l.sleep { return 0.0; }
    let bell = smooth(0.64, 0.73, v) * (1.0 - smooth(0.74, 0.80, v));
    l.depth * HEM_PX * wave(l.sway - 0.15) * bell
}

/// 发梢的摆幅（第 v 行、左右两侧分别），逻辑像素
pub fn hair_dx(l: &LiveView, v: f32) -> (f32, f32) {
    if !l.warping() || l.sleep { return (0.0, 0.0); }
    // 越往发梢越大，发梢下面就是身子了 → 收住
    let w = smooth(0.28, 0.47, v) * (1.0 - smooth(0.49, 0.54, v));
    let a = l.depth * HAIR_PX * w;
    (a * wave(l.sway), a * wave(l.sway - 0.08))
}

/// 头发列权重：左侧发束 / 右侧发束（u = 列 0 左 → 1 右）。中间（脸、眼睛）恒为 0，保证贴片对得上
pub fn hair_cols(u: f32) -> (f32, f32) { (1.0 - smooth(0.10, 0.22, u), smooth(0.62, 0.74, u)) }

/// 某一点（u, v）的总位移（dx, dy），逻辑像素；dy 正 = 向下。测试和贴片对齐用
pub fn offset_at(l: &LiveView, u: f32, v: f32) -> (f32, f32) {
    let (hl, hr) = hair_dx(l, v);
    let (cl, cr) = hair_cols(u);
    (row_dx(l, v) + hl * cl + hr * cr, -row_lift(l, v))
}

// ---------------------------------------------------------------- 扭曲（逐行重采样）
/// 复用的扭曲缓冲（一张立绘大小）+ 列权重表 + 直接画进窗口时用的一行暂存
#[derive(Default)]
pub struct Warper {
    buf: Option<Pixmap>,
    /// 列权重表对应的 (宽, 外扩)
    cols_key: (u32, u32),
    cols: Vec<(f32, f32)>,
    row: Vec<u8>,
}

/// 第 y 行（源图坐标）的四个位移，单位源图像素。pad = 源图四周外扩（光晕图比立绘大一圈）
fn row_params(l: &LiveView, y: usize, h: usize, pad: u32, k: f32) -> (f32, f32, f32, f32) {
    let inner = h as f32 - 2.0 * pad as f32;
    let v = (y as f32 + 0.5 - pad as f32) / inner.max(1.0);
    let (hl, hr) = hair_dx(l, v);
    (row_lift(l, v) * k, row_dx(l, v) * k, hl * k, hr * k)
}

impl Warper {
    fn ensure_cols(&mut self, w: u32, pad: u32) {
        if self.cols_key != (w, pad) || self.cols.len() != w as usize {
            let inner = (w as f32 - 2.0 * pad as f32).max(1.0);
            self.cols = (0..w).map(|x| hair_cols((x as f32 + 0.5 - pad as f32) / inner)).collect();
            self.cols_key = (w, pad);
        }
    }

    /// 把 src 按 l 扭曲进内部缓冲。box_h = src 对应的逻辑高度（#pet 盒子 293）
    pub fn warp(&mut self, src: &Pixmap, l: &LiveView, box_h: f32) -> Option<&Pixmap> { self.warp_padded(src, l, box_h, 0) }

    /// 同 warp，但 src 四周比立绘多 pad 像素（光晕图）：扭曲场按里面那张立绘的位置算，光晕跟着身子一起动
    pub fn warp_padded(&mut self, src: &Pixmap, l: &LiveView, box_h: f32, pad: u32) -> Option<&Pixmap> {
        let (w, h) = (src.width(), src.height());
        if self.buf.as_ref().is_none_or(|b| b.width() != w || b.height() != h) {
            self.buf = Pixmap::new(w, h);
        }
        self.ensure_cols(w, pad);
        let k = (h as f32 - 2.0 * pad as f32) / box_h;
        let dst = self.buf.as_mut()?;
        let stride = w as usize * 4;
        let (wu, hu) = (w as usize, h as usize);
        for (y, out) in dst.data_mut().chunks_exact_mut(stride).enumerate() {
            warp_row(src.data(), out, wu, hu, &self.cols, y, row_params(l, y, hu, pad, k));
        }
        self.buf.as_ref()
    }

    /// 快速路径：扭曲后直接 src-over 画进 dst 的整数像素位置 (x, y)，mirror = 水平镜像，opacity = 整体不透明度。
    /// 省掉中间缓冲和一次 tiny-skia 的双线性重采样；不动的行（腿、脚）直接拿源行，不做插值
    #[allow(clippy::too_many_arguments)]
    pub fn warp_blit(&mut self, src: &Pixmap, l: &LiveView, box_h: f32, pad: u32, dst: &mut Pixmap, x: i32, y: i32, mirror: bool, opacity: f32) {
        let a = (opacity.clamp(0.0, 1.0) * 256.0).round() as u32;
        if a == 0 { return; }
        let (w, h) = (src.width() as usize, src.height() as usize);
        self.ensure_cols(w as u32, pad);
        self.row.resize(w * 4, 0);
        let k = (h as f32 - 2.0 * pad as f32) / box_h;
        let (dw, dh) = (dst.width() as i32, dst.height() as i32);
        let stride = w * 4;
        let dstride = dw as usize * 4;
        let dd = dst.data_mut();
        for sy in 0..h {
            let dy = y + sy as i32;
            if dy < 0 || dy >= dh { continue; }
            let p = if l.warping() { row_params(l, sy, h, pad, k) } else { (0.0, 0.0, 0.0, 0.0) };
            let row: &[u8] = if p == (0.0, 0.0, 0.0, 0.0) {
                &src.data()[sy * stride..(sy + 1) * stride]
            } else {
                warp_row(src.data(), &mut self.row, w, h, &self.cols, sy, p);
                &self.row
            };
            let drow = &mut dd[dy as usize * dstride..(dy as usize + 1) * dstride];
            blend_row(drow, dw, row, x, mirror, a);
        }
    }
}

/// 一行 src-over（预乘 RGBA）：src 行画到 dst 行的 x 处，mirror = 水平镜像，a = 整体不透明度（256 = 不透明）。
/// 不透明/全透明像素走捷径
pub fn blend_row(drow: &mut [u8], dw: i32, row: &[u8], x: i32, mirror: bool, a: u32) {
    let w = (row.len() / 4) as i32;
    for i in 0..w {
        let dx = if mirror { x + w - 1 - i } else { x + i };
        if dx < 0 || dx >= dw { continue; }
        let s = &row[i as usize * 4..i as usize * 4 + 4];
        if s[3] == 0 { continue; }
        let s = if a >= 256 { [s[0], s[1], s[2], s[3]] } else { [0, 1, 2, 3].map(|c| ((s[c] as u32 * a + 128) >> 8) as u8) };
        let sa = s[3] as u32;
        let d = &mut drow[dx as usize * 4..dx as usize * 4 + 4];
        if sa == 255 || d[3] == 0 {
            d.copy_from_slice(&s);
            continue;
        }
        let inv = 255 - sa;
        for c in 0..4 {
            let v = d[c] as u32 * inv + 128;
            d[c] = (s[c] as u32 + ((v + (v >> 8)) >> 8)).min(255) as u8;
        }
    }
}

/// 目标行 y 取源行 y + lift（内容上移），水平位移 dx = 衣摆 + 列权重 × 发梢；双线性采样（预乘 RGBA 直接插值）。
/// 只有竖直位移的行/像素走整数权重的两行插值（大部分像素），有水平位移的才做完整双线性
fn warp_row(src: &[u8], out: &mut [u8], w: usize, h: usize, cols: &[(f32, f32)], y: usize, (lift, hem, hl, hr): (f32, f32, f32, f32)) {
    const ZERO: [u8; 0] = [];
    let stride = w * 4;
    let src_row = |y: isize| -> &[u8] { if y < 0 || y >= h as isize { &ZERO } else { &src[y as usize * stride..(y as usize + 1) * stride] } };
    let sy = y as f32 + lift;
    let y0f = sy.floor();
    let fy = sy - y0f;
    let y0 = y0f as isize;
    let (ra, rb) = (src_row(y0), src_row(y0 + 1));
    let wy = (fy * 256.0 + 0.5) as u32;
    let horiz = hem != 0.0 || hl != 0.0 || hr != 0.0;
    if !horiz {
        vlerp(ra, rb, out, wy);
        return;
    }
    let px = |r: &[u8], x: isize, c: usize| -> f32 { if r.is_empty() || x < 0 || x >= w as isize { 0.0 } else { r[x as usize * 4 + c] as f32 } };
    for x in 0..w {
        let (cl, cr) = cols[x];
        let dx = hem + hl * cl + hr * cr;
        let o = &mut out[x * 4..x * 4 + 4];
        if dx == 0.0 {
            vlerp(ra.get(x * 4..x * 4 + 4).unwrap_or(&ZERO), rb.get(x * 4..x * 4 + 4).unwrap_or(&ZERO), o, wy);
            continue;
        }
        let sx = x as f32 - dx;
        let x0f = sx.floor();
        let fx = sx - x0f;
        let x0 = x0f as isize;
        for (c, oc) in o.iter_mut().enumerate() {
            let top = px(ra, x0, c) + (px(ra, x0 + 1, c) - px(ra, x0, c)) * fx;
            let bot = px(rb, x0, c) + (px(rb, x0 + 1, c) - px(rb, x0, c)) * fx;
            *oc = (top + (bot - top) * fy + 0.5) as u8;
        }
        // 预乘不变量：颜色不超过 alpha
        let al = o[3];
        for oc in &mut o[..3] { if *oc > al { *oc = al; } }
    }
}

/// 测试用：整张图逐行扭曲（和 Warper 同一套逐行代码）
#[cfg(test)]
fn warp_rows(src: &[u8], dst: &mut [u8], w: usize, h: usize, cols: &[(f32, f32)], row: impl Fn(f32) -> (f32, f32, f32, f32)) {
    for (y, out) in dst.chunks_exact_mut(w * 4).enumerate() {
        warp_row(src, out, w, h, cols, y, row((y as f32 + 0.5) / h as f32));
    }
}

/// 两行按 wy/256 插值（空切片 = 透明行）。整数运算，编译器能向量化
fn vlerp(a: &[u8], b: &[u8], out: &mut [u8], wy: u32) {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => out.fill(0),
        (false, true) => for (o, &x) in out.iter_mut().zip(a) { *o = ((x as u32 * (256 - wy) + 128) >> 8) as u8 },
        (true, false) => for (o, &x) in out.iter_mut().zip(b) { *o = ((x as u32 * wy + 128) >> 8) as u8 },
        (false, false) => {
            if wy == 0 { out.copy_from_slice(a); return; }
            for ((o, &x), &y) in out.iter_mut().zip(a).zip(b) { *o = ((x as u32 * (256 - wy) + y as u32 * wy + 128) >> 8) as u8 }
        }
    }
}

// ---------------------------------------------------------------- 贴片
/// 一个表情的贴片（和立绘同画布尺寸，未缩放）。皮肤包那一路填它
#[derive(Default)]
pub struct Overlays {
    pub blink: Option<Pixmap>,
    pub mouth_open: Option<Pixmap>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Blink, Mouth }

/// 实际画出来的那张立绘的名字（贴片按它找）：原装 walk1/walk2 的头和 idle 一样；换装按 sprites 的别名映射
fn drawn_stem(face: Face, costume: &str) -> String {
    if is_original(costume) {
        return match face { Face::Walk1 | Face::Walk2 => "idle", f => f.stem() }.to_string();
    }
    let src = crate::render::sprites::costume_src(costume, face);
    src.rsplit('/').next().unwrap_or("").trim_end_matches(".png").to_string()
}

/// 原装卫衣立绘（"" 或 "hoodie"）：贴片在 sprites/live/<立绘>_…，不带衣服前缀
fn is_original(costume: &str) -> bool { matches!(costume, "" | "hoodie") }

/// 内置贴片的资源路径：原装 sprites/live/<立绘>_<blink|mouthOpen>.png，换装 sprites/live/<衣服>_<立绘>_….png。
/// 没有这张贴片（闭眼的表情、五官差太多的表情）返回 None
pub fn builtin_path(face: Face, costume: &str, kind: Kind) -> Option<String> {
    let k = match kind { Kind::Blink => "blink", Kind::Mouth => "mouthOpen" };
    let stem = drawn_stem(face, costume);
    let p = if is_original(costume) { format!("sprites/live/{stem}_{k}.png") } else { format!("sprites/live/{costume}_{stem}_{k}.png") };
    crate::assets::get(&p).is_some().then_some(p)
}

/// 这个表情（在这套衣服下）有没有某种贴片
pub fn available(face: Face, costume: &str, kind: Kind) -> bool { builtin_path(face, costume, kind).is_some() }

/// 取一个表情的贴片（解码，未缩放）。唯一入口：皮肤包以后在这里按皮肤读文件
pub fn overlays_for(face: Face, costume: &str) -> Overlays {
    let load = |k: Kind| -> Option<Pixmap> { Pixmap::decode_png(crate::assets::get(&builtin_path(face, costume, k)?)?).ok() };
    Overlays { blink: load(Kind::Blink), mouth_open: load(Kind::Mouth) }
}

/// 缩放 + 裁好的贴片：pm 放在立绘缩放图的 (x, y) 处
pub struct Patch {
    pub pm: Pixmap,
    pub x: i32,
    pub y: i32,
}

#[derive(Default)]
pub struct Patches {
    pub blink: Option<Patch>,
    pub mouth: Option<Patch>,
}

impl Patches {
    pub fn from_overlays(o: Overlays, w: u32, h: u32) -> Patches {
        Patches { blink: o.blink.and_then(|p| patch(p, w, h)), mouth: o.mouth_open.and_then(|p| patch(p, w, h)) }
    }
}

/// 缩放到 (w,h) 后裁到不透明部分的外接矩形（四周留 1px）
fn patch(src: Pixmap, w: u32, h: u32) -> Option<Patch> {
    let s = scale_to(src, w, h)?;
    let (sw, sh) = (s.width() as usize, s.height() as usize);
    let d = s.data();
    let (mut x0, mut y0, mut x1, mut y1) = (sw, sh, 0usize, 0usize);
    for y in 0..sh {
        for x in 0..sw {
            if d[(y * sw + x) * 4 + 3] > 0 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if x1 < x0 { return None; }
    let (x0, y0) = (x0.saturating_sub(1), y0.saturating_sub(1));
    let (x1, y1) = ((x1 + 1).min(sw - 1), (y1 + 1).min(sh - 1));
    let (cw, ch) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut pm = Pixmap::new(cw as u32, ch as u32)?;
    for y in 0..ch {
        let from = ((y0 + y) * sw + x0) * 4;
        pm.data_mut()[y * cw * 4..(y + 1) * cw * 4].copy_from_slice(&d[from..from + cw * 4]);
    }
    Some(Patch { pm, x: x0 as i32, y: y0 as i32 })
}

/// 和 sprites::load_scaled 同样的思路：先 2× 盒式降采样，再双三次
fn scale_to(mut src: Pixmap, w: u32, h: u32) -> Option<Pixmap> {
    while src.width() >= w * 2 && src.height() >= h * 2 {
        let (hw, hh) = (src.width() / 2, src.height() / 2);
        let mut dst = Pixmap::new(hw.max(1), hh.max(1))?;
        let sw = src.width() as usize;
        let s = src.data();
        let d = dst.data_mut();
        for y in 0..hh as usize {
            for x in 0..hw as usize {
                for c in 0..4 {
                    let i = |xx: usize, yy: usize| s[(yy * sw + xx) * 4 + c] as u32;
                    let v = i(2 * x, 2 * y) + i(2 * x + 1, 2 * y) + i(2 * x, 2 * y + 1) + i(2 * x + 1, 2 * y + 1);
                    d[(y * hw as usize + x) * 4 + c] = ((v + 2) / 4) as u8;
                }
            }
        }
        src = dst;
    }
    let mut dst = Pixmap::new(w.max(1), h.max(1))?;
    let t = Transform::from_scale(w as f32 / src.width() as f32, h as f32 / src.height() as f32);
    dst.draw_pixmap(0, 0, src.as_ref(), &PixmapPaint { quality: FilterQuality::Bicubic, ..Default::default() }, t, None);
    Some(dst)
}

/// (表情, 衣服, 尺寸)
type PatchKey = (Face, String, (u32, u32));

/// 每个 (表情, 衣服, 尺寸) 一份缩放好的贴片，最多留 CAP 份（几十 KB）
#[derive(Default)]
pub struct PatchCache {
    entries: Vec<(PatchKey, Rc<Patches>, u64)>,
    tick: u64,
}

impl PatchCache {
    const CAP: usize = 4;

    pub fn get(&mut self, face: Face, costume: &str, size: (u32, u32)) -> Rc<Patches> {
        self.tick += 1;
        if let Some(e) = self.entries.iter_mut().find(|e| e.0.0 == face && e.0.1 == costume && e.0.2 == size) {
            e.2 = self.tick;
            return e.1.clone();
        }
        let p = Rc::new(Patches::from_overlays(overlays_for(face, costume), size.0, size.1));
        if self.entries.len() >= Self::CAP
            && let Some(i) = self.entries.iter().enumerate().min_by_key(|(_, e)| e.2).map(|(i, _)| i)
        {
            self.entries.swap_remove(i);
        }
        self.entries.push(((face, costume.to_string(), size), p.clone(), self.tick));
        p
    }

    pub fn clear(&mut self) { self.entries.clear(); }
}

// ---------------------------------------------------------------- 眨眼排程（纯逻辑）
/// 一下眨眼的时长（ms）：闭 60 → 停 40 → 睁 90
const CLOSE_MS: f32 = 60.0;
const HOLD_MS: f32 = 40.0;
const OPEN_MS: f32 = 90.0;
pub const BLINK_MS: u64 = (CLOSE_MS + HOLD_MS + OPEN_MS) as u64;
/// 连眨两下时第二下在第一下开始后多久
pub const DOUBLE_GAP_MS: u64 = 260;

/// 一下眨眼开始后 t 毫秒时的闭眼程度
pub fn blink_curve(t: f32) -> f32 {
    if t < 0.0 { 0.0 }
    else if t < CLOSE_MS { smooth(0.0, 1.0, t / CLOSE_MS) }
    else if t < CLOSE_MS + HOLD_MS { 1.0 }
    else if t < CLOSE_MS + HOLD_MS + OPEN_MS { 1.0 - smooth(0.0, 1.0, (t - CLOSE_MS - HOLD_MS) / OPEN_MS) }
    else { 0.0 }
}

/// 随机眨眼：间隔 2~6 秒，约 20% 连眨两下
#[derive(Clone, Debug, Default)]
pub struct BlinkSched {
    /// 下一次眨眼的时刻
    pub next: u64,
    /// 正在进行的眨眼的起点（连眨时两个）
    starts: [Option<u64>; 2],
}

impl BlinkSched {
    pub const MIN_GAP: f64 = 2000.0;
    pub const MAX_GAP: f64 = 6000.0;
    pub const DOUBLE_P: f64 = 0.2;

    pub fn new(now: u64, r: f64) -> Self { Self { next: now + Self::gap(r), starts: [None; 2] } }

    fn gap(r: f64) -> u64 { (Self::MIN_GAP + r.clamp(0.0, 1.0) * (Self::MAX_GAP - Self::MIN_GAP)) as u64 }

    /// 到点了：开始眨（allowed=false 时只排下一次）。r_gap/r_double ∈ [0,1) 随机数
    pub fn fire(&mut self, now: u64, allowed: bool, r_gap: f64, r_double: f64) {
        if allowed {
            self.starts = [Some(now), (r_double < Self::DOUBLE_P).then_some(now + DOUBLE_GAP_MS)];
        }
        self.next = now + Self::gap(r_gap);
    }

    /// 打断（换成闭眼表情、被拎起来……）
    pub fn cancel(&mut self) { self.starts = [None; 2]; }

    /// now 时刻的闭眼程度
    pub fn amount(&self, now: u64) -> f32 {
        self.starts.iter().flatten().map(|&s| blink_curve(now as f32 - s as f32)).fold(0.0, f32::max)
    }

    /// 还在眨（要高帧率）
    pub fn active(&self, now: u64) -> bool { self.starts.iter().flatten().any(|&s| now + 1 >= s && now < s + BLINK_MS) }

    /// 下一个需要醒来的时刻（连眨的第二下）
    pub fn due(&self, now: u64) -> Option<u64> { self.starts.iter().flatten().copied().filter(|&s| s > now).min() }
}

// ---------------------------------------------------------------- 口型（RMS → 张嘴）
/// 低于这个分贝算闭嘴，高于 FULL_DB 全张
const FLOOR_DB: f32 = -40.0;
const FULL_DB: f32 = -16.0;

/// 一个 RMS（0..1 满幅）对应的目标张嘴程度
pub fn rms_to_open(rms: f32) -> f32 {
    if rms.is_nan() || rms <= 1e-6 { return 0.0; }
    let db = 20.0 * rms.log10();
    ((db - FLOOR_DB) / (FULL_DB - FLOOR_DB)).clamp(0.0, 1.0)
}

/// 包络：张嘴快（attack），闭嘴稍慢（release），按帧间隔 dt 毫秒推进
#[derive(Clone, Copy, Debug, Default)]
pub struct MouthEnv {
    pub value: f32,
}

impl MouthEnv {
    const ATTACK_MS: f32 = 35.0;
    const RELEASE_MS: f32 = 90.0;

    pub fn step(&mut self, target: f32, dt_ms: f32) -> f32 {
        let tau = if target > self.value { Self::ATTACK_MS } else { Self::RELEASE_MS };
        let a = 1.0 - (-dt_ms.max(0.0) / tau).exp();
        self.value += (target - self.value) * a;
        if self.value < 0.01 { self.value = 0.0; }
        self.value
    }
}

/// 张嘴程度 → 贴片不透明度：小声时不张，过了门槛很快张满（贴片半透明的时间越短越不“鬼影”）
pub fn mouth_alpha(open: f32) -> f32 { smooth(0.18, 0.45, open) }

// ---------------------------------------------------------------- 省电（平台层探测，行为层读）
use std::sync::atomic::{AtomicBool, Ordering};
static LOW_POWER: AtomicBool = AtomicBool::new(false);

/// 平台层：她被藏起来 / 前台是全屏程序 / 用电池时置 true，灵动效果退回老的低帧率
pub fn set_low_power(on: bool) { LOW_POWER.store(on, Ordering::Relaxed); }
pub fn low_power() -> bool { LOW_POWER.load(Ordering::Relaxed) }

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> LiveView { LiveView { depth: 1.0, ..Default::default() } }

    #[test]
    fn breath_curve_shape() {
        assert!(breath_curve(0.0).abs() < 1e-4);
        assert!((breath_curve(0.4) - 1.0).abs() < 1e-4);
        assert!(breath_curve(1.0).abs() < 1e-4);
        assert!((breath_curve(0.2) - 0.5).abs() < 1e-3);
        // 连续、单峰
        let mut prev = breath_curve(0.0);
        for i in 1..=40 { let v = breath_curve(i as f32 / 100.0); assert!(v >= prev - 1e-5); prev = v; }
        for i in 41..=100 { let v = breath_curve(i as f32 / 100.0); assert!(v <= prev + 1e-5); prev = v; }
    }

    #[test]
    fn warp_off_is_identity() {
        let l = LiveView::default();
        for v in [0.0, 0.3, 0.5, 0.7, 1.0] {
            assert_eq!(row_lift(&l, v), 0.0);
            assert_eq!(row_dx(&l, v), 0.0);
            assert_eq!(hair_dx(&l, v), (0.0, 0.0));
        }
    }

    #[test]
    fn feet_never_move_and_head_moves_most() {
        for i in 0..50 {
            let p = i as f32 / 50.0;
            let l = LiveView { breath: p, bob: p * 1.7, sway: p * 0.6, ..on() };
            for u in [0.0, 0.3, 0.5, 0.9] {
                let (dx, dy) = offset_at(&l, u, 0.95);
                assert!(dx.abs() < 1e-6 && dy.abs() < 1e-6, "feet moved at p={p}");
            }
            // 头部总位移不超过 ~2px（细微）
            assert!(row_lift(&l, FACE_V).abs() <= BREATH_PX + BOB_PX + 1e-4);
        }
        // 吸气顶点：头抬起来、胸口在中间
        let l = LiveView { breath: 0.4, bob: 0.0, ..on() };
        assert!((row_lift(&l, 0.2) - BREATH_PX).abs() < 1e-4);
        let mid = row_lift(&l, 0.63);
        assert!(mid > 0.0 && mid < BREATH_PX);
        // 睡觉更深
        let s = LiveView { depth: 1.6, sleep: true, breath: 0.4, ..Default::default() };
        assert!(row_lift(&s, 0.2) > row_lift(&l, 0.2));
        assert_eq!(hair_dx(&s, 0.45), (0.0, 0.0));
    }

    #[test]
    fn face_region_moves_rigidly() {
        // 眼睛/嘴那一块（u 0.22..0.62, v 0.28..0.48）位移处处相同 → 贴片整体平移就对得上
        for i in 0..20 {
            let p = i as f32 / 20.0;
            let l = LiveView { breath: p, bob: p * 1.3 + 0.1, sway: p * 2.1, ..on() };
            let (rx, ry) = offset_at(&l, 0.4, FACE_V);
            for u in [0.23, 0.3, 0.45, 0.61] {
                for v in [0.28, 0.35, 0.42, 0.47] {
                    let (dx, dy) = offset_at(&l, u, v);
                    assert!((dx - rx).abs() < 1e-3 && (dy - ry).abs() < 1e-3, "u={u} v={v}");
                }
            }
            assert!(rx.abs() < 1e-6);
        }
    }

    #[test]
    fn hair_tips_sway_more_than_roots() {
        let l = LiveView { sway: 0.25, ..on() };
        let (tip, _) = hair_dx(&l, 0.47);
        let (root, _) = hair_dx(&l, 0.30);
        assert!(tip > root && tip <= HAIR_PX + 1e-4 && root >= 0.0);
        assert_eq!(hair_cols(0.42), (0.0, 0.0));
        assert!(hair_cols(0.05).0 > 0.99 && hair_cols(0.8).1 > 0.99);
    }

    #[test]
    fn warp_rows_shift() {
        // 4×4 图，第 1 行不透明；整体上移 1 行 → 第 0 行不透明
        let (w, h) = (4usize, 4usize);
        let mut src = vec![0u8; w * h * 4];
        for x in 0..w { for c in 0..4 { src[(w + x) * 4 + c] = 200; } }
        let mut dst = vec![0u8; w * h * 4];
        let cols = vec![(0.0, 0.0); w];
        warp_rows(&src, &mut dst, w, h, &cols, |_| (1.0, 0.0, 0.0, 0.0));
        assert!(dst[..w * 4].iter().all(|&b| b == 200));
        assert!(dst[w * 4..].iter().all(|&b| b == 0));
        // 半像素：两行各一半
        warp_rows(&src, &mut dst, w, h, &cols, |_| (0.5, 0.0, 0.0, 0.0));
        assert_eq!(dst[0], 100);
        assert_eq!(dst[w * 4], 100);
        // 水平半像素（衣摆）：x=0 列取到左边界外 → 一半
        warp_rows(&src, &mut dst, w, h, &cols, |_| (0.0, 0.5, 0.0, 0.0));
        assert_eq!(dst[w * 4], 100);
        assert_eq!(dst[(w + 1) * 4], 200);
    }

    #[test]
    fn blink_curve_shape() {
        assert_eq!(blink_curve(-1.0), 0.0);
        assert_eq!(blink_curve(0.0), 0.0);
        assert_eq!(blink_curve(CLOSE_MS + 1.0), 1.0);
        assert!(blink_curve(CLOSE_MS / 2.0) > 0.3 && blink_curve(CLOSE_MS / 2.0) < 0.7);
        assert_eq!(blink_curve(BLINK_MS as f32), 0.0);
    }

    #[test]
    fn blink_scheduler() {
        let mut b = BlinkSched::new(0, 0.0);
        assert_eq!(b.next, 2000);
        assert_eq!(BlinkSched::new(0, 0.999_999).next, 5999);
        b.fire(2000, true, 0.5, 0.9); // 单眨
        assert_eq!(b.next, 6000);
        assert!(b.active(2000) && b.active(2100));
        assert_eq!(b.amount(2000 + 80), 1.0);
        assert!(!b.active(2000 + BLINK_MS));
        assert_eq!(b.amount(2000 + BLINK_MS + 5), 0.0);
        assert_eq!(b.due(2000), None);
        b.fire(6000, true, 0.0, 0.1); // 连眨
        assert_eq!(b.due(6000), Some(6000 + DOUBLE_GAP_MS));
        assert!(b.amount(6000 + DOUBLE_GAP_MS + 80) > 0.99);
        // 不允许（闭眼表情）：只排下一次
        let mut c = BlinkSched::new(0, 0.0);
        c.fire(2000, false, 0.0, 0.0);
        assert_eq!(c.amount(2050), 0.0);
        assert_eq!(c.next, 4000);
        b.cancel();
        assert_eq!(b.amount(6000 + DOUBLE_GAP_MS + 80), 0.0);
        // 间隔分布在 2~6 秒
        for i in 0..100 {
            let mut s = BlinkSched::new(0, 0.0);
            s.fire(0, true, i as f64 / 100.0, 1.0);
            assert!((2000..6000).contains(&s.next));
        }
    }

    #[test]
    fn rms_mapping() {
        assert_eq!(rms_to_open(0.0), 0.0);
        assert_eq!(rms_to_open(f32::NAN), 0.0);
        assert_eq!(rms_to_open(0.005), 0.0); // -46 dB
        assert_eq!(rms_to_open(0.5), 1.0); // -6 dB
        let a = rms_to_open(0.03);
        let b = rms_to_open(0.08);
        assert!(a > 0.0 && b > a && b < 1.0);
        assert_eq!(mouth_alpha(0.0), 0.0);
        assert_eq!(mouth_alpha(1.0), 1.0);
    }

    #[test]
    fn mouth_envelope() {
        let mut e = MouthEnv::default();
        // 张嘴快：一帧（55ms）就过门槛
        let v = e.step(1.0, 55.0);
        assert!(v > 0.7, "{v}");
        // 闭嘴慢一点，但几帧内回到 0
        let v1 = e.step(0.0, 55.0);
        assert!(v1 < v && v1 > 0.1);
        for _ in 0..8 { e.step(0.0, 55.0); }
        assert_eq!(e.value, 0.0);
    }

    #[test]
    fn builtin_overlays_exist_and_crop_small() {
        for f in [Face::Idle, Face::Walk1, Face::Happy, Face::Think, Face::Shy, Face::Love, Face::Surprised] {
            assert!(available(f, "", Kind::Mouth), "{f:?}");
        }
        for f in [Face::Idle, Face::Walk2, Face::Think, Face::Love, Face::Stare, Face::Surprised, Face::Hungry] {
            assert!(available(f, "", Kind::Blink), "{f:?}");
        }
        for f in [Face::Sleep, Face::Happy, Face::Shy, Face::Yawn, Face::Dance, Face::Eat] {
            assert!(!available(f, "", Kind::Blink), "{f:?} has closed eyes");
        }
        // 原装卫衣现在的 id 是 "hoodie"，和 "" 用同一套贴片；默认水手服只有待机眨眼
        assert_eq!(builtin_path(Face::Idle, "hoodie", Kind::Mouth), builtin_path(Face::Idle, "", Kind::Mouth));
        assert!(available(Face::Idle, "sailor", Kind::Blink));
        // 换装：按实际画的那张（shy/love → happy，stare → think，yawn → idle）
        for c in ["maid", "xmas", "jk"] {
            for f in [Face::Idle, Face::Yawn, Face::Think, Face::Stare, Face::Hungry] { assert!(available(f, c, Kind::Blink), "{c} {f:?}"); }
            for f in [Face::Idle, Face::Love, Face::Shy, Face::Think] { assert!(available(f, c, Kind::Mouth), "{c} {f:?}"); }
            for f in [Face::Love, Face::Sleep, Face::Drag] { assert!(!available(f, c, Kind::Blink), "{c} {f:?}"); }
        }
        assert_eq!(builtin_path(Face::Shy, "maid", Kind::Mouth).as_deref(), Some("sprites/live/maid_happy_mouthOpen.png"));
        let p = Patches::from_overlays(overlays_for(Face::Idle, ""), 300, 440);
        let b = p.blink.as_ref().unwrap();
        let m = p.mouth.as_ref().unwrap();
        // 小贴片：只有眼/嘴那一块
        assert!(b.pm.width() < 140 && b.pm.height() < 70, "{}x{}", b.pm.width(), b.pm.height());
        assert!(m.pm.width() < 60 && m.pm.height() < 50, "{}x{}", m.pm.width(), m.pm.height());
        // 位置在脸上（画布 400×587 → 300×440）
        let v = (m.y as f32 + m.pm.height() as f32 / 2.0) / 440.0;
        assert!((0.36..0.48).contains(&v), "mouth v={v}");
        let v = (b.y as f32 + b.pm.height() as f32 / 2.0) / 440.0;
        assert!((0.30..0.40).contains(&v), "eyes v={v}");
    }

    #[test]
    fn patch_cache_bounded() {
        let mut c = PatchCache::default();
        for f in [Face::Idle, Face::Happy, Face::Think, Face::Shy, Face::Love, Face::Surprised] {
            c.get(f, "", (200, 293));
        }
        assert!(c.entries.len() <= PatchCache::CAP);
        let a = c.get(Face::Surprised, "", (200, 293));
        let b = c.get(Face::Surprised, "", (200, 293));
        assert!(Rc::ptr_eq(&a, &b));
    }
    #[test]
    fn blend_row_opacity_and_mirror() {
        // dst: 3 像素，第 0 个已有半透明红
        let mut d = vec![0u8; 12];
        d[0..4].copy_from_slice(&[100, 0, 0, 100]);
        let row = [10u8, 20, 30, 255, 0, 0, 0, 0, 50, 50, 50, 128];
        blend_row(&mut d, 3, &row, 0, false, 256);
        assert_eq!(&d[0..4], &[10, 20, 30, 255]); // 不透明直接盖
        assert_eq!(&d[4..8], &[0, 0, 0, 0]); // 透明跳过
        assert_eq!(&d[8..12], &[50, 50, 50, 128]); // 目标空直接拷
        // 镜像：第 0 个源像素落到最右
        let mut m = vec![0u8; 12];
        blend_row(&mut m, 3, &row, 0, true, 256);
        assert_eq!(&m[8..12], &[10, 20, 30, 255]);
        // 半透明叠半透明：预乘 src-over
        let mut e = vec![100u8, 0, 0, 100];
        blend_row(&mut e, 1, &[0, 0, 100, 100], 0, false, 256);
        assert_eq!(e[3] as u32, 100 + (100 * 155 + 127) / 255);
        assert!(e[0] > 55 && e[0] < 65 && e[2] == 100);
        // 整体不透明度一半
        let mut h = vec![0u8; 4];
        blend_row(&mut h, 1, &[200, 100, 0, 255], 0, false, 128);
        assert_eq!(&h[..], &[100, 50, 0, 128]);
        // 出界不写
        let mut c = vec![0u8; 4];
        blend_row(&mut c, 1, &row, -2, false, 256);
        assert_eq!(&c[..], &[50, 50, 50, 128]);
    }

    fn test_img(w: u32, h: u32) -> Pixmap {
        let mut p = Pixmap::new(w, h).unwrap();
        for (i, px) in p.data_mut().chunks_exact_mut(4).enumerate() {
            let (x, y) = (i as u32 % w, i as u32 / w);
            if (x / 3 + y / 5) % 2 == 0 { px.copy_from_slice(&[(x * 7 % 256) as u8 / 2, (y * 3 % 256) as u8 / 2, 60, 200]); }
        }
        p
    }

    #[test]
    fn warp_blit_matches_buffered_warp() {
        let src = test_img(60, 88);
        let l = LiveView { depth: 1.0, breath: 0.33, bob: 0.2, sway: 0.27, ..Default::default() };
        let mut w = Warper::default();
        let buf = w.warp(&src, &l, 88.0).unwrap().clone();
        let mut dst = Pixmap::new(80, 100).unwrap();
        w.warp_blit(&src, &l, 88.0, 0, &mut dst, 7, 9, false, 1.0);
        // 直接画进窗口的结果 = 扭曲进缓冲再整像素拷贝
        for y in 0..88usize {
            for x in 0..60usize {
                let a = &buf.data()[(y * 60 + x) * 4..][..4];
                let b = &dst.data()[((y + 9) * 80 + x + 7) * 4..][..4];
                assert_eq!(a, b, "x={x} y={y}");
            }
        }
        // 不扭曲 = 原样拷贝（像素级不变 → 不发虚）
        let mut d2 = Pixmap::new(60, 88).unwrap();
        w.warp_blit(&src, &LiveView::default(), 88.0, 0, &mut d2, 0, 0, false, 1.0);
        assert_eq!(d2.data(), src.data());
    }

    #[test]
    fn padded_warp_follows_inner_sprite() {
        // 光晕图 = 立绘四周外扩 pad：扭曲场按里面立绘的位置算，和立绘本身的位移一致
        let pad = 6u32;
        let src = test_img(60, 88);
        let mut big = Pixmap::new(60 + 2 * pad, 88 + 2 * pad).unwrap();
        big.draw_pixmap(pad as i32, pad as i32, src.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        let l = LiveView { depth: 1.0, breath: 0.4, bob: 0.1, sway: 0.3, ..Default::default() };
        let mut w = Warper::default();
        let inner = w.warp(&src, &l, 88.0).unwrap().clone();
        let outer = w.warp_padded(&big, &l, 88.0, pad).unwrap().clone();
        let ow = (60 + 2 * pad) as usize;
        let mut diff = 0u32;
        for y in 0..88usize {
            for x in 0..60usize {
                let a = &inner.data()[(y * 60 + x) * 4..][..4];
                let b = &outer.data()[((y + pad as usize) * ow + x + pad as usize) * 4..][..4];
                diff = diff.max(a.iter().zip(b).map(|(p, q)| (*p as i32 - *q as i32).unsigned_abs()).max().unwrap());
            }
        }
        // 边缘行可能从外扩区取到透明像素，内部一致
        assert!(diff <= 200, "{diff}");
        let mut same = 0;
        for y in 10..80usize { for x in 10..50usize {
            if inner.data()[(y * 60 + x) * 4..][..4] == outer.data()[((y + pad as usize) * ow + x + pad as usize) * 4..][..4] { same += 1; }
        } }
        assert!(same as f32 > 0.99 * 70.0 * 40.0, "{same}");
    }

    // ------------------------------------------------------------ 贴片覆盖率矩阵
    /// 每套衣服应该有的贴片（按画出来的那张立绘算）：睁眼的要眨眼，能说话的要张嘴。
    /// 闭眼/表情太夸张的（sleep、drag、eat、sit）不要求
    const WANT: [(&str, &[Kind]); 4] = [
        ("idle", &[Kind::Blink, Kind::Mouth]),
        ("happy", &[Kind::Mouth]),
        ("hungry", &[Kind::Blink]),
        ("think", &[Kind::Blink, Kind::Mouth]),
    ];

    /// 还没补上的缺口（衣服, 立绘, 种类）。补一张就从这里删一行；测试会拦住「补了没删」和「新缺口」
    const KNOWN_GAPS: &[(&str, &str, Kind)] = &[
        ("sailor", "hungry", Kind::Blink),
        ("sailor", "think", Kind::Blink),
        ("pajama", "idle", Kind::Blink),
        ("pajama", "hungry", Kind::Blink),
        ("pajama", "think", Kind::Blink),
        ("idol", "idle", Kind::Blink),
        ("idol", "hungry", Kind::Blink),
        ("idol", "think", Kind::Blink),
        ("swim", "idle", Kind::Blink),
        ("swim", "hungry", Kind::Blink),
        ("swim", "think", Kind::Blink),
    ];

    /// 衣服 → (表情, 立绘)：同一张立绘只算一次
    fn stems_of(costume: &str) -> Vec<(Face, String)> {
        let mut v: Vec<(Face, String)> = Vec::new();
        for f in Face::ALL {
            let s = drawn_stem(f, costume);
            if !v.iter().any(|(_, x)| *x == s) { v.push((f, s)); }
        }
        v
    }

    /// 打印矩阵：cargo test --release patch_coverage -- --nocapture
    #[test]
    fn patch_coverage_matrix() {
        let mark = |f: Face, c: &str, k: Kind, ch: char| if available(f, c, k) { ch } else { '·' };
        let mut gaps = Vec::new();
        println!("贴片覆盖率（B=眨眼 M=张嘴 ·=没有）");
        for (c, _) in crate::core::costume::COSTUMES {
            let row: Vec<String> = stems_of(c).iter()
                .map(|(f, s)| format!("{s}:{}{}", mark(*f, c, Kind::Blink, 'B'), mark(*f, c, Kind::Mouth, 'M')))
                .collect();
            println!("{c:7} {}", row.join(" "));
            for (stem, kinds) in WANT {
                let Some((f, _)) = stems_of(c).into_iter().find(|(_, s)| s == stem) else { continue };
                for &k in kinds {
                    if !available(f, c, k) { gaps.push((c, stem, k)); }
                }
            }
        }
        for g in &gaps {
            assert!(KNOWN_GAPS.contains(g), "新缺口（贴片丢了？）：{g:?}");
        }
        for g in KNOWN_GAPS {
            assert!(gaps.contains(g), "已经补上了，从 KNOWN_GAPS 删掉：{g:?}");
        }
        let total = crate::core::costume::COSTUMES.len() * WANT.iter().map(|(_, k)| k.len()).sum::<usize>();
        println!("覆盖 {}/{total}", total - gaps.len());
    }

    /// assets/sprites/live 里每张贴片都有表情用得到，且和它贴的那张立绘一样大
    #[test]
    fn patches_used_and_sized() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/sprites/live");
        let mut used = std::collections::HashMap::new();
        for c in ["hoodie"].into_iter().chain(crate::core::costume::COSTUMES.iter().map(|(c, _)| *c)) {
            for f in Face::ALL {
                let base = crate::render::sprites::layers_for(f, c, &[]).base;
                for k in [Kind::Blink, Kind::Mouth] {
                    if let Some(p) = builtin_path(f, c, k) { used.insert(p, base.clone()); }
                }
            }
        }
        for e in std::fs::read_dir(dir).unwrap() {
            let name = e.unwrap().file_name().into_string().unwrap();
            let p = format!("sprites/live/{name}");
            let base = used.get(&p).unwrap_or_else(|| panic!("没有表情用到这张贴片：{p}"));
            let a = Pixmap::decode_png(crate::assets::get(&p).unwrap()).unwrap();
            let b = Pixmap::decode_png(crate::assets::get(base).unwrap()).unwrap();
            assert_eq!((a.width(), a.height()), (b.width(), b.height()), "{p} vs {base}");
        }
    }
}
