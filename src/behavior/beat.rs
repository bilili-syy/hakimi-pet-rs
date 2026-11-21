//! 节拍：内置鼓点合成 + 歌曲的简易节拍检测。都是纯函数，不加依赖。
//!
//! 检测流程（够跳舞用就行，不求专业）：
//! 1. 后台线程用 rodio 的解码器边读边解码（不把歌读进内存），混成单声道，每 ~10ms 算一次 RMS 能量；
//! 2. 能量取对数后做一阶差分、只留上升沿 → 起音强度（onset）；
//! 3. 起音强度自相关，在 70~180 BPM 的范围里找峰（轻微偏向 120 BPM，防止半速/倍速）；
//! 4. 在峰附近用「梳子」细调周期（0.05 跳步）并找相位：哪个起点上每拍的起音强度加起来最大；
//! 5. 顺带出一条 50ms 一格的音量包络（0~255），舞步力度跟着它变。
//!
//! 解码只看开头 max_secs 秒，内存只有能量数组（30 秒 ≈ 3000 个 f32）和包络（≈ 600 字节）。
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub const MIN_BPM: f64 = 70.0;
pub const MAX_BPM: f64 = 180.0;
/// 包络一格多少毫秒
pub const ENV_MS: u32 = 50;
/// 自相关峰值 / 平均值 低于这个就认为没有明显节拍
const MIN_CONFIDENCE: f32 = 1.25;

/// 检测结果
#[derive(Clone, Debug, PartialEq)]
pub struct BeatInfo {
    /// (每拍毫秒, 第一拍相对音频开头的毫秒)；节拍不明显时为 None
    pub tempo: Option<(f64, f64)>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub confidence: f32,
    /// 音量包络：ENV_MS 一格，0~255（按 95 分位归一化）
    pub env: Vec<u8>,
}

impl BeatInfo {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn bpm(&self) -> Option<f64> { self.tempo.map(|(p, _)| 60_000.0 / p) }
    /// t 毫秒处的音量 0..1（超出范围 None）
    pub fn level(&self, t_ms: f64) -> Option<f32> {
        if t_ms < 0.0 { return None; }
        let i = (t_ms / ENV_MS as f64) as usize;
        self.env.get(i).map(|v| *v as f32 / 255.0)
    }
}

/// 单声道采样 → 每跳一个 RMS 能量（跳长 = 采样率 / 100，约 10ms）
pub struct EnergyMeter {
    hop: usize,
    sr: u32,
    acc: f64,
    k: usize,
    pub energies: Vec<f32>,
}

impl EnergyMeter {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(100);
        Self { hop: (sr / 100) as usize, sr, acc: 0.0, k: 0, energies: Vec::new() }
    }
    pub fn hop_ms(&self) -> f64 { self.hop as f64 * 1000.0 / self.sr as f64 }
    pub fn push(&mut self, s: f32) {
        self.acc += (s as f64) * (s as f64);
        self.k += 1;
        if self.k == self.hop {
            self.energies.push((self.acc / self.hop as f64).sqrt() as f32);
            self.acc = 0.0;
            self.k = 0;
        }
    }
}

/// 起音强度：log 能量的上升沿
fn onsets(e: &[f32]) -> Vec<f32> {
    let l: Vec<f32> = e.iter().map(|x| (1.0 + 100.0 * x.max(0.0)).ln()).collect();
    let mut o = vec![0.0; l.len()];
    for i in 1..l.len() { o[i] = (l[i] - l[i - 1]).max(0.0); }
    o
}

/// 音量包络（ENV_MS 一格）
pub fn envelope(e: &[f32], hop_ms: f64) -> Vec<u8> {
    if !hop_ms.is_finite() || hop_ms <= 0.0 { return Vec::new(); }
    let per = ((ENV_MS as f64 / hop_ms).round() as usize).max(1);
    let cells: Vec<f32> = e.chunks(per).map(|c| c.iter().sum::<f32>() / c.len() as f32).collect();
    if cells.is_empty() { return Vec::new(); }
    let mut sorted = cells.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let p95 = sorted[((sorted.len() - 1) as f32 * 0.95) as usize].max(1e-6);
    cells.iter().map(|c| ((c / p95).min(1.0) * 255.0).round() as u8).collect()
}

