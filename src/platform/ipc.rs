//! 本机事件接口：别的程序（编译脚本、Claude Code / Codex 的 hook……）让哈基米做出反应。
//!
//! 用法：`hakimi-pet.exe --event done|error|wait|info [--source 名字] [--msg "一句话"]`
//! - 已经有一只在跑：把事件交给它，自己立刻退出（不会起第二只）
//! - 没有在跑：安静退出，什么都不做（退出码 1）
//!
//! 传输用 WM_COPYDATA 发给同一桌面上的宠物窗口：只有同一用户会话、同一桌面、
//! 完整性级别不低于宠物的进程才发得进来（UIPI），不开任何端口。
//! 载荷是一行 JSON（ExtEvent），带魔数，长度有上限；msg 截到 200 字。
use crate::types::{ExtEvent, ExtKind};

/// COPYDATASTRUCT.dwData 魔数："HKE1"
pub const MAGIC: usize = 0x484B_4531;
/// 载荷上限（字节）
pub const MAX_PAYLOAD: usize = 4096;
/// msg 最多保留的字符数
pub const MAX_MSG: usize = 200;
const MAX_SOURCE: usize = 32;
/// 同一来源同一种事件，这么久之内只算一次
pub const THROTTLE_MS: u64 = 3000;

fn kind_of(s: &str) -> Option<ExtKind> {
    Some(match s.trim().to_ascii_lowercase().as_str() {
        "done" | "finish" | "finished" | "ok" => ExtKind::Done,
        "error" | "fail" | "failed" => ExtKind::Error,
        "wait" | "waiting" | "ask" | "notify" => ExtKind::Wait,
        "info" | "say" => ExtKind::Info,
        _ => return None,
    })
}

fn clip(s: &str, n: usize) -> String {
    let s: String = s.chars().filter(|c| !c.is_control() || *c == '\n').collect();
    let s = s.trim();
    if s.chars().count() <= n { s.to_string() } else { s.chars().take(n).collect::<String>() + "…" }
}

fn clean(ev: ExtEvent) -> ExtEvent {
    ExtEvent { kind: ev.kind, source: clip(&ev.source, MAX_SOURCE), msg: clip(&ev.msg, MAX_MSG) }
}

/// 解析命令行。没有 `--event` 返回 None（正常启动宠物）；有但写错了返回 Some(Err)。
pub fn parse_args(args: &[String]) -> Option<Result<ExtEvent, String>> {
    let i = args.iter().position(|a| a == "--event")?;
    let Some(kind) = args.get(i + 1).and_then(|k| kind_of(k)) else {
        return Some(Err("用法：--event done|error|wait|info [--source 名字] [--msg 内容]".into()));
    };
    // 值可以是多个参数（调用方没加引号时 "Claude Code" 会被拆开）：一直取到下一个 --开关
    let val = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .map(|j| args[j + 1..].iter().take_while(|a| !a.starts_with("--")).cloned().collect::<Vec<_>>().join(" "))
            .unwrap_or_default()
    };
    Some(Ok(clean(ExtEvent { kind, source: val("--source"), msg: val("--msg") })))
}

pub fn encode(ev: &ExtEvent) -> Vec<u8> {
    let mut b = serde_json::to_vec(ev).unwrap_or_default();
    b.truncate(MAX_PAYLOAD);
    b
}

/// 收到的载荷 → 事件；格式不对 / 太大一律丢掉
pub fn decode(magic: usize, data: &[u8]) -> Option<ExtEvent> {
    if magic != MAGIC || data.is_empty() || data.len() > MAX_PAYLOAD {
        return None;
    }
    serde_json::from_slice::<ExtEvent>(data).ok().map(clean)
}

/// 同源同类事件限流
#[derive(Default)]
pub struct Throttle {
    last: Vec<(String, ExtKind, u64)>,
}

impl Throttle {
    pub fn allow(&mut self, ev: &ExtEvent, now: u64) -> bool {
        self.last.retain(|(_, _, t)| now.saturating_sub(*t) < THROTTLE_MS);
        if self.last.iter().any(|(s, k, _)| *s == ev.source && *k == ev.kind) {
            return false;
        }
        self.last.push((ev.source.clone(), ev.kind, now));
        true
    }
}

/// 命令行模式：把事件交给正在跑的那只。交到了返回 true
pub fn send(ev: &ExtEvent) -> bool {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_COPYDATA};
    use windows::core::PCWSTR;
    let data = encode(ev);
    unsafe {
        let Ok(w) = FindWindowW(super::sys::WINDOW_CLASS, PCWSTR::null()) else { return false };
        if w.0.is_null() {
            return false;
        }
        let cds = COPYDATASTRUCT { dwData: MAGIC, cbData: data.len() as u32, lpData: data.as_ptr() as *mut _ };
        let mut res = 0usize;
        let r = SendMessageTimeoutW(
            w,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM(&cds as *const _ as isize),
            SMTO_ABORTIFHUNG,
            2000,
            Some(&mut res),
        );
        r.0 != 0 && res == 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn args() {
        assert!(parse_args(&a(&["pet.exe"])).is_none());
        let ev = parse_args(&a(&["pet.exe", "--event", "DONE", "--source", "Claude Code", "--msg", "改完了"])).unwrap().unwrap();
        assert_eq!(ev, ExtEvent { kind: ExtKind::Done, source: "Claude Code".into(), msg: "改完了".into() });
        assert!(parse_args(&a(&["pet.exe", "--event"])).unwrap().is_err());
        let ev = parse_args(&a(&["pet.exe", "--event", "wait", "--source", "Claude", "Code", "--msg", "a", "b"])).unwrap().unwrap();
        assert_eq!((ev.source.as_str(), ev.msg.as_str()), ("Claude Code", "a b"));
        assert!(parse_args(&a(&["pet.exe", "--event", "boom"])).unwrap().is_err());
        let ev = parse_args(&a(&["pet.exe", "--event", "error"])).unwrap().unwrap();
        assert_eq!((ev.kind, ev.source.as_str(), ev.msg.as_str()), (ExtKind::Error, "", ""));
    }

    #[test]
    fn roundtrip_and_limits() {
        let ev = ExtEvent { kind: ExtKind::Wait, source: "codex".into(), msg: "要你确认\u{7}".into() };
        let got = decode(MAGIC, &encode(&ev)).unwrap();
        assert_eq!(got.msg, "要你确认");
        assert!(decode(MAGIC + 1, &encode(&ev)).is_none());
        assert!(decode(MAGIC, b"not json").is_none());
        assert!(decode(MAGIC, &vec![b' '; MAX_PAYLOAD + 1]).is_none());
        let long = ExtEvent { kind: ExtKind::Info, source: "x".repeat(100), msg: "喵".repeat(500) };
        let got = decode(MAGIC, &encode(&long)).unwrap();
        assert_eq!(got.msg.chars().count(), MAX_MSG + 1);
        assert_eq!(got.source.chars().count(), MAX_SOURCE + 1);
    }

    #[test]
    fn throttle() {
        let mut t = Throttle::default();
        let d = ExtEvent { kind: ExtKind::Done, source: "a".into(), msg: String::new() };
        let e = ExtEvent { kind: ExtKind::Error, ..d.clone() };
        assert!(t.allow(&d, 1000));
        assert!(!t.allow(&d, 2000));
        assert!(t.allow(&e, 2000));
        assert!(t.allow(&d, 1000 + THROTTLE_MS));
    }
}
