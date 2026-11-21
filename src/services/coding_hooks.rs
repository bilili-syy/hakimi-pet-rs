//! 一键接入编码助手通知（Phase 3 第 3 步）：Claude Code (`~/.claude/settings.json`) 与 Codex (`~/.codex/config.toml`)
//!
//! 设计原则：
//! - 无损合并：绝不覆盖用户的模型、MCP、权限或其它自定义 Hook，只增删哈基米自己的回调条目。
//! - 先备份、可撤销：首次写入前生成 `.hakimi.bak` 备份；点「撤销接入」精准剥离哈基米条目并恢复用户原有的 `notify`。
//! - 静默容错：写入的命令统一附带 `--quiet`，桌宠没打开时回调静默退出码 0，绝不在终端刷 Hook 报错。
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

const BACKUP_PREFIX: &str = "# [hakimi-backup] ";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HookStatus {
    pub claude_installed: bool,
    pub codex_installed: bool,
    pub claude_backup: bool,
    pub codex_backup: bool,
}

impl HookStatus {
    pub fn any_installed(&self) -> bool {
        self.claude_installed || self.codex_installed
    }

    pub fn summary_text(&self) -> String {
        match (self.claude_installed, self.codex_installed) {
            (true, true) => "✔ 已接入 Claude Code 与 Codex（跑完/等确认时提醒，随时可撤销）".into(),
            (true, false) => "✔ 已接入 Claude Code（Codex 未接入）".into(),
            (false, true) => "✔ 已接入 Codex（Claude Code 未接入）".into(),
            (false, false) => "Claude Code / Codex 跑完、报错或等确认时叫她提醒你".into(),
        }
    }
}

