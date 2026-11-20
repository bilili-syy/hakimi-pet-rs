//! 音频播放（rodio）。同一时间只放一个声音：新的 play 会先停掉旧的（旧的发 AudioDone）。
//! 一个常驻后台线程持有唯一的输出流（MixerDeviceSink），第一次出声时才打开设备，
//! 空闲 IDLE_CLOSE 后关掉设备释放 WASAPI 资源，下次出声再开。
//! 歌曲文件边读边解码（BufReader），不整个读进内存。
#![allow(dead_code)]
use crate::types::{AppEvent, Audio, EventSender};
use std::io::{BufReader, Cursor};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// 空闲多久关掉音频设备
const IDLE_CLOSE: Duration = Duration::from_secs(30);
/// 播放中多久检查一次是否放完
const POLL: Duration = Duration::from_millis(40);

enum Src {
    Mem(Vec<u8>),
    File(PathBuf),
}

enum Cmd {
    Play { token: u64, src: Src, volume: f32, max_secs: u32 },
    Stop,
}

static TX: OnceLock<Mutex<Sender<Cmd>>> = OnceLock::new();
static EVENTS: OnceLock<EventSender> = OnceLock::new();
static PLAYING: AtomicBool = AtomicBool::new(false);
/// 口型同步用：正在播放的声音最近 ~20ms 的 RMS（0..1 满幅，f32 的位），声卡线程写、UI 线程无锁读
static LEVEL: AtomicU32 = AtomicU32::new(0);

/// 当前出声的音量（滚动 RMS，0 = 静音/没在放）。和音量设置无关（量的是解码出来的样本）
pub fn level() -> f32 { f32::from_bits(LEVEL.load(Ordering::Relaxed)) }

fn set_level(v: f32) { LEVEL.store(v.to_bits(), Ordering::Relaxed); }

/// 启动时调用一次，保存 tx 用来发 AudioDone
pub fn init(tx: EventSender) {
    let _ = EVENTS.set(tx);
    sender();
}

fn sender() -> Option<Sender<Cmd>> {
    let m = TX.get_or_init(|| {
        let (tx, rx) = channel();
        if let Err(e) = std::thread::Builder::new().name("hakimi-audio".into()).spawn(move || worker(rx)) {
            crate::core::log::log(&format!("audio thread failed {e}"));
        }
        Mutex::new(tx)
    });
    Some(m.lock().unwrap_or_else(|e| e.into_inner()).clone())
}

fn done(token: u64) {
    if let Some(tx) = EVENTS.get() { tx.send(AppEvent::AudioDone { token }); }
}

fn send(cmd: Cmd) {
    let token = match &cmd { Cmd::Play { token, .. } => Some(*token), Cmd::Stop => None };
    let ok = sender().is_some_and(|s| s.send(cmd).is_ok());
    if !ok && let Some(t) = token { done(t); }
}

/// 播放编码好的音频（wav/ogg），volume 0~1；放完发 AppEvent::AudioDone { token }
pub fn play(token: u64, audio: Audio, volume: f32) {
    PLAYING.store(true, Ordering::SeqCst);
    send(Cmd::Play { token, src: Src::Mem(audio.0), volume, max_secs: 0 });
}

/// 播放本地歌曲文件（mp3/ogg/wav/m4a/flac/aac），最多 max_secs 秒；放完或到时发 AudioDone
pub fn play_file(token: u64, path: PathBuf, volume: f32, max_secs: u32) {
    PLAYING.store(true, Ordering::SeqCst);
    send(Cmd::Play { token, src: Src::File(path), volume, max_secs });
}

/// 停止当前声音
pub fn stop() { send(Cmd::Stop); }

/// 当前是否在出声（行为层用来判断气泡要不要多留一会儿）
pub fn is_playing() -> bool { PLAYING.load(Ordering::SeqCst) }

