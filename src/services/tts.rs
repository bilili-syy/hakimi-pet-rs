//! 语音：移植 Electron 版 src/main/tts.js
//! - 小米 MiMo TTS（OpenAI 兼容 /v1/chat/completions），三种模式 clone / preset / design
//!   - clone：一次性请求，回 base64 mp3（实测 SSE 也只回一个大块，流式没收益）
//!   - preset / design：前台台词走 SSE 流式 pcm16，收到第一块就开始播（audio::stream_open 句柄），收完存成 wav 缓存
//!   - 预热、设置窗口试听：一律一次性 mp3
//! - 延迟追踪：HAKIMI_TRACE=1 时每句记「查缓存 → 发请求 → 首字节 → 首块音频/收完 → 首音播出」（见 [`lat`]）
//! - 内置台词：assets/voice/lines/<sha1(text) 前16位>.ogg（只在 clone 模式用，crate::assets::get 读）
//! - tts-cache：每句存一份，两个键（台词+情绪、只按台词）用硬链接指向同一个文件；按修改时间淘汰，最多 400 句 / 40MB
//! - 同一时间只发一个请求；新台词来了旧的立刻回 None；429 冷却 10 秒；5xx/网络错误/超时重试一次（第一次的超时按字数 8–20 秒，见 [`first_timeout`]）
//! - 预热：后台慢慢合成固定台词，前台说话时让路
//!
//! 和原版的差别：ureq 是同步的，没法真正掐断在途请求。被顶掉的请求会立刻回 `audio: None`，
//! 但 HTTP 请求本身会跑完（结果照样进缓存），下一句排在它后面发，保证不并发（防限流）。
#![allow(dead_code)]
use crate::core::config::TtsConfig;
use crate::types::{AppEvent, Audio, EventSender, SpeakKind, VoiceMood};
use base64::Engine;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

pub const API: &str = "https://api.xiaomimimo.com/v1/chat/completions";
/// 每句存两份：按「台词+情绪」和只按台词（换了情绪也能秒播）
/// tts-cache 最多存多少句（每句两个键、一份文件）
pub const MAX_CACHE: usize = 400;
/// 让 MiMo 直接回 mp3：同一句 wav ~250KB → mp3 ~30–40KB，下载和缓存都小 6–8 倍，rodio 能直接解
const AUDIO_FORMAT: &str = "mp3";
/// tts-cache 总大小上限（字节）
pub const MAX_CACHE_BYTES: u64 = 40 * 1024 * 1024;
/// 缓存文件扩展名：新写的按内容判断（mp3 / wav），读的时候两种都认
const CACHE_EXTS: [&str; 2] = ["mp3", "wav"];
/// 预热最多多少条（main.js 里 items.slice(0, 300)）
pub const MAX_PREWARM: usize = 300;
const COOLDOWN: Duration = Duration::from_secs(10);
/// MiMo 流式 pcm16 的采样率（单声道 16 位）
pub const STREAM_RATE: u32 = 24_000;
/// 流式请求整次的超时（秒）。长句 ~10 秒音频实测 3 秒收完
const STREAM_TIMEOUT: u64 = 30;

/// 状态 → 情绪标签（MiMo 支持在文本开头写 (风格)）
pub fn mood_tag(m: VoiceMood) -> &'static str {
    match m {
        VoiceMood::Happy => "开心 俏皮",
        VoiceMood::Sad => "委屈",
        VoiceMood::Hungry => "撒娇 委屈",
        VoiceMood::Sleepy => "慵懒 疲惫",
        VoiceMood::Mad => "生气 撒娇",
        VoiceMood::Think => "俏皮",
        VoiceMood::Calm => "温柔",
    }
}

// ------------------------------------------------------------------ 文本 & 缓存键

/// 原版 clean()：去 emoji、合并波浪号、trim、截到 300 个 UTF-16 单位
pub fn clean(text: &str) -> String {
    let mut s = String::with_capacity(text.len());
    let mut last_tilde = false;
    for c in text.chars() {
        let cp = c as u32;
        if (0x1F000..=0x1FAFF).contains(&cp) || (0x2600..=0x27BF).contains(&cp) || cp == 0xFE0F { continue; }
        if c == '～' || c == '~' {
            if !last_tilde { s.push('～'); }
            last_tilde = true;
            continue;
        }
        last_tilde = false;
        s.push(c);
    }
    let s = s.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    let mut out = String::new();
    let mut units = 0;
    for c in s.chars() {
        units += c.len_utf16();
        if units > 300 { break; }
        out.push(c);
    }
    out
}

/// 至少有一个汉字/字母/数字才念（原版 /[\u4e00-\u9fa5A-Za-z0-9]/）
fn speakable(text: &str) -> bool {
    text.chars().any(|c| c.is_ascii_alphanumeric() || ('\u{4e00}'..='\u{9fa5}').contains(&c))
}

fn sha1_hex(s: &str) -> String { sha1_smol::Sha1::from(s.as_bytes()).digest().to_string() }

fn mood_key(m: Option<VoiceMood>) -> &'static str { m.map(VoiceMood::key).unwrap_or("") }

/// 原版 cacheKey：sha1([mode, voice, mode==clone ? '' : style, mood, text].join('|'))；
/// sovits 模式把参考音频路径与参考文本编进键里；角色包自定义了 clone_ref 时也编进键里避免与哈基米原声撞缓存
pub fn cache_key(t: &TtsConfig, text: &str, mood: Option<VoiceMood>) -> String {
    if t.mode == "sovits" {
        return sha1_hex(&["sovits", t.sovits_ref.trim(), t.sovits_prompt.trim(), mood_key(mood), text].join("|"));
    }
    if t.mode == "clone" && !t.clone_ref.trim().is_empty() {
        return sha1_hex(&["clone-skin", t.clone_ref.trim(), mood_key(mood), text].join("|"));
    }
    let style = if t.mode == "clone" { "" } else { t.style.as_str() };
    sha1_hex(&[t.mode.as_str(), t.voice.as_str(), style, mood_key(mood), text].join("|"))
}

/// 原版 textKey：'T' + sha1([mode, voice, text].join('|'))
pub fn text_key(t: &TtsConfig, text: &str) -> String {
    if t.mode == "sovits" {
        return format!("T{}", sha1_hex(&["sovits", t.sovits_ref.trim(), t.sovits_prompt.trim(), text].join("|")));
    }
    if t.mode == "clone" && !t.clone_ref.trim().is_empty() {
        return format!("T{}", sha1_hex(&["clone-skin", t.clone_ref.trim(), text].join("|")));
    }
    if t.mode == "design" {
        return format!("T{}", sha1_hex(&["design", t.voice.as_str(), t.style.as_str(), text].join("|")));
    }
    format!("T{}", sha1_hex(&[t.mode.as_str(), t.voice.as_str(), text].join("|")))
}

/// 内置台词能不能播（它们是 Ogg Vorbis，rodio 能解；Opus 不行）。
/// 保险：启动后第一次用到时拿一个文件试解一次，万一换进了解不了的格式就跳过内置台词，走 tts-cache / 在线合成。
pub fn bundled_playable() -> bool {
    static OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OK.get_or_init(|| {
        let ok = crate::assets::get("voice/lines.json")
            .and_then(|b| serde_json::from_slice::<Value>(b).ok())
            .and_then(|v| {
                let t = TtsConfig::default();
                v.as_object()?.keys().find_map(|k| bundled(&t, k))
            })
            .is_some_and(super::audio::can_decode);
        if !ok && !cfg!(test) { crate::core::log::log("bundled voice lines not decodable, using tts-cache / MiMo"); }
        ok
    })
}

