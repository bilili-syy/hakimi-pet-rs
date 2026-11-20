//! 爬窗口：找前台窗口、判断能不能坐上去、坐着时看窗口有没有挪走。
//! 纯计算（window_perch / side_perch / moved）不碰 Win32，可单测。
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetClassNameW, GetForegroundWindow, GetWindowLongW, GetWindowRect, IsIconic, IsWindow,
    IsWindowVisible, IsZoomed, WS_EX_TOOLWINDOW,
};

/// 矩形（物理像素）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct R {
    pub l: i32,
    pub t: i32,
    pub r: i32,
    pub b: i32,
}

impl From<RECT> for R {
    fn from(r: RECT) -> Self { R { l: r.left, t: r.top, r: r.right, b: r.bottom } }
}

/// 坐到窗口 win 上沿时她窗口的左上角；坐不了返回 None。
/// work：窗口所在显示器的工作区；pet_cx：她现在的中线 x；pet_w/pet_h：她的窗口大小
pub fn window_perch(win: R, work: R, pet_cx: i32, pet_w: i32, pet_h: i32, scale: f32) -> Option<(i32, i32)> {
    let (pw, ph) = (pet_w as f32, pet_h as f32);
    if ((win.r - win.l) as f32) < pw * 0.7 || win.b - win.t < (60.0 * scale) as i32 {
        return None;
    }
    // 铺满工作区的（最大化/全屏）不算
    if win.l <= work.l + 2 && win.r >= work.r - 2 && win.t <= work.t + 2 && win.b >= work.b - 2 {
        return None;
    }
    // 上沿离地面太近没意思，离屏幕顶太近头会顶出去（允许气泡那块空白出屏）
    if (win.t as f32) > work.b as f32 - 150.0 * scale {
        return None;
    }
    let y = (win.t as f32 - ph + 8.0 * scale).round() as i32;
    if (y as f32) < work.t as f32 - 60.0 * scale {
        return None;
    }
    let lo = win.l.max(work.l) as f32;
    let hi = win.r.min(work.r) as f32;
    if hi - lo < pw * 0.5 {
        return None;
    }
    let (a, b) = (lo + pw * 0.3, hi - pw * 0.3);
    let cx = if a <= b { (pet_cx as f32).clamp(a, b) } else { (lo + hi) / 2.0 };
    Some(((cx - pw / 2.0).round() as i32, y))
}

/// 屏幕侧边（离她近的那边）：探出 2/3 个身子
pub fn side_perch(work: R, pet_cx: i32, pet_w: i32, pet_h: i32) -> (i32, i32) {
    let mid = (work.l + work.r) / 2;
    let x = if pet_cx < mid { work.l - pet_w / 3 } else { work.r - pet_w * 2 / 3 };
    (x, work.b - pet_h)
}

/// 窗口挪动了没有（2 像素以内算没动）
pub fn moved(a: R, b: R) -> bool {
    (a.l - b.l).abs() > 2 || (a.t - b.t).abs() > 2 || (a.r - b.r).abs() > 2
}

fn class_name(h: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// 窗口的可见边框（去掉 Win10/11 看不见的拖拽边）
pub fn frame_rect(h: HWND) -> Option<R> {
    unsafe {
        let mut r = RECT::default();
        if DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut RECT as *mut _, std::mem::size_of::<RECT>() as u32).is_ok()
            && r.right > r.left
        {
            return Some(r.into());
        }
        GetWindowRect(h, &mut r).ok()?;
        Some(r.into())
    }
}

/// 窗口现在还能坐吗（存在、可见、没最小化/最大化、没被藏起来）
pub fn usable(h: HWND) -> bool {
    unsafe {
        if h.0.is_null() || !IsWindow(Some(h)).as_bool() || !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() || IsZoomed(h).as_bool() {
            return false;
        }
        let mut cloaked: u32 = 0;
        if DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, 4).is_ok() && cloaked != 0 {
            return false;
        }
    }
    true
}

/// 当前前台窗口（排除自己、桌面、任务栏、工具窗口）
pub fn foreground(own: &[HWND]) -> Option<(HWND, R)> {
    let h = unsafe { GetForegroundWindow() };
    if h.0.is_null() || own.contains(&h) || !usable(h) {
        return None;
    }
    let ex = unsafe { GetWindowLongW(h, GWL_EXSTYLE) } as u32;
    if ex & WS_EX_TOOLWINDOW.0 != 0 {
        return None;
    }
    const SKIP: [&str; 8] = [
        "Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd", "NotifyIconOverflowWindow",
        "Windows.UI.Core.CoreWindow", "HakimiPetRsWindow", "HakimiPetRsToy",
    ];
    let cls = class_name(h);
    if SKIP.contains(&cls.as_str()) || cls.starts_with("#32768") {
        return None;
    }
    Some((h, frame_rect(h)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: R = R { l: 0, t: 0, r: 1920, b: 1040 };
    const PW: i32 = 360;
    const PH: i32 = 540;

    #[test]
    fn perch_on_a_normal_window() {
        let win = R { l: 400, t: 600, r: 1400, b: 1000 };
        let (x, y) = window_perch(win, WORK, 900, PW, PH, 1.5).unwrap();
        assert_eq!(y, 600 - PH + 12);
        assert_eq!(x, 900 - PW / 2);
        // 她在窗口外面：挪到窗口范围里
        let (x, _) = window_perch(win, WORK, 100, PW, PH, 1.5).unwrap();
        assert!(x + PW / 2 >= 400 + (PW as f32 * 0.3) as i32);
    }

    #[test]
    fn perch_rejects_bad_windows() {
        // 最大化/铺满
        assert!(window_perch(WORK, WORK, 900, PW, PH, 1.5).is_none());
        // 太窄
        assert!(window_perch(R { l: 400, t: 600, r: 520, b: 900 }, WORK, 900, PW, PH, 1.5).is_none());
        // 上沿太高：头顶出去
        assert!(window_perch(R { l: 400, t: 200, r: 1400, b: 900 }, WORK, 900, PW, PH, 1.5).is_none());
        // 上沿太低：贴着地面
        assert!(window_perch(R { l: 400, t: 960, r: 1400, b: 1040 }, WORK, 900, PW, PH, 1.5).is_none());
        // 大半在屏幕外
        assert!(window_perch(R { l: 1800, t: 600, r: 2800, b: 900 }, WORK, 900, PW, PH, 1.5).is_none());
    }

    #[test]
    fn side_and_moved() {
        assert_eq!(side_perch(WORK, 100, PW, PH), (-PW / 3, 1040 - PH));
        assert_eq!(side_perch(WORK, 1500, PW, PH), (1920 - PW * 2 / 3, 1040 - PH));
        let a = R { l: 10, t: 10, r: 100, b: 100 };
        assert!(!moved(a, R { l: 11, t: 12, r: 101, b: 300 }));
        assert!(moved(a, R { l: 30, t: 10, r: 120, b: 100 }));
    }
}
