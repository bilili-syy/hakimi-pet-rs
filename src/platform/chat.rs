//! 聊天输入框：原生 Win32 Edit（输入法开箱即用）。
//! 分层窗口（UpdateLayeredWindow）的子控件画不出来，所以输入框放在一个独立的弹出窗口里，
//! 贴在她脚边（原版 #chat：左右各 6px、底部 6px），跟着她移动。
//! 没有 comctl32 v6 清单时 EM_SETCUEBANNER 不生效，所以灰色提示字由我们自己画（edit_proc 的 WM_PAINT）；
//! 右侧画一个「×」关闭按钮，点了等同 Esc（发 ChatCancel）。
use super::sys::wide;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use crate::types::{AppEvent, WIN_W};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreatePen, CreateRoundRectRgn,
    CreateSolidBrush, DEFAULT_CHARSET, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteObject,
    DrawTextW, EndPaint, FF_DONTCARE, FW_NORMAL, GetDC, HBRUSH, HFONT, HGDIOBJ, InvalidateRect, LineTo, MoveToEx,
    OUT_DEFAULT_PRECIS, PAINTSTRUCT, PS_SOLID, ReleaseDC, RoundRect, SelectObject, SetBkColor, SetBkMode, SetTextColor,
    SetWindowRgn, TRANSPARENT,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_ESCAPE, VK_RETURN,
};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

const HOST_CLASS: PCWSTR = w!("HakimiPetRsChat");
/// 输入框逻辑高度：7+7 内边距 + 一行字 + 2+2 边框
const H: f32 = 32.0;
const MARGIN: f32 = 6.0;
const MAX_LEN: usize = 200;
const EM_SETLIMITTEXT: u32 = 0x00C5;
/// host_proc 里 WM_CTLCOLOREDIT 要用的底色画刷
static BG_BRUSH: AtomicIsize = AtomicIsize::new(0);
/// 输入框里有没有字（EN_CHANGE 时更新）。行为层用它决定别的动作要不要顺手关掉输入框
static HAS_TEXT: AtomicBool = AtomicBool::new(false);
/// 输入法正在组字（这时 Edit 里还没字，但不该再画提示字）
static COMPOSING: AtomicBool = AtomicBool::new(false);
/// 鼠标是否悬停在「×」上
static CLOSE_HOT: AtomicBool = AtomicBool::new(false);
/// 「×」按钮的逻辑宽度（贴右侧内边距里）
const CLOSE_W: f32 = 18.0;
/// 灰色提示字（原版 placeholder 是「和哈基米说点什么…（回车发送，Esc 关闭）」，228px 宽放不下，缩短）
const CUE: &str = "说点什么…回车发送，Esc 关闭";
const CUE_COLOR: COLORREF = rgb(0xb0, 0x9c, 0x8c);
const CLOSE_COLOR: COLORREF = rgb(0xa8, 0x8e, 0x7c);
const CLOSE_HOT_COLOR: COLORREF = rgb(0xea, 0x43, 0x35);
const EN_CHANGE: u32 = 0x0300;
const WM_MOUSELEAVE: u32 = 0x02A3;

const BG: COLORREF = rgb(255, 250, 240);
const BORDER: COLORREF = rgb(0x42, 0x85, 0xf4);
const TEXT: COLORREF = rgb(0x5a, 0x3b, 0x2e);

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

pub struct Chat {
    host: HWND,
    edit: HWND,
    font: HFONT,
    bg: HBRUSH,
    scale: f32,
    pub visible: bool,
}

