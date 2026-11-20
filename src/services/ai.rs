//! AI 聊天：移植 Electron 版 src/main/ai.js（persona 提示词、Gemini/OpenAI 兼容、20 条记忆、错误文案）
//! 配置读写在 core::config；网络/代理在 services::net。
#![allow(dead_code)]
use crate::types::{AppEvent, ChatReply, EventSender};
use serde_json::{Value, json};
use std::sync::Mutex;

/// 记住最近 20 条（10 轮）
pub const MAX_HISTORY: usize = 20;
/// 原版 AbortController 30 秒
const TIMEOUT_SECS: u64 = 30;

const NEED_KEY: &str = "哈基米的大脑还没接上喵……右键我选「设置」，把 Gemini API Key 填进去就能聊天啦！";
const ERR_GENERIC: &str = "哈基米的脑袋卡住了喵……等会儿再问我一次？";
const ERR_TIMEOUT: &str = "网络好慢喵……30 秒都没等到回音。是不是代理没开？";
const ERR_KEY: &str = "API Key 好像不对喵……右键「设置」检查一下？";
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

/// 同一时间只处理一条聊天（chat.json 读改写不打架）
static BUSY: Mutex<()> = Mutex::new(());

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
    if c.api_key.is_empty() {
        return ChatReply { ok: false, need_key: true, text: NEED_KEY.into() };
    }
    let _g = BUSY.lock().unwrap_or_else(|e| e.into_inner());
    let mut history = load_history();
    history.push(json!({ "role": "user", "text": user_text, "t": now_ms() }));
    let start = history.len().saturating_sub(MAX_HISTORY);
    let system = persona(&c, ctx);
    let r = if c.provider == "openai" {
        call_openai(&c, &system, &history[start..])
    } else {
        call_gemini(&c, &system, &history[start..])
    };
    match r {
        Ok(reply) => {
            history.push(json!({ "role": "model", "text": reply, "t": now_ms() }));
            save_history(&history);
            ChatReply { ok: true, need_key: false, text: reply }
        }
        Err(e) => {
            crate::core::log::log(&format!("ai error {}", e.detail));
            ChatReply { ok: false, need_key: false, text: e.user_text().into() }
        }
    }
}

/// 清空 chat.json（原版 saveHistory([])，写一个空数组）
pub fn clear_history() {
    let _g = BUSY.lock().unwrap_or_else(|e| e.into_inner());
    save_history(&[]);
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
    persona_at(c, ctx, local_time())
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

fn post(url: &str, headers: &[(&str, &str)], body: &Value) -> Result<Value, AiError> {
    let (status, text) = super::net::post_json(url, headers, body, TIMEOUT_SECS).map_err(|e| {
        let kind = if matches!(e, ureq::Error::Timeout(_)) {
            ErrKind::Timeout
        } else if super::net::is_network_error(&e) {
            ErrKind::Network
        } else {
            ErrKind::Other
        };
        AiError { kind, detail: e.to_string() }
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
        return Err(AiError { kind: ErrKind::Status(status), detail: format!("HTTP {status}: {msg}") });
    }
    json.ok_or_else(|| AiError::other("bad json"))
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

pub fn gemini_body(model: &str, system: &str, history: &[Value]) -> Value {
    let contents: Vec<Value> = history
        .iter()
        .map(|m| json!({ "role": if is_user(m) { "user" } else { "model" }, "parts": [{ "text": msg_text(m) }] }))
        .collect();
    let mut body = json!({
        "system_instruction": { "parts": [{ "text": system }] },
        "contents": contents,
        "generationConfig": { "temperature": 0.9, "maxOutputTokens": 300 },
    });
    if model.contains("2.5-flash") {
        // 桌宠聊天不需要深度思考，省时间
        body["generationConfig"]["thinkingConfig"] = json!({ "thinkingBudget": 0 });
    }
    body
}

fn call_gemini(c: &crate::core::config::Config, system: &str, history: &[Value]) -> Result<String, AiError> {
    let base = if c.base_url.is_empty() { "https://generativelanguage.googleapis.com/v1beta" } else { c.base_url.as_str() };
    let base = base.strip_suffix('/').unwrap_or(base);
    let url = format!("{base}/models/{}:generateContent", encode_component(&c.model));
    let json = post(&url, &[("x-goog-api-key", &c.api_key)], &gemini_body(&c.model, system, history))?;
    parse_gemini(&json)
}

pub fn parse_gemini(json: &Value) -> Result<String, AiError> {
    let cand = json.get("candidates").and_then(|c| c.get(0));
    let text: String = cand
        .and_then(|c| c.get("content"))
        .and_then(|c| c.get("parts"))
        .and_then(Value::as_array)
        .map(|parts| parts.iter().map(|p| p.get("text").and_then(Value::as_str).unwrap_or("")).collect())
        .unwrap_or_default();
    if text.is_empty() {
        let reason = cand.and_then(|c| c.get("finishReason")).and_then(Value::as_str);
        return Err(AiError::other(match reason {
            Some(r) => format!("空回复（{r}）"),
            None => "空回复".into(),
        }));
    }
    Ok(text.trim().to_string())
}

pub fn openai_body(model: &str, system: &str, history: &[Value]) -> Value {
    let mut messages = vec![json!({ "role": "system", "content": system })];
    messages.extend(
        history.iter().map(|m| json!({ "role": if is_user(m) { "user" } else { "assistant" }, "content": msg_text(m) })),
    );
    json!({ "model": model, "temperature": 0.9, "max_tokens": 300, "messages": messages })
}

fn call_openai(c: &crate::core::config::Config, system: &str, history: &[Value]) -> Result<String, AiError> {
    let base = if c.base_url.is_empty() { "https://api.openai.com/v1" } else { c.base_url.as_str() };
    let base = base.strip_suffix('/').unwrap_or(base);
    let auth = format!("Bearer {}", c.api_key);
    let json = post(&format!("{base}/chat/completions"), &[("Authorization", &auth)], &openai_body(&c.model, system, history))?;
    parse_openai(&json)
}

pub fn parse_openai(json: &Value) -> Result<String, AiError> {
    let text = json
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if text.is_empty() { return Err(AiError::other("空回复")); }
    Ok(text.trim().to_string())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
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
        assert_eq!(g["generationConfig"]["maxOutputTokens"], 300);
        assert!(gemini_body("gemini-2.0-pro", "sys", &h)["generationConfig"].get("thinkingConfig").is_none());
        let o = openai_body("gpt-4o-mini", "sys", &h);
        assert_eq!(o["messages"][0]["role"], "system");
        assert_eq!(o["messages"][2]["role"], "assistant");
        assert_eq!(o["max_tokens"], 300);
    }

    #[test]
    fn parse_replies() {
        let g = json!({"candidates":[{"content":{"parts":[{"text":" 你好"},{"text":"喵 "}]}}]});
        assert_eq!(parse_gemini(&g).unwrap(), "你好喵");
        let e = parse_gemini(&json!({"candidates":[{"finishReason":"SAFETY"}]})).unwrap_err();
        assert_eq!(e.detail, "空回复（SAFETY）");
        assert_eq!(parse_openai(&json!({"choices":[{"message":{"content":"ok\n"}}]})).unwrap(), "ok");
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
    fn encode() {
        assert_eq!(encode_component("gemini-2.5-flash"), "gemini-2.5-flash");
        assert_eq!(encode_component("a b/c"), "a%20b%2Fc");
    }
}
