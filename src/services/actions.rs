//! 聊天受控动作与安全白名单工具（Phase 3 第 8 步）：
//! 1. AI 回复末尾或正文中可带 `[do:...]` 动作标签；`extract` 负责把所有 `[do:...]` 标签干净剥离（气泡与 TTS 绝不念出标签），
//!    并把合法标签解析为 `ChatAct` 列表交由行为层/平台层执行。
//! 2. `parse_local_direct` 对常见无歧义短指令（如「跳支舞」「换女仆装」「开 25 分钟番茄钟」「打开记事本」）支持离线直达，
//!    即使没填 API Key 也能让哈基米听懂并动手执行。
//! 3. 外部打开目标（`open:<target>`）严格走安全白名单（`resolve_open_target`）：
//!    - 只认内置安全实用工具（记事本/计算器/画图）、哈基米自身目录（歌单/数据/皮肤/周报）或 `config.json` `allowedApps` 里的合法 `.exe`；
//!    - 严禁命令行/脚本解释器（`cmd`/`powershell`/`pwsh`/`wscript`/`mshta`/`rundll32` 等）、严禁相对路径或 `..`、严禁附加任何命令行参数；
//!    - 通过白名单后仍必须弹出确认框（`platform::tool_confirm`），主人点「允许打开」才真正启动。
use crate::core::memo::{Dt, Repeat};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

/// 单轮回复最多执行的动作数（防止模型一次塞十几个动作）
const MAX_ACTS_PER_REPLY: usize = 4;

/// 聊天触发的一条受控动作
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatAct {
    /// 跳一支舞：`""` 随机，或 `"wiggle"`/`"disco"`/`"spin"`/`"march"`/`"sway"`/`"hop"`/`"song"`/`"listen"`
    Dance(String),
    /// 唱哈基米之歌 / 放 songs 文件夹里的歌
    Song,
    /// 换整套衣服：`sailor`/`hoodie`/`pajama`/`idol`/`swim`/`xmas`/`maid`/`jk`
    Costume(String),
    /// 戴或摘配饰：`("santa"|"glasses", on)`
    Outfit(String, bool),
    /// 摘掉全部配饰
    ClearOutfit,
    /// 开始 N 分钟番茄钟（1..=180）
    PomodoroStart(u32),
    /// 结束当前番茄钟
    PomodoroStop,
    /// 顺手记一条备忘或定时提醒
    Memo {
        text: String,
        time: Option<Dt>,
        repeat: Repeat,
    },
    /// 天气播报
    Weather,
    /// 去睡觉
    Sleep,
    /// 去溜达
    Walk,
    /// 喝蜂蜜水
    Honey,
    /// 展开状态卡
    Status,
    /// 逗猫棒小游戏
    Game,
    /// 爬到当前窗口顶上
    Climb,
    /// 请求打开白名单程序或文件夹（必须经白名单校验 + 用户弹窗确认）
    OpenTarget(String),
}

/// 通过白名单校验后的可打开目标
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SafeTarget {
    /// 系统 `%SystemRoot%\System32` 下的安全桌面小工具（零参数启动）
    SystemExe {
        key: &'static str,
        display: &'static str,
        path: PathBuf,
    },
    /// 哈基米自己的安全文件夹（用资源管理器打开）
    AppFolder {
        key: &'static str,
        display: &'static str,
        path: PathBuf,
    },
    /// 用户在 `config.json` `allowedApps` 显式登记的本机 `.exe`（零参数启动）
    UserExe {
        key: String,
        display: String,
        path: PathBuf,
    },
}

impl SafeTarget {
    pub fn display_name(&self) -> &str {
        match self {
            SafeTarget::SystemExe { display, .. } => display,
            SafeTarget::AppFolder { display, .. } => display,
            SafeTarget::UserExe { display, .. } => display,
        }
    }

    pub fn target_path(&self) -> &Path {
        match self {
            SafeTarget::SystemExe { path, .. }
            | SafeTarget::AppFolder { path, .. }
            | SafeTarget::UserExe { path, .. } => path,
        }
    }
}

