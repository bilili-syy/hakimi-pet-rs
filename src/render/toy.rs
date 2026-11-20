//! 桌面小道具（单独的小分层窗口里画）：小鱼干 / 罐头 / 逗猫棒。全部矢量，不占资源文件。
//! 坐标系：TOY 逻辑像素见方，再整体乘 DPI 缩放。
use super::shapes::{fill_rrect, paint};
use crate::types::{Frame, ToyKind};
use tiny_skia::{FillRule, LineCap, Path, PathBuilder, Pixmap, Stroke, Transform};

/// 道具窗口边长（逻辑像素）
pub const TOY: f32 = 64.0;
/// 逗猫棒的毛球中心（光标就对在这里）
pub const TEASER_TIP: (f32, f32) = (18.0, 16.0);

fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32) -> Option<Path> {
    let k = 0.552_284_8;
    let mut pb = PathBuilder::new();
    pb.move_to(cx + rx, cy);
    pb.cubic_to(cx + rx, cy + ry * k, cx + rx * k, cy + ry, cx, cy + ry);
    pb.cubic_to(cx - rx * k, cy + ry, cx - rx, cy + ry * k, cx - rx, cy);
    pb.cubic_to(cx - rx, cy - ry * k, cx - rx * k, cy - ry, cx, cy - ry);
    pb.cubic_to(cx + rx * k, cy - ry, cx + rx, cy - ry * k, cx + rx, cy);
    pb.close();
    pb.finish()
}

fn fill(pm: &mut Pixmap, p: Option<Path>, c: u32, a: f32, t: Transform) {
    if let Some(p) = p {
        pm.fill_path(&p, &paint(c, a), FillRule::Winding, t, None);
    }
}

fn stroke(pm: &mut Pixmap, p: Option<Path>, c: u32, w: f32, t: Transform) {
    if let Some(p) = p {
        let s = Stroke { width: w, line_cap: LineCap::Round, ..Default::default() };
        pm.stroke_path(&p, &paint(c, 1.0), &s, t, None);
    }
}

fn fish(pm: &mut Pixmap, t: Transform) {
    // 斜着放的小鱼干：身子、尾巴、鱼鳍、眼睛、几道烤痕
    let t = t.pre_concat(Transform::from_rotate_at(-18.0, 32.0, 34.0));
    let outline = 0x6b4226;
    let mut tail = PathBuilder::new();
    tail.move_to(44.0, 34.0);
    tail.line_to(58.0, 24.0);
    tail.quad_to(54.0, 34.0, 58.0, 44.0);
    tail.close();
    let tail = tail.finish();
    fill(pm, tail.clone(), 0xc98a4b, 1.0, t);
    stroke(pm, tail, outline, 1.6, t);
    let body = ellipse(28.0, 34.0, 20.0, 10.0);
    fill(pm, body.clone(), 0xd9a066, 1.0, t);
    fill(pm, ellipse(28.0, 38.0, 16.0, 5.0), 0xf3d7a8, 0.9, t);
    stroke(pm, body, outline, 1.6, t);
    let mut fin = PathBuilder::new();
    fin.move_to(24.0, 25.0);
    fin.quad_to(30.0, 18.0, 36.0, 26.0);
    stroke(pm, fin.finish(), outline, 1.6, t);
    for x in [26.0, 32.0, 38.0] {
        let mut pb = PathBuilder::new();
        pb.move_to(x, 29.0);
        pb.quad_to(x - 2.0, 34.0, x, 39.0);
        stroke(pm, pb.finish(), 0xa8692f, 1.2, t);
    }
    fill(pm, ellipse(15.5, 32.0, 2.6, 2.6), 0x2b1a10, 1.0, t);
    fill(pm, ellipse(14.8, 31.2, 0.9, 0.9), 0xffffff, 1.0, t);
}