/// 这段字节 rodio 能不能解码（不开声卡）。用来探测内置台词的格式是否受支持。
pub fn can_decode(bytes: &[u8]) -> bool {
    let len = bytes.len() as u64;
    rodio::Decoder::builder()
        .with_data(Cursor::new(bytes.to_vec()))
        .with_byte_len(len)
        .with_seekable(true)
        .build()
        .is_ok_and(|mut d| d.next().is_some())
}

// ------------------------------------------------------------------ 后台线程

struct Current {
    token: u64,
    player: rodio::Player,
}

fn worker(rx: Receiver<Cmd>) {
    let mut sink: Option<rodio::MixerDeviceSink> = None;
    let mut cur: Option<Current> = None;
    let mut idle_since = Instant::now();
    loop {
        let msg = if cur.is_some() {
            rx.recv_timeout(POLL)
        } else if sink.is_some() {
            rx.recv_timeout(IDLE_CLOSE)
        } else {
            rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
        };
        match msg {
            Ok(Cmd::Play { token, src, volume, max_secs }) => {
                finish(&mut cur);
                match start(&mut sink, src, volume, max_secs) {
                    Ok(player) => cur = Some(Current { token, player }),
                    Err(e) => {
                        crate::core::log::log(&format!("audio error {e}"));
                        done(token);
                    }
                }
            }
            Ok(Cmd::Stop) => finish(&mut cur),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                finish(&mut cur);
                return;
            }
        }
        if cur.as_ref().is_some_and(|c| c.player.empty()) {
            let c = cur.take();
            if let Some(c) = c { done(c.token); }
        }
        if cur.is_some() {
            idle_since = Instant::now();
        } else {
            // 只有队列里确实没有新的 play 时才认为静音了
            PLAYING.store(false, Ordering::SeqCst);
            set_level(0.0);
            if sink.is_some() && idle_since.elapsed() >= IDLE_CLOSE {
                sink = None; // 关掉设备
            }
        }
    }
}

/// 停掉当前的声音并发 AudioDone
fn finish(cur: &mut Option<Current>) {
    if let Some(c) = cur.take() {
        c.player.stop();
        drop(c.player);
        done(c.token);
    }
}

fn start(sink: &mut Option<rodio::MixerDeviceSink>, src: Src, volume: f32, max_secs: u32) -> Result<rodio::Player, String> {
    if sink.is_none() {
        let mut s = rodio::DeviceSinkBuilder::open_default_sink().map_err(|e| format!("open device: {e}"))?;
        s.log_on_drop(false);
        *sink = Some(s);
    }
    let Some(s) = sink.as_ref() else { return Err("no device".into()) };
    let player = rodio::Player::connect_new(s.mixer());
    player.set_volume(volume.clamp(0.0, 1.0));
    match src {
        Src::Mem(bytes) => {
            let len = bytes.len() as u64;
            let dec = rodio::Decoder::builder()
                .with_data(Cursor::new(bytes))
                .with_byte_len(len)
                .with_seekable(true)
                .build()
                .map_err(|e| format!("decode: {e}"))?;
            append(&player, dec, max_secs);
        }
        Src::File(path) => {
            let f = std::fs::File::open(&path).map_err(|e| format!("open {}: {e}", path.display()))?;
            let len = f.metadata().map(|m| m.len()).unwrap_or(0);
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            let mut b = rodio::Decoder::builder().with_data(BufReader::new(f)).with_byte_len(len).with_seekable(true);
            if !ext.is_empty() { b = b.with_hint(&ext); }
            let dec = b.build().map_err(|e| format!("decode {}: {e}", path.display()))?;
            append(&player, dec, max_secs);
        }
    }
    Ok(player)
}

fn append<S>(player: &rodio::Player, src: S, max_secs: u32)
where
    S: rodio::Source + Send + 'static,
{
    if max_secs > 0 {
        player.append(Tap::new(src.take_duration(Duration::from_secs(u64::from(max_secs)))));
    } else {
        player.append(Tap::new(src));
    }
}

