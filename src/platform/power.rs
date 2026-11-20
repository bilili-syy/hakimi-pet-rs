//! 省电探测（伪 Live2D 用）：她被藏起来/最小化、前台是全屏程序（游戏、全屏视频）、锁屏，
//! 或者笔记本在用电池时，告诉渲染层把灵动效果关掉、退回老的低帧率（render::live::set_low_power）。
//! 每 PROBE_MS 在 UI 线程里查一次，几次 user32 调用，不起线程。
use std::cell::Cell;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetForegroundWindow, GetWindowRect, IsIconic, IsWindowVisible};

const PROBE_MS: u64 = 2000;

thread_local! {
    static NEXT: Cell<u64> = const { Cell::new(0) };
}

/// tick 里调用：到点了就探测一次并更新 render::live 的省电标志
pub fn maybe_probe(now: u64, pet: HWND) {
    if NEXT.with(|n| n.get()) > now { return; }
    NEXT.with(|n| n.set(now + PROBE_MS));
    crate::render::live::set_low_power(probe(pet));
}

fn probe(pet: HWND) -> bool {
    unsafe {
        if !IsWindowVisible(pet).as_bool() || IsIconic(pet).as_bool() { return true; }
        let fg = GetForegroundWindow();
        // 锁屏 / UAC 安全桌面时拿不到前台窗口
        if fg.is_invalid() { return true; }
        if fg != pet && is_fullscreen(fg, pet) { return true; }
    }
    on_battery()
}

/// 前台窗口盖住了她所在的整个显示器（不是任务栏/桌面）
unsafe fn is_fullscreen(fg: HWND, pet: HWND) -> bool {
    let mut cls = [0u16; 32];
    let n = unsafe { GetClassNameW(fg, &mut cls) } as usize;
    let cls = String::from_utf16_lossy(&cls[..n.min(cls.len())]);
    if matches!(cls.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") { return false; }
    let mon = unsafe { MonitorFromWindow(pet, MONITOR_DEFAULTTONEAREST) };
    if unsafe { MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST) } != mon { return false; }
    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    let mut r = RECT::default();
    if !unsafe { GetMonitorInfoW(mon, &mut mi) }.as_bool() || unsafe { GetWindowRect(fg, &mut r) }.is_err() { return false; }
    covers(&r, &mi.rcMonitor)
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