/// 内置台词（仅在哈基米默认声线下启用：未指定自定义 clone_ref 的 clone 模式，或未指定 sovitsRef 的默认 GPT-SoVITS 模式）
pub fn bundled(t: &TtsConfig, text: &str) -> Option<&'static [u8]> {
    let default_hakimi = (t.mode == "clone" && t.clone_ref.trim().is_empty())
        || (t.mode == "sovits" && t.sovits_ref.trim().is_empty());
    if !default_hakimi { return None; }
    crate::assets::get(&format!("voice/lines/{}.ogg", &sha1_hex(text)[..16]))
}

fn cache_dir() -> PathBuf { crate::core::paths::tts_cache_dir() }

/// 内置 → 缓存（命中缓存会更新修改时间，LRU）
pub fn read_cache(t: &TtsConfig, text: &str, mood: Option<VoiceMood>) -> Option<Vec<u8>> {
    // 角色包的 voiceDir（文件名规则同内置台词）优先；放不出来的格式跳过
    if let Some(b) = crate::core::skin::voice_line(text).filter(|b| super::audio::can_decode(b)) {
        crate::core::log::trace(&format!("voice skin {text}"));
        return Some(b);
    }
    if let Some(b) = bundled(t, text).filter(|_| bundled_playable()) {
        crate::core::log::trace(&format!("voice bundled {text}"));
        return Some(b.to_vec());
    }
    for k in [cache_key(t, text, mood), text_key(t, text)] {
        let Some(file) = CACHE_EXTS.iter().map(|e| cache_dir().join(format!("{k}.{e}"))).find(|f| f.is_file()) else { continue };
        if let Ok(buf) = std::fs::read(&file) {
            if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&file) {
                let _ = f.set_modified(std::time::SystemTime::now());
            }
            crate::core::log::trace(&format!("voice cache {text}"));
            return Some(buf);
        }
    }
    None
}

/// 音频内容 → 缓存扩展名
fn audio_ext(b: &[u8]) -> &'static str {
    if b.starts_with(b"RIFF") { "wav" } else if b.starts_with(b"ID3") || (b.len() > 1 && b[0] == 0xFF && b[1] & 0xE0 == 0xE0) { "mp3" } else { "wav" }
}

fn write_cache(t: &TtsConfig, text: &str, mood: Option<VoiceMood>, audio: &[u8]) {
    write_cache_in(&cache_dir(), &cache_key(t, text, mood), &text_key(t, text), audio);
    trim_cache_in(&cache_dir(), MAX_CACHE, MAX_CACHE_BYTES);
}

/// 存一份；第二个键用硬链接指过去（不占第二份空间），硬链接失败再复制
fn write_cache_in(dir: &std::path::Path, key: &str, text_key: &str, audio: &[u8]) {
    if std::fs::create_dir_all(dir).is_err() { return; }
    let ext = audio_ext(audio);
    // 同一个键的旧格式文件先删，免得读到旧的
    for k in [key, text_key] {
        for e in CACHE_EXTS { let _ = std::fs::remove_file(dir.join(format!("{k}.{e}"))); }
    }
    let main = dir.join(format!("{key}.{ext}"));
    if std::fs::write(&main, audio).is_err() { return; }
    if key == text_key { return; }
    let alias = dir.join(format!("{text_key}.{ext}"));
    if std::fs::hard_link(&main, &alias).is_err() { let _ = std::fs::write(&alias, audio); }
}

/// 按修改时间淘汰，直到句数和总大小都在上限内。
/// 同一句的两个硬链接修改时间和大小都一样，按 (修改时间, 大小) 归成一组，整组留或整组删
fn trim_cache_in(dir: &std::path::Path, max_entries: usize, max_bytes: u64) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut groups: std::collections::HashMap<(std::time::SystemTime, u64), Vec<PathBuf>> = std::collections::HashMap::new();
    for e in rd.flatten() {
        let Ok(m) = e.metadata() else { continue };
        if !m.is_file() { continue; }
        let Ok(t) = m.modified() else { continue };
        groups.entry((t, m.len())).or_default().push(e.path());
    }
    let mut groups: Vec<_> = groups.into_iter().collect();
    groups.sort_by_key(|g| std::cmp::Reverse(g.0.0));
    let (mut n, mut bytes, mut full) = (0usize, 0u64, false);
    for ((_, len), paths) in groups {
        if !full && n < max_entries && bytes.saturating_add(len) <= max_bytes {
            n += 1;
            bytes = bytes.saturating_add(len);
        } else {
            full = true;
            for p in paths { let _ = std::fs::remove_file(p); }
        }
    }
}

// ------------------------------------------------------------------ 延迟追踪

/// 每句（按行为层 token）从 speak() 开始计时，HAKIMI_TRACE=1 时各阶段写一行 trace：
/// `tts lat #12 +3ms cache miss` … `tts lat #12 +1840ms first sound`。平时也计时（很便宜），只是不写
mod lat {
    use std::sync::Mutex;
    use std::time::Instant;
    static T0: Mutex<Vec<(u64, Instant)>> = Mutex::new(Vec::new());

    pub fn begin(token: u64) {
        let mut v = T0.lock().unwrap_or_else(|e| e.into_inner());
        v.retain(|(_, t)| t.elapsed().as_secs() < 120);
        v.retain(|(k, _)| *k != token);
        if v.len() >= 64 { v.remove(0); }
        v.push((token, Instant::now()));
    }

    /// 从 speak() 到现在多少毫秒（不认识的 token = None）
    pub fn since(token: u64) -> Option<u128> {
        let v = T0.lock().unwrap_or_else(|e| e.into_inner());
        v.iter().find(|(k, _)| *k == token).map(|(_, t)| t.elapsed().as_millis())
    }

    pub fn mark(token: u64, what: &str) {
        if let Some(ms) = since(token) { crate::core::log::trace(&format!("tts lat #{token} +{ms}ms {what}")); }
    }

    pub fn link(from: u64, to: u64) {
        let mut v = T0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = v.iter_mut().find(|(k, _)| *k == from) { e.0 = to; }
    }

    /// 首音：记一行并结束这句的计时
    pub fn end(token: u64) {
        let mut v = T0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = v.iter().position(|(k, _)| *k == token) {
            let (_, t) = v.remove(i);
            drop(v);
            crate::core::log::trace(&format!("tts lat #{token} +{}ms first sound", t.elapsed().as_millis()));
        }
    }
}

/// 行为层换了 token 去播（语音回答：合成用一个 token，播放用另一个）：把计时挪过去
pub fn lat_link(from: u64, to: u64) { lat::link(from, to); }

/// audio 线程开始出声时调（语音声道）
pub fn lat_sound(token: u64) { lat::end(token); }

/// 错误信息里万一带了 Key（服务端回显请求头之类），换成 ***
fn redact(msg: &str, key: &str) -> String {
    let k = key.trim();
    if k.len() >= 6 { msg.replace(k, "***") } else { msg.to_string() }
}

// ------------------------------------------------------------------ 请求体

fn ref_voice() -> Option<String> {
    // 参考音频：7.4 秒 64kbps mp3（约 58KB，原来是 345KB 的 wav，请求体 ~460KB → ~77KB）。
    // 只在发 clone 请求时临时转 base64，用完即丢
    crate::assets::get("voice/hakimi-voice.mp3")
        .map(|b| format!("data:audio/mpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(b)))
}

fn ref_voice_for(t: &TtsConfig) -> Option<String> {
    if !t.clone_ref.trim().is_empty() {
        let p = std::path::Path::new(t.clone_ref.trim());
        if let Ok(bytes) = std::fs::read(p)
            && !bytes.is_empty()
            && bytes.len() as u64 <= crate::core::skin::MAX_VOICE_BYTES
        {
            let mime = match p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
                "wav" => "audio/wav",
                "ogg" => "audio/ogg",
                "flac" => "audio/flac",
                _ => "audio/mpeg",
            };
            return Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)));
        }
    }
    ref_voice()
}