/// 从能量序列估计节拍。hop_ms = 每个能量点的时长
pub fn analyze(e: &[f32], hop_ms: f64) -> BeatInfo {
    if !hop_ms.is_finite() || hop_ms <= 0.0 {
        return BeatInfo { tempo: None, confidence: 0.0, env: Vec::new() };
    }
    let env = envelope(e, hop_ms);
    let none = |confidence| BeatInfo { tempo: None, confidence, env: env.clone() };
    let o = onsets(e);
    let n = o.len();
    let lo = (60_000.0 / (MAX_BPM * hop_ms)).floor() as usize;
    let hi = (60_000.0 / (MIN_BPM * hop_ms)).ceil() as usize;
    if lo < 2 || n < hi.saturating_mul(4) { return none(0.0); }
    let acf = |lag: usize| -> f32 {
        let s: f32 = (0..n - lag).map(|i| o[i] * o[i + lag]).sum();
        s / (n - lag) as f32
    };
    let vals: Vec<(usize, f32)> = (lo..=hi).map(|l| (l, acf(l))).collect();
    let mean = vals.iter().map(|v| v.1).sum::<f32>() / vals.len() as f32;
    if mean <= 1e-9 { return none(0.0); }
    let weight = |lag: f64| {
        let bpm = 60_000.0 / (lag * hop_ms);
        let z = (bpm / 120.0).log2();
        (-0.5 * z * z).exp() as f32
    };
    let Some(&(best, peak)) = vals.iter().max_by(|a, b| (a.1 * weight(a.0 as f64)).total_cmp(&(b.1 * weight(b.0 as f64)))) else {
        return none(0.0);
    };
    let confidence = peak / mean;
    if confidence < MIN_CONFIDENCE { return none(confidence); }
    // 梳子细调：周期在 best±1 跳里 0.05 一档，相位 1 跳一档
    let mut top = (0.0f32, best as f64, 0usize);
    let mut p = best as f64 - 1.0;
    while p <= best as f64 + 1.0 + 1e-9 {
        if p >= 2.0 {
            for phi in 0..(p.ceil() as usize) {
                let (mut s, mut c, mut k) = (0.0f32, 0u32, 0.0f64);
                loop {
                    let i = (phi as f64 + k * p).round() as usize;
                    if i >= n { break; }
                    // 允许 ±1 跳的误差
                    let lo_i = i.saturating_sub(1);
                    let hi_i = (i + 1).min(n - 1);
                    s += o[lo_i..=hi_i].iter().copied().fold(0.0, f32::max);
                    c += 1;
                    k += 1.0;
                }
                if c > 0 && s / c as f32 > top.0 { top = (s / c as f32, p, phi); }
            }
        }
        p += 0.05;
    }
    let (_, period, phi) = top;
    BeatInfo { tempo: Some((period * hop_ms, phi as f64 * hop_ms)), confidence, env }
}

// ------------------------------------------------------------------ 后台检测
/// 检测中的结果槽：poll() 拿到 Some 之后就不会再有了
#[derive(Clone, Default)]
pub struct Pending(Arc<Mutex<Option<Option<BeatInfo>>>>);

impl Pending {
    /// Some(结果) = 检测完了（结果本身可能是 None = 解码失败）；None = 还在算
    pub fn poll(&self) -> Option<Option<BeatInfo>> { self.0.lock().ok()?.take() }
    pub fn put(&self, r: Option<BeatInfo>) {
        if let Ok(mut g) = self.0.lock() { *g = Some(r); }
    }
}

// ------------------------------------------------------------------ 跟着外面在放的歌跳
/// 实时节拍的格子长度（毫秒）。电平表采样受系统定时器影响大约 15 ms 一次，20 ms 一格够稳
pub const LIVE_HOP_MS: f64 = 20.0;

/// 后台线程（services/livebeat.rs）每隔一会儿报一次：最近几秒系统声音的节拍
#[derive(Clone, Debug)]
pub struct LiveReport {
    /// 节拍检测结果（相位相对 start）；数据还不够时为 None
    pub info: Option<BeatInfo>,
    /// 这段数据第一格的时刻
    pub start: Instant,
    /// 已经连续安静了多久（毫秒）
    pub quiet_ms: u64,
    /// 开始以来听到过声音没有
    pub heard: bool,
    /// 电平表打不开（没有输出设备等）
    pub failed: bool,
}

