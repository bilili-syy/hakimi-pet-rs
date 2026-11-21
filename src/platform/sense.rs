//! 场景感知（v1.1）：用户走开/回来、打字很快、鼠标乱晃、全屏程序、后台在放音乐。
//! UI 线程每 100ms 采一次（GetLastInputInfo + GetCursorPos，几次 user32 调用），只在状态变化时发 AppEvent::Sense。
//! 不装键盘钩子、不记按了什么键：「打字」= 有输入但光标没动的 100ms 格子占比。
//! 音乐：后台线程每 2 秒读一次默认输出设备的峰值电平（WASAPI 电平表）+ 有没有常见音乐播放器在跑。
use crate::types::{AppEvent, SenseEv};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use windows::Win32::Foundation::{HWND, POINT};

const STEP_MS: u64 = 100;
/// 打字：最近 20 秒（200 格）里至少 35% 的格子在敲键盘
const TYPE_WINDOW: usize = 200;
const TYPE_RATIO: f32 = 0.35;
const TYPE_COOL_MS: u64 = 15 * 60_000;
/// 晃鼠标：1.5 秒内来回 ≥4 次、总路程 ≥ 900 物理像素
const SHAKE_WIN_MS: u64 = 1500;
const SHAKE_FLIPS: usize = 4;
const SHAKE_PATH: i32 = 900;
const SHAKE_COOL_MS: u64 = 45_000;
/// 走开：3 分钟 = 第一级（自己玩），10 分钟 = 第二级（睡），回来时离开 ≥ 5 分钟才算「回来了」
pub const IDLE_1: u32 = 180;
pub const IDLE_2: u32 = 600;
pub const BACK_MIN: u32 = 300;

static PEAK: AtomicU32 = AtomicU32::new(0);
static PLAYER: AtomicBool = AtomicBool::new(false);
static STARTED: AtomicBool = AtomicBool::new(false);

/// 纯逻辑部分（可单测）
#[derive(Default)]
pub struct Sense {
    next: u64,
    last_input: u32,
    cursor: Option<(i32, i32)>,
    kb: VecDeque<bool>,
    type_cool: u64,
    shake_dir: i32,
    flips: VecDeque<(u64, i32)>,
    shake_cool: u64,
    idle_stage: u8,
    max_idle: u32,
    fs: bool,
    fs_votes: u8,
    music: bool,
    music_votes: i8,
    next_slow: u64,
}

pub struct Probe {
    pub now: u64,
    /// GetLastInputInfo 的 dwTime（变了 = 有新输入）
    pub input_tick: u32,
    pub idle_sec: u32,
    pub cursor: (i32, i32),
}

impl Sense {
    /// 每 STEP_MS 一次
    pub fn step(&mut self, p: &Probe, out: &mut Vec<SenseEv>) {
        let moved = self.cursor.is_some_and(|c| c != p.cursor);
        let input = self.last_input != 0 && p.input_tick != self.last_input;
        // ---- 打字
        self.kb.push_back(input && !moved);
        while self.kb.len() > TYPE_WINDOW { self.kb.pop_front(); }
        if self.kb.len() == TYPE_WINDOW && p.now >= self.type_cool {
            let n = self.kb.iter().filter(|b| **b).count();
            if n as f32 / TYPE_WINDOW as f32 >= TYPE_RATIO {
                self.type_cool = p.now + TYPE_COOL_MS;
                out.push(SenseEv::Typing);
            }
        }
        // ---- 晃鼠标（只看横向来回）
        if let Some((cx, _)) = self.cursor {
            let dx = p.cursor.0 - cx;
            if dx.abs() >= 25 {
                let d = dx.signum();
                if d != self.shake_dir {
                    self.shake_dir = d;
                    self.flips.push_back((p.now, 0));
                }
                if let Some(last) = self.flips.back_mut() { last.1 += dx.abs(); }
            }
            while self.flips.front().is_some_and(|f| p.now.saturating_sub(f.0) > SHAKE_WIN_MS) { self.flips.pop_front(); }
            let path: i32 = self.flips.iter().map(|f| f.1).sum();
            if self.flips.len() > SHAKE_FLIPS && path >= SHAKE_PATH && p.now >= self.shake_cool {
                self.shake_cool = p.now + SHAKE_COOL_MS;
                self.flips.clear();
                out.push(SenseEv::CursorShake);
            }
        }
        // ---- 走开 / 回来
        if p.idle_sec >= IDLE_2 && self.idle_stage < 2 {
            self.idle_stage = 2;
            out.push(SenseEv::UserIdle(2));
        } else if p.idle_sec >= IDLE_1 && self.idle_stage < 1 {
            self.idle_stage = 1;
            out.push(SenseEv::UserIdle(1));
        }
        self.max_idle = self.max_idle.max(p.idle_sec);
        if p.idle_sec < 2 && self.idle_stage > 0 {
            if self.max_idle >= BACK_MIN { out.push(SenseEv::UserBack { secs: self.max_idle }); }
            self.idle_stage = 0;
        }
        if p.idle_sec < 2 { self.max_idle = 0; }
        self.last_input = p.input_tick;
        self.cursor = Some(p.cursor);
    }

