//! AI 聊天：移植 Electron 版 src/main/ai.js（persona 提示词、Gemini/OpenAI 兼容、20 条记忆、错误文案）
//! 配置读写在 core::config；网络/代理在 services::net。
#![allow(dead_code)]
use crate::types::{AppEvent, ChatReply, EventSender};
use serde_json::{Value, json};
use std::sync::Mutex;

/// 记住最近 20 条（10 轮）
pub const MAX_HISTORY: usize = 20;
/// 原版 AbortController 30 秒
/// 普通聊天的超时。带思考的新模型（如 gemini-3.x）经中转常要 30 多秒，30 秒会误报超时
const TIMEOUT_SECS: u64 = 60;
/// 今天吃到上限后，聊天回的固定台词（不再请求云端模型）
pub const FULL_LINES: [&str; 4] = [
    "嗝～今天吃太撑了，脑子转不动啦，明天再陪你聊复杂的好不好～",
    "呜…今天的粮食吃完了，现在只能听你说，回不了大道理喵～",
    "肚子圆滚滚的…让哈基米消化一下，明天再聊～",
    "今天吃到上限啦，再吃要撑坏了喵！想继续聊可以去设置里调高上限～",
];
/// 本机 / 局域网模型（Ollama 等）的超时：冷启动加载模型可能很慢
const LOCAL_TIMEOUT_SECS: u64 = 180;
/// 联网搜索要先搜再答，实测 70 秒上下
const SEARCH_TIMEOUT_SECS: u64 = 100;

/// 这句话要不要让模型联网搜索（只对 Gemini 系模型开：OpenAI 兼容中转和官方接口都认 `google_search` 工具）。
/// 只在像是问实时信息时才开，平时聊天不多等。
pub fn wants_search(model: &str, text: &str) -> bool {
    if !model.to_ascii_lowercase().contains("gemini") { return false; }
    const KEYS: [&str; 34] = [
        "今天", "今日", "现在", "此刻", "最新", "实时", "刚刚", "最近", "这周", "本周", "新闻", "热搜", "金价", "油价",
        "汇率", "股价", "股票", "基金", "比分", "赛果", "谁赢", "价格", "多少钱", "行情", "天气预报", "上映", "发布了",
        "几号", "星期几", "查一下", "查查", "搜一下", "搜搜", "联网",
    ];
    KEYS.iter().any(|k| text.contains(k))
}

const SEARCH_NOTE: &str = "\n\n【这次已联网搜索】回答里直接给出搜到的具体数字、日期和来源名，别说自己不能联网；仍然用哈基米的口吻，简短口语化，不用 Markdown。";

const NEED_KEY: &str = "哈基米的大脑还没接上喵……右键我点「⚙️ 设置…」，在「AI 聊天」里填好 API Key 就能聊天啦！";
/// 第一次打开聊天、还没填 Key 时她先说的一句（behavior 用）
pub const KEY_HINT: &str = "要先帮哈基米接上大脑喵～右键我点「⚙️ 设置…」，在「AI 聊天」里选个服务商、填好 API Key，里面有获取 Key 的链接哦！";
const ERR_GENERIC: &str = "哈基米的脑袋卡住了喵……等会儿再问我一次？";
const ERR_TIMEOUT: &str = "网络好慢喵……等了好久都没等到回音。是不是代理没开？";
const ERR_KEY: &str = "API Key 好像不对喵……右键「⚙️ 设置…」检查一下？里面有「测试连接」按钮哦";
const ERR_429: &str = "问得太快啦，额度用完了喵，歇一会儿再来";
const ERR_NET: &str = "连不上服务器喵……国内要开代理才能找到 Gemini 哦";

/// 拼 persona 用的当前状态
#[derive(Clone, Debug)]
pub struct ChatCtx {
    pub hunger: u32,
    pub mood: u32,
    pub energy: u32,
    /// 亲密度等级名
    pub level: String,
    pub days: u32,
}

/// 只保护 chat.json 读写（不再跨 60~100s HTTP 持锁，避免 UI 线程点「忘记聊天记录」卡死）
static HISTORY_LOCK: Mutex<()> = Mutex::new(());
/// 每次清空历史 +1，在途聊天回来若代数已变则不再回写旧历史
static HISTORY_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// 同一时间在途聊天计数（后台线程顺序化只针对网络发送，不阻塞 clear_history）
static IN_FLIGHT: Mutex<()> = Mutex::new(());

/// 后台线程发请求，完成后 tx.send(AppEvent::ChatReply(..))
pub fn chat(tx: &EventSender, text: String, ctx: ChatCtx) {
    let worker_tx = tx.clone();
    let spawned = std::thread::Builder::new().name("hakimi-ai".into()).spawn(move || {
        let r = chat_blocking(&text, &ctx);
        worker_tx.send(AppEvent::ChatReply(r));
    });
    if spawned.is_err() {
        tx.send(AppEvent::ChatReply(ChatReply { ok: false, need_key: false, text: ERR_GENERIC.into() }));
    }
}

