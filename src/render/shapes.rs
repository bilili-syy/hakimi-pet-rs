//! 矢量小图形：圆角矩形、特效字形（♥ ♪ ♫ ✦ 💧 💢 ！）。
//! 原版特效是文字字形（emoji 还是彩色的），这里用矢量画，不依赖字体。
use super::FxShape;
use tiny_skia::{Color, FillRule, Paint, Path, PathBuilder, Pixmap, Stroke, Transform};

pub fn rgb(c: u32, a: f32) -> Color {
    Color::from_rgba8(((c >> 16) & 255) as u8, ((c >> 8) & 255) as u8, (c & 255) as u8, (a.clamp(0.0, 1.0) * 255.0) as u8)
}

pub fn paint(c: u32, a: f32) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(rgb(c, a));
    p.anti_alias = true;
    p
}

pub fn rrect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let k = 0.552_284_8 * r;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

#[allow(clippy::too_many_arguments)]
pub fn fill_rrect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32, c: u32, a: f32, t: Transform) {
    if let Some(p) = rrect(x, y, w, h, r) {
        pm.fill_path(&p, &paint(c, a), FillRule::Winding, t, None);
    }
}

/// 以 (0,0) 为中心、字号 1 的图形路径
fn glyph_path(shape: FxShape) -> Option<Path> {
    let mut pb = PathBuilder::new();
    match shape {
        FxShape::Heart => {
            pb.move_to(0.0, 0.32);
            pb.cubic_to(-0.05, 0.27, -0.42, 0.02, -0.42, -0.15);
            pb.cubic_to(-0.42, -0.30, -0.31, -0.38, -0.21, -0.38);
            pb.cubic_to(-0.10, -0.38, -0.03, -0.31, 0.0, -0.24);
            pb.cubic_to(0.03, -0.31, 0.10, -0.38, 0.21, -0.38);
            pb.cubic_to(0.31, -0.38, 0.42, -0.30, 0.42, -0.15);
            pb.cubic_to(0.42, 0.02, 0.05, 0.27, 0.0, 0.32);
            pb.close();
        }
        FxShape::Spark => {
            let (o, i) = (0.42, 0.09);
            pb.move_to(0.0, -o);
            pb.quad_to(i, -i, o, 0.0);
            pb.quad_to(i, i, 0.0, o);
            pb.quad_to(-i, i, -o, 0.0);
            pb.quad_to(-i, -i, 0.0, -o);
            pb.close();
        }
        FxShape::Note(n) => {
            // 符头（斜椭圆近似）+ 符干 + 符尾/横梁
            let head = |pb: &mut PathBuilder, cx: f32, cy: f32| {
                pb.push_oval(tiny_skia::Rect::from_xywh(cx - 0.12, cy - 0.085, 0.24, 0.17).unwrap());
            };
            if n == 0 {
                head(&mut pb, -0.08, 0.25);
                pb.push_rect(tiny_skia::Rect::from_xywh(0.02, -0.38, 0.05, 0.63).unwrap());
                pb.move_to(0.07, -0.38);
                pb.cubic_to(0.12, -0.25, 0.30, -0.20, 0.22, 0.0);
                pb.cubic_to(0.24, -0.15, 0.14, -0.22, 0.07, -0.24);
                pb.close();
            } else {
                head(&mut pb, -0.26, 0.27);
                head(&mut pb, 0.20, 0.20);
                pb.push_rect(tiny_skia::Rect::from_xywh(-0.16, -0.30, 0.05, 0.57).unwrap());
                pb.push_rect(tiny_skia::Rect::from_xywh(0.30, -0.37, 0.05, 0.57).unwrap());
                pb.move_to(-0.16, -0.30);
                pb.line_to(0.35, -0.37);
                pb.line_to(0.35, -0.27);
                pb.line_to(-0.16, -0.20);
                pb.close();
                if n == 2 {
                    pb.move_to(-0.16, -0.14);
                    pb.line_to(0.35, -0.21);
                    pb.line_to(0.35, -0.13);
                    pb.line_to(-0.16, -0.06);
                    pb.close();
                }
            }
        }
        FxShape::Drop => {
            pb.move_to(0.0, -0.42);
            pb.cubic_to(0.08, -0.25, 0.30, -0.05, 0.30, 0.13);
            pb.cubic_to(0.30, 0.30, 0.16, 0.42, 0.0, 0.42);
            pb.cubic_to(-0.16, 0.42, -0.30, 0.30, -0.30, 0.13);
            pb.cubic_to(-0.30, -0.05, -0.08, -0.25, 0.0, -0.42);
            pb.close();
        }
        FxShape::Anger => {
            // 四个弯角
            for (sx, sy) in [(1.0f32, 1.0f32), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
                pb.move_to(sx * 0.08, sy * 0.38);
                pb.quad_to(sx * 0.10, sy * 0.12, sx * 0.38, sy * 0.08);
                pb.line_to(sx * 0.38, sy * 0.18);
                pb.quad_to(sx * 0.18, sy * 0.20, sx * 0.18, sy * 0.38);
                pb.close();
            }
        }
        FxShape::Exclaim => {
            pb.move_to(-0.07, -0.40);
            pb.line_to(0.07, -0.40);
            pb.line_to(0.04, 0.14);
            pb.line_to(-0.04, 0.14);
            pb.close();
            pb.push_circle(0.0, 0.29, 0.065);
        }
    }
    pb.finish()
}

/// 在 t（已经包含字号缩放、以字形中心为原点）下画一个特效字形：先白描边（原版 text-shadow），再填色
pub fn draw_glyph(pm: &mut Pixmap, shape: FxShape, color: u32, opacity: f32, t: Transform, size: f32) {
    let Some(path) = glyph_path(shape) else { return };
    let stroke = Stroke { width: 2.0 / size.max(1.0), ..Default::default() };
    pm.stroke_path(&path, &paint(0xFFFFFF, opacity * 0.85), &stroke, t, None);
    pm.fill_path(&path, &paint(color, opacity), FillRule::Winding, t, None);
    if shape == FxShape::Drop {
        // 高光
        let mut pb = PathBuilder::new();
        pb.push_oval(tiny_skia::Rect::from_xywh(-0.17, 0.02, 0.09, 0.18).unwrap());
        if let Some(p) = pb.finish() {
            pm.fill_path(&p, &paint(0xFFFFFF, opacity * 0.8), FillRule::Winding, t, None);
        }
    }
}
