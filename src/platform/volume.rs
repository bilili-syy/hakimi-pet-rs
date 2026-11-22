//! 独立桌宠音量控制浮窗（`HakimiVolumePopup`）：
//! 贴在哈基米身边的暖奶油色迷你音量面板，支持：
//! - 「🗣️ 语音」与「🎵 音乐」两条独立丝滑抗锯齿滑块（拖动中 120Hz 实时改正在播放的音量，松手一次性写盘）
//! - 「🔇 静音 / 🔊 恢复」一键总静音按钮与右上角「×」关闭按钮
//! - 悬停在某一行上直接用鼠标滚轮 ±5% 微调对应声道
//! - 两种呼出方式：
//!   1. `open_interactive()`（右键菜单「🎛️ 打开音量调节条…」或聊天口令）：获取焦点，点外部（`WA_INACTIVE`）或按 `Esc` 自动收起；
//!   2. `open_osd()`（按住 `Ctrl + 滚轮` 在桌宠身上快调）：`SW_SHOWNOACTIVATE` 不抢当前工作窗口焦点，停手 2.2 秒自动淡隐（鼠标移入面板时保持显示，可继续精调）。
use super::settings::{aa_round, ui_faces};
use super::sys::wide;
use crate::core::config;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_ESCAPE, VK_LEFT, VK_RIGHT,
    VK_SPACE,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

const CLASS: PCWSTR = w!("HakimiVolumePopup");
const LOGICAL_W: f32 = 242.0;
const LOGICAL_H: f32 = 132.0;
const TRACK_X0: f32 = 70.0;
const TRACK_X1: f32 = 192.0;
const ROW_VOICE_Y: f32 = 58.0;
const ROW_MUSIC_Y: f32 = 92.0;
const OSD_TIMER_ID: usize = 42;
const OSD_HIDE_MS: u32 = 2200;
const WM_MOUSELEAVE_U32: u32 = 0x02A3;
const WM_DPICHANGED_U32: u32 = 0x02E0;
const WA_INACTIVE_U16: usize = 0;

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

const BG: COLORREF = rgb(0xff, 0xfb, 0xf4);
const BORDER: COLORREF = rgb(0xe5, 0xcf, 0xaf);
const TEXT: COLORREF = rgb(0x3a, 0x24, 0x16);
const SUBTEXT: COLORREF = rgb(0x9a, 0x85, 0x73);
const TRACK_BG: COLORREF = rgb(0xef, 0xe3, 0xd3);
const ACCENT: COLORREF = rgb(0xf5, 0xa6, 0x23);
const ACCENT_HOT: COLORREF = rgb(0xe0, 0x92, 0x12);
const MUTED_FILL: COLORREF = rgb(0xc8, 0xb8, 0xa6);
const WHITE: COLORREF = rgb(0xff, 0xff, 0xff);
const PILL_BG: COLORREF = rgb(0xff, 0xf1, 0xdc);
const PILL_HOT: COLORREF = rgb(0xfb, 0xe4, 0xc2);
const PILL_BORDER: COLORREF = rgb(0xe6, 0xc8, 0x9c);
const CLOSE_IDLE: COLORREF = rgb(0xa8, 0x8e, 0x7c);
const CLOSE_HOT_BG: COLORREF = rgb(0xfd, 0xec, 0xea);
const CLOSE_HOT_FG: COLORREF = rgb(0xea, 0x43, 0x35);