/// 行为层和后台线程之间的结果槽 + 停止开关
#[derive(Clone, Default)]
pub struct LiveFeed {
    slot: Arc<Mutex<Option<LiveReport>>>,
    stop: Arc<AtomicBool>,
}

impl LiveFeed {
    pub fn put(&self, r: LiveReport) {
        if let Ok(mut g) = self.slot.lock() { *g = Some(r); }
    }
    /// 取最新一次报告（取走就没了）
    pub fn poll(&self) -> Option<LiveReport> { self.slot.lock().ok()?.take() }
    pub fn stop(&self) { self.stop.store(true, Ordering::Relaxed); }
    pub fn stopped(&self) -> bool { self.stop.load(Ordering::Relaxed) }
}

/// 不规则采样 (毫秒, 峰值) → 等间隔格子：每格取最大值，空格沿用上一格
pub fn bin_peaks(samples: &[(f64, f32)], hop_ms: f64) -> Vec<f32> {
    let Some(&(t0, _)) = samples.first() else { return Vec::new() };
    if !hop_ms.is_finite() || hop_ms <= 0.0 { return Vec::new(); }
    let last = samples.last().map(|s| s.0).unwrap_or(t0);
    let n = ((last - t0) / hop_ms).floor() as usize + 1;
    let mut out: Vec<Option<f32>> = vec![None; n];
    for &(t, v) in samples {
        let i = (((t - t0) / hop_ms).floor() as usize).min(n - 1);
        out[i] = Some(out[i].map_or(v, |o| o.max(v)));
    }
    let mut prev = 0.0;
    out.into_iter().map(|c| { if let Some(v) = c { prev = v; } prev }).collect()
}

/// 后台线程解码歌曲开头 max_secs 秒做节拍检测
pub fn analyze_file(path: PathBuf, max_secs: u32) -> Pending {
    let slot = Pending::default();
    let out = slot.clone();
    let spawned = std::thread::Builder::new().name("hakimi-beat".into()).spawn(move || {
        let r = decode_energies(&path, max_secs).map(|(e, hop)| analyze(&e, hop));
        #[cfg(not(test))]
        if r.is_none() { crate::core::log::log(&format!("beat: decode failed {}", path.display())); }
        out.put(r);
    });
    if spawned.is_err() { slot.put(None); }
    slot
}

fn decode_energies(path: &std::path::Path, max_secs: u32) -> Option<(Vec<f32>, f64)> {
    use rodio::Source;
    let f = std::fs::File::open(path).ok()?;
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let mut b = rodio::Decoder::builder().with_data(std::io::BufReader::new(f)).with_byte_len(len).with_seekable(true);
    if !ext.is_empty() { b = b.with_hint(&ext); }
    let dec = b.build().ok()?;
    let ch = dec.channels().get() as usize;
    let sr = dec.sample_rate().get();
    let max = (sr as usize).saturating_mul(max_secs as usize).saturating_mul(ch);
    let mut m = EnergyMeter::new(sr);
    let (mut acc, mut k) = (0.0f32, 0usize);
    for s in dec.take(max) {
        acc += s;
        k += 1;
        if k == ch {
            m.push(acc / ch as f32);
            acc = 0.0;
            k = 0;
        }
    }
    let hop = m.hop_ms();
    (!m.energies.is_empty()).then_some((m.energies, hop))
}

// ------------------------------------------------------------------ 内置鼓点
pub const SYNTH_RATE: u32 = 16_000;

