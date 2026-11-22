//! 音频播放（rodio）。两个声道：
//! - 语音（play）：同一时间只放一句，新的 play 先停掉旧的（旧的发 AudioDone）
//! - 音乐（play_file）：同一时间只放一首，新歌顶掉旧歌
//!
//! 两个声道互不打断，混在一起出声；有语音时音乐压低到 DUCK 倍，语音放完恢复。
//! 一个常驻后台线程持有唯一的输出流（MixerDeviceSink），第一次出声时才打开设备，
//! 空闲 IDLE_CLOSE 后关掉设备释放 WASAPI 资源，下次出声再开。
//! 歌曲文件边读边解码（BufReader），不整个读进内存。
//!
//! 流式语音（MiMo SSE pcm16）：tts 先拿 [`stream_open`] 开一个 PCM 缓冲，把它的「句柄」当 Audio 交给行为层，
//! 行为层照常 play；这里认出句柄就边收边播（缓冲暂时空了先补静音，收完放完才算结束）。
//!
//! 口型电平（level）和 is_playing 只看语音，不看音乐，也不看设置窗口的试听（token ≥ PREVIEW_TOKEN_MIN）。
#![allow(dead_code)]
use crate::types::{AppEvent, Audio, EventSender};
use std::collections::VecDeque;
use std::io::{BufReader, Cursor};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use rodio::Source as _;
use std::time::{Duration, Instant};

/// 空闲多久关掉音频设备
const IDLE_CLOSE: Duration = Duration::from_secs(30);
/// 播放中多久检查一次是否放完
const POLL: Duration = Duration::from_millis(40);
/// 说话时音乐压到多少
pub const DUCK: f32 = 0.35;
/// 这个值以上的 token 算「试听」（设置窗口的 TEST_TOKEN = u64::MAX - 7）：照常出声，但不带口型、不算在说话
pub const PREVIEW_TOKEN_MIN: u64 = u64::MAX - 1024;
/// 流式语音：缓冲空着、又这么久没收到新数据，就当断流结束
const STREAM_STALL: Duration = Duration::from_secs(8);

pub fn is_preview(token: u64) -> bool { token >= PREVIEW_TOKEN_MIN }

enum Src {
    Mem(Vec<u8>),
    File(PathBuf),
    Stream(Arc<PcmStream>),
}

enum Cmd {
    Play { token: u64, src: Src, volume: f32, gain: f32, max_secs: u32, music: bool },
    SetVolume { voice: f32, music: f32 },
    Stop { voice: bool, music: bool },
    /// 停掉正在放这个 token 的声道（不管是语音还是音乐）
    StopToken(u64),
}

static TX: OnceLock<Mutex<Sender<Cmd>>> = OnceLock::new();
static EVENTS: OnceLock<EventSender> = OnceLock::new();
static PLAYING: AtomicBool = AtomicBool::new(false);
/// 口型同步用：正在播放的语音最近 ~20ms 的 RMS（0..1 满幅，f32 的位），声卡线程写、UI 线程无锁读
static LEVEL: AtomicU32 = AtomicU32::new(0);

/// 当前语音的音量（滚动 RMS，0 = 静音/没在说话）。和音量设置无关（量的是解码出来的样本）；音乐和试听不算
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
    crate::core::log::trace(&format!("audio done:{token}"));
    if let Some(tx) = EVENTS.get() { tx.send(AppEvent::AudioDone { token }); }
}

fn send(cmd: Cmd) -> bool {
    let token = match &cmd { Cmd::Play { token, .. } => Some(*token), _ => None };
    let ok = sender().is_some_and(|s| s.send(cmd).is_ok());
    if !ok && let Some(t) = token { done(t); }
    ok
}