static POPUP_HWND: AtomicIsize = AtomicIsize::new(0);
static POPUP_VISIBLE: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragKind {
    None,
    Voice,
    Music,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HoverPart {
    None,
    MuteBtn,
    CloseBtn,
    VoiceSlider,
    MusicSlider,
}

struct VolPopup {
    hwnd: HWND,
    scale: f32,
    font: HFONT,
    bold: HFONT,
    small: HFONT,
    drag: DragKind,
    hover: HoverPart,
    tracking_leave: bool,
    osd_mode: bool,
    dirty_disk: bool,
}

thread_local! {
    static POP: RefCell<Option<VolPopup>> = const { RefCell::new(None) };
}

pub(crate) fn hwnd() -> Option<HWND> {
    let h = POPUP_HWND.load(Ordering::Relaxed);
    (h != 0).then_some(HWND(h as *mut _))
}

pub fn is_visible() -> bool {
    POPUP_VISIBLE.load(Ordering::Relaxed)
}

/// 逻辑横坐标转音量百分比（0..=100）
pub fn pos_to_pct(lx: f32) -> u8 {
    let span = (TRACK_X1 - TRACK_X0).max(1.0);
    (((lx - TRACK_X0) / span).clamp(0.0, 1.0) * 100.0).round() as u8
}

/// 滚轮 delta 计算步进后的百分比（每格 ±120 = ±5%）
pub fn step_pct(cur: u8, delta: i32) -> u8 {
    if delta == 0 {
        return cur;
    }
    let raw = (delta as f32 / 120.0 * 5.0).round() as i32;
    let step = if raw == 0 { delta.signum() } else { raw };
    (cur as i32 + step).clamp(0, 100) as u8
}

/// 选择不遮挡桌宠脸和气泡的摆放坐标（优先贴左侧，空间不够贴右侧，最终夹在工作区内）
pub fn pick_popup_pos(pet: RECT, pop: (i32, i32), wa: RECT, gap: i32) -> (i32, i32) {
    let (pop_w, pop_h) = pop;
    let left_x = pet.left - pop_w - gap;
    let right_x = pet.right + gap;
    let x = if left_x >= wa.left {
        left_x
    } else if right_x + pop_w <= wa.right {
        right_x
    } else {
        left_x.clamp(wa.left, (wa.right - pop_w).max(wa.left))
    };
    let ideal_y = pet.bottom - pop_h - gap * 2;
    let y = ideal_y.clamp(wa.top, (wa.bottom - pop_h).max(wa.top));
    (x, y)
}

fn dpi_scale(anchor: HWND) -> f32 {
    let d = unsafe {
        let wd = if !anchor.is_invalid() { GetDpiForWindow(anchor) } else { 0 };
        if wd > 0 { wd } else { GetDpiForSystem() }
    };
    (if d == 0 { 96 } else { d }) as f32 / 96.0
}

unsafe fn create_fonts(s: f32) -> (HFONT, HFONT, HFONT) {
    let (body_face, bold_face) = ui_faces();
    let mk = |size: f32, bold: bool| unsafe {
        let fw = wide(if bold { bold_face } else { body_face });
        CreateFontW(
            -(size * s).round() as i32,
            0,
            0,
            0,
            if bold { FW_BOLD.0 as i32 } else { FW_NORMAL.0 as i32 },
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            FF_DONTCARE.0 as u32,
            PCWSTR(fw.as_ptr()),
        )
    };
    (mk(13.0, false), mk(13.5, true), mk(11.0, false))
}

unsafe fn delete_fonts(fonts: (HFONT, HFONT, HFONT)) {
    for f in [fonts.0, fonts.1, fonts.2] {
        if !f.is_invalid() {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(f.0));
            }
        }
    }
}

fn ensure_created() -> Option<HWND> {
    if let Some(h) = POP.with(|c| c.borrow().as_ref().map(|p| p.hwnd))
        && unsafe { IsWindow(Some(h)).as_bool() }
    {
        return Some(h);
    }
    let owner = super::main_hwnd().unwrap_or_default();
    let s = dpi_scale(owner);
    unsafe {
        let hinst = GetModuleHandleW(None).ok()?;
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinst.into(),
            lpszClassName: CLASS,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let w = (LOGICAL_W * s).round() as i32;
        let h = (LOGICAL_H * s).round() as i32;
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            CLASS,
            w!("哈基米音量"),
            WS_POPUP,
            0,
            0,
            w,
            h,
            if owner.is_invalid() { None } else { Some(owner) },
            None,
            Some(hinst.into()),
            None,
        )
        .ok()?;
        let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
        apply_round_rgn(hwnd, w, h, s);
        let (font, bold, small) = create_fonts(s);
        POPUP_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
        POP.with(|c| {
            *c.borrow_mut() = Some(VolPopup {
                hwnd,
                scale: s,
                font,
                bold,
                small,
                drag: DragKind::None,
                hover: HoverPart::None,
                tracking_leave: false,
                osd_mode: false,
                dirty_disk: false,
            });
        });
        Some(hwnd)
    }
}

