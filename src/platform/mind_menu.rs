//! 右键/托盘菜单「🧰 工具 ▸ 🧠 她的记忆」：打开 memory.json、清空记忆（弹框二次确认）。
//! 菜单项本身在 menu.rs 的 MENU 表里（`Sub(MIND_TITLE, MIND)`）；这里只管 id → 动作 和确认框。
use crate::types::{MenuAction, MindAct};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_DEFBUTTON2, MB_ICONQUESTION, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO, MessageBoxW,
};
use windows::core::HSTRING;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        assert_eq!(parse("mind:open"), Some(MenuAction::Mind(MindAct::OpenFile)));
        assert_eq!(parse("mind:clear"), Some(MenuAction::Mind(MindAct::ClearAsk)));
        assert_eq!(parse("mind:x"), None);
        assert_eq!(parse("memo:open"), None);
        // 菜单表里有这两项，而且能映射成动作
        let ids = super::super::menu::all_ids();
        for id in ["mind:open", "mind:clear"] {
            assert!(ids.iter().any(|x| x == id), "{id} not in MENU");
            assert!(super::super::menu::parse_id(id, |_| false).is_some());
        }
    }
}