/// 合成一段「哈基米牌」鼓点：每拍底鼓、反拍踩镲、2/4 拍军鼓、一条四音贝斯。
/// 16kHz 单声道 16bit WAV（20 秒约 640KB，放完就释放）
pub fn synth_wav(bpm: f32, secs: f32) -> Vec<u8> {
    let sr = SYNTH_RATE as f32;
    let n = (secs.max(0.5) * sr) as usize;
    let period = 60.0 / bpm.clamp(40.0, 240.0);
    // A2 A2 F2 G2
    const BASS: [f32; 4] = [110.0, 110.0, 87.31, 98.0];
    let mut seed: u32 = 0x9e37_79b9;
    let mut noise = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed as f32 / u32::MAX as f32) * 2.0 - 1.0
    };
    let mut buf = vec![0.0f32; n];
    for (i, out) in buf.iter_mut().enumerate() {
        let t = i as f32 / sr;
        let beat = (t / period) as usize;
        let tb = t - beat as f32 * period;
        let mut v = 0.0;
        // 底鼓：50Hz 起扫的正弦
        if tb < 0.25 {
            let ph = std::f32::consts::TAU * (50.0 * tb + 90.0 / 30.0 * (1.0 - (-30.0 * tb).exp()));
            v += ph.sin() * (-tb * 18.0).exp() * 0.9;
        }
        let nz = noise();
        // 反拍踩镲
        let to = tb - period / 2.0;
        if (0.0..0.045).contains(&to) { v += nz * (-to * 110.0).exp() * 0.22; }
        // 2、4 拍军鼓
        if beat % 2 == 1 && tb < 0.18 {
            v += nz * (-tb * 22.0).exp() * 0.3 + (std::f32::consts::TAU * 190.0 * tb).sin() * (-tb * 30.0).exp() * 0.2;
        }
        // 贝斯
        let f = BASS[(beat / 2) % 4];
        let env = (tb / 0.01).min(1.0) * (-tb * 3.0).exp();
        v += (std::f32::consts::TAU * f * t).sin() * env * 0.16;
        *out = v;
    }
    let peak = buf.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
    let fade_in = (0.03 * sr) as usize;
    let fade_out = (0.6 * sr) as usize;
    let mut wav = Vec::with_capacity(44 + n * 2);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + n as u32 * 2).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&SYNTH_RATE.to_le_bytes());
    wav.extend_from_slice(&(SYNTH_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(n as u32 * 2).to_le_bytes());
    for (i, v) in buf.iter().enumerate() {
        let mut g = 0.7 / peak;
        if i < fade_in { g *= i as f32 / fade_in as f32; }
        if i + fade_out > n { g *= (n - i) as f32 / fade_out as f32; }
        let s = (v * g * 32767.0).clamp(-32768.0, 32767.0) as i16;
        wav.extend_from_slice(&s.to_le_bytes());
    }
    wav
}

#[cfg(test)]
mod tests {
    use super::*;

    fn energies_of(wav: &[u8]) -> (Vec<f32>, f64) {
        let mut m = EnergyMeter::new(SYNTH_RATE);
        for c in wav[44..].as_chunks::<2>().0 { m.push(i16::from_le_bytes(*c) as f32 / 32768.0); }
        let hop = m.hop_ms();
        (m.energies, hop)
    }

    #[test]
    fn synth_is_valid_wav() {
        let w = synth_wav(120.0, 2.0);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(w.len(), 44 + 2 * 2 * SYNTH_RATE as usize);
        assert!(crate::services::audio::can_decode(&w));
    }

    #[test]
    fn detects_tempo_and_phase_of_builtin_beat() {
        for bpm in [96.0f32, 110.0, 128.0] {
            let (e, hop) = energies_of(&synth_wav(bpm, 12.0));
            assert!((hop - 10.0).abs() < 1e-9);
            let info = analyze(&e, hop);
            let got = info.bpm().expect("tempo");
            assert!((got - bpm as f64).abs() / (bpm as f64) < 0.015, "bpm {bpm} → {got}");
            let (period, phase) = info.tempo.unwrap();
            // 第一拍在 0：相位应该贴着 0（或贴着一个周期）
            let d = phase.min(period - phase);
            assert!(d <= 20.0, "phase {phase} of {period}");
            assert!(info.confidence > 1.5);
            assert_eq!(info.env.len(), 12_000 / ENV_MS as usize);
        }
    }

