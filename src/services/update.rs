//! 检查更新：读 GitHub 上最新的 Release，和自己的版本号比
pub const RELEASES_URL: &str = "https://github.com/bilili-syy/hakimi-pet-rs/releases/latest";
const API_URL: &str = "https://api.github.com/repos/bilili-syy/hakimi-pet-rs/releases/latest";

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// 返回最新版本的 tag（如 "v1.2.0"），以及它是否比当前版本新
pub fn check() -> Result<(String, bool), String> {
    let (status, text) = super::net::get_text(API_URL, 15, 256 * 1024).map_err(|e| {
        if super::net::is_network_error(&e) { "连不上 GitHub，检查一下网络或代理".to_string() } else { format!("请求失败：{e}") }
    })?;
    if status == 404 { return Err("还没有发布过版本".into()); }
    if status != 200 { return Err(format!("GitHub 返回 {status}，稍后再试")); }
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|_| "GitHub 返回的内容看不懂".to_string())?;
    let tag = v.get("tag_name").and_then(|t| t.as_str()).unwrap_or("").trim().to_string();
    if tag.is_empty() { return Err("没读到版本号".into()); }
    let newer = is_newer(&tag, CURRENT);
    Ok((tag, newer))
}

/// "v1.2.0" > "1.1.0"？正式版 > 同版本号的预发布版（如 1.2.0 > 1.2.0-beta.1）
pub fn is_newer(tag: &str, cur: &str) -> bool { parse(tag) > parse(cur) }

fn parse(s: &str) -> (u32, u32, u32, bool) {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let s = s.split('+').next().unwrap_or("");
    let (core, pre) = match s.split_once('-') {
        Some((c, p)) => (c, !p.is_empty()),
        None => (s, false),
    };
    let mut it = core.split('.').map(|x| x.parse::<u32>().unwrap_or(0));
    let (a, b, c) = (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(0));
    let valid = a > 0 || b > 0 || c > 0;
    // release = true > prerelease = false；无法解析的视为 (0,0,0,false)
    (a, b, c, valid && !pre)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_newer("v1.2.0", "1.1.0"));
        assert!(is_newer("v1.1.1", "1.1.0"));
        assert!(is_newer("2.0", "1.9.9"));
        assert!(is_newer("v1.2.0", "1.2.0-beta.1"));
        assert!(!is_newer("v1.2.0-beta.1", "1.2.0"));
        assert!(!is_newer("v1.2-1", "1.2.0"));
        assert!(!is_newer("v1.1.0", "1.1.0"));
        assert!(!is_newer("v1.0.9", "1.1.0"));
        assert!(!is_newer("garbage", "1.1.0"));
    }
}
