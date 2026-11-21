//! 平台层（负责人：A 路「窗口与系统」）：Win32 分层窗口、消息循环、鼠标、拖拽/下落/溜达物理、
//! 托盘与右键菜单、聊天输入框、单实例、开机自启、摸鱼监督、提醒与番茄钟。
//! 公开函数签名是约定，别改名改参数；内部实现随意。
//!
//! 线程：全部在 UI 线程。别的线程只通过 EventSender（队列 + PostMessage）进来。
//! 定时：只有一个 one-shot SetTimer，每次按「下一帧 / 物理 / 提醒 / 番茄钟 / 摸鱼采样」里最早的到期时间重排，
//! 空闲时完全睡在 GetMessage 里，不忙等。
//! 重入：Handler 和平台状态各放一个 RefCell；调用 Handler 期间不持有平台状态，反之亦然。
//! 事件统一进 PENDING 队列按顺序投递，菜单动作的「平台额外动作」作为一条 After 排在 Handler 之后。
#![allow(dead_code)]

mod chat;
mod memo_menu;
mod mind_menu;
mod skin_menu;
mod menu;
mod perch;
mod power;
mod report;
mod sense;
mod settings;
mod sys;
mod timers;
mod hrtimer;
pub mod ipc;
mod toy;
mod watch;
#[cfg(test)]
mod selftest;

use crate::types::{
    AppEvent, Button, ChatInputCmd, EventSender, Frame, Handler, MenuAction, MouseEv, PerchKind, PomodoroEv, ToyCmd,
    ToyEv, ToyKind, WIN_H, WIN_W, WinCmd,
};
use chat::Chat;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, AtomicU32, AtomicU64, Ordering};
use timers::{LocalTime, Pomodoro, Reminders};
use watch::Supervisor;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC,
    CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetMonitorInfoW, HBITMAP, HDC, HGDIOBJ,
    MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint, MonitorFromWindow, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

pub(crate) const WM_APP_EVENT: u32 = WM_APP + 1;
pub(crate) const WM_APP_MENU: u32 = WM_APP + 2;
pub(crate) const WM_APP_SECOND: u32 = WM_APP + 3;
const WM_APP_DPI: u32 = WM_APP + 4;
const WM_APP_CLEAR_MIND: u32 = WM_APP + 5;
/// 自动化/自测用：PostMessage(hwnd, WM_APP+9, i, 0) 等于点了 DEBUG_MENU[i] 那个菜单项
const WM_APP_DEBUG_MENU: u32 = WM_APP + 9;
/// 10 = 打开设置窗口，11 = 打开聊天框
const DEBUG_MENU: [&str; 12] =
    ["feed", "feednow", "game", "climb", "weather", "checkin", "status", "walk", "pat", "dance:", "settings", "chat"];
/// 自测用：PostMessage(hwnd, WM_APP+10, 0, 0) = 在她身上弹出右键菜单（截图用）
const WM_APP_DEBUG_POPUP: u32 = WM_APP + 10;
const TIMER_ID: usize = 1;
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_POWERBROADCAST: u32 = 0x0218;
const WM_TIMECHANGE: u32 = 0x001E;
/// 备忘提醒最多隔多久重新核对一次本地时间（改了系统时间、睡眠醒来、手改 memo.json 都能跟上）
const MEMO_RECHECK: u64 = 5 * 60_000;
/// 溜达/下落的物理步长（原版 setInterval 16）
const PHYS_STEP: u64 = 16;
/// 有输入事件后最快多久补一帧
const EVENT_FRAME_GAP: u64 = 16;

static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);
static SCALE_BITS: AtomicU32 = AtomicU32::new(0x3f80_0000); // 1.0f32
/// 系统 DPI 缩放（不含用户在设置里选的大小倍数）
static DPI_BITS: AtomicU32 = AtomicU32::new(0x3f80_0000);
static EVENT_QUEUE: Mutex<VecDeque<AppEvent>> = Mutex::new(VecDeque::new());
pub(crate) static MENU_QUEUE: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
/// 番茄钟结束时间（GetTickCount64 单调毫秒，计入睡眠/休眠时间），0 = 没开
static POMO_END: AtomicU64 = AtomicU64::new(0);
static MOYU_TODAY_SEC: AtomicU32 = AtomicU32::new(0);

pub(crate) fn main_hwnd() -> Option<HWND> {
    let h = MAIN_HWND.load(Ordering::Relaxed);
    (h != 0).then_some(HWND(h as *mut _))
}

pub(crate) fn current_scale() -> f32 {
    f32::from_bits(SCALE_BITS.load(Ordering::Relaxed))
}

thread_local! {
    /// 外部事件限流（platform/ipc.rs）
    static EXT_THROTTLE: RefCell<ipc::Throttle> = RefCell::new(ipc::Throttle::default());
    /// 全屏程序（游戏等）在前台时是我们把她藏起来的：退出全屏再放出来；主人自己藏的不管
    static FS_HIDDEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// 全屏自动隐藏：(现在全屏没有, 她现在可见没有, 之前是不是全屏藏的) → (要不要改可见性, 新标记)
fn fs_autohide(fs: bool, visible: bool, hidden_by_fs: bool) -> (Option<bool>, bool) {
    match (fs, visible, hidden_by_fs) {
        (true, true, _) => (Some(false), true),
        (true, false, h) => (None, h),
        (false, _, true) => (Some(true), false),
        (false, _, false) => (None, false),
    }
}

fn apply_fs_autohide(fs: bool) {
    let visible = with(|s| s.is_visible()).unwrap_or(true);
    // 设置里关了：不再藏；之前藏着的放出来
    let fs = fs && crate::core::config::get().fullscreen_hide;
    let (act, flag) = fs_autohide(fs, visible, FS_HIDDEN.with(|c| c.get()));
    FS_HIDDEN.with(|c| c.set(flag));
    if let Some(on) = act {
        crate::core::log::trace(&format!("fullscreen autohide: visible={on}"));
        with(|s| s.set_visible(on));
        menu::refresh();
    }
}

/// 单调毫秒时钟（起点 1000ms，永不回拨、永不为 0），驱动帧循环/物理/提醒间隔
pub(crate) fn now_ms() -> u64 {
    static ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    1_000 + ORIGIN.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
}

/// 番茄钟专用单调毫秒时钟：Windows GetTickCount64 明确计入 S3 睡眠和 S4 休眠时间，且不受手动改系统时间影响
pub(crate) fn pomo_now_ms() -> u64 {
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() }.max(1)
}

/// 任意线程：把事件送回 UI 线程
pub(crate) fn post_event(ev: AppEvent) {
    EVENT_QUEUE.lock().unwrap_or_else(|e| e.into_inner()).push_back(ev);
    if let Some(h) = main_hwnd() {
        unsafe {
            let _ = PostMessageW(Some(h), WM_APP_EVENT, WPARAM(0), LPARAM(0));
        }
    }
}

// ---------------------------------------------------------------- 公开接口

