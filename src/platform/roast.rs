//! 看屏吐槽（默认关）：Ctrl+Alt+K / 菜单「👀 看屏吐槽」→ 截前台窗口（不含她自己）→ 缩小 →
//! 弹确认框显示缩略图 → 主人点「给她看」才发 AppEvent::ScreenRoast。
//! 截图只在内存里，不落盘；关掉开关时热键注销；在录屏（OBS）或模型不能看图时菜单灰掉。
use crate::types::AppEvent;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmFlush, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::DRAWITEMSTRUCT;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, RegisterHotKey, SetFocus, UnregisterHotKey, VK_ESCAPE, VK_RETURN,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

/// WM_HOTKEY 的 id
pub const HOTKEY_ID: i32 = 0x4B48;
const VK_K: u32 = 0x4B;
/// 发给模型的图最长边
const MAX_W: u32 = 1024;
const MAX_H: u32 = 768;
/// 确认框里缩略图（逻辑像素）
const THUMB_W: f32 = 440.0;
const THUMB_H: f32 = 280.0;
const ID_OK: usize = 1;
const ID_NO: usize = 2;
const BS_OWNERDRAW_U32: u32 = 0x0B;
const ODS_SELECTED_U32: u32 = 0x01;
const ODS_FOCUS_U32: u32 = 0x10;
const WM_DPICHANGED_U32: u32 = 0x02E0;

// 和设置窗口保持完全一致的暖奶油色系
const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}
const BG: COLORREF = rgb(0xff, 0xfb, 0xf4);
const FIELD: COLORREF = rgb(0xff, 0xff, 0xff);
const LINE: COLORREF = rgb(0xea, 0xd9, 0xc3);
const TEXT: COLORREF = rgb(0x3a, 0x24, 0x16);
const GREY: COLORREF = rgb(0x9a, 0x85, 0x73);
const ACCENT: COLORREF = rgb(0xf5, 0xa6, 0x23);
const ACCENT_DOWN: COLORREF = rgb(0xdc, 0x8c, 0x10);
const BTN: COLORREF = rgb(0xff, 0xf1, 0xdc);
const BTN_DOWN: COLORREF = rgb(0xf6, 0xe0, 0xc2);

static REGISTERED: AtomicBool = AtomicBool::new(false);
static HOTKEY_CONFLICT: AtomicBool = AtomicBool::new(false);
static DLG_HWND: AtomicIsize = AtomicIsize::new(0);

struct Pending {
    png: Vec<u8>,
    /// 缩略图 BGRA（自上而下）
    bgra: Vec<u8>,
    w: i32,
    h: i32,
    title: String,
    dlg: HWND,
    font: HFONT,
    bold: HFONT,
    btn_bold: HFONT,
    icon: Option<HICON>,
}
// SAFETY: 只在 UI 线程上创建/使用；放进 Mutex 只是为了能当 static
unsafe impl Send for Pending {}
static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

const OFF: &str = "设置里没开";
const NO_VISION: &str = "当前模型不能看图";
const RECORDING: &str = "录屏中";
const BUSY_MSG: &str = "正忙着想上一句";

/// 现在不能用的原因（菜单灰掉时显示），None = 能用
pub fn blocked() -> Option<&'static str> {
    let c = crate::core::config::get();
    if !c.screen_roast {
        return Some(OFF);
    }
    if !crate::services::ai::vision_ok(&c.provider, &c.model) {
        return Some(NO_VISION);
    }
    if super::chat::is_busy() {
        return Some(BUSY_MSG);
    }
    if recorder_running() {
        return Some(RECORDING);
    }
    None
}

fn note_for(why: &str) -> &'static str {
    match why {
        OFF => "看屏吐槽还没开喵，在设置「常规」里打开～",
        NO_VISION => "现在用的模型看不了图片喵…换个能看图的（比如 Gemini）再给我看～",
        BUSY_MSG => "哈基米还在想上一句呢，等我想完再看喵～",
        _ => "你在录屏/直播呢，哈基米先不看～",
    }
}

