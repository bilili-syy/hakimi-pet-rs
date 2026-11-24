//! 检查更新：读 GitHub 上最新的 Release，和自己的版本号比
use std::sync::Mutex;

pub const RELEASES_URL: &str = "https://github.com/bilili-syy/hakimi-pet-rs/releases/latest";
const API_URL: &str = "https://api.github.com/repos/bilili-syy/hakimi-pet-rs/releases/latest";
const ALLOWED_URL_PREFIX: &str = "https://github.com/bilili-syy/hakimi-pet-rs/";

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// 最新 Release 信息
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub tag: String,
    pub url: String,
    /// 从 Release 正文提取的简短更新摘要（最多几行纯文本，用于弹窗展示）
    pub notes: String,
}

/// 一次检查更新的完整结果
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateCheck {
    pub info: ReleaseInfo,
    pub newer: bool,
}

static LATEST_NEWER: Mutex<Option<ReleaseInfo>> = Mutex::new(None);

/// 本次运行期间如果已经发现过更新版本，返回它的信息
pub fn cached_newer() -> Option<ReleaseInfo> {
    LATEST_NEWER.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// 拉取 GitHub 最新 Release 详情并与当前版本比较；发现新版本会自动缓存供菜单/设置页读取
pub fn check_info() -> Result<UpdateCheck, String> {
    let (status, text) = super::net::get_text(API_URL, 15, 256 * 1024).map_err(|e| {
        if super::net::is_network_error(&e) {
            "连不上 GitHub，检查一下网络或代理".to_string()
        } else {
            format!("请求失败：{e}")
        }
    })?;
    if status == 404 {
        return Err("还没有发布过版本".into());
    }
    if status != 200 {
        return Err(format!("GitHub 返回 {status}，稍后再试"));
    }
    let chk = parse_release_json(&text, CURRENT)?;
    if chk.newer {
        *LATEST_NEWER.lock().unwrap_or_else(|e| e.into_inner()) = Some(chk.info.clone());
    }
    Ok(chk)
}

pub fn parse_release_json(text: &str, cur: &str) -> Result<UpdateCheck, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|_| "GitHub 返回的内容看不懂".to_string())?;
    let tag = v.get("tag_name").and_then(|t| t.as_str()).unwrap_or("").trim().to_string();
    if tag.is_empty() {
        return Err("没读到版本号".into());
    }
    let raw_url = v.get("html_url").and_then(|u| u.as_str()).unwrap_or("").trim();
    let url = if raw_url.starts_with(ALLOWED_URL_PREFIX) {
        raw_url.to_string()
    } else {
        RELEASES_URL.to_string()
    };
    let body = v.get("body").and_then(|b| b.as_str()).unwrap_or("");
    let notes = summarize_notes(body);
    let newer = is_newer(&tag, cur);
    Ok(UpdateCheck { info: ReleaseInfo { tag, url, notes }, newer })
}

/// 提取最多 4 行可读更新说明（去掉常见 Markdown 标记，避免弹窗过长）
pub fn summarize_notes(body: &str) -> String {
    let mut out = Vec::new();
    let mut total_chars = 0usize;
    for raw in body.lines() {
        let s = raw.trim().trim_start_matches(['#', '>', ' ']).trim();
        if s.is_empty() || s.starts_with("---") || s.starts_with("```") {
            continue;
        }
        let cleaned = s.replace("**", "").replace('`', "");
        let line: String = cleaned.chars().take(60).collect();
        if line.is_empty() {
            continue;
        }
        total_chars += line.chars().count();
        out.push(line);
        if out.len() >= 4 || total_chars >= 180 {
            break;
        }
    }
    out.join("\n")
}

/// "v1.2.0" > "1.1.0"？正式版 > 同版本号的预发布版（如 1.2.0 > 1.2.0-beta.1）
pub fn is_newer(tag: &str, cur: &str) -> bool {
    parse(tag) > parse(cur)
}

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

    #[test]
    fn parse_release_validates_url_and_notes() {
        let j = r###"{
            "tag_name": "v1.5.0",
            "html_url": "https://github.com/bilili-syy/hakimi-pet-rs/releases/tag/v1.5.0",
            "body": "## 更新亮点\n- **修复** 全部立绘双尾巴\n- 新增 `检查更新` 入口\n"
        }"###;
        let chk = parse_release_json(j, "1.4.0").unwrap();
        assert!(chk.newer);
        assert_eq!(chk.info.tag, "v1.5.0");
        assert!(chk.info.url.ends_with("/v1.5.0"));
        assert!(chk.info.notes.contains("修复 全部立绘双尾巴"));
        assert!(!chk.info.notes.contains("**"));

        let bad_url = r#"{"tag_name":"v1.5.0","html_url":"https://evil.example/x","body":""}"#;
        let chk2 = parse_release_json(bad_url, "1.4.0").unwrap();
        assert_eq!(chk2.info.url, RELEASES_URL);
    }
}