/// 创建窗口、托盘，跑消息循环直到退出。
/// make 拿到 EventSender（内部 PostMessage 到 UI 线程）后构造 Handler。
pub fn run(make: impl FnOnce(EventSender) -> Box<dyn Handler>) {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    // 只允许开一只哈基米；第二个实例通知第一个后直接退出（不构造 Handler）
    let Some(_mutex) = sys::single_instance(WM_APP_SECOND) else {
        crate::core::log::log("second instance, exit");
        return;
    };
    crate::core::save::cleanup_stale_tmps();
    crate::core::session::begin();
    let Some(hwnd) = create_main_window() else {
        crate::core::log::log("create window failed");
        crate::core::session::end();
        return;
    };
    MAIN_HWND.store(hwnd.0 as isize, Ordering::Relaxed);

    let dpi = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0;
    let dpi = if dpi > 0.0 { dpi } else { 1.0 };
    DPI_BITS.store(dpi.to_bits(), Ordering::Relaxed);
    // 实际缩放 = 系统 DPI × 设置里的「哈基米大小」
    let scale = dpi * crate::core::config::get().scale;
    SCALE_BITS.store(scale.to_bits(), Ordering::Relaxed);
    let w = (WIN_W as f32 * scale).round() as i32;
    let h = (WIN_H as f32 * scale).round() as i32;
    // 屏幕偏右的位置，不挤在托盘时钟旁边
    let wa = work_area_at(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
    let x = (wa.left as f32 + (wa.right - wa.left) as f32 * 0.62 - w as f32 / 2.0).round() as i32;
    let y = wa.bottom - h;
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
    }

    let tx = EventSender::new(post_event);
    let handler = make(tx);
    HANDLER.with(|c| *c.borrow_mut() = Some(handler));

    let hinst: HINSTANCE = unsafe { GetModuleHandleW(None).map(|m| m.into()).unwrap_or_default() };
    let chat = Chat::new(hwnd, hinst, scale);
    let now = now_ms();
    let cfg = crate::core::config::get();
    STATE.with(|c| {
        *c.borrow_mut() = Some(Plat {
            hwnd,
            scale,
            x,
            y,
            w,
            h,
            dib: None,
            walk: None,
            fall: None,
            phys_last: now,
            phys_acc: 0,
            drag_off: (0, 0),
            next_frame: now,
            last_frame: 0,
            reminders: Reminders::schedule(&cfg.reminders, now),
            pomodoro: None,
            sup: Supervisor::new(LocalTime::now().day_key()),
            watch_next: cfg.moyu.enabled.then_some(now + watch::STEP as u64 * 1000),
            dbl_pending: false,
            tracking: false,
            chat,
            quitting: false,
            shown_once: false,
            hop: None,
            perch: None,
            toy: None,
            cursor_poll: 0,
            cursor_next: 0,
            last_cursor: (i32::MIN, i32::MIN),
        })
    });
    menu::init();
    hrtimer::init();
    rearm();

    unsafe {
        let mut msg = MSG::default();
        if let Some(t) = hrtimer::handle() {
            // 高精度定时器到点 = 一次 tick；其余照常分发消息
            'pump: loop {
                let r = MsgWaitForMultipleObjectsEx(Some(&[t]), windows::Win32::System::Threading::INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
                if r == windows::Win32::Foundation::WAIT_FAILED {
                    // 句柄异常时回退到阻塞 GetMessage，防止 100% CPU 空转
                    if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
                        break 'pump;
                    }
                    if !settings::dialog_message(&msg) {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    continue;
                }
                if r == windows::Win32::Foundation::WAIT_OBJECT_0 {
                    tick();
                }
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == WM_QUIT {
                        break 'pump;
                    }
                    // 设置窗口：Tab 切换、回车保存、Esc 取消
                    if settings::dialog_message(&msg) {
                        continue;
                    }
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        } else {
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                if settings::dialog_message(&msg) {
                    continue;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    menu::shutdown();
    STATE.with(|c| c.borrow_mut().take());
    HANDLER.with(|c| c.borrow_mut().take());
    MAIN_HWND.store(0, Ordering::Relaxed);
}

/// 番茄钟剩余分钟，没开返回 None
pub fn pomodoro_left() -> Option<u32> {
    let end = POMO_END.load(Ordering::Relaxed);
    (end != 0).then(|| timers::minutes_left(end, pomo_now_ms()))
}

/// 今天摸鱼了几分钟；摸鱼监督关闭时返回 None
pub fn moyu_today() -> Option<u32> {
    if !crate::core::config::get().moyu.enabled {
        return None;
    }
    Some((MOYU_TODAY_SEC.load(Ordering::Relaxed) as f64 / 60.0).round() as u32)
}

/// 用资源管理器 / 默认程序打开文件或文件夹（ShellExecuteW）
pub fn open_path(path: &std::path::Path) {
    sys::open_path(path)
}

// ---------------------------------------------------------------- 状态

struct Walk {
    /// 在追道具（走完发 Toy(Reached) 而不是 WalkDone）
    chase: bool,
    dir: i32,
    speed: f32,
    distance: f32,
    moved: f32,
    xf: f32,
    min_x: f32,
    max_x: f32,
}

/// 跳上窗口/屏幕边的抛物线
struct Hop {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    t0: u64,
    dur: u64,
    arc: f32,
    kind: PerchKind,
    target: Option<(HWND, perch::R)>,
}

/// 正坐在某个窗口上沿
struct Perch {
    hwnd: HWND,
    rect: perch::R,
    next: u64,
}

const PERCH_CHECK: u64 = 250;

struct Fall {
    vy: f32,
    xf: f32,
    yf: f32,
    floor: f32,
}

struct Dib {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    w: i32,
    h: i32,
}

impl Drop for Dib {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(HGDIOBJ(self.bmp.0));
            let _ = DeleteDC(self.dc);
        }
    }
}

struct Plat {
    hwnd: HWND,
    scale: f32,
    /// 窗口位置/大小（物理像素）
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    dib: Option<Dib>,
    walk: Option<Walk>,
    fall: Option<Fall>,
    phys_last: u64,
    phys_acc: u64,
    /// 左键按下时光标相对窗口左上角的偏移
    drag_off: (i32, i32),
    next_frame: u64,
    last_frame: u64,
    reminders: Reminders,
    pomodoro: Option<Pomodoro>,
    sup: Supervisor,
    watch_next: Option<u64>,
    dbl_pending: bool,
    tracking: bool,
    chat: Option<Chat>,
    quitting: bool,
    shown_once: bool,
    hop: Option<Hop>,
    perch: Option<Perch>,
    toy: Option<toy::Toy>,
    /// 光标轮询间隔（0 = 关）
    cursor_poll: u32,
    cursor_next: u64,
    last_cursor: (i32, i32),
}

enum Pending {
    Ev(AppEvent),
    /// Handler 处理完这个菜单动作之后，平台层自己要做的事
    After(MenuAction),
}

thread_local! {
    static STATE: RefCell<Option<Plat>> = const { RefCell::new(None) };
    static HANDLER: RefCell<Option<Box<dyn Handler>>> = RefCell::new(None);
    static PENDING: RefCell<VecDeque<Pending>> = const { RefCell::new(VecDeque::new()) };
    static PUMPING: Cell<bool> = const { Cell::new(false) };
    static QUITTING: Cell<bool> = const { Cell::new(false) };
}

/// 借平台状态（重入时借不到就返回 None）
fn with<R>(f: impl FnOnce(&mut Plat) -> R) -> Option<R> {
    STATE.with(|c| {
        let mut g = c.try_borrow_mut().ok()?;
        g.as_mut().map(f)
    })
}

fn with_handler<R>(f: impl FnOnce(&mut dyn Handler) -> R) -> Option<R> {
    HANDLER.with(|c| {
        let mut g = c.try_borrow_mut().ok()?;
        g.as_mut().map(|h| f(h.as_mut()))
    })
}

fn emit(ev: AppEvent) {
    PENDING.with(|q| q.borrow_mut().push_back(Pending::Ev(ev)));
}

/// 依次把待投递事件交给 Handler，执行它返回的 WinCmd
fn pump() {
    if PUMPING.with(|p| p.replace(true)) {
        return;
    }
    let mut any = false;
    while let Some(p) = PENDING.with(|q| q.borrow_mut().pop_front()) {
        if QUITTING.with(|q| q.get()) {
            continue;
        }
        any = true;
        match p {
            Pending::Ev(ev) => {
                let mut out = Vec::new();
                with_handler(|h| h.event(ev, &mut out));
                apply(out);
            }
            Pending::After(a) => after(a),
        }
    }
    if any {
        // 有事发生：尽快补一帧，让画面跟上
        with(|s| s.next_frame = s.next_frame.min((s.last_frame + EVENT_FRAME_GAP).max(now_ms())));
    }
    PUMPING.with(|p| p.set(false));
    rearm();
}

/// 重排唯一的定时器到最早的到期时间
fn rearm() {
    with(|s| {
        if s.quitting {
            return;
        }
        let now = now_ms();
        let mut due = s.next_frame;
        if s.phys_active() {
            due = due.min(s.phys_last + PHYS_STEP);
        }
        if let Some(p) = &s.perch {
            due = due.min(p.next);
        }
        if s.cursor_poll > 0 {
            due = due.min(s.cursor_next);
        }
        if let Some(t) = s.watch_next {
            due = due.min(t);
        }
        if let Some(t) = s.reminders.next_deadline() {
            due = due.min(t);
        }
        if let Some(p) = &s.pomodoro {
            due = due.min(now + p.end.saturating_sub(pomo_now_ms()));
        }
        // 备忘录：排到下一条提醒的时间
        if let Some(ms) = crate::core::memo::ms_until_next(crate::core::memo::Now::get()) {
            due = due.min(now + ms.min(MEMO_RECHECK));
        }
        let ms = due.saturating_sub(now).clamp(1, 0x7fff_ffff) as u32;
        unsafe {
            SetTimer(Some(s.hwnd), TIMER_ID, ms, None);
        }
        hrtimer::arm(ms);
    });
}

/// 两小时内要响的备忘提醒，先把语音合成进缓存（数据变了或每 10 分钟看一次）
fn memo_prewarm() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST: AtomicU64 = AtomicU64::new(u64::MAX);
    let now = crate::core::memo::Now::get().dt;
    let key = (crate::core::memo::version() << 32) | ((now.to_min() / 10) as u64 & 0xffff_ffff);
    if LAST.swap(key, Ordering::Relaxed) == key { return; }
    let soon = now.add_min(120);
    let items: Vec<_> = crate::core::memo::pending()
        .into_iter()
        .filter(|i| i.due().is_some_and(|d| d <= soon))
        .flat_map(|i| crate::behavior::memo_voices(&i.text))
        .collect();
    if !items.is_empty() {
        crate::services::tts::prewarm_now(items);
    }
}

thread_local! {
    /// 备忘录轮询节流（至多每秒查一次，睡眠醒来/改时间立即清零）
    static MEMO_NEXT: Cell<u64> = const { Cell::new(0) };
    /// 主窗口左键是否处于按下/捕获中（配合 WM_CAPTURECHANGED 防止拖动卡死）
    static LEFT_DOWN: Cell<bool> = const { Cell::new(false) };
}

fn tick() {
    // 正在 pump() 里重入（如模态弹窗）：不要 1ms 重排空转，pump 结束时会统一 rearm()
    if PUMPING.with(|p| p.get()) {
        return;
    }
    let now = now_ms();
    let mut evs = Vec::new();
    let pomo_done = with(|s| {
        s.physics(now, &mut evs);
        s.check_perch(now, &mut evs);
        s.poll_cursor(now, &mut evs);
        s.timers(now, &mut evs)
    })
    .unwrap_or(false);
    if pomo_done {
        menu::refresh();
    }
    // 备忘录到点（最多每秒核对一次，避免 60~120Hz 每帧锁表；睡眠醒来/改时间会重置 MEMO_NEXT）
    if now >= MEMO_NEXT.with(|m| m.get()) {
        MEMO_NEXT.with(|m| m.set(now + 1000));
        let hits = crate::core::memo::poll(crate::core::memo::Now::get());
        if !hits.is_empty() {
            evs.push(AppEvent::Memo(crate::types::MemoEv::Due(hits)));
            menu::refresh();
        }
        memo_prewarm();
    }
    // 省电探测（藏起来/全屏程序/锁屏/电池 → 灵动效果降帧），每 2 秒一次；放在 sense 前让 sense 复用全屏探测结果
    if let Some(h) = with(|s| s.hwnd) {
        power::maybe_probe(now, h);
    }
    // 场景感知：走开/回来、打字、晃鼠标、全屏、音乐
    if let Some(h) = with(|s| s.hwnd) {
        sense::maybe_poll(now, h, &mut evs);
    }
    for e in &evs {
        if let AppEvent::Sense(crate::types::SenseEv::Fullscreen(fs)) = e { apply_fs_autohide(*fs); }
    }
    for e in evs {
        emit(e);
    }
    pump();
    // 帧
    let due = with(|s| !s.quitting && now >= s.next_frame).unwrap_or(false);
    if due {
        let mut out = Vec::new();
        let delay = with_handler(|h| h.frame(now, &mut out)).unwrap_or(200);
        apply(out);
        let scale = current_scale();
        with_handler(|h| {
            if let Some(f) = h.render(scale) {
                with(|s| s.blit(f));
            }
        });
        with(|s| {
            s.last_frame = now;
            s.next_frame = now + delay.max(1) as u64;
        });
        pump(); // apply 可能产生了事件
    }
    rearm();
}

/// 执行 Handler 返回的窗口命令
fn apply(out: Vec<WinCmd>) {
    for cmd in out {
        match cmd {
            WinCmd::Quit => {
                do_quit();
                return;
            }
            WinCmd::Walk { dir, distance, speed } => {
                with(|s| s.start_walk(dir, distance, speed));
            }
            WinCmd::WalkStop => {
                with(|s| s.walk = None);
            }
            WinCmd::CursorPoll(ms) => {
                with(|s| {
                    s.cursor_poll = ms;
                    s.cursor_next = now_ms();
                });
            }
            WinCmd::Climb(kind) => {
                if let Some(ev) = with(|s| s.climb(kind)).flatten() {
                    emit(ev);
                }
            }
            WinCmd::Unperch => {
                with(|s| {
                    s.perch = None;
                    s.hop = None;
                    s.walk = None;
                    s.start_fall();
                });
            }
            WinCmd::Toy(t) => {
                // 先把旧 Toy 移出 STATE 再销毁窗口，避免 DestroyWindow 同步派发 WM_CAPTURECHANGED 时重入 STATE.borrow_mut()
                let old_toy = if matches!(t, ToyCmd::Spawn { .. } | ToyCmd::Remove) {
                    with(|s| s.toy.take()).flatten()
                } else {
                    None
                };
                drop(old_toy);
                if let Some(ev) = with(|s| s.toy_cmd(t)).flatten() {
                    emit(ev);
                }
            }
            WinCmd::DragMove { sx, sy } => {
                with(|s| {
                    s.fall = None;
                    s.walk = None;
                    s.hop = None;
                    s.perch = None;
                    s.move_to(sx - s.drag_off.0, sy - s.drag_off.1);
                });
            }
            WinCmd::DragEnd => {
                with(|s| s.start_fall());
            }
            WinCmd::Show => {
                with(|s| s.set_visible(true));
                menu::refresh();
            }
            WinCmd::Hide => {
                with(|s| s.set_visible(false));
                menu::refresh();
            }
            WinCmd::ChatInput(c) => {
                with(|s| {
                    let (x, y, h) = (s.x, s.y, s.h);
                    if let Some(chat) = s.chat.as_mut() {
                        match c {
                            ChatInputCmd::Open => {
                                chat.layout(x, y, h);
                                chat.open();
                            }
                            ChatInputCmd::Close => chat.close(),
                            ChatInputCmd::Enable(on) => chat.enable(on),
                        }
                    }
                });
            }
        }
    }
    rearm();
}

/// 菜单 / 托盘点击
fn on_menu(a: MenuAction) {
    match &a {
        // 先读一遍文件，Handler 拿到的就是新配置
        MenuAction::ReloadSettings => {
            crate::core::config::reload();
        }
        // 原版：win.show(); win.focus(); 再通知渲染进程开聊天
        MenuAction::Chat => {
            with(|s| s.set_visible(true));
        }
        _ => {}
    }
    PENDING.with(|q| {
        let mut q = q.borrow_mut();
        q.push_back(Pending::Ev(AppEvent::Menu(a.clone())));
        q.push_back(Pending::After(a));
    });
}

/// 平台类菜单动作（docs/ARCHITECTURE.md「菜单动作谁来执行」）
fn after(a: MenuAction) {
    let now = now_ms();
    match a {
        MenuAction::ToggleVisible => {
            // 主人自己动了显示/隐藏：以后全屏退出时不再替她做主
            FS_HIDDEN.with(|c| c.set(false));
            let visible = with(|s| s.is_visible()).unwrap_or(true);
            with(|s| s.set_visible(!visible));
            if !visible {
                emit(AppEvent::Peek);
            }
        }
        MenuAction::CallToCenter => {
            with(|s| s.call_to_center());
            emit(AppEvent::Peek);
        }
        MenuAction::PomodoroStart(m) => {
            with(|s| {
                let p = Pomodoro::start(m, pomo_now_ms());
                POMO_END.store(p.end, Ordering::Relaxed);
                s.pomodoro = Some(p);
            });
            emit(AppEvent::Pomodoro(PomodoroEv::Start { minutes: m }));
        }
        MenuAction::PomodoroStop => {
            let had = with(|s| s.pomodoro.take().is_some()).unwrap_or(false);
            POMO_END.store(0, Ordering::Relaxed);
            if had {
                emit(AppEvent::Pomodoro(PomodoroEv::Cancel));
            }
        }
        MenuAction::AutoStart(on) => sys::set_autostart(on),
        MenuAction::OpenSettings => settings::open(0),
        MenuAction::OpenConfigFile => {
            let f = crate::core::paths::config_file();
            if !f.exists() {
                crate::core::config::reload(); // 确保文件存在且字段齐全
            }
            open_path(&f);
        }
        MenuAction::OpenDataDir => {
            let d = crate::core::paths::data_dir();
            let _ = std::fs::create_dir_all(&d);
            open_path(&d);
        }
        MenuAction::OpenSongDir => {
            let d = crate::core::paths::songs_dir();
            let _ = std::fs::create_dir_all(&d);
            open_path(&d);
        }
        MenuAction::ReloadSettings => {
            let cfg = crate::core::config::get();
            let dpi = f32::from_bits(DPI_BITS.load(Ordering::Relaxed));
            with(|s| {
                s.reminders.reschedule(&cfg.reminders, now);
                s.set_watch(cfg.moyu.enabled, now);
                s.set_scale(dpi * cfg.scale);
            });
        }
        MenuAction::Moyu(on) => {
            with(|s| s.set_watch(on, now));
        }
        MenuAction::Quit => do_quit(),
        MenuAction::Memo(crate::types::MemoAct::OpenFile) => open_path(&crate::core::memo::ensure_file()),
        MenuAction::Memo(crate::types::MemoAct::OpenWindow) => settings::open(3),
        MenuAction::Report => settings::open(4),
        MenuAction::ExportChat => {
            let r = report::export_chat();
            match &r {
                Ok(p) => report::reveal(p),
                Err(e) => crate::core::log::log(&format!("chat export: {e}")),
            }
            post_event(AppEvent::ChatExported(r.is_ok()));
        }
        MenuAction::Mind(crate::types::MindAct::OpenFile) => open_path(&crate::core::memory::ensure_file()),
        // 清空记忆：推迟到 pump() 外弹模态确认框，避免在 PUMPING=true 期间阻塞事件队列
        MenuAction::Mind(crate::types::MindAct::ClearAsk) => {
            if let Some(h) = main_hwnd() {
                unsafe { let _ = PostMessageW(Some(h), WM_APP_CLEAR_MIND, WPARAM(0), LPARAM(0)); }
            }
        }
        // 「🎭 角色」→「打开皮肤文件夹」（不存在先建）
        MenuAction::Costume(c) if c == crate::core::skin::MENU_OPEN => open_path(&crate::core::skin::ensure_dir()),
        _ => {}
    }
    menu::refresh();
}

fn do_quit() {
    if QUITTING.with(|q| q.replace(true)) {
        return;
    }
    with(|s| s.quitting = true);
    settings::close_now();
    hrtimer::close();
    with_handler(|h| h.quit());
    crate::core::session::end();
    menu::shutdown();
    let old_toy = with(|s| s.toy.take()).flatten();
    drop(old_toy);
    let hwnd = with(|s| {
        s.chat = None;
        s.hwnd
    });
    if let Some(h) = hwnd {
        unsafe {
            let _ = KillTimer(Some(h), TIMER_ID);
            let _ = DestroyWindow(h);
        }
    }
}

/// 聊天输入框里有没有打了字（行为层用：空输入框遇到别的动作就顺手关掉，有字就留着）
pub fn chat_has_text() -> bool { chat::has_text() }

/// 聊天框里按了 Esc / 点了「×」
pub(crate) fn chat_escape() {
    with(|s| {
        if let Some(c) = s.chat.as_mut() {
            c.close();
        }
    });
    post_event(AppEvent::ChatCancel);
}

// ---------------------------------------------------------------- 窗口

fn work_area_of(hwnd: HWND) -> RECT {
    unsafe {
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(mon, &mut mi).as_bool() {
            mi.rcWork
        } else {
            RECT { left: 0, top: 0, right: 1920, bottom: 1080 }
        }
    }
}

fn work_area_at(pt: POINT, flag: windows::Win32::Graphics::Gdi::MONITOR_FROM_FLAGS) -> RECT {
    unsafe {
        let mon = MonitorFromPoint(pt, flag);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(mon, &mut mi).as_bool() {
            mi.rcWork
        } else {
            RECT { left: 0, top: 0, right: 1920, bottom: 1080 }
        }
    }
}

impl Plat {
    fn work_area(&self) -> RECT {
        work_area_at(POINT { x: self.x + self.w / 2, y: self.y + self.h / 2 }, MONITOR_DEFAULTTONEAREST)
    }

    fn is_visible(&self) -> bool {
        unsafe { IsWindowVisible(self.hwnd).as_bool() }
    }

    fn set_visible(&mut self, on: bool) {
        unsafe {
            let _ = ShowWindow(self.hwnd, if on { SW_SHOWNOACTIVATE } else { SW_HIDE });
        }
        if on {
            self.shown_once = true;
            self.layout_chat();
        }
        if let Some(c) = &self.chat {
            c.show_with_pet(on);
        }
    }

    fn layout_chat(&self) {
        if let Some(c) = &self.chat
            && c.visible
        {
            c.layout(self.x, self.y, self.h);
        }
    }

    fn move_to(&mut self, x: i32, y: i32) {
        if (x, y) == (self.x, self.y) {
            return;
        }
        self.x = x;
        self.y = y;
        unsafe {
            let _ = SetWindowPos(self.hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
        self.layout_chat();
    }

    fn call_to_center(&mut self) {
        self.walk = None;
        self.fall = None;
        self.hop = None;
        self.perch = None;
        let wa = self.work_area();
        let x = (wa.left as f32 + (wa.right - wa.left) as f32 * 0.62 - self.w as f32 / 2.0).round() as i32;
        self.move_to(x, wa.bottom - self.h);
        self.set_visible(true);
    }

    fn start_walk(&mut self, dir: i32, distance: f32, speed: f32) {
        let wa = self.work_area();
        let m = 20.0 * self.scale;
        let min_x = wa.left as f32 - m;
        let max_x = ((wa.right - self.w) as f32 + m).max(min_x);
        self.walk = Some(Walk { chase: false, dir: dir.signum(), speed, distance, moved: 0.0, xf: self.x as f32, min_x, max_x });
        self.perch = None;
        self.hop = None;
        self.phys_last = now_ms();
        self.phys_acc = 0;
    }

    /// 松手：带重力掉到任务栏上方
    fn start_fall(&mut self) {
        let wa = self.work_area();
        let floor = (wa.bottom - self.h) as f32;
        let lo = wa.left as f32 - self.w as f32 / 3.0;
        let hi = (wa.right as f32 - self.w as f32 * 2.0 / 3.0).max(lo);
        let xf = (self.x as f32).clamp(lo, hi);
        self.fall = Some(Fall { vy: 0.0, xf, yf: self.y as f32, floor });
        self.phys_last = now_ms();
        self.phys_acc = 0;
    }

    fn phys_active(&self) -> bool {
        self.walk.is_some()
            || self.fall.is_some()
            || self.hop.is_some()
            || self.toy.as_ref().is_some_and(|t| t.falling || t.kind == ToyKind::Teaser)
    }

    fn physics(&mut self, now: u64, evs: &mut Vec<AppEvent>) {
        self.hop_step(now, evs);
        if !self.phys_active() {
            self.phys_last = now;
            self.phys_acc = 0;
            return;
        }
        self.phys_acc += now.saturating_sub(self.phys_last);
        self.phys_last = now;
        let steps = self.phys_acc / PHYS_STEP;
        self.phys_acc %= PHYS_STEP;
        // 卡太久（比如拖着菜单）不追帧，最多补 8 步
        for _ in 0..steps.min(8) {
            self.phys_step(evs);
        }
    }

    fn phys_step(&mut self, evs: &mut Vec<AppEvent>) {
        let s = self.scale;
        if let Some(f) = &mut self.fall {
            f.vy += 2.2 * s;
            f.yf += f.vy;
            let mut landed = None;
            if f.yf >= f.floor {
                f.yf = f.floor;
                landed = Some(f.vy > 25.0 * s);
            }
            let (x, y) = (f.xf.round() as i32, f.yf.round() as i32);
            if let Some(big) = landed {
                self.fall = None;
                evs.push(AppEvent::Landed { big });
            }
            self.move_to(x, y);
        }
        if let Some(w) = &mut self.walk {
            w.xf += w.dir as f32 * w.speed;
            w.moved += w.speed;
            let hit_edge = w.xf <= w.min_x || w.xf >= w.max_x;
            w.xf = w.xf.clamp(w.min_x, w.max_x);
            let x = w.xf.round() as i32;
            let done = hit_edge || w.moved >= w.distance;
            if done {
                let chase = w.chase;
                self.walk = None;
                if chase {
                    let ok = self.toy_dx().is_some_and(|dx| dx.abs() as f32 <= 60.0 * s);
                    evs.push(AppEvent::Toy(ToyEv::Reached { ok }));
                } else {
                    evs.push(AppEvent::WalkDone { hit_edge });
                }
            }
            let y = self.y;
            self.move_to(x, y);
        }
        self.toy_step(evs);
    }

    /// 提醒 / 番茄钟 / 摸鱼采样。返回番茄钟是否刚结束（要刷新菜单）
    fn timers(&mut self, now: u64, evs: &mut Vec<AppEvent>) -> bool {
        let mut pomo_done = false;
        if let Some(p) = self.pomodoro
            && pomo_now_ms() >= p.end
        {
            self.pomodoro = None;
            POMO_END.store(0, Ordering::Relaxed);
            evs.push(AppEvent::Pomodoro(PomodoroEv::Done { minutes: p.minutes }));
            pomo_done = true;
        }
        let rem_due = self.reminders.next_deadline().is_some_and(|t| now >= t);
        let watch_due = self.watch_next.is_some_and(|t| now >= t);
        if rem_due || watch_due {
            let lt = LocalTime::now();
            if rem_due {
                for k in self.reminders.poll(now, lt) {
                    evs.push(AppEvent::Remind(k));
                }
            }
            if watch_due {
                self.watch_next = Some(now + watch::STEP as u64 * 1000);
                let cfg = crate::core::config::get().moyu;
                if cfg.enabled {
                    let sample = watch::sample();
                    if let Some(ev) = self.sup.on_sample(&sample, &cfg, self.pomodoro.is_some(), lt.day_key()) {
                        let (kind, level, minutes) = match &ev {
                            crate::types::MoyuEv::Nag { level, minutes, .. } => ("nag", *level, *minutes),
                            crate::types::MoyuEv::Back { minutes } => ("back", 0, *minutes),
                        };
                        crate::core::log::log(&format!(
                            "moyu {{\"type\":\"{kind}\",\"level\":{level},\"minutes\":{minutes}}}"
                        ));
                        evs.push(AppEvent::Moyu(ev));
                    }
                    MOYU_TODAY_SEC.store(self.sup.today_sec(), Ordering::Relaxed);
                }
            }
        }
        pomo_done
    }

    fn set_watch(&mut self, on: bool, now: u64) {
        self.watch_next = on.then_some(self.watch_next.unwrap_or(now + watch::STEP as u64 * 1000));
    }

    fn set_scale(&mut self, scale: f32) {
        if (scale - self.scale).abs() < f32::EPSILON {
            return;
        }
        self.scale = scale;
        SCALE_BITS.store(scale.to_bits(), Ordering::Relaxed);
        if let Some(c) = self.chat.as_mut() {
            c.set_scale(scale);
        }
        self.next_frame = now_ms();
    }

    /// 把一帧（预乘 BGRA）贴到分层窗口
    fn blit(&mut self, f: &Frame) {
        let (fw, fh) = (f.w as i32, f.h as i32);
        let Some(need) = (fw > 0 && fh > 0)
            .then(|| (fw as usize).checked_mul(fh as usize).and_then(|n| n.checked_mul(4)))
            .flatten()
        else {
            return;
        };
        if f.bgra.len() < need {
            return;
        }
        if self.dib.as_ref().is_none_or(|d| d.w != fw || d.h != fh) {
            self.dib = None;
            self.dib = create_dib(fw, fh);
        }
        let Some(dib) = &self.dib else { return };
        unsafe {
            std::ptr::copy_nonoverlapping(f.bgra.as_ptr(), dib.bits, need);
        }
        // 尺寸变了（DPI 变化）：以脚底中点为锚；若正在拖拽则按比例缩放 drag_off 防止跳动
        if (fw, fh) != (self.w, self.h) {
            if LEFT_DOWN.with(|d| d.get()) && self.w > 0 && self.h > 0 {
                self.drag_off = (
                    (self.drag_off.0 as f32 * fw as f32 / self.w as f32).round() as i32,
                    (self.drag_off.1 as f32 * fh as f32 / self.h as f32).round() as i32,
                );
            }
            self.x += (self.w - fw) / 2;
            self.y += self.h - fh;
            self.w = fw;
            self.h = fh;
        }
        let dst = POINT { x: self.x, y: self.y };
        let size = SIZE { cx: fw, cy: fh };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let r = unsafe {
            UpdateLayeredWindow(
                self.hwnd,
                None,
                Some(&dst),
                Some(&size),
                Some(dib.dc),
                Some(&src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
        };
        // 只在出错状态变化时记一笔，免得每帧刷日志
        static ULW_FAILING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if ULW_FAILING.swap(r.is_err(), Ordering::Relaxed) != r.is_err() {
            match &r {
                Err(e) => crate::core::log::log(&format!(
                    "UpdateLayeredWindow failed: {e} (frame {fw}x{fh} at {},{})",
                    self.x, self.y
                )),
                Ok(()) => crate::core::log::log("UpdateLayeredWindow ok again"),
            }
        }
        self.layout_chat();
        if !self.shown_once {
            self.set_visible(true);
        }
    }

    fn mouse_pos(&self, lp: LPARAM) -> (f32, f32, i32, i32) {
        let cx = (lp.0 & 0xffff) as u16 as i16 as i32;
        let cy = ((lp.0 >> 16) & 0xffff) as u16 as i16 as i32;
        (cx as f32 / self.scale, cy as f32 / self.scale, self.x + cx, self.y + cy)
    }
}

// ---------------------------------------------------------------- 新互动：光标、爬窗口、小道具
impl Plat {
    /// 她嘴巴的屏幕位置（立绘 400×587 里约 (168, 235)，盒子 200×293 在窗口 (20, 67)）
    fn mouth(&self) -> (i32, i32) {
        (self.x + (104.0 * self.scale) as i32, self.y + (185.0 * self.scale) as i32)
    }

    fn poll_cursor(&mut self, now: u64, evs: &mut Vec<AppEvent>) {
        if self.cursor_poll == 0 || now < self.cursor_next {
            return;
        }
        self.cursor_next = now + self.cursor_poll as u64;
        if !self.is_visible() {
            return;
        }
        let mut p = POINT::default();
        if unsafe { GetCursorPos(&mut p) }.is_err() {
            return;
        }
        let rel = (p.x - self.x, p.y - self.y);
        if rel == self.last_cursor {
            return;
        }
        self.last_cursor = rel;
        evs.push(AppEvent::Cursor { x: rel.0 as f32 / self.scale, y: rel.1 as f32 / self.scale });
    }

    /// 开始跳上窗口/屏幕边；不行返回 ClimbFailed
    fn climb(&mut self, kind: PerchKind) -> Option<AppEvent> {
        if self.fall.is_some() || self.hop.is_some() || !self.is_visible() {
            return Some(AppEvent::ClimbFailed);
        }
        let cx = self.x + self.w / 2;
        let (target, to) = match kind {
            PerchKind::WindowTop => {
                let own: Vec<HWND> = std::iter::once(self.hwnd).chain(self.toy.as_ref().map(|t| t.hwnd)).collect();
                let Some((h, r)) = perch::foreground(&own) else { return Some(AppEvent::ClimbFailed) };
                // 直接按目标窗口所在显示器取工作区，兼容副屏在主屏上方（负 Y 坐标）的情况
                let work: perch::R = work_area_of(h).into();
                let Some(to) = perch::window_perch(r, work, cx, self.w, self.h, self.scale) else {
                    return Some(AppEvent::ClimbFailed);
                };
                (Some((h, r)), to)
            }
            PerchKind::ScreenSide => (None, perch::side_perch(self.work_area().into(), cx, self.w, self.h)),
        };
        self.walk = None;
        self.perch = None;
        let (x0, y0) = (self.x as f32, self.y as f32);
        let (x1, y1) = (to.0 as f32, to.1 as f32);
        let dist = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt() / self.scale;
        let dur = (450.0 + dist * 0.6).clamp(450.0, 1300.0) as u64;
        let arc = 50.0 * self.scale + (y0 - y1).max(0.0) * 0.15;
        self.hop = Some(Hop { x0, y0, x1, y1, t0: now_ms(), dur, arc, kind, target });
        self.phys_last = now_ms();
        self.phys_acc = 0;
        None
    }

    fn hop_step(&mut self, now: u64, evs: &mut Vec<AppEvent>) {
        let Some(h) = &self.hop else { return };
        let p = (now.saturating_sub(h.t0) as f32 / h.dur as f32).min(1.0);
        let x = h.x0 + (h.x1 - h.x0) * p;
        let y = h.y0 + (h.y1 - h.y0) * p - h.arc * 4.0 * p * (1.0 - p);
        let (kind, target) = (h.kind, h.target);
        self.move_to(x.round() as i32, y.round() as i32);
        if p >= 1.0 {
            self.hop = None;
            if let Some((hwnd, rect)) = target {
                // 跳的途中窗口没了/挪了：直接掉下去
                if !perch::usable(hwnd) || perch::frame_rect(hwnd).is_none_or(|r| perch::moved(r, rect)) {
                    self.start_fall();
                    evs.push(AppEvent::PerchLost);
                    return;
                }
                self.perch = Some(Perch { hwnd, rect, next: now + PERCH_CHECK });
            }
            evs.push(AppEvent::Perched(kind));
        }
    }

    /// 坐着的窗口还在不在原地；不在了就掉下去
    fn check_perch(&mut self, now: u64, evs: &mut Vec<AppEvent>) {
        let Some(p) = &mut self.perch else { return };
        if now < p.next {
            return;
        }
        p.next = now + PERCH_CHECK;
        let ok = perch::usable(p.hwnd) && perch::frame_rect(p.hwnd).is_some_and(|r| !perch::moved(r, p.rect));
        if !ok {
            self.perch = None;
            self.start_fall();
            evs.push(AppEvent::PerchLost);
        }
    }

    /// 道具中心相对她身体中线（物理像素）
    fn toy_dx(&self) -> Option<i32> {
        let t = self.toy.as_ref()?;
        Some(t.center().0 - (self.x + self.w / 2))
    }

    fn toy_floor(&self, t: &toy::Toy) -> i32 {
        let wa = work_area_at(POINT { x: t.x + t.size / 2, y: t.y + t.size / 2 }, MONITOR_DEFAULTTONEAREST);
        // 小鱼干画在 64 格子中间偏下，底下留了一截透明
        wa.bottom - t.size + (16.0 * self.scale) as i32
    }

    fn toy_cmd(&mut self, cmd: ToyCmd) -> Option<AppEvent> {
        match cmd {
            ToyCmd::Spawn { kind, side, offset } => {
                self.toy = None;
                let size = (crate::render::toy::TOY * self.scale).round() as i32;
                let (x, y) = if kind == ToyKind::Teaser {
                    let mut p = POINT::default();
                    unsafe {
                        let _ = GetCursorPos(&mut p);
                    }
                    self.teaser_pos(p.x, p.y)
                } else {
                    let wa = self.work_area();
                    let cx = self.x + self.w / 2;
                    let off = (offset * self.scale) as i32;
                    let mut x = cx + side.signum() * off - size / 2;
                    if x < wa.left || x + size > wa.right {
                        x = cx - side.signum() * off - size / 2;
                    }
                    (x.clamp(wa.left, (wa.right - size).max(wa.left)), (self.y + (67.0 * self.scale) as i32).max(wa.top))
                };
                let mut t = toy::Toy::create(kind, x, y, self.scale)?;
                t.falling = kind != ToyKind::Teaser;
                self.toy = Some(t);
                self.phys_last = now_ms();
                self.phys_acc = 0;
                None
            }
            ToyCmd::Fall => {
                if let Some(t) = &mut self.toy {
                    t.falling = true;
                    t.vy = 0.0;
                    self.phys_last = now_ms();
                }
                None
            }
            ToyCmd::Chase { speed } => {
                let dx = self.toy_dx()?;
                let reach = (40.0 * self.scale) as i32;
                if dx.abs() <= reach {
                    return Some(AppEvent::Toy(ToyEv::Reached { ok: true }));
                }
                self.start_walk(dx.signum(), (dx.abs() - reach / 2) as f32, speed);
                if let Some(w) = &mut self.walk {
                    w.chase = true;
                }
                None
            }
            ToyCmd::Remove => {
                self.toy = None;
                None
            }
        }
    }

    /// 逗猫棒窗口位置：毛球对准光标
    fn teaser_pos(&self, cx: i32, cy: i32) -> (i32, i32) {
        let (tx, ty) = crate::render::toy::TEASER_TIP;
        (cx - (tx * self.scale) as i32, cy - (ty * self.scale) as i32)
    }

    fn toy_step(&mut self, evs: &mut Vec<AppEvent>) {
        let Some(kind) = self.toy.as_ref().map(|t| t.kind) else { return };
        if kind == ToyKind::Teaser {
            let mut p = POINT::default();
            if unsafe { GetCursorPos(&mut p) }.is_ok() {
                let (x, y) = self.teaser_pos(p.x, p.y);
                if let Some(t) = &mut self.toy {
                    t.move_to(x, y);
                }
            }
            return;
        }
        let floor = match &self.toy {
            Some(t) if t.falling && !t.dragging => self.toy_floor(t),
            _ => return,
        };
        let s = self.scale;
        let pet_cx = self.x + self.w / 2;
        let Some(t) = &mut self.toy else { return };
        t.vy += 1.6 * s;
        let mut y = t.y as f32 + t.vy;
        if y >= floor as f32 {
            y = floor as f32;
            t.falling = false;
            t.vy = 0.0;
            evs.push(AppEvent::Toy(ToyEv::Landed { dx: (t.center().0 - pet_cx) as f32 / s }));
        }
        let x = t.x;
        t.move_to(x, y.round() as i32);
    }
}

fn create_dib(w: i32, h: i32) -> Option<Dib> {
    unsafe {
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            return None;
        }
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // 自上而下
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let Ok(bmp) = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(dc);
            return None;
        };
        if bits.is_null() {
            let _ = DeleteObject(HGDIOBJ(bmp.0));
            let _ = DeleteDC(dc);
            return None;
        }
        let old = SelectObject(dc, HGDIOBJ(bmp.0));
        Some(Dib { dc, bmp, old, bits: bits as *mut u8, w, h })
    }
}

fn create_main_window() -> Option<HWND> {
    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None).ok()?.into();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: sys::WINDOW_CLASS,
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            return None;
        }
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            sys::WINDOW_CLASS,
            w!("哈基米"),
            WS_POPUP,
            0,
            0,
            WIN_W as i32,
            WIN_H as i32,
            None,
            None,
            Some(hinst),
            None,
        )
        .ok()
    }
}