/// 进程名是不是录屏软件（小写、去掉 .exe）
pub fn is_recorder(exe: &str) -> bool {
    let e = exe.to_ascii_lowercase();
    let stem = e.strip_suffix(".exe").unwrap_or(&e);
    matches!(stem, "obs64" | "obs32" | "obs" | "obs-studio")
}

fn recorder_running() -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    // SAFETY: 快照句柄用完关掉；结构体在栈上
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return false };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut found = false;
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                if is_recorder(&String::from_utf16_lossy(&e.szExeFile[..n])) {
                    found = true;
                    break;
                }
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
        found
    }
}

/// Ctrl+Alt+K 热键是否注册失败（被别的程序占了）
pub fn hotkey_conflict() -> bool {
    HOTKEY_CONFLICT.load(Ordering::Relaxed)
}

/// 按设置注册/注销 Ctrl+Alt+K（启动时、设置重新读取后调）
pub fn sync_hotkey(hwnd: HWND) {
    let want = crate::core::config::get().screen_roast;
    if want && REGISTERED.load(Ordering::Relaxed) {
        return;
    }
    if !want && !REGISTERED.load(Ordering::Relaxed) {
        HOTKEY_CONFLICT.store(false, Ordering::Relaxed);
        return;
    }
    // SAFETY: hwnd 是主窗口
    unsafe {
        if want {
            match RegisterHotKey(Some(hwnd), HOTKEY_ID, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, VK_K) {
                Ok(()) => {
                    REGISTERED.store(true, Ordering::Relaxed);
                    HOTKEY_CONFLICT.store(false, Ordering::Relaxed);
                }
                Err(e) => {
                    REGISTERED.store(false, Ordering::Relaxed);
                    HOTKEY_CONFLICT.store(true, Ordering::Relaxed);
                    crate::core::log::log(&format!("roast hotkey Ctrl+Alt+K register failed: {e}"));
                }
            }
        } else {
            let _ = UnregisterHotKey(Some(hwnd), HOTKEY_ID);
            REGISTERED.store(false, Ordering::Relaxed);
            HOTKEY_CONFLICT.store(false, Ordering::Relaxed);
        }
    }
}

/// 退出时注销热键并关闭确认框
pub fn shutdown(hwnd: HWND) {
    close_existing();
    HOTKEY_CONFLICT.store(false, Ordering::Relaxed);
    if REGISTERED.swap(false, Ordering::Relaxed) {
        unsafe {
            let _ = UnregisterHotKey(Some(hwnd), HOTKEY_ID);
        }
    }
}

/// 主消息循环：转发 Tab / 回车 / Esc 给看屏吐槽确认框
pub fn dialog_message(msg: &MSG) -> bool {
    let h = DLG_HWND.load(Ordering::Relaxed);
    if h == 0 {
        return false;
    }
    let dlg = HWND(h as *mut _);
    unsafe {
        if !IsWindow(Some(dlg)).as_bool() || (msg.hwnd != dlg && !IsChild(dlg, msg.hwnd).as_bool()) {
            return false;
        }
        // BS_OWNERDRAW 按钮下显式支持回车（焦点在「算了」上触发取消，否则默认触发「给她看」）与 Esc
        if msg.message == WM_KEYDOWN {
            if msg.wParam.0 == VK_ESCAPE.0 as usize {
                let _ = PostMessageW(Some(dlg), WM_COMMAND, WPARAM(ID_NO), LPARAM(0));
                return true;
            }
            if msg.wParam.0 == VK_RETURN.0 as usize {
                let focus = GetFocus();
                let no_btn = GetDlgItem(Some(dlg), ID_NO as i32).unwrap_or_default();
                let cmd = if !no_btn.is_invalid() && focus == no_btn { ID_NO } else { ID_OK };
                let _ = PostMessageW(Some(dlg), WM_COMMAND, WPARAM(cmd), LPARAM(0));
                return true;
            }
        }
        IsDialogMessageW(dlg, msg).as_bool()
    }
}

