//! 聊天「打开白名单工具/文件夹」确认弹窗（`HakimiToolConfirm`）：
//! 先过 `services::actions::resolve_open_target` 安全白名单，再弹暖奶油色确认框，
//! 主人明确点击「允许打开」（或按回车）才以零参数方式启动目标；白名单外的目标直接拒绝不弹窗。
use crate::services::actions::{RejectReason, SafeTarget, resolve_open_target};
use crate::types::AppEvent;
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::DRAWITEMSTRUCT;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus, VK_ESCAPE, VK_RETURN};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

const CLASS: PCWSTR = w!("HakimiToolConfirm");
const ID_OK: usize = 1;
const ID_NO: usize = 2;
const BS_OWNERDRAW_U32: u32 = 0x0B;
const ODS_SELECTED_U32: u32 = 0x01;
const ODS_FOCUS_U32: u32 = 0x10;
const WM_DPICHANGED_U32: u32 = 0x02E0;

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

static DLG_HWND: AtomicIsize = AtomicIsize::new(0);

struct PendingTool {
    target: SafeTarget,
    dlg: HWND,
    font: HFONT,
    bold: HFONT,
    btn_bold: HFONT,
    icon: Option<HICON>,
}
// SAFETY: 仅在 UI 线程读写
unsafe impl Send for PendingTool {}
static PENDING: Mutex<Option<PendingTool>> = Mutex::new(None);

pub(crate) fn hwnd() -> Option<HWND> {
    let h = DLG_HWND.load(Ordering::Relaxed);
    (h != 0).then_some(HWND(h as *mut _))
}

pub fn close_existing() {
    let old = PENDING.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|p| p.dlg);
    if let Some(h) = old {
        unsafe {
            let _ = DestroyWindow(h);
        }
    }
}

/// 主消息循环：处理 Tab / 回车 / Esc（严格限定焦点属于当前确认框）
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

/// 请求打开某个名称的目标：先走安全白名单，通过则弹确认框，被拒则冒气泡说明原因
pub fn request_open(raw_target: &str) {
    let cfg = crate::core::config::get();
    match resolve_open_target(raw_target, &cfg.allowed_apps, super::report::dir) {
        Ok(safe) => show_confirm(safe),
        Err(reason) => {
            crate::core::log::log(&format!("tool_open:rejected:{raw_target}:{reason:?}"));
            let msg = match reason {
                RejectReason::BlockedExecutable => "这个属于系统命令行/脚本工具，为了安全哈基米不能碰喵～",
                RejectReason::MissingFile => "登记的这个程序路径好像找不到了喵，去 config.json 检查一下？",
                RejectReason::NotWhitelisted => "这个不在哈基米的安全名单里喵～目前只能开记事本、计算器、画图、歌单/数据文件夹或 allowedApps 里登记的程序哦",
            };
            super::post_event(AppEvent::RoastNote(msg.into()));
        }
    }
}

fn execute_safe_target(target: &SafeTarget) {
    crate::core::log::log(&format!("tool_open:confirmed:{}", target.display_name()));
    if std::env::var_os("HAKIMI_DRY_RUN_OPEN").is_some_and(|v| v == "1") {
        return;
    }
    match target {
        SafeTarget::AppFolder { path, .. } => {
            let _ = std::fs::create_dir_all(path);
            super::open_path(path);
        }
        SafeTarget::SystemExe { path, .. } | SafeTarget::UserExe { path, .. } => {
            // 严格零参数启动，杜绝命令行注入
            if let Err(e) = std::process::Command::new(path).spawn() {
                crate::core::log::log(&format!("tool_open:spawn_err:{e}"));
                super::post_event(AppEvent::RoastNote("唔…启动失败了喵，看看权限或路径对不对？".into()));
            }
        }
    }
}

fn dpi_for(hwnd: HWND) -> u32 {
    let d = unsafe {
        let wd = if !hwnd.is_invalid() { GetDpiForWindow(hwnd) } else { 0 };
        if wd > 0 { wd } else { GetDpiForSystem() }
    };
    if d == 0 { 96 } else { d }
}

