//! 语音：移植 Electron 版 src/main/tts.js
//! - 小米 MiMo TTS（OpenAI 兼容 /v1/chat/completions，返回 base64 wav），三种模式 clone / preset / design
//! - 内置台词：assets/voice/lines/<sha1(text) 前16位>.ogg（只在 clone 模式用，crate::assets::get 读）
//! - tts-cache：每句存两份（台词+情绪、只按台词），最多 800 个文件，按修改时间淘汰
//! - 同一时间只发一个请求；新台词来了旧的立刻回 None；429 冷却 10 秒；5xx/网络错误重试一次
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
use std::path::PathBuf;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

pub const API: &str = "https://api.xiaomimimo.com/v1/chat/completions";
/// 每句存两份：按「台词+情绪」和只按台词（换了情绪也能秒播）
pub const MAX_CACHE: usize = 800;
/// 预热最多多少条（main.js 里 items.slice(0, 300)）
pub const MAX_PREWARM: usize = 300;
const COOLDOWN: Duration = Duration::from_secs(10);

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

/// 原版 cacheKey：sha1([mode, voice, mode==clone ? '' : style, mood, text].join('|'))
pub fn cache_key(t: &TtsConfig, text: &str, mood: Option<VoiceMood>) -> String {
    let style = if t.mode == "clone" { "" } else { t.style.as_str() };
    sha1_hex(&[t.mode.as_str(), t.voice.as_str(), style, mood_key(mood), text].join("|"))
}

/// 原版 textKey：'T' + sha1([mode, voice, text].join('|'))
pub fn text_key(t: &TtsConfig, text: &str) -> String {
    format!("T{}", sha1_hex(&[t.mode.as_str(), t.voice.as_str(), text].join("|")))
}

/// 内置台词能不能播：它们是 Ogg **Opus**，而 rodio 0.23 / symphonia 0.5 只有 Vorbis 解码器。
/// 启动后第一次用到时拿一个文件试解一次；不能播就跳过内置台词，走 tts-cache / 在线合成，
/// 免得命中内置台词却放不出声。等集成方加上 Opus 解码（或把台词转成 Vorbis）后自动生效。
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
        if !ok && !cfg!(test) { crate::core::log::log("bundled voice lines not decodable (Ogg Opus), using tts-cache / MiMo"); }
        ok
    })
}

/// 内置台词（哈基米默认声线，只在 clone 模式用）
pub fn bundled(t: &TtsConfig, text: &str) -> Option<&'static [u8]> {
    if t.mode != "clone" { return None; }
    crate::assets::get(&format!("voice/lines/{}.ogg", &sha1_hex(text)[..16]))
}

fn cache_dir() -> PathBuf { crate::core::paths::tts_cache_dir() }

/// 内置 → 缓存（命中缓存会更新修改时间，LRU）
pub fn read_cache(t: &TtsConfig, text: &str, mood: Option<VoiceMood>) -> Option<Vec<u8>> {
    // 角色包的 voiceDir（文件名规则同内置台词）优先；放不出来的格式跳过
    if let Some(b) = crate::core::skin::voice_line(text).filter(|b| super::audio::can_decode(b)) { return Some(b); }
    if let Some(b) = bundled(t, text).filter(|_| bundled_playable()) { return Some(b.to_vec()); }
    for k in [cache_key(t, text, mood), text_key(t, text)] {
        let file = cache_dir().join(format!("{k}.wav"));
        if let Ok(buf) = std::fs::read(&file) {
            if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&file) {
                let _ = f.set_modified(std::time::SystemTime::now());
            }
            return Some(buf);
        }
    }
    None
}

fn write_cache(t: &TtsConfig, text: &str, mood: Option<VoiceMood>, wav: &[u8]) {
    let dir = cache_dir();
    if std::fs::create_dir_all(&dir).is_err() { return; }
    let _ = std::fs::write(dir.join(format!("{}.wav", cache_key(t, text, mood))), wav);
    let _ = std::fs::write(dir.join(format!("{}.wav", text_key(t, text))), wav);
    trim_cache();
}

fn trim_cache() {
    let Ok(rd) = std::fs::read_dir(cache_dir()) else { return };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd
        .flatten()
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if files.len() <= MAX_CACHE { return; }
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    for (_, p) in files.into_iter().skip(MAX_CACHE) { let _ = std::fs::remove_file(p); }
}

// ------------------------------------------------------------------ 请求体

fn ref_voice() -> Option<String> {
    // 参考音频约 350KB，只在发 clone 请求时临时转 base64，用完即丢
    crate::assets::get("voice/hakimi-voice.wav")
        .map(|b| format!("data:audio/wav;base64,{}", base64::engine::general_purpose::STANDARD.encode(b)))
}

pub fn build_body(t: &TtsConfig, text: &str, mood: Option<VoiceMood>) -> Value {
    build_body_with(t, text, mood, ref_voice)
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
            "audio": { "format": "wav" },
        }),
        _ => match refv() {
            Some(r) => json!({
                "model": "mimo-v2.5-tts-voiceclone",
                "messages": [{ "role": "user", "content": "" }, { "role": "assistant", "content": content }],
                "audio": { "format": "wav", "voice": r },
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
        "audio": { "format": "wav", "voice": voice },
    })
}

// ------------------------------------------------------------------ 调度

struct Job {
    seq: u64,
    t: TtsConfig,
    text: String,
    mood: Option<VoiceMood>,
}