pub fn build_body(t: &TtsConfig, text: &str, mood: Option<VoiceMood>) -> Value {
    build_body_with(t, text, mood, || ref_voice_for(t))
}

fn build_body_with(t: &TtsConfig, text: &str, mood: Option<VoiceMood>, refv: impl FnOnce() -> Option<String>) -> Value {
    let content = match mood {
        Some(m) if !text.starts_with(['（', '(', '[']) => format!("({}){text}", mood_tag(m)),
        _ => text.to_string(),
    };
    match t.mode.as_str() {
        "preset" => preset_body(t, &content),
        "design" => json!({
            "model": "mimo-v2.5-tts-voicedesign",
            "messages": [{ "role": "user", "content": t.style }, { "role": "assistant", "content": content }],
            "audio": { "format": AUDIO_FORMAT },
        }),
        _ => match refv() {
            Some(r) => json!({
                "model": "mimo-v2.5-tts-voiceclone",
                "messages": [{ "role": "user", "content": "" }, { "role": "assistant", "content": content }],
                "audio": { "format": AUDIO_FORMAT, "voice": r },
            }),
            // 参考音频丢了就退回预置音色
            None => preset_body(t, &content),
        },
    }
}

fn preset_body(t: &TtsConfig, content: &str) -> Value {
    let voice = if t.voice.is_empty() { "冰糖" } else { t.voice.as_str() };
    json!({
        "model": "mimo-v2.5-tts",
        "messages": [{ "role": "user", "content": t.style }, { "role": "assistant", "content": content }],
        "audio": { "format": AUDIO_FORMAT, "voice": voice },
    })
}

// ------------------------------------------------------------------ 调度

struct Job {
    seq: u64,
    token: u64,
    t: TtsConfig,
    text: String,
    mood: Option<VoiceMood>,
}

struct State {
    next_seq: u64,
    /// 最近一次「打断式」说话/停止的序号；正在流式接收的请求若 seq <= abort_seq 则立刻掐断
    abort_seq: u64,
    /// 还没回复的前台请求 (seq, token, tx)；保证每个 token 恰好回一次
    pending: Vec<(u64, u64, EventSender)>,
    /// 等着发的前台请求（只留最新一条）
    fg: Option<Job>,
    cooldown_until: Option<Instant>,
    last_fg: Option<Instant>,
    warm: VecDeque<(String, Option<VoiceMood>)>,
    /// 下一次预热的时间；None = 预热没在跑
    warm_at: Option<Instant>,
    worker: bool,
}

static STATE: Mutex<State> = Mutex::new(State {
    next_seq: 0,
    abort_seq: 0,
    pending: Vec::new(),
    fg: None,
    cooldown_until: None,
    last_fg: None,
    warm: VecDeque::new(),
    warm_at: None,
    worker: false,
});
static WAKE: Condvar = Condvar::new();

fn state() -> std::sync::MutexGuard<'static, State> { STATE.lock().unwrap_or_else(|e| e.into_inner()) }

/// 回复一个前台请求（已经回过的忽略）
fn answer(seq: u64, audio: Option<Vec<u8>>) {
    let hit = {
        let mut st = state();
        st.pending.iter().position(|p| p.0 == seq).map(|i| st.pending.remove(i))
    };
    if let Some((_, token, tx)) = hit {
        tx.send(AppEvent::Speech { token, audio: audio.map(Audio) });
    }
}

fn is_pending(seq: u64) -> bool { state().pending.iter().any(|p| p.0 == seq) }

fn in_cooldown(st: &State) -> bool { st.cooldown_until.is_some_and(|t| Instant::now() < t) }

/// 主动取消当前正在排队或流式接收的前台语音请求（stop_voice / 关掉语音时调）
pub fn cancel_active() {
    let old = {
        let mut st = state();
        st.next_seq += 1;
        st.abort_seq = st.next_seq;
        st.fg = None;
        std::mem::take(&mut st.pending)
    };
    for (_, tok, otx) in old {
        otx.send(AppEvent::Speech { token: tok, audio: None });
    }
}

/// 合成/取缓存，完成后 tx.send(AppEvent::Speech { token, audio })。
/// 根据 config.tts 判断：enabled=false / speak=off / speak=chat 且 kind=Line → audio=None。
/// 新请求来了要掐掉旧的在途请求（旧的回 None）。
pub fn speak(tx: &EventSender, token: u64, text: String, mood: Option<VoiceMood>, kind: SpeakKind) {
    speak_inner(tx, token, text, mood, kind, false);
}

/// 语音回答（ChatVoice）在上一句正在播时预合成下一句：排进 fg 队列，但不掐断正在流式收尾的上一句
pub fn speak_prefetch(tx: &EventSender, token: u64, text: String, mood: Option<VoiceMood>, kind: SpeakKind) {
    speak_inner(tx, token, text, mood, kind, true);
}

fn speak_inner(tx: &EventSender, token: u64, text: String, mood: Option<VoiceMood>, kind: SpeakKind, prefetch: bool) {
    let t = crate::core::skin::effective_tts(crate::core::config::get().tts);
    lat::begin(token);
    let (seq, old) = {
        let mut st = state();
        st.next_seq += 1;
        let seq = st.next_seq;
        if !prefetch {
            st.abort_seq = seq;
        }
        let old = std::mem::take(&mut st.pending);
        st.fg = None;
        st.pending.push((seq, token, tx.clone()));
        (seq, old)
    };
    for (_, tok, otx) in old { otx.send(AppEvent::Speech { token: tok, audio: None }); }

    if !t.enabled || t.speak == "off" || (t.speak == "chat" && kind != SpeakKind::Chat) {
        answer(seq, None);
        return;
    }
    let spawned = std::thread::Builder::new().name("hakimi-tts-lookup".into()).spawn(move || {
        let text = clean(&text);
        if text.is_empty() || !speakable(&text) { return answer(seq, None); }
        if let Some(a) = read_cache(&t, &text, mood) {
            lat::mark(token, "cache hit");
            return answer(seq, Some(a));
        }
        if !online_ok(&t) { return answer(seq, None); }
        let mut st = state();
        if in_cooldown(&st) {
            drop(st);
            crate::core::log::trace("tts skipped: cooling down after 429");
            return answer(seq, None);
        }
        if !st.pending.iter().any(|p| p.0 == seq) { return; }
        lat::mark(token, "cache miss, queued");
        st.fg = Some(Job { seq, token, t, text, mood });
        ensure_worker(&mut st);
        drop(st);
        WAKE.notify_all();
    });
    if spawned.is_err() { answer(seq, None); }
}

/// 插队预合成（快到点的备忘提醒这类马上要用的话）：放到预合成队列最前面，半秒后开始
pub fn prewarm_now(items: Vec<(String, Option<VoiceMood>)>) {
    let mut st = state();
    for it in items.into_iter().rev() {
        if !st.warm.iter().any(|w| *w == it) { st.warm.push_front(it); }
    }
    st.warm.truncate(MAX_PREWARM);
    let soon = Instant::now() + Duration::from_millis(500);
    st.warm_at = Some(st.warm_at.map_or(soon, |a| a.min(soon)));
    ensure_worker(&mut st);
    drop(st);
    WAKE.notify_all();
}

/// 后台慢慢预合成固定台词（最多 300 条），前台说话时让路
pub fn prewarm(items: Vec<(String, Option<VoiceMood>)>) {
    let mut st = state();
    st.warm = items.into_iter().take(MAX_PREWARM).collect();
    if st.warm_at.is_none() { st.warm_at = Some(Instant::now() + Duration::from_secs(15)); }
    ensure_worker(&mut st);
    drop(st);
    WAKE.notify_all();
}