impl Chat {
    pub fn new(owner: HWND, instance: windows::Win32::Foundation::HINSTANCE, scale: f32) -> Option<Self> {
        unsafe {
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(host_proc),
                hInstance: instance,
                lpszClassName: HOST_CLASS,
                hCursor: LoadCursorW(None, IDC_IBEAM).unwrap_or_default(),
                ..Default::default()
            };
            RegisterClassExW(&wc);
            let host = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                HOST_CLASS,
                w!("哈基米聊天"),
                WS_POPUP | WS_CLIPCHILDREN,
                0,
                0,
                10,
                10,
                Some(owner),
                None,
                Some(instance),
                None,
            )
            .ok()?;
            let edit = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("EDIT"),
                PCWSTR::null(),
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                0,
                0,
                10,
                10,
                Some(host),
                None,
                Some(instance),
                None,
            )
            .ok()?;
            let _ = SetWindowSubclass(edit, Some(edit_proc), 1, 0);
            SendMessageW(edit, EM_SETLIMITTEXT, Some(WPARAM(MAX_LEN)), None);
            let bg = CreateSolidBrush(BG);
            BG_BRUSH.store(bg.0 as isize, Ordering::Relaxed);
            let mut c = Self { host, edit, font: HFONT::default(), bg, scale, visible: false };
            c.set_scale(scale);
            Some(c)
        }
    }

    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale;
        unsafe {
            if !self.font.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(self.font.0));
            }
            self.font = CreateFontW(
                -(12.0 * scale).round() as i32,
                0,
                0,
                0,
                FW_NORMAL.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                FF_DONTCARE.0 as u32,
                w!("Microsoft YaHei"),
            );
            SendMessageW(self.edit, WM_SETFONT, Some(WPARAM(self.font.0 as usize)), Some(LPARAM(1)));
        }
    }

    /// 按宠物窗口位置（物理像素）摆放
    pub fn layout(&self, pet_x: i32, pet_y: i32, pet_h: i32) {
        let s = self.scale;
        let w = ((WIN_W as f32 - 2.0 * MARGIN) * s).round() as i32;
        let h = (H * s).round() as i32;
        let x = pet_x + (MARGIN * s).round() as i32;
        let y = pet_y + pet_h - (MARGIN * s).round() as i32 - h;
        let pad_x = (10.0 * s).round() as i32;
        let close_w = (CLOSE_W * s).round() as i32;
        let line = (17.0 * s).round() as i32;
        unsafe {
            // 只是挪位置（她在走路）时不重建圆角区域、不重画，免得闪
            let mut old = RECT::default();
            let _ = GetWindowRect(self.host, &mut old);
            let resized = old.right - old.left != w || old.bottom - old.top != h;
            let _ = SetWindowPos(self.host, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
            if !resized {
                return;
            }
            let _ = SetWindowPos(
                self.edit,
                None,
                pad_x,
                (h - line) / 2,
                w - 2 * pad_x - close_w,
                line,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            let r = (28.0 * s).round() as i32;
            let rgn = CreateRoundRectRgn(0, 0, w + 1, h + 1, r, r);
            SetWindowRgn(self.host, Some(rgn), true);
            let _ = InvalidateRect(Some(self.host), None, true);
        }
    }

    pub fn open(&mut self) {
        self.visible = true;
        unsafe {
            let _ = ShowWindow(self.host, SW_SHOW);
            let _ = SetForegroundWindow(self.host);
            let _ = SetFocus(Some(self.edit));
        }
    }

    pub fn close(&mut self) {
        self.visible = false;
        unsafe {
            let _ = ShowWindow(self.host, SW_HIDE);
        }
    }

    /// 宠物整体隐藏/显示时跟着走（不改 visible 标记）
    pub fn show_with_pet(&self, pet_visible: bool) {
        if self.visible {
            unsafe {
                let _ = ShowWindow(self.host, if pet_visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
            }
        }
    }

    pub fn enable(&self, on: bool) {
        unsafe {
            let _ = EnableWindow(self.edit, on);
            if on && self.visible {
                let _ = SetFocus(Some(self.edit));
            }
            let _ = InvalidateRect(Some(self.edit), None, true);
        }
    }
}

impl Drop for Chat {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.host);
            let _ = DeleteObject(HGDIOBJ(self.font.0));
            let _ = DeleteObject(HGDIOBJ(self.bg.0));
        }
    }
}

/// 输入框里有没有打了字（不算空白）。行为层判断「别的动作要不要顺手关掉空输入框」用
pub fn has_text() -> bool { HAS_TEXT.load(Ordering::Relaxed) }

/// 「×」按钮在 host 客户区里的矩形
fn close_rect(host: HWND) -> RECT {
    let s = super::current_scale();
    let mut rc = RECT::default();
    unsafe {
        let _ = GetClientRect(host, &mut rc);
    }
    let pad = (6.0 * s).round() as i32;
    let w = (CLOSE_W * s).round() as i32;
    RECT { left: rc.right - pad - w, top: rc.top, right: rc.right - pad / 2, bottom: rc.bottom }
}