/// 白名单拒绝原因
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RejectReason {
    /// 不在内置名单或 `allowedApps` 中
    NotWhitelisted,
    /// 属于被禁止的终端/脚本/宿主解释器，或不是合法绝对路径 `.exe`
    BlockedExecutable,
    /// 登记的 `.exe` 文件不存在
    MissingFile,
}

/// 严禁通过聊天打开的终端、脚本解释器与系统工具（全小写不含后缀）
const BLOCKED_STEMS: &[&str] = &[
    "cmd", "powershell", "pwsh", "wt", "conhost", "bash", "sh", "zsh", "wsl", "wslconfig",
    "wscript", "cscript", "mshta", "rundll32", "regsvr32", "msiexec", "installutil",
    "certutil", "bitsadmin", "curl", "wget", "Invoke-Expression", "python", "pythonw",
    "node", "deno", "bun", "perl", "ruby", "php", "schtasks", "reg", "regedit", "sc",
    "net", "net1", "netsh", "powershell_ise", "ftp", " finger", "arp", "at", " taskkill",
];

/// 拼进 AI 系统提示词末尾的受控动作说明
pub fn prompt_hint(allowed_apps: &BTreeMap<String, String>) -> String {
    let mut s = String::from(
        "【动作能力】只有当主人明确要求你做某个动作，或顺手帮主人设提醒/打开工具时，才在回复末尾附上对应标签（可不加，平时普通闲聊绝对不要加！标签不会显示给主人）：\n\
- 跳舞/唱歌：`[do:dance]`（随机跳舞）、`[do:dance:song]`（放歌跳）、`[do:dance:listen]`（听电脑音乐跳）、`[do:song]`（唱歌）\n\
- 换装/配饰：`[do:costume:maid]`（可选 `sailor`水手服/`hoodie`卫衣/`pajama`睡衣/`idol`偶像服/`swim`泳装/`xmas`圣诞裙/`maid`女仆装/`jk`）、`[do:outfit:glasses:on]`、`[do:outfit:santa:on]`、`[do:outfit:clear]`\n\
- 番茄钟/日常：`[do:pomo:25]`（开N分钟番茄钟）、`[do:pomo:stop]`、`[do:weather]`、`[do:sleep]`、`[do:walk]`、`[do:honey]`、`[do:game]`、`[do:climb]`、`[do:status]`\n\
- 记备忘/提醒：`[do:memo:YYYY-MM-DD HH:MM|none|事项]`（重复填 `none`/`daily`/`workday`/`weekly`）或 `[do:memo:note|随手记内容]`\n\
- 打开白名单工具（会先弹确认框请主人批准）：`[do:open:notepad]`（记事本）、`[do:open:calc]`（计算器）、`[do:open:mspaint]`（画图）、`[do:open:songs]`（歌单文件夹）、`[do:open:datadir]`（数据文件夹）、`[do:open:skins]`（皮肤文件夹）、`[do:open:reports]`（周报文件夹）",
    );
    if !allowed_apps.is_empty() {
        let keys: Vec<&str> = allowed_apps.keys().take(12).map(String::as_str).collect();
        s.push_str(&format!("，以及主人登记的程序：{}", keys.join("、")));
    }
    s
}

fn open_close_pair(c: char) -> Option<char> {
    match c {
        '[' => Some(']'),
        '【' => Some('】'),
        '［' => Some('］'),
        _ => None,
    }
}

fn strip_do_prefix(inner: &str) -> Option<&str> {
    let t = inner.trim();
    if t.len() < 3 {
        return None;
    }
    let head: String = t.chars().take(3).collect();
    if head.eq_ignore_ascii_case("do:") {
        Some(t[head.len()..].trim())
    } else if t.chars().take(2).collect::<String>().eq_ignore_ascii_case("do")
        && t[2..].starts_with('：')
    {
        Some(t[2 + '：'.len_utf8()..].trim())
    } else {
        None
    }
}

