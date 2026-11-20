//! 聊天输入框：原生 Win32 Edit（输入法开箱即用）。
//! 分层窗口（UpdateLayeredWindow）的子控件画不出来，所以输入框放在一个独立的弹出窗口里，
//! 贴在她脚边（原版 #chat：左右各 6px、底部 6px），跟着她移动。
use super::sys::wide;
use std::sync::atomic::{AtomicIsize, Ordering};
use crate::types::{AppEvent, WIN_W};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreatePen, CreateRoundRectRgn,
    CreateSolidBrush, DEFAULT_CHARSET, DeleteObject, EndPaint, FF_DONTCARE, FW_NORMAL, HBRUSH, HFONT, HGDIOBJ,
    OUT_DEFAULT_PRECIS, PAINTSTRUCT, PS_SOLID, RoundRect, SelectObject, SetBkColor, SetTextColor, SetWindowRgn,
    InvalidateRect,
};
use windows::Win32::UI::Controls::EM_SETCUEBANNER;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus, VK_ESCAPE, VK_RETURN};
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
            let cue = wide("和哈基米说点什么…（回车发送，Esc 关闭）");
            SendMessageW(edit, EM_SETCUEBANNER, Some(WPARAM(1)), Some(LPARAM(cue.as_ptr() as isize)));
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
        let line = (17.0 * s).round() as i32;
        unsafe {
            let _ = SetWindowPos(self.host, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
            let _ = SetWindowPos(self.edit, None, pad_x, (h - line) / 2, w - 2 * pad_x, line, SWP_NOZORDER | SWP_NOACTIVATE);
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
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
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