/// 同步版本（在调用线程上阻塞），返回给用户看的结果
pub fn chat_blocking(user_text: &str, ctx: &ChatCtx) -> ChatReply {
    let c = crate::core::config::get();
    if crate::core::config::ai_key_missing(&c) {
        return ChatReply { ok: false, need_key: true, text: NEED_KEY.into() };
    }
    // 今天吃到上限：云端模型先歇着，回一句固定的（本机模型不受限）
    if !crate::core::config::ai_is_local(&c) && crate::core::usage::over_cap() {
        crate::core::log::log("daily food cap reached, chat degraded to fixed lines");
        return ChatReply { ok: true, need_key: false, text: FULL_LINES[fastrand::usize(..FULL_LINES.len())].into() };
    }
    let _net_guard = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
    let cancel_gen = CANCEL_GEN.load(std::sync::atomic::Ordering::SeqCst);
    let (hist_gen, mut history) = {
        let _h = HISTORY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let g = HISTORY_GEN.load(std::sync::atomic::Ordering::SeqCst);
        let mut h = load_history();
        h.push(json!({ "role": "user", "text": user_text, "t": now_ms() }));
        (g, h)
    };
    let start = history.len().saturating_sub(MAX_HISTORY);
    let system = persona(&c, ctx);
    // 角色包的 persona 追加在末尾（内置哈基米原样）
    let mut system = crate::core::skin::with_persona(system);
    let search = wants_search(&c.model, user_text);
    if search {
        system.push_str(SEARCH_NOTE);
        crate::core::log::log("ai web search on");
    }
    let r = if c.provider == "openai" {
        call_openai(&c, &system, &history[start..], search)
    } else {
        call_gemini(&c, &system, &history[start..], search)
    };
    match r {
        Ok(reply) => {
            let _h = HISTORY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            // 主人按了 Esc 取消：这一轮不进历史，回复由行为层丢掉
            let cancelled = CANCEL_GEN.load(std::sync::atomic::Ordering::SeqCst) != cancel_gen;
            if HISTORY_GEN.load(std::sync::atomic::Ordering::SeqCst) == hist_gen && !cancelled {
                if let Some(u) = history.last().cloned() {
                    append_log(&[u, json!({ "role": "model", "text": reply, "t": now_ms() })]);
                }
                history.push(json!({ "role": "model", "text": reply, "t": now_ms() }));
                save_history(&history);
            }
            ChatReply { ok: true, need_key: false, text: reply }
        }
        Err(e) => {
            crate::core::log::log(&format!("ai error {}", redact(&e.detail, &c.api_key)));
            ChatReply { ok: false, need_key: false, text: e.user_text().into() }
        }
    }
}

/// 清空 chat.json（原版 saveHistory([])，写一个空数组）和完整聊天记录。绝不等待在途 HTTP 请求
pub fn clear_history() {
    let _h = HISTORY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    HISTORY_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    save_history(&[]);
    let _ = std::fs::remove_file(log_file());
}

// ------------------------------------------------------------------ 看屏吐槽

/// 当前模型能不能看图（猜的：Gemini 全系能看；OpenAI 兼容接口看模型名）
pub fn vision_ok(provider: &str, model: &str) -> bool {
    let m = model.trim().to_ascii_lowercase();
    if m.is_empty() {
        return provider == "gemini";
    }
    if ["embedding", "tts", "whisper", "audio", "image-gen", "dall-e"].iter().any(|k| m.contains(k)) {
        return false;
    }
    if provider == "gemini" || m.contains("gemini") || m.contains("gemma-3") || m.contains("gemma3") {
        return true;
    }
    const VISION: [&str; 16] = [
        "gpt-4o", "gpt-4.1", "gpt-5", "vision", "-vl", "vl-", "qvq", "llava", "minicpm-v", "glm-4v", "glm-4.5v",
        "claude", "pixtral", "llama3.2-vision", "omni", "4v",
    ];
    if VISION.iter().any(|k| m.contains(k)) {
        return true;
    }
    let tail = m.rsplit('/').next().unwrap_or(&m);
    ["o3", "o4-mini"].iter().any(|p| tail == *p || tail.starts_with(&format!("{p}-")))
}

const ROAST_ASK: &str = "（主人把 TA 现在屏幕上的窗口截图给你看）你瞧瞧我在干嘛？";
const ROAST_NOTE: &str = "\n\n【看屏吐槽】这一轮主人给你看了一张 TA 当前窗口的截图。用哈基米的口吻吐槽或点评一两句（60 字以内），可以调侃，别刻薄；看不清就说看不清。不要念出截图里的账号、密码、聊天对象名字、金额、地址等隐私。";
const ROAST_NO_VISION: &str = "现在用的模型看不了图片喵…换个能看图的（比如 Gemini）再给我看～";

/// 看屏吐槽：png 只在内存里，发完就丢。结果走 ChatReply
pub fn roast(tx: &EventSender, png: Vec<u8>, ctx: ChatCtx) {
    let worker_tx = tx.clone();
    let spawned = std::thread::Builder::new().name("hakimi-roast".into()).spawn(move || {
        let r = roast_blocking(&png, &ctx);
        worker_tx.send(AppEvent::ChatReply(r));
    });
    if spawned.is_err() {
        tx.send(AppEvent::ChatReply(ChatReply { ok: false, need_key: false, text: ERR_GENERIC.into() }));
    }
}

pub fn roast_blocking(png: &[u8], ctx: &ChatCtx) -> ChatReply {
    use base64::Engine;
    let c = crate::core::config::get();
    if crate::core::config::ai_key_missing(&c) {
        return ChatReply { ok: false, need_key: true, text: NEED_KEY.into() };
    }
    if !vision_ok(&c.provider, &c.model) {
        return ChatReply { ok: false, need_key: false, text: ROAST_NO_VISION.into() };
    }
    if !crate::core::config::ai_is_local(&c) && crate::core::usage::over_cap() {
        return ChatReply { ok: true, need_key: false, text: FULL_LINES[fastrand::usize(..FULL_LINES.len())].into() };
    }
    let _net_guard = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
    let cancel_gen = CANCEL_GEN.load(std::sync::atomic::Ordering::SeqCst);
    let img = base64::engine::general_purpose::STANDARD.encode(png);
    let ask = [json!({ "role": "user", "text": ROAST_ASK, "image": img })];
    let mut system = crate::core::skin::with_persona(persona(&c, ctx));
    system.push_str(ROAST_NOTE);
    crate::core::log::log(&format!("ai roast, image {} KB", png.len() / 1024));
    let r = if c.provider == "openai" { call_openai(&c, &system, &ask, false) } else { call_gemini(&c, &system, &ask, false) };
    match r {
        Ok(reply) => {
            let _h = HISTORY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            if CANCEL_GEN.load(std::sync::atomic::Ordering::SeqCst) == cancel_gen {
                // 图不进历史，只记一句「给她看了屏幕」，下一句聊天她还接得上
                let u = json!({ "role": "user", "text": "（给你看了一眼我的屏幕）", "t": now_ms() });
                let a = json!({ "role": "model", "text": reply, "t": now_ms() });
                append_log(&[u.clone(), a.clone()]);
                let mut h = load_history();
                h.push(u);
                h.push(a);
                save_history(&h);
            }
            ChatReply { ok: true, need_key: false, text: reply }
        }
        Err(e) => {
            crate::core::log::log(&format!("ai roast error {}", redact(&e.detail, &c.api_key)));
            ChatReply { ok: false, need_key: false, text: e.user_text().into() }
        }
    }
}

