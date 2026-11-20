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

/// 缓存一个 Agent（代理+超时没变就复用，省得每次重建 TLS 配置和连接池）
static CACHE: Mutex<Option<(String, u64, ureq::Agent)>> = Mutex::new(None);

/// 带代理、带超时（整次请求 timeout_secs 秒）的 Agent。非 2xx 不当错误，调用方自己看 status。
pub fn agent(timeout_secs: u64) -> ureq::Agent {
    let proxy = current_proxy();
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((p, t, a)) = cache.as_ref()
        && *p == proxy
        && *t == timeout_secs
    {
        return a.clone();
    }
    let a = build(&proxy, timeout_secs);
    *cache = Some((proxy, timeout_secs, a.clone()));
    a
}

fn build(proxy: &str, timeout_secs: u64) -> ureq::Agent {
    let p = if proxy.is_empty() {
        None
    } else {
        match ureq::Proxy::new(proxy) {
            Ok(p) => Some(p),
            Err(e) => {
                crate::core::log::log(&format!("proxy ignored ({proxy}): {e}"));
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
pub fn agent_with_proxy(setting: &str, timeout_secs: u64) -> ureq::Agent {
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
    let pick = if s.contains('=') {
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

/// POST JSON，返回 (HTTP 状态码, 响应文本)。非 2xx 也返回 Ok，由调用方判断。
/// headers 里可能有 API Key，这里不打日志。
pub fn post_json(url: &str, headers: &[(&str, &str)], body: &serde_json::Value, timeout_secs: u64) -> Result<(u16, String), ureq::Error> {
    post_json_via(&agent(timeout_secs), url, headers, body)
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

/// GET，返回 (HTTP 状态码, 响应文本)。响应体最多 limit 字节
pub fn get_text(url: &str, timeout_secs: u64, limit: u64) -> Result<(u16, String), ureq::Error> {
    let mut res = agent(timeout_secs).get(url).call()?;
    let status = res.status().as_u16();
    let text = res.body_mut().with_config().limit(limit).read_to_string()?;
    Ok((status, text))
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
        assert_eq!(normalize("http=127.0.0.1:1;https=127.0.0.1:2;socks=127.0.0.1:3").as_deref(), Some("http://127.0.0.1:2"));
        assert_eq!(normalize("http=127.0.0.1:1").as_deref(), Some("http://127.0.0.1:1"));
        assert_eq!(normalize("socks=127.0.0.1:3"), None);
        assert_eq!(normalize("socks5://127.0.0.1:3"), None);
        assert_eq!(normalize("  "), None);
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