fn ensure_worker(st: &mut State) {
    if st.worker { return; }
    if std::thread::Builder::new().name("hakimi-tts".into()).spawn(worker).is_ok() { st.worker = true; }
}

enum Work { Fg(Job), Warm(TtsConfig, String, Option<VoiceMood>, Option<Instant>) }

fn worker() {
    loop {
        let work = {
            let mut st = state();
            loop {
                if let Some(job) = st.fg.take() {
                    st.last_fg = Some(Instant::now());
                    break Work::Fg(job);
                }
                let now = Instant::now();
                match st.warm_at {
                    Some(at) if now >= at => {
                        // 在锁外读配置，避免持有 STATE 锁期间触发磁盘 I/O
                        drop(st);
                        let cfg = crate::core::config::get();
                        let t = crate::core::skin::effective_tts(cfg.tts);
                        st = state();
                        if let Some(job) = st.fg.take() {
                            st.last_fg = Some(Instant::now());
                            break Work::Fg(job);
                        }
                        // 本机 GPT-SoVITS 合成慢且占本地 GPU/CPU，不做百句后台预热；设了每日粮食上限时也别在后台悄悄扣额度
                        let warm_ok = t.enabled && online_ok(&t) && t.speak != "off" && t.mode != "sovits" && cfg.daily_food_cap == 0;
                        if !warm_ok || st.warm.is_empty() {
                            st.warm_at = None;
                            continue;
                        }
                        let now = Instant::now();
                        // 前台刚念过话（3 秒内）、正在冷却时先让路
                        let mut until = now;
                        if let Some(c) = st.cooldown_until { until = until.max(c); }
                        if let Some(l) = st.last_fg { until = until.max(l + Duration::from_secs(3)); }
                        if until > now {
                            st.warm_at = Some(until + Duration::from_millis(500));
                            continue;
                        }
                        let (text, mood) = st.warm.pop_front().unwrap_or_default();
                        let prev_at = st.warm_at;
                        break Work::Warm(t, text, mood, prev_at);
                    }
                    Some(at) => {
                        st = WAKE.wait_timeout(st, at - now).unwrap_or_else(|e| e.into_inner()).0;
                    }
                    None => {
                        st = WAKE.wait(st).unwrap_or_else(|e| e.into_inner());
                    }
                }
            }
        };
        match work {
            Work::Fg(job) => run_fg(job),
            Work::Warm(t, raw, mood, prev_at) => {
                let text = clean(&raw);
                let next = if !text.is_empty() && speakable(&text) && read_cache(&t, &text, mood).is_none() {
                    let _ = request_once(&t, &text, mood, 25);
                    Duration::from_secs(6)
                } else {
                    Duration::from_millis(50)
                };
                let mut st = state();
                // 如果期间 prewarm_now() 插入了更紧急的时间，别覆盖它
                if st.warm_at == prev_at && st.warm_at.is_some() {
                    st.warm_at = Some(Instant::now() + next);
                }
            }
        }
    }
}

/// clone 一次性请求第一次的超时（秒），按字数给：实测正常时短句 ~2 秒、65 字 ~4–5 秒，
/// 但偶尔会有单个请求卡 20–40 秒（服务端，下一个请求又正常）。与其干等，不如早点断开重发一次（重发给 25 秒）
pub fn first_timeout(text: &str) -> u64 { (6 + text.chars().count() as u64 * 15 / 100).clamp(8, 20) }

/// 现在能不能在线合成：要有办法合成（MiMo 要 Key），且 MiMo 今天没吃到上限（本机 GPT-SoVITS 不受限）
pub fn online_ok(t: &TtsConfig) -> bool { t.can_synth() && (t.mode == "sovits" || !crate::core::usage::over_cap()) }

/// 这个模式的前台台词走不走流式
pub fn streams(t: &TtsConfig) -> bool { matches!(t.mode.as_str(), "preset" | "design") }

fn run_fg(job: Job) {
    let mut result = Err(Fail { retry: false, msg: "superseded".into() });
    let stream = streams(&job.t);
    for attempt in 0..2 {
        if !is_pending(job.seq) { break; }
        lat::mark(job.token, if attempt == 0 { "send" } else { "retry" });
        result = if stream {
            request_stream(&job, attempt)
        } else {
            let secs = if job.t.mode == "sovits" { SOVITS_TIMEOUT } else if attempt == 0 { first_timeout(&job.text) } else { 25 };
            request_once_lat(&job.t, &job.text, job.mood, secs, job.token)
        };
        match &result {
            Err(f) if f.retry => continue,
            _ => break,
        }
    }
    {
        let mut st = state();
        st.last_fg = Some(Instant::now());
    }
    match result {
        Ok(Some(wav)) => {
            crate::core::log::trace(&format!("voice online {} ({} B)", job.text, wav.len()));
            answer(job.seq, Some(wav))
        }
        // 流式：已经在首块时回复过了
        Ok(None) => crate::core::log::trace(&format!("voice stream done {}", job.text)),
        Err(f) => {
            if is_pending(job.seq) && f.msg != "superseded" {
                crate::core::log::log(&format!("tts error {}（这句不出声，只显示文字）", redact(&f.msg, &job.t.api_key)));
            }
            answer(job.seq, None);
        }
    }
}

fn request_once_lat(t: &TtsConfig, text: &str, mood: Option<VoiceMood>, timeout_secs: u64, token: u64) -> Result<Option<Vec<u8>>, Fail> {
    let r = request_once_at(t, text, mood, timeout_secs, Some(token));
    r.map(Some)
}

struct StreamOutcome {
    pcm: Vec<u8>,
    had_sink: bool,
    chunks: u32,
    err: Option<String>,
    retryable_err: bool,
    superseded: bool,
}

/// 逐行消费 SSE 音频流；每行前检查 `is_superseded()`，被新台词顶掉立即停止读取并调用 `sink.finish()`
fn drain_sse_stream<R: BufRead>(
    reader: R,
    mut is_superseded: impl FnMut() -> bool,
    mut still_pending: impl FnMut() -> bool,
    mut on_first: impl FnMut(Vec<u8>, usize),
) -> StreamOutcome {
    let mut pcm: Vec<u8> = Vec::new();
    let mut sink: Option<std::sync::Arc<super::audio::PcmStream>> = None;
    let mut chunks = 0u32;
    let mut err: Option<String> = None;
    let mut retryable_err = false;
    let mut superseded = false;
    for line in reader.lines() {
        // 已被新台词顶掉：立刻退出循环丢弃 reader 断开连接，不阻塞单 worker
        if is_superseded() {
            superseded = true;
            break;
        }
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                err = Some(e.to_string());
                retryable_err = true;
                break;
            }
        };
        match sse_line(&line) {
            Sse::Audio(b) => {
                chunks += 1;
                if sink.is_none() {
                    // 首块之前已经被新台词顶掉：断开连接，不要了
                    if !still_pending() {
                        superseded = true;
                        break;
                    }
                    let (handle, w) = super::audio::stream_open(STREAM_RATE);
                    let len = b.len();
                    w.push_pcm16(&b);
                    sink = Some(w);
                    on_first(handle.0, len);
                } else if let Some(w) = &sink {
                    w.push_pcm16(&b);
                }
                pcm.extend_from_slice(&b);
            }
            Sse::Done => break,
            Sse::Error(m) => {
                err = Some(m);
                break;
            }
            Sse::Other => {}
        }
    }
    if let Some(w) = &sink { w.finish(); }
    StreamOutcome { pcm, had_sink: sink.is_some(), chunks, err, retryable_err, superseded }
}

