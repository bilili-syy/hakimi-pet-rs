//! 设置窗口的自绘外观：控件配色、圆角、导航栏和按钮的 owner-draw
use super::*;

pub(super) fn ctl_color(hdc: HDC, ctl: HWND) -> LRESULT {
    let id = unsafe { windows::Win32::UI::WindowsAndMessaging::GetDlgCtrlID(ctl) };
    // 输入框（含禁用/只读时走 CTLCOLORSTATIC 的情况）：白底，禁用时奶油底配灰字
    if is_edit(ctl) {
        let on = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(ctl).as_bool() };
        unsafe {
            SetTextColor(hdc, if on { TEXT } else { GREY });
            SetBkColor(hdc, if on { FIELD } else { BG });
        }
        return LRESULT(if on { FIELD_BRUSH.with(|b| b.get()) } else { BRUSH.with(|b| b.get()) });
    }
    let colors = RESULT_COLOR.with(|c| c.get());
    let color = match id {
        ID_KEY_LINK | ID_TTS_LINK => LINK,
        ID_NOTE => GREY,
        ID_MEMO_MSG => OK_GREEN,
        ID_TEST_RESULT | ID_PLAY_RESULT => {
            let v = colors[if id == ID_TEST_RESULT { 0 } else { 1 }];
            if v > 0 { OK_GREEN } else if v < 0 { ERR_RED } else { GREY }
        }
        _ if (ID_HINT0..ID_HINT0 + 100).contains(&id) => GREY,
        _ => TEXT,
    };
    unsafe {
        SetTextColor(hdc, color);
        SetBkColor(hdc, BG);
    }
    LRESULT(BRUSH.with(|b| b.get()))
}

// ---------------------------------------------------------------- 自绘外观

/// 界面字体：有思源黑体就用（和气泡同一套），没有退回微软雅黑 UI
pub(super) fn ui_faces() -> (&'static str, &'static str) {
    static F: std::sync::LazyLock<(&'static str, &'static str)> = std::sync::LazyLock::new(|| {
        if font_installed("Noto Sans SC") { ("Noto Sans SC", "Noto Sans SC") } else { ("Microsoft YaHei UI", "Microsoft YaHei UI") }
    });
    *F
}

/// 系统里有没有这个字体（建一个看 GDI 实际给的是不是它）
pub(super) fn font_installed(name: &str) -> bool {
    use windows::Win32::Graphics::Gdi::{CreateCompatibleDC, DeleteDC, GetTextFaceW, SelectObject};
    unsafe {
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            return false;
        }
        let face = wide(name);
        let f = CreateFontW(-16, 0, 0, 0, 400, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, 0, PCWSTR(face.as_ptr()));
        let old = SelectObject(dc, f.into());
        let mut buf = [0u16; 64];
        let n = GetTextFaceW(dc, Some(&mut buf)).max(0) as usize;
        SelectObject(dc, old);
        let _ = DeleteObject(f.into());
        let _ = DeleteDC(dc);
        String::from_utf16_lossy(&buf[..n.min(64)]).trim_end_matches('\0').eq_ignore_ascii_case(name)
    }
}

pub(super) fn is_edit(h: HWND) -> bool {
    let mut buf = [0u16; 16];
    let n = unsafe { windows::Win32::UI::WindowsAndMessaging::GetClassNameW(h, &mut buf) }.max(0) as usize;
    String::from_utf16_lossy(&buf[..n]).eq_ignore_ascii_case("Edit")
}

/// Win11 标题栏染成导航栏的奶油色（老系统不支持就算了）
pub(super) fn style_caption(hwnd: HWND) {
    use windows::Win32::Graphics::Dwm::{DWMWINDOWATTRIBUTE, DwmSetWindowAttribute};
    for (attr, c) in [(35, SIDE), (36, TEXT), (34, LINE)] {
        unsafe {
            let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(attr), &c as *const COLORREF as *const _, 4);
        }
    }
}

pub(super) fn cref(c: COLORREF) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8((c.0 & 0xff) as u8, (c.0 >> 8 & 0xff) as u8, (c.0 >> 16 & 0xff) as u8, 255)
}