fn mouse(lp: LPARAM, f: impl FnOnce(&mut Plat, (f32, f32, i32, i32)) -> MouseEv) {
    if let Some(ev) = with(|s| {
        let p = s.mouse_pos(lp);
        f(s, p)
    }) {
        emit(AppEvent::Mouse(ev));
        pump();
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_APP_EVENT => {
                let evs: Vec<AppEvent> = EVENT_QUEUE.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect();
                for e in evs {
                    emit(e);
                }
                pump();
                LRESULT(0)
            }
            WM_APP_MENU => {
                let ids: Vec<String> = MENU_QUEUE.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect();
                for id in ids {
                    if let Some(a) = menu::action_for(&id) {
                        on_menu(a);
                    }
                }
                pump();
                LRESULT(0)
            }
            WM_APP_CLEAR_MIND => {
                if mind_menu::confirm_clear(main_hwnd()) {
                    on_menu(MenuAction::Mind(crate::types::MindAct::Clear));
                    pump();
                }
                LRESULT(0)
            }
            WM_APP_DEBUG_MENU => {
                if let Some(a) = DEBUG_MENU.get(wp.0).and_then(|id| menu::parse_id(id, |_| false)) {
                    on_menu(a);
                    pump();
                }
                LRESULT(0)
            }
            WM_COPYDATA => {
                // 外部事件（platform/ipc.rs）。处理了返回 1，发送方据此判断送到没有
                let cds = lp.0 as *const windows::Win32::System::DataExchange::COPYDATASTRUCT;
                if cds.is_null() {
                    return LRESULT(0);
                }
                let c = &*cds;
                let n = c.cbData as usize;
                if c.lpData.is_null() || n == 0 || n > ipc::MAX_PAYLOAD {
                    return LRESULT(0);
                }
                let data = std::slice::from_raw_parts(c.lpData as *const u8, n);
                let Some(ev) = ipc::decode(c.dwData, data) else { return LRESULT(0) };
                if EXT_THROTTLE.with(|t| t.borrow_mut().allow(&ev, now_ms())) {
                    crate::core::log::log(&format!("external {:?} from {:?}", ev.kind, ev.source));
                    emit(AppEvent::External(ev));
                    pump();
                }
                LRESULT(1)
            }
            WM_APP_SECOND => {
                // 再双击一次程序 = 把她叫到屏幕中间
                with(|s| s.call_to_center());
                emit(AppEvent::SecondInstance);
                pump();
                menu::refresh();
                LRESULT(0)
            }
            WM_TIMER if wp.0 == menu::TRAY_CLICK_TIMER => {
                menu::tray_click_timer();
                LRESULT(0)
            }
            WM_APP_DEBUG_POPUP => {
                menu::popup();
                LRESULT(0)
            }
            WM_TIMER if wp.0 == TIMER_ID => {
                tick();
                LRESULT(0)
            }
            WM_DPICHANGED => {
                // 可能是在借着状态时（SetWindowPos 里）同步进来的，转一道 Post 再处理
                let _ = PostMessageW(Some(hwnd), WM_APP_DPI, WPARAM(wp.0 & 0xffff), LPARAM(0));
                LRESULT(0)
            }
            WM_APP_DPI => {
                let dpi = wp.0 as f32;
                if dpi > 0.0 {
                    let dpi = dpi / 96.0;
                    DPI_BITS.store(dpi.to_bits(), Ordering::Relaxed);
                    let user = crate::core::config::get().scale;
                    with(|s| s.set_scale(dpi * user));
                    rearm();
                }
                LRESULT(0)
            }
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                let dbl = msg == WM_LBUTTONDBLCLK;
                LEFT_DOWN.with(|d| d.set(true));
                let _ = SetCapture(hwnd);
                mouse(lp, |s, (x, y, sx, sy)| {
                    s.drag_off = (sx - s.x, sy - s.y);
                    s.dbl_pending = dbl;
                    MouseEv::Down { button: Button::Left, x, y, sx, sy }
                });
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                LEFT_DOWN.with(|d| d.set(false));
                let _ = ReleaseCapture();
                let mut dbl = None;
                mouse(lp, |s, (x, y, sx, sy)| {
                    if std::mem::take(&mut s.dbl_pending) {
                        dbl = Some((x, y));
                    }
                    MouseEv::Up { button: Button::Left, x, y, sx, sy }
                });
                if let Some((x, y)) = dbl {
                    emit(AppEvent::Mouse(MouseEv::DoubleClick { x, y }));
                    pump();
                }
                LRESULT(0)
            }
            WM_CAPTURECHANGED => {
                // 拖动中被 Alt+Tab / UAC / 弹窗抢走鼠标捕获：主动结束拖拽防止卡死
                if LEFT_DOWN.with(|d| d.replace(false)) {
                    let mut pt = POINT::default();
                    let _ = GetCursorPos(&mut pt);
                    if let Some(ev) = with(|s| {
                        s.dbl_pending = false;
                        MouseEv::Up {
                            button: Button::Left,
                            x: (pt.x - s.x) as f32 / s.scale,
                            y: (pt.y - s.y) as f32 / s.scale,
                            sx: pt.x,
                            sy: pt.y,
                        }
                    }) {
                        emit(AppEvent::Mouse(ev));
                        pump();
                    }
                }
                LRESULT(0)
            }
            WM_RBUTTONDOWN => {
                mouse(lp, |_, (x, y, sx, sy)| MouseEv::Down { button: Button::Right, x, y, sx, sy });
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                mouse(lp, |_, (x, y, sx, sy)| MouseEv::Up { button: Button::Right, x, y, sx, sy });
                menu::popup();
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                mouse(lp, |s, (x, y, sx, sy)| {
                    if !s.tracking {
                        s.tracking = true;
                        let mut t = TRACKMOUSEEVENT {
                            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                            dwFlags: TME_LEAVE,
                            hwndTrack: s.hwnd,
                            dwHoverTime: 0,
                        };
                        let _ = TrackMouseEvent(&mut t);
                    }
                    MouseEv::Move { x, y, sx, sy }
                });
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                let delta = ((wp.0 >> 16) & 0xffff) as u16 as i16 as i32;
                let sx = (lp.0 & 0xffff) as u16 as i16 as i32;
                let sy = ((lp.0 >> 16) & 0xffff) as u16 as i16 as i32;
                if let Some(ev) = with(|s| MouseEv::Wheel { delta, x: (sx - s.x) as f32 / s.scale, y: (sy - s.y) as f32 / s.scale }) {
                    emit(AppEvent::Mouse(ev));
                    pump();
                }
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                with(|s| s.tracking = false);
                emit(AppEvent::Mouse(MouseEv::Leave));
                pump();
                LRESULT(0)
            }
            WM_CLOSE => {
                do_quit();
                LRESULT(0)
            }
            // 睡眠醒来 / 改了系统时间：马上重排定时器，错过的备忘会在下一次 tick 合并补发
            WM_POWERBROADCAST | WM_TIMECHANGE => {
                MEMO_NEXT.with(|m| m.set(0));
                rearm();
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_ENDSESSION if wp.0 != 0 => {
                // 关机/注销：存档
                do_quit();
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

#[cfg(test)]
mod pomo_clock_tests {
    use super::*;

    #[test]
    fn pomodoro_left_uses_same_clock_as_start() {
        let prev = POMO_END.load(Ordering::Relaxed);
        let p = Pomodoro::start(25, pomo_now_ms());
        POMO_END.store(p.end, Ordering::Relaxed);
        let left = pomodoro_left();
        POMO_END.store(prev, Ordering::Relaxed);
        assert_eq!(left, Some(25));
    }
}

#[cfg(test)]
mod fs_autohide_tests {
    use super::fs_autohide;

    #[test]
    fn hides_in_fullscreen_and_comes_back() {
        // 进全屏：可见 → 藏起来并记下是我们藏的
        assert_eq!(fs_autohide(true, true, false), (Some(false), true));
        // 全屏期间保持
        assert_eq!(fs_autohide(true, false, true), (None, true));
        // 退出全屏 → 放出来
        assert_eq!(fs_autohide(false, false, true), (Some(true), false));
    }

    #[test]
    fn leaves_manual_hide_alone() {
        // 主人自己藏的：进全屏、出全屏都不动
        assert_eq!(fs_autohide(true, false, false), (None, false));
        assert_eq!(fs_autohide(false, false, false), (None, false));
    }
}