fn in_close(host: HWND, lp: LPARAM) -> bool {
    let x = (lp.0 & 0xffff) as u16 as i16 as i32;
    let y = ((lp.0 >> 16) & 0xffff) as u16 as i16 as i32;
    let r = close_rect(host);
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

/// 画「×」：两条斜线，悬停时变红
unsafe fn paint_close(hdc: windows::Win32::Graphics::Gdi::HDC, host: HWND) {
    unsafe {
        let s = super::current_scale();
        let r = close_rect(host);
        let cx = (r.left + r.right) / 2;
        let cy = (r.top + r.bottom) / 2;
        let k = (4.0 * s).round() as i32;
        let color = if CLOSE_HOT.load(Ordering::Relaxed) { CLOSE_HOT_COLOR } else { CLOSE_COLOR };
        let pen = CreatePen(PS_SOLID, (1.6 * s).round().max(1.0) as i32, color);
        let op = SelectObject(hdc, HGDIOBJ(pen.0));
        let _ = MoveToEx(hdc, cx - k, cy - k, None);
        let _ = LineTo(hdc, cx + k + 1, cy + k + 1);
        let _ = MoveToEx(hdc, cx + k, cy - k, None);
        let _ = LineTo(hdc, cx - k - 1, cy + k + 1);
        SelectObject(hdc, op);
        let _ = DeleteObject(HGDIOBJ(pen.0));
    }
}

/// 空输入框上画灰色提示字（在 Edit 自己画完之后叠上去）
unsafe fn paint_cue(edit: HWND) {
    unsafe {
        if GetWindowTextLengthW(edit) > 0 || COMPOSING.load(Ordering::Relaxed) {
            return;
        }
        let hdc = GetDC(Some(edit));
        if hdc.is_invalid() {
            return;
        }
        let font = SendMessageW(edit, WM_GETFONT, None, None);
        let of = SelectObject(hdc, HGDIOBJ(font.0 as *mut _));
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, CUE_COLOR);
        let mut rc = RECT::default();
        let _ = GetClientRect(edit, &mut rc);
        rc.left += 2; // 和 Edit 默认左边距对齐，光标落在提示字开头
        let mut text = wide(CUE);
        text.pop(); // 去掉结尾的 0
        DrawTextW(hdc, &mut text, &mut rc, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
        SelectObject(hdc, of);
        ReleaseDC(Some(edit), hdc);
    }
}

fn edit_text(edit: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(edit);
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(edit, &mut buf);
        String::from_utf16_lossy(&buf[..got.max(0) as usize])
    }
}

unsafe extern "system" fn edit_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    unsafe {
        match msg {
            WM_KEYDOWN if wp.0 == VK_RETURN.0 as usize => {
                let text = edit_text(hwnd).trim().to_string();
                if !text.is_empty() {
                    let _ = SetWindowTextW(hwnd, w!(""));
                    super::post_event(AppEvent::ChatSubmit(text));
                }
                return LRESULT(0);
            }
            WM_KEYDOWN if wp.0 == VK_ESCAPE.0 as usize => {
                super::chat_escape();
                return LRESULT(0);
            }
            // 吞掉回车/Esc 的 WM_CHAR，否则单行 Edit 会“叮”一声
            WM_CHAR if wp.0 == 0x0d || wp.0 == 0x1b => return LRESULT(0),
            WM_PAINT => {
                let r = DefSubclassProc(hwnd, msg, wp, lp);
                paint_cue(hwnd);
                return r;
            }
            WM_IME_STARTCOMPOSITION | WM_IME_ENDCOMPOSITION => {
                COMPOSING.store(msg == WM_IME_STARTCOMPOSITION, Ordering::Relaxed);
                let _ = InvalidateRect(Some(hwnd), None, true);
            }
            // 失焦/禁用/启用时 Edit 只重画一部分，整个重画一次免得提示字残影
            WM_SETFOCUS | WM_KILLFOCUS | WM_ENABLE => {
                let r = DefSubclassProc(hwnd, msg, wp, lp);
                let _ = InvalidateRect(Some(hwnd), None, true);
                return r;
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, wp, lp)
    }
}

