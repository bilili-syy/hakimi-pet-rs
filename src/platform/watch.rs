//! 摸鱼监督：替代 Electron 版 watch.js 的常驻 PowerShell 子进程。
//! 直接调 user32 取「前台窗口进程名 + 标题 + 键鼠空闲秒数」，由 UI 线程定时器每 5 秒采样一次，
//! 不起线程、不起子进程。只在本机判断，不记录标题，只统计摸鱼秒数。
use crate::core::config::MoyuConfig;
use crate::types::MoyuEv;

/// 采样间隔（秒），和原版一致
pub const STEP: u32 = 5;
/// 键鼠空闲超过这么多秒就不算（人不在电脑前）
pub const IDLE_LIMIT: u32 = 120;

#[derive(Clone, Debug, Default)]
pub struct Sample {
    /// 进程名（不带 .exe），和 PowerShell Get-Process 的 ProcessName 一致
    pub proc_name: String,
    pub title: String,
    pub idle_sec: u32,
}

/// 判断一次采样是不是在摸鱼；返回命中的关键词/进程名（给台词用），否则 None
pub fn classify(s: &Sample, cfg: &MoyuConfig) -> Option<String> {
    let title = s.title.to_lowercase();
    let proc_ = s.proc_name.to_lowercase();
    if title.is_empty() && proc_.is_empty() {
        return None;
    }
    // 看她自己不算
    if proc_.contains("哈基米") || proc_ == "electron" || proc_ == "hakimi-pet" {
        return None;
    }
    if let Some(p) = cfg.processes.iter().find(|k| !k.is_empty() && proc_.contains(&k.to_lowercase())) {
        return Some(p.clone());
    }
    cfg.keywords
        .iter()
        .find(|w| !w.is_empty() && title.contains(&w.to_lowercase()))
        .map(|k| k.trim().to_string())
}

fn round_min(sec: u32) -> u32 {
    (sec as f64 / 60.0).round() as u32
}

/// 监督逻辑：连续摸鱼达到阈值就催，之后每 10 分钟再催一次（最多三档），回去干活 60 秒后夸一句
#[derive(Debug, Default)]
pub struct Supervisor {
    moyu_sec: u32,
    calm_sec: u32,
    nag_level: u8,
    next_nag_at: u32,
    today_key: (u16, u16, u16),
    today_sec: u32,
}

impl Supervisor {
    pub fn new(day: (u16, u16, u16)) -> Self {
        Self { today_key: day, ..Default::default() }
    }

    /// 处理一次采样。cfg.enabled 由调用方判断。day = 本地日期 (年, 月, 日)
    pub fn on_sample(&mut self, s: &Sample, cfg: &MoyuConfig, pomodoro: bool, day: (u16, u16, u16)) -> Option<MoyuEv> {
        if day != self.today_key {
            self.today_key = day;
            self.today_sec = 0;
        }
        if s.idle_sec > IDLE_LIMIT {
            return None;
        }
        if let Some(hit) = classify(s, cfg) {
            self.moyu_sec += STEP;
            self.today_sec += STEP;
            self.calm_sec = 0;
            let limit = if pomodoro { cfg.pomodoro_minutes } else { cfg.minutes } * 60;
            if self.nag_level == 0 && self.moyu_sec >= limit {
                self.nag_level = 1;
                self.next_nag_at = self.moyu_sec + 600;
                return Some(MoyuEv::Nag { level: 1, minutes: round_min(self.moyu_sec), what: hit, pomodoro });
            } else if self.nag_level > 0 && self.moyu_sec >= self.next_nag_at {
                self.nag_level = (self.nag_level + 1).min(3);
                self.next_nag_at = self.moyu_sec + 600;
                return Some(MoyuEv::Nag { level: self.nag_level, minutes: round_min(self.moyu_sec), what: hit, pomodoro });
            }
            None
        } else {
            self.calm_sec += STEP;
            let mut ev = None;
            if self.calm_sec >= 60 {
                if self.nag_level > 0 {
                    ev = Some(MoyuEv::Back { minutes: round_min(self.moyu_sec) });
                }
                self.moyu_sec = 0;
                self.nag_level = 0;
            }
            ev
        }
    }

    pub fn today_sec(&self) -> u32 {
        self.today_sec
    }
}

