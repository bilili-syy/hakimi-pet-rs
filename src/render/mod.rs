//! 渲染层（负责人：C 路「渲染与行为」，集成负责人）
//! tiny-skia 软渲染：立绘 + 尾巴/星星分层动画 + 换装 + 特效 + 气泡 + 状态卡 + zzZ
//! 布局全部按原版 index.html/style.css 的逻辑像素（240×360）算，再整体乘 DPI 缩放。
pub mod anim;
pub mod shapes;
pub mod sprites;
pub mod text;

pub use anim::Xf;
use crate::types::{Face, Frame, WIN_H, WIN_W};
use std::rc::Rc;
use tiny_skia::{BlendMode, FilterQuality, Pixmap, PixmapPaint, Transform};

// ---------------------------------------------------------------- 场景（行为层每帧填）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FxShape {
    Heart,
    /// 0 = ♪，1 = ♫，2 = ♬
    Note(u8),
    Spark,
    Drop,
    Anger,
    Exclaim,
}

/// 一个特效字形。x/y 是元素左上角（#pet 盒子 200×293 内的坐标），size 是字号
#[derive(Clone, Debug, PartialEq)]
pub struct FxSprite {
    pub shape: FxShape,
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub xf: Xf,
    pub color: u32,
}

/// zzZ 的一个字母（窗口坐标，元素左上角）
#[derive(Clone, Debug, PartialEq)]
pub struct ZGlyph {
    pub ch: char,
    pub size: f32,
    pub color: u32,
    pub x: f32,
    pub y: f32,
    pub opacity: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BubbleView {
    pub text: String,
    pub opacity: f32,
    /// 字太多超出 max-height 时的滚动进度 0..1
    pub scroll: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StatusView {
    pub days: u32,
    pub hunger: f32,
    pub mood: f32,
    pub energy: f32,
    pub level: String,
    pub affection: f64,
    pub moyu: Option<u32>,
    pub opacity: f32,
}

/// 行为层每帧交给渲染层的场景描述
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub face: Face,
    /// "" / xmas / maid / jk
    pub costume: String,
    /// santa / glasses / scarf（按配置里的顺序叠）
    pub outfits: Vec<String>,
    /// #pet 的动画变换（原点：盒子底边中点）
    pub pet: Xf,
    /// #body 的 scaleX（1 正常，-1 镜像；中间值 = 翻转过渡中）
    pub flip: f32,
    pub tail: Xf,
    pub star: Xf,
    /// 喝了蜂蜜水的金色光晕（drop-shadow）
    pub glow: bool,
    pub bubble: Option<BubbleView>,
    pub status: Option<StatusView>,
    pub zzz: Vec<ZGlyph>,
    pub fx: Vec<FxSprite>,
}

impl Default for Scene {
    fn default() -> Self {
        Self {
            face: Face::Idle,
            costume: String::new(),
            outfits: Vec::new(),
            pet: Xf::ID,
            flip: 1.0,
            tail: Xf::ID,
            star: Xf::ID,
            glow: false,
            bubble: None,
            status: None,
            zzz: Vec::new(),
            fx: Vec::new(),
        }
    }
}

/// 逻辑坐标矩形
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}
impl Rect {
    pub fn contains(&self, x: f32, y: f32) -> bool { x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h }
}

// ---------------------------------------------------------------- 布局常量（style.css）
const PET_X: f32 = 20.0;
const PET_W: f32 = 200.0;
const PET_H: f32 = 293.0;
const PET_Y: f32 = WIN_H as f32 - PET_H;
const C_TEXT: u32 = 0x5a3b2e;
const C_BG: u32 = 0xfffaf0;
const C_BORDER: u32 = 0xfbbc04;
const C_RED: u32 = 0xea4335;
const C_GREEN: u32 = 0x34a853;
const C_BLUE: u32 = 0x4285f4;
const C_YELLOW: u32 = 0xfbbc04;

/// CSS transform（translate → rotate → scale）绕 origin
pub fn xf_matrix(xf: &Xf, ox: f32, oy: f32) -> Transform {
    Transform::from_translate(ox + xf.tx, oy + xf.ty)
        .pre_concat(Transform::from_rotate(xf.rot))
        .pre_scale(xf.sx, xf.sy)
        .pre_translate(-ox, -oy)
}

struct BubbleText {
    key: String,
    lines: Vec<(Option<text::Coverage>, f32)>,
    cell: f32,
}