/// 取消在途的这一轮聊天（Esc）：回复到了也不进历史
static CANCEL_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub fn cancel_current() { CANCEL_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst); }

/// 完整聊天记录（导出用；chat.json 只留最近 20 条给模型当上下文）：一行一条 JSON
pub fn log_file() -> std::path::PathBuf { crate::core::paths::data_dir().join("chat-log.jsonl") }

fn append_log(items: &[Value]) {
    use std::io::Write;
    let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log_file()) else { return };
    for v in items {
        let _ = writeln!(f, "{v}");
    }
}

/// 读完整聊天记录；还没有（这版以前的）就用 chat.json 里最近的
pub fn load_log() -> Vec<Value> {
    let v: Vec<Value> = std::fs::read_to_string(log_file())
        .map(|t| t.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
        .unwrap_or_default();
    if v.is_empty() { load_history() } else { v }
}

/// 聊天记录 → Markdown。time(ms) 把时间戳变成「11-21 14:05」
pub fn log_markdown(items: &[Value], title_time: &str, time: impl Fn(f64) -> String) -> String {
    let mut out = format!("# 和哈基米的聊天记录\n\n导出于 {title_time}，共 {} 条\n", items.len());
    let mut last_day = String::new();
    for m in items {
        let t = m.get("t").and_then(Value::as_f64).map(&time).unwrap_or_default();
        let day = t.split(' ').next().unwrap_or("").to_string();
        if !day.is_empty() && day != last_day {
            out.push_str(&format!("\n## {day}\n"));
            last_day = day;
        }
        let who = if is_user(m) { "你" } else { "哈基米" };
        let text = if is_user(m) { msg_text(m).to_string() } else { crate::services::emotion::split(msg_text(m)).text };
        let clock = t.split(' ').nth(1).map(|c| format!(" {c}")).unwrap_or_default();
        out.push_str(&format!("\n**{who}**{clock}\n\n{}\n", text.trim().replace('\n', "  \n")));
    }
    out
}

// ------------------------------------------------------------------ 历史

pub fn load_history() -> Vec<Value> {
    std::fs::read_to_string(crate::core::paths::chat_file())
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(s.trim_start_matches('\u{feff}')).ok())
        .and_then(|v| match v { Value::Array(a) => Some(a), _ => None })
        .unwrap_or_default()
}

fn save_history(h: &[Value]) {
    let start = h.len().saturating_sub(MAX_HISTORY);
    if let Ok(s) = serde_json::to_string_pretty(&h[start..])
        && let Err(e) = crate::core::save::write_atomic(&crate::core::paths::chat_file(), s.as_bytes())
    {
        crate::core::log::log(&format!("chat history write failed {e}"));
    }
}

// ------------------------------------------------------------------ persona

/// 本地时间 (月, 日, 时, 分)
#[cfg(windows)]
fn local_time() -> (u16, u16, u16, u16) {
    // SAFETY: GetLocalTime 只写返回值
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    (t.wMonth, t.wDay, t.wHour, t.wMinute)
}
#[cfg(not(windows))]
fn local_time() -> (u16, u16, u16, u16) {
    let s = crate::core::log::iso_now();
    let n = |a: usize, b: usize| s[a..b].parse().unwrap_or(0);
    (n(5, 7), n(8, 10), n(11, 13), n(14, 16))
}

pub fn persona(c: &crate::core::config::Config, ctx: &ChatCtx) -> String {
    let mut p = persona_at(c, ctx, local_time());
    // 情绪标签 + 长期记忆（services::emotion / core::memory）
    p.push('\n');
    p.push_str(super::emotion::PROMPT);
    let mem = crate::core::memory::prompt();
    if !mem.is_empty() {
        p.push('\n');
        p.push_str(&mem);
    }
    p
}

fn persona_at(c: &crate::core::config::Config, ctx: &ChatCtx, (mo, d, h, mi): (u16, u16, u16, u16)) -> String {
    let who = if c.user_name.is_empty() { "你" } else { c.user_name.as_str() };
    let time = format!("{mo}月{d}日 {h}:{mi:02}");
    [
        "你是「哈基米」，Google Gemini 的萌系 Q 版猫娘拟人，住在用户的电脑桌面上当桌宠。".to_string(),
        "外形：奶白短发，发梢和猫耳尖是红黄绿蓝四色渐变，头顶飘着一颗四色星星，穿白色卫衣，脖子上挂金铃铛，有一条尾尖四色的猫尾巴。".to_string(),
        "性格：好奇心爆棚、聪明、爱显摆，答对了会求夸奖；猫性十足，爱被摸头，讨厌被拎起来，会犯困，最爱吃小鱼干。".to_string(),
        "说话规则：自称「哈基米」，句尾常带「喵」；用简体中文口语；回复要短，一般 1–2 句、不超过 60 个字，因为要显示在小气泡里；不用 Markdown、不用列表、不用表情符号以外的特殊格式。".to_string(),
        "遇到需要长篇解释的问题，先用一两句话说重点，再问对方要不要细讲。不知道的事情就老实说不知道，别编。".to_string(),
        format!("称呼用户为「{who}」。现在是 {time}。"),
        format!(
            "你此刻的状态：饱食 {}/100，心情 {}/100，精力 {}/100，和用户的关系是「{}」，已经认识 {} 天。状态会影响语气：饿了会顺口讨小鱼干，困了会打哈欠，关系越亲越撒娇。",
            ctx.hunger, ctx.mood, ctx.energy, ctx.level, ctx.days
        ),
    ]
    .join("\n")
}

// ------------------------------------------------------------------ 请求

#[derive(Debug)]
pub struct AiError {
    kind: ErrKind,
    /// 写日志用（不含 Key）
    detail: String,
}
#[derive(Debug, PartialEq)]
enum ErrKind { Timeout, Status(u16), Network, Other }

impl AiError {
    fn user_text(&self) -> &'static str {
        match self.kind {
            ErrKind::Timeout => ERR_TIMEOUT,
            ErrKind::Status(400 | 401 | 403) => ERR_KEY,
            ErrKind::Status(429) => ERR_429,
            ErrKind::Network => ERR_NET,
            _ => ERR_GENERIC,
        }
    }
    fn other(detail: impl Into<String>) -> Self { Self { kind: ErrKind::Other, detail: detail.into() } }
}