/// 采一次样（Win32）
#[cfg(windows)]
pub fn sample() -> Sample {
    use windows::Win32::Foundation::{CloseHandle, HWND};
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId};
    use windows::core::PWSTR;

    let mut out = Sample::default();
    unsafe {
        let mut lii = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        if GetLastInputInfo(&mut lii).as_bool() {
            out.idle_sec = GetTickCount().wrapping_sub(lii.dwTime) / 1000;
        }
        let h: HWND = GetForegroundWindow();
        if h.0.is_null() {
            return out;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        // 看她自己（即使 exe 改过名）都不算摸鱼
        if pid == std::process::id() {
            return out;
        }
        let mut buf = [0u16; 512];
        let n = GetWindowTextW(h, &mut buf);
        if n > 0 {
            out.title = String::from_utf16_lossy(&buf[..n as usize]);
        }
        if pid != 0
            && let Ok(ph) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
        {
            let mut path = [0u16; 1024];
            let mut len = path.len() as u32;
            if QueryFullProcessImageNameW(ph, PROCESS_NAME_WIN32, PWSTR(path.as_mut_ptr()), &mut len).is_ok() {
                let full = String::from_utf16_lossy(&path[..len as usize]);
                out.proc_name = proc_name_from_path(&full);
            }
            let _ = CloseHandle(ph);
        }
    }
    out
}

/// "C:\\x\\Steam.exe" -> "Steam"
pub fn proc_name_from_path(full: &str) -> String {
    let file = full.rsplit(['\\', '/']).next().unwrap_or(full);
    match file.rfind('.') {
        Some(i) if file[i..].eq_ignore_ascii_case(".exe") => file[..i].to_string(),
        _ => file.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> MoyuConfig {
        MoyuConfig::default()
    }
    fn s(p: &str, t: &str) -> Sample {
        Sample { proc_name: p.into(), title: t.into(), idle_sec: 0 }
    }
    const D: (u16, u16, u16) = (2026, 11, 20);

    #[test]
    fn classify_matches_process_then_keyword() {
        assert_eq!(classify(&s("Steam", "Library"), &cfg()).as_deref(), Some("steam"));
        assert_eq!(classify(&s("chrome", "【4K】xxx - 哔哩哔哩_bilibili"), &cfg()).as_deref(), Some("bilibili"));
        assert_eq!(classify(&s("chrome", "YouTube - Google Chrome"), &cfg()).as_deref(), Some("youtube"));
        assert_eq!(classify(&s("Code", "main.rs - VS Code"), &cfg()), None);
        assert_eq!(classify(&s("", ""), &cfg()), None);
        assert_eq!(classify(&s("hakimi-pet", "bilibili"), &cfg()), None);
        assert_eq!(classify(&s("electron", "bilibili"), &cfg()), None);
    }

    #[test]
    fn proc_name_strip() {
        assert_eq!(proc_name_from_path(r"C:\Program Files\Steam\steam.EXE"), "steam");
        assert_eq!(proc_name_from_path("foo"), "foo");
    }

    #[test]
    fn nag_levels_and_back() {
        let c = cfg(); // minutes = 20
        let mut sup = Supervisor::new(D);
        let mut evs = vec![];
        // 摸鱼 45 分钟
        for _ in 0..(45 * 60 / STEP) {
            if let Some(e) = sup.on_sample(&s("chrome", "bilibili"), &c, false, D) {
                evs.push(e);
            }
        }
        assert_eq!(evs.len(), 3);
        assert!(matches!(&evs[0], MoyuEv::Nag { level: 1, minutes: 20, .. }));
        assert!(matches!(&evs[1], MoyuEv::Nag { level: 2, minutes: 30, .. }));
        assert!(matches!(&evs[2], MoyuEv::Nag { level: 3, minutes: 40, .. }));
        assert_eq!(sup.today_sec(), 45 * 60);
        // 回去干活 55 秒还不算，60 秒才夸
        for _ in 0..11 {
            assert_eq!(sup.on_sample(&s("Code", "x"), &c, false, D), None);
        }
        assert_eq!(sup.on_sample(&s("Code", "x"), &c, false, D), Some(MoyuEv::Back { minutes: 45 }));
    }

    #[test]
    fn pomodoro_threshold_and_idle_and_day_reset() {
        let c = cfg(); // pomodoroMinutes = 2
        let mut sup = Supervisor::new(D);
        let mut hit = None;
        for _ in 0..(2 * 60 / STEP) {
            hit = sup.on_sample(&s("steam", ""), &c, true, D).or(hit);
        }
        assert!(matches!(hit, Some(MoyuEv::Nag { level: 1, pomodoro: true, .. })));
        // 空闲不算
        let idle = Sample { idle_sec: 121, ..s("steam", "") };
        let before = sup.today_sec();
        assert_eq!(sup.on_sample(&idle, &c, true, D), None);
        assert_eq!(sup.today_sec(), before);
        // 跨天清零
        sup.on_sample(&s("Code", ""), &c, false, (2026, 11, 21));
        assert_eq!(sup.today_sec(), 0);
    }
}