/// 流式合成一句（preset/design）：收到第一块音频就回复行为层（句柄），之后边收边往缓冲里写；
/// 收完存 wav 缓存。首块之前被顶掉就直接断开连接（真的掐断，不像一次性请求只能等它跑完）
fn request_stream(job: &Job, attempt: u32) -> Result<Option<Vec<u8>>, Fail> {
    let t = &job.t;
    let key = t.api_key.trim();
    let mut body = build_body(t, &job.text, job.mood);
    body["audio"]["format"] = json!("pcm16");
    body["stream"] = json!(true);
    let (status, reader) = super::net::post_json_stream(API, &[("api-key", key)], &body, STREAM_TIMEOUT).map_err(|e| Fail {
        retry: true,
        msg: if matches!(e, ureq::Error::Timeout(_)) { "timeout".into() } else { e.to_string() },
    })?;
    lat::mark(job.token, &format!("headers (HTTP {status})"));
    if status == 429 {
        state().cooldown_until = Some(Instant::now() + COOLDOWN);
        return Err(Fail { retry: false, msg: "HTTP 429（限流，冷却 10 秒）".into() });
    }
    if !(200..300).contains(&status) {
        let mut text = String::new();
        let _ = std::io::Read::read_to_string(&mut std::io::Read::take(reader, 4096), &mut text);
        let snippet: String = text.chars().take(200).collect();
        return Err(Fail { retry: status >= 500, msg: format!("HTTP {status}: {snippet}") });
    }
    let out = drain_sse_stream(
        std::io::BufReader::new(reader),
        || state().abort_seq >= job.seq,
        || is_pending(job.seq),
        |handle, len| {
            lat::mark(job.token, &format!("first audio chunk ({len} B)"));
            answer(job.seq, Some(handle));
        },
    );
    if out.superseded {
        return if out.had_sink { Ok(None) } else { Err(Fail { retry: false, msg: "superseded".into() }) };
    }
    let (pcm, chunks) = (out.pcm, out.chunks);
    match (out.had_sink, out.err) {
        (true, None) => {
            lat::mark(job.token, &format!("stream done ({chunks} chunks, {} B pcm)", pcm.len()));
            let wav = pcm16_wav(STREAM_RATE, &pcm);
            write_cache(t, &job.text, job.mood, &wav);
            crate::core::usage::add_tts_chars(job.text.chars().count() as u64);
            Ok(None)
        }
        // 播到一半断了：已经收到的照样放完，不进缓存
        (true, Some(e)) => {
            crate::core::usage::add_tts_chars(job.text.chars().count() as u64);
            crate::core::log::log(&format!("tts stream cut off after {chunks} chunks: {}", redact(&e, &t.api_key)));
            Ok(None)
        }
        (false, e) => Err(Fail { retry: attempt == 0 && out.retryable_err, msg: e.unwrap_or_else(|| "no audio in stream".into()) }),
    }
}

/// SSE 的一行
#[derive(Debug, PartialEq)]
enum Sse {
    Audio(Vec<u8>),
    Done,
    Error(String),
    Other,
}

fn sse_line(line: &str) -> Sse {
    let Some(p) = line.trim().strip_prefix("data:") else { return Sse::Other };
    let p = p.trim();
    if p == "[DONE]" { return Sse::Done; }
    let Ok(v) = serde_json::from_str::<Value>(p) else { return Sse::Other };
    if let Some(e) = v.get("error") {
        let m = e.get("message").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| e.to_string());
        return Sse::Error(m.chars().take(200).collect());
    }
    let data = v.pointer("/choices/0/delta/audio/data").and_then(Value::as_str).unwrap_or("");
    if data.is_empty() { return Sse::Other; }
    match base64::engine::general_purpose::STANDARD.decode(data.trim()) {
        Ok(b) if !b.is_empty() => Sse::Audio(b),
        Ok(_) => Sse::Other,
        Err(e) => Sse::Error(format!("bad base64: {e}")),
    }
}

/// 16 位单声道 PCM → wav 文件字节
pub fn pcm16_wav(rate: u32, pcm: &[u8]) -> Vec<u8> {
    let n = (pcm.len() & !1) as u32;
    let mut v = Vec::with_capacity(44 + n as usize);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + n).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * 2).to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&n.to_le_bytes());
    v.extend_from_slice(&pcm[..n as usize]);
    v
}

struct Fail {
    retry: bool,
    msg: String,
}

/// 发一次合成请求；成功写缓存并返回音频字节（mp3）
fn request_once(t: &TtsConfig, text: &str, mood: Option<VoiceMood>, timeout_secs: u64) -> Result<Vec<u8>, Fail> {
    request_once_at(t, text, mood, timeout_secs, None)
}

fn request_once_at(t: &TtsConfig, text: &str, mood: Option<VoiceMood>, timeout_secs: u64, token: Option<u64>) -> Result<Vec<u8>, Fail> {
    if t.mode == "sovits" {
        let wav = sovits_request(t, text, timeout_secs)?;
        if let Some(tok) = token { lat::mark(tok, &format!("sovits ({} B)", wav.len())); }
        write_cache(t, text, mood, &wav);
        return Ok(wav);
    }
    let body = build_body(t, text, mood);
    let r = super::net::post_json(API, &[("api-key", t.api_key.trim())], &body, timeout_secs);
    drop(body);
    if let Some(tok) = token { lat::mark(tok, "response read"); }
    let (status, text_body) = r.map_err(|e| Fail {
        retry: true,
        msg: if matches!(e, ureq::Error::Timeout(_)) { "timeout".into() } else { e.to_string() },
    })?;
    if status == 429 {
        state().cooldown_until = Some(Instant::now() + COOLDOWN);
        return Err(Fail { retry: false, msg: "HTTP 429（限流，冷却 10 秒）".into() });
    }
    if !(200..300).contains(&status) {
        let snippet: String = text_body.chars().take(200).collect();
        return Err(Fail { retry: status >= 500, msg: format!("HTTP {status}: {snippet}") });
    }
    let json: Value = serde_json::from_str(&text_body).map_err(|e| Fail { retry: true, msg: e.to_string() })?;
    drop(text_body);
    let b64 = json
        .pointer("/choices/0/message/audio/data")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Fail { retry: false, msg: "no audio in response".into() })?;
    let wav = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|e| Fail { retry: false, msg: format!("bad base64: {e}") })?;
    if let Some(tok) = token { lat::mark(tok, &format!("decoded ({} B)", wav.len())); }
    write_cache(t, text, mood, &wav);
    crate::core::usage::add_tts_chars(text.chars().count() as u64);
    Ok(wav)
}

// ------------------------------------------------------------------ 本机 GPT-SoVITS（api_v2：POST /tts，直接回 wav）

/// 本机合成慢（CPU 上一句可能十几秒），超时给宽一点
const SOVITS_TIMEOUT: u64 = 60;

/// 参考音频路径：用户填了就用；没填就把内置的哈基米录音导出到数据目录（只导一次）
fn sovits_ref_path(t: &TtsConfig) -> Option<String> {
    let r = t.sovits_ref.trim();
    if !r.is_empty() { return Some(r.to_string()); }
    let path = crate::core::paths::data_dir().join("sovits-ref.mp3");
    let want = crate::assets::get("voice/hakimi-voice.mp3")?;
    let same = std::fs::metadata(&path).map(|m| m.len() == want.len() as u64).unwrap_or(false);
    if !same && std::fs::write(&path, want).is_err() { return None; }
    Some(path.to_string_lossy().into_owned())
}

/// 内置参考音频（assets/voice/hakimi-voice.mp3）对应的准确台词：GPT-SoVITS v3/v4 要求非空 prompt_text
const SOVITS_DEFAULT_PROMPT: &str = "主人，你回来啦！哈基米等你好久了喵，今天也要好好吃饭哦！";

