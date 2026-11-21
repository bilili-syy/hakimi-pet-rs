//! HTTP：ureq Agent，带代理。
//! 代理优先级：config.proxy 非空用它；为空则读注册表
//! HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings 的 ProxyEnable/ProxyServer
//! （跟随系统代理，Clash 之类开了「系统代理」即可），都没有就直连。
//! 只支持 http/https 代理（ureq 没开 socks-proxy feature）；PAC（AutoConfigURL）不支持。
#![allow(dead_code)]
use std::sync::Mutex;
use std::time::Duration;

/// 连接超时（全局超时由调用方给）
const CONNECT_TIMEOUT_SECS: u64 = 10;

/// 按代理缓存 Agent（每个代理复用一份 TLS 配置和连接池；请求超时按单次请求设置，不因超时不同而重建）
static CACHE: Mutex<Vec<(String, ureq::Agent)>> = Mutex::new(Vec::new());

/// 脱敏代理地址里的 `user:pass@` 凭据（写日志用）
pub fn redact_proxy(proxy: &str) -> String {
    if let Some((head, tail)) = proxy.split_once("://")
        && let Some((_, host)) = tail.rsplit_once('@')
    {
        return format!("{head}://***@{host}");
    }
    if let Some((_, host)) = proxy.rsplit_once('@') {
        return format!("***@{host}");
    }
    proxy.to_string()
}

/// 目标 URL 指向本机或局域网（Ollama / LM Studio 等）：一律直连不走代理
pub fn is_loopback_url(url: &str) -> bool { crate::core::config::is_local_url(url) }

fn cached_agent(proxy: &str) -> ureq::Agent {
    {
        let cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, a)) = cache.iter().find(|(p, _)| p == proxy) {
            return a.clone();
        }
    }
    // 在锁外构建 TLS 配置和连接池，避免跨服务互相阻塞
    let a = build(proxy, 0);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, existing)) = cache.iter().find(|(p, _)| p == proxy) {
        return existing.clone();
    }
    if cache.len() >= 4 {
        cache.remove(0);
    }
    cache.push((proxy.to_string(), a.clone()));
    a
}

/// 为指定 URL 取复用的 Agent（跟随系统代理且访问 localhost/127.0.0.1 时自动直连）
fn agent_for_url(url: &str) -> ureq::Agent {
    let cfg = crate::core::config::get().proxy;
    let cfg = cfg.trim();
    // 本机 / 局域网（Ollama 等）永远直连：代理软件转发 127.0.0.1 会转到代理自己那里去
    let proxy = if is_loopback_url(url) {
        String::new()
    } else if !cfg.is_empty() {
        normalize(cfg).unwrap_or_default()
    } else {
        system_proxy().and_then(|s| normalize(&s)).unwrap_or_default()
    };
    cached_agent(&proxy)
}

/// 带代理、带超时（整次请求 timeout_secs 秒）的 Agent。非 2xx 不当错误，调用方自己看 status。
pub fn agent(timeout_secs: u64) -> ureq::Agent {
    if timeout_secs == 0 {
        cached_agent(&current_proxy())
    } else {
        build(&current_proxy(), timeout_secs)
    }
}