    /// 每 2 秒一次：全屏 / 音乐（连着两次才算变，免得闪）
    pub fn slow(&mut self, fullscreen: bool, music_now: bool, out: &mut Vec<SenseEv>) {
        if fullscreen != self.fs {
            self.fs_votes += 1;
            if self.fs_votes >= 2 {
                self.fs = fullscreen;
                self.fs_votes = 0;
                out.push(SenseEv::Fullscreen(fullscreen));
            }
        } else {
            self.fs_votes = 0;
        }
        // 音乐：连续 4 次（8 秒）有声才算开始，连续 3 次没声才算停
        if music_now { self.music_votes = self.music_votes.max(0) + 1 } else { self.music_votes = self.music_votes.min(0) - 1 }
        if !self.music && self.music_votes >= 4 {
            self.music = true;
            out.push(SenseEv::Music(true));
        } else if self.music && self.music_votes <= -3 {
            self.music = false;
            out.push(SenseEv::Music(false));
        }
    }
}

thread_local! {
    static S: RefCell<Sense> = RefCell::new(Sense::default());
}

/// tick 里调用
/// 当前是否处于全屏状态（设置重读时同步全屏自动隐藏用）
pub fn is_fullscreen() -> bool {
    S.with(|s| s.try_borrow().is_ok_and(|g| g.fs))
}

pub fn maybe_poll(now: u64, pet: HWND, evs: &mut Vec<AppEvent>) {
    let due = S.with(|s| {
        let mut s = s.borrow_mut();
        if now < s.next { return false; }
        s.next = now + STEP_MS;
        true
    });
    if !due { return; }
    if !STARTED.swap(true, Ordering::Relaxed) { start_meter(); }
    let mut out = Vec::new();
    let p = probe(now);
    S.with(|s| {
        let mut s = s.borrow_mut();
        s.step(&p, &mut out);
        if now >= s.next_slow {
            s.next_slow = now + 2000;
            let fs = unsafe { super::power::fullscreen_now(now, pet) };
            let peak = f32::from_bits(PEAK.load(Ordering::Relaxed));
            let music = PLAYER.load(Ordering::Relaxed) && peak > 0.02 && !crate::services::audio::is_playing();
            s.slow(fs, music, &mut out);
        }
    });
    for e in out {
        crate::core::log::trace(&format!("sense {e:?}"));
        evs.push(AppEvent::Sense(e));
    }
}

fn probe(now: u64) -> Probe {
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = Probe { now, input_tick: 0, idle_sec: 0, cursor: (0, 0) };
    unsafe {
        let mut lii = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        if GetLastInputInfo(&mut lii).as_bool() {
            p.input_tick = lii.dwTime;
            p.idle_sec = GetTickCount().wrapping_sub(lii.dwTime) / 1000;
        }
        let mut pt = POINT::default();
        if GetCursorPos(&mut pt).is_ok() { p.cursor = (pt.x, pt.y); }
    }
    p
}

const PLAYERS: [&str; 12] = [
    "cloudmusic", "qqmusic", "spotify", "kugou", "kwmusic", "foobar2000", "musicbee", "aimp",
    "applemusic", "music.ui", "lx-music-desktop", "yesplaymusic",
];

/// 后台线程：峰值电平 + 播放器在不在（只在检测到播放器时才激活 COM 电平表，平时不打扰 audiosrv）
fn start_meter() {
    let _ = std::thread::Builder::new().name("sense-meter".into()).spawn(|| unsafe {
        use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
        use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender};
        use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let open_meter = || -> Option<IAudioMeterInformation> {
            (|| -> windows::core::Result<IAudioMeterInformation> {
                let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
                let dev = en.GetDefaultAudioEndpoint(eRender, eConsole)?;
                dev.Activate(CLSCTX_ALL, None)
            })()
            .ok()
        };
        let mut meter: Option<IAudioMeterInformation> = None;
        let mut tick = 0u32;
        loop {
            if tick.is_multiple_of(5) {
                let running = player_running();
                PLAYER.store(running, Ordering::Relaxed);
                if running {
                    // 换了输出设备（插耳机）或刚检测到播放器时刷新电平表
                    meter = open_meter();
                } else {
                    meter = None;
                }
            }
            tick = tick.wrapping_add(1);
            let peak = if PLAYER.load(Ordering::Relaxed) {
                match meter.as_ref().and_then(|m| m.GetPeakValue().ok()) {
                    Some(v) => v,
                    None => {
                        meter = open_meter();
                        meter.as_ref().and_then(|m| m.GetPeakValue().ok()).unwrap_or(0.0)
                    }
                }
            } else {
                0.0
            };
            PEAK.store(peak.to_bits(), Ordering::Relaxed);
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    });
}