/// 抗锯齿圆角矩形：在 tiny-skia 里画好再贴到 DC 上（GDI 的 RoundRect 边缘有锯齿）。
/// rc 是设备像素；bg 是四个角外面的底色
pub(super) fn aa_round(hdc: HDC, rc: RECT, radius: f32, bg: COLORREF, fill: COLORREF, stroke: Option<(COLORREF, f32)>) {
    use windows::Win32::Graphics::Gdi::{BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, SetDIBitsToDevice};
    let (w, h) = ((rc.right - rc.left).max(1) as u32, (rc.bottom - rc.top).max(1) as u32);
    let Some(mut pm) = tiny_skia::Pixmap::new(w, h) else { return };
    pm.fill(cref(bg));
    let inset = stroke.map(|s| s.1 / 2.0).unwrap_or(0.0);
    let (x0, y0, x1, y1) = (inset, inset, w as f32 - inset, h as f32 - inset);
    let r = radius.min((x1 - x0) / 2.0).min((y1 - y0) / 2.0);
    let k = r * 0.4477; // 圆弧的三次贝塞尔近似
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(x0 + r, y0);
    pb.line_to(x1 - r, y0);
    pb.cubic_to(x1 - k, y0, x1, y0 + k, x1, y0 + r);
    pb.line_to(x1, y1 - r);
    pb.cubic_to(x1, y1 - k, x1 - k, y1, x1 - r, y1);
    pb.line_to(x0 + r, y1);
    pb.cubic_to(x0 + k, y1, x0, y1 - k, x0, y1 - r);
    pb.line_to(x0, y0 + r);
    pb.cubic_to(x0, y0 + k, x0 + k, y0, x0 + r, y0);
    pb.close();
    let Some(path) = pb.finish() else { return };
    let mut paint = tiny_skia::Paint { anti_alias: true, ..Default::default() };
    paint.set_color(cref(fill));
    pm.fill_path(&path, &paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
    if let Some((c, wd)) = stroke {
        paint.set_color(cref(c));
        let st = tiny_skia::Stroke { width: wd, ..Default::default() };
        pm.stroke_path(&path, &paint, &st, tiny_skia::Transform::identity(), None);
    }
    // RGBA（不透明，预乘无影响）→ BGRA
    let mut bgra = pm.take();
    for p in bgra.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
    }
    let bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32),
            biPlanes: 1,
            biBitCount: 32,
            ..Default::default()
        },
        ..Default::default()
    };
    unsafe {
        SetDIBitsToDevice(hdc, rc.left, rc.top, w, h, 0, 0, 0, h, bgra.as_ptr() as *const _, &bi, DIB_RGB_COLORS);
    }
}

pub(super) fn fill(hdc: HDC, rc: RECT, c: COLORREF) {
    let cached = if c == BG {
        BRUSH.with(|b| b.get())
    } else if c == SIDE {
        SIDE_BRUSH.with(|b| b.get())
    } else if c == FIELD {
        FIELD_BRUSH.with(|b| b.get())
    } else {
        0
    };
    unsafe {
        if cached != 0 {
            windows::Win32::Graphics::Gdi::FillRect(hdc, &rc, HBRUSH(cached as *mut _));
        } else {
            let b = CreateSolidBrush(c);
            windows::Win32::Graphics::Gdi::FillRect(hdc, &rc, b);
            let _ = DeleteObject(HGDIOBJ(b.0));
        }
    }
}