fn build(proxy: &str, timeout_secs: u64) -> ureq::Agent {
    let p = if proxy.is_empty() {
        None
    } else {
        match ureq::Proxy::new(proxy) {
            Ok(p) => Some(p),
            Err(e) => {
                crate::core::log::log(&format!("proxy ignored ({}): {e}", redact_proxy(proxy)));
                None
            }
        }
    };
    let t = if timeout_secs == 0 { None } else { Some(Duration::from_secs(timeout_secs)) };
    ureq::Agent::config_builder()
        .proxy(p)
        .timeout_global(t)
        .timeout_connect(Some(Duration::from_secs(CONNECT_TIMEOUT_SECS)))
        .http_status_as_error(false)
        .user_agent(concat!("hakimi-pet/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// 按「设置窗口里还没保存的代理设置」建 Agent（测试连接用）。setting 为空 = 跟随系统代理
pub fn agent_with_proxy(setting: &str, timeout_secs: u64, url: &str) -> ureq::Agent {
    if is_loopback_url(url) { return build("", timeout_secs); }
    let s = setting.trim();
    let proxy = if s.is_empty() { system_proxy().and_then(|p| normalize(&p)) } else { normalize(s) };
    build(&proxy.unwrap_or_default(), timeout_secs)
}

/// 现在该用的代理地址（"" = 直连）
pub fn current_proxy() -> String {
    let cfg = crate::core::config::get().proxy;
    let cfg = cfg.trim();
    if !cfg.is_empty() {
        return normalize(cfg).unwrap_or_default();
    }
    system_proxy().and_then(|s| normalize(&s)).unwrap_or_default()
}

/// 把 Electron proxyRules / 注册表 ProxyServer 的写法转成 ureq 认的 URL：
/// - `127.0.0.1:7890` → `http://127.0.0.1:7890`
/// - `http=127.0.0.1:7890;https=127.0.0.1:7890;socks=…` → 取 https，其次 http
/// - socks 代理不支持 → None
pub fn normalize(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() { return None; }
    // 仅当不含 "://" 时才按 Windows 注册表 `http=host:port;https=host:port` 解析，
    // 避免带 `=` 的代理密码 URL（如 base64 密码）被误判
    let pick = if !s.contains("://") && s.contains('=') {
        let mut http = None;
        let mut https = None;
        for part in s.split(';') {
            if let Some((k, v)) = part.split_once('=') {
                match k.trim().to_ascii_lowercase().as_str() {
                    "https" => https = Some(v.trim()),
                    "http" => http = Some(v.trim()),
                    _ => {}
                }
            }
        }
        https.or(http)?
    } else {
        s.split(';').next().unwrap_or("").trim()
    };
    if pick.is_empty() { return None; }
    let lower = pick.to_ascii_lowercase();
    if lower.starts_with("socks") { return None; }
    if lower.starts_with("http://") || lower.starts_with("https://") {
        Some(pick.to_string())
    } else {
        Some(format!("http://{pick}"))
    }
}

/// 读系统代理（ProxyEnable=1 时返回 ProxyServer）
#[cfg(windows)]
pub fn system_proxy() -> Option<String> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::w;
    let key = w!(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings");
    let mut enable: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: 缓冲区和长度都是本地变量，RegGetValueW 只写入 size 字节
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key, w!("ProxyEnable"), RRF_RT_REG_DWORD, None,
            Some(&mut enable as *mut u32 as *mut _), Some(&mut size))
    };
    if r.is_err() || enable == 0 { return None; }
    let mut buf = [0u16; 1024];
    let mut size = std::mem::size_of_val(&buf) as u32;
    // SAFETY: 同上，size 是 buf 的字节数
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key, w!("ProxyServer"), RRF_RT_REG_SZ, None,
            Some(buf.as_mut_ptr() as *mut _), Some(&mut size))
    };
    if r.is_err() { return None; }
    let n = (size as usize / 2).min(buf.len());
    let s = String::from_utf16_lossy(&buf[..n]);
    let s = s.trim_end_matches('\0').trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

#[cfg(not(windows))]
pub fn system_proxy() -> Option<String> { None }

/// 响应体上限（TTS 返回 base64 wav，一句话通常几百 KB）
pub const MAX_BODY: u64 = 32 * 1024 * 1024;

fn req_timeout(timeout_secs: u64) -> Option<Duration> {
    if timeout_secs == 0 { None } else { Some(Duration::from_secs(timeout_secs)) }
}

/// POST JSON，返回 (HTTP 状态码, 响应文本)。非 2xx 也返回 Ok，由调用方判断。
/// headers 里可能有 API Key，这里不打日志。
pub fn post_json(url: &str, headers: &[(&str, &str)], body: &serde_json::Value, timeout_secs: u64) -> Result<(u16, String), ureq::Error> {
    let agent = agent_for_url(url);
    let mut req = agent.post(url).config().timeout_global(req_timeout(timeout_secs)).build().header("Content-Type", "application/json");
    for (k, v) in headers { req = req.header(*k, *v); }
    let mut res = req.send(serde_json::to_vec(body).unwrap_or_default())?;
    let status = res.status().as_u16();
    let text = res.body_mut().with_config().limit(MAX_BODY).read_to_string()?;
    Ok((status, text))
}

/// 同 post_json，但用指定的 Agent
pub fn post_json_via(agent: &ureq::Agent, url: &str, headers: &[(&str, &str)], body: &serde_json::Value) -> Result<(u16, String), ureq::Error> {
    let mut req = agent.post(url).header("Content-Type", "application/json");
    for (k, v) in headers { req = req.header(*k, *v); }
    let mut res = req.send(serde_json::to_vec(body).unwrap_or_default())?;
    let status = res.status().as_u16();
    let text = res.body_mut().with_config().limit(MAX_BODY).read_to_string()?;
    Ok((status, text))
}

