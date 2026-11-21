//! 看屏吐槽（默认关）：Ctrl+Alt+K / 菜单「👀 看屏吐槽」→ 截前台窗口（不含她自己）→ 缩小 →
//! 弹确认框显示缩略图 → 主人点「给她看」才发 AppEvent::ScreenRoast。
//! 截图只在内存里，不落盘；关掉开关时热键注销；在录屏（OBS）或模型不能看图时菜单灰掉。
use crate::types::AppEvent;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, RegisterHotKey, SetFocus, UnregisterHotKey,
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

static REGISTERED: AtomicBool = AtomicBool::new(false);

struct Pending {
    png: Vec<u8>,
    /// 缩略图 BGRA（自上而下）
    bgra: Vec<u8>,
    w: i32,
    h: i32,
    dlg: HWND,
    font: HFONT,
}
// SAFETY: 只在 UI 线程上创建/使用；放进 Mutex 只是为了能当 static
unsafe impl Send for Pending {}
static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

const OFF: &str = "设置里没开";
const NO_VISION: &str = "当前模型不能看图";
const RECORDING: &str = "录屏中";

/// 现在不能用的原因（菜单灰掉时显示），None = 能用
pub fn blocked() -> Option<&'static str> {
    let c = crate::core::config::get();
    if !c.screen_roast {
        return Some(OFF);
    }
    if !crate::services::ai::vision_ok(&c.provider, &c.model) {
        return Some(NO_VISION);
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

/// 按设置注册/注销 Ctrl+Alt+K（启动时、设置重新读取后调）
pub fn sync_hotkey(hwnd: HWND) {
    let want = crate::core::config::get().screen_roast;
    if REGISTERED.load(Ordering::Relaxed) == want {
        return;
    }
    // SAFETY: hwnd 是主窗口
    unsafe {
        if want {
            match RegisterHotKey(Some(hwnd), HOTKEY_ID, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, VK_K) {
                Ok(()) => REGISTERED.store(true, Ordering::Relaxed),
                Err(e) => crate::core::log::log(&format!("roast hotkey Ctrl+Alt+K register failed: {e}")),
            }
        } else {
            let _ = UnregisterHotKey(Some(hwnd), HOTKEY_ID);
            REGISTERED.store(false, Ordering::Relaxed);
        }
    }
}

/// 热键 / 菜单：截图并弹确认框
pub fn trigger() {
    if let Some(why) = blocked() {
        super::post_event(AppEvent::RoastNote(note_for(why).into()));
        return;
    }
    let Some(target) = target_window() else {
        super::post_event(AppEvent::RoastNote("先点一下你想让我看的窗口嘛～".into()));
        return;
    };
    let Some((rgba, w, h)) = capture(target) else {
        super::post_event(AppEvent::RoastNote("唔…这个窗口我看不清".into()));
        return;
    };
    let Some(pm) = shrink(rgba, w, h, MAX_W, MAX_H) else { return };
    let Ok(png) = pm.encode_png() else { return };
    let bgra: Vec<u8> = pm.data().as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], 255]).collect();
    show_confirm(png, bgra, pm.width() as i32, pm.height() as i32);
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

/// 从屏幕上拷这个窗口那块（不带 CAPTUREBLT，分层窗口——也就是她自己——不会被拍进去）。返回 RGBA
fn capture(h: HWND) -> Option<(Vec<u8>, u32, u32)> {
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
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
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
        let out = match CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(bmp) if !bits.is_null() => {
                let old = SelectObject(mem, HGDIOBJ(bmp.0));
                let ok = BitBlt(mem, 0, 0, w, hgt, Some(screen), l, t, SRCCOPY).is_ok();
                let px = std::slice::from_raw_parts(bits as *const u8, (w * hgt * 4) as usize);
                let rgba: Vec<u8> = if ok { px.as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], 255]).collect() } else { Vec::new() };
                SelectObject(mem, old);
                let _ = DeleteObject(HGDIOBJ(bmp.0));
                ok.then_some((rgba, w as u32, hgt as u32))
            }
            _ => None,
        };
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        out
    }
}

/// 等比缩到 max_w × max_h 以内（本来就小就不放大）
pub fn shrink(rgba: Vec<u8>, w: u32, h: u32, max_w: u32, max_h: u32) -> Option<tiny_skia::Pixmap> {
    let src = tiny_skia::Pixmap::from_vec(rgba, tiny_skia::IntSize::from_wh(w, h)?)?;
    let k = (max_w as f32 / w as f32).min(max_h as f32 / h as f32).min(1.0);
    if k >= 1.0 {
        return Some(src);
    }
    let (nw, nh) = (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1));
    let mut dst = tiny_skia::Pixmap::new(nw, nh)?;
    let paint = tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bicubic, ..Default::default() };
    dst.draw_pixmap(0, 0, src.as_ref(), &paint, tiny_skia::Transform::from_scale(nw as f32 / w as f32, nh as f32 / h as f32), None);
    Some(dst)
}

// ---------------------------------------------------------------- 确认框

const CLASS: PCWSTR = w!("HakimiRoastConfirm");
const ASK: &str = "给哈基米看这一张？只发给你设的聊天模型，不存盘。";

fn dpi_scale() -> f32 {
    // SAFETY: 无参数
    let d = unsafe { windows::Win32::UI::HiDpi::GetDpiForSystem() };
    if d == 0 { 1.0 } else { d as f32 / 96.0 }
}

