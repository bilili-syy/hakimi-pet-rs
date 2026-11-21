//! 桌面小道具窗口：小鱼干/罐头（能拖、会掉到地上）、逗猫棒（跟着光标、点穿）。
//! 一次只有一个道具；生成时建窗口，收走时销毁。物理（下落/跟随）由 mod.rs 的 16ms 物理步驱动。
use super::{Dib, create_dib, emit, pump, with};
use crate::types::{AppEvent, ToyEv, ToyKind};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

pub struct Toy {
    pub hwnd: HWND,
    pub kind: ToyKind,
    pub x: i32,
    pub y: i32,
    pub size: i32,
    pub falling: bool,
    pub vy: f32,
    pub dragging: bool,
    pub drag_off: (i32, i32),
    _dib: Option<Dib>,
}

impl Drop for Toy {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn register() -> bool {
    use std::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.load(Ordering::Acquire) {
        return true;
    }
    unsafe {
        let Ok(m) = GetModuleHandleW(None) else { return false };
        let hinst: HINSTANCE = m.into();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_HAND).unwrap_or_default(),
            lpszClassName: w!("HakimiPetRsToy"),
            ..Default::default()
        };
        let ok = RegisterClassExW(&wc) != 0;
        if ok { DONE.store(true, Ordering::Release); }
        ok
    }
}

impl Toy {
    /// 建窗口并画好（左上角 (x, y)，物理像素）
    pub fn create(kind: ToyKind, x: i32, y: i32, scale: f32) -> Option<Toy> {
        if !register() {
            return None;
        }
        let frame = crate::render::toy::render(kind, scale);
        let size = frame.w as i32;
        if size <= 0 || frame.w != frame.h {
            return None;
        }
        let need = (size as usize).checked_mul(size as usize)?.checked_mul(4)?;
        if frame.bgra.len() < need {
            return None;
        }
        let mut ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE;
        if kind == ToyKind::Teaser {
            ex |= WS_EX_TRANSPARENT; // 逗猫棒跟着光标，不能挡住点击
        }
        let hinst: HINSTANCE = unsafe { GetModuleHandleW(None).ok()?.into() };
        let hwnd = unsafe {
            CreateWindowExW(ex, w!("HakimiPetRsToy"), w!("哈基米的小道具"), WS_POPUP, x, y, size, size, None, None, Some(hinst), None).ok()?
        };
        let Some(dib) = create_dib(size, size) else {
            unsafe { let _ = DestroyWindow(hwnd); }
            return None;
        };
        unsafe {
            std::ptr::copy_nonoverlapping(frame.bgra.as_ptr(), dib.bits, need);
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let _ = UpdateLayeredWindow(
                hwnd,
                None,
                Some(&POINT { x, y }),
                Some(&SIZE { cx: size, cy: size }),
                Some(dib.dc),
                Some(&POINT { x: 0, y: 0 }),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, size, size, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
        Some(Toy { hwnd, kind, x, y, size, falling: false, vy: 0.0, dragging: false, drag_off: (0, 0), _dib: Some(dib) })
    }

    pub fn move_to(&mut self, x: i32, y: i32) {
        if (x, y) == (self.x, self.y) {
            return;
        }
        self.x = x;
        self.y = y;
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    pub fn center(&self) -> (i32, i32) { (self.x + self.size / 2, self.y + self.size / 2) }
}

fn cursor() -> (i32, i32) {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

/// 松手：报告道具相对她嘴巴的位置
fn release() {
    let ev = with(|s| {
        let scale = s.scale;
        let (mx, my) = s.mouth();
        let t = s.toy.as_mut()?;
        if !t.dragging {
            return None;
        }
        t.dragging = false;
        let (cx, cy) = t.center();
        Some(AppEvent::Toy(ToyEv::Dropped { dx: (cx - mx) as f32 / scale, dy: (cy - my) as f32 / scale }))
    })
    .flatten();
    if let Some(e) = ev {
        emit(e);
        pump();
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_LBUTTONDOWN => {
                let _ = SetCapture(hwnd);
                let (cx, cy) = cursor();
                let grabbed = with(|s| {
                    let t = s.toy.as_mut()?;
                    if t.kind == ToyKind::Teaser {
                        return None;
                    }
                    t.dragging = true;
                    t.falling = false;
                    t.vy = 0.0;
                    t.drag_off = (cx - t.x, cy - t.y);
                    Some(())
                })
                .flatten()
                .is_some();
                if grabbed {
                    emit(AppEvent::Toy(ToyEv::Grabbed));
                    pump();
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let (cx, cy) = cursor();
                with(|s| {
                    if let Some(t) = s.toy.as_mut()
                        && t.dragging
                    {
                        let (ox, oy) = t.drag_off;
                        t.move_to(cx - ox, cy - oy);
                    }
                });
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let _ = ReleaseCapture();
                release();
                LRESULT(0)
            }
            WM_CAPTURECHANGED => {
                release();
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}