/// 解析用户主目录（支持 `HAKIMI_TEST_HOME` 供自动化 E2E 隔离测试，绝不污染真实配置）
pub fn user_home() -> Option<PathBuf> {
    if let Ok(test_home) = std::env::var("HAKIMI_TEST_HOME") {
        let t = test_home.trim();
        if !t.is_empty() {
            return Some(PathBuf::from(t));
        }
    }
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

pub fn status() -> HookStatus {
    let Some(home) = user_home() else { return HookStatus::default() };
    status_in(&home)
}

pub fn install() -> Result<HookStatus, String> {
    let home = user_home().ok_or("找不到当前用户主目录（USERPROFILE）")?;
    let exe = std::env::current_exe().map_err(|e| format!("无法获取程序自身路径：{e}"))?;
    install_in(&home, &exe)
}

pub fn uninstall() -> Result<HookStatus, String> {
    let home = user_home().ok_or("找不到当前用户主目录（USERPROFILE）")?;
    uninstall_in(&home)
}

fn claude_path(home: &Path) -> PathBuf { home.join(".claude").join("settings.json") }
fn claude_bak_path(home: &Path) -> PathBuf { home.join(".claude").join("settings.json.hakimi.bak") }
fn codex_path(home: &Path) -> PathBuf { home.join(".codex").join("config.toml") }
fn codex_bak_path(home: &Path) -> PathBuf { home.join(".codex").join("config.toml.hakimi.bak") }

fn is_hakimi_cmd(cmd: &str) -> bool {
    let lower = cmd.to_ascii_lowercase();
    cmd.contains("--event")
        && (lower.contains("hakimi-pet")
            || (cmd.contains("--source") && (cmd.contains("Claude Code") || cmd.contains("Codex")) && cmd.contains("--quiet")))
}

// ------------------------------------------------------------------ Claude Code JSON

fn claude_cmd(exe: &Path, event: &str) -> String {
    let p = exe.to_string_lossy().replace('"', "");
    format!("\"{p}\" --event {event} --source \"Claude Code\" --quiet")
}

fn rule_is_pure_hakimi(rule: &Value) -> bool {
    let Some(arr) = rule.get("hooks").and_then(Value::as_array) else { return false };
    !arr.is_empty()
        && arr.iter().all(|h| {
            h.get("command").and_then(Value::as_str).is_some_and(is_hakimi_cmd)
        })
}

fn strip_hakimi_from_hook_array(arr: &mut Vec<Value>) -> bool {
    let before = serde_json::to_string(arr).unwrap_or_default();
    let mut kept = Vec::with_capacity(arr.len());
    for mut rule in arr.drain(..) {
        if rule_is_pure_hakimi(&rule) {
            continue;
        }
        if let Some(inner) = rule.get_mut("hooks").and_then(Value::as_array_mut) {
            inner.retain(|h| !h.get("command").and_then(Value::as_str).is_some_and(is_hakimi_cmd));
            if inner.is_empty() {
                continue;
            }
        }
        kept.push(rule);
    }
    *arr = kept;
    serde_json::to_string(arr).unwrap_or_default() != before
}

fn claude_has_hakimi(root: &Map<String, Value>) -> bool {
    let Some(hooks) = root.get("hooks").and_then(Value::as_object) else { return false };
    ["Stop", "Notification"].iter().all(|ev| {
        hooks.get(*ev).and_then(Value::as_array).is_some_and(|arr| {
            arr.iter().any(|rule| {
                rule.get("hooks")
                    .and_then(Value::as_array)
                    .is_some_and(|inner| inner.iter().any(|h| h.get("command").and_then(Value::as_str).is_some_and(is_hakimi_cmd)))
            })
        })
    })
}

fn claude_any_hakimi(root: &Map<String, Value>) -> bool {
    let Some(hooks) = root.get("hooks").and_then(Value::as_object) else { return false };
    ["Stop", "Notification"].iter().any(|ev| {
        hooks.get(*ev).and_then(Value::as_array).is_some_and(|arr| {
            arr.iter().any(|rule| {
                rule.get("hooks")
                    .and_then(Value::as_array)
                    .is_some_and(|inner| inner.iter().any(|h| h.get("command").and_then(Value::as_str).is_some_and(is_hakimi_cmd)))
            })
        })
    })
}

fn read_claude_map(path: &Path) -> Result<Map<String, Value>, String> {
    if !path.exists() {
        return Ok(Map::new());
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
    let trimmed = text.trim_start_matches('\u{feff}').trim();
    if trimmed.is_empty() {
        return Ok(Map::new());
    }
    let v: Value = serde_json::from_str(trimmed)
        .map_err(|_| "Claude Code 的 settings.json 不是标准 JSON，为保护你的原配置未做修改".to_string())?;
    match v {
        Value::Object(m) => Ok(m),
        _ => Err("Claude Code 的 settings.json 顶层不是对象，未做修改".into()),
    }
}

fn install_claude(home: &Path, exe: &Path) -> Result<(), String> {
    let path = claude_path(home);
    let bak = claude_bak_path(home);
    let mut root = read_claude_map(&path)?;
    if path.exists() && !claude_any_hakimi(&root) && !bak.exists() {
        let _ = std::fs::copy(&path, &bak);
    }
    let hooks_val = root.entry("hooks").or_insert_with(|| Value::Object(Map::new()));
    if !hooks_val.is_object() {
        return Err("Claude Code settings.json 中的 hooks 字段不是对象，未做修改".into());
    }
    let hooks = hooks_val.as_object_mut().unwrap();
    for (ev_name, kind) in [("Stop", "done"), ("Notification", "wait")] {
        let entry = hooks.entry(ev_name).or_insert_with(|| Value::Array(Vec::new()));
        let Some(arr) = entry.as_array_mut() else {
            return Err(format!("Claude Code hooks.{ev_name} 不是数组，未做修改"));
        };
        strip_hakimi_from_hook_array(arr);
        arr.push(json!({
            "hooks": [
                {
                    "type": "command",
                    "command": claude_cmd(exe, kind),
                }
            ]
        }));
    }
    let out = serde_json::to_string_pretty(&Value::Object(root)).map_err(|e| e.to_string())?;
    crate::core::save::write_atomic(&path, format!("{out}\n").as_bytes())
        .map_err(|e| format!("写入 {} 失败：{e}", path.display()))
}

fn uninstall_claude(home: &Path) -> Result<(), String> {
    let path = claude_path(home);
    let bak = claude_bak_path(home);
    if !path.exists() {
        let _ = std::fs::remove_file(&bak);
        return Ok(());
    }
    let mut root = read_claude_map(&path)?;
    let mut changed = false;
    if let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) {
        for ev_name in ["Stop", "Notification"] {
            if let Some(arr) = hooks.get_mut(ev_name).and_then(Value::as_array_mut)
                && strip_hakimi_from_hook_array(arr)
            {
                changed = true;
            }
            if hooks.get(ev_name).and_then(Value::as_array).is_some_and(Vec::is_empty) {
                hooks.remove(ev_name);
                changed = true;
            }
        }
        if hooks.is_empty() {
            root.remove("hooks");
            changed = true;
        }
    }
    if changed {
        let out = serde_json::to_string_pretty(&Value::Object(root)).map_err(|e| e.to_string())?;
        crate::core::save::write_atomic(&path, format!("{out}\n").as_bytes())
            .map_err(|e| format!("写回 {} 失败：{e}", path.display()))?;
    }
    let _ = std::fs::remove_file(&bak);
    Ok(())
}

// ------------------------------------------------------------------ Codex TOML

fn escape_toml_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

fn codex_notify_line(exe: &Path) -> String {
    let p = escape_toml_str(&exe.to_string_lossy());
    format!("notify = [\"{p}\", \"--event\", \"done\", \"--source\", \"Codex\", \"--quiet\"]")
}

/// 扫描 TOML 顶层区域（第一个 `[table]` 之前）：返回 `(第一个 [table] 所在行号, 顶层 notify 赋值的起止闭区间)`
fn scan_codex_preamble(lines: &[&str]) -> (usize, Option<(usize, usize)>) {
    let mut i = 0usize;
    let mut notify_span = None;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            i += 1;
            continue;
        }
        if trimmed.starts_with('[') {
            return (i, notify_span);
        }
        if let Some(after_key) = trimmed.strip_prefix("notify")
            && after_key.trim_start().starts_with('=')
        {
            let rhs = after_key.trim_start().strip_prefix('=').unwrap_or("");
            let mut depth = 0i32;
            let mut end = i;
            let mut saw_bracket = false;
            for (li, ln) in lines[i..].iter().enumerate() {
                let text_part = if li == 0 { rhs } else { *ln };
                let mut in_dq = false;
                let mut in_sq = false;
                let mut escaped = false;
                for ch in text_part.chars() {
                    if in_dq {
                        if escaped {
                            escaped = false;
                        } else if ch == '\\' {
                            escaped = true;
                        } else if ch == '"' {
                            in_dq = false;
                        }
                        continue;
                    }
                    if in_sq {
                        if ch == '\'' {
                            in_sq = false;
                        }
                        continue;
                    }
                    match ch {
                        '#' => break,
                        '"' => in_dq = true,
                        '\'' => in_sq = true,
                        '[' => {
                            saw_bracket = true;
                            depth += 1;
                        }
                        ']' => depth -= 1,
                        _ => {}
                    }
                }
                end = i + li;
                if !saw_bracket || depth <= 0 {
                    break;
                }
            }
            notify_span = Some((i, end));
            i = end + 1;
            continue;
        }
        i += 1;
    }
    (lines.len(), notify_span)
}