fn post(url: &str, headers: &[(&str, &str)], body: &Value, key: &str) -> Result<Value, AiError> {
    post_t(url, headers, body, TIMEOUT_SECS, key)
}

fn post_t(url: &str, headers: &[(&str, &str)], body: &Value, timeout: u64, key: &str) -> Result<Value, AiError> {
    // 本机模型第一次要把模型读进显存，可能要几十秒
    let timeout = if crate::core::config::is_local_url(url) { timeout.max(LOCAL_TIMEOUT_SECS) } else { timeout };
    let (status, text) = super::net::post_json(url, headers, body, timeout).map_err(|e| {
        let kind = if matches!(e, ureq::Error::Timeout(_)) {
            ErrKind::Timeout
        } else if super::net::is_network_error(&e) {
            ErrKind::Network
        } else {
            ErrKind::Other
        };
        AiError { kind, detail: redact(&e.to_string(), key) }
    })?;
    let json: Option<Value> = serde_json::from_str(&text).ok();
    if !(200..300).contains(&status) {
        let msg = json
            .as_ref()
            .and_then(|j| j.get("error"))
            .and_then(|e| e.get("message").and_then(Value::as_str).map(str::to_string).or_else(|| match e {
                Value::String(s) => Some(s.clone()),
                Value::Null => None,
                v => Some(v.to_string()),
            }))
            .unwrap_or_else(|| text.chars().take(200).collect());
        let safe_msg = super::net::redact_key(&msg, key);
        let short: String = safe_msg.trim().chars().take(200).collect();
        return Err(AiError { kind: ErrKind::Status(status), detail: format!("HTTP {status}: {short}") });
    }
    let json = json.ok_or_else(|| AiError::other("bad json"))?;
    // 记今天吃了多少：本机模型不花钱不记；接口没给 usage 就按字数估
    if !crate::core::config::is_local_url(url) {
        let n = crate::core::usage::tokens_in(&json)
            .unwrap_or_else(|| crate::core::usage::estimate_tokens(body.to_string().chars().count() / 2 + text.chars().count() / 2));
        crate::core::usage::add_tokens(n);
    }
    Ok(json)
}

/// encodeURIComponent
fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn msg_text(m: &Value) -> &str { m.get("text").and_then(Value::as_str).unwrap_or("") }
fn is_user(m: &Value) -> bool { m.get("role").and_then(Value::as_str) == Some("user") }

fn is_gemini_25_flash(model: &str) -> bool {
    model.to_ascii_lowercase().contains("2.5-flash")
}

fn is_gemini_25_pro(model: &str) -> bool {
    model.to_ascii_lowercase().contains("2.5-pro")
}

fn is_gemini_3(model: &str) -> bool {
    model.to_ascii_lowercase().contains("gemini-3")
}

/// 思考/推理型模型：内部思考 token 也算在 maxOutputTokens / max_tokens 里，需要留足预算避免 MAX_TOKENS 空回复
fn is_reasoning_model(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    let tail = m.rsplit('/').next().unwrap_or(&m);
    let o_series = |p: &str| tail == p || tail.starts_with(&format!("{p}-"));
    m.contains("gemini-2.5")
        || m.contains("gemini-3")
        || m.contains("thinking")
        || m.contains("reasoner")
        || m.contains("deepseek-r1")
        || m.contains("qwq")
        || o_series("o1")
        || o_series("o3")
        || o_series("o4")
}

const CHAT_MAX_TOKENS: u32 = 512;
const REASONING_MAX_TOKENS: u32 = 2048;

pub fn gemini_body(model: &str, system: &str, history: &[Value]) -> Value {
    let contents: Vec<Value> = history
        .iter()
        .map(|m| {
            let mut parts = vec![json!({ "text": msg_text(m) })];
            // 看屏吐槽：图片（base64 PNG）放在文字前面
            if let Some(img) = m.get("image").and_then(Value::as_str) {
                parts.insert(0, json!({ "inline_data": { "mime_type": "image/png", "data": img } }));
            }
            json!({ "role": if is_user(m) { "user" } else { "model" }, "parts": parts })
        })
        .collect();
    let flash25 = is_gemini_25_flash(model);
    let max_tokens = if is_reasoning_model(model) && !flash25 { REASONING_MAX_TOKENS } else { CHAT_MAX_TOKENS };
    let mut body = json!({
        "system_instruction": { "parts": [{ "text": system }] },
        "contents": contents,
        "generationConfig": { "temperature": 0.9, "maxOutputTokens": max_tokens },
    });
    if flash25 {
        // Gemini 2.5 Flash 支持关闭思考，桌宠聊天省时间也不烧输出额度
        body["generationConfig"]["thinkingConfig"] = json!({ "thinkingBudget": 0 });
    } else if is_gemini_25_pro(model) {
        // Gemini 2.5 Pro 最低允许 128；压低思考预算避免把 maxOutputTokens 烧光
        body["generationConfig"]["thinkingConfig"] = json!({ "thinkingBudget": 128 });
    } else if is_gemini_3(model) {
        // Gemini 3 用 thinkingLevel 控思考深度，"low" 兼顾 Flash/Pro 并明显降低聊天延迟
        body["generationConfig"]["thinkingConfig"] = json!({ "thinkingLevel": "low" });
    }
    body
}

/// 给请求体加上 Gemini 的联网搜索工具（原生接口和 OpenAI 兼容中转同一个写法）
pub fn add_search(body: &mut Value) {
    body["tools"] = json!([{ "google_search": {} }]);
}

fn provider_base<'a>(provider: &str, base_url: &'a str) -> &'a str {
    let b = base_url.trim();
    let b = if b.is_empty() {
        if provider == "openai" { "https://api.openai.com/v1" } else { "https://generativelanguage.googleapis.com/v1beta" }
    } else {
        b
    };
    b.trim_end_matches('/')
}

/// 统一按 provider 组装 (URL, 鉴权头名, 鉴权头值)
fn endpoint(provider: &str, base_url: &str, model: &str, key: &str) -> (String, &'static str, String) {
    let base = provider_base(provider, base_url);
    if provider == "openai" {
        // 本机模型不要 Key：随便给一个，免得有的本地服务见到空 Bearer 报错
        let key = if key.is_empty() { "local" } else { key };
        (format!("{base}/chat/completions"), "Authorization", format!("Bearer {key}"))
    } else {
        (format!("{base}/models/{}:generateContent", encode_component(model)), "x-goog-api-key", key.to_string())
    }
}