/// POST JSON，返回 (HTTP 状态码, 响应体读取器)：流式（SSE）边收边读用。非 2xx 也返回 Ok。
/// 读取器丢掉就断开连接（可以用来真正掐断不要了的请求）。headers 里可能有 API Key，这里不打日志。
pub fn post_json_stream(url: &str, headers: &[(&str, &str)], body: &serde_json::Value, timeout_secs: u64) -> Result<(u16, ureq::BodyReader<'static>), ureq::Error> {
    let agent = agent_for_url(url);
    let mut req = agent
        .post(url)
        .config()
        .timeout_global(req_timeout(timeout_secs))
        .build()
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream");
    for (k, v) in headers { req = req.header(*k, *v); }
    let res = req.send(serde_json::to_vec(body).unwrap_or_default())?;
    let status = res.status().as_u16();
    let reader = res.into_body().into_with_config().limit(MAX_BODY).reader();
    Ok((status, reader))
}

/// GET，返回 (HTTP 状态码, 响应文本)。响应体最多 limit 字节
pub fn get_text(url: &str, timeout_secs: u64, limit: u64) -> Result<(u16, String), ureq::Error> {
    let agent = agent_for_url(url);
    let mut res = agent.get(url).config().timeout_global(req_timeout(timeout_secs)).build().call()?;
    let status = res.status().as_u16();
    let text = res.body_mut().with_config().limit(limit).read_to_string()?;
    Ok((status, text))
}

/// 遮蔽错误信息里可能回显的 Key
pub fn redact_key(msg: &str, key: &str) -> String {
    let k = key.trim();
    if k.len() >= 6 { msg.replace(k, "***") } else { msg.to_string() }
}

/// 网络类错误（连不上/DNS/TLS/代理），对应原版 `net::ERR_*` / `fetch failed`
pub fn is_network_error(e: &ureq::Error) -> bool {
    matches!(
        e,
        ureq::Error::Io(_)
            | ureq::Error::HostNotFound
            | ureq::Error::ConnectionFailed
            | ureq::Error::Tls(_)
            | ureq::Error::Rustls(_)
            | ureq::Error::ConnectProxyFailed(_)
            | ureq::Error::InvalidProxyUrl
    )
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn proxy_formats() {
        assert_eq!(normalize("127.0.0.1:7890").as_deref(), Some("http://127.0.0.1:7890"));
        assert_eq!(normalize("http://127.0.0.1:7890").as_deref(), Some("http://127.0.0.1:7890"));
        assert_eq!(normalize("http://user:p=ss@127.0.0.1:7890").as_deref(), Some("http://user:p=ss@127.0.0.1:7890"));
        assert_eq!(normalize("http=127.0.0.1:1;https=127.0.0.1:2;socks=127.0.0.1:3").as_deref(), Some("http://127.0.0.1:2"));
        assert_eq!(normalize("http=127.0.0.1:1").as_deref(), Some("http://127.0.0.1:1"));
        assert_eq!(normalize("socks=127.0.0.1:3"), None);
        assert_eq!(normalize("socks5://127.0.0.1:3"), None);
        assert_eq!(normalize("  "), None);
        assert_eq!(super::redact_proxy("http://user:secret@127.0.0.1:7890"), "http://***@127.0.0.1:7890");
        assert_eq!(super::redact_proxy("http://127.0.0.1:7890"), "http://127.0.0.1:7890");
        assert!(super::is_loopback_url("http://127.0.0.1:11434/v1"));
        assert!(super::is_loopback_url("http://localhost:8080/v1"));
        assert!(super::is_loopback_url("http://[::1]:11434/v1"));
        assert!(!super::is_loopback_url("https://localhost.example.com/v1"));
        assert!(!super::is_loopback_url("https://api.openai.com/v1"));
    }
}

#[cfg(test)]
mod sys_tests {
    #[test]
    fn system_proxy_does_not_panic() {
        let p = super::system_proxy();
        if let Some(s) = p { assert!(!s.contains('\0')); }
    }

    /// 真联网（走系统代理）：假 Key 打 MiMo TTS（国内直连可达），应该拿到 4xx，而不是 TLS/网络错误。
    /// `cargo test -- --ignored live_mimo` 手动跑。不读写 config.json。
    #[test]
    #[ignore]
    fn live_mimo_bad_key() {
        let proxy = super::system_proxy().and_then(|s| super::normalize(&s)).unwrap_or_default();
        let agent = super::build(&proxy, 30);
        let body = serde_json::json!({"model":"mimo-v2.5-tts","messages":[{"role":"assistant","content":"喵"}],"audio":{"format":"wav","voice":"冰糖"}});
        let r = super::post_json_via(&agent, crate::services::tts::API, &[("api-key", "invalid-key-for-test")], &body);
        match r {
            Ok((status, _)) => assert!((400..500).contains(&status), "status {status}"),
            Err(e) => panic!("network error via proxy {proxy:?}: {e}"),
        }
    }
}
