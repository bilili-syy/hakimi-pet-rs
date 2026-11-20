//! 「👗 换装」里的「🎭 角色」子菜单：列出所有角色包（内置哈基米在最前，当前的打勾），
//! 再加「📂 打开皮肤文件夹」「🔄 刷新皮肤列表」。
//! 菜单 id：`skin:<文件夹名>` / `skin:@open` / `skin:@reload`，都映射成 `MenuAction::Costume("skin:…")`
//! （types.rs 冻结，不加新变体）；行为层 behavior/skin.rs 认这个前缀，打开文件夹由平台层 on_menu 做。
//! 皮肤列表变了（core::skin::version）才重建；菜单弹出期间不动它。
use crate::core::skin;
use crate::types::MenuAction;
use std::cell::RefCell;
use tray_icon::menu::{CheckMenuItem, MenuItem, PredefinedMenuItem, Submenu};

/// 子菜单里最多列多少个角色包
const MAX_ITEMS: usize = 40;

struct SkinMenu {
    sub: Submenu,
    items: Vec<(String, CheckMenuItem)>,
    shown: Option<u64>,
}

thread_local! {
    static SM: RefCell<Option<SkinMenu>> = const { RefCell::new(None) };
}

/// 菜单文字里的 & 是助记符，要转义；太长的截断
fn label(s: &str, max: usize) -> String {
    let mut t: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        t.push('…');
    }
    t.replace('&', "&&")
}

/// 追加到「换装」子菜单末尾
pub fn init(costume_sub: &Submenu) {
    let sub = Submenu::new("🎭 角色", true);
    let _ = costume_sub.append(&PredefinedMenuItem::separator());
    let _ = costume_sub.append(&sub);
    SM.with(|m| *m.borrow_mut() = Some(SkinMenu { sub, items: Vec::new(), shown: None }));
    refresh(false);
}

pub fn refresh(popup_open: bool) {
    if popup_open {
        return;
    }
    let v = skin::version();
    let active = skin::active_id();
    SM.with(|m| {
        let Ok(mut g) = m.try_borrow_mut() else { return };
        let Some(m) = g.as_mut() else { return };
        if m.shown != Some(v) {
            m.shown = Some(v);
            while m.sub.remove_at(0).is_some() {}
            m.items.clear();
            let list = skin::list();
            for (id, name) in list.iter().take(MAX_ITEMS) {
                let c = CheckMenuItem::with_id(format!("{}{id}", skin::MENU_PREFIX), label(name, 30), true, false, None);
                let _ = m.sub.append(&c);
                m.items.push((id.clone(), c));
            }
            if list.len() > MAX_ITEMS {
                let _ = m.sub.append(&MenuItem::new(format!("…还有 {} 个（太多了没列出来）", list.len() - MAX_ITEMS), false, None));
            }
            let bad = skin::errors().len();
            if bad > 0 {
                let _ = m.sub.append(&MenuItem::new(format!("（{bad} 个角色包读不了，见 hakimi.log）"), false, None));
            }
            let _ = m.sub.append(&PredefinedMenuItem::separator());
            let _ = m.sub.append(&MenuItem::with_id(skin::MENU_OPEN, "📂 打开皮肤文件夹", true, None));
            let _ = m.sub.append(&MenuItem::with_id(skin::MENU_RELOAD, "🔄 刷新皮肤列表", true, None));
        }
        // 勾选状态每次都同步（点了已选中的那项，muda 会自己把勾去掉）
        for (id, c) in &m.items {
            c.set_checked(*id == active);
        }
    });
}

pub fn shutdown() {
    SM.with(|m| {
        if let Ok(mut g) = m.try_borrow_mut() {
            *g = None;
        }
    });
}

/// 菜单 id → 动作（纯映射，可测试）
pub fn parse(id: &str) -> Option<MenuAction> {
    let rest = id.strip_prefix(skin::MENU_PREFIX)?;
    if rest.is_empty() {
        return None;
    }
    Some(MenuAction::Costume(id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        assert_eq!(parse("skin:小薄荷"), Some(MenuAction::Costume("skin:小薄荷".into())));
        assert_eq!(parse("skin:hakimi"), Some(MenuAction::Costume("skin:hakimi".into())));
        assert_eq!(parse(skin::MENU_OPEN), Some(MenuAction::Costume(skin::MENU_OPEN.into())));
        assert_eq!(parse(skin::MENU_RELOAD), Some(MenuAction::Costume(skin::MENU_RELOAD.into())));
        assert_eq!(parse("skin:"), None);
        assert_eq!(parse("costume:maid"), None);
        assert_eq!(label("a&b", 30), "a&&b");
        assert_eq!(label("一二三四", 2), "一二…");
    }
}
