//! 省电探测（伪 Live2D 用）：她被藏起来/最小化、前台是全屏程序（游戏、全屏视频）、锁屏，
//! 或者笔记本在用电池时，告诉渲染层把灵动效果关掉、退回老的低帧率（render::live::set_low_power）。
//! 每 PROBE_MS 在 UI 线程里查一次，几次 user32 调用，不起线程。
use std::cell::Cell;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::UI::Shell::{
    QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_STYLE, GetClassNameW, GetForegroundWindow, GetWindowLongW, GetWindowRect, IsIconic, IsWindowVisible,
    WS_CAPTION, WS_THICKFRAME,
};

const PROBE_MS: u64 = 2000;

thread_local! {
    static NEXT: Cell<u64> = const { Cell::new(0) };
    static LAST_FS: Cell<(u64, bool)> = const { Cell::new((0, false)) };
}

/// tick 里调用：到点了就探测一次并更新 render::live 的省电标志
pub fn maybe_probe(now: u64, pet: HWND) {
    if NEXT.with(|n| n.get()) > now { return; }
    NEXT.with(|n| n.set(now + PROBE_MS));
    let (low, fs) = probe(pet);
    LAST_FS.with(|c| c.set((now, fs)));
    crate::render::live::set_low_power(low);
}

fn probe(pet: HWND) -> (bool, bool) {
    unsafe {
        let fg = GetForegroundWindow();
        let fs = !fg.is_invalid() && fg != pet && is_fullscreen(fg, pet);
        if !IsWindowVisible(pet).as_bool() || IsIconic(pet).as_bool() { return (true, fs); }
        // 锁屏 / UAC 安全桌面时拿不到前台窗口
        if fg.is_invalid() { return (true, false); }
        if fs { return (true, true); }
    }
    (on_battery(), false)
}

/// 前台窗口盖住了她所在的整个显示器（不是任务栏/桌面，也不误判自动隐藏任务栏下的普通最大化窗口）。
/// 结合 SHQueryUserNotificationState 兜底 D3D 独占全屏、演示模式以及 F11 保留窗口样式的全屏程序。
unsafe fn is_fullscreen(fg: HWND, pet: HWND) -> bool {
    let quns = unsafe { SHQueryUserNotificationState() }.ok();
    if matches!(quns, Some(QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE)) {
        return true;
    }
    let mut cls = [0u16; 32];
    let n = unsafe { GetClassNameW(fg, &mut cls) } as usize;
    let cls = String::from_utf16_lossy(&cls[..n.min(cls.len())]);
    if matches!(cls.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") { return false; }
    // 普通桌面态（非 QUNS_BUSY）下，排除带标准标题栏+可调边框的窗口，避免把自动隐藏任务栏时的最大化误判为全屏；
    // 若 Shell 已判定为 QUNS_BUSY（如 F11 全屏但未去掉样式位），则放行后续尺寸覆盖校验。
    let style = unsafe { GetWindowLongW(fg, GWL_STYLE) } as u32;
    if quns != Some(QUNS_BUSY) && (style & WS_CAPTION.0) == WS_CAPTION.0 && (style & WS_THICKFRAME.0) != 0 {
        return false;
    }
    let mon = unsafe { MonitorFromWindow(pet, MONITOR_DEFAULTTONEAREST) };
    if unsafe { MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST) } != mon { return false; }
    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    if !unsafe { GetMonitorInfoW(mon, &mut mi) }.as_bool() { return false; }
    let mut r = RECT::default();
    let got_dwm = unsafe {
        DwmGetWindowAttribute(fg, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut RECT as *mut _, std::mem::size_of::<RECT>() as u32).is_ok()
    };
    if !got_dwm && unsafe { GetWindowRect(fg, &mut r) }.is_err() { return false; }
    covers(&r, &mi.rcMonitor)
}

/// 前台是不是全屏程序（不算她自己）。若 power 刚刚探测过则直接复用结果，省掉重复 Win32 查询
pub unsafe fn fullscreen_now(now: u64, pet: HWND) -> bool {
    let (t, fs) = LAST_FS.with(|c| c.get());
    if t > 0 && now.saturating_sub(t) < PROBE_MS {
        return fs;
    }
    let fg = unsafe { GetForegroundWindow() };
    let fs = !fg.is_invalid() && fg != pet && unsafe { is_fullscreen(fg, pet) };
    LAST_FS.with(|c| c.set((now, fs)));
    fs
}

fn covers(r: &RECT, m: &RECT) -> bool { r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom }

fn on_battery() -> bool {
    let mut st = SYSTEM_POWER_STATUS::default();
    // ACLineStatus：0 = 电池，1 = 插电，255 = 未知（台式机）
    unsafe { GetSystemPowerStatus(&mut st) }.is_ok() && st.ACLineStatus == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_monitor() {
        let m = RECT { left: 0, top: 0, right: 2560, bottom: 1440 };
        assert!(covers(&RECT { left: 0, top: 0, right: 2560, bottom: 1440 }, &m));
        assert!(covers(&RECT { left: -8, top: -8, right: 2568, bottom: 1448 }, &m));
        // 最大化（让出任务栏）不算全屏
        assert!(!covers(&RECT { left: -8, top: -8, right: 2568, bottom: 1392 }, &m));
        assert!(!covers(&RECT { left: 100, top: 100, right: 900, bottom: 700 }, &m));
    }
}
