//! 右键/托盘菜单里的「📝 备忘录」子菜单：列出没完成的条目（时间 + 内容），每条可「完成」「删除」，
//! 还有「稍后 10 分钟再提醒」（正在响时）、「打开 memo.json」「清空已完成」。
//! 数据变了（core::memo::version）或者过了零点才重建；菜单弹出期间不动它。
use crate::core::memo::{self as store, Now};
use crate::types::{MemoAct, MenuAction};
use std::cell::RefCell;
use tray_icon::menu::{Menu, MenuId, MenuItem, PredefinedMenuItem, Submenu};

/// 子菜单里最多列多少条
const MAX_ITEMS: usize = 20;

struct MemoMenu {
    sub: Submenu,
    /// (数据版本, 日期)
    shown: Option<(u64, (i32, u32, u32))>,
}

thread_local! {
    static MM: RefCell<Option<MemoMenu>> = const { RefCell::new(None) };
}

/// 插到 root 里 after 那一项的后面（找不到就放最后）
pub fn init(root: &Menu, after: &MenuId) {
    let sub = Submenu::new("📝 备忘录", true);
    let items = root.items();
    let pos = items.iter().position(|k| k.id() == after).map(|p| p + 1).unwrap_or(items.len());
    let _ = root.insert(&sub, pos);
    MM.with(|m| *m.borrow_mut() = Some(MemoMenu { sub, shown: None }));
    refresh(false);
}

/// 菜单文字里的 & 是助记符，要转义；太长的截断
fn label(s: &str, max: usize) -> String {
    let mut t: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        t.push('…');
    }
    t.replace('&', "&&")
}

pub fn refresh(popup_open: bool) {
    if popup_open {
        return;
    }
    let today = Now::get().dt;
    let key = (store::version(), today.date());
    MM.with(|m| {
        let Ok(mut g) = m.try_borrow_mut() else { return };
        let Some(m) = g.as_mut() else { return };
        if m.shown == Some(key) {
            return;
        }
        m.shown = Some(key);
        while m.sub.remove_at(0).is_some() {}

        let item = |id: String, text: &str| MenuItem::with_id(id, text, true, None);
        let pending = store::pending();
        if !store::ringing().is_empty() {
            let _ = m.sub.append(&item("memo:snooze".into(), "⏰ 稍后 10 分钟再提醒"));
            let _ = m.sub.append(&PredefinedMenuItem::separator());
        }
        if pending.is_empty() {
            let _ = m.sub.append(&MenuItem::new("（还没有待办，聊天里说「提醒我…」）", false, None));
        }
        for it in pending.iter().take(MAX_ITEMS) {
            let s = Submenu::new(format!("{}  {}", it.when_label(today), label(&it.text, 24)), true);
            let _ = s.append(&item(format!("memo:done:{}", it.id), "✅ 完成"));
            let _ = s.append(&item(format!("memo:del:{}", it.id), "🗑️ 删除"));
            let _ = m.sub.append(&s);
        }
        if pending.len() > MAX_ITEMS {
            let more = format!("…还有 {} 条（在 memo.json 里）", pending.len() - MAX_ITEMS);
            let _ = m.sub.append(&MenuItem::new(more, false, None));
        }
        let _ = m.sub.append(&PredefinedMenuItem::separator());
        let _ = m.sub.append(&item("memo:open".into(), "打开 memo.json"));
        let _ = m.sub.append(&item("memo:clear".into(), "清空已完成"));
        m.sub.set_text(if pending.is_empty() { "📝 备忘录".to_string() } else { format!("📝 备忘录（{}）", pending.len()) });
    });
}

/// 菜单 id → 动作
pub fn parse(id: &str) -> Option<MenuAction> {
    let rest = id.strip_prefix("memo:")?;
    let a = match rest {
        "snooze" => MemoAct::Snooze,
        "open" => MemoAct::OpenFile,
        "clear" => MemoAct::ClearDone,
        _ => {
            if let Some(n) = rest.strip_prefix("done:") {
                MemoAct::Done(n.parse().ok()?)
            } else {
                MemoAct::Delete(rest.strip_prefix("del:")?.parse().ok()?)
            }
        }
    };
    Some(MenuAction::Memo(a))
}

pub fn shutdown() {
    MM.with(|m| {
        if let Ok(mut g) = m.try_borrow_mut() {
            *g = None;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        assert_eq!(parse("memo:done:12"), Some(MenuAction::Memo(MemoAct::Done(12))));
        assert_eq!(parse("memo:del:3"), Some(MenuAction::Memo(MemoAct::Delete(3))));
        assert_eq!(parse("memo:snooze"), Some(MenuAction::Memo(MemoAct::Snooze)));
        assert_eq!(parse("memo:open"), Some(MenuAction::Memo(MemoAct::OpenFile)));
        assert_eq!(parse("memo:clear"), Some(MenuAction::Memo(MemoAct::ClearDone)));
        assert_eq!(parse("memo:done:x"), None);
        assert_eq!(parse("chat"), None);
        assert_eq!(label("a&b", 24), "a&&b");
        assert_eq!(label("一二三", 2), "一二…");
    }
}