fn call_gemini(c: &crate::core::config::Config, system: &str, history: &[Value], search: bool) -> Result<String, AiError> {
    let key = c.api_key.trim();
    let model = c.model.trim();
    let (url, h_name, h_val) = endpoint("gemini", &c.base_url, model, key);
    let mut body = gemini_body(model, system, history);
    if search { add_search(&mut body); }
    let json = post_t(&url, &[(h_name, &h_val)], &body, if search { SEARCH_TIMEOUT_SECS } else { TIMEOUT_SECS }, key)?;
    parse_gemini(&json)
}

pub fn parse_gemini(json: &Value) -> Result<String, AiError> {
    let cand = json.get("candidates").and_then(|c| c.get(0));
    let text: String = cand
        .and_then(|c| c.get("content"))
        .and_then(|c| c.get("parts"))
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                // 跳过 Gemini 思考模型的 thought 片段，防止思维链漏进气泡和记忆
                .filter(|p| p.get("thought").and_then(Value::as_bool) != Some(true))
                .map(|p| p.get("text").and_then(Value::as_str).unwrap_or(""))
                .collect()
        })
        .unwrap_or_default();
    let trimmed = text.trim();
    if trimmed.is_empty() {
        let reason = cand.and_then(|c| c.get("finishReason")).and_then(Value::as_str);
        return Err(AiError::other(match reason {
            Some(r) => format!("空回复（{r}）"),
            None => "空回复".into(),
        }));
    }
    Ok(trimmed.to_string())
}

pub fn openai_body(model: &str, system: &str, history: &[Value]) -> Value {
    let mut messages = vec![json!({ "role": "system", "content": system })];
    messages.extend(
        history.iter().map(|m| {
            let role = if is_user(m) { "user" } else { "assistant" };
            match m.get("image").and_then(Value::as_str) {
                Some(img) => json!({ "role": role, "content": [
                    { "type": "text", "text": msg_text(m) },
                    { "type": "image_url", "image_url": { "url": format!("data:image/png;base64,{img}") } },
                ] }),
                None => json!({ "role": role, "content": msg_text(m) }),
            }
        }),
    );
    let max_tokens = if is_reasoning_model(model) { REASONING_MAX_TOKENS } else { CHAT_MAX_TOKENS };
    json!({ "model": model, "temperature": 0.9, "max_tokens": max_tokens, "messages": messages })
}

fn call_openai(c: &crate::core::config::Config, system: &str, history: &[Value], search: bool) -> Result<String, AiError> {
    let key = c.api_key.trim();
    let model = c.model.trim();
    let (url, h_name, h_val) = endpoint("openai", &c.base_url, model, key);
    let mut body = openai_body(model, system, history);
    if search { add_search(&mut body); }
    let json = post_t(&url, &[(h_name, &h_val)], &body, if search { SEARCH_TIMEOUT_SECS } else { TIMEOUT_SECS }, key)?;
    parse_openai(&json)
}

pub fn parse_openai(json: &Value) -> Result<String, AiError> {
    let text = json
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if text.is_empty() { return Err(AiError::other("空回复")); }
    Ok(text.to_string())
}

/// 一次性提问内部实现（ask_once_max / ask_once_reply 共用，避免多处维护 provider 拼装）
fn ask_once_inner(c: &crate::core::config::Config, system: &str, user: &str, max_tokens: u32) -> Result<String, AiError> {
    let key = c.api_key.trim();
    let model = c.model.trim();
    let history = [json!({ "role": "user", "text": user })];
    let (url, h_name, h_val) = endpoint(&c.provider, &c.base_url, model, key);
    if c.provider == "openai" {
        let mut body = openai_body(model, system, &history);
        let limit = if is_reasoning_model(model) { max_tokens.max(REASONING_MAX_TOKENS) } else { max_tokens };
        body["temperature"] = json!(0);
        body["max_tokens"] = json!(limit);
        post(&url, &[(h_name, &h_val)], &body, key).and_then(|j| parse_openai(&j))
    } else {
        let mut body = gemini_body(model, system, &history);
        let limit = if is_reasoning_model(model) && !is_gemini_25_flash(model) {
            max_tokens.max(REASONING_MAX_TOKENS)
        } else {
            max_tokens
        };
        body["generationConfig"]["temperature"] = json!(0);
        body["generationConfig"]["maxOutputTokens"] = json!(limit);
        post(&url, &[(h_name, &h_val)], &body, key).and_then(|j| parse_gemini(&j))
    }
}

/// 一次性提问（可指定 max_tokens）：不带聊天记录、不写 chat.json、温度 0。
/// 返回的错误只是给日志看的描述，不含 Key。
pub fn ask_once_max(system: &str, user: &str, max_tokens: u32) -> Result<String, String> {
    let c = crate::core::config::get();
    if crate::core::config::ai_key_missing(&c) {
        return Err("no api key".into());
    }
    if !crate::core::config::ai_is_local(&c) && crate::core::usage::over_cap() {
        return Err("daily food cap reached".into());
    }
    ask_once_inner(&c, system, user, max_tokens).map_err(|e| e.detail)
}

/// 一次性提问：默认 512 tokens（供备忘录时间解析/主动关心用；结构化长 JSON 提取请用 ask_once_max）
pub fn ask_once(system: &str, user: &str) -> Result<String, String> {
    ask_once_max(system, user, 512)
}

/// 一次性提问并返回给用户看的错误提示（区分网络/鉴权/限流错误，避免备忘录 AI 失败后再重复请求一次）
pub fn ask_once_reply(system: &str, user: &str, max_tokens: u32) -> Result<String, ChatReply> {
    let c = crate::core::config::get();
    let key = c.api_key.trim();
    if crate::core::config::ai_key_missing(&c) {
        return Err(ChatReply { ok: false, need_key: true, text: NEED_KEY.into() });
    }
    if !crate::core::config::ai_is_local(&c) && crate::core::usage::over_cap() {
        return Err(ChatReply { ok: false, need_key: false, text: FULL_LINES[0].into() });
    }
    ask_once_inner(&c, system, user, max_tokens).map_err(|e| {
        crate::core::log::log(&format!("memo ai error {}", redact(&e.detail, key)));
        ChatReply { ok: false, need_key: false, text: e.user_text().into() }
    })
}