struct Layer {
    key: String,
    pm: Pixmap,
    /// 设备像素位置
    x: i32,
    y: i32,
    rect: Rect,
}

pub struct Renderer {
    pm: Option<Pixmap>,
    frame: Frame,
    last: Option<(Scene, f32)>,
    sprites: sprites::SpriteCache,
    text: text::TextEngine,
    bubble: Option<Layer>,
    bubble_text: Option<BubbleText>,
    status: Option<Layer>,
    glyphs: Vec<((char, u32), Rc<text::Coverage>)>,
    pub bubble_rect: Option<Rect>,
    pub status_rect: Option<Rect>,
    /// 气泡文字超出 max-height（行为层据此自动滚动）
    pub bubble_overflow: bool,
}

impl Default for Renderer {
    fn default() -> Self { Self::new() }
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            pm: None,
            frame: Frame { w: 0, h: 0, bgra: Vec::new() },
            last: None,
            sprites: sprites::SpriteCache::new(12),
            text: text::TextEngine::new(),
            bubble: None,
            bubble_text: None,
            status: None,
            glyphs: Vec::new(),
            bubble_rect: None,
            status_rect: None,
            bubble_overflow: false,
        }
    }

    /// 画面有变化才返回新帧
    #[allow(dead_code)]
    pub fn render(&mut self, scene: &Scene, scale: f32) -> Option<&Frame> {
        if self.update(scene, scale) { Some(&self.frame) } else { None }
    }

    /// 画一帧到内部缓冲；没变化返回 false
    pub fn update(&mut self, scene: &Scene, scale: f32) -> bool {
        let scale = if scale.is_finite() && scale > 0.1 { scale } else { 1.0 };
        if let Some((s, sc)) = &self.last
            && s == scene
            && *sc == scale
        {
            return false;
        }
        self.draw(scene, scale);
        self.last = Some((scene.clone(), scale));
        let Some(pm) = self.pm.as_ref() else { return false };
        let (w, h) = (pm.width(), pm.height());
        self.frame.w = w;
        self.frame.h = h;
        self.frame.bgra.resize((w * h * 4) as usize, 0);
        for (d, s) in self.frame.bgra.as_chunks_mut::<4>().0.iter_mut().zip(pm.data().as_chunks::<4>().0.iter()) {
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
            d[3] = s[3];
        }
        true
    }

    pub fn frame(&self) -> &Frame { &self.frame }

    /// 只画到内部 Pixmap（测试用）
    pub fn draw(&mut self, scene: &Scene, scale: f32) {
        let w = (WIN_W as f32 * scale).round() as u32;
        let h = (WIN_H as f32 * scale).round() as u32;
        if self.pm.as_ref().is_none_or(|p| p.width() != w || p.height() != h) {
            self.pm = Pixmap::new(w, h);
            self.sprites.clear();
            self.bubble = None;
            self.bubble_text = None;
            self.status = None;
            self.glyphs.clear();
        }
        let Some(mut pm) = self.pm.take() else { return };
        pm.fill(tiny_skia::Color::TRANSPARENT);
        let root = Transform::from_scale(scale, scale);

        self.draw_zzz(&mut pm, scene, scale);
        self.draw_pet(&mut pm, scene, scale, root);
        self.draw_fx(&mut pm, scene, root);
        self.draw_bubble(&mut pm, scene, scale);
        self.draw_status(&mut pm, scene, scale);
        self.pm = Some(pm);
    }

    #[allow(dead_code)]
    pub fn pixmap(&self) -> Option<&Pixmap> { self.pm.as_ref() }

    fn draw_pet(&mut self, pm: &mut Pixmap, scene: &Scene, scale: f32, root: Transform) {
        let pet = root.pre_concat(xf_matrix(&scene.pet, PET_X + PET_W / 2.0, WIN_H as f32));
        let cx = PET_X + PET_W / 2.0;
        let cy = PET_Y + PET_H / 2.0;
        let body = pet
            .pre_translate(cx, cy)
            .pre_scale(scene.flip, 1.0)
            .pre_translate(-cx, -cy)
            .pre_translate(PET_X, PET_Y);
        let (sw, sh) = ((PET_W * scale).round() as u32, (PET_H * scale).round() as u32);
        let to_box = |p: &Pixmap| Transform::from_scale(PET_W / p.width() as f32, PET_H / p.height() as f32);
        let paint = PixmapPaint { quality: FilterQuality::Bilinear, ..Default::default() };

        let layers = sprites::layers_for(scene.face, &scene.costume, &scene.outfits);
        let base = self.sprites.asset(&layers.base, sw, sh);
        if let Some(base) = &base {
            if scene.glow {
                let key = format!("glow:{}", layers.base);
                let glow = self.sprites.get_or(&key, (sw, sh), || {
                    let (g, pad) = sprites::make_glow(base, 4.0 * scale, 0xfbbc04, 0.9)?;
                    // 把外扩量编码进宽度差里，画的时候再算回来
                    let _ = pad;
                    Some(g)
                });
                if let Some(g) = glow {
                    let pad = (g.width() as f32 - base.width() as f32) / 2.0;
                    let t = body.pre_concat(to_box(base)).pre_translate(-pad, -pad);
                    pm.draw_pixmap(0, 0, g.as_ref().as_ref(), &paint, t, None);
                }
            }
            pm.draw_pixmap(0, 0, base.as_ref().as_ref(), &paint, body.pre_concat(to_box(base)), None);
        }
        if layers.tail
            && let Some(t) = self.sprites.asset(sprites::TAIL, sw, sh)
        {
            {
                let m = body.pre_concat(xf_matrix(&scene.tail, 0.6 * PET_W, 0.797 * PET_H)).pre_concat(to_box(&t));
                pm.draw_pixmap(0, 0, t.as_ref().as_ref(), &paint, m, None);
            }
        }
        if layers.star
            && let Some(s) = self.sprites.asset(sprites::STAR, sw, sh)
        {
            {
                let m = body.pre_concat(xf_matrix(&scene.star, 0.395 * PET_W, 0.123 * PET_H)).pre_concat(to_box(&s));
                pm.draw_pixmap(0, 0, s.as_ref().as_ref(), &paint, m, None);
                if scene.star.bright > 1.001 {
                    // brightness(b) ≈ 再叠一层 (b-1) 的加色
                    let p = PixmapPaint { opacity: (scene.star.bright - 1.0).min(1.0), blend_mode: BlendMode::Plus, quality: FilterQuality::Bilinear };
                    pm.draw_pixmap(0, 0, s.as_ref().as_ref(), &p, m, None);
                }
            }
        }
        if let Some((ox, oy)) = layers.outfit_offset {
            for id in &scene.outfits {
                let path = format!("outfits/{id}.png");
                if let Some(o) = self.sprites.asset(&path, sw, sh) {
                    let m = body.pre_translate(ox / 400.0 * PET_W, oy / 587.0 * PET_H).pre_concat(to_box(&o));
                    pm.draw_pixmap(0, 0, o.as_ref().as_ref(), &paint, m, None);
                }
            }
        }
    }

    fn draw_fx(&mut self, pm: &mut Pixmap, scene: &Scene, root: Transform) {
        if scene.fx.is_empty() { return; }
        let pet = root.pre_concat(xf_matrix(&scene.pet, PET_X + PET_W / 2.0, WIN_H as f32)).pre_translate(PET_X, PET_Y);
        for f in &scene.fx {
            if f.xf.opacity <= 0.003 { continue; }
            // 元素盒：宽约 1em，高 = 行高（YaHei normal ≈ 1.32em），变换原点在盒中心
            let (bw, bh) = (f.size * 0.9, f.size * 1.32);
            let (cx, cy) = (f.x + bw / 2.0, f.y + bh / 2.0);
            let t = pet.pre_concat(xf_matrix(&f.xf, cx, cy)).pre_translate(cx, cy).pre_scale(f.size, f.size);
            shapes::draw_glyph(pm, f.shape, f.color, f.xf.opacity, t, f.size);
        }
    }

    fn glyph(&mut self, ch: char, px: f32) -> Option<Rc<text::Coverage>> {
        let k = (ch, (px * 4.0) as u32);
        if let Some((_, c)) = self.glyphs.iter().find(|(kk, _)| *kk == k) { return Some(c.clone()); }
        let c = Rc::new(self.text.raster(&ch.to_string(), px, true)?);
        if self.glyphs.len() > 8 { self.glyphs.remove(0); }
        self.glyphs.push((k, c.clone()));
        Some(c)
    }

    fn draw_zzz(&mut self, pm: &mut Pixmap, scene: &Scene, scale: f32) {
        for z in &scene.zzz {
            if z.opacity <= 0.003 { continue; }
            if let Some(c) = self.glyph(z.ch, z.size * scale) {
                text::blit(pm, &c, ((150.0 + z.x) * scale).round() as i32, ((120.0 + z.y) * scale).round() as i32, z.color, z.opacity, None);
            }
        }
    }

    // ------------------------------------------------------------ 气泡
    fn draw_bubble(&mut self, pm: &mut Pixmap, scene: &Scene, scale: f32) {
        let Some(b) = &scene.bubble else { self.bubble_rect = None; self.bubble_overflow = false; return };
        if b.opacity <= 0.003 || b.text.is_empty() { self.bubble_rect = None; self.bubble_overflow = false; return; }
        let key = format!("{}|{}|{:.3}", scale, b.text, b.scroll);
        if self.bubble.as_ref().is_none_or(|l| l.key != key) {
            self.bubble = self.build_bubble(&b.text, b.scroll, scale).map(|(pm, rect)| Layer { key, pm, x: (8.0 * scale).round() as i32, y: 0, rect });
        }
        if let Some(l) = &self.bubble {
            let p = PixmapPaint { opacity: b.opacity, ..Default::default() };
            pm.draw_pixmap(l.x, l.y, l.pm.as_ref(), &p, Transform::identity(), None);
            self.bubble_rect = Some(l.rect);
        }
    }

    /// 气泡：left/right 8px、border 2、padding 6/10 → 内容宽 200；13px 行高 1.35；内容最多 128 高
    fn build_bubble(&mut self, text: &str, scroll: f32, s: f32) -> Option<(Pixmap, Rect)> {
        let px = 13.0 * s;
        let lh = 13.0 * 1.35 * s;
        let content_w = 200.0 * s;
        let tkey = format!("{s}|{text}");
        if self.bubble_text.as_ref().is_none_or(|t| t.key != tkey) {
            let lines = self.text.wrap(text, content_w, px, false);
            let cell = self.text.cell_height(px, false);
            let mut out = Vec::new();
            for line in &lines {
                let lw = self.text.text_width(line, px, false);
                out.push((self.text.raster(line, px, false), 12.0 * s + (content_w - lw) / 2.0));
            }
            self.bubble_text = Some(BubbleText { key: tkey, lines: out, cell });
        }
        let bt = self.bubble_text.take()?;
        let full_h = bt.lines.len() as f32 * lh;
        let content_h = full_h.min(128.0 * s);
        self.bubble_overflow = full_h > content_h + 0.5;
        let box_w = 224.0 * s;
        let box_h = content_h + 16.0 * s;
        let mut pm = Pixmap::new(box_w.ceil() as u32 + 1, (box_h + 6.0 * s).ceil() as u32 + 1)?;
        draw_card(&mut pm, box_w, box_h, s, 1.0);
        let scroll_px = (scroll.clamp(0.0, 1.0) * (full_h - content_h).max(0.0)).round();
        let (top, bottom) = ((8.0 * s) as i32, (8.0 * s + content_h).round() as i32);
        for (i, (cov, x)) in bt.lines.iter().enumerate() {
            let y = 8.0 * s + i as f32 * lh - scroll_px;
            if y + lh < top as f32 || y > bottom as f32 { continue; }
            if let Some(c) = cov {
                let clip = Some((0, top, pm.width() as i32, bottom));
                text::blit(&mut pm, c, x.round() as i32, (y + (lh - bt.cell) / 2.0).round() as i32, C_TEXT, 1.0, clip);
            }
        }
        self.bubble_text = Some(bt);
        Some((pm, Rect { x: 8.0, y: 0.0, w: 224.0, h: box_h / s }))
    }

    // ------------------------------------------------------------ 状态卡
    fn draw_status(&mut self, pm: &mut Pixmap, scene: &Scene, scale: f32) {
        let Some(st) = &scene.status else { self.status_rect = None; return };
        if st.opacity <= 0.003 { self.status_rect = None; return; }
        let key = format!("{scale}|{st:?}").replace(&format!("opacity: {:?}", st.opacity), "");
        if self.status.as_ref().is_none_or(|l| l.key != key) {
            self.status = self.build_status(st, scale).map(|(pm, rect)| Layer { key, pm, x: (14.0 * scale).round() as i32, y: (46.0 * scale).round() as i32, rect });
        }
        if let Some(l) = &self.status {
            let p = PixmapPaint { opacity: st.opacity, ..Default::default() };
            pm.draw_pixmap(l.x, l.y, l.pm.as_ref(), &p, Transform::identity(), None);
            self.status_rect = Some(l.rect);
        }
    }

    /// 状态卡：top 46、左右 14、border 2、padding 8/10 → 内容宽 188；12px
    fn build_status(&mut self, st: &StatusView, s: f32) -> Option<(Pixmap, Rect)> {
        let px = 12.0 * s;
        let lh = self.text.cell_height(px, false).max(px * 1.2);
        let lhb = self.text.cell_height(px, true).max(px * 1.2);
        let box_w = 212.0 * s;
        let rows = 4 + st.moyu.is_some() as usize;
        let content_h = lhb + 4.0 * s + rows as f32 * lh + (rows as f32 - 1.0) * 3.0 * s + 3.0 * s;
        let box_h = content_h + 20.0 * s;
        let mut pm = Pixmap::new(box_w.ceil() as u32 + 1, (box_h + 6.0 * s).ceil() as u32 + 1)?;
        draw_card(&mut pm, box_w, box_h, s, 0.96);
        let x0 = 12.0 * s;
        let cw = 188.0 * s;
        let mut y = 10.0 * s;
        // 标题
        let title = format!("哈基米 · 认识第 {} 天", st.days);
        let tw = self.text.text_width(&title, px, true);
        self.text_at(&mut pm, &title, x0 + (cw - tw) / 2.0, y, lhb, px, true, C_TEXT);
        y += lhb + 4.0 * s;
        let bars = [("饱食", st.hunger, C_YELLOW), ("心情", st.mood, C_RED), ("精力", st.energy, C_GREEN)];
        for (label, v, color) in bars {
            self.text_at(&mut pm, label, x0, y, lh, px, false, C_TEXT);
            let bx = x0 + 34.0 * s;
            let bw = cw - 34.0 * s - 30.0 * s;
            let by = y + (lh - 8.0 * s) / 2.0;
            let id = Transform::identity();
            shapes::fill_rrect(&mut pm, bx, by, bw, 8.0 * s, 4.0 * s, 0xf1e6d6, 1.0, id);
            let fw = bw * (v.round().clamp(0.0, 100.0) / 100.0);
            if fw > 0.5 { shapes::fill_rrect(&mut pm, bx, by, fw, 8.0 * s, 4.0 * s, color, 1.0, id); }
            let n = format!("{}", v.round() as i64);
            let nw = self.text.text_width(&n, px, false);
            self.text_at(&mut pm, &n, x0 + cw - nw, y, lh, px, false, C_TEXT);
            y += lh + 3.0 * s;
        }
        self.text_at(&mut pm, "亲密", x0, y, lh, px, false, C_TEXT);
        self.text_at(&mut pm, &st.level, x0 + 34.0 * s, y, lh, px, true, C_BLUE);
        let n = format!("{}", st.affection);
        let nw = self.text.text_width(&n, px, false);
        self.text_at(&mut pm, &n, x0 + cw - nw, y, lh, px, false, C_TEXT);
        y += lh + 3.0 * s;
        if let Some(m) = st.moyu {
            self.text_at(&mut pm, "摸鱼", x0, y, lh, px, false, C_TEXT);
            self.text_at(&mut pm, &format!("今天 {m} 分钟"), x0 + 34.0 * s, y, lh, px, true, C_RED);
        }
        Some((pm, Rect { x: 14.0, y: 46.0, w: 212.0, h: box_h / s }))
    }

    #[allow(clippy::too_many_arguments)]
    fn text_at(&mut self, pm: &mut Pixmap, s: &str, x: f32, y: f32, lh: f32, px: f32, bold: bool, color: u32) {
        let cell = self.text.cell_height(px, bold);
        if let Some(c) = self.text.raster(s, px, bold) {
            text::blit(pm, &c, x.round() as i32, (y + (lh - cell) / 2.0).round() as i32, color, 1.0, None);
        }
    }
}

