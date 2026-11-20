//! 右键/托盘菜单里的「🧠 她的记忆」子菜单：记得几件事、打开 memory.json、清空记忆（弹框二次确认）。
//! 数据变了（core::memory::version）才改文字；菜单弹出期间不动它。
use crate::types::{MenuAction, MindAct};
use std::cell::RefCell;
use tray_icon::menu::{Menu, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_DEFBUTTON2, MB_ICONQUESTION, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO, MessageBoxW,
};
use windows::core::HSTRING;

struct MindMenu {
    sub: Submenu,
    count: MenuItem,
    shown: Option<u64>,
}

thread_local! {
    static MM: RefCell<Option<MindMenu>> = const { RefCell::new(None) };
}

/// 插到 root 里 before 那一项的前面（找不到就放最后）
pub fn init(root: &Menu, before: &MenuId) {
    let sub = Submenu::new("🧠 她的记忆", true);
    let count = MenuItem::new("（还没记住什么）", false, None);
    let _ = sub.append(&count);
    let _ = sub.append(&PredefinedMenuItem::separator());
    let _ = sub.append(&MenuItem::with_id("mind:open", "打开 memory.json", true, None));
    let _ = sub.append(&MenuItem::with_id("mind:clear", "清空记忆…", true, None));
    let items = root.items();
    let pos = items.iter().position(|k| k.id() == before).unwrap_or(items.len());
    let _ = root.insert(&sub, pos);
    MM.with(|m| *m.borrow_mut() = Some(MindMenu { sub, count, shown: None }));
    refresh(false);
}

pub fn count_label(n: usize) -> String {
    if n == 0 { "（还没记住什么，聊天里说「记住…」）".into() } else { format!("记得关于你的 {n} 件事") }
}

pub fn refresh(popup_open: bool) {
    if popup_open {
        return;
    }
    let v = crate::core::memory::version();
    MM.with(|m| {
        let Ok(mut g) = m.try_borrow_mut() else { return };
        let Some(m) = g.as_mut() else { return };
        if m.shown == Some(v) {
            return;
        }
        m.shown = Some(v);
        let n = crate::core::memory::count();
        m.count.set_text(count_label(n));
        m.sub.set_text(if n == 0 { "🧠 她的记忆".to_string() } else { format!("🧠 她的记忆（{n}）") });
    });
}

/// 菜单 id → 动作
pub fn parse(id: &str) -> Option<MenuAction> {
    Some(MenuAction::Mind(match id {
        "mind:open" => MindAct::OpenFile,
        "mind:clear" => MindAct::ClearAsk,
        _ => return None,
    }))
}

/// 「清空记忆」二次确认（模态框，默认按钮是「否」）
pub fn confirm_clear(owner: Option<HWND>) -> bool {
    let n = crate::core::memory::count();
    let text = HSTRING::from(format!(
        "真的要让哈基米忘掉关于你的全部 {n} 件事吗？\n\n清空后没法恢复（可以先「打开 memory.json」备份一下）。"
    ));
    let title = HSTRING::from("清空记忆");
    // SAFETY: 两个字符串在调用期间有效；owner 是本进程的窗口或空
    let r = unsafe {
        MessageBoxW(owner, &text, &title, MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2 | MB_TOPMOST | MB_SETFOREGROUND)
    };
    r == IDYES
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
        assert_eq!(parse("mind:open"), Some(MenuAction::Mind(MindAct::OpenFile)));
        assert_eq!(parse("mind:clear"), Some(MenuAction::Mind(MindAct::ClearAsk)));
        assert_eq!(parse("mind:x"), None);
        assert_eq!(parse("memo:open"), None);
        assert!(count_label(0).contains("还没记住"));
        assert_eq!(count_label(3), "记得关于你的 3 件事");
    }
}