/// 透传样本的同时算滚动 RMS（每 ~20ms 一个窗口）写进 LEVEL。样本由声卡线程按播放进度拉取，
/// 所以 LEVEL 跟着「正在出声」的位置走（只差设备缓冲那十几毫秒）
struct Tap<S> {
    inner: S,
    acc: f32,
    n: u32,
    win: u32,
}

impl<S: rodio::Source> Tap<S> {
    fn new(inner: S) -> Self {
        let rate = u32::from(inner.sample_rate()) * u32::from(inner.channels().get());
        Self { inner, acc: 0.0, n: 0, win: (rate / 50).max(64) }
    }
}

impl<S: rodio::Source> Iterator for Tap<S> {
    type Item = rodio::Sample;

    #[inline]
    fn next(&mut self) -> Option<rodio::Sample> {
        let s = self.inner.next();
        match s {
            Some(v) => {
                self.acc += v * v;
                self.n += 1;
                if self.n >= self.win {
                    set_level((self.acc / self.n as f32).sqrt());
                    self.acc = 0.0;
                    self.n = 0;
                }
            }
            None => set_level(0.0),
        }
        s
    }

    fn size_hint(&self) -> (usize, Option<usize>) { self.inner.size_hint() }
}

impl<S: rodio::Source> rodio::Source for Tap<S> {
    fn current_span_len(&self) -> Option<usize> { self.inner.current_span_len() }
    fn channels(&self) -> rodio::ChannelCount { self.inner.channels() }
    fn sample_rate(&self) -> rodio::SampleRate { self.inner.sample_rate() }
    fn total_duration(&self) -> Option<Duration> { self.inner.total_duration() }
    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> { self.inner.try_seek(pos) }
}

#[cfg(test)]
mod tests {
    use super::can_decode;

    #[test]
    fn tap_measures_rms_and_passes_samples_through() {
        use rodio::Source;
        // 1 秒 440Hz 正弦，幅度 0.5 → RMS ≈ 0.354
        let wave = rodio::source::SineWave::new(440.0).amplify(0.5).take_duration(std::time::Duration::from_millis(200));
        let rate = u32::from(wave.sample_rate());
        let mut tap = super::Tap::new(wave);
        assert_eq!(u32::from(tap.sample_rate()), rate);
        let first: Vec<f32> = tap.by_ref().take(rate as usize / 10).collect();
        assert_eq!(first.len(), rate as usize / 10);
        let l = super::level();
        assert!((l - 0.354).abs() < 0.03, "rms {l}");
        // 放完归零
        for _ in tap.by_ref() {}
        assert_eq!(super::level(), 0.0);
    }

    #[test]
    fn decode_wav_and_probe_ogg() {
        // 不开声卡，只验证 rodio 能解 mp3（参考音频）和 wav（MiMo 返回的格式）
        assert!(can_decode(crate::assets::get("voice/hakimi-voice.mp3").unwrap()));
        assert!(can_decode(&crate::services::tts::tests_support::tiny_wav()));
        assert!(!can_decode(b"not audio"));
        // 内置台词是 Ogg Vorbis（48k 单声道），必须能解，否则每句都要在线合成
        let lines: serde_json::Value = serde_json::from_slice(crate::assets::get("voice/lines.json").unwrap()).unwrap();
        let t = crate::core::config::TtsConfig::default();
        let ogg = lines.as_object().unwrap().keys().find_map(|k| crate::services::tts::bundled(&t, k)).unwrap();
        assert!(can_decode(ogg));
        assert!(crate::services::tts::bundled_playable());
    }