/// 从模型回复中剥离全部 `[do:...]` 标签，返回 `(干净文本, 解析出的动作列表)`
pub fn extract(text: &str) -> (String, Vec<ChatAct>) {
    let mut clean = String::with_capacity(text.len());
    let mut acts = Vec::new();
    let mut rest = text;

    while let Some((idx, open_ch)) = rest.char_indices().find(|&(_, c)| open_close_pair(c).is_some()) {
        clean.push_str(&rest[..idx]);
        let close_ch = open_close_pair(open_ch).unwrap();
        let after_open = &rest[idx + open_ch.len_utf8()..];
        let Some(end_rel) = after_open.find(close_ch) else {
            // 没闭合：原样保留剩余字符
            clean.push(open_ch);
            rest = after_open;
            continue;
        };
        let inner = &after_open[..end_rel];
        // 防止跨行误吞大段正文
        if inner.contains('\n') || inner.chars().count() > 120 {
            clean.push(open_ch);
            rest = after_open;
            continue;
        }
        if let Some(spec) = strip_do_prefix(inner) {
            if acts.len() < MAX_ACTS_PER_REPLY
                && let Some(act) = parse_act_spec(spec)
                && !acts.contains(&act)
            {
                acts.push(act);
            }
            rest = &after_open[end_rel + close_ch.len_utf8()..];
        } else {
            clean.push(open_ch);
            clean.push_str(inner);
            clean.push(close_ch);
            rest = &after_open[end_rel + close_ch.len_utf8()..];
        }
    }
    clean.push_str(rest);
    (clean.trim().to_string(), acts)
}

fn parse_act_spec(spec: &str) -> Option<ChatAct> {
    let spec = spec.trim();
    if spec.is_empty() {
        return None;
    }
    // memo 单独按 `memo:...` 解析，因为事项正文可能含冒号（如 18:00）
    if let Some(m) = strip_prefix_ci(spec, "memo:") {
        return parse_memo_spec(m.trim());
    }
    if let Some(t) = strip_prefix_ci(spec, "open:") {
        let key = t.trim();
        return (!key.is_empty() && key.chars().count() <= 40).then(|| ChatAct::OpenTarget(key.to_string()));
    }
    let parts: Vec<&str> = spec.split(':').map(str::trim).collect();
    let kind = parts[0].to_ascii_lowercase();
    match kind.as_str() {
        "dance" => {
            let arg = parts.get(1).copied().unwrap_or("").to_ascii_lowercase();
            let valid = matches!(
                arg.as_str(),
                "" | "random" | "wiggle" | "disco" | "spin" | "march" | "sway" | "hop" | "song" | "listen"
            );
            valid.then(|| ChatAct::Dance(if arg == "random" { String::new() } else { arg }))
        }
        "song" | "sing" => Some(ChatAct::Song),
        "costume" => {
            let raw = parts.get(1).copied()?.to_ascii_lowercase();
            let id = match raw.as_str() {
                "default" | "sailor" => "sailor",
                "hoodie" | "pajama" | "idol" | "swim" | "xmas" | "maid" | "jk" => raw.as_str(),
                _ => return None,
            };
            Some(ChatAct::Costume(id.to_string()))
        }
        "outfit" => {
            let item = parts.get(1).copied()?.to_ascii_lowercase();
            if item == "clear" || item == "none" || item == "off" {
                return Some(ChatAct::ClearOutfit);
            }
            if !matches!(item.as_str(), "santa" | "glasses") {
                return None;
            }
            let on = match parts.get(2).copied().unwrap_or("on").to_ascii_lowercase().as_str() {
                "on" | "1" | "true" | "wear" => true,
                "off" | "0" | "false" | "remove" => false,
                _ => return None,
            };
            Some(ChatAct::Outfit(item, on))
        }
        "pomo" | "pomodoro" => {
            let arg = parts.get(1).copied().unwrap_or("25").to_ascii_lowercase();
            if matches!(arg.as_str(), "stop" | "off" | "cancel" | "end") {
                return Some(ChatAct::PomodoroStop);
            }
            let mins: u32 = arg.trim_end_matches("m").trim_end_matches("min").parse().ok()?;
            (1..=180).contains(&mins).then_some(ChatAct::PomodoroStart(mins))
        }
        "weather" => Some(ChatAct::Weather),
        "sleep" => Some(ChatAct::Sleep),
        "walk" => Some(ChatAct::Walk),
        "honey" => Some(ChatAct::Honey),
        "status" => Some(ChatAct::Status),
        "game" | "teaser" => Some(ChatAct::Game),
        "climb" | "perch" => Some(ChatAct::Climb),
        _ => None,
    }
}