/// 缩略图在确认框里的大小（物理像素）
fn thumb_size(w: i32, h: i32, s: f32) -> (i32, i32) {
    let k = ((THUMB_W * s) / w as f32).min((THUMB_H * s) / h as f32).min(1.0 * s.max(1.0));
    (((w as f32 * k).round() as i32).max(1), ((h as f32 * k).round() as i32).max(1))
}

fn close_existing() {
    let old = PENDING.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|p| p.dlg);
    if let Some(h) = old {
        unsafe {
            let _ = DestroyWindow(h);
        }
    }
}

fn show_confirm(png: Vec<u8>, bgra: Vec<u8>, w: i32, h: i32) {
    close_existing();
    let s = dpi_scale();
    let (tw, th) = thumb_size(w, h, s);
    let pad = (16.0 * s).round() as i32;
    let text_h = (24.0 * s).round() as i32;
    let btn_w = (96.0 * s).round() as i32;
    let btn_h = (30.0 * s).round() as i32;
    let cw = (tw + 2 * pad).max((THUMB_W * s) as i32 + 2 * pad);
    let ch = pad + text_h + pad / 2 + th + pad + btn_h + pad;
    unsafe {
        let Ok(hinst) = windows::Win32::System::LibraryLoader::GetModuleHandleW(None) else { return };
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(proc_),
            hInstance: hinst.into(),
            lpszClassName: CLASS,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as usize as *mut _),
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let style = WS_POPUP | WS_CAPTION | WS_SYSMENU;
        let mut r = RECT { left: 0, top: 0, right: cw, bottom: ch };
        let _ = AdjustWindowRectEx(&mut r, style, false, WS_EX_TOPMOST);
        let (ww, wh) = (r.right - r.left, r.bottom - r.top);
        let x = (GetSystemMetrics(SM_CXSCREEN) - ww) / 2;
        let y = (GetSystemMetrics(SM_CYSCREEN) - wh) / 2;
        let Ok(dlg) = CreateWindowExW(WS_EX_TOPMOST, CLASS, w!("哈基米 · 看屏吐槽"), style, x, y, ww, wh, None, None, Some(hinst.into()), None) else {
            return;
        };
        let font = CreateFontW(
            -(13.0 * s).round() as i32, 0, 0, 0, FW_NORMAL.0 as i32, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, FF_DONTCARE.0 as u32, w!("Microsoft YaHei"),
        );
        let by = ch - pad - btn_h;
        let mk = |text: PCWSTR, id: usize, x: i32, def: bool| {
            let st = WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(if def { BS_DEFPUSHBUTTON as u32 } else { 0 });
            let b = CreateWindowExW(WINDOW_EX_STYLE(0), w!("BUTTON"), text, st, x, by, btn_w, btn_h, Some(dlg), Some(HMENU(id as *mut _)), Some(hinst.into()), None);
            if let Ok(b) = b {
                SendMessageW(b, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
            }
            b
        };
        let ok = mk(w!("给她看"), ID_OK, cw - pad - 2 * btn_w - pad / 2, true);
        let _ = mk(w!("算了"), ID_NO, cw - pad - btn_w, false);
        *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending { png, bgra, w, h, dlg, font });
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
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let s = dpi_scale();
                let pad = (16.0 * s).round() as i32;
                let text_h = (24.0 * s).round() as i32;
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                let g = PENDING.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(p) = g.as_ref().filter(|p| p.dlg == hwnd) {
                    let of = SelectObject(hdc, HGDIOBJ(p.font.0));
                    SetBkMode(hdc, TRANSPARENT);
                    SetTextColor(hdc, windows::Win32::Foundation::COLORREF(0x2e3b5a));
                    let mut tr = RECT { left: pad, top: pad, right: rc.right - pad, bottom: pad + text_h };
                    let mut t: Vec<u16> = ASK.encode_utf16().collect();
                    DrawTextW(hdc, &mut t, &mut tr, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX);
                    SelectObject(hdc, of);
                    let (tw, th) = thumb_size(p.w, p.h, s);
                    let tx = (rc.right - tw) / 2;
                    let ty = pad + text_h + pad / 2;
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
                    StretchDIBits(hdc, tx, ty, tw, th, 0, 0, p.w, p.h, Some(p.bgra.as_ptr() as *const _), &bmi, DIB_RGB_COLORS, SRCCOPY);
                    // 细边框
                    let pen = CreatePen(PS_SOLID, 1, windows::Win32::Foundation::COLORREF(0xc8c8c8));
                    let op = SelectObject(hdc, HGDIOBJ(pen.0));
                    let ob = SelectObject(hdc, GetStockObject(NULL_BRUSH));
                    let _ = Rectangle(hdc, tx - 1, ty - 1, tx + tw + 1, ty + th + 1);
                    SelectObject(hdc, op);
                    SelectObject(hdc, ob);
                    let _ = DeleteObject(HGDIOBJ(pen.0));
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
                let mut g = PENDING.lock().unwrap_or_else(|e| e.into_inner());
                if g.as_ref().is_some_and(|p| p.dlg == hwnd) {
                    // 截图跟着框一起丢掉
                    if let Some(p) = g.take()
                        && !p.font.is_invalid()
                    {
                        let _ = DeleteObject(HGDIOBJ(p.font.0));
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