/// 请求体（不带心情标签：GPT-SoVITS 会把「(开心)」当字念出来）
pub fn sovits_body(t: &TtsConfig, text: &str, ref_path: &str) -> Value {
    let prompt = t.sovits_prompt.trim();
    let prompt_text = if prompt.is_empty() && t.sovits_ref.trim().is_empty() { SOVITS_DEFAULT_PROMPT } else { prompt };
    json!({
        "text": text,
        "text_lang": "zh",
        "ref_audio_path": ref_path,
        "prompt_text": prompt_text,
        "prompt_lang": "zh",
        "media_type": "wav",
        "streaming_mode": false,
        "text_split_method": "cut5",
    })
}

fn sovits_url(t: &TtsConfig) -> String {
    let base = t.sovits_url.trim().trim_end_matches('/');
    let base = if base.is_empty() {
        "http://127.0.0.1:9880".to_string()
    } else if base.contains("://") {
        base.to_string()
    } else {
        format!("http://{base}")
    };
    if base.ends_with("/tts") { base } else { format!("{base}/tts") }
}

fn sovits_request(t: &TtsConfig, text: &str, timeout_secs: u64) -> Result<Vec<u8>, Fail> {
    let refp = sovits_ref_path(t).ok_or_else(|| Fail { retry: false, msg: "sovits: no reference audio".into() })?;
    let body = sovits_body(t, text, &refp);
    let (status, bytes) = super::net::post_json_bytes(&sovits_url(t), &body, timeout_secs).map_err(|e| Fail {
        retry: false,
        msg: if matches!(e, ureq::Error::Timeout(_)) { "timeout".into() } else { format!("sovits: {e}") },
    })?;
    if !(200..300).contains(&status) {
        let snippet: String = String::from_utf8_lossy(&bytes).chars().take(200).collect();
        return Err(Fail { retry: false, msg: format!("HTTP {status}: {snippet}") });
    }
    if bytes.len() < 64 || !super::audio::can_decode(&bytes) {
        return Err(Fail { retry: false, msg: "sovits: response is not audio".into() });
    }
    Ok(bytes)
}

// ------------------------------------------------------------------ 设置窗口「试听」

/// 试听用的一句（内置录音里也有，没填 Key 时 clone 模式直接放内置的）
pub const SAMPLE: &str = "喵？找哈基米有事吗～";