/// 背景：内容区奶油底、左侧导航栏、底部分隔线、每个输入框的圆角外框
pub(super) fn paint_bg(hwnd: HWND, hdc: HDC) {
    let mut cr = RECT::default();
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut cr);
    }
    let (scale, frames, page) = DLG.with(|c| match c.try_borrow() {
        Ok(g) => g.as_ref().map(|d| (d.scale, d.frames.iter().map(|f| (f.0, f.1, f.2)).collect::<Vec<_>>(), d.page)),
        Err(_) => None,
    })
    .unwrap_or((1.0, Vec::new(), 0));
    let px = |v: f32| (v * scale).round() as i32;
    fill(hdc, cr, BG);
    let side = px(SIDE_W);
    fill(hdc, RECT { right: side, ..cr }, SIDE);
    fill(hdc, RECT { left: side, right: side + px(1.0).max(1), ..cr }, LINE);
    let fy = px(FOOT_Y);
    fill(hdc, RECT { left: side + px(16.0), top: fy, right: cr.right - px(16.0), bottom: fy + px(1.0).max(1) }, LINE);
    let focus = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() };
    for (r, h, p) in frames {
        if p != 0 && p != page {
            continue;
        }
        let rc = RECT { left: px(r[0]), top: px(r[1]), right: px(r[0] + r[2]), bottom: px(r[1] + r[3]) };
        let on = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(h).as_bool() };
        let (stroke, wd) = if focus == h { (ACCENT, 1.6 * scale) } else { (LINE, 1.0 * scale) };
        aa_round(hdc, rc, 7.0 * scale, BG, if on { FIELD } else { BG }, Some((stroke, wd)));
    }
}

/// 自绘：左侧导航（选中项橙色圆角块）和按钮（主按钮橙色，其他奶油色描边）
pub(super) fn draw_item(di: &windows::Win32::UI::Controls::DRAWITEMSTRUCT) {
    use windows::Win32::Graphics::Gdi::{DT_CENTER, DT_LEFT, DT_SINGLELINE, DT_VCENTER, DrawTextW, SelectObject, SetBkMode, TRANSPARENT};
    let Some((scale, font, nav, nav_bold)) = DLG.with(|c| c.try_borrow().ok().and_then(|g| g.as_ref().map(|d| (d.scale, d.font, d.nav, d.nav_bold))))
    else {
        return;
    };
    let hdc = di.hDC;
    let state = di.itemState.0;
    let rc = di.rcItem;
    let text = |h: HWND, idx: u32| -> Vec<u16> {
        if di.CtlType.0 == ODT_LISTBOX {
            PAGES.get(idx as usize).map(|s| s.encode_utf16().collect()).unwrap_or_default()
        } else {
            get_text(h).encode_utf16().collect()
        }
    };
    unsafe {
        SetBkMode(hdc, TRANSPARENT);
        if di.CtlType.0 == ODT_LISTBOX {
            if di.itemID == u32::MAX {
                return;
            }
            let sel = state & ODS_SELECTED != 0;
            fill(hdc, rc, SIDE);
            let m = (3.0 * scale).round() as i32;
            let pill = RECT { left: rc.left, top: rc.top + m, right: rc.right, bottom: rc.bottom - m };
            if sel {
                aa_round(hdc, pill, 9.0 * scale, SIDE, ACCENT, None);
            }
            let old = SelectObject(hdc, (if sel { nav_bold } else { nav }).into());
            SetTextColor(hdc, if sel { rgb(0xff, 0xff, 0xff) } else { TEXT });
            let mut t = text(di.hwndItem, di.itemID);
            let mut tr = RECT { left: rc.left + (16.0 * scale) as i32, ..pill };
            DrawTextW(hdc, &mut t, &mut tr, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
            SelectObject(hdc, old);
        } else if di.CtlType.0 == ODT_BUTTON {
            let primary = di.CtlID as i32 == IDOK;
            let down = state & ODS_SELECTED != 0;
            let disabled = state & ODS_DISABLED != 0;
            let (bg, border, fg) = match (primary, down) {
                (true, false) => (ACCENT, None, rgb(0xff, 0xff, 0xff)),
                (true, true) => (ACCENT_DOWN, None, rgb(0xff, 0xff, 0xff)),
                (false, false) => (BTN, Some((LINE, 1.0 * scale)), TEXT),
                (false, true) => (BTN_DOWN, Some((LINE, 1.0 * scale)), TEXT),
            };
            aa_round(hdc, rc, 8.0 * scale, BG, bg, border);
            let old = SelectObject(hdc, (if primary { nav_bold } else { font }).into());
            SetTextColor(hdc, if disabled { GREY } else { fg });
            let mut t = text(di.hwndItem, 0);
            let mut tr = rc;
            DrawTextW(hdc, &mut t, &mut tr, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
            SelectObject(hdc, old);
        }
    }
}