fn strip_prefix_ci<'a>(s: &'a str, prefix_lower: &str) -> Option<&'a str> {
    if s.len() < prefix_lower.len() || !s.is_char_boundary(prefix_lower.len()) {
        return None;
    }
    s[..prefix_lower.len()].eq_ignore_ascii_case(prefix_lower).then_some(&s[prefix_lower.len()..])
}

fn parse_memo_spec(rest: &str) -> Option<ChatAct> {
    // 支持两种格式：
    // 1) note|买猫粮
    // 2) YYYY-MM-DD HH:MM|none|开会
    let mut segs = rest.splitn(3, '|');
    let first = segs.next()?.trim();
    let second = segs.next()?.trim();
    if first.eq_ignore_ascii_case("note") || first.eq_ignore_ascii_case("none") || first.is_empty() {
        let text = match segs.next() {
            Some(third) if !third.trim().is_empty() => third.trim(),
            _ => second,
        };
        let clean: String = text.chars().filter(|c| !c.is_control()).take(120).collect();
        return (!clean.trim().is_empty()).then(|| ChatAct::Memo {
            text: clean.trim().to_string(),
            time: None,
            repeat: Repeat::None,
        });
    }
    let third = segs.next()?.trim();
    let dt = Dt::parse(first)?;
    let repeat = match second.to_ascii_lowercase().as_str() {
        "daily" => Repeat::Daily,
        "workday" => Repeat::Workday,
        "weekly" => Repeat::Weekly,
        _ => Repeat::None,
    };
    let clean: String = third.chars().filter(|c| !c.is_control()).take(120).collect();
    (!clean.trim().is_empty()).then(|| ChatAct::Memo {
        text: clean.trim().to_string(),
        time: Some(dt),
        repeat,
    })
}