/// 原版卡片：圆角 12、2px #fbbc04 边、#fffaf0 底、下方三层四色 box-shadow（红 2/绿 4/蓝 6）
fn draw_card(pm: &mut Pixmap, w: f32, h: f32, s: f32, bg_alpha: f32) {
    let id = Transform::identity();
    let r = 12.0 * s;
    shapes::fill_rrect(pm, 0.0, 6.0 * s, w, h, r, C_BLUE, 1.0, id);
    shapes::fill_rrect(pm, 0.0, 4.0 * s, w, h, r, C_GREEN, 1.0, id);
    shapes::fill_rrect(pm, 0.0, 2.0 * s, w, h, r, C_RED, 1.0, id);
    shapes::fill_rrect(pm, 0.0, 0.0, w, h, r, C_BORDER, 1.0, id);
    // 内底：先挖空再填，半透明背景下边框不透出来
    if let Some(p) = shapes::rrect(2.0 * s, 2.0 * s, w - 4.0 * s, h - 4.0 * s, r - 2.0 * s) {
        let mut clear = shapes::paint(0, 1.0);
        clear.blend_mode = BlendMode::Clear;
        pm.fill_path(&p, &clear, tiny_skia::FillRule::Winding, id, None);
        pm.fill_path(&p, &shapes::paint(C_BG, bg_alpha), tiny_skia::FillRule::Winding, id, None);
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    #[test]
    fn transform_origin() {
        // 绕底边中点缩放：原点不动
        let m = xf_matrix(&Xf { sx: 1.5, sy: 0.5, ..Xf::ID }, 120.0, 360.0);
        let mut p = [tiny_skia::Point::from_xy(120.0, 360.0)];
        m.map_points(&mut p);
        assert!((p[0].x - 120.0).abs() < 1e-3 && (p[0].y - 360.0).abs() < 1e-3);
        let m = xf_matrix(&Xf { ty: -18.0, ..Xf::ID }, 0.0, 0.0);
        let mut p = [tiny_skia::Point::from_xy(5.0, 5.0)];
        m.map_points(&mut p);
        assert!((p[0].y + 13.0).abs() < 1e-3);
    }

    #[test]
    fn renders_and_skips_unchanged() {
        let mut r = Renderer::new();
        let mut sc = Scene::default();
        sc.bubble = Some(BubbleView { text: "哈基米上线啦喵～".into(), opacity: 1.0, scroll: 0.0 });
        let f = r.render(&sc, 1.5).expect("first frame");
        assert_eq!((f.w, f.h), (360, 540));
        assert!(f.bgra.as_chunks::<4>().0.iter().any(|p| p[3] == 255));
        assert!(r.render(&sc, 1.5).is_none());
        sc.pet.ty = -3.0;
        assert!(r.render(&sc, 1.5).is_some());
        assert!(r.bubble_rect.is_some());
    }

    /// 手动看效果：HAKIMI_SNAP=out.png cargo test snapshot -- --ignored
    #[test]
    #[ignore]
    fn snapshot() {
        let out = std::env::var("HAKIMI_SNAP").unwrap_or_else(|_| "snap.png".into());
        let mut r = Renderer::new();
        let mut sc = Scene::default();
        sc.outfits = vec!["scarf".into(), "glasses".into()];
        sc.glow = true;
        sc.star.bright = 1.2;
        sc.tail.rot = 6.0;
        sc.bubble = Some(BubbleView { text: "哈基米～哈基米～哈基米～哈基米哦南北绿豆～ 阿西嘎～阿西嘎～哈呀库奶龙～曼波！🎵 Hello world, this is a long English sentence.".into(), opacity: 1.0, scroll: 0.0 });
        sc.zzz = vec![ZGlyph { ch: 'z', size: 14.0, color: C_BLUE, x: 5.0, y: -10.0, opacity: 1.0 }, ZGlyph { ch: 'Z', size: 24.0, color: C_RED, x: 20.0, y: -40.0, opacity: 0.7 }];
        let shapes = [FxShape::Heart, FxShape::Note(0), FxShape::Note(1), FxShape::Note(2), FxShape::Spark, FxShape::Drop, FxShape::Anger, FxShape::Exclaim];
        sc.fx = shapes.iter().enumerate().map(|(i, &sh)| FxSprite { shape: sh, x: 10.0 + i as f32 * 22.0, y: 150.0, size: 20.0, xf: Xf::ID, color: [C_RED, C_BLUE, C_GREEN, C_YELLOW][i % 4] }).collect();
        sc.status = Some(StatusView { days: 12, hunger: 66.0, mood: 80.0, energy: 33.0, level: "好朋友".into(), affection: 88.0, moyu: Some(23), opacity: 1.0 });
        r.draw(&sc, 1.5);
        r.pixmap().unwrap().save_png(&out).unwrap();
        let mut sc2 = Scene::default();
        sc2.face = Face::Sit;
        sc2.costume = "maid".into();
        sc2.outfits = vec!["santa".into()];
        sc2.pet = Xf { rot: -6.0, ..Xf::ID };
        sc2.flip = -1.0;
        sc2.bubble = Some(BubbleView { text: "欢迎回来，主人喵～".into(), opacity: 0.8, scroll: 0.0 });
        r.draw(&sc2, 1.0);
        r.pixmap().unwrap().save_png(out.replace(".png", "2.png")).unwrap();
    }
}