unsafe fn apply_round_rgn(hwnd: HWND, w: i32, h: i32, s: f32) {
    let rad = (14.0 * s).round() as i32;
    unsafe {
        let rgn = CreateRoundRectRgn(0, 0, w + 1, h + 1, rad, rad);
        if !rgn.is_invalid() {
            let _ = SetWindowRgn(hwnd, Some(rgn), true);
        }
    }
}

fn place_near_pet(hwnd: HWND) {
    let Some(pet) = super::main_hwnd() else { return };
    let mut pr = RECT::default();
    if unsafe { GetWindowRect(pet, &mut pr) }.is_err() {
        return;
    }
    let s = dpi_scale(pet);
    POP.with(|c| {
        if let Some(p) = c.borrow_mut().as_mut()
            && (p.scale - s).abs() > 0.01
        {
            let old = (p.font, p.bold, p.small);
            p.scale = s;
            let nf = unsafe { create_fonts(s) };
            p.font = nf.0;
            p.bold = nf.1;
            p.small = nf.2;
            unsafe { delete_fonts(old) };
        }
    });
    let w = (LOGICAL_W * s).round() as i32;
    let h = (LOGICAL_H * s).round() as i32;
    let wa = super::work_area_of(pet);
    let gap = (8.0 * s).round() as i32;
    let (x, y) = pick_popup_pos(pr, (w, h), wa, gap);
    unsafe {
        apply_round_rgn(hwnd, w, h, s);
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
    }
}