struct State {
    next_seq: u64,
    /// 还没回复的前台请求 (seq, token, tx)；保证每个 token 恰好回一次
    pending: Vec<(u64, u64, EventSender)>,
    /// 等着发的前台请求（只留最新一条）
    fg: Option<Job>,
    fg_busy: bool,
    cooldown_until: Option<Instant>,
    last_fg: Option<Instant>,
    warm: VecDeque<(String, Option<VoiceMood>)>,
    /// 下一次预热的时间；None = 预热没在跑
    warm_at: Option<Instant>,
    worker: bool,
}

static STATE: Mutex<State> = Mutex::new(State {
    next_seq: 0,
    pending: Vec::new(),
    fg: None,
    fg_busy: false,
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

/// 合成/取缓存，完成后 tx.send(AppEvent::Speech { token, audio })。
/// 根据 config.tts 判断：enabled=false / speak=off / speak=chat 且 kind=Line → audio=None。
/// 新请求来了要掐掉旧的在途请求（旧的回 None）。
pub fn speak(tx: &EventSender, token: u64, text: String, mood: Option<VoiceMood>, kind: SpeakKind) {
    let t = crate::core::config::get().tts;
    let (seq, old) = {
        let mut st = state();
        st.next_seq += 1;
        let seq = st.next_seq;
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
        if let Some(a) = read_cache(&t, &text, mood) { return answer(seq, Some(a)); }
        if t.api_key.is_empty() { return answer(seq, None); }
        let mut st = state();
        if in_cooldown(&st) {
            drop(st);
            return answer(seq, None);
        }
        if !st.pending.iter().any(|p| p.0 == seq) { return; }
        st.fg = Some(Job { seq, t, text, mood });
        ensure_worker(&mut st);
        drop(st);
        WAKE.notify_all();
    });
    if spawned.is_err() { answer(seq, None); }
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

enum Work { Fg(Job), Warm(TtsConfig, String, Option<VoiceMood>) }

fn worker() {
    loop {
        let work = {
            let mut st = state();
            loop {
                if let Some(job) = st.fg.take() {
                    st.fg_busy = true;
                    st.last_fg = Some(Instant::now());
                    break Work::Fg(job);
                }
                let now = Instant::now();
                match st.warm_at {
                    Some(at) if now >= at => {
                        let t = crate::core::config::get().tts;
                        if !t.enabled || t.api_key.is_empty() || t.speak == "off" || st.warm.is_empty() {
                            st.warm_at = None;
                            continue;
                        }
                        // 前台刚念过话（3 秒内）、正在冷却时先让路
                        let mut until = now;
                        if let Some(c) = st.cooldown_until { until = until.max(c); }
                        if let Some(l) = st.last_fg { until = until.max(l + Duration::from_secs(3)); }
                        if until > now {
                            st.warm_at = Some(until + Duration::from_millis(500));
                            continue;
                        }
                        let (text, mood) = st.warm.pop_front().unwrap_or_default();
                        break Work::Warm(t, text, mood);
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
            Work::Warm(t, raw, mood) => {
                let text = clean(&raw);
                let next = if !text.is_empty() && read_cache(&t, &text, mood).is_none() {
                    let _ = request_once(&t, &text, mood, 25);
                    Duration::from_secs(6)
                } else {
                    Duration::from_millis(50)
                };
                let mut st = state();
                if st.warm_at.is_some() { st.warm_at = Some(Instant::now() + next); }
            }
        }
    }
}

fn run_fg(job: Job) {
    let mut result = Err(Fail { retry: false, msg: "superseded".into() });
    for attempt in 0..2 {
        if !is_pending(job.seq) { break; }
        result = request_once(&job.t, &job.text, job.mood, if attempt == 0 { 20 } else { 25 });
        match &result {
            Err(f) if f.retry => continue,
            _ => break,
        }
    }
    {
        let mut st = state();
        st.fg_busy = false;
        st.last_fg = Some(Instant::now());
    }
    match result {
        Ok(wav) => answer(job.seq, Some(wav)),
        Err(f) => {
            if is_pending(job.seq) && f.msg != "superseded" {
                crate::core::log::log(&format!("tts error {}", f.msg));
            }
            answer(job.seq, None);
        }
    }
}

struct Fail {
    retry: bool,
    msg: String,
}

/// 发一次合成请求；成功写缓存并返回 wav 字节
fn request_once(t: &TtsConfig, text: &str, mood: Option<VoiceMood>, timeout_secs: u64) -> Result<Vec<u8>, Fail> {
    let body = build_body(t, text, mood);
    let r = super::net::post_json(API, &[("api-key", &t.api_key)], &body, timeout_secs);
    drop(body);
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
    write_cache(t, text, mood, &wav);
    Ok(wav)
}

// ------------------------------------------------------------------ 设置窗口「试听」

/// 试听用的一句（内置录音里也有，没填 Key 时 clone 模式直接放内置的）
pub const SAMPLE: &str = "喵？找哈基米有事吗～";

/// 用设置窗口里还没保存的语音设置合成一句（阻塞，后台线程调）。失败返回中文原因（不含 Key）
pub fn synth_test(t: &TtsConfig, text: &str) -> Result<Vec<u8>, String> {
    if t.api_key.trim().is_empty() { return Err("还没填 MiMo Key".into()); }
    let mut t = t.clone();
    t.api_key = t.api_key.trim().to_string();
    request_once(&t, &clean(text), None, 25).map_err(|f| {
        let m = f.msg;
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
    fn bundled_lines_present() {
        let t = TtsConfig::default();
        let lines: Value = serde_json::from_slice(crate::assets::get("voice/lines.json").unwrap()).unwrap();
        let hits = lines.as_object().unwrap().keys().filter(|k| bundled(&t, &clean(k)).is_some()).count();
        assert!(hits >= 87, "only {hits} bundled lines");
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
        assert!(ref_voice().is_some_and(|s| s.starts_with("data:audio/wav;base64,UklG")));
    }
}