/// 离线/免 Key 直达：识别简短明确的自然口令（带疑问词或长句交还给 AI 聊天处理）
pub fn parse_local_direct(text: &str) -> Option<(ChatAct, &'static str)> {
    let clean: String = text
        .trim()
        .trim_matches(|c: char| "。！!～~，,、 ".contains(c))
        .to_string();
    let chars = clean.chars().count();
    if chars == 0 || chars > 20 {
        return None;
    }
    // 疑问句一律交给 AI 回答，不劫持成命令
    if ["?", "？", "吗", "呢", "怎么", "什么", "为什么", "能不能", "可以吗", "好不好", "哪"]
        .iter()
        .any(|q| clean.contains(q))
    {
        return None;
    }
    let s = clean
        .trim_start_matches("哈基米")
        .trim_start_matches("帮我")
        .trim_start_matches("请")
        .trim_start_matches("给我")
        .trim_start_matches("去")
        .trim();

    // 1. 打开白名单工具/目录
    for prefix in ["打开", "启动", "开一下"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            let target = rest.trim();
            if !target.is_empty() && target.chars().count() <= 16 {
                return Some((ChatAct::OpenTarget(target.to_string()), "好喵！哈基米帮你看看能不能打开～"));
            }
        }
    }

    // 2. 番茄钟
    if matches!(s, "结束番茄钟" | "停掉番茄钟" | "停止番茄钟" | "关闭番茄钟" | "取消番茄钟") {
        return Some((ChatAct::PomodoroStop, ""));
    }
    if s.contains("番茄钟") && (s.starts_with('开') || s.starts_with("开始") || s.starts_with("设") || s.starts_with("来个") || s.starts_with("番茄钟")) {
        let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
        let mins = digits.parse::<u32>().ok().unwrap_or(25).clamp(1, 180);
        return Some((ChatAct::PomodoroStart(mins), ""));
    }

    // 3. 换装与配饰
    if matches!(s, "摘掉配饰" | "摘掉配件" | "取下配饰" | "摘掉帽子和眼镜") {
        return Some((ChatAct::ClearOutfit, ""));
    }
    if matches!(s, "戴上眼镜" | "戴眼镜" | "戴圆眼镜" | "戴上圆眼镜") {
        return Some((ChatAct::Outfit("glasses".into(), true), ""));
    }
    if matches!(s, "摘掉眼镜" | "摘眼镜" | "取下眼镜") {
        return Some((ChatAct::Outfit("glasses".into(), false), ""));
    }
    if matches!(s, "戴上圣诞帽" | "戴圣诞帽" | "戴帽子" | "戴上帽子") {
        return Some((ChatAct::Outfit("santa".into(), true), ""));
    }
    if matches!(s, "摘掉圣诞帽" | "摘圣诞帽" | "摘掉帽子" | "摘帽子") {
        return Some((ChatAct::Outfit("santa".into(), false), ""));
    }
    if s.starts_with('换') || s.starts_with('穿') {
        let lower = s.to_ascii_lowercase();
        for (kw, id) in [
            ("女仆", "maid"),
            ("睡衣", "pajama"),
            ("卫衣", "hoodie"),
            ("偶像", "idol"),
            ("泳装", "swim"),
            ("泳衣", "swim"),
            ("圣诞", "xmas"),
            ("jk", "jk"),
            ("水手服", "sailor"),
            ("默认衣服", "sailor"),
        ] {
            if lower.contains(kw) {
                return Some((ChatAct::Costume(id.into()), ""));
            }
        }
    }

    // 4. 跳舞 / 唱歌 / 逗猫棒
    if matches!(s, "跟着歌跳" | "听歌跳舞" | "跟着音乐跳") {
        return Some((ChatAct::Dance("listen".into()), ""));
    }
    if matches!(s, "跳支舞" | "跳个舞" | "跳舞" | "来支舞" | "来段舞蹈") {
        return Some((ChatAct::Dance(String::new()), ""));
    }
    if matches!(s, "唱首歌" | "放首歌" | "来首歌" | "唱支歌") {
        return Some((ChatAct::Song, ""));
    }
    if matches!(s, "玩逗猫棒" | "逗猫棒") {
        return Some((ChatAct::Game, ""));
    }

    None
}

fn system32_exe(exe_name: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    root.join("System32").join(exe_name)
}

/// 校验一个路径是否为安全的本机 `.exe`（必须是绝对路径、无 `..`、扩展名为 `.exe`、且不在危险解释器黑名单中）
pub fn validate_user_exe_path(raw_path: &str) -> Result<PathBuf, RejectReason> {
    let trimmed = raw_path.trim().trim_matches('"');
    if trimmed.is_empty() || trimmed.chars().any(|c| c.is_control()) {
        return Err(RejectReason::BlockedExecutable);
    }
    let p = PathBuf::from(trimmed);
    if !p.is_absolute() {
        return Err(RejectReason::BlockedExecutable);
    }
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(RejectReason::BlockedExecutable);
    }
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
    if !ext.eq_ignore_ascii_case("exe") {
        return Err(RejectReason::BlockedExecutable);
    }
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").trim().to_ascii_lowercase();
    if stem.is_empty() || BLOCKED_STEMS.iter().any(|b| b.trim().eq_ignore_ascii_case(&stem)) {
        return Err(RejectReason::BlockedExecutable);
    }
    if !p.is_file() {
        return Err(RejectReason::MissingFile);
    }
    Ok(p)
}