/// 还在通道里排队、尚未被 worker 消费的非试听语音 Play 计数，避免 PLAYING 被过早清零
static PENDING_VOICE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// 播放一句语音（wav/ogg/mp3，或 [`stream_open`] 给的流式句柄），volume 0~1；放完发 AppEvent::AudioDone { token }。
/// 会顶掉正在念的上一句，不影响正在放的歌
pub fn play(token: u64, audio: Audio, volume: f32) {
    let tracked = !is_preview(token);
    if tracked {
        PENDING_VOICE.fetch_add(1, Ordering::SeqCst);
        PLAYING.store(true, Ordering::SeqCst);
    }
    let src = match take_stream(&audio.0) {
        Some(s) => Src::Stream(s),
        None => Src::Mem(audio.0),
    };
    if !send(Cmd::Play { token, src, volume, gain: 1.0, max_secs: 0, music: false }) && tracked {
        let prev = PENDING_VOICE.fetch_sub(1, Ordering::SeqCst);
        if prev <= 1 {
            PLAYING.store(false, Ordering::SeqCst);
        }
    }
}

/// 播放本地歌曲文件（mp3/ogg/wav/m4a/flac/aac），最多 max_secs 秒；放完或到时发 AudioDone。
/// 顶掉正在放的上一首，不打断语音（有语音时自动压低）
pub fn play_file(token: u64, path: PathBuf, volume: f32, max_secs: u32) {
    let _ = send(Cmd::Play { token, src: Src::File(path), volume, gain: 1.0, max_secs, music: true });
}

/// 全停（语音和音乐）
pub fn stop() { let _ = send(Cmd::Stop { voice: true, music: true }); }

/// 停掉这个 token 的声音（语音或音乐，哪个在放它停哪个；已经放完的不管）
pub fn stop_token(token: u64) { let _ = send(Cmd::StopToken(token)); }

/// 内存里的一段音乐（比如跳舞的内置鼓点）走音乐声道：不被说话顶掉，说话时自动压低
pub fn play_music_mem(token: u64, audio: Audio, volume: f32, max_secs: u32) {
    play_music_mem_scaled(token, audio, volume, 1.0, max_secs);
}

/// 带相对增益系数的内存音乐播放（如内置鼓点固定乘 0.8，实时调音量时也保留 0.8 比例）
pub fn play_music_mem_scaled(token: u64, audio: Audio, base_volume: f32, gain: f32, max_secs: u32) {
    let g = gain.clamp(0.0, 1.0);
    let volume = (base_volume.clamp(0.0, 1.0) * g).clamp(0.0, 1.0);
    let _ = send(Cmd::Play { token, src: Src::Mem(audio.0), volume, gain: g, max_secs, music: true });
}

/// 实时调节正在播放的语音与音乐声道音量（滑块拖动、静音切换、Ctrl+滚轮时立即生效；未初始化声道时不拉起后台线程）
pub fn set_volumes(voice: f32, music: f32) {
    if let Some(m) = TX.get() {
        let s = m.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let _ = s.send(Cmd::SetVolume { voice: voice.clamp(0.0, 1.0), music: music.clamp(0.0, 1.0) });
    }
}

/// 按当前内存配置同步正在播放的语音与音乐音量
pub fn apply_config_volume() {
    let t = crate::core::config::get().tts;
    set_volumes(t.voice_vol(), t.music_vol());
}

/// 只停语音（歌照放）
pub fn stop_voice() { let _ = send(Cmd::Stop { voice: true, music: false }); }

/// 只停音乐（话照说）
pub fn stop_music() { let _ = send(Cmd::Stop { voice: false, music: true }); }

/// 当前是否在说话（行为层用来判断气泡要不要多留一会儿、口型要不要动）。音乐和试听不算
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

// ------------------------------------------------------------------ 流式 PCM

/// 句柄格式：MAGIC + id（小端 u64）。只在 tts → 行为层 → audio::play 之间传，不落盘
const MAGIC: &[u8; 8] = b"HKPCMSTR";
/// 最多同时挂几个还没被 play 取走的流（行为层没播就丢了的，最老的先扔）
const MAX_OPEN: usize = 8;

/// 边收边播的 16 位单声道 PCM 缓冲。tts 线程写，声卡线程读
pub struct PcmStream {
    rate: u32,
    /// 快速原子标志：缓冲有数据或已结束时为 true，欠载期间声卡线程无需每样本抢 Mutex
    ready: AtomicBool,
    buf: Mutex<PcmBuf>,
}