    /// 诊断：按 start() 里 Src::Mem 完全一样的方式解码一句内置台词，把样本写成 16 位 wav，
    /// 另外按 48k 双声道（常见声卡格式）走一遍 rodio 的统一转换，看重采样/声道复制有没有问题。
    /// `HAKIMI_DUMP_TEXT=再戳要挠你了喵！ HAKIMI_DUMP_DIR=... cargo test -- --ignored dump_bundled_line`
    #[test]
    #[ignore]
    fn dump_bundled_line() {
        use rodio::Source;
        let text = std::env::var("HAKIMI_DUMP_TEXT").unwrap_or_else(|_| "再戳要挠你了喵！".into());
        let dir = std::path::PathBuf::from(std::env::var("HAKIMI_DUMP_DIR").unwrap_or_else(|_| ".".into()));
        let t = crate::core::config::TtsConfig::default();
        let bytes = crate::services::tts::bundled(&t, &text).expect("bundled line").to_vec();
        let len = bytes.len() as u64;
        let build = |b: Vec<u8>| {
            rodio::Decoder::builder().with_data(std::io::Cursor::new(b)).with_byte_len(len).with_seekable(true).build().unwrap()
        };
        let dec = build(bytes.clone());
        let (sr, ch): (u32, u16) = (dec.sample_rate().into(), dec.channels().into());
        let raw: Vec<f32> = dec.collect();
        println!("decoded: {sr} Hz, {ch} ch, {} samples, {:.3}s", raw.len(), raw.len() as f64 / f64::from(sr) / f64::from(ch));
        write_wav(&dir.join("rodio_raw.wav"), sr, ch, &raw);
        // 和真机一样：Player 接到 48k 双声道的 mixer 上，从 mixer 输出端拉样本（声卡就是这么拉的）
        let (mixer, out) = rodio::mixer::mixer(2.try_into().unwrap(), 48_000.try_into().unwrap());
        let player = rodio::Player::connect_new(&mixer);
        player.set_volume(1.0);
        player.append(build(bytes));
        let conv: Vec<f32> = out.take(96_000 * 4).collect();
        let last = conv.iter().rposition(|s| s.abs() > 1e-4).unwrap_or(0);
        println!("via mixer 48k/2ch: last non-silent sample at {:.3}s, player empty={}", last as f64 / 96_000.0, player.empty());
        write_wav(&dir.join("rodio_48k_stereo.wav"), 48_000, 2, &conv);
    }

    fn write_wav(path: &std::path::Path, sr: u32, ch: u16, samples: &[f32]) {
        let data_len = (samples.len() * 2) as u32;
        let mut out = Vec::with_capacity(44 + data_len as usize);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&ch.to_le_bytes());
        out.extend_from_slice(&sr.to_le_bytes());
        out.extend_from_slice(&(sr * u32::from(ch) * 2).to_le_bytes());
        out.extend_from_slice(&(ch * 2).to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for s in samples {
            out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
        }
        std::fs::write(path, out).unwrap();
    }

    /// 真开声卡（音量 0，不出声）：验证 play → AudioDone、stop → AudioDone。
    /// `cargo test -- --ignored audio_device` 手动跑；CI/无声卡环境跳过。
    #[test]
    #[ignore]
    fn audio_device_roundtrip() {
        use crate::types::{AppEvent, Audio, EventSender};
        use std::sync::mpsc::channel;
        use std::time::Duration;
        let (tx, rx) = channel();
        let tx = std::sync::Mutex::new(tx);
        super::init(EventSender::new(move |ev| {
            if let AppEvent::AudioDone { token } = ev { let _ = tx.lock().unwrap().send(token); }
        }));
        let wav = crate::assets::get("voice/hakimi-voice.mp3").unwrap().to_vec();
        super::play(1, Audio(wav.clone()), 0.0);
        assert!(super::is_playing());
        std::thread::sleep(Duration::from_millis(300));
        super::play(2, Audio(wav), 0.0); // 顶掉 1
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), 1);
        std::thread::sleep(Duration::from_millis(300));
        super::stop();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), 2);
        std::thread::sleep(Duration::from_millis(200));
        assert!(!super::is_playing());
    }
}
