//! 渲染性能统计：设了 HAKIMI_TRACE=1 时每 5 秒往 hakimi.log 写一行，平时什么都不做（不计时、不读 CPU）。
//! `trace perf 5.0s ticks 150 (30.0/s) drawn 148 (29.6/s) render avg 0.62ms p95 1.10ms max 1.9ms gap avg 33.4ms max 48ms cpu 1.8% core (0.2% total, 8 核) live 1 scale 1.25`
//! - ticks：平台层每次帧回调都会调 render，等于实际的帧调度次数；drawn：画面真变了、重新画了的帧
//! - render：Renderer::update 自己的耗时（画 + 转 BGRA），不含 UpdateLayeredWindow
//! - gap：相邻两次 render 调用的间隔 = 实际帧间隔（含 SetTimer 的 15.6ms 粒度）
//! - cpu：整个进程在这 5 秒里的 CPU 时间 / 墙钟（GetProcessTimes），包含平台层、音频线程等全部开销
use std::time::{Duration, Instant};

const WINDOW: Duration = Duration::from_secs(5);

pub struct Perf {
    on: bool,
    start: Option<Instant>,
    last_call: Option<Instant>,
    ticks: u32,
    drawn: u32,
    live: u32,
    renders: Vec<f32>,
    gap_sum: f32,
    gap_max: f32,
    cpu0: u64,
}

impl Perf {
    pub fn new() -> Self {
        let on = std::env::var_os("HAKIMI_TRACE").is_some_and(|v| !v.is_empty() && v != "0");
        Self { on, start: None, last_call: None, ticks: 0, drawn: 0, live: 0, renders: Vec::new(), gap_sum: 0.0, gap_max: 0.0, cpu0: 0 }
    }

    /// 关着时返回 None，调用方就不计时
    pub fn begin(&self) -> Option<Instant> { self.on.then(Instant::now) }

    /// 一次 update 结束：t0 = begin() 的返回，drawn = 这一帧真画了，live = 灵动扭曲开着
    pub fn end(&mut self, t0: Option<Instant>, drawn: bool, live: bool, scale: f32) {
        let Some(t0) = t0 else { return };
        let now = Instant::now();
        let start = *self.start.get_or_insert_with(|| {
            self.cpu0 = process_cpu_100ns();
            t0
        });
        if let Some(prev) = self.last_call {
            let g = (t0 - prev).as_secs_f32() * 1000.0;
            self.gap_sum += g;
            self.gap_max = self.gap_max.max(g);
        }
        self.last_call = Some(t0);
        self.ticks += 1;
        if drawn {
            self.drawn += 1;
            self.renders.push((now - t0).as_secs_f32() * 1000.0);
        }
        if live { self.live += 1; }
        let el = now - start;
        if el >= WINDOW {
            let cpu1 = process_cpu_100ns();
            crate::core::log::trace(&self.report(el, cpu1.saturating_sub(self.cpu0), scale));
            *self = Self { on: true, start: Some(now), last_call: self.last_call, cpu0: cpu1, ..Self::new_off() };
        }
    }

    fn new_off() -> Self { Self { on: false, start: None, last_call: None, ticks: 0, drawn: 0, live: 0, renders: Vec::new(), gap_sum: 0.0, gap_max: 0.0, cpu0: 0 } }

    /// 一行统计（纯函数部分，方便测试）
    fn report(&self, el: Duration, cpu_100ns: u64, scale: f32) -> String {
        let s = el.as_secs_f32().max(1e-3);
        let mut r = self.renders.clone();
        r.sort_by(|a, b| a.total_cmp(b));
        let avg = if r.is_empty() { 0.0 } else { r.iter().sum::<f32>() / r.len() as f32 };
        let p95 = r.get(((r.len() as f32 * 0.95) as usize).min(r.len().saturating_sub(1))).copied().unwrap_or(0.0);
        let max = r.last().copied().unwrap_or(0.0);
        let gaps = self.ticks.saturating_sub(1).max(1) as f32;
        let core = cpu_100ns as f32 / 1e4 / (s * 1000.0) * 100.0;
        let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        format!(
            "perf {s:.1}s ticks {} ({:.1}/s) drawn {} ({:.1}/s) render avg {avg:.2}ms p95 {p95:.2}ms max {max:.1}ms gap avg {:.1}ms max {:.0}ms cpu {core:.1}% core ({:.2}% total, {n} 核) live {} scale {scale}",
            self.ticks,
            self.ticks as f32 / s,
            self.drawn,
            self.drawn as f32 / s,
            self.gap_sum / gaps,
            self.gap_max,
            core / n as f32,
            self.live,
        )
    }
}

impl Default for Perf {
    fn default() -> Self { Self::new() }
}

/// 进程累计 CPU 时间（内核 + 用户），单位 100ns
#[cfg(windows)]
fn process_cpu_100ns() -> u64 {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    let ft = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    match unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) } {
        Ok(()) => ft(k) + ft(u),
        Err(_) => 0,
    }
}

#[cfg(not(windows))]
fn process_cpu_100ns() -> u64 { 0 }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_by_default_does_nothing() {
        let mut p = Perf::new_off();
        assert!(p.begin().is_none());
        p.end(None, true, true, 1.0);
        assert_eq!(p.ticks, 0);
    }

    #[test]
    fn report_numbers() {
        let mut p = Perf::new_off();
        p.ticks = 151;
        p.drawn = 150;
        p.renders = (0..150).map(|i| if i < 140 { 0.5 } else { 2.0 }).collect();
        p.gap_sum = 150.0 * 33.0;
        p.gap_max = 47.0;
        p.live = 150;
        // 5 秒里用了 0.1 秒 CPU = 一个核的 2%
        let r = p.report(Duration::from_secs(5), 1_000_000, 1.25);
        assert!(r.contains("ticks 151 (30.2/s)"), "{r}");
        assert!(r.contains("drawn 150 (30.0/s)"), "{r}");
        assert!(r.contains("p95 2.00ms"), "{r}");
        assert!(r.contains("gap avg 33.0ms max 47ms"), "{r}");
        assert!(r.contains("cpu 2.0% core"), "{r}");
    }
}
