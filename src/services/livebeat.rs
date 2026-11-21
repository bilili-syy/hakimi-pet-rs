//! 跟着「外面在放的歌」跳（菜单 🎧）：读系统默认输出设备的峰值电平表，几秒一报节拍。
//!
//! 不录音、不抓音频数据：只用 IAudioMeterInformation 的峰值（和音量合成器里那根绿条一样），
//! 大约每 15 ms 读一次，留最近 8 秒，每 1.5 秒用 behavior::beat::analyze 估一次节拍。
//! 行为层 stop() 之后线程自己退出。
use crate::behavior::beat::{self, LiveFeed, LiveReport};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// 保留多久的电平
const WINDOW: Duration = Duration::from_secs(8);
/// 至少攒够这么久才开始估节拍
const MIN_SPAN: Duration = Duration::from_secs(4);
/// 多久报一次
const REPORT_EVERY: Duration = Duration::from_millis(1500);
/// 峰值高于这个算「有声音」
pub const LOUD: f32 = 0.01;

pub fn start(feed: LiveFeed) {
    let _ = std::thread::Builder::new().name("live-beat".into()).spawn(move || run(feed));
}

fn run(feed: LiveFeed) {
    use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
    use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender};
    use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};

    struct ComGuard(bool);
    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }

    // 先声明 _com，保证局部变量析构时 meter 先释放、最后再 CoUninitialize
    let _com = ComGuard(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok());
    let open = || -> Option<IAudioMeterInformation> {
        unsafe {
            (|| -> windows::core::Result<IAudioMeterInformation> {
                let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
                let dev = en.GetDefaultAudioEndpoint(eRender, eConsole)?;
                dev.Activate(CLSCTX_ALL, None)
            })()
            .ok()
        }
    };
    let begin = Instant::now();
    let Some(mut meter) = open() else {
        feed.put(LiveReport { info: None, start: begin, quiet_ms: 0, heard: false, failed: true });
        return;
    };
    let mut samples: VecDeque<(Instant, f32)> = VecDeque::new();
    let mut last_loud = begin;
    let mut heard = false;
    let mut last_report = begin;
    let mut fails = 0u32;
    while !feed.stopped() {
        let peak = match unsafe { meter.GetPeakValue() } {
            Ok(v) => {
                fails = 0;
                v
            }
            Err(_) => {
                // 换了输出设备（插拔耳机）：重新打开
                fails += 1;
                if let Some(m) = open() { meter = m; }
                if fails > 200 {
                    feed.put(LiveReport { info: None, start: begin, quiet_ms: 0, heard, failed: true });
                    return;
                }
                0.0
            }
        };
        let now = Instant::now();
        samples.push_back((now, peak));
        while samples.front().is_some_and(|s| now.duration_since(s.0) > WINDOW) {
            samples.pop_front();
        }
        if peak > LOUD {
            last_loud = now;
            heard = true;
        }
        if now.duration_since(last_report) >= REPORT_EVERY {
            last_report = now;
            // 插拔耳机/切默认声卡时旧设备不会报错而是持续回 0.0：安静期间顺手重取默认输出端点
            if peak <= LOUD
                && let Some(m) = open()
            {
                meter = m;
            }
            let start = samples.front().map_or(now, |s| s.0);
            let info = (now.duration_since(start) >= MIN_SPAN).then(|| {
                let pts: Vec<(f64, f32)> = samples.iter().map(|(t, v)| (t.duration_since(start).as_secs_f64() * 1000.0, *v)).collect();
                beat::analyze(&beat::bin_peaks(&pts, beat::LIVE_HOP_MS), beat::LIVE_HOP_MS)
            });
            let quiet_ms = now.duration_since(last_loud).as_millis() as u64;
            feed.put(LiveReport { info, start, quiet_ms, heard, failed: false });
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