struct PcmBuf {
    samples: VecDeque<f32>,
    /// 上一块剩下的半个样本（字节数是奇数时）
    odd: Option<u8>,
    done: bool,
    last_push: Instant,
    /// 一共收到多少个样本（统计/测试用）
    total: usize,
}

impl PcmStream {
    fn new(rate: u32) -> Self {
        Self {
            rate,
            ready: AtomicBool::new(false),
            buf: Mutex::new(PcmBuf { samples: VecDeque::new(), odd: None, done: false, last_push: Instant::now(), total: 0 }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PcmBuf> { self.buf.lock().unwrap_or_else(|e| e.into_inner()) }

    /// 追加一块小端 16 位 PCM 字节（有最大缓冲上限防消费端停止拉取导致内存无限涨）
    pub fn push_pcm16(&self, bytes: &[u8]) {
        let max_samples = (self.rate as usize).saturating_mul(60);
        let mut b = self.lock();
        let mut it = bytes.iter().copied();
        let mut pushed = 0usize;
        let mut push_one = |samples: &mut VecDeque<f32>, lo: u8, hi: u8| {
            if samples.len() < max_samples {
                samples.push_back(f32::from(i16::from_le_bytes([lo, hi])) / 32768.0);
                pushed += 1;
            }
        };
        if let Some(lo) = b.odd.take() {
            match it.next() {
                Some(hi) => push_one(&mut b.samples, lo, hi),
                None => {
                    b.odd = Some(lo);
                    return;
                }
            }
        }
        loop {
            match (it.next(), it.next()) {
                (Some(lo), Some(hi)) => push_one(&mut b.samples, lo, hi),
                (Some(lo), None) => {
                    b.odd = Some(lo);
                    break;
                }
                _ => break,
            }
        }
        b.total = b.total.saturating_add(pushed);
        b.last_push = Instant::now();
        if !b.samples.is_empty() {
            self.ready.store(true, Ordering::Release);
        }
    }

    /// 收完了（正常结束或出错都调）：放完缓冲里剩下的就结束
    pub fn finish(&self) {
        let mut b = self.lock();
        b.done = true;
        self.ready.store(true, Ordering::Release);
    }

    pub fn is_done(&self) -> bool { self.lock().done }

    /// 一次最多取 n 个样本；返回是否已经结束且取空
    fn pull(&self, n: usize, out: &mut Vec<f32>) -> bool {
        let mut b = self.lock();
        let k = n.min(b.samples.len());
        out.extend(b.samples.drain(..k));
        if b.samples.is_empty() && !b.done {
            self.ready.store(false, Ordering::Release);
        }
        let stalled = b.samples.is_empty() && b.last_push.elapsed() > STREAM_STALL;
        (b.done || stalled) && b.samples.is_empty()
    }
}

static STREAMS: Mutex<Vec<(u64, Arc<PcmStream>)>> = Mutex::new(Vec::new());
static STREAM_SEQ: AtomicU64 = AtomicU64::new(1);

/// 开一个流式 PCM 缓冲（rate Hz、单声道、16 位）。返回能当 Audio 传的句柄和写入端
pub fn stream_open(rate: u32) -> (Audio, Arc<PcmStream>) {
    let id = STREAM_SEQ.fetch_add(1, Ordering::Relaxed);
    let s = Arc::new(PcmStream::new(rate.max(8000)));
    let mut v = STREAMS.lock().unwrap_or_else(|e| e.into_inner());
    v.push((id, s.clone()));
    while v.len() > MAX_OPEN { v.remove(0); }
    let mut h = MAGIC.to_vec();
    h.extend_from_slice(&id.to_le_bytes());
    (Audio(h), s)
}

/// 这段 Audio 是不是流式句柄
pub fn is_stream(bytes: &[u8]) -> bool { bytes.len() == 16 && bytes.starts_with(MAGIC) }

/// 句柄 → 流（取走，一个流只能播一次）
fn take_stream(bytes: &[u8]) -> Option<Arc<PcmStream>> {
    if !is_stream(bytes) { return None; }
    let id = u64::from_le_bytes(bytes[8..16].try_into().ok()?);
    let mut v = STREAMS.lock().unwrap_or_else(|e| e.into_inner());
    let i = v.iter().position(|(k, _)| *k == id)?;
    Some(v.remove(i).1)
}

/// 流式 PCM → rodio Source。缓冲暂时空了补静音（不结束），收完且放空才结束
struct StreamSrc {
    s: Arc<PcmStream>,
    local: Vec<f32>,
    pos: usize,
    ended: bool,
    /// 欠载静音样本计数：每 ~10ms 才加锁检查一次断流超时，平时仅做一次原子读
    underrun: u32,
}

impl StreamSrc {
    fn new(s: Arc<PcmStream>) -> Self { Self { s, local: Vec::with_capacity(2048), pos: 0, ended: false, underrun: 0 } }
}

impl Iterator for StreamSrc {
    type Item = rodio::Sample;

    fn next(&mut self) -> Option<rodio::Sample> {
        if self.pos >= self.local.len() {
            if self.ended { return None; }
            self.local.clear();
            self.pos = 0;
            // 无新数据也没结束时，仅每 ~10ms 查一次 stall 超时，避免 24kHz 自旋抢 Mutex
            let stall_every = (self.s.rate / 100).max(128);
            if !self.s.ready.load(Ordering::Acquire) {
                self.underrun += 1;
                if self.underrun < stall_every {
                    return Some(0.0);
                }
                self.underrun = 0;
            }
            let end = self.s.pull(1024, &mut self.local);
            if self.local.is_empty() {
                if end {
                    self.ended = true;
                    return None;
                }
                return Some(0.0);
            }
            self.underrun = 0;
        }
        let v = self.local[self.pos];
        self.pos += 1;
        Some(v)
    }
}

impl rodio::Source for StreamSrc {
    fn current_span_len(&self) -> Option<usize> { None }
    fn channels(&self) -> rodio::ChannelCount { std::num::NonZero::<u16>::MIN }
    fn sample_rate(&self) -> rodio::SampleRate { std::num::NonZero::new(self.s.rate).unwrap_or(std::num::NonZero::<u32>::MIN) }
    fn total_duration(&self) -> Option<Duration> { None }
}

// ------------------------------------------------------------------ 后台线程

struct Current {
    token: u64,
    player: rodio::Player,
    /// 当前目标音量（音乐声道未乘 DUCK 前的音量）
    volume: f32,
    /// 曲目固有增益（如内置鼓点 0.8，歌曲/语音 1.0）
    gain: f32,
}

fn worker(rx: Receiver<Cmd>) {
    let mut sink: Option<rodio::MixerDeviceSink> = None;
    let mut voice: Option<Current> = None;
    let mut music: Option<Current> = None;
    let mut ducked = false;
    let mut idle_since = Instant::now();
    loop {
        let busy = voice.is_some() || music.is_some();
        let msg = if busy {
            rx.recv_timeout(POLL)
        } else if sink.is_some() {
            rx.recv_timeout(IDLE_CLOSE)
        } else {
            rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
        };
        match msg {
            Ok(Cmd::Play { token, src, volume, gain, max_secs, music: is_music }) => {
                let tracked = !is_music && !is_preview(token);
                if tracked {
                    PENDING_VOICE.fetch_sub(1, Ordering::SeqCst);
                }
                let slot = if is_music { &mut music } else { &mut voice };
                finish(slot, !is_music);
                let meter = tracked;
                match start(&mut sink, src, volume, max_secs, meter) {
                    Ok(player) => {
                        crate::core::log::trace(&format!("audio start:{token}{}", if is_music { " music" } else { "" }));
                        if !is_music { super::tts::lat_sound(token); }
                        *slot = Some(Current { token, player, volume, gain });
                        ducked = false; // 下面按现状重新压
                    }
                    Err(e) => {
                        crate::core::log::log(&format!("audio error {e}"));
                        done(token);
                    }
                }
            }
            Ok(Cmd::SetVolume { voice: v_vol, music: m_vol }) => {
                let talking = voice.is_some();
                if let Some(v) = &mut voice {
                    v.volume = (v_vol.clamp(0.0, 1.0) * v.gain).clamp(0.0, 1.0);
                    v.player.set_volume(v.volume);
                }
                if let Some(m) = &mut music {
                    m.volume = (m_vol.clamp(0.0, 1.0) * m.gain).clamp(0.0, 1.0);
                    m.player.set_volume(duck_volume(m.volume, talking));
                    ducked = talking;
                }
            }
            Ok(Cmd::Stop { voice: v, music: m }) => {
                if v { finish(&mut voice, true); }
                if m { finish(&mut music, false); }
            }
            Ok(Cmd::StopToken(t)) => {
                if voice.as_ref().is_some_and(|c| c.token == t) { finish(&mut voice, true); }
                if music.as_ref().is_some_and(|c| c.token == t) { finish(&mut music, false); }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                finish(&mut voice, true);
                finish(&mut music, false);
                return;
            }
        }
        for (idx, slot) in [&mut voice, &mut music].into_iter().enumerate() {
            if slot.as_ref().is_some_and(|c| c.player.empty()) && let Some(c) = slot.take() {
                if idx == 0 { set_level(0.0); }
                done(c.token);
            }
        }
        // 说话时压低音乐，说完恢复
        let talking = voice.is_some();
        if let Some(m) = &music && talking != ducked {
            m.player.set_volume(duck_volume(m.volume, talking));
            ducked = talking;
        }
        let voice_playing = voice.as_ref().is_some_and(|c| !is_preview(c.token))
            || PENDING_VOICE.load(Ordering::SeqCst) > 0;
        PLAYING.store(voice_playing, Ordering::SeqCst);
        if !voice_playing {
            set_level(0.0);
        }
        if voice.is_some() || music.is_some() {
            idle_since = Instant::now();
        } else if sink.is_some() && idle_since.elapsed() >= IDLE_CLOSE {
            sink = None; // 关掉设备
        }
    }
}

/// 音乐该用的音量：说话时压到 DUCK 倍
pub fn duck_volume(base: f32, talking: bool) -> f32 {
    let v = base.clamp(0.0, 1.0);
    if talking { v * DUCK } else { v }
}

/// 停掉这个声道当前的声音并发 AudioDone
fn finish(cur: &mut Option<Current>, is_voice: bool) {
    if let Some(c) = cur.take() {
        c.player.stop();
        drop(c.player);
        if is_voice { set_level(0.0); }
        done(c.token);
    }
}

fn start(sink: &mut Option<rodio::MixerDeviceSink>, src: Src, volume: f32, max_secs: u32, meter: bool) -> Result<rodio::Player, String> {
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
            let t0 = Instant::now();
            let dec = rodio::Decoder::builder()
                .with_data(Cursor::new(bytes))
                .with_byte_len(len)
                .with_seekable(true)
                .build()
                .map_err(|e| format!("decode: {e}"))?;
            crate::core::log::trace(&format!("audio decoder ready {}ms", t0.elapsed().as_millis()));
            append(&player, Box::new(dec), max_secs, meter);
        }
        Src::Stream(st) => append(&player, Box::new(StreamSrc::new(st)), max_secs, meter),
        Src::File(path) => {
            let f = std::fs::File::open(&path).map_err(|e| format!("open {}: {e}", path.display()))?;
            let len = f.metadata().map(|m| m.len()).unwrap_or(0);
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            let mut b = rodio::Decoder::builder().with_data(BufReader::new(f)).with_byte_len(len).with_seekable(true);
            if !ext.is_empty() { b = b.with_hint(&ext); }
            let dec = b.build().map_err(|e| format!("decode {}: {e}", path.display()))?;
            append(&player, Box::new(dec), max_secs, meter);
        }
    }
    Ok(player)
}

/// 所有声音统一装进一个 Box<dyn Source>：Tap / take_duration / Player 的整条泛型链只实例化一份
/// （每多一种来源类型都会多一份重采样、混音代码，exe 会变大）
type BoxSrc = Box<dyn rodio::Source + Send>;

fn append(player: &rodio::Player, src: BoxSrc, max_secs: u32, meter: bool) {
    let src: BoxSrc = if max_secs > 0 { Box::new(src.take_duration(Duration::from_secs(u64::from(max_secs)))) } else { src };
    player.append(Tap::new(src, meter));
}

/// 透传样本的同时算滚动 RMS（每 ~20ms 一个窗口）写进 LEVEL（meter=false 时不写：音乐、试听）。
/// 样本由声卡线程按播放进度拉取，所以 LEVEL 跟着「正在出声」的位置走（只差设备缓冲那十几毫秒）
struct Tap<S> {
    inner: S,
    acc: f32,
    n: u32,
    win: u32,
    meter: bool,
}

impl<S: rodio::Source> Tap<S> {
    fn new(inner: S, meter: bool) -> Self {
        let rate = u32::from(inner.sample_rate()) * u32::from(inner.channels().get());
        Self { inner, acc: 0.0, n: 0, win: (rate / 50).max(64), meter }
    }
}

impl<S: rodio::Source> Iterator for Tap<S> {
    type Item = rodio::Sample;