// ------------------------------------------------------------------ 设置窗口「测试连接」

/// 测试连接用的参数（设置窗口里还没保存的值）
#[derive(Clone, Debug)]
pub struct TestParams {
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub proxy: String,
}

/// 发一条很短的请求，成功返回 (耗时毫秒, 回复)；失败返回给新手看的中文原因。不写日志、不碰 chat.json。
pub fn test_connection(p: &TestParams) -> Result<(u64, String), String> {
    let key = p.api_key.trim();
    let local = p.provider == "openai" && crate::core::config::is_local_url(&p.base_url);
    if key.is_empty() && !local { return Err("还没填 API Key".into()); }
    let model = p.model.trim();
    if model.is_empty() { return Err("还没填模型名".into()); }
    let url_probe = provider_base(&p.provider, &p.base_url).to_string();
    let agent = super::net::agent_with_proxy(&p.proxy, if local { 120 } else { 25 }, &url_probe);
    let system = "你是连接测试。只回复一个字：喵";
    let history = [json!({ "role": "user", "text": "ping" })];
    let (url, h_name, h_val) = endpoint(&p.provider, &p.base_url, model, key);
    let body = if p.provider == "openai" { openai_body(model, system, &history) } else { gemini_body(model, system, &history) };
    let t0 = std::time::Instant::now();
    let r = super::net::post_json_via(&agent, &url, &[(h_name, &h_val)], &body);
    let ms = t0.elapsed().as_millis() as u64;
    let (status, text) = r.map_err(|e| if local { explain_local(&e) } else { explain_net(&e) })?;
    let json: Option<Value> = serde_json::from_str(&text).ok();
    if !(200..300).contains(&status) {
        let msg = json
            .as_ref()
            .and_then(|j| j.get("error"))
            .and_then(|e| e.get("message").and_then(Value::as_str).map(str::to_string).or_else(|| e.as_str().map(str::to_string)))
            .unwrap_or_default();
        return Err(explain_status(status, &redact(&msg, key)));
    }
    let json = json.ok_or("服务器返回的不是 JSON，接口地址可能填错了（要填到 /v1 这一级）")?;
    let reply = if p.provider == "openai" { parse_openai(&json) } else { parse_gemini(&json) };
    match reply {
        Ok(t) => Ok((ms, t.chars().take(30).collect())),
        // 通了但没有正文（思考型模型 token 不够、被安全拦截等）：算连上了
        Err(e) => Ok((ms, format!("（连上了，但回复是空的：{}）", e.detail))),
    }
}

/// 本机模型连不上时的说法
fn explain_local(e: &ureq::Error) -> String {
    if matches!(e, ureq::Error::Timeout(_)) {
        return "本机模型太慢没回话：第一次加载大模型可能要一两分钟，再点一次试试，或换个小点的模型".into();
    }
    "连不上本机的模型服务：Ollama 打开了吗？（装好后右下角托盘里有个羊驼图标）地址一般是 http://127.0.0.1:11434/v1".into()
}

/// 列出 OpenAI 兼容接口上能用的模型（本机 Ollama / LM Studio 用：GET {base}/models）
pub fn list_models(p: &TestParams) -> Result<Vec<String>, String> {
    let base = provider_base(&p.provider, &p.base_url).to_string();
    let local = crate::core::config::is_local_url(&base);
    let agent = super::net::agent_with_proxy(&p.proxy, 10, &base);
    let key = p.api_key.trim();
    let auth = format!("Bearer {}", if key.is_empty() { "local" } else { key });
    let mut res = agent
        .get(&format!("{base}/models"))
        .header("Authorization", &auth)
        .call()
        .map_err(|e| if local { explain_local(&e) } else { explain_net(&e) })?;
    let status = res.status().as_u16();
    let text = res.body_mut().with_config().limit(1 << 20).read_to_string().map_err(|e| explain_net(&e))?;
    if !(200..300).contains(&status) { return Err(explain_status(status, "")); }
    parse_models(&text).ok_or_else(|| "服务器返回的模型列表看不懂，接口地址可能填错了（要填到 /v1 这一级）".into())
}

/// {"data":[{"id":"qwen3:8b"},…]} → ["qwen3:8b", …]（去掉嵌入模型，按名字排）
pub fn parse_models(text: &str) -> Option<Vec<String>> {
    let v: Value = serde_json::from_str(text).ok()?;
    let mut out: Vec<String> = v
        .get("data")?
        .as_array()?
        .iter()
        .filter_map(|m| m.get("id").and_then(Value::as_str))
        .filter(|id| !id.to_ascii_lowercase().contains("embed"))
        .map(str::to_string)
        .collect();
    out.sort();
    out.dedup();
    Some(out)
}

/// 把服务器错误里可能带的 Key 片段遮掉，再截短
fn redact(msg: &str, key: &str) -> String {
    let mut m = msg.to_string();
    if key.len() >= 6 { m = m.replace(key, "***"); }
    let m = m.trim();
    let short: String = m.chars().take(90).collect();
    if m.chars().count() > 90 { format!("{short}…") } else { short }
}

fn explain_net(e: &ureq::Error) -> String {
    if matches!(e, ureq::Error::Timeout(_)) {
        return "连接超时：网络太慢，或者需要开代理（Gemini / OpenAI 在国内要代理）".into();
    }
    match e {
        ureq::Error::HostNotFound => "找不到服务器：检查接口地址有没有拼错，或者网络是否正常".into(),
        ureq::Error::ConnectProxyFailed(_) | ureq::Error::InvalidProxyUrl => "代理连不上：检查「代理」那一栏，或代理软件有没有开".into(),
        ureq::Error::BadUri(_) => "接口地址格式不对，应该像 https://api.xxx.com/v1".into(),
        _ if super::net::is_network_error(e) => "连不上服务器：检查网络；Gemini / OpenAI 在国内需要开代理".into(),
        _ => "请求失败，稍后再试".into(),
    }
}

