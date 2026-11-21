//! 聊天输入框：原生 Win32 Edit（输入法开箱即用）。
//! 分层窗口（UpdateLayeredWindow）的子控件画不出来，所以输入框放在一个独立的弹出窗口里，
//! 贴在她脚边（原版 #chat：左右各 6px、底部 6px），跟着她移动。
//! 没有 comctl32 v6 清单时 EM_SETCUEBANNER 不生效，所以灰色提示字由我们自己画（edit_proc 的 WM_PAINT）；
//! 右侧画一个「×」关闭按钮，点了等同 Esc（发 ChatCancel）。
//! 多行：Shift+回车换行，框往上长（最多 MAX_LINES 行，再多就滚动）；回车发送。
//! 等她回复时输入框变只读，Esc = 不等了（发 ChatStop）；平时 Esc 关闭。
//! ↑/↓ 翻以前发过的话（光标在第一行/最后一行时），从完整聊天记录里取。
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
    GetKeyState, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_BACK, VK_CONTROL, VK_DOWN, VK_ESCAPE,
    VK_RETURN, VK_SHIFT, VK_UP,
};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

const HOST_CLASS: PCWSTR = w!("HakimiPetRsChat");
/// 输入框逻辑高度：7+7 内边距 + 一行字 + 2+2 边框
const H: f32 = 32.0;
const MARGIN: f32 = 6.0;
const MAX_LEN: usize = 1000;
const EM_GETSEL: u32 = 0x00B0;
const EM_SETLIMITTEXT: u32 = 0x00C5;
const EM_SETSEL: u32 = 0x00B1;
const EM_GETLINECOUNT: u32 = 0x00BA;
const EM_LINEFROMCHAR: u32 = 0x00C9;
const EM_REPLACESEL: u32 = 0x00C2;
const EM_SETREADONLY: u32 = 0x00CF;
const ES_MULTILINE_: u32 = 0x0004;
const ES_AUTOVSCROLL_: u32 = 0x0040;
/// 框最多长到几行，再多就在框里滚动
const MAX_LINES: i32 = 4;
/// 一行字的逻辑高度
const LINE: f32 = 17.0;
/// 现在框有几行高
static LINES: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(1);
/// 在等她回复（输入框只读，Esc = 不等了）
static BUSY: AtomicBool = AtomicBool::new(false);
/// 发过的话（↑/↓ 翻）
static HIST: std::sync::Mutex<Option<InputHistory>> = std::sync::Mutex::new(None);
/// host_proc 里 WM_CTLCOLOREDIT 要用的底色画刷
static BG_BRUSH: AtomicIsize = AtomicIsize::new(0);
/// 输入框里有没有字（EN_CHANGE 时更新）。行为层用它决定别的动作要不要顺手关掉输入框
static HAS_TEXT: AtomicBool = AtomicBool::new(false);
/// 输入法正在组字（这时 Edit 里还没字，但不该再画提示字）
static COMPOSING: AtomicBool = AtomicBool::new(false);
/// 鼠标是否悬停在「×」上
static CLOSE_HOT: AtomicBool = AtomicBool::new(false);
/// 宠物脚边锚定底边（物理像素），防多行缩回单行时在顶边夹取后向下漂移
static ANCHOR_BOTTOM: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
/// 当前输入框 HWND（用于判断焦点是否仍在输入框内）
static EDIT_HWND: AtomicIsize = AtomicIsize::new(0);
/// 标记刚刚由 WM_KEYDOWN(Ctrl+Backspace) 处理过一次删词，避免紧随其后的 WM_CHAR(0x7F) 重复删
static ATE_CTRL_BACK: AtomicBool = AtomicBool::new(false);
/// 「×」按钮的逻辑宽度（贴右侧内边距里）
const CLOSE_W: f32 = 18.0;
/// 灰色提示字（原版 placeholder 是「和哈基米说点什么…（回车发送，Esc 关闭）」，228px 宽放不下，缩短）
const CUE: &str = "回车发送 · Shift+回车换行 · ↑翻旧话";
/// 等回复时的提示字
const CUE_BUSY: &str = "她在想…按 Esc 或点 × 不等了";
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
    pub(crate) host: HWND,
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
            let Some(edit) = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("EDIT"),
                PCWSTR::null(),
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_MULTILINE_ | ES_AUTOVSCROLL_),
                0,
                0,
                10,
                10,
                Some(host),
                None,
                Some(instance),
                None,
            )
            .ok() else {
                let _ = DestroyWindow(host);
                return None;
            };
            let _ = SetWindowSubclass(edit, Some(edit_proc), 1, 0);
            SendMessageW(edit, EM_SETLIMITTEXT, Some(WPARAM(MAX_LEN)), None);
            EDIT_HWND.store(edit.0 as isize, Ordering::Relaxed);
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
            // 先创建新字体并 WM_SETFONT，再释放旧字体，避免控件持有悬空 HFONT
            let new_font = CreateFontW(
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
            if new_font.is_invalid() {
                return;
            }
            let old = std::mem::replace(&mut self.font, new_font);
            SendMessageW(self.edit, WM_SETFONT, Some(WPARAM(self.font.0 as usize)), Some(LPARAM(1)));
            if !old.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(old.0));
            }
        }
    }

    /// 按宠物窗口位置（物理像素）摆放
    pub fn layout(&self, pet_x: i32, pet_y: i32, pet_h: i32) {
        let s = self.scale;
        let w = ((WIN_W as f32 - 2.0 * MARGIN) * s).round() as i32;
        let x = pet_x + (MARGIN * s).round() as i32;
        let bottom = pet_y + pet_h - (MARGIN * s).round() as i32;
        ANCHOR_BOTTOM.store(bottom, Ordering::Relaxed);
        place(self.host, self.edit, s, x, bottom, w, false);
    }

    pub fn open(&mut self) {
        self.visible = true;
        sync_edit(self.host, self.edit);
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

    /// 等回复时 on=false：不禁用（那样收不到 Esc），改成只读
    pub fn enable(&self, on: bool) {
        BUSY.store(!on, Ordering::Relaxed);
        unsafe {
            SendMessageW(self.edit, EM_SETREADONLY, Some(WPARAM(!on as usize)), None);
            if on && self.visible {
                let _ = SetFocus(Some(self.edit));
            }
            let _ = InvalidateRect(Some(self.edit), None, true);
        }
    }
}