fn exe_matches_player(utf16: &[u16]) -> bool {
    let mut buf = [0u8; 64];
    let mut n = 0usize;
    for &u in utf16 {
        if u == 0 { break; }
        if u > 0x7f || n >= buf.len() { return false; }
        buf[n] = (u as u8).to_ascii_lowercase();
        n += 1;
    }
    let name = &buf[..n];
    let stem = name.strip_suffix(b".exe").unwrap_or(name);
    PLAYERS.iter().any(|p| p.as_bytes() == stem)
}

unsafe fn player_running() -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
    let Ok(snap) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else { return false };
    let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
    let mut found = false;
    if unsafe { Process32FirstW(snap, &mut e) }.is_ok() {
        loop {
            if exe_matches_player(&e.szExeFile) { found = true; break; }
            if unsafe { Process32NextW(snap, &mut e) }.is_err() { break; }
        }
    }
    let _ = unsafe { CloseHandle(snap) };
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(s: &mut Sense, now: u64, tick: u32, idle: u32, cur: (i32, i32)) -> Vec<SenseEv> {
        let mut o = Vec::new();
        s.step(&Probe { now, input_tick: tick, idle_sec: idle, cursor: cur }, &mut o);
        o
    }

    #[test]
    fn typing_detected_once() {
        let mut s = Sense::default();
        let mut got = 0;
        for i in 0..400u64 {
            // 每两格敲一次键，光标不动
            let tick = 1000 + (i / 2) as u32;
            got += run(&mut s, i * 100, tick, 0, (500, 500)).iter().filter(|e| **e == SenseEv::Typing).count();
        }
        assert_eq!(got, 1);
        // 只动鼠标不算
        let mut s = Sense::default();
        for i in 0..400u64 {
            let o = run(&mut s, i * 100, 1000 + i as u32, 0, (i as i32 % 2 * 40, 0));
            assert!(!o.contains(&SenseEv::Typing));
        }
    }

    #[test]
    fn shake_and_idle_back() {
        let mut s = Sense::default();
        let mut got = Vec::new();
        for i in 0..12u64 {
            let x = if i % 2 == 0 { 100 } else { 400 };
            got.extend(run(&mut s, 1000 + i * 100, 50 + i as u32, 0, (x, 300)));
        }
        assert!(got.contains(&SenseEv::CursorShake));
        // 小幅抖动不算
        let mut s = Sense::default();
        for i in 0..30u64 {
            let o = run(&mut s, 1000 + i * 100, 50, 0, (100 + (i as i32 % 2) * 20, 300));
            assert!(o.is_empty());
        }
        let mut s = Sense::default();
        assert!(run(&mut s, 0, 1, 0, (0, 0)).is_empty());
        assert_eq!(run(&mut s, 100, 1, 200, (0, 0)), vec![SenseEv::UserIdle(1)]);
        assert_eq!(run(&mut s, 200, 1, 700, (0, 0)), vec![SenseEv::UserIdle(2)]);
        assert!(run(&mut s, 300, 1, 900, (0, 0)).is_empty());
        assert_eq!(run(&mut s, 400, 2, 0, (5, 5)), vec![SenseEv::UserBack { secs: 900 }]);
        assert!(run(&mut s, 500, 3, 0, (5, 5)).is_empty());
    }

    #[test]
    fn slow_debounces() {
        let mut s = Sense::default();
        let mut o = Vec::new();
        s.slow(true, false, &mut o);
        assert!(o.is_empty());
        s.slow(true, false, &mut o);
        assert_eq!(o, vec![SenseEv::Fullscreen(true)]);
        o.clear();
        for _ in 0..3 { s.slow(true, true, &mut o); }
        assert!(o.is_empty());
        s.slow(true, true, &mut o);
        assert_eq!(o, vec![SenseEv::Music(true)]);
        o.clear();
        for _ in 0..3 { s.slow(true, false, &mut o); }
        assert_eq!(o, vec![SenseEv::Music(false)]);
    }
}