unsafe fn make_fonts(s: f32) -> (HFONT, HFONT, HFONT) {
    let (body_face, bold_face) = super::settings::ui_faces();
    let mk = |size: f32, weight: i32| unsafe {
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
    (mk(13.0, FW_NORMAL.0 as i32), mk(17.0, FW_BOLD.0 as i32), mk(14.0, FW_BOLD.0 as i32))
}

fn fill_solid(hdc: HDC, rc: RECT, c: COLORREF) {
    unsafe {
        let b = CreateSolidBrush(c);
        FillRect(hdc, &rc, b);
        let _ = DeleteObject(HGDIOBJ(b.0));
    }
}

fn show_confirm(target: SafeTarget) {
    close_existing();
    let anchor = super::main_hwnd().unwrap_or_default();
    let dpi = dpi_for(anchor);
    let s = dpi as f32 / 96.0;
    let px = |v: f32| (v * s).round() as i32;
    let cw = px(440.0);
    let ch = px(218.0);
    let pad = px(20.0);
    let foot_h = px(54.0);
    let btn_w = px(98.0);
    let btn_h = px(32.0);

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
        let mon = MonitorFromWindow(anchor, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let (x, y) = if GetMonitorInfoW(mon, &mut mi).as_bool() {
            let wa = mi.rcWork;
            (wa.left + (wa.right - wa.left - ww) / 2, wa.top + (wa.bottom - wa.top - wh).max(0) / 2)
        } else {
            ((GetSystemMetrics(SM_CXSCREEN) - ww) / 2, (GetSystemMetrics(SM_CYSCREEN) - wh).max(0) / 2)
        };
        let Ok(dlg) = CreateWindowExW(
            WS_EX_TOPMOST,
            CLASS,
            w!("哈基米 · 打开确认"),
            style,
            x,
            y,
            ww,
            wh,
            None,
            None,
            Some(hinst.into()),
            None,
        ) else {
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
        let (font, bold, btn_bold) = make_fonts(s);
        let by = ch - foot_h + (foot_h - btn_h) / 2;
        let gap = px(10.0);
        let mk_btn = |text: PCWSTR, id: usize, bx: i32| {
            let st = WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW_U32);
            let b = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("BUTTON"),
                text,
                st,
                bx,
                by,
                btn_w,
                btn_h,
                Some(dlg),
                Some(HMENU(id as *mut _)),
                Some(hinst.into()),
                None,
            );
            if let Ok(b) = b {
                SendMessageW(b, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
            }
            b
        };
        let ok = mk_btn(w!("允许打开"), ID_OK, cw - pad - 2 * btn_w - gap);
        let _ = mk_btn(w!("算了"), ID_NO, cw - pad - btn_w);
        *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(PendingTool {
            target,
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
                    let (nf, nb, nbb) = make_fonts(s);
                    let mut old = [HFONT::default(); 3];
                    if let Some(p) = PENDING.lock().unwrap_or_else(|e| e.into_inner()).as_mut().filter(|p| p.dlg == hwnd) {
                        old = [
                            std::mem::replace(&mut p.font, nf),
                            std::mem::replace(&mut p.bold, nb),
                            std::mem::replace(&mut p.btn_bold, nbb),
                        ];
                    }
                    let mut cr = RECT::default();
                    let _ = GetClientRect(hwnd, &mut cr);
                    let (pad, foot_h, btn_w, btn_h, gap) = (px(20.0), px(54.0), px(98.0), px(32.0), px(10.0));
                    let by = cr.bottom - foot_h + (foot_h - btn_h) / 2;
                    for (id, bx) in [(ID_OK, cr.right - pad - 2 * btn_w - gap), (ID_NO, cr.right - pad - btn_w)] {
                        if let Ok(b) = GetDlgItem(Some(hwnd), id as i32) {
                            SendMessageW(b, WM_SETFONT, Some(WPARAM(nf.0 as usize)), Some(LPARAM(0)));
                            let _ = SetWindowPos(b, None, bx, by, btn_w, btn_h, SWP_NOZORDER | SWP_NOACTIVATE);
                        }
                    }
                    for f in old {
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
                let label = if primary { "允许打开" } else { "算了" };
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
                let foot_h = px(54.0);
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                fill_solid(hdc, rc, BG);
                let g = PENDING.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(p) = g.as_ref().filter(|p| p.dlg == hwnd) {
                    SetBkMode(hdc, TRANSPARENT);
                    let of = SelectObject(hdc, HGDIOBJ(p.bold.0));
                    SetTextColor(hdc, TEXT);
                    let mut head_rc = RECT { left: pad, top: pad - px(2.0), right: rc.right - pad, bottom: pad + px(26.0) };
                    let mut head_t: Vec<u16> = "允许哈基米帮你打开这个吗？".encode_utf16().collect();
                    DrawTextW(hdc, &mut head_t, &mut head_rc, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);

                    // 白底圆角信息卡片
                    let card_top = pad + px(34.0);
                    let card_bot = rc.bottom - foot_h - px(14.0);
                    let card_rc = RECT { left: pad, top: card_top, right: rc.right - pad, bottom: card_bot };
                    super::settings::aa_round(hdc, card_rc, 10.0 * s, BG, FIELD, Some((LINE, 1.0 * s)));

                    SelectObject(hdc, HGDIOBJ(p.btn_bold.0));
                    SetTextColor(hdc, TEXT);
                    let mut name_rc = RECT { left: card_rc.left + px(14.0), top: card_top + px(12.0), right: card_rc.right - px(14.0), bottom: card_top + px(36.0) };
                    let mut name_t: Vec<u16> = p.target.display_name().encode_utf16().collect();
                    DrawTextW(hdc, &mut name_t, &mut name_rc, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX);

                    SelectObject(hdc, HGDIOBJ(p.font.0));
                    SetTextColor(hdc, GREY);
                    let path_str = p.target.target_path().to_string_lossy();
                    let mut path_rc = RECT { left: card_rc.left + px(14.0), top: card_top + px(38.0), right: card_rc.right - px(14.0), bottom: card_top + px(58.0) };
                    let mut path_t: Vec<u16> = path_str.encode_utf16().collect();
                    DrawTextW(hdc, &mut path_t, &mut path_rc, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_PATH_ELLIPSIS | DT_NOPREFIX);

                    let mut note_rc = RECT { left: card_rc.left + px(14.0), top: card_top + px(60.0), right: card_rc.right - px(14.0), bottom: card_bot - px(8.0) };
                    let mut note_t: Vec<u16> = "仅限安全名单目标，零参数启动，不执行任何命令。".encode_utf16().collect();
                    DrawTextW(hdc, &mut note_t, &mut note_rc, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX);

                    // 底部分隔线与快捷键提示
                    let foot_y = rc.bottom - foot_h;
                    fill_solid(hdc, RECT { left: pad, top: foot_y, right: rc.right - pad, bottom: foot_y + px(1.0).max(1) }, LINE);
                    let mut hint_rc = RECT { left: pad, top: foot_y, right: rc.right / 2, bottom: rc.bottom };
                    let mut hint_t: Vec<u16> = "回车允许 · Esc 算了".encode_utf16().collect();
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
                    let target = PENDING.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|p| p.dlg == hwnd).map(|p| p.target.clone());
                    let _ = DestroyWindow(hwnd);
                    if let Some(t) = target {
                        execute_safe_target(&t);
                    }
                } else if id == ID_NO || id == IDCANCEL.0 as usize {
                    crate::core::log::log("tool_open:cancelled");
                    let _ = DestroyWindow(hwnd);
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                crate::core::log::log("tool_open:cancelled");
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                let _ = DLG_HWND.compare_exchange(hwnd.0 as isize, 0, Ordering::Relaxed, Ordering::Relaxed);
                LRESULT(0)
            }
            WM_NCDESTROY => {
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