unsafe extern "system" fn host_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                let s = super::current_scale();
                let pen = CreatePen(PS_SOLID, (2.0 * s).round().max(1.0) as i32, BORDER);
                let brush = CreateSolidBrush(BG);
                let op = SelectObject(hdc, HGDIOBJ(pen.0));
                let ob = SelectObject(hdc, HGDIOBJ(brush.0));
                let r = (28.0 * s).round() as i32;
                let _ = RoundRect(hdc, rc.left + 1, rc.top + 1, rc.right - 1, rc.bottom - 1, r, r);
                SelectObject(hdc, op);
                SelectObject(hdc, ob);
                let _ = DeleteObject(HGDIOBJ(pen.0));
                let _ = DeleteObject(HGDIOBJ(brush.0));
                paint_close(hdc, hwnd);
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_COMMAND if (wp.0 >> 16) as u32 & 0xffff == EN_CHANGE => {
                let edit = HWND(lp.0 as *mut _);
                let has = !edit_text(edit).trim().is_empty();
                HAS_TEXT.store(has, Ordering::Relaxed);
                // 从有字变空 / 从空变有字：提示字要出现或消失，整个重画
                let _ = InvalidateRect(Some(edit), None, true);
                LRESULT(0)
            }
            WM_SETCURSOR => {
                let mut pt = windows::Win32::Foundation::POINT::default();
                let _ = GetCursorPos(&mut pt);
                let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut pt);
                let lpp = LPARAM(((pt.y as i16 as u16 as isize) << 16) | (pt.x as i16 as u16 as isize));
                if (wp.0 as *mut core::ffi::c_void) == hwnd.0 && in_close(hwnd, lpp) {
                    let _ = SetCursor(LoadCursorW(None, IDC_HAND).ok());
                    return LRESULT(1);
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_MOUSEMOVE => {
                let hot = in_close(hwnd, lp);
                if CLOSE_HOT.swap(hot, Ordering::Relaxed) != hot {
                    let r = close_rect(hwnd);
                    let _ = InvalidateRect(Some(hwnd), Some(&r), false);
                }
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut tme);
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                if CLOSE_HOT.swap(false, Ordering::Relaxed) {
                    let r = close_rect(hwnd);
                    let _ = InvalidateRect(Some(hwnd), Some(&r), false);
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => LRESULT(0),
            WM_LBUTTONUP => {
                if in_close(hwnd, lp) {
                    CLOSE_HOT.store(false, Ordering::Relaxed);
                    super::chat_escape();
                } else if let Ok(edit) = GetWindow(hwnd, GW_CHILD) {
                    // 点到内边距：把焦点交给输入框
                    let _ = SetFocus(Some(edit));
                }
                LRESULT(0)
            }
            WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
                let hdc = windows::Win32::Graphics::Gdi::HDC(wp.0 as *mut _);
                SetTextColor(hdc, TEXT);
                SetBkColor(hdc, BG);
                LRESULT(BG_BRUSH.load(Ordering::Relaxed))
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;

    fn pump(ms: u64) {
        let end = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        unsafe {
            let mut msg = MSG::default();
            while std::time::Instant::now() < end {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }

    /// 只开聊天框（不开宠物主窗口、不抢单实例锁），看提示字、「×」和 has_text。
    /// 在真桌面上跑并截图：`cargo test chat_box_visual -- --ignored --nocapture`
    /// HAKIMI_CHAT_AT="x,y" 指定宠物窗口左上角（物理像素），默认 (800,300)
    #[test]
    #[ignore]
    fn chat_box_visual() {
        let scale = 1.5f32;
        super::super::SCALE_BITS.store(scale.to_bits(), Ordering::Relaxed);
        let (px, py) = std::env::var("HAKIMI_CHAT_AT")
            .ok()
            .and_then(|v| {
                let (a, b) = v.split_once(',')?;
                Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
            })
            .unwrap_or((800, 300));
        unsafe {
            let hinst: windows::Win32::Foundation::HINSTANCE = GetModuleHandleW(None).unwrap().into();
            let owner = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("STATIC"),
                w!("owner"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(hinst),
                None,
            )
            .unwrap();
            let mut c = Chat::new(owner, hinst, scale).unwrap();
            let pet_h = (crate::types::WIN_H as f32 * scale).round() as i32;
            c.layout(px, py, pet_h);
            c.open();
            pump(3000);
            eprintln!("[chat] phase=empty has_text={}", has_text());
            assert!(!has_text());
            let _ = SetWindowTextW(c.edit, w!("今天天气怎么样"));
            pump(2500);
            eprintln!("[chat] phase=typed has_text={}", has_text());
            assert!(has_text());
            // 跟着她走：挪 200px，框要跟过去
            c.layout(px - 200, py, pet_h);
            let mut r = RECT::default();
            let _ = GetWindowRect(c.host, &mut r);
            eprintln!("[chat] moved host to {},{}", r.left, r.top);
            assert_eq!(r.left, px - 200 + (MARGIN * scale).round() as i32);
            let _ = SetWindowTextW(c.edit, w!("  "));
            pump(300);
            assert!(!has_text(), "空白不算有字");
            let _ = SetWindowTextW(c.edit, w!(""));
            pump(1500);
            // 点「×」：发 ChatCancel
            let cr = close_rect(c.host);
            let (cx, cy) = ((cr.left + cr.right) / 2, (cr.top + cr.bottom) / 2);
            let lp = LPARAM(((cy as isize) << 16) | cx as isize);
            SendMessageW(c.host, WM_LBUTTONUP, None, Some(lp));
            let got = super::super::EVENT_QUEUE.lock().unwrap().drain(..).any(|e| matches!(e, AppEvent::ChatCancel));
            eprintln!("[chat] close button -> ChatCancel={got}");
            assert!(got);
            c.close();
            drop(c);
            let _ = DestroyWindow(owner);
        }
    }
}