impl Drop for Chat {
    fn drop(&mut self) {
        BG_BRUSH.store(0, Ordering::Relaxed);
        EDIT_HWND.store(0, Ordering::Relaxed);
        unsafe {
            let _ = DestroyWindow(self.host);
            if !self.font.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(self.font.0));
            }
            if !self.bg.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(self.bg.0));
            }
        }
    }
}

/// 摆框：底边固定在 bottom，高度跟着行数长，并夹在当前屏幕工作区内免得靠边时被截断。force = 行数变了，就算大小没变也重摆输入框
fn place(host: HWND, edit: HWND, s: f32, x: i32, bottom: i32, w: i32, force: bool) {
    let n = LINES.load(Ordering::Relaxed).clamp(1, MAX_LINES);
    let line = (LINE * s).round() as i32;
    let h = (H * s).round() as i32 + (n - 1) * line;
    let wa = super::work_area_at(
        windows::Win32::Foundation::POINT { x: x + w / 2, y: bottom - h / 2 },
        windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST,
    );
    let edge = (4.0 * s).round().max(2.0) as i32;
    let x = x.clamp(wa.left + edge, (wa.right - w - edge).max(wa.left + edge));
    let y = (bottom - h).clamp(wa.top + edge, (wa.bottom - h - edge).max(wa.top + edge));
    let pad_x = (10.0 * s).round() as i32;
    let close_w = (CLOSE_W * s).round() as i32;
    unsafe {
        // 只是挪位置（她在走路）时不重建圆角区域、不重画，免得闪
        let mut old = RECT::default();
        let _ = GetWindowRect(host, &mut old);
        let resized = old.right - old.left != w || old.bottom - old.top != h;
        let _ = SetWindowPos(host, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
        if !resized && !force {
            return;
        }
        let _ = SetWindowPos(edit, None, pad_x, (h - line * n) / 2, w - 2 * pad_x - close_w, line * n, SWP_NOZORDER | SWP_NOACTIVATE);
        let r = (28.0 * s).round() as i32;
        let rgn = CreateRoundRectRgn(0, 0, w + 1, h + 1, r, r);
        if SetWindowRgn(host, Some(rgn), true) == 0 {
            let _ = DeleteObject(HGDIOBJ(rgn.0));
        }
        let _ = InvalidateRect(Some(host), None, true);
    }
}

/// 字变了：行数变了就把框往上长/缩
fn fit_lines(host: HWND, edit: HWND) {
    let n = unsafe { SendMessageW(edit, EM_GETLINECOUNT, None, None).0 as i32 }.clamp(1, MAX_LINES);
    if LINES.swap(n, Ordering::Relaxed) == n {
        return;
    }
    let mut r = RECT::default();
    unsafe {
        let _ = GetWindowRect(host, &mut r);
    }
    let anchor = ANCHOR_BOTTOM.load(Ordering::Relaxed);
    let bottom = if anchor != 0 { anchor } else { r.bottom };
    place(host, edit, super::current_scale(), r.left, bottom, r.right - r.left, true);
}

/// ↑/↓ 翻发过的话（纯逻辑，可测）
#[derive(Default)]
pub struct InputHistory {
    items: Vec<String>,
    /// 正在看第几条；None = 没在翻
    pos: Option<usize>,
    /// 开始翻之前框里没发出去的字
    draft: String,
}
const HIST_MAX: usize = 50;
impl InputHistory {
    pub fn new(items: Vec<String>) -> Self {
        let start = items.len().saturating_sub(HIST_MAX);
        Self { items: items[start..].to_vec(), ..Self::default() }
    }
    pub fn push(&mut self, s: &str) {
        self.pos = None;
        self.draft.clear();
        if self.items.last().map(String::as_str) != Some(s) {
            self.items.push(s.to_string());
            if self.items.len() > HIST_MAX {
                self.items.remove(0);
            }
        }
    }
    /// 往前翻一条；已经到最早的就停在那（返回 None = 不动）
    pub fn up(&mut self, current: &str) -> Option<String> {
        let p = match self.pos {
            None if self.items.is_empty() => return None,
            None => {
                self.draft = current.to_string();
                self.items.len() - 1
            }
            Some(0) => return None,
            Some(p) => p - 1,
        };
        self.pos = Some(p);
        Some(self.items[p].clone())
    }
    /// 往后翻；翻过最新的一条回到没发出去的字
    pub fn down(&mut self) -> Option<String> {
        let p = self.pos?;
        if p + 1 < self.items.len() {
            self.pos = Some(p + 1);
            Some(self.items[p + 1].clone())
        } else {
            self.pos = None;
            Some(std::mem::take(&mut self.draft))
        }
    }
}

fn with_hist<R>(f: impl FnOnce(&mut InputHistory) -> R) -> R {
    let mut g = HIST.lock().unwrap_or_else(|e| e.into_inner());
    let h = g.get_or_insert_with(|| {
        // 第一次用：从完整聊天记录里拿主人说过的话
        let mine = crate::services::ai::load_log()
            .iter()
            .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
            .filter_map(|m| m.get("text").and_then(|t| t.as_str()).map(str::to_string))
            .collect();
        InputHistory::new(mine)
    });
    f(h)
}

/// 清空输入框的 ↑/↓ 历史缓存（「忘掉聊天记录」时同步调用）
pub fn clear_input_history() {
    *HIST.lock().unwrap_or_else(|e| e.into_inner()) = Some(InputHistory::default());
}

/// 同步多行 Edit 的字数标志、框高度和灰色提示字（ES_MULTILINE 在 SetWindowTextW 时不会发 EN_CHANGE，必须显式同步）
fn sync_edit(host: HWND, edit: HWND) {
    if host.is_invalid() || edit.is_invalid() {
        return;
    }
    let has = !edit_text(edit).trim().is_empty();
    HAS_TEXT.store(has, Ordering::Relaxed);
    fit_lines(host, edit);
    unsafe {
        let _ = InvalidateRect(Some(edit), None, true);
    }
}

fn set_edit(edit: HWND, text: &str) {
    let w = wide(&text.replace('\n', "\r\n"));
    let end = w.len().saturating_sub(1);
    unsafe {
        // WM_SETTEXT 在 edit_proc 里会同步触发 sync_edit
        let _ = SetWindowTextW(edit, PCWSTR(w.as_ptr()));
        // 光标放到最后（避免 64 位下 usize::MAX>>1 低 32 位截成 -1 导致取消选区但不挪光标）
        SendMessageW(edit, EM_SETSEL, Some(WPARAM(end)), Some(LPARAM(end as isize)));
    }
}

/// 光标在第几行 / 一共几行（包括自动折行）
fn caret_line(edit: HWND) -> (i32, i32) {
    unsafe {
        let cur = SendMessageW(edit, EM_LINEFROMCHAR, Some(WPARAM(usize::MAX)), None).0 as i32;
        let n = SendMessageW(edit, EM_GETLINECOUNT, None, None).0 as i32;
        (cur, n.max(1))
    }
}

fn key_down(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool { unsafe { GetKeyState(vk.0 as i32) < 0 } }

/// 输入框里有没有打了字（不算空白）。行为层判断「别的动作要不要顺手关掉空输入框」用
pub fn has_text() -> bool { HAS_TEXT.load(Ordering::Relaxed) }

/// 当前是否正处于等待 AI 回复状态
pub fn is_busy() -> bool { BUSY.load(Ordering::Relaxed) }

/// 聊天输入框是否正拥有键盘焦点
pub fn is_focused() -> bool {
    let e = EDIT_HWND.load(Ordering::Relaxed);
    e != 0 && unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetFocus().0 as isize == e }
}

unsafe fn delete_prev_word(hwnd: HWND) {
    if BUSY.load(Ordering::Relaxed) {
        return;
    }
    unsafe {
        let (mut s, mut e) = (0u32, 0u32);
        SendMessageW(hwnd, EM_GETSEL, Some(WPARAM(&mut s as *mut u32 as usize)), Some(LPARAM(&mut e as *mut u32 as isize)));
        if s == e && s > 0 {
            let to = prev_word_utf16(&edit_text(hwnd), s as usize);
            SendMessageW(hwnd, EM_SETSEL, Some(WPARAM(to)), Some(LPARAM(e as isize)));
        }
        if s != e || s > 0 {
            let empty = wide("");
            SendMessageW(hwnd, EM_REPLACESEL, Some(WPARAM(1)), Some(LPARAM(empty.as_ptr() as isize)));
            if let Ok(host) = GetParent(hwnd) {
                sync_edit(host, hwnd);
            }
        }
    }
}

/// Ctrl+Backspace 向前删词的落点（UTF-16 偏移）：跳过紧挨光标的空白，再删掉一整段同类型字符
pub(crate) fn prev_word_utf16(text: &str, caret_utf16: usize) -> usize {
    let mut chars: Vec<(usize, char)> = Vec::new();
    let mut u16_pos = 0usize;
    for ch in text.chars() {
        if u16_pos >= caret_utf16 {
            break;
        }
        chars.push((u16_pos, ch));
        u16_pos += ch.len_utf16();
    }
    let Some(&(_, last)) = chars.last() else { return 0 };
    if last == '\n' {
        chars.pop();
        if chars.last().is_some_and(|&(_, c)| c == '\r') {
            chars.pop();
        }
        return chars.last().map_or(0, |&(p, c)| p + c.len_utf16());
    }
    while chars.last().is_some_and(|&(_, c)| c.is_whitespace() && c != '\r' && c != '\n') {
        chars.pop();
    }
    let kind = |c: char| -> u8 {
        if c.is_ascii_alphanumeric() || c == '_' {
            1
        } else if ('\u{4e00}'..='\u{9fff}').contains(&c) || c.is_alphanumeric() {
            2
        } else if c.is_whitespace() {
            0
        } else {
            3
        }
    };
    let Some(&(_, target_ch)) = chars.last() else { return 0 };
    let k = kind(target_ch);
    if k == 0 {
        return chars.last().map_or(0, |&(p, c)| p + c.len_utf16());
    }
    while chars.last().is_some_and(|&(_, c)| kind(c) == k) {
        chars.pop();
    }
    chars.last().map_or(0, |&(p, c)| p + c.len_utf16())
}

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
        let mut text = wide(if BUSY.load(Ordering::Relaxed) { CUE_BUSY } else { CUE });
        text.pop(); // 去掉结尾的 0
        DrawTextW(hdc, &mut text, &mut rc, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
        SelectObject(hdc, of);
        ReleaseDC(Some(edit), hdc);
    }
}