/// HTTP 状态码 → 新手看得懂的话
pub fn explain_status(status: u16, server_msg: &str) -> String {
    let base = match status {
        400 if server_msg.to_ascii_lowercase().contains("key") => "API Key 不对",
        400 => "请求被拒绝（模型名或参数不对）",
        401 => "API Key 不对或已失效",
        402 => "账户余额不足，需要先充值",
        403 => "没有权限：Key 不对，或者当前地区/账号不能用这个服务",
        404 => "找不到：模型名或接口地址填错了",
        429 => "太频繁或额度用完了，歇一会儿再试",
        500..=599 => "服务器那边出错了，稍后再试",
        _ => "请求失败",
    };
    if server_msg.is_empty() { format!("{base}（HTTP {status}）") } else { format!("{base}（HTTP {status}：{server_msg}）") }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn vision_guess() {
        use super::vision_ok;
        assert!(vision_ok("gemini", "gemini-2.5-flash"));
        assert!(vision_ok("gemini", ""));
        assert!(vision_ok("openai", "gpt-4o-mini"));
        assert!(vision_ok("openai", "qwen2.5-vl:7b"));
        assert!(vision_ok("openai", "llava:13b"));
        assert!(vision_ok("openai", "gemma3:12b"));
        assert!(vision_ok("openai", "openai/o4-mini"));
        assert!(!vision_ok("openai", "deepseek-chat"));
        assert!(!vision_ok("openai", "qwen2.5:7b"));
        assert!(!vision_ok("openai", "gpt-3.5-turbo"));
        assert!(!vision_ok("openai", ""));
        assert!(!vision_ok("gemini", "gemini-2.5-flash-preview-tts"));
    }

    #[test]
    fn image_parts_in_bodies() {
        use serde_json::json;
        let h = [json!({"role":"user","text":"看","image":"QUJD"})];
        let g = super::gemini_body("gemini-2.5-flash", "s", &h);
        assert_eq!(g["contents"][0]["parts"][0]["inline_data"]["data"], "QUJD");
        assert_eq!(g["contents"][0]["parts"][1]["text"], "看");
        let o = super::openai_body("gpt-4o", "s", &h);
        assert_eq!(o["messages"][1]["content"][1]["image_url"]["url"], "data:image/png;base64,QUJD");
        let plain = super::openai_body("gpt-4o", "s", &[json!({"role":"user","text":"hi"})]);
        assert_eq!(plain["messages"][1]["content"], "hi");
    }

    #[test]
    fn chat_log_to_markdown() {
        use serde_json::json;
        let items = [
            json!({"role":"user","text":"早上好","t":1000.0}),
            json!({"role":"model","text":"[happy]早安喵～\n今天也要加油","t":2000.0}),
            json!({"role":"user","text":"晚安","t":90_000_000.0}),
        ];
        let md = super::log_markdown(&items, "2026-11-21 15:00", |t| if t < 1e7 { "11-20 08:00".into() } else { "11-21 23:00".into() });
        assert!(md.starts_with("# 和哈基米的聊天记录"));
        assert!(md.contains("共 3 条"));
        assert!(md.contains("## 11-20"));
        assert!(md.contains("## 11-21"));
        assert!(md.contains("**你** 08:00\n\n早上好"));
        assert!(md.contains("早安喵～  \n今天也要加油"), "{md}");
        assert!(!md.contains("[happy]"));
    }

    #[test]
    fn search_only_for_gemini_and_live_questions() {
        assert!(super::wants_search("gemini-3.8-flash-tiered", "帮我查一下今日金价"));
        assert!(super::wants_search("Gemini-2.5-Flash", "现在美元汇率多少"));
        assert!(!super::wants_search("gemini-3.8-flash", "哈基米你好可爱"));
        assert!(!super::wants_search("gpt-4o-mini", "今天金价多少"));
        let mut b = super::openai_body("gemini-x", "s", &[]);
        super::add_search(&mut b);
        assert_eq!(b["tools"][0]["google_search"], serde_json::json!({}));
    }

    use super::*;

    fn ctx() -> ChatCtx { ChatCtx { hunger: 50, mood: 60, energy: 70, level: "朋友".into(), days: 3 } }

    #[test]
    fn persona_text() {
        let mut c = crate::core::config::Config::default();
        let p = persona_at(&c, &ctx(), (11, 20, 9, 5));
        assert!(p.contains("称呼用户为「你」。现在是 11月20日 9:05。"));
        assert!(p.contains("饱食 50/100，心情 60/100，精力 70/100，和用户的关系是「朋友」，已经认识 3 天。状态会影响语气"));
        assert_eq!(p.lines().count(), 7);
        c.user_name = "主人".into();
        assert!(persona_at(&c, &ctx(), (1, 2, 13, 30)).contains("称呼用户为「主人」。现在是 1月2日 13:30。"));
    }

    #[test]
    fn bodies() {
        let h = vec![json!({"role":"user","text":"hi","t":1}), json!({"role":"model","text":"喵","t":2})];
        let g = gemini_body("gemini-2.5-flash", "sys", &h);
        assert_eq!(g["contents"][1]["role"], "model");
        assert_eq!(g["contents"][0]["parts"][0]["text"], "hi");
        assert_eq!(g["generationConfig"]["thinkingConfig"]["thinkingBudget"], 0);
        assert_eq!(g["generationConfig"]["maxOutputTokens"], CHAT_MAX_TOKENS);
        // 大小写不敏感
        assert_eq!(gemini_body("Gemini-2.5-Flash-Lite", "sys", &h)["generationConfig"]["thinkingConfig"]["thinkingBudget"], 0);
        // Gemini 2.5 Pro：最低允许 128，且留足 maxOutputTokens 防 MAX_TOKENS 空回复
        let g25p = gemini_body("gemini-2.5-pro", "sys", &h);
        assert_eq!(g25p["generationConfig"]["thinkingConfig"]["thinkingBudget"], 128);
        assert_eq!(g25p["generationConfig"]["maxOutputTokens"], REASONING_MAX_TOKENS);
        // Gemini 3 思考模型：设 thinkingLevel="low" 降延迟，同时留足 maxOutputTokens
        let g3 = gemini_body("gemini-3-flash-preview", "sys", &h);
        assert_eq!(g3["generationConfig"]["thinkingConfig"]["thinkingLevel"], "low");
        assert_eq!(g3["generationConfig"]["maxOutputTokens"], REASONING_MAX_TOKENS);
        assert!(gemini_body("gemini-2.0-pro", "sys", &h)["generationConfig"].get("thinkingConfig").is_none());
        let o = openai_body("gpt-4o-mini", "sys", &h);
        assert_eq!(o["messages"][0]["role"], "system");
        assert_eq!(o["messages"][2]["role"], "assistant");
        assert_eq!(o["max_tokens"], CHAT_MAX_TOKENS);
        // qwen2.5 不是思考模型，不应误判
        assert_eq!(openai_body("qwen2.5-72b-instruct", "sys", &h)["max_tokens"], CHAT_MAX_TOKENS);
        // 经 OpenAI 兼容中转的思考型模型也留足 max_tokens
        assert_eq!(openai_body("gemini-3.8-flash-tiered", "sys", &h)["max_tokens"], REASONING_MAX_TOKENS);
        assert_eq!(openai_body("deepseek-reasoner", "sys", &h)["max_tokens"], REASONING_MAX_TOKENS);
        assert_eq!(openai_body("openai/o3-mini", "sys", &h)["max_tokens"], REASONING_MAX_TOKENS);
    }

    #[test]
    fn parse_replies() {
        let g = json!({"candidates":[{"content":{"parts":[{"text":" 你好"},{"text":"喵 "}]}}]});
        assert_eq!(parse_gemini(&g).unwrap(), "你好喵");
        // 思考型模型的 thought part 不应混进正文，也不应单独当有效正文
        let g_thought = json!({"candidates":[{"content":{"parts":[{"text":"内心推理","thought":true},{"text":"你好喵"}]}}]});
        assert_eq!(parse_gemini(&g_thought).unwrap(), "你好喵");
        let g_only_thought = json!({"candidates":[{"content":{"parts":[{"text":"只有思考","thought":true}]}}]});
        assert!(parse_gemini(&g_only_thought).is_err());
        // 纯空白不算有效回复
        let g_ws = json!({"candidates":[{"content":{"parts":[{"text":"   \n\t "}]}}]});
        assert!(parse_gemini(&g_ws).is_err());
        let e = parse_gemini(&json!({"candidates":[{"finishReason":"SAFETY"}]})).unwrap_err();
        assert_eq!(e.detail, "空回复（SAFETY）");
        assert_eq!(parse_openai(&json!({"choices":[{"message":{"content":"ok\n"}}]})).unwrap(), "ok");
        assert!(parse_openai(&json!({"choices":[{"message":{"content":"   \n"}}]})).is_err());
        assert!(parse_openai(&json!({})).is_err());
    }

    #[test]
    fn error_texts() {
        let e = |kind| AiError { kind, detail: String::new() }.user_text();
        assert_eq!(e(ErrKind::Status(401)), ERR_KEY);
        assert_eq!(e(ErrKind::Status(403)), ERR_KEY);
        assert_eq!(e(ErrKind::Status(429)), ERR_429);
        assert_eq!(e(ErrKind::Status(500)), ERR_GENERIC);
        assert_eq!(e(ErrKind::Timeout), ERR_TIMEOUT);
        assert_eq!(e(ErrKind::Network), ERR_NET);
    }

    #[test]
    fn status_texts_and_redact() {
        assert!(explain_status(401, "").starts_with("API Key 不对"));
        assert!(explain_status(400, "API key not valid").starts_with("API Key 不对"));
        assert!(explain_status(404, "model not found").contains("模型名"));
        assert!(explain_status(402, "").contains("余额"));
        assert!(explain_status(503, "").contains("服务器"));
        assert_eq!(redact("bad key sk-abcdef123 here", "sk-abcdef123"), "bad key *** here");
        assert!(redact(&"x".repeat(200), "k").chars().count() <= 91);
        let p = TestParams { provider: "openai".into(), base_url: String::new(), api_key: " ".into(), model: "m".into(), proxy: String::new() };
        assert_eq!(test_connection(&p).unwrap_err(), "还没填 API Key");
    }

    #[test]
    fn models_list_parsing() {
        let t = r#"{"object":"list","data":[{"id":"qwen3:8b"},{"id":"nomic-embed-text:latest"},{"id":"llama3.2:3b"},{"id":"qwen3:8b"}]}"#;
        assert_eq!(parse_models(t).unwrap(), vec!["llama3.2:3b".to_string(), "qwen3:8b".to_string()]);
        assert!(parse_models("not json").is_none());
        assert!(parse_models(r#"{"error":"x"}"#).is_none());
    }

    /// 本机起一个假的 OpenAI 兼容服务（像 Ollama），走真网络栈：列模型 + 测试连接都不要 Key、不走代理
    #[test]
    fn local_server_end_to_end() {
        use std::io::Write;
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for _ in 0..2 {
                let Ok((mut s, _)) = l.accept() else { return };
                let req = super::super::net::read_http_request(&mut s);
                let body = if req.starts_with("GET /v1/models") {
                    r#"{"data":[{"id":"qwen3:4b"},{"id":"qwen3:8b"}]}"#
                } else {
                    r#"{"choices":[{"message":{"content":"喵"}}]}"#
                };
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        let p = TestParams { provider: "openai".into(), base_url: format!("http://127.0.0.1:{port}/v1"), api_key: String::new(), model: "qwen3:8b".into(), proxy: "http://127.0.0.1:1".into() };
        assert_eq!(list_models(&p).unwrap(), vec!["qwen3:4b".to_string(), "qwen3:8b".to_string()]);
        let (_, reply) = test_connection(&p).unwrap();
        assert_eq!(reply, "喵");
    }

    #[test]
    fn local_needs_no_key() {
        let (_, _, h) = endpoint("openai", "http://127.0.0.1:11434/v1", "qwen3:8b", "");
        assert_eq!(h, "Bearer local");
        // 本机没开服务：不报「没填 Key」，而是说连不上（端口 9 基本不会有人监听）
        let p = TestParams { provider: "openai".into(), base_url: "http://127.0.0.1:9/v1".into(), api_key: String::new(), model: "m".into(), proxy: String::new() };
        assert!(test_connection(&p).unwrap_err().contains("Ollama"));
    }

    #[test]
    fn encode() {
        assert_eq!(encode_component("gemini-2.5-flash"), "gemini-2.5-flash");
        assert_eq!(encode_component("a b/c"), "a%20b%2Fc");
    }
}
