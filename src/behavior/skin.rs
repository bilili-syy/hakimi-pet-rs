//! 角色包的行为层钩子（数据在 core::skin，立绘在 render::skin_loader，菜单在 platform::skin_menu）
//! - 换装菜单「🎭 角色」发来的 `MenuAction::Costume("skin:…")`：换皮肤、刷新列表、写 save.json 的 `skin`
//! - 台词替换：skin.json 的 `lines` 按原台词全文或分类名（下表）换掉固定台词；聊天回复不换
use super::{Behavior, St, lines};
use crate::core::skin;

/// 台词分类名（skin.json `lines` 的键，不分大小写）→ 原台词表
pub const CATEGORIES: &[(&str, &[&str])] = &[
    ("poke", lines::POKE), ("poke_great", lines::POKE_GREAT), ("poke_sad", lines::POKE_SAD),
    ("poke_tired", lines::POKE_TIRED), ("poke_hungry", lines::POKE_HUNGRY), ("poke_mad", lines::POKE_MAD),
    ("feed", lines::FEED), ("feed_starving", lines::FEED_STARVING), ("full", lines::FULL), ("pat", lines::PAT),
    ("drag", lines::DRAG), ("land_big", lines::LAND_BIG), ("hello", lines::HELLO), ("walk", lines::WALK),
    ("edge", lines::EDGE), ("sit", lines::SIT), ("sleepy", lines::SLEEPY), ("wake", lines::WAKE),
    ("wake_up", lines::WAKE_UP), ("night", lines::NIGHT), ("hungry", lines::HUNGRY), ("starving", lines::STARVING),
    ("tired", lines::TIRED), ("sad", lines::SAD), ("think", lines::THINK), ("water", lines::WATER),
    ("stand_up", lines::STAND_UP), ("off_work", lines::OFF_WORK), ("pomo_done", lines::POMO_DONE),
    ("pomo_cancel", lines::POMO_CANCEL), ("chase", lines::CHASE), ("peek", lines::PEEK), ("reloaded", lines::RELOADED),
    ("outfit_on", lines::OUTFIT_ON), ("outfit_off", lines::OUTFIT_OFF), ("moyu_on", lines::MOYU_ON),
    ("moyu_off", lines::MOYU_OFF), ("moyu_back", lines::MOYU_BACK), ("forgot", lines::FORGOT), ("shy", lines::SHY),
    ("rub", lines::RUB), ("dizzy", lines::DIZZY), ("surprised", lines::SURPRISED), ("cry", lines::CRY),
    ("hiss", lines::HISS), ("yawn", lines::YAWN), ("love", lines::LOVE), ("stare", lines::STARE),
    ("dance", lines::DANCE), ("song", lines::SONG), ("honey", lines::HONEY), ("fast", lines::FAST), ("slow", lines::SLOW),
];

/// 换角色成功后她说的话（分类名 skin_on，可以在 lines 里覆盖）
pub const SKIN_ON: &[&str] = &["变身完成！现在是「{name}」啦～", "当当～「{name}」来陪你了！"];
const SKIN_MISSING: &str = "找不到这个角色了喵……文件夹是不是被挪走了？右键「刷新皮肤列表」试试";

/// 这句台词属于哪个分类（找不到 None）
pub fn category_of(text: &str) -> Option<&'static str> {
    CATEGORIES.iter().find(|(_, l)| l.contains(&text)).map(|(k, _)| *k)
}

/// 台词钩子：当前角色包替换后的台词（没有替换 → None）
pub fn line_override(text: &str) -> Option<String> { skin::line_override(text, category_of(text)) }

impl Behavior {
    /// `Costume("skin:…")` 交到这里；不是皮肤动作返回 false，交还给原来的换装逻辑
    pub(super) fn on_skin_menu(&mut self, id: &str) -> bool {
        if id == skin::MENU_OPEN {
            // 平台层负责打开文件夹
            return true;
        }
        if id == skin::MENU_RELOAD {
            let (n, bad) = if self.io { skin::rescan() } else { (0, 0) };
            // 当前包被删了会自动回到内置；存档里记的 id 不动（包放回来再刷新就恢复）
            let mut t = format!("找到 {n} 个角色包喵！");
            if bad > 0 { t.push_str(&format!("有 {bad} 个读不了，详情在日志里")); }
            self.say(&t, 3000);
            return true;
        }
        let Some(target) = id.strip_prefix(skin::MENU_PREFIX) else { return false };
        let target = if target.is_empty() { skin::BUILTIN_ID } else { target };
        let switched = if self.io {
            skin::set_active(target).map(|s| s.map(|s| s.display_name().to_string()))
        } else if target == skin::BUILTIN_ID {
            Ok(None)
        } else {
            Err(())
        };
        match switched {
            Ok(name) => {
                skin::stamp(&mut self.stats.data.extra, target);
                self.save();
                self.call(format!("skin:{target}"));
                let name = name.unwrap_or_else(|| "哈基米".into());
                let line = skin::line_override("", Some("skin_on"))
                    .unwrap_or_else(|| super::pick(SKIN_ON))
                    .replace("{name}", &name);
                self.react(St::Happy, 2500, Some(line));
            }
            Err(()) => self.say(SKIN_MISSING, 3000),
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stats::Stats;
    use crate::types::{AppEvent, EventSender, MenuAction};

    fn mk() -> Behavior {
        let (s, _) = Stats::load(None);
        Behavior::with(EventSender::new(|_| {}), s, false, 0.0, String::new(), Vec::new(), false)
    }

    fn menu(b: &mut Behavior, id: &str, now: u64) {
        let mut out = Vec::new();
        b.event_at(AppEvent::Menu(MenuAction::Costume(id.into())), now, &mut out);
    }

    #[test]
    fn categories_cover_lines() {
        assert_eq!(category_of("喵？找哈基米有事吗～"), Some("poke"));
        assert_eq!(category_of("小鱼干！哈基米最爱了喵～"), Some("feed"));
        assert_eq!(category_of("随便说的"), None);
        // 分类名唯一
        let mut names: Vec<_> = CATEGORIES.iter().map(|c| c.0).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), CATEGORIES.len());
        // 内置皮肤（测试里没有激活的角色包）不替换
        assert_eq!(line_override("喵？找哈基米有事吗～"), None);
    }

    #[test]
    fn skin_menu_switch_and_save_field() {
        let mut b = mk();
        menu(&mut b, "skin:hakimi", 3000);
        assert_eq!(b.state, St::Happy);
        assert_eq!(skin::saved_id(&b.stats.data.extra), "hakimi");
        assert!(b.calls.iter().any(|c| c == "skin:hakimi"));
        // 换角色不动整套衣服
        assert_eq!(b.scene.costume, "");
        // 不存在的包：不写存档，提示一句
        b.stats.data.extra.remove(skin::SAVE_KEY);
        menu(&mut b, "skin:不存在的包", 6000);
        assert!(!b.stats.data.extra.contains_key(skin::SAVE_KEY));
        assert!(b.bubble.as_ref().is_some_and(|x| x.text == SKIN_MISSING));
        // 刷新 / 打开文件夹不当成换装
        menu(&mut b, skin::MENU_RELOAD, 9000);
        assert!(b.bubble.as_ref().is_some_and(|x| x.text.starts_with("找到 0 个角色包")));
        menu(&mut b, skin::MENU_OPEN, 12000);
        assert_eq!(b.scene.costume, "");
        // 普通换装照旧
        menu(&mut b, "maid", 15000);
        assert_eq!(b.scene.costume, "maid");
    }
}