fn can(pm: &mut Pixmap, t: Transform) {
    // 猫罐头：圆柱 + 四色标签 + 小鱼图案 + 拉环
    fill(pm, ellipse(32.0, 52.0, 20.0, 6.0), 0x8d99a6, 1.0, t);
    fill_rrect(pm, 12.0, 22.0, 40.0, 30.0, 2.0, 0xb8c2cc, 1.0, t);
    let colors = [0xea4335, 0xfbbc04, 0x34a853, 0x4285f4];
    for (i, c) in colors.iter().enumerate() {
        fill_rrect(pm, 12.0 + i as f32 * 10.0, 28.0, 10.0, 18.0, 0.0, *c, 1.0, t);
    }
    fill(pm, ellipse(32.0, 37.0, 9.0, 5.0), 0xffffff, 0.95, t);
    let mut tail = PathBuilder::new();
    tail.move_to(39.0, 37.0);
    tail.line_to(44.0, 33.0);
    tail.line_to(44.0, 41.0);
    tail.close();
    fill(pm, tail.finish(), 0xffffff, 0.95, t);
    fill(pm, ellipse(28.0, 36.0, 1.2, 1.2), 0x2b1a10, 1.0, t);
    fill(pm, ellipse(32.0, 22.0, 20.0, 6.0), 0xdfe5ea, 1.0, t);
    stroke(pm, ellipse(32.0, 22.0, 20.0, 6.0), 0x8d99a6, 1.4, t);
    stroke(pm, ellipse(32.0, 22.0, 6.0, 2.5), 0x8d99a6, 1.6, t);
}

fn teaser(pm: &mut Pixmap, t: Transform) {
    // 逗猫棒：杆子从右下伸到左上，末端一根绳挂着毛球和羽毛
    let (tx, ty) = TEASER_TIP;
    let mut stick = PathBuilder::new();
    stick.move_to(62.0, 62.0);
    stick.line_to(40.0, 36.0);
    stroke(pm, stick.finish(), 0x8b5a2b, 3.0, t);
    let mut line = PathBuilder::new();
    line.move_to(40.0, 36.0);
    line.quad_to(30.0, 20.0, tx + 4.0, ty + 2.0);
    stroke(pm, line.finish(), 0x5a3b2e, 1.0, t);
    let feathers = [(0xea4335, -40.0), (0xfbbc04, -10.0), (0x34a853, 20.0), (0x4285f4, 50.0)];
    for (c, a) in feathers {
        let ft = t.pre_concat(Transform::from_rotate_at(a + 180.0, tx, ty));
        fill(pm, ellipse(tx, ty - 9.0, 3.2, 9.0), c, 0.95, ft);
    }
    fill(pm, ellipse(tx, ty, 6.5, 6.5), 0xff7eb6, 1.0, t);
    fill(pm, ellipse(tx - 2.0, ty - 2.0, 2.0, 2.0), 0xffffff, 0.8, t);
}

/// 画一帧道具（预乘 BGRA）
pub fn render(kind: ToyKind, scale: f32) -> Frame {
    let n = (TOY * scale).round().max(8.0) as u32;
    let Some(mut pm) = Pixmap::new(n, n) else { return Frame { w: 0, h: 0, bgra: Vec::new() } };
    let t = Transform::from_scale(scale, scale);
    match kind {
        ToyKind::Fish => fish(&mut pm, t),
        ToyKind::Can => can(&mut pm, t),
        ToyKind::Teaser => teaser(&mut pm, t),
    }
    let mut bgra = pm.take();
    for p in bgra.as_chunks_mut::<4>().0.iter_mut() {
        p.swap(0, 2);
    }
    Frame { w: n, h: n, bgra }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toys_have_pixels() {
        for k in [ToyKind::Fish, ToyKind::Can, ToyKind::Teaser] {
            let f = render(k, 1.5);
            assert_eq!((f.w, f.h), (96, 96));
            assert!(f.bgra.as_chunks::<4>().0.iter().filter(|p| p[3] > 200).count() > 200, "{k:?}");
        }
    }
}