fn codex_span_is_hakimi(lines: &[&str], (a, b): (usize, usize)) -> bool {
    let joined = lines[a..=b].join(" ");
    is_hakimi_cmd(&joined)
}

fn codex_has_hakimi(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().collect();
    let (_, span) = scan_codex_preamble(&lines);
    span.is_some_and(|sp| codex_span_is_hakimi(&lines, sp))
}

fn split_lines_keep_style(text: &str) -> (Vec<String>, &'static str) {
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let v = if text.is_empty() {
        Vec::new()
    } else {
        text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_string()).collect()
    };
    // 去掉末尾因 trailing newline 产生的空元素，写回时统一加结尾换行
    let mut v = v;
    if v.last().is_some_and(String::is_empty) {
        v.pop();
    }
    (v, nl)
}

fn install_codex(home: &Path, exe: &Path) -> Result<(), String> {
    let path = codex_path(home);
    let bak = codex_bak_path(home);
    let raw = if path.exists() {
        std::fs::read_to_string(&path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?
    } else {
        String::new()
    };
    let clean = raw.trim_start_matches('\u{feff}');
    if path.exists() && !codex_has_hakimi(clean) && !bak.exists() {
        let _ = std::fs::copy(&path, &bak);
    }
    let (mut lines, nl) = split_lines_keep_style(clean);
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let (first_table, span) = scan_codex_preamble(&refs);
    let new_line = codex_notify_line(exe);

    match span {
        Some((a, b)) if codex_span_is_hakimi(&refs, (a, b)) => {
            lines.splice(a..=b, [new_line]);
        }
        Some((a, b)) => {
            // 用户原本配了自己的 notify：原位注释备份，下面写哈基米的，撤销时无损还原
            let mut repl: Vec<String> = lines[a..=b].iter().map(|l| format!("{BACKUP_PREFIX}{l}")).collect();
            repl.push(new_line);
            lines.splice(a..=b, repl);
        }
        None => {
            if first_table == 0 {
                if !lines.is_empty() {
                    lines.splice(0..0, [new_line, String::new()]);
                } else {
                    lines.push(new_line);
                }
            } else {
                // 插在第一个 [table] 之前（保持与上方顶层键紧凑，与下方 [table] 留一行空行）
                let mut ins_at = first_table;
                while ins_at > 0 && lines[ins_at - 1].trim().is_empty() {
                    ins_at -= 1;
                }
                lines.insert(ins_at, new_line);
            }
        }
    }

    let mut out = lines.join(nl);
    out.push_str(nl);
    crate::core::save::write_atomic(&path, out.as_bytes())
        .map_err(|e| format!("写入 {} 失败：{e}", path.display()))
}

fn uninstall_codex(home: &Path) -> Result<(), String> {
    let path = codex_path(home);
    let bak = codex_bak_path(home);
    if !path.exists() {
        let _ = std::fs::remove_file(&bak);
        return Ok(());
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
    let (mut lines, nl) = split_lines_keep_style(raw.trim_start_matches('\u{feff}'));
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let (first_table, span) = scan_codex_preamble(&refs);
    let mut changed = false;

    if let Some((a, b)) = span
        && codex_span_is_hakimi(&refs, (a, b))
    {
        lines.drain(a..=b);
        changed = true;
    }

    // 还原原位注释备份的用户原 notify（仅扫描顶层区域）
    let limit = first_table.min(lines.len());
    for line in &mut lines[..limit] {
        if let Some(orig) = line.strip_prefix(BACKUP_PREFIX) {
            *line = orig.to_string();
            changed = true;
        }
    }
    // 清理顶端多余空行
    while lines.first().is_some_and(|l| l.trim().is_empty()) {
        lines.remove(0);
        changed = true;
    }

    if changed {
        let out = if lines.is_empty() {
            String::new()
        } else {
            let mut s = lines.join(nl);
            s.push_str(nl);
            s
        };
        crate::core::save::write_atomic(&path, out.as_bytes())
            .map_err(|e| format!("写回 {} 失败：{e}", path.display()))?;
    }
    let _ = std::fs::remove_file(&bak);
    Ok(())
}

// ------------------------------------------------------------------ 组合入口

pub fn status_in(home: &Path) -> HookStatus {
    let claude_installed = read_claude_map(&claude_path(home))
        .ok()
        .is_some_and(|m| claude_has_hakimi(&m));
    let codex_installed = std::fs::read_to_string(codex_path(home))
        .ok()
        .is_some_and(|t| codex_has_hakimi(&t));
    HookStatus {
        claude_installed,
        codex_installed,
        claude_backup: claude_bak_path(home).is_file(),
        codex_backup: codex_bak_path(home).is_file(),
    }
}

pub fn install_in(home: &Path, exe: &Path) -> Result<HookStatus, String> {
    install_claude(home, exe)?;
    install_codex(home, exe)?;
    crate::core::log::log("coding hooks installed for Claude Code and Codex");
    Ok(status_in(home))
}

pub fn uninstall_in(home: &Path) -> Result<HookStatus, String> {
    uninstall_claude(home)?;
    uninstall_codex(home)?;
    crate::core::log::log("coding hooks uninstalled for Claude Code and Codex");
    Ok(status_in(home))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_home(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hakimi-hooks-{}-{tag}-{}", std::process::id(), fastrand::u32(..)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn fresh_install_idempotent_update_and_clean_uninstall() {
        let home = tmp_home("fresh");
        let exe1 = Path::new(r"D:\Apps\Hakimi\hakimi-pet.exe");
        let exe2 = Path::new(r"C:\Program Files\Hakimi\hakimi-pet.exe");

        assert!(!status_in(&home).any_installed());
        let st = install_in(&home, exe1).unwrap();
        assert!(st.claude_installed && st.codex_installed);

        // 换路径再次安装：幂等更新命令路径，不重复堆叠条目
        let st2 = install_in(&home, exe2).unwrap();
        assert!(st2.claude_installed && st2.codex_installed);
        let cj: Value = serde_json::from_str(&std::fs::read_to_string(claude_path(&home)).unwrap()).unwrap();
        assert_eq!(cj["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert_eq!(cj["hooks"]["Notification"].as_array().unwrap().len(), 1);
        assert!(cj["hooks"]["Stop"][0]["hooks"][0]["command"].as_str().unwrap().contains("Program Files"));

        let ct = std::fs::read_to_string(codex_path(&home)).unwrap();
        assert_eq!(ct.matches("notify =").count(), 1);
        assert!(ct.contains(r#"C:\\Program Files\\Hakimi\\hakimi-pet.exe"#), "{ct}");

        let st3 = uninstall_in(&home).unwrap();
        assert!(!st3.any_installed());
        let cj_after: Value = serde_json::from_str(&std::fs::read_to_string(claude_path(&home)).unwrap()).unwrap();
        assert!(cj_after.get("hooks").is_none(), "{cj_after}");
        assert!(std::fs::read_to_string(codex_path(&home)).unwrap().trim().is_empty());

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn preserves_existing_claude_hooks_and_restores_codex_multiline_notify() {
        let home = tmp_home("preserve");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::create_dir_all(home.join(".codex")).unwrap();

        let orig_claude = r#"{
  "model": "claude-opus-4-6",
  "hooks": {
    "Stop": [
      {
        "hooks": [
          { "type": "command", "command": "echo user-stop-hook" }
        ]
      }
    ]
  }
}"#;
        std::fs::write(claude_path(&home), orig_claude).unwrap();

        let orig_codex = "model = \"gpt-5-codex\"\r\nnotify = [\r\n  \"python\",\r\n  \"C:\\\\my\\\\hook.py\"\r\n]\r\n\r\n[mcp_servers.git]\r\ncommand = \"uvx\"\r\n";
        std::fs::write(codex_path(&home), orig_codex).unwrap();

        let exe = Path::new(r"D:\hakimi\hakimi-pet.exe");
        let st = install_in(&home, exe).unwrap();
        assert!(st.claude_installed && st.codex_installed);
        assert!(st.claude_backup && st.codex_backup, "首次修改已有配置必须生成 .hakimi.bak 备份");

        // Claude：原 model 和原 echo hook 仍在，哈基米追加在后面
        let cj: Value = serde_json::from_str(&std::fs::read_to_string(claude_path(&home)).unwrap()).unwrap();
        assert_eq!(cj["model"], "claude-opus-4-6");
        assert_eq!(cj["hooks"]["Stop"].as_array().unwrap().len(), 2);

        // Codex：哈基米 notify 位于顶层（在 [mcp_servers.git] 之前）
        let ct = std::fs::read_to_string(codex_path(&home)).unwrap();
        let notify_pos = ct.find("notify = [\"D:\\\\hakimi\\\\hakimi-pet.exe\"").expect(&ct);
        let table_pos = ct.find("[mcp_servers.git]").expect(&ct);
        assert!(notify_pos < table_pos, "TOML notify must stay before [table] headers:\n{ct}");

        // 模拟用户在接入后又手动新增了字段，测试撤销时不会用旧备份粗暴覆盖用户新改的设置
        let ct_edited = ct.replace("command = \"uvx\"", "command = \"npx\"");
        std::fs::write(codex_path(&home), ct_edited).unwrap();

        let st_off = uninstall_in(&home).unwrap();
        assert!(!st_off.any_installed());

        let cj_restored: Value = serde_json::from_str(&std::fs::read_to_string(claude_path(&home)).unwrap()).unwrap();
        assert_eq!(cj_restored["model"], "claude-opus-4-6");
        assert_eq!(cj_restored["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert_eq!(cj_restored["hooks"]["Stop"][0]["hooks"][0]["command"], "echo user-stop-hook");
        assert!(cj_restored["hooks"].get("Notification").is_none());

        let ct_restored = std::fs::read_to_string(codex_path(&home)).unwrap();
        assert!(ct_restored.contains("notify = [\r\n  \"python\",\r\n  \"C:\\\\my\\\\hook.py\"\r\n]"), "{ct_restored}");
        assert!(ct_restored.contains("command = \"npx\""), "用户在接入期间改的其它配置应完好保留:\n{ct_restored}");
        assert!(!ct_restored.contains("hakimi-pet.exe"));

        let _ = std::fs::remove_dir_all(&home);
    }
}