/// 热键 / 菜单：截图并弹确认框
pub fn trigger() {
    // 先关掉上一张未处理的确认框，避免把它自己拍进新截图里
    let was_open = close_existing();
    if let Some(why) = blocked() {
        super::post_event(AppEvent::RoastNote(note_for(why).into()));
        return;
    }
    let Some(target) = target_window() else {
        super::post_event(AppEvent::RoastNote("先点一下你想让我看的窗口嘛～".into()));
        return;
    };
    let Some((rgba, w, h)) = capture(target, was_open) else {
        super::post_event(AppEvent::RoastNote("唔…这个窗口我看不清".into()));
        return;
    };
    let Some(pm) = shrink(rgba, w, h, MAX_W, MAX_H) else { return };
    let Ok(png) = pm.encode_png() else { return };
    let bgra: Vec<u8> = pm.data().as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], 255]).collect();
    show_confirm(target, png, bgra, pm.width() as i32, pm.height() as i32);
}

fn own(h: HWND) -> bool {
    let mut pid = 0u32;
    // SAFETY: 只读
    unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
    pid == std::process::id()
}

fn class_name(h: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// 能拿来看的窗口：可见、没最小化、没被藏、不是自己/桌面/任务栏/工具窗口，有标题、够大
fn candidate(h: HWND) -> bool {
    unsafe {
        if h.0.is_null() || !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() || own(h) {
            return false;
        }
        let mut cloaked: u32 = 0;
        if DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, 4).is_ok() && cloaked != 0 {
            return false;
        }
        let ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
        if ex & WS_EX_TOOLWINDOW.0 != 0 || GetWindowTextLengthW(h) == 0 {
            return false;
        }
    }
    const SKIP: [&str; 5] = ["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd", "Windows.UI.Core.CoreWindow"];
    if SKIP.contains(&class_name(h).as_str()) {
        return false;
    }
    frame(h).is_some_and(|r| r.right - r.left >= 120 && r.bottom - r.top >= 80)
}

/// 前台窗口；前台是她自己（点了她的菜单）就按 Z 序往下找第一个像样的
fn target_window() -> Option<HWND> {
    unsafe {
        let fg = GetForegroundWindow();
        if candidate(fg) {
            return Some(fg);
        }
        let mut h = GetTopWindow(None).ok()?;
        for _ in 0..500 {
            if candidate(h) {
                return Some(h);
            }
            h = GetWindow(h, GW_HWNDNEXT).ok()?;
        }
    }
    None
}

fn frame(h: HWND) -> Option<RECT> {
    unsafe {
        let mut r = RECT::default();
        if DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut RECT as *mut _, std::mem::size_of::<RECT>() as u32).is_ok()
            && r.right > r.left
        {
            return Some(r);
        }
        GetWindowRect(h, &mut r).ok()?;
        Some(r)
    }
}

