//! 快捷面板的 `tiny-skia` 抗锯齿圆角图形与全彩矢量小图标（零图片资源体积，任意 DPI 下清晰无锯齿）。
use tiny_skia::{Color, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Stroke, Transform};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelIcon {
    Paw,
    RoleMask,
    Chat,
    Fish,
    Honey,
    Teaser,
    Dance,
    Climb,
    Pomo,
    Roast,
    Speaker(bool),
    MusicNote(bool),
}

pub fn rgba(rgb: u32, a: f32) -> Color {
    Color::from_rgba8(
        ((rgb >> 16) & 0xff) as u8,
        ((rgb >> 8) & 0xff) as u8,
        (rgb & 0xff) as u8,
        (a.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

fn paint(rgb: u32, a: f32) -> Paint<'static> {
    let mut p = Paint { anti_alias: true, ..Default::default() };
    p.set_color(rgba(rgb, a));
    p
}

pub fn rrect_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    if w <= 0.5 || h <= 0.5 {
        return None;
    }
    let r = r.min(w * 0.5).min(h * 0.5).max(0.0);
    let (x1, y1) = (x + w, y + h);
    let k = r * 0.4477;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x1 - r, y);
    if r > 0.0 {
        pb.cubic_to(x1 - k, y, x1, y + k, x1, y + r);
    }
    pb.line_to(x1, y1 - r);
    if r > 0.0 {
        pb.cubic_to(x1, y1 - k, x1 - k, y1, x1 - r, y1);
    }
    pb.line_to(x + r, y1);
    if r > 0.0 {
        pb.cubic_to(x + k, y1, x, y1 - k, x, y1 - r);
    }
    pb.line_to(x, y + r);
    if r > 0.0 {
        pb.cubic_to(x, y + k, x + k, y, x + r, y);
    }
    pb.close();
    pb.finish()
}