/// 把 `open:<raw>` 解析成安全白名单目标；不在白名单或命中危险解释器立即拒绝
pub fn resolve_open_target(
    raw: &str,
    allowed_apps: &BTreeMap<String, String>,
    reports_dir: impl FnOnce() -> PathBuf,
) -> Result<SafeTarget, RejectReason> {
    let k = raw.trim();
    if k.is_empty() {
        return Err(RejectReason::NotWhitelisted);
    }
    let lower = k.to_ascii_lowercase();
    match lower.as_str() {
        "notepad" | "notepad.exe" | "记事本" => {
            let path = system32_exe("notepad.exe");
            return if path.is_file() {
                Ok(SafeTarget::SystemExe { key: "notepad", display: "记事本 (notepad.exe)", path })
            } else {
                Err(RejectReason::MissingFile)
            };
        }
        "calc" | "calc.exe" | "calculator" | "计算器" => {
            let path = system32_exe("calc.exe");
            return if path.is_file() {
                Ok(SafeTarget::SystemExe { key: "calc", display: "计算器 (calc.exe)", path })
            } else {
                Err(RejectReason::MissingFile)
            };
        }
        "mspaint" | "mspaint.exe" | "paint" | "画图" => {
            let path = system32_exe("mspaint.exe");
            return if path.is_file() {
                Ok(SafeTarget::SystemExe { key: "mspaint", display: "画图 (mspaint.exe)", path })
            } else {
                Err(RejectReason::MissingFile)
            };
        }
        "songs" | "songs_dir" | "songdir" | "歌单" | "歌单文件夹" | "放歌的文件夹" => {
            return Ok(SafeTarget::AppFolder {
                key: "songs",
                display: "哈基米歌单文件夹 (songs)",
                path: crate::core::paths::songs_dir(),
            });
        }
        "datadir" | "data_dir" | "数据目录" | "数据文件夹" => {
            return Ok(SafeTarget::AppFolder {
                key: "datadir",
                display: "哈基米数据文件夹",
                path: crate::core::paths::data_dir(),
            });
        }
        "skins" | "skins_dir" | "皮肤目录" | "皮肤文件夹" => {
            return Ok(SafeTarget::AppFolder {
                key: "skins",
                display: "哈基米角色包文件夹 (skins)",
                path: crate::core::paths::data_dir().join("skins"),
            });
        }
        "reports" | "reports_dir" | "周报目录" | "周报文件夹" => {
            return Ok(SafeTarget::AppFolder {
                key: "reports",
                display: "每周陪伴报告文件夹",
                path: reports_dir(),
            });
        }
        _ => {}
    }

    // 显式拒绝常见危险程序名，便于区分返回 BlockedExecutable
    let stem_guess = lower.strip_suffix(".exe").unwrap_or(&lower);
    if BLOCKED_STEMS.iter().any(|b| b.trim().eq_ignore_ascii_case(stem_guess)) {
        return Err(RejectReason::BlockedExecutable);
    }

    // 查找用户登记的 allowedApps（大小写不敏感）
    if let Some((orig_k, raw_path)) = allowed_apps.iter().find(|(ak, _)| ak.trim().eq_ignore_ascii_case(k)) {
        let path = validate_user_exe_path(raw_path)?;
        let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("app.exe");
        return Ok(SafeTarget::UserExe {
            key: orig_k.clone(),
            display: format!("{orig_k} ({fname})"),
            path,
        });
    }

    Err(RejectReason::NotWhitelisted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_strips_tags_and_parses_multiple_actions() {
        let (txt, acts) = extract("[happy]好呀，这就换上女仆装给你跳支舞喵！[do:costume:maid][do:dance:wiggle]");
        assert_eq!(txt, "[happy]好呀，这就换上女仆装给你跳支舞喵！");
        assert_eq!(
            acts,
            vec![ChatAct::Costume("maid".into()), ChatAct::Dance("wiggle".into())]
        );

        // 全角括号 + 大小写混合 + 未知 do 标签也会被剥干净不外泄
        let (txt2, acts2) = extract("帮你定好啦喵～【DO:pomo:25】［do:unknown_magic］[do:outfit:glasses:on]");
        assert_eq!(txt2, "帮你定好啦喵～");
        assert_eq!(
            acts2,
            vec![ChatAct::PomodoroStart(25), ChatAct::Outfit("glasses".into(), true)]
        );

        // memo 带时间（含冒号）与随手记
        let (txt3, acts3) = extract("记下来啦喵！[do:memo:2026-12-01 09:30|workday|晨会] [do:memo:note|买猫砂]");
        assert_eq!(txt3, "记下来啦喵！");
        assert_eq!(
            acts3,
            vec![
                ChatAct::Memo {
                    text: "晨会".into(),
                    time: Dt::new(2026, 12, 1, 9, 30),
                    repeat: Repeat::Workday,
                },
                ChatAct::Memo {
                    text: "买猫砂".into(),
                    time: None,
                    repeat: Repeat::None,
                }
            ]
        );
    }

    #[test]
    fn local_direct_commands_work_and_questions_pass_through() {
        assert_eq!(parse_local_direct("跳支舞").map(|x| x.0), Some(ChatAct::Dance(String::new())));
        assert_eq!(parse_local_direct("哈基米换套女仆装！").map(|x| x.0), Some(ChatAct::Costume("maid".into())));
        assert_eq!(parse_local_direct("戴上圆眼镜").map(|x| x.0), Some(ChatAct::Outfit("glasses".into(), true)));
        assert_eq!(parse_local_direct("开个45分钟番茄钟").map(|x| x.0), Some(ChatAct::PomodoroStart(45)));
        assert_eq!(parse_local_direct("结束番茄钟").map(|x| x.0), Some(ChatAct::PomodoroStop));
        assert_eq!(parse_local_direct("打开记事本").map(|x| x.0), Some(ChatAct::OpenTarget("记事本".into())));
        // 带疑问词不劫持
        assert_eq!(parse_local_direct("你会跳支舞吗？"), None);
        assert_eq!(parse_local_direct("为什么要打开记事本呢"), None);
    }

    #[test]
    fn allowlist_accepts_safe_targets_and_rejects_shells_and_unknowns() {
        let mut allowed = BTreeMap::new();
        // 即便用户在 allowedApps 里误配了 cmd.exe 或相对路径，也必须硬拒绝
        allowed.insert("bad_shell".into(), r"C:\Windows\System32\cmd.exe".into());
        allowed.insert("bad_ps".into(), r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe".into());
        allowed.insert("bad_rel".into(), r"..\notepad.exe".into());
        allowed.insert("bad_bat".into(), r"C:\tools\run.bat".into());
        let cur_exe = std::env::current_exe().unwrap();
        allowed.insert("my_app".into(), cur_exe.to_string_lossy().to_string());

        let rep_dir = || PathBuf::from(r"C:\Temp\reports");

        // 内置白名单通过
        assert!(matches!(
            resolve_open_target("notepad", &allowed, rep_dir),
            Ok(SafeTarget::SystemExe { key: "notepad", .. })
        ));
        assert!(matches!(
            resolve_open_target("歌单文件夹", &allowed, rep_dir),
            Ok(SafeTarget::AppFolder { key: "songs", .. })
        ));
        // 合法登记的本机 .exe 通过
        assert!(matches!(
            resolve_open_target("MY_APP", &allowed, rep_dir),
            Ok(SafeTarget::UserExe { .. })
        ));

        // 各类非法/危险目标全部被拒
        assert_eq!(resolve_open_target("cmd", &allowed, rep_dir), Err(RejectReason::BlockedExecutable));
        assert_eq!(resolve_open_target("powershell.exe", &allowed, rep_dir), Err(RejectReason::BlockedExecutable));
        assert_eq!(resolve_open_target("bad_shell", &allowed, rep_dir), Err(RejectReason::BlockedExecutable));
        assert_eq!(resolve_open_target("bad_ps", &allowed, rep_dir), Err(RejectReason::BlockedExecutable));
        assert_eq!(resolve_open_target("bad_rel", &allowed, rep_dir), Err(RejectReason::BlockedExecutable));
        assert_eq!(resolve_open_target("bad_bat", &allowed, rep_dir), Err(RejectReason::BlockedExecutable));
        assert_eq!(resolve_open_target("random_unlisted_app", &allowed, rep_dir), Err(RejectReason::NotWhitelisted));
    }
}