    #[test]
    fn detects_offset_click_track() {
        // 100 BPM 咔哒声，从 230ms 开始
        let mut m = EnergyMeter::new(8000);
        let period = 0.6f32;
        for i in 0..(8000 * 10) {
            let t = i as f32 / 8000.0 - 0.23;
            let tb = t.rem_euclid(period);
            let v = if t >= 0.0 && tb < 0.02 { 0.8 } else { 0.01 * ((i * 7919 % 13) as f32 / 13.0 - 0.5) };
            m.push(v);
        }
        let info = analyze(&m.energies, m.hop_ms());
        let (p, ph) = info.tempo.expect("tempo");
        assert!((p - 600.0).abs() < 8.0, "period {p}");
        assert!((ph - 230.0).abs() <= 20.0, "phase {ph}");
    }

    #[test]
    fn noise_or_silence_has_no_tempo() {
        let e = vec![0.0f32; 2000];
        assert!(analyze(&e, 10.0).tempo.is_none());
        let mut s: u32 = 12345;
        let e: Vec<f32> = (0..3000).map(|_| { s = s.wrapping_mul(1_103_515_245).wrapping_add(12345); (s >> 16) as f32 / 65536.0 }).collect();
        assert!(analyze(&e, 10.0).tempo.is_none());
        assert!(analyze(&[0.5; 10], 10.0).tempo.is_none());
    }

    #[test]
    fn envelope_normalizes_and_levels() {
        let mut e = vec![0.1f32; 100];
        e.extend(vec![1.0f32; 100]);
        let env = envelope(&e, 10.0);
        assert_eq!(env.len(), 40);
        assert!((25..=26).contains(&env[0]));
        assert_eq!(env[39], 255);
        let info = BeatInfo { tempo: None, confidence: 0.0, env };
        assert!(info.level(10.0).unwrap() < 0.2);
        assert!(info.level(1990.0).unwrap() > 0.99);
        assert!(info.level(5000.0).is_none() && info.level(-1.0).is_none());
    }

    #[test]
    fn pending_hands_over_once() {
        let p = Pending::default();
        assert!(p.poll().is_none());
        p.put(None);
        assert_eq!(p.poll(), Some(None));
        assert!(p.poll().is_none());
        // 解码不了的文件 → 结果是 None
        let r = analyze_file(PathBuf::from("Z:/definitely/missing.mp3"), 5);
        let mut got = None;
        for _ in 0..200 {
            if let Some(x) = r.poll() { got = Some(x); break; }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(got, Some(None));
    }

    #[test]
    fn analyze_file_decodes_with_rodio() {
        let path = std::env::temp_dir().join(format!("hakimi-beat-{}.wav", std::process::id()));
        std::fs::write(&path, synth_wav(120.0, 8.0)).unwrap();
        let p = analyze_file(path.clone(), 6);
        let mut got = None;
        for _ in 0..500 {
            if let Some(x) = p.poll() { got = Some(x); break; }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let _ = std::fs::remove_file(&path);
        let info = got.expect("finished").expect("decoded");
        assert!((info.bpm().unwrap() - 120.0).abs() < 2.0, "{:?}", info.bpm());
        assert_eq!(info.env.len(), 6000 / ENV_MS as usize);
    }

    #[test]
    fn bin_peaks_fills_gaps() {
        let s = [(0.0, 0.1), (5.0, 0.3), (31.0, 0.2), (75.0, 0.5)];
        assert_eq!(bin_peaks(&s, 20.0), vec![0.3, 0.2, 0.2, 0.5]);
        assert!(bin_peaks(&[], 20.0).is_empty());
    }

    #[test]
    fn live_peaks_find_tempo() {
        // 120 BPM 的鼓点，电平表大约 15.6 ms 采一次（带抖动），每拍后 80 ms 衰减
        let mut s = Vec::new();
        let mut t = 0.0f64;
        fastrand::seed(5);
        while t < 8000.0 {
            let ph = t % 500.0;
            let v = if ph < 80.0 { 0.8 - ph as f32 / 200.0 } else { 0.05 + fastrand::f32() * 0.02 };
            s.push((t, v));
            t += 15.6 + fastrand::f64() * 2.0;
        }
        let info = analyze(&bin_peaks(&s, LIVE_HOP_MS), LIVE_HOP_MS);
        let bpm = info.bpm().expect("tempo");
        assert!((bpm - 120.0).abs() < 3.0, "bpm {bpm}");
    }
}