pub fn ellipse_path(cx: f32, cy: f32, rx: f32, ry: f32) -> Option<Path> {
    if rx <= 0.2 || ry <= 0.2 {
        return None;
    }
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

pub fn fill_rrect(pm: &mut Pixmap, r: [f32; 4], rad: f32, fill_rgb: u32, stroke: Option<(u32, f32)>, xf: Transform) {
    let inset = stroke.map(|s| s.1 * 0.5).unwrap_or(0.0);
    let Some(path) = rrect_path(r[0] + inset, r[1] + inset, r[2] - inset * 2.0, r[3] - inset * 2.0, (rad - inset).max(0.0)) else {
        return;
    };
    pm.fill_path(&path, &paint(fill_rgb, 1.0), FillRule::Winding, xf, None);
    if let Some((sc, sw)) = stroke {
        let st = Stroke { width: sw, ..Default::default() };
        pm.stroke_path(&path, &paint(sc, 1.0), &st, xf, None);
    }
}

pub fn fill_circle(pm: &mut Pixmap, cx: f32, cy: f32, r: f32, fill_rgb: u32, stroke: Option<(u32, f32)>, xf: Transform) {
    let Some(path) = ellipse_path(cx, cy, r, r) else { return };
    pm.fill_path(&path, &paint(fill_rgb, 1.0), FillRule::Winding, xf, None);
    if let Some((sc, sw)) = stroke {
        let st = Stroke { width: sw, ..Default::default() };
        pm.stroke_path(&path, &paint(sc, 1.0), &st, xf, None);
    }
}

pub fn hline(pm: &mut Pixmap, x0: f32, x1: f32, y: f32, rgb: u32, width: f32, xf: Transform) {
    let mut pb = PathBuilder::new();
    pb.move_to(x0, y);
    pb.line_to(x1, y);
    if let Some(p) = pb.finish() {
        let st = Stroke { width, ..Default::default() };
        pm.stroke_path(&p, &paint(rgb, 1.0), &st, xf, None);
    }
}

fn fill_opt(pm: &mut Pixmap, p: Option<Path>, rgb: u32, a: f32, xf: Transform) {
    if let Some(p) = p {
        pm.fill_path(&p, &paint(rgb, a), FillRule::Winding, xf, None);
    }
}

fn stroke_opt(pm: &mut Pixmap, p: Option<Path>, rgb: u32, w: f32, xf: Transform) {
    if let Some(p) = p {
        let st = Stroke { width: w, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Default::default() };
        pm.stroke_path(&p, &paint(rgb, 1.0), &st, xf, None);
    }
}

/// 在逻辑坐标 `(cx, cy)` 中心绘制全彩矢量图标：`dpi_s` 为窗口 DPI 缩放，`icon_s` 为图标自身缩放倍数
pub fn draw_icon(pm: &mut Pixmap, kind: PanelIcon, cx: f32, cy: f32, dpi_s: f32, icon_s: f32) {
    // 先按窗口 DPI 定位到 (cx, cy)，再只缩放图标局部坐标 [-12..12]
    let xf = Transform::from_scale(dpi_s, dpi_s).pre_translate(cx, cy).pre_scale(icon_s, icon_s);
    match kind {
        PanelIcon::Paw => {
            fill_opt(pm, ellipse_path(0.0, 2.8, 5.2, 4.0), 0xf5a623, 1.0, xf);
            for (bx, by) in [(-5.6, -2.5), (-2.0, -5.2), (2.0, -5.2), (5.6, -2.5)] {
                fill_opt(pm, ellipse_path(bx, by, 2.1, 2.5), 0xf5a623, 1.0, xf);
            }
            fill_opt(pm, ellipse_path(0.0, 3.2, 2.8, 1.9), 0xffdf9e, 0.85, xf);
        }
        PanelIcon::RoleMask => {
            fill_opt(pm, rrect_path(-7.5, -5.5, 9.5, 9.5, 3.2), 0x5ba4f5, 1.0, xf);
            fill_opt(pm, rrect_path(-1.5, -3.5, 9.5, 9.5, 3.2), 0xf5a623, 1.0, xf);
            for (ex, ey) in [(1.5, 0.0), (5.0, 0.0)] {
                fill_opt(pm, ellipse_path(ex, ey, 1.1, 1.1), 0xffffff, 1.0, xf);
            }
        }
        PanelIcon::Chat => {
            let mut tail = PathBuilder::new();
            tail.move_to(-4.0, 4.5);
            tail.line_to(-7.5, 8.2);
            tail.line_to(-1.0, 5.5);
            tail.close();
            fill_opt(pm, tail.finish(), 0x62aef7, 1.0, xf);
            fill_opt(pm, ellipse_path(0.5, -0.8, 9.2, 7.0), 0x62aef7, 1.0, xf);
            fill_opt(pm, ellipse_path(0.5, -1.2, 7.8, 5.6), 0x8cc6ff, 0.45, xf);
            for dx in [-3.6, 0.5, 4.6] {
                fill_opt(pm, ellipse_path(dx, -0.8, 1.35, 1.35), 0xffffff, 1.0, xf);
            }
        }
        PanelIcon::Fish => {
            let t = xf.pre_rotate(-14.0);
            let mut tail = PathBuilder::new();
            tail.move_to(5.0, 0.0);
            tail.line_to(10.5, -4.6);
            tail.quad_to(8.5, 0.0, 10.5, 4.6);
            tail.close();
            let tp = tail.finish();
            fill_opt(pm, tp.clone(), 0xc98a4b, 1.0, t);
            stroke_opt(pm, tp, 0x7a4a28, 1.1, t);
            let body = ellipse_path(-1.8, 0.0, 7.8, 4.6);
            fill_opt(pm, body.clone(), 0xe2a866, 1.0, t);
            fill_opt(pm, ellipse_path(-1.8, 1.6, 6.0, 2.2), 0xf8dfb8, 0.9, t);
            stroke_opt(pm, body, 0x7a4a28, 1.1, t);
            for gx in [-2.5, 0.5, 3.2] {
                let mut pb = PathBuilder::new();
                pb.move_to(gx, -2.3);
                pb.line_to(gx - 0.8, 2.3);
                stroke_opt(pm, pb.finish(), 0xa8692f, 1.0, t);
            }
            fill_opt(pm, ellipse_path(-6.2, -0.8, 1.3, 1.3), 0x2b1a10, 1.0, t);
        }
        PanelIcon::Honey => {
            // 琥珀色蜂蜜罐 + 金黄溢出糖霜 + 小木盖
            fill_opt(pm, rrect_path(-6.8, -3.5, 13.6, 11.5, 4.8), 0xf5a623, 1.0, xf);
            fill_opt(pm, rrect_path(-5.0, -1.5, 10.0, 7.2, 3.2), 0xffca4b, 0.75, xf);
            // 罐口蜂蜜滴落花边
            fill_opt(pm, rrect_path(-7.2, -5.0, 14.4, 3.4, 1.7), 0xffe07d, 1.0, xf);
            for dx in [-4.2, 0.2, 4.0] {
                fill_opt(pm, ellipse_path(dx, -1.5, 1.6, 2.2), 0xffe07d, 1.0, xf);
            }
            fill_opt(pm, rrect_path(-5.6, -7.8, 11.2, 3.0, 1.4), 0x8d5524, 1.0, xf);
            fill_opt(pm, ellipse_path(0.0, -8.5, 2.0, 1.3), 0xa66a38, 1.0, xf);
        }
        PanelIcon::Teaser => {
            // 斜杆 + 粉红毛球 + 蓝绿羽毛
            let mut rod = PathBuilder::new();
            rod.move_to(-8.5, 8.5);
            rod.line_to(2.5, -2.5);
            stroke_opt(pm, rod.finish(), 0x9c6b43, 2.0, xf);
            let mut string = PathBuilder::new();
            string.move_to(2.5, -2.5);
            string.quad_to(6.5, -6.5, 6.2, -1.5);
            stroke_opt(pm, string.finish(), 0xcab59f, 1.1, xf);
            // 两片小羽毛
            let f1 = xf.pre_translate(7.5, 2.5).pre_rotate(28.0);
            fill_opt(pm, ellipse_path(0.0, 0.0, 1.9, 4.6), 0x4fc3f7, 1.0, f1);
            let f2 = xf.pre_translate(4.5, 3.5).pre_rotate(-18.0);
            fill_opt(pm, ellipse_path(0.0, 0.0, 1.8, 4.2), 0xffb74d, 1.0, f2);
            // 毛绒球
            fill_opt(pm, ellipse_path(5.8, -2.0, 4.0, 4.0), 0xff6b8b, 1.0, xf);
            fill_opt(pm, ellipse_path(4.6, -3.2, 1.4, 1.4), 0xffb2c3, 0.9, xf);
        }
        PanelIcon::Dance => {
            // 欢快双八分音符 + 小四角星
            fill_opt(pm, ellipse_path(-5.2, 5.2, 3.0, 2.3), 0xec5b86, 1.0, xf);
            fill_opt(pm, ellipse_path(3.2, 3.6, 3.0, 2.3), 0xec5b86, 1.0, xf);
            let mut stems = PathBuilder::new();
            stems.move_to(-2.5, 5.0);
            stems.line_to(-2.5, -5.5);
            stems.line_to(5.8, -7.2);
            stems.line_to(5.8, 3.4);
            stroke_opt(pm, stems.finish(), 0xec5b86, 2.1, xf);
            // 黄色小闪星
            let mut star = PathBuilder::new();
            let (sx, sy) = (-7.2, -4.5);
            star.move_to(sx, sy - 3.2);
            star.quad_to(sx, sy, sx + 3.2, sy);
            star.quad_to(sx, sy, sx, sy + 3.2);
            star.quad_to(sx, sy, sx - 3.2, sy);
            star.quad_to(sx, sy, sx, sy - 3.2);
            star.close();
            fill_opt(pm, star.finish(), 0xffb300, 1.0, xf);
        }
        PanelIcon::Climb => {
            // 小窗口卡片 + 趴在窗沿上的两只猫爪与猫耳剪影
            fill_opt(pm, rrect_path(-8.5, -3.5, 17.0, 12.0, 2.6), 0xffffff, 1.0, xf);
            stroke_opt(pm, rrect_path(-8.5, -3.5, 17.0, 12.0, 2.6), 0x8cb8e8, 1.4, xf);
            fill_opt(pm, rrect_path(-8.5, -3.5, 17.0, 3.8, 2.2), 0x64b5f6, 1.0, xf);
            // 窗顶探出的小橘猫头与两只白爪爪
            let mut ears = PathBuilder::new();
            ears.move_to(-4.2, -3.5);
            ears.line_to(-3.2, -8.2);
            ears.line_to(-0.8, -5.8);
            ears.line_to(0.8, -5.8);
            ears.line_to(3.2, -8.2);
            ears.line_to(4.2, -3.5);
            ears.close();
            fill_opt(pm, ears.finish(), 0xf5a623, 1.0, xf);
            fill_opt(pm, ellipse_path(-3.8, -3.0, 2.1, 1.6), 0xfff3e0, 1.0, xf);
            fill_opt(pm, ellipse_path(3.8, -3.0, 2.1, 1.6), 0xfff3e0, 1.0, xf);
            stroke_opt(pm, ellipse_path(-3.8, -3.0, 2.1, 1.6), 0xd48817, 1.0, xf);
            stroke_opt(pm, ellipse_path(3.8, -3.0, 2.1, 1.6), 0xd48817, 1.0, xf);
        }
        PanelIcon::Pomo => {
            // 红番茄 + 绿叶蒂 + 白色指针
            fill_opt(pm, ellipse_path(0.0, 1.4, 8.2, 7.2), 0xef5350, 1.0, xf);
            fill_opt(pm, ellipse_path(-2.5, -0.8, 3.2, 2.2), 0xff8a80, 0.45, xf);
            let mut leaf = PathBuilder::new();
            leaf.move_to(0.0, -5.2);
            leaf.quad_to(-4.5, -8.5, -6.2, -4.5);
            leaf.quad_to(-2.2, -4.2, 0.0, -5.2);
            leaf.move_to(0.0, -5.2);
            leaf.quad_to(4.5, -8.5, 6.2, -4.5);
            leaf.quad_to(2.2, -4.2, 0.0, -5.2);
            fill_opt(pm, leaf.finish(), 0x43a047, 1.0, xf);
            let mut hands = PathBuilder::new();
            hands.move_to(0.0, -2.0);
            hands.line_to(0.0, 1.8);
            hands.line_to(3.2, 3.2);
            stroke_opt(pm, hands.finish(), 0xffffff, 1.6, xf);
        }
        PanelIcon::Roast => {
            // 一双灵动吃瓜大眼睛
            for ex in [-4.3, 4.3] {
                let eo = ellipse_path(ex, 0.0, 4.1, 5.8);
                fill_opt(pm, eo.clone(), 0xffffff, 1.0, xf);
                stroke_opt(pm, eo, 0xb89f88, 1.2, xf);
                fill_opt(pm, ellipse_path(ex - 0.8, 0.3, 2.2, 3.0), 0x3a2416, 1.0, xf);
                fill_opt(pm, ellipse_path(ex - 1.5, -1.1, 0.85, 0.85), 0xffffff, 1.0, xf);
            }
        }
        PanelIcon::Speaker(muted) => {
            let col = if muted { 0xb59f8c } else { 0x6b4630 };
            let mut spk = PathBuilder::new();
            spk.move_to(-6.5, -2.5);
            spk.line_to(-3.2, -2.5);
            spk.line_to(1.2, -6.2);
            spk.line_to(1.2, 6.2);
            spk.line_to(-3.2, 2.5);
            spk.line_to(-6.5, 2.5);
            spk.close();
            fill_opt(pm, spk.finish(), col, 1.0, xf);
            if muted {
                let mut x_mark = PathBuilder::new();
                x_mark.move_to(3.5, -3.0);
                x_mark.line_to(8.0, 3.0);
                x_mark.move_to(8.0, -3.0);
                x_mark.line_to(3.5, 3.0);
                stroke_opt(pm, x_mark.finish(), 0xe05245, 1.7, xf);
            } else {
                let mut w1 = PathBuilder::new();
                w1.move_to(3.6, -3.2);
                w1.quad_to(6.2, 0.0, 3.6, 3.2);
                stroke_opt(pm, w1.finish(), 0xf5a623, 1.6, xf);
                let mut w2 = PathBuilder::new();
                w2.move_to(5.8, -5.4);
                w2.quad_to(9.5, 0.0, 5.8, 5.4);
                stroke_opt(pm, w2.finish(), 0xf5a623, 1.4, xf);
            }
        }
        PanelIcon::MusicNote(muted) => {
            let col = if muted { 0xb59f8c } else { 0x6b4630 };
            let accent = if muted { 0xb59f8c } else { 0xf5a623 };
            fill_opt(pm, ellipse_path(-3.6, 3.8, 2.4, 1.9), col, 1.0, xf);
            fill_opt(pm, ellipse_path(2.8, 2.6, 2.4, 1.9), accent, 1.0, xf);
            let mut st = PathBuilder::new();
            st.move_to(-1.5, 3.6);
            st.line_to(-1.5, -4.6);
            st.line_to(4.9, -5.8);
            st.line_to(4.9, 2.4);
            stroke_opt(pm, st.finish(), col, 1.7, xf);
        }
    }
}

/// 将不透明 RGBA `Pixmap` 一次性贴到 GDI `HDC` 上
pub unsafe fn blit_pixmap_to_dc(hdc: windows::Win32::Graphics::Gdi::HDC, pm: &Pixmap) {
    use windows::Win32::Graphics::Gdi::{BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, SetDIBitsToDevice};
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let mut bgra = pm.data().to_vec();
    for px in bgra.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            ..Default::default()
        },
        ..Default::default()
    };
    unsafe {
        SetDIBitsToDevice(hdc, 0, 0, w as u32, h as u32, 0, 0, 0, h as u32, bgra.as_ptr() as *const _, &bi, DIB_RGB_COLORS);
    }
}
