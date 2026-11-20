//! 高精度单次定时器（Win10 1803+ 的 CREATE_WAITABLE_TIMER_HIGH_RESOLUTION）。
//! SetTimer 的粒度是 15.6ms：要 16ms 实际会在 15.6/31.2 之间跳，只有 ~40fps，更别说 120fps。
//! 短间隔（< SHORT_MS）额外挂一个高精度定时器，消息循环用 MsgWaitForMultipleObjectsEx 等它；
//! SetTimer 照样挂着兜底（菜单/对话框的模态循环里只有 WM_TIMER 能进来）。创建失败就只用 SetTimer。
use std::sync::atomic::{AtomicIsize, Ordering};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CancelWaitableTimer, CreateWaitableTimerExW, SetWaitableTimer,
};
use windows::core::PCWSTR;

static TIMER: AtomicIsize = AtomicIsize::new(0);
/// 小于这个间隔才用高精度定时器；更长的等待交给 SetTimer 就够准，也更省电
pub const SHORT_MS: u32 = 40;
/// TIMER_ALL_ACCESS
const ACCESS: u32 = 0x1F_0003;

pub fn init() {
    let h = unsafe { CreateWaitableTimerExW(None, PCWSTR::null(), CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, ACCESS) };
    match h {
        Ok(h) if !h.is_invalid() => TIMER.store(h.0 as isize, Ordering::Relaxed),
        _ => crate::core::log::log("high-res timer unavailable, using SetTimer"),
    }
}

pub fn handle() -> Option<HANDLE> {
    let v = TIMER.load(Ordering::Relaxed);
    (v != 0).then_some(HANDLE(v as *mut _))
}

/// 和 SetTimer 同时调：短间隔挂高精度定时器，长间隔把它取消
pub fn arm(ms: u32) {
    let Some(h) = handle() else { return };
    unsafe {
        if ms < SHORT_MS {
            // 负数 = 相对时间，单位 100ns
            let due = -(ms.max(1) as i64) * 10_000;
            let _ = SetWaitableTimer(h, &due, 0, None, None, false);
        } else {
            let _ = CancelWaitableTimer(h);
        }
    }
}