/// 从屏幕上拷这个窗口那块（截图前后临时给自身可见窗口挂 WDA_EXCLUDEFROMCAPTURE，保证 DWM 合成桌面里不含桌宠/聊天框/道具）。返回 RGBA
fn capture(h: HWND, flush_closed: bool) -> Option<(Vec<u8>, u32, u32)> {
    unsafe {
        let r = frame(h)?;
        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vr = vx + GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let vb = vy + GetSystemMetrics(SM_CYVIRTUALSCREEN);
        let (l, t, rr, b) = (r.left.max(vx), r.top.max(vy), r.right.min(vr), r.bottom.min(vb));
        let (w, hgt) = (rr - l, b - t);
        if w < 8 || hgt < 8 {
            return None;
        }
        let byte_len = (w as usize).checked_mul(hgt as usize).and_then(|n| n.checked_mul(4))?;
        let own_wins = super::own_visible_windows();
        for &ow in &own_wins {
            let _ = SetWindowDisplayAffinity(ow, WDA_EXCLUDEFROMCAPTURE);
        }
        if flush_closed || !own_wins.is_empty() {
            let _ = DwmFlush();
        }
        let screen = GetDC(None);
        let mem = CreateCompatibleDC((!screen.is_invalid()).then_some(screen));
        let out = if mem.is_invalid() {
            None
        } else {
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -hgt,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let res = match CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(bmp) => {
                    let got = if !bits.is_null() {
                        let old = SelectObject(mem, HGDIOBJ(bmp.0));
                        #[link(name = "user32")]
                        unsafe extern "system" {
                            fn PrintWindow(hwnd: HWND, hdc_blt: HDC, flags: u32) -> i32;
                        }
                        let ok = (!screen.is_invalid() && BitBlt(mem, 0, 0, w, hgt, Some(screen), l, t, SRCCOPY).is_ok())
                            || PrintWindow(h, mem, 2) != 0
                            || PrintWindow(h, mem, 0) != 0;
                        let _ = GdiFlush();
                        let rgba: Vec<u8> = if ok {
                            let px = std::slice::from_raw_parts(bits as *const u8, byte_len);
                            px.as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], 255]).collect()
                        } else {
                            Vec::new()
                        };
                        SelectObject(mem, old);
                        ok.then_some((rgba, w as u32, hgt as u32))
                    } else {
                        None
                    };
                    let _ = DeleteObject(HGDIOBJ(bmp.0));
                    got
                }
                Err(_) => None,
            };
            let _ = DeleteDC(mem);
            res
        };
        if !screen.is_invalid() {
            ReleaseDC(None, screen);
        }
        for &ow in &own_wins {
            let _ = SetWindowDisplayAffinity(ow, WDA_NONE);
        }
        out
    }
}

/// 等比缩到 max_w × max_h 以内（本来就小就不放大；大于 2x 时走逐级对半盒式滤波防摩尔纹）
pub fn shrink(rgba: Vec<u8>, w: u32, h: u32, max_w: u32, max_h: u32) -> Option<tiny_skia::Pixmap> {
    let src = tiny_skia::Pixmap::from_vec(rgba, tiny_skia::IntSize::from_wh(w, h)?)?;
    let k = (max_w as f32 / w as f32).min(max_h as f32 / h as f32).min(1.0);
    if k >= 1.0 {
        return Some(src);
    }
    let (nw, nh) = (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1));
    crate::render::sprites::scale_pixmap(src, nw, nh)
}

// ---------------------------------------------------------------- 确认框

const CLASS: PCWSTR = w!("HakimiRoastConfirm");
const TITLE_HEAD: &str = "给哈基米看看这个窗口？";
const SUBTITLE_NOTE: &str = "只发给你当前设置的聊天模型，截图留在内存里不存盘。";
const FOOT_HINT: &str = "回车发送 · Esc 算了";

fn window_title(h: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h).max(0) as usize;
        if n == 0 {
            return String::new();
        }
        let mut buf = vec![0u16; n + 1];
        let got = GetWindowTextW(h, &mut buf).max(0) as usize;
        String::from_utf16_lossy(&buf[..got.min(n)]).trim().to_string()
    }
}

fn dpi_for(hwnd: HWND) -> u32 {
    let d = unsafe {
        let wd = if !hwnd.is_invalid() { GetDpiForWindow(hwnd) } else { 0 };
        if wd > 0 { wd } else { GetDpiForSystem() }
    };
    if d == 0 { 96 } else { d }
}

/// 缩略图在确认框里的大小（物理像素）
fn thumb_size(w: i32, h: i32, s: f32) -> (i32, i32) {
    let k = ((THUMB_W * s) / w as f32).min((THUMB_H * s) / h as f32).min(1.0 * s.max(1.0));
    (((w as f32 * k).round() as i32).max(1), ((h as f32 * k).round() as i32).max(1))
}

pub fn close_existing() -> bool {
    let old = PENDING.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|p| p.dlg);
    if let Some(h) = old {
        unsafe {
            let _ = DestroyWindow(h);
        }
        true
    } else {
        false
    }
}