/// 用设置窗口里还没保存的语音设置合成一句（阻塞，后台线程调）。失败返回中文原因（不含 Key）
pub fn synth_test(t: &TtsConfig, text: &str) -> Result<Vec<u8>, String> {
    if t.mode == "sovits" {
        return sovits_request(t, &clean(text), SOVITS_TIMEOUT).map_err(|f| {
            if f.msg == "timeout" {
                "本机 GPT-SoVITS 太慢没回话（超过 60 秒）".into()
            } else if f.msg.starts_with("HTTP ") {
                let s: String = f.msg.chars().take(80).collect();
                format!("GPT-SoVITS 合成失败：{s}")
            } else if f.msg.contains("not audio") {
                "地址上的服务回的不是声音：确认开的是 GPT-SoVITS 的 api_v2.py".into()
            } else {
                "连不上本机 GPT-SoVITS：先运行 python api_v2.py（默认端口 9880）".into()
            }
        });
    }
    if t.api_key.trim().is_empty() { return Err("还没填 MiMo Key".into()); }
    let mut t = t.clone();
    t.api_key = t.api_key.trim().to_string();
    request_once(&t, &clean(text), None, 25).map_err(|f| {
        let m = redact(&f.msg, &t.api_key);
        if m.contains("HTTP 401") || m.contains("HTTP 403") {
            "MiMo Key 不对或已失效".into()
        } else if m.contains("429") {
            "请求太频繁（限流），过 10 秒再试".into()
        } else if m == "timeout" {
            "连接超时，检查一下网络".into()
        } else if m.starts_with("HTTP ") {
            let s: String = m.chars().take(80).collect();
            format!("合成失败：{s}")
        } else {
            "连不上 MiMo 服务器，检查一下网络".into()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_text() {
        assert_eq!(clean("  你好😺～～~喵❤️  "), "你好～喵");
        assert_eq!(clean("a~~b"), "a～b");
        assert_eq!(clean(&"喵".repeat(400)).chars().count(), 300);
        assert!(!speakable("～！？"));
        assert!(speakable("喵"));
        assert!(speakable("ok"));
    }

    #[test]
    fn keys_match_electron() {
        let t = TtsConfig::default();
        // 与 Node crypto.createHash('sha1') 结果一致
        assert_eq!(cache_key(&t, "你好", Some(VoiceMood::Happy)), "829763512f6496c609ff0635be51f93d6cb31730");
        assert_eq!(text_key(&t, "你好"), "T783e6dcc5275504b2cdd82cf0981b0805e4c67f9");
        assert_eq!(&sha1_hex("喵？找哈基米有事吗～")[..16], "2b60dc85bb8a2964");
    }

    #[test]
    fn sovits_request_shape() {
        let mut t = TtsConfig { mode: "sovits".into(), ..TtsConfig::default() };
        assert!(t.can_synth());
        assert!(bundled(&t, SAMPLE).is_some(), "本机声线默认用内置录音");
        let b = sovits_body(&t, "你好呀", "C:/x/ref.mp3");
        assert_eq!(b["text"], "你好呀");
        assert_eq!(b["ref_audio_path"], "C:/x/ref.mp3");
        assert!(!b["prompt_text"].as_str().unwrap().is_empty(), "默认参考音频自动附带台词文本以兼容 v3/v4");
        assert_eq!(b["media_type"], "wav");
        assert_eq!(sovits_url(&t), "http://127.0.0.1:9880/tts");
        t.sovits_url = "127.0.0.1:9880".into();
        assert_eq!(sovits_url(&t), "http://127.0.0.1:9880/tts");
        t.sovits_url = "http://192.168.1.5:9880/tts/".into();
        assert_eq!(sovits_url(&t), "http://192.168.1.5:9880/tts");
        // 自定义 sovitsRef 不再强走内置哈基米录音，且缓存键与默认隔离
        let k_default = cache_key(&t, "你好", None);
        t.sovits_ref = "D:/custom/other.wav".into();
        assert!(bundled(&t, SAMPLE).is_none());
        assert_ne!(cache_key(&t, "你好", None), k_default);
        t.mode = "preset".into();
        assert!(!t.can_synth());
    }

    /// 假的本机 GPT-SoVITS：回一段 wav，走真网络
    #[test]
    fn sovits_end_to_end() {
        use std::io::Write;
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let wav = pcm16_wav(16000, &vec![0u8; 3200]);
        let body = wav.clone();
        std::thread::spawn(move || {
            let Ok((mut s, _)) = l.accept() else { return };
            let _ = super::super::net::read_http_request(&mut s);
            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = s.write_all(&body);
        });
        let t = TtsConfig { mode: "sovits".into(), sovits_url: format!("http://127.0.0.1:{port}"), sovits_ref: "C:/ref.mp3".into(), ..TtsConfig::default() };
        assert!(sovits_request(&t, "喵", 10).ok() == Some(wav));
        // 没开服务：中文提示
        let t = TtsConfig { mode: "sovits".into(), sovits_url: "http://127.0.0.1:9".into(), sovits_ref: "C:/ref.mp3".into(), ..TtsConfig::default() };
        assert!(synth_test(&t, "喵").unwrap_err().contains("api_v2"));
    }

    #[test]
    fn bundled_lines_present() {
        let t = TtsConfig::default();
        let lines: Value = serde_json::from_slice(crate::assets::get("voice/lines.json").unwrap()).unwrap();
        let obj = lines.as_object().unwrap();
        let hits = obj.keys().filter(|k| bundled(&t, &clean(k)).is_some()).count();
        assert_eq!(hits, obj.len(), "every lines.json entry must have a matching .ogg");
        assert!(hits >= 370, "only {hits} bundled lines");
        // 抽样校验新增台词也能正常解码
        for sample in [
            "诶嘿～被哈基米电到了没有喵？",
            "吸溜……突然好想吃烤小鱼干喵",
            "好耶——！给主人举爪爪打 call 喵！",
            "小鱼干！哈基米来啦——",
            "逗猫棒！哈基米要抓住它喵！",
        ] {
            let bytes = bundled(&t, &clean(sample)).expect(sample);
            assert!(super::super::audio::can_decode(bytes), "cannot decode {sample}");
        }
        let mut preset = t.clone();
        preset.mode = "preset".into();
        assert!(bundled(&preset, "喵？找哈基米有事吗～").is_none());
    }

    #[test]
    fn bodies() {
        let mut t = TtsConfig::default();
        let b = build_body_with(&t, "你好", Some(VoiceMood::Happy), || Some("data:x".into()));
        assert_eq!(b["model"], "mimo-v2.5-tts-voiceclone");
        assert_eq!(b["messages"][1]["content"], "(开心 俏皮)你好");
        assert_eq!(b["audio"]["voice"], "data:x");
        let b = build_body_with(&t, "（小声）你好", Some(VoiceMood::Sad), || None);
        assert_eq!(b["model"], "mimo-v2.5-tts");
        assert_eq!(b["messages"][1]["content"], "（小声）你好");
        t.mode = "design".into();
        let b = build_body_with(&t, "你好", None, || None);
        assert_eq!(b["model"], "mimo-v2.5-tts-voicedesign");
        assert_eq!(b["messages"][0]["content"], t.style.as_str());
        assert!(b["audio"].get("voice").is_none());
        assert!(ref_voice().is_some_and(|s| s.starts_with("data:audio/mpeg;base64,") && s.len() < 100_000));
    }
}

#[cfg(test)]
mod stream_tests {
    use super::*;

    #[test]
    fn parse_sse_lines() {
        let b64 = base64::engine::general_purpose::STANDARD.encode([1u8, 0, 2, 0]);
        let l = format!(r#"data: {{"choices":[{{"delta":{{"audio":{{"data":"{b64}"}}}}}}]}}"#);
        assert_eq!(sse_line(&l), Sse::Audio(vec![1, 0, 2, 0]));
        assert_eq!(sse_line("data: [DONE]"), Sse::Done);
        assert_eq!(sse_line(""), Sse::Other);
        assert_eq!(sse_line(": keep-alive"), Sse::Other);
        assert_eq!(sse_line(r#"data: {"choices":[{"delta":{"role":"assistant"}}]}"#), Sse::Other);
        assert_eq!(sse_line(r#"data: {"error":{"message":"quota"}}"#), Sse::Error("quota".into()));
        assert!(matches!(sse_line(r#"data: {"choices":[{"delta":{"audio":{"data":"@@@"}}}]}"#), Sse::Error(_)));
    }

    #[test]
    fn wav_from_pcm_decodes() {
        let pcm: Vec<u8> = (0..4800u32).flat_map(|i| (((i % 100) as i16 - 50) * 300).to_le_bytes()).collect();
        let wav = pcm16_wav(STREAM_RATE, &pcm);
        assert_eq!(wav.len(), 44 + pcm.len());
        assert_eq!(audio_ext(&wav), "wav");
        assert!(crate::services::audio::can_decode(&wav));
        // 奇数字节截掉半个样本
        assert_eq!(pcm16_wav(STREAM_RATE, &[1, 2, 3]).len(), 46);
    }

    #[test]
    fn stream_only_for_preset_and_design() {
        let mut t = TtsConfig::default();
        assert!(!streams(&t));
        t.mode = "preset".into();
        assert!(streams(&t));
        t.mode = "design".into();
        assert!(streams(&t));
    }

    #[test]
    fn first_timeout_by_length() {
        assert_eq!(first_timeout("喵～"), 8);
        assert_eq!(first_timeout(&"喵".repeat(30)), 10);
        assert_eq!(first_timeout(&"喵".repeat(65)), 15);
        assert_eq!(first_timeout(&"喵".repeat(300)), 20);
    }

    #[test]
    fn redact_key() {
        assert_eq!(redact("bad key sk-abcdef123 here", "sk-abcdef123"), "bad key *** here");
        assert_eq!(redact("x", ""), "x");
    }

    #[test]
    fn lat_marks() {
        lat::begin(9_000_001);
        assert!(lat::since(9_000_001).is_some());
        lat::link(9_000_001, 9_000_003);
        assert!(lat::since(9_000_001).is_none());
        lat::end(9_000_003);
        assert!(lat::since(9_000_003).is_none());
        lat::begin(9_000_001);
        lat::end(9_000_001);
        assert!(lat::since(9_000_001).is_none());
        assert!(lat::since(9_000_002).is_none());
    }

    #[test]
    fn stream_cancel_stops_reading_and_marks_superseded() {
        let chunk_b64 = base64::engine::general_purpose::STANDARD.encode([0x01u8, 0x02, 0x03, 0x04]);
        let sse = format!(
            "data: {{\"choices\":[{{\"delta\":{{\"audio\":{{\"data\":\"{chunk_b64}\"}}}}}}]}}\n\
             data: {{\"choices\":[{{\"delta\":{{\"audio\":{{\"data\":\"{chunk_b64}\"}}}}}}]}}\n\
             data: {{\"choices\":[{{\"delta\":{{\"audio\":{{\"data\":\"{chunk_b64}\"}}}}}}]}}\n\
             data: [DONE]\n"
        );
        // 1. 首块发出后被新台词顶掉：立刻中止后续块读取，不再消费剩余 SSE 行
        let mut seen_lines = 0usize;
        let mut first_called = 0usize;
        let out = drain_sse_stream(
            std::io::Cursor::new(sse.as_bytes()),
            || {
                seen_lines += 1;
                seen_lines >= 2 // 读完第 1 块后下一行即检测到被顶掉
            },
            || true,
            |_, _| first_called += 1,
        );
        assert!(out.superseded);
        assert!(out.had_sink);
        assert_eq!(out.chunks, 1);
        assert_eq!(first_called, 1);
        assert_eq!(out.pcm.len(), 4);

        // 2. 首块之前已被顶掉：不打开音频流也不回调 on_first
        let mut first_called2 = 0usize;
        let out2 = drain_sse_stream(
            std::io::Cursor::new(sse.as_bytes()),
            || true,
            || false,
            |_, _| first_called2 += 1,
        );
        assert!(out2.superseded);
        assert!(!out2.had_sink);
        assert_eq!(out2.chunks, 0);
        assert_eq!(first_called2, 0);

        // 3. SSE 返回 error 事件：retryable_err 为 false，避免对业务逻辑错误无意义重试
        let err_sse = "data: {\"error\":{\"message\":\"bad voice\"}}\n";
        let out3 = drain_sse_stream(
            std::io::Cursor::new(err_sse.as_bytes()),
            || false,
            || true,
            |_, _| {},
        );
        assert!(!out3.superseded);
        assert!(!out3.retryable_err);
        assert_eq!(out3.err.as_deref(), Some("bad voice"));
    }
}

/// 真联网测延迟：`HAKIMI_LIVE=1 cargo test --release -- --ignored live_latency --nocapture`。
/// 只读 config.json（不走 config::get，那个会写回文件），不打印 Key；合成结果会进 tts-cache（文字带时间戳，不会和平时的台词撞）。
/// 声卡真开，但音量 0；「首音」= play 之后第一次量到非静音样本
#[cfg(test)]
mod live_tests {
    use super::*;
    use crate::types::{Audio, EventSender};
    use std::sync::mpsc::{Receiver, channel};

    fn cfg() -> Option<TtsConfig> {
        let text = std::fs::read_to_string(crate::core::paths::config_file()).ok()?;
        let t = crate::core::config::parse(&text).tts;
        (!t.api_key.trim().is_empty()).then_some(t)
    }

    fn first_sound(token: u64, a: Vec<u8>) -> u128 {
        // 先等上一句停干净（电平归零），否则量到的是上一句的尾巴
        crate::services::audio::stop();
        let w = Instant::now();
        while (crate::services::audio::level() > 0.0 || crate::services::audio::is_playing()) && w.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let t0 = Instant::now();
        crate::services::audio::play(token, Audio(a), 0.0);
        while t0.elapsed() < Duration::from_secs(15) {
            if crate::services::audio::level() > 0.0005 { return t0.elapsed().as_millis(); }
            std::thread::sleep(Duration::from_millis(1));
        }
        u128::MAX
    }

    fn nonce() -> String {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() % 1000).unwrap_or(0);
        format!("第{n}次")
    }

    /// 走真的前台调度（run_fg）：返回 (收到可播音频的耗时, 首音耗时, 合成整句完成的耗时)
    fn fg(t: &TtsConfig, text: &str, token: u64, rx: &Receiver<AppEvent>, tx: &EventSender) -> (u128, u128, u128) {
        let seq = 1_000_000 + token;
        state().pending.push((seq, token, tx.clone()));
        let job = Job { seq, token, t: t.clone(), text: text.to_string(), mood: Some(VoiceMood::Happy) };
        let t0 = Instant::now();
        let h = std::thread::spawn(move || run_fg(job));
        let audio = loop {
            match rx.recv_timeout(Duration::from_secs(40)) {
                Ok(AppEvent::Speech { token: k, audio }) if k == token => break audio,
                Ok(_) => continue,
                Err(_) => break None,
            }
        };
        let got = t0.elapsed().as_millis();
        let Some(a) = audio else { return (got, u128::MAX, t0.elapsed().as_millis()) };
        let fs = first_sound(token, a.0);
        let _ = h.join();
        (got, got + fs, t0.elapsed().as_millis())
    }

    #[test]
    #[ignore]
    fn live_latency() {
        if std::env::var_os("HAKIMI_LIVE").is_none() { return; }
        let Some(base) = cfg() else { return println!("no MiMo key, skipped") };
        let (etx, rx) = channel();
        let etx = std::sync::Mutex::new(etx);
        let tx = EventSender::new(move |ev| { let _ = etx.lock().unwrap().send(ev); });
        crate::services::audio::init(tx.clone());
        let reply = "今天天气很好喵，阳光暖洋洋的，晒得哈基米都想打滚了！我们一起出去散散步吧，顺便买点小鱼干～记得带伞哦，下午可能会下雨，别淋湿了喵。";
        let mut tok = 100u64;
        let mut next = || { tok += 1; tok };

        // 1. 缓存命中（内置台词，clone）
        let mut clone = base.clone();
        clone.mode = "clone".into();
        for _ in 0..3 {
            let t0 = Instant::now();
            let a = read_cache(&clone, SAMPLE, None).expect("bundled line");
            let read = t0.elapsed().as_millis();
            let fs = first_sound(next(), a);
            println!("cache hit: read {read}ms, first sound {}ms", read + fs);
            std::thread::sleep(Duration::from_millis(2500));
        }
        // 2. clone：整段回复一次合成（改之前语音回答的做法） vs 第一段（改之后）
        // = behavior::chatvoice::split_sentences(reply)[0]
        let first = "今天天气很好喵，阳光暖洋洋的，晒得哈基米都想打滚了！";
        for _ in 0..3 {
            let (got, fs, _) = fg(&clone, &format!("{}{reply}", nonce()), next(), &rx, &tx);
            println!("clone whole reply ({} chars): audio {got}ms, first sound {fs}ms", reply.chars().count());
            let (got, fs, _) = fg(&clone, &format!("{}{first}", nonce()), next(), &rx, &tx);
            println!("clone first chunk ({} chars): audio {got}ms, first sound {fs}ms", first.chars().count() + 4);
        }
        // 3. preset：一次性 mp3（改之前） vs SSE 流式（改之后）
        let mut preset = base.clone();
        preset.mode = "preset".into();
        for text in ["喵～今天也要开开心心的哦！", reply] {
            for _ in 0..2 {
                let k = next();
                let t0 = Instant::now();
                let a = request_once(&preset, &format!("{}{text}", nonce()), Some(VoiceMood::Happy), 25).ok();
                let got = t0.elapsed().as_millis();
                let fs = a.map(|a| got + first_sound(k, a)).unwrap_or(u128::MAX);
                println!("preset mp3 one-shot ({} chars): audio {got}ms, first sound {fs}ms", text.chars().count());
                let (got, fs, all) = fg(&preset, &format!("{}{text}", nonce()), next(), &rx, &tx);
                println!("preset pcm16 stream ({} chars): first chunk {got}ms, first sound {fs}ms, all received {all}ms", text.chars().count());
            }
        }
        crate::services::audio::stop();
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    /// 0.1 秒 24kHz 单声道静音 wav（MiMo 默认返回的格式）
    pub fn tiny_wav() -> Vec<u8> {
        let n: u32 = 2400;
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + n * 2).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&24_000u32.to_le_bytes());
        v.extend_from_slice(&48_000u32.to_le_bytes());
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(n * 2).to_le_bytes());
        v.resize(v.len() + n as usize * 2, 0);
        v
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hk-tts-cache-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn one_copy_two_keys() {
        let d = tmp("dedupe");
        let wav = tests_support::tiny_wav();
        write_cache_in(&d, "k1", "Tk1", &wav);
        assert_eq!(std::fs::read(d.join("k1.wav")).unwrap(), wav);
        assert_eq!(std::fs::read(d.join("Tk1.wav")).unwrap(), wav);
        // mp3 内容存成 .mp3，并替掉同键的旧 wav
        let mp3 = crate::assets::get("voice/hakimi-voice.mp3").unwrap();
        assert_eq!(audio_ext(mp3), "mp3");
        write_cache_in(&d, "k1", "Tk1", mp3);
        assert!(!d.join("k1.wav").exists() && !d.join("Tk1.wav").exists());
        assert_eq!(std::fs::read(d.join("Tk1.mp3")).unwrap(), mp3);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn trim_by_count_and_bytes() {
        let d = tmp("trim");
        for i in 0..6 {
            write_cache_in(&d, &format!("k{i}"), &format!("T{i}"), &vec![i as u8; 1000 + i]);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // 12 个文件、6 份内容；按大小只留得下最新的 3 句（硬链接不重复计）
        trim_cache_in(&d, 100, 3100);
        let left: Vec<_> = std::fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name().into_string().unwrap()).collect();
        assert_eq!(left.len(), 6, "{left:?}");
        assert!(left.iter().any(|f| f == "T3.wav") && !left.iter().any(|f| f.starts_with("k2") || f.starts_with("T2")));
        assert!(left.iter().any(|f| f.starts_with("k5")));
        trim_cache_in(&d, 1, u64::MAX);
        // 按句数：留最新一句的两个键
        assert!(d.join("k5.wav").exists() && d.join("T5.wav").exists());
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 2);
        let _ = std::fs::remove_dir_all(&d);
    }
}