/// 交互式打开（右键菜单或聊天口令触发：获取焦点，点外部自动收起）
pub fn open_interactive() {
    let Some(hwnd) = ensure_created() else { return };
    place_near_pet(hwnd);
    POP.with(|c| {
        if let Some(p) = c.borrow_mut().as_mut() {
            p.osd_mode = false;
        }
    });
    POPUP_VISIBLE.store(true, Ordering::Relaxed);
    unsafe {
        let _ = KillTimer(Some(hwnd), OSD_TIMER_ID);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

/// 轻提示打开（`Ctrl + 滚轮` 触发：不抢焦点，2.2 秒无交互自动收起）
pub fn open_osd() {
    let Some(hwnd) = ensure_created() else { return };
    place_near_pet(hwnd);
    let already_interactive = POP.with(|c| {
        if let Some(p) = c.borrow_mut().as_mut() {
            if POPUP_VISIBLE.load(Ordering::Relaxed) && !p.osd_mode {
                return true;
            }
            p.osd_mode = true;
        }
        false
    });
    POPUP_VISIBLE.store(true, Ordering::Relaxed);
    unsafe {
        if !already_interactive {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            SetTimer(Some(hwnd), OSD_TIMER_ID, OSD_HIDE_MS, None);
        }
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

/// 外部改了音量时重绘面板（若正开着）
pub fn sync_if_open() {
    if !is_visible() {
        return;
    }
    if let Some(h) = hwnd() {
        unsafe {
            let _ = InvalidateRect(Some(h), None, false);
        }
    }
}

/// 桌宠移动时，若音量面板开着且未在拖拽滑块则跟着走
pub fn follow_pet() {
    if !is_visible() {
        return;
    }
    let dragging = POP.with(|c| c.borrow().as_ref().is_some_and(|p| p.drag != DragKind::None));
    if dragging {
        return;
    }
    if let Some(h) = hwnd() {
        place_near_pet(h);
    }
}

pub fn hide() {
    flush_if_dirty();
    POPUP_VISIBLE.store(false, Ordering::Relaxed);
    POP.with(|c| {
        if let Some(p) = c.borrow_mut().as_mut() {
            p.drag = DragKind::None;
            p.hover = HoverPart::None;
            p.osd_mode = false;
        }
    });
    if let Some(h) = hwnd() {
        unsafe {
            let _ = KillTimer(Some(h), OSD_TIMER_ID);
            let _ = ShowWindow(h, SW_HIDE);
        }
    }
}

pub fn shutdown() {
    flush_if_dirty();
    POPUP_VISIBLE.store(false, Ordering::Relaxed);
    let old = POP.with(|c| c.borrow_mut().take());
    POPUP_HWND.store(0, Ordering::Relaxed);
    if let Some(p) = old {
        unsafe {
            let _ = KillTimer(Some(p.hwnd), OSD_TIMER_ID);
            if IsWindow(Some(p.hwnd)).as_bool() {
                let _ = DestroyWindow(p.hwnd);
            }
            delete_fonts((p.font, p.bold, p.small));
        }
    }
}

fn flush_if_dirty() {
    let dirty = POP.with(|c| {
        c.borrow_mut()
            .as_mut()
            .map(|p| std::mem::replace(&mut p.dirty_disk, false))
            .unwrap_or(false)
    });
    if dirty {
        config::flush();
        super::menu::refresh();
        super::settings::sync_external_volume();
    }
}

fn mute_rect() -> [f32; 4] {
    [138.0, 9.0, 68.0, 23.0]
}

fn close_rect() -> [f32; 4] {
    [210.0, 9.0, 22.0, 23.0]
}

fn hit_rect(lx: f32, ly: f32, r: [f32; 4]) -> bool {
    lx >= r[0] && lx <= r[0] + r[2] && ly >= r[1] && ly <= r[1] + r[3]
}

fn hit_slider(lx: f32, ly: f32, row_y: f32) -> bool {
    (TRACK_X0 - 10.0..=TRACK_X1 + 10.0).contains(&lx) && (row_y - 13.0..=row_y + 13.0).contains(&ly)
}

fn hover_at(lx: f32, ly: f32) -> HoverPart {
    if hit_rect(lx, ly, close_rect()) {
        HoverPart::CloseBtn
    } else if hit_rect(lx, ly, mute_rect()) {
        HoverPart::MuteBtn
    } else if hit_slider(lx, ly, ROW_VOICE_Y) {
        HoverPart::VoiceSlider
    } else if hit_slider(lx, ly, ROW_MUSIC_Y) {
        HoverPart::MusicSlider
    } else {
        HoverPart::None
    }
}

fn apply_slider_drag(kind: DragKind, lx: f32, commit_disk: bool) {
    let pct = pos_to_pct(lx);
    let val = pct as f32 / 100.0;
    let upd = |c: &mut config::Config| {
        match kind {
            DragKind::Voice => c.tts.volume = val,
            DragKind::Music => c.tts.music_volume = val,
            DragKind::None => {}
        }
        if pct > 0 && c.tts.muted {
            c.tts.muted = false;
        }
    };
    if commit_disk {
        config::update(upd);
        super::menu::refresh();
        super::settings::sync_external_volume();
    } else {
        config::update_in_memory(upd);
        POP.with(|c| {
            if let Some(p) = c.borrow_mut().as_mut() {
                p.dirty_disk = true;
            }
        });
    }
    crate::services::audio::apply_config_volume();
    if let Some(h) = hwnd() {
        unsafe {
            let _ = InvalidateRect(Some(h), None, false);
        }
    }
}

fn toggle_mute() {
    let c = config::update(|cfg| cfg.tts.muted = !cfg.tts.muted);
    if c.tts.muted {
        crate::services::tts::cancel_active();
    }
    crate::services::audio::apply_config_volume();
    super::menu::refresh();
    super::settings::sync_external_volume();
    if let Some(h) = hwnd() {
        unsafe {
            let _ = InvalidateRect(Some(h), None, false);
        }
    }
}

unsafe fn paint(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let (w, h) = ((rc.right - rc.left).max(1), (rc.bottom - rc.top).max(1));

        let mem_dc = CreateCompatibleDC(Some(hdc));
        let mem_bmp = CreateCompatibleBitmap(hdc, w, h);
        let old_bmp = SelectObject(mem_dc, HGDIOBJ(mem_bmp.0));

        let (s, font, bold, small, hover, drag) = POP.with(|c| {
            c.borrow()
                .as_ref()
                .map(|p| (p.scale, p.font, p.bold, p.small, p.hover, p.drag))
                .unwrap_or((1.0, HFONT::default(), HFONT::default(), HFONT::default(), HoverPart::None, DragKind::None))
        });
        let px = |v: f32| (v * s).round() as i32;
        let rect_px = |x: f32, y: f32, rw: f32, rh: f32| RECT {
            left: px(x),
            top: px(y),
            right: px(x + rw),
            bottom: px(y + rh),
        };

        // 外框与奶油底色
        let full = RECT { left: 0, top: 0, right: w, bottom: h };
        let bg_brush = CreateSolidBrush(BG);
        FillRect(mem_dc, &full, bg_brush);
        let _ = DeleteObject(HGDIOBJ(bg_brush.0));
        aa_round(mem_dc, full, 13.0 * s, BG, BG, Some((BORDER, (1.5 * s).max(1.0))));

        let tts = config::get().tts;
        let muted = tts.muted;
        let v_pct = tts.voice_pct();
        let m_pct = tts.music_pct();

        SetBkMode(mem_dc, TRANSPARENT);

        // 标题
        let old_f = SelectObject(mem_dc, HGDIOBJ(bold.0));
        SetTextColor(mem_dc, TEXT);
        let title = wide(if muted { "🔇 已静音" } else { "🔊 桌宠音量" });
        let mut tr = rect_px(14.0, 9.0, 122.0, 24.0);
        DrawTextW(mem_dc, &mut title.clone()[..title.len() - 1], &mut tr, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);

        // 静音切换胶囊按钮
        let mr = mute_rect();
        let mr_px = rect_px(mr[0], mr[1], mr[2], mr[3]);
        let mute_hot = hover == HoverPart::MuteBtn;
        let (m_fill, m_stroke, m_fg, m_txt) = if muted {
            (if mute_hot { ACCENT_HOT } else { ACCENT }, None, WHITE, "🔊 恢复")
        } else {
            (
                if mute_hot { PILL_HOT } else { PILL_BG },
                Some((PILL_BORDER, (1.0 * s).max(1.0))),
                TEXT,
                "🔇 静音",
            )
        };
        aa_round(mem_dc, mr_px, 11.0 * s, BG, m_fill, m_stroke);
        SelectObject(mem_dc, HGDIOBJ(if muted { bold.0 } else { font.0 }));
        SetTextColor(mem_dc, m_fg);
        let mw = wide(m_txt);
        let mut mtr = mr_px;
        DrawTextW(mem_dc, &mut mw.clone()[..mw.len() - 1], &mut mtr, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);

        // 右上角 × 关闭按钮
        let cr = close_rect();
        let cr_px = rect_px(cr[0], cr[1], cr[2], cr[3]);
        let close_hot = hover == HoverPart::CloseBtn;
        if close_hot {
            aa_round(mem_dc, cr_px, 6.0 * s, BG, CLOSE_HOT_BG, None);
        }
        let pen = CreatePen(PS_SOLID, (1.6 * s).round().max(1.0) as i32, if close_hot { CLOSE_HOT_FG } else { CLOSE_IDLE });
        let old_pen = SelectObject(mem_dc, HGDIOBJ(pen.0));
        let (cx, cy, d) = ((cr_px.left + cr_px.right) / 2, (cr_px.top + cr_px.bottom) / 2, (4.2 * s).round() as i32);
        let _ = MoveToEx(mem_dc, cx - d, cy - d, None);
        let _ = LineTo(mem_dc, cx + d + 1, cy + d + 1);
        let _ = MoveToEx(mem_dc, cx + d, cy - d, None);
        let _ = LineTo(mem_dc, cx - d - 1, cy + d + 1);
        SelectObject(mem_dc, old_pen);
        let _ = DeleteObject(HGDIOBJ(pen.0));

        // 两条抗锯齿滑块行：语音 + 音乐
        let draw_row = |label_str: &str, row_y: f32, pct: u8, active: bool| {
            SelectObject(mem_dc, HGDIOBJ(font.0));
            SetTextColor(mem_dc, if muted { SUBTEXT } else { TEXT });
            let lw = wide(label_str);
            let mut lr = rect_px(14.0, row_y - 12.0, 54.0, 24.0);
            DrawTextW(mem_dc, &mut lw.clone()[..lw.len() - 1], &mut lr, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);

            // 轨道底槽
            let track_h = 6.0;
            let trk = rect_px(TRACK_X0, row_y - track_h / 2.0, TRACK_X1 - TRACK_X0, track_h);
            aa_round(mem_dc, trk, 3.0 * s, BG, TRACK_BG, None);

            // 已填充进度
            let ratio = (pct as f32 / 100.0).clamp(0.0, 1.0);
            let fill_w = (TRACK_X1 - TRACK_X0) * ratio;
            let fill_color = if muted { MUTED_FILL } else { ACCENT };
            if fill_w >= 2.0 {
                let fl = rect_px(TRACK_X0, row_y - track_h / 2.0, fill_w.max(track_h), track_h);
                aa_round(mem_dc, fl, 3.0 * s, TRACK_BG, fill_color, None);
            }

            // 圆形滑块拇指
            let r_thumb = if active { 7.8 } else { 6.8 };
            let tx = TRACK_X0 + fill_w;
            let thumb_rc = rect_px(tx - r_thumb, row_y - r_thumb, r_thumb * 2.0, r_thumb * 2.0);
            aa_round(mem_dc, thumb_rc, r_thumb * s, BG, WHITE, Some((fill_color, (2.0 * s).max(1.2))));

            // 右侧百分比读数
            let p_str = format!("{pct}%");
            let pw = wide(&p_str);
            let mut pr = rect_px(196.0, row_y - 12.0, 34.0, 24.0);
            SetTextColor(mem_dc, if muted { SUBTEXT } else { TEXT });
            DrawTextW(mem_dc, &mut pw.clone()[..pw.len() - 1], &mut pr, DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        };

        draw_row(
            "🗣️ 语音",
            ROW_VOICE_Y,
            v_pct,
            drag == DragKind::Voice || hover == HoverPart::VoiceSlider,
        );
        draw_row(
            "🎵 音乐",
            ROW_MUSIC_Y,
            m_pct,
            drag == DragKind::Music || hover == HoverPart::MusicSlider,
        );

        // 底部小字提示
        SelectObject(mem_dc, HGDIOBJ(small.0));
        SetTextColor(mem_dc, SUBTEXT);
        let tip = wide("滚轮可单调 · Ctrl+滚轮在桌宠身上快调");
        let mut tip_rc = rect_px(14.0, 109.0, 214.0, 18.0);
        DrawTextW(mem_dc, &mut tip.clone()[..tip.len() - 1], &mut tip_rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);

        SelectObject(mem_dc, old_f);
        let _ = BitBlt(hdc, 0, 0, w, h, Some(mem_dc), 0, 0, SRCCOPY);
        SelectObject(mem_dc, old_bmp);
        let _ = DeleteObject(HGDIOBJ(mem_bmp.0));
        let _ = DeleteDC(mem_dc);
        let _ = EndPaint(hwnd, &ps);
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let coords = || {
        let s = POP.with(|c| c.borrow().as_ref().map(|p| p.scale).unwrap_or(1.0));
        let cx = (lp.0 & 0xffff) as u16 as i16 as f32 / s;
        let cy = ((lp.0 >> 16) & 0xffff) as u16 as i16 as f32 / s;
        (cx, cy)
    };

    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            unsafe { paint(hwnd) };
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let clickable = POP.with(|c| {
                c.borrow().as_ref().is_some_and(|p| p.hover != HoverPart::None || p.drag != DragKind::None)
            });
            unsafe {
                let cur = LoadCursorW(None, if clickable { IDC_HAND } else { IDC_ARROW }).unwrap_or_default();
                SetCursor(Some(cur));
            }
            LRESULT(1)
        }
        WM_MOUSEMOVE => {
            let (lx, ly) = coords();
            let drag = POP.with(|c| {
                let mut g = c.borrow_mut();
                let Some(p) = g.as_mut() else { return DragKind::None };
                if !p.tracking_leave {
                    p.tracking_leave = true;
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    unsafe {
                        let _ = TrackMouseEvent(&mut tme);
                    }
                }
                if p.osd_mode {
                    unsafe {
                        let _ = KillTimer(Some(hwnd), OSD_TIMER_ID);
                    }
                }
                let next_hover = hover_at(lx, ly);
                if next_hover != p.hover {
                    p.hover = next_hover;
                    unsafe {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                }
                p.drag
            });
            if drag != DragKind::None {
                apply_slider_drag(drag, lx, false);
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE_U32 => {
            let rearm_osd = POP.with(|c| {
                let mut g = c.borrow_mut();
                let Some(p) = g.as_mut() else { return false };
                p.tracking_leave = false;
                if p.hover != HoverPart::None {
                    p.hover = HoverPart::None;
                    unsafe {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                }
                p.osd_mode && p.drag == DragKind::None
            });
            if rearm_osd {
                unsafe {
                    SetTimer(Some(hwnd), OSD_TIMER_ID, 1200, None);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let (lx, ly) = coords();
            // 用户主动点击了 OSD 面板：升级为交互态，点外部即可收起
            POP.with(|c| {
                if let Some(p) = c.borrow_mut().as_mut()
                    && p.osd_mode
                {
                    p.osd_mode = false;
                    unsafe {
                        let _ = KillTimer(Some(hwnd), OSD_TIMER_ID);
                        let _ = SetForegroundWindow(hwnd);
                        let _ = SetFocus(Some(hwnd));
                    }
                }
            });
            if hit_rect(lx, ly, close_rect()) {
                hide();
                return LRESULT(0);
            }
            if hit_rect(lx, ly, mute_rect()) {
                toggle_mute();
                return LRESULT(0);
            }
            let kind = if hit_slider(lx, ly, ROW_VOICE_Y) {
                DragKind::Voice
            } else if hit_slider(lx, ly, ROW_MUSIC_Y) {
                DragKind::Music
            } else {
                DragKind::None
            };
            if kind != DragKind::None {
                POP.with(|c| {
                    if let Some(p) = c.borrow_mut().as_mut() {
                        p.drag = kind;
                    }
                });
                unsafe {
                    let _ = SetCapture(hwnd);
                }
                apply_slider_drag(kind, lx, false);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let (lx, _) = coords();
            let drag = POP.with(|c| {
                c.borrow_mut()
                    .as_mut()
                    .map(|p| std::mem::replace(&mut p.drag, DragKind::None))
                    .unwrap_or(DragKind::None)
            });
            if drag != DragKind::None {
                unsafe {
                    let _ = ReleaseCapture();
                }
                apply_slider_drag(drag, lx, true);
            }
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            POP.with(|c| {
                if let Some(p) = c.borrow_mut().as_mut() {
                    p.drag = DragKind::None;
                }
            });
            flush_if_dirty();
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp.0 >> 16) & 0xffff) as u16 as i16 as i32;
            let mut pt = POINT {
                x: (lp.0 & 0xffff) as u16 as i16 as i32,
                y: ((lp.0 >> 16) & 0xffff) as u16 as i16 as i32,
            };
            unsafe {
                let _ = ScreenToClient(hwnd, &mut pt);
            }
            let s = POP.with(|c| c.borrow().as_ref().map(|p| p.scale).unwrap_or(1.0));
            let ly = pt.y as f32 / s;
            let is_music = ly >= (ROW_VOICE_Y + ROW_MUSIC_Y) * 0.5;
            config::update(|cfg| {
                if is_music {
                    let next = step_pct(cfg.tts.music_pct(), delta);
                    cfg.tts.music_volume = next as f32 / 100.0;
                    if delta > 0 && next > 0 {
                        cfg.tts.muted = false;
                    }
                } else {
                    let next = step_pct(cfg.tts.voice_pct(), delta);
                    cfg.tts.volume = next as f32 / 100.0;
                    if delta > 0 && next > 0 {
                        cfg.tts.muted = false;
                    }
                }
            });
            crate::services::audio::apply_config_volume();
            super::menu::refresh();
            super::settings::sync_external_volume();
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let vk = wp.0 as u16;
            if vk == VK_ESCAPE.0 {
                hide();
            } else if vk == VK_SPACE.0 || vk == b'M' as u16 {
                toggle_mute();
            } else if vk == VK_LEFT.0 || vk == VK_RIGHT.0 {
                let d = if vk == VK_RIGHT.0 { 120 } else { -120 };
                config::update(|cfg| {
                    let next = step_pct(cfg.tts.voice_pct(), d);
                    cfg.tts.volume = next as f32 / 100.0;
                    if d > 0 && next > 0 {
                        cfg.tts.muted = false;
                    }
                });
                crate::services::audio::apply_config_volume();
                super::menu::refresh();
                super::settings::sync_external_volume();
                unsafe {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            LRESULT(0)
        }
        WM_ACTIVATE => {
            if (wp.0 & 0xffff) == WA_INACTIVE_U16 && is_visible() {
                let osd = POP.with(|c| c.borrow().as_ref().is_some_and(|p| p.osd_mode));
                if !osd {
                    hide();
                }
            }
            LRESULT(0)
        }
        WM_TIMER if wp.0 == OSD_TIMER_ID => {
            hide();
            LRESULT(0)
        }
        WM_DPICHANGED_U32 => {
            if is_visible() {
                place_near_pet(hwnd);
                unsafe {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_math_and_stepping() {
        assert_eq!(pos_to_pct(TRACK_X0 - 50.0), 0);
        assert_eq!(pos_to_pct(TRACK_X0), 0);
        assert_eq!(pos_to_pct((TRACK_X0 + TRACK_X1) / 2.0), 50);
        assert_eq!(pos_to_pct(TRACK_X1), 100);
        assert_eq!(pos_to_pct(TRACK_X1 + 50.0), 100);

        assert_eq!(step_pct(50, 120), 55);
        assert_eq!(step_pct(50, -120), 45);
        assert_eq!(step_pct(98, 120), 100);
        assert_eq!(step_pct(3, -120), 0);
        // 触控板微幅滚动至少动 1%
        assert_eq!(step_pct(50, 15), 51);
        assert_eq!(step_pct(50, -15), 49);
    }

    #[test]
    fn popup_placement_stays_on_screen_without_overlapping_pet() {
        let wa = RECT { left: 0, top: 0, right: 1920, bottom: 1040 };
        // 桌宠在右下偏中：优先摆在桌宠左侧
        let pet1 = RECT { left: 1200, top: 680, right: 1440, bottom: 1040 };
        let (x, y) = pick_popup_pos(pet1, (242, 132), wa, 8);
        assert_eq!(x, 1200 - 242 - 8);
        assert!(y >= wa.top && y + 132 <= wa.bottom);

        // 桌宠贴在屏幕最左边：自动换到桌宠右侧
        let pet2 = RECT { left: 10, top: 680, right: 250, bottom: 1040 };
        let (x2, _) = pick_popup_pos(pet2, (242, 132), wa, 8);
        assert_eq!(x2, 250 + 8);
    }
}