unsafe fn make_roast_fonts(s: f32) -> (HFONT, HFONT, HFONT) {
    let (body_face, bold_face) = super::settings::ui_faces();
    let mk_font = |size: f32, weight: i32| unsafe {
        let face_name = if weight >= FW_BOLD.0 as i32 { bold_face } else { body_face };
        let fw = super::sys::wide(face_name);
        CreateFontW(
            -(size * s).round() as i32,
            0,
            0,
            0,
            weight,
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
    (mk_font(13.0, FW_NORMAL.0 as i32), mk_font(18.0, FW_BOLD.0 as i32), mk_font(14.0, FW_BOLD.0 as i32))
}

fn fill_solid(hdc: HDC, rc: RECT, c: COLORREF) {
    unsafe {
        let b = CreateSolidBrush(c);
        FillRect(hdc, &rc, b);
        let _ = DeleteObject(HGDIOBJ(b.0));
    }
}

fn show_confirm(target: HWND, png: Vec<u8>, bgra: Vec<u8>, w: i32, h: i32) {
    close_existing();
    let dpi = dpi_for(target);
    let s = dpi as f32 / 96.0;
    let px = |v: f32| (v * s).round() as i32;
    let (tw, th) = thumb_size(w, h, s);
    let pad = px(20.0);
    let card_pad = px(10.0);
    let head_h = px(52.0);
    let foot_h = px(56.0);
    let btn_w = px(98.0);
    let btn_h = px(32.0);
    let card_w = (tw + 2 * card_pad).max(px(THUMB_W) + 2 * card_pad);
    let card_h = th + 2 * card_pad;
    let cw = card_w + 2 * pad;
    let ch = pad + head_h + px(10.0) + card_h + px(14.0) + foot_h;
    let win_title = window_title(target);
    unsafe {
        let Ok(hinst) = windows::Win32::System::LibraryLoader::GetModuleHandleW(None) else { return };
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(proc_),
            hInstance: hinst.into(),
            lpszClassName: CLASS,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let style = WS_POPUP | WS_CAPTION | WS_SYSMENU;
        let mut r = RECT { left: 0, top: 0, right: cw, bottom: ch };
        let _ = AdjustWindowRectExForDpi(&mut r, style, false, WS_EX_TOPMOST, dpi);
        let (ww, wh) = (r.right - r.left, r.bottom - r.top);
        let mon = MonitorFromWindow(target, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let (x, y) = if GetMonitorInfoW(mon, &mut mi).as_bool() {
            let wa = mi.rcWork;
            (wa.left + (wa.right - wa.left - ww) / 2, wa.top + (wa.bottom - wa.top - wh).max(0) / 2)
        } else {
            ((GetSystemMetrics(SM_CXSCREEN) - ww) / 2, (GetSystemMetrics(SM_CYSCREEN) - wh).max(0) / 2)
        };
        let Ok(dlg) = CreateWindowExW(WS_EX_TOPMOST, CLASS, w!("哈基米 · 看屏吐槽"), style, x, y, ww, wh, None, None, Some(hinst.into()), None) else {
            return;
        };
        let _ = SetWindowDisplayAffinity(dlg, WDA_EXCLUDEFROMCAPTURE);
        super::settings::style_caption(dlg);
        let icon = super::settings::make_icon();
        if let Some(ic) = icon {
            SendMessageW(dlg, WM_SETICON, Some(WPARAM(0)), Some(LPARAM(ic.0 as isize)));
            SendMessageW(dlg, WM_SETICON, Some(WPARAM(1)), Some(LPARAM(ic.0 as isize)));
        }
        DLG_HWND.store(dlg.0 as isize, Ordering::Relaxed);
        let (font, bold, btn_bold) = make_roast_fonts(s);
        let by = ch - foot_h + (foot_h - btn_h) / 2;
        let mk = |text: PCWSTR, id: usize, bx: i32| {
            let st = WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW_U32);
            let b = CreateWindowExW(WINDOW_EX_STYLE(0), w!("BUTTON"), text, st, bx, by, btn_w, btn_h, Some(dlg), Some(HMENU(id as *mut _)), Some(hinst.into()), None);
            if let Ok(b) = b {
                SendMessageW(b, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
            }
            b
        };
        let gap = px(10.0);
        let ok = mk(w!("给她看"), ID_OK, cw - pad - 2 * btn_w - gap);
        let _ = mk(w!("算了"), ID_NO, cw - pad - btn_w);
        *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending {
            png,
            bgra,
            w,
            h,
            title: win_title,
            dlg,
            font,
            bold,
            btn_bold,
            icon,
        });
        let _ = ShowWindow(dlg, SW_SHOW);
        let _ = SetForegroundWindow(dlg);
        if let Ok(b) = ok {
            let _ = SetFocus(Some(b));
        }
    }
}

unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_ERASEBKGND => LRESULT(1),
            WM_SETCURSOR => {
                let child = HWND(wp.0 as *mut _);
                if child != hwnd {
                    let id = GetDlgCtrlID(child) as usize;
                    if id == ID_OK || id == ID_NO {
                        let _ = SetCursor(LoadCursorW(None, IDC_HAND).ok());
                        return LRESULT(1);
                    }
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_DPICHANGED_U32 => {
                let dpi = (wp.0 & 0xffff) as f32;
                if let Some(r) = (lp.0 as *const RECT).as_ref() {
                    let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                }
                if dpi > 0.0 {
                    let s = dpi / 96.0;
                    let px = |v: f32| (v * s).round() as i32;
                    let (new_font, new_bold, new_btn_bold) = make_roast_fonts(s);
                    let mut old_fonts = [HFONT::default(); 3];
                    if let Some(p) = PENDING.lock().unwrap_or_else(|e| e.into_inner()).as_mut().filter(|p| p.dlg == hwnd) {
                        old_fonts = [
                            std::mem::replace(&mut p.font, new_font),
                            std::mem::replace(&mut p.bold, new_bold),
                            std::mem::replace(&mut p.btn_bold, new_btn_bold),
                        ];
                    }
                    let mut cr = RECT::default();
                    let _ = GetClientRect(hwnd, &mut cr);
                    let (pad, foot_h, btn_w, btn_h, gap) = (px(20.0), px(56.0), px(98.0), px(32.0), px(10.0));
                    let by = cr.bottom - foot_h + (foot_h - btn_h) / 2;
                    for (id, bx) in [(ID_OK, cr.right - pad - 2 * btn_w - gap), (ID_NO, cr.right - pad - btn_w)] {
                        if let Ok(b) = GetDlgItem(Some(hwnd), id as i32) {
                            SendMessageW(b, WM_SETFONT, Some(WPARAM(new_font.0 as usize)), Some(LPARAM(0)));
                            let _ = SetWindowPos(b, None, bx, by, btn_w, btn_h, SWP_NOZORDER | SWP_NOACTIVATE);
                        }
                    }
                    for f in old_fonts {
                        if !f.is_invalid() {
                            let _ = DeleteObject(HGDIOBJ(f.0));
                        }
                    }
                    let _ = InvalidateRect(Some(hwnd), None, true);
                }
                LRESULT(0)
            }
            WM_DRAWITEM => {
                let di = &*(lp.0 as *const DRAWITEMSTRUCT);
                let s = dpi_for(hwnd) as f32 / 96.0;
                let primary = di.CtlID as usize == ID_OK;
                let down = di.itemState.0 & ODS_SELECTED_U32 != 0;
                let focused = di.itemState.0 & ODS_FOCUS_U32 != 0;
                let (bg, border, fg) = match (primary, down) {
                    (true, false) => (ACCENT, focused.then_some((TEXT, 1.5 * s)), rgb(0xff, 0xff, 0xff)),
                    (true, true) => (ACCENT_DOWN, focused.then_some((TEXT, 1.5 * s)), rgb(0xff, 0xff, 0xff)),
                    (false, false) => (BTN, Some(if focused { (ACCENT, 1.6 * s) } else { (LINE, 1.0 * s) }), TEXT),
                    (false, true) => (BTN_DOWN, Some(if focused { (ACCENT, 1.6 * s) } else { (LINE, 1.0 * s) }), TEXT),
                };
                super::settings::aa_round(di.hDC, di.rcItem, 8.0 * s, BG, bg, border);
                let f = PENDING
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_ref()
                    .filter(|p| p.dlg == hwnd)
                    .map(|p| if primary { p.btn_bold } else { p.font })
                    .unwrap_or_default();
                let old = if !f.is_invalid() { SelectObject(di.hDC, HGDIOBJ(f.0)) } else { HGDIOBJ::default() };
                SetBkMode(di.hDC, TRANSPARENT);
                SetTextColor(di.hDC, fg);
                let label = if primary { "给她看" } else { "算了" };
                let mut t: Vec<u16> = label.encode_utf16().collect();
                let mut rc = di.rcItem;
                DrawTextW(di.hDC, &mut t, &mut rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
                if !old.is_invalid() {
                    SelectObject(di.hDC, old);
                }
                LRESULT(1)
            }
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let s = dpi_for(hwnd) as f32 / 96.0;
                let px = |v: f32| (v * s).round() as i32;
                let pad = px(20.0);
                let card_pad = px(10.0);
                let head_h = px(52.0);
                let foot_h = px(56.0);
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                fill_solid(hdc, rc, BG);
                let g = PENDING.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(p) = g.as_ref().filter(|p| p.dlg == hwnd) {
                    SetBkMode(hdc, TRANSPARENT);
                    // 顶部主标题（与设置页标题同字重同色）
                    let of = SelectObject(hdc, HGDIOBJ(p.bold.0));
                    SetTextColor(hdc, TEXT);
                    let mut head_rc = RECT { left: pad, top: pad - px(2.0), right: rc.right - pad, bottom: pad + px(26.0) };
                    let mut head_t: Vec<u16> = TITLE_HEAD.encode_utf16().collect();
                    DrawTextW(hdc, &mut head_t, &mut head_rc, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX);

                    // 副标题（带目标窗口名 + 隐私说明，暖灰字）
                    SelectObject(hdc, HGDIOBJ(p.font.0));
                    SetTextColor(hdc, GREY);
                    let sub = if p.title.is_empty() {
                        SUBTITLE_NOTE.to_string()
                    } else {
                        let short: String = if p.title.chars().count() > 18 {
                            p.title.chars().take(17).collect::<String>() + "…"
                        } else {
                            p.title.clone()
                        };
                        format!("「{short}」 · {SUBTITLE_NOTE}")
                    };
                    let mut sub_rc = RECT { left: pad, top: pad + px(28.0), right: rc.right - pad, bottom: pad + head_h };
                    let mut sub_t: Vec<u16> = sub.encode_utf16().collect();
                    DrawTextW(hdc, &mut sub_t, &mut sub_rc, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX);

                    // 圆角白底预览卡片 + 居中截图缩略图
                    let card_top = pad + head_h + px(10.0);
                    let (tw, th) = thumb_size(p.w, p.h, s);
                    let card_rc = RECT { left: pad, top: card_top, right: rc.right - pad, bottom: card_top + th + 2 * card_pad };
                    super::settings::aa_round(hdc, card_rc, 10.0 * s, BG, FIELD, Some((LINE, 1.0 * s)));

                    let tx = (rc.right - tw) / 2;
                    let ty = card_top + card_pad;
                    let bmi = BITMAPINFO {
                        bmiHeader: BITMAPINFOHEADER {
                            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                            biWidth: p.w,
                            biHeight: -p.h,
                            biPlanes: 1,
                            biBitCount: 32,
                            biCompression: BI_RGB.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    SetStretchBltMode(hdc, HALFTONE);
                    let _ = SetBrushOrgEx(hdc, 0, 0, None);
                    StretchDIBits(hdc, tx, ty, tw, th, 0, 0, p.w, p.h, Some(p.bgra.as_ptr() as *const _), &bmi, DIB_RGB_COLORS, SRCCOPY);
                    // 截图四周浅暖色细边，白底网页也不会糊进卡片底色里
                    let pen = CreatePen(PS_SOLID, 1, LINE);
                    let op = SelectObject(hdc, HGDIOBJ(pen.0));
                    let ob = SelectObject(hdc, GetStockObject(NULL_BRUSH));
                    let _ = Rectangle(hdc, tx - 1, ty - 1, tx + tw + 1, ty + th + 1);
                    SelectObject(hdc, op);
                    SelectObject(hdc, ob);
                    let _ = DeleteObject(HGDIOBJ(pen.0));

                    // 底部分隔线 + 左侧快捷键轻提示
                    let foot_y = rc.bottom - foot_h;
                    fill_solid(hdc, RECT { left: pad, top: foot_y, right: rc.right - pad, bottom: foot_y + px(1.0).max(1) }, LINE);
                    let mut hint_rc = RECT { left: pad, top: foot_y, right: rc.right / 2, bottom: rc.bottom };
                    let mut hint_t: Vec<u16> = FOOT_HINT.encode_utf16().collect();
                    DrawTextW(hdc, &mut hint_t, &mut hint_rc, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
                    SelectObject(hdc, of);
                }
                drop(g);
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_COMMAND => {
                let id = wp.0 & 0xffff;
                if id == ID_OK {
                    let png = PENDING.lock().unwrap_or_else(|e| e.into_inner()).as_mut().map(|p| std::mem::take(&mut p.png));
                    if let Some(png) = png.filter(|p| !p.is_empty()) {
                        super::post_event(AppEvent::ScreenRoast(png));
                    }
                    let _ = DestroyWindow(hwnd);
                } else if id == ID_NO || id == IDCANCEL.0 as usize {
                    let _ = DestroyWindow(hwnd);
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                let _ = DLG_HWND.compare_exchange(hwnd.0 as isize, 0, Ordering::Relaxed, Ordering::Relaxed);
                LRESULT(0)
            }
            WM_NCDESTROY => {
                // 等子按钮全部销毁后再释放共享字体、图标和内存截图，避免 GDI UAF
                let mut g = PENDING.lock().unwrap_or_else(|e| e.into_inner());
                if g.as_ref().is_some_and(|p| p.dlg == hwnd)
                    && let Some(p) = g.take()
                {
                    for f in [p.font, p.bold, p.btn_bold] {
                        if !f.is_invalid() {
                            let _ = DeleteObject(HGDIOBJ(f.0));
                        }
                    }
                    if let Some(ic) = p.icon {
                        let _ = DestroyIcon(ic);
                    }
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorder_names() {
        assert!(is_recorder("obs64.exe"));
        assert!(is_recorder("OBS32.EXE"));
        assert!(!is_recorder("obsidian.exe"));
        assert!(!is_recorder("chrome.exe"));
    }

    #[test]
    fn shrink_keeps_aspect() {
        let pm = shrink(vec![200; 2000 * 1000 * 4], 2000, 1000, 1024, 768).unwrap();
        assert_eq!((pm.width(), pm.height()), (1024, 512));
        let small = shrink(vec![200; 300 * 200 * 4], 300, 200, 1024, 768).unwrap();
        assert_eq!((small.width(), small.height()), (300, 200));
        let tall = shrink(vec![200; 800 * 1600 * 4], 800, 1600, 1024, 768).unwrap();
        assert_eq!((tall.width(), tall.height()), (384, 768));
        assert!(pm.encode_png().is_ok());
    }

    #[test]
    fn thumb_fits_box() {
        let (w, h) = thumb_size(1024, 512, 1.0);
        assert!(w <= 440 && h <= 280 && (w == 440 || h == 280));
        let (w, h) = thumb_size(1024, 512, 1.5);
        assert!(w <= 660 && h <= 420);
    }
}