    #[inline]
    fn next(&mut self) -> Option<rodio::Sample> {
        let s = self.inner.next();
        if !self.meter { return s; }
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
        let mut tap = super::Tap::new(wave, true);
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
    fn tap_without_meter_leaves_level_alone() {
        use rodio::Source;
        // 音乐/试听不动口型电平（这里只验证不写，并行测试里别的 Tap 可能在写，所以只看自己写没写）
        let wave = rodio::source::SineWave::new(440.0).take_duration(std::time::Duration::from_millis(50));
        let mut tap = super::Tap::new(wave, false);
        assert!(!tap.meter);
        assert!(tap.by_ref().count() > 0);
        assert_eq!(tap.n, 0);
    }

    #[test]
    fn preview_and_duck() {
        assert!(super::is_preview(u64::MAX - 7));
        assert!(!super::is_preview(12345));
        assert!((super::duck_volume(0.8, true) - 0.8 * super::DUCK).abs() < 1e-6);
        assert_eq!(super::duck_volume(0.8, false), 0.8);
        assert_eq!(super::duck_volume(3.0, false), 1.0);
    }

    #[test]
    fn pcm_stream_plays_while_receiving() {
        use rodio::Source;
        let (h, s) = super::stream_open(24_000);
        assert!(super::is_stream(&h.0));
        assert!(!super::is_stream(b"RIFF...."));
        let st = super::take_stream(&h.0).expect("handle");
        assert!(super::take_stream(&h.0).is_none(), "一个流只能取一次");
        let mut src = super::StreamSrc::new(st);
        assert_eq!(u32::from(src.sample_rate()), 24_000);
        assert_eq!(src.channels().get(), 1);
        // 还没数据：补静音，不结束
        assert_eq!(src.next(), Some(0.0));
        // 奇数字节分两块送：0x4000 = 16384 → 0.5
        s.push_pcm16(&[0x00]);
        s.push_pcm16(&[0x40, 0x00, 0xC0]);
        assert_eq!(src.next(), Some(0.5));
        assert_eq!(src.next(), Some(-0.5));
        assert_eq!(src.next(), Some(0.0)); // 又空了
        s.push_pcm16(&[0xFF, 0x7F]);
        s.finish();
        assert!((src.next().unwrap() - 32767.0 / 32768.0).abs() < 1e-6);
        assert_eq!(src.next(), None);
        assert_eq!(src.next(), None);
    }

    #[test]
    fn stream_registry_is_bounded() {
        let hs: Vec<_> = (0..super::MAX_OPEN + 3).map(|_| super::stream_open(24_000).0).collect();
        // 最老的几个被扔掉，最新的还在
        assert!(super::take_stream(&hs.last().unwrap().0).is_some());
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