fn edit_text(edit: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(edit).max(0);
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(edit, &mut buf);
        String::from_utf16_lossy(&buf[..got.max(0) as usize])
    }
}

unsafe extern "system" fn edit_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    unsafe {
        match msg {
            WM_KEYDOWN if wp.0 == VK_RETURN.0 as usize => {
                if BUSY.load(Ordering::Relaxed) {
                    return LRESULT(0);
                }
                if key_down(VK_SHIFT) {
                    // Shift+回车：换行
                    let nl = wide("\r\n");
                    SendMessageW(hwnd, EM_REPLACESEL, Some(WPARAM(1)), Some(LPARAM(nl.as_ptr() as isize)));
                    if let Ok(host) = GetParent(hwnd) {
                        sync_edit(host, hwnd);
                    }
                    return LRESULT(0);
                }
                let text = edit_text(hwnd).replace("\r\n", "\n").trim().to_string();
                if !text.is_empty() {
                    with_hist(|h| h.push(&text));
                    set_edit(hwnd, "");
                    super::post_event(AppEvent::ChatSubmit(text));
                }
                return LRESULT(0);
            }
            WM_KEYDOWN if wp.0 == VK_ESCAPE.0 as usize => {
                if BUSY.load(Ordering::Relaxed) {
                    super::post_event(AppEvent::ChatStop);
                } else {
                    super::chat_escape();
                }
                return LRESULT(0);
            }
            WM_KEYDOWN if (wp.0 == VK_UP.0 as usize || wp.0 == VK_DOWN.0 as usize) && !BUSY.load(Ordering::Relaxed) => {
                let (cur, n) = caret_line(hwnd);
                let up = wp.0 == VK_UP.0 as usize;
                // 光标在第一行按 ↑ / 最后一行按 ↓ 才翻；多行里面上下照常挪光标
                if (up && cur == 0) || (!up && cur >= n - 1) {
                    let now = edit_text(hwnd).replace("\r\n", "\n");
                    if let Some(t) = with_hist(|h| if up { h.up(&now) } else { h.down() }) {
                        set_edit(hwnd, &t);
                    }
                    return LRESULT(0);
                }
            }
            // Ctrl+A 全选（多行 Edit 默认不认）
            WM_KEYDOWN if wp.0 == 'A' as usize && key_down(VK_CONTROL) => {
                SendMessageW(hwnd, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
                return LRESULT(0);
            }
            // Ctrl+Backspace 向前删词（原生 Edit 默认会插一个 0x7F 方框字符）
            WM_KEYDOWN if wp.0 == VK_BACK.0 as usize && key_down(VK_CONTROL) => {
                ATE_CTRL_BACK.store(true, Ordering::Relaxed);
                delete_prev_word(hwnd);
                return LRESULT(0);
            }
            WM_KEYDOWN => {
                ATE_CTRL_BACK.store(false, Ordering::Relaxed);
            }
            WM_CHAR if wp.0 == 0x7f => {
                if !ATE_CTRL_BACK.swap(false, Ordering::Relaxed) {
                    delete_prev_word(hwnd);
                }
                return LRESULT(0);
            }
            // 吞掉回车/Esc/Ctrl+A/Ctrl+回车 的 WM_CHAR，否则会“叮”一声或插进怪字符
            WM_CHAR if [0x0d, 0x1b, 0x01, 0x0a].contains(&wp.0) => return LRESULT(0),
            WM_PAINT => {
                let r = DefSubclassProc(hwnd, msg, wp, lp);
                paint_cue(hwnd);
                return r;
            }
            WM_IME_STARTCOMPOSITION | WM_IME_ENDCOMPOSITION => {
                COMPOSING.store(msg == WM_IME_STARTCOMPOSITION, Ordering::Relaxed);
                let _ = InvalidateRect(Some(hwnd), None, true);
            }
            WM_SETTEXT => {
                let r = DefSubclassProc(hwnd, msg, wp, lp);
                if let Ok(host) = GetParent(hwnd) {
                    sync_edit(host, hwnd);
                }
                return r;
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
    static TRACKING: AtomicBool = AtomicBool::new(false);
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                let s = super::current_scale();
                let pen = CreatePen(PS_SOLID, (2.0 * s).round().max(1.0) as i32, BORDER);
                let cached_bg = BG_BRUSH.load(Ordering::Relaxed);
                let (brush, owned_brush) = if cached_bg != 0 {
                    (HBRUSH(cached_bg as *mut _), false)
                } else {
                    (CreateSolidBrush(BG), true)
                };
                let op = SelectObject(hdc, HGDIOBJ(pen.0));
                let ob = SelectObject(hdc, HGDIOBJ(brush.0));
                let r = (28.0 * s).round() as i32;
                let _ = RoundRect(hdc, rc.left + 1, rc.top + 1, rc.right - 1, rc.bottom - 1, r, r);
                SelectObject(hdc, op);
                SelectObject(hdc, ob);
                let _ = DeleteObject(HGDIOBJ(pen.0));
                if owned_brush {
                    let _ = DeleteObject(HGDIOBJ(brush.0));
                }
                paint_close(hdc, hwnd);
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_COMMAND if (wp.0 >> 16) as u32 & 0xffff == EN_CHANGE => {
                let edit = HWND(lp.0 as *mut _);
                sync_edit(hwnd, edit);
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
                if !TRACKING.swap(true, Ordering::Relaxed) {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut tme);
                }
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                TRACKING.store(false, Ordering::Relaxed);
                if CLOSE_HOT.swap(false, Ordering::Relaxed) {
                    let r = close_rect(hwnd);
                    let _ = InvalidateRect(Some(hwnd), Some(&r), false);
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => LRESULT(0),
            WM_LBUTTONUP => {
                if in_close(hwnd, lp) {
                    if CLOSE_HOT.swap(false, Ordering::Relaxed) {
                        let r = close_rect(hwnd);
                        let _ = InvalidateRect(Some(hwnd), Some(&r), false);
                    }
                    if BUSY.load(Ordering::Relaxed) {
                        super::post_event(AppEvent::ChatStop);
                    } else {
                        super::chat_escape();
                    }
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

    #[test]
    fn input_history_up_down() {
        let mut h = InputHistory::new(vec!["a".into(), "b".into()]);
        assert_eq!(h.down(), None, "没在翻时 ↓ 不动");
        assert_eq!(h.up("草稿").as_deref(), Some("b"));
        assert_eq!(h.up("b").as_deref(), Some("a"));
        assert_eq!(h.up("a"), None, "到最早的停住");
        assert_eq!(h.down().as_deref(), Some("b"));
        assert_eq!(h.down().as_deref(), Some("草稿"), "翻回来恢复没发的字");
        assert_eq!(h.down(), None);
        h.push("c");
        h.push("c");
        assert_eq!(h.up("").as_deref(), Some("c"));
        assert_eq!(h.up("c").as_deref(), Some("b"), "连续两次一样的只记一条");
        let mut e = InputHistory::new(Vec::new());
        assert_eq!(e.up("x"), None);
        let big = InputHistory::new((0..80).map(|i| i.to_string()).collect());
        assert_eq!(big.items.len(), HIST_MAX);
        assert_eq!(big.items[0], "30");
    }
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

    #[test]
    fn ctrl_backspace_word_boundaries() {
        let u16_len = |s: &str| s.encode_utf16().count();
        let t = "hello world  ";
        assert_eq!(prev_word_utf16(t, u16_len(t)), u16_len("hello "));
        let t2 = "哈基米 今天好累";
        assert_eq!(prev_word_utf16(t2, u16_len(t2)), u16_len("哈基米 "));
        let t3 = "测试！！！";
        assert_eq!(prev_word_utf16(t3, u16_len(t3)), u16_len("测试"));
        let t4 = "第一行\r\n";
        assert_eq!(prev_word_utf16(t4, u16_len(t4)), u16_len("第一行"));
    }
}
