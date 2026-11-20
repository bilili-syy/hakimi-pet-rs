//! 托盘图标 + 右键菜单（右键 & 托盘共用一份菜单，和 main.js menuTemplate 一致）。
//! 菜单对象只在 UI 线程（thread_local）；点击经 MenuEvent 回调放进队列再 PostMessage 给主窗口。
use super::{MENU_QUEUE, POMO_END, WM_APP_MENU, main_hwnd, now_ms, timers};
use crate::types::MenuAction;
use std::cell::{Cell, RefCell};
use tray_icon::menu::{CheckMenuItem, ContextMenu, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{IsWindowVisible, PostMessageW};

pub const COSTUMES: [(&str, &str); 4] =
    [("", "🤍 原装卫衣"), ("xmas", "🎄 圣诞小红裙"), ("maid", "🎀 女仆装"), ("jk", "🏫 JK 水手服")];
pub const OUTFITS: [(&str, &str); 3] = [("santa", "🎅 圣诞帽"), ("glasses", "👓 圆眼镜"), ("scarf", "🧣 四色围巾")];
/// 番茄钟项在顶层菜单里的位置
const POMO_POS: usize = 15;
/// 托盘左键单击的伪菜单 id
pub const TRAY_LEFT: &str = "tray:left";

struct Menus {
    root: Menu,
    pomo_sub: Submenu,
    pomo_stop: MenuItem,
    pomo_running_shown: bool,
    costumes: Vec<(&'static str, CheckMenuItem)>,
    outfits: Vec<(&'static str, CheckMenuItem)>,
    tts: CheckMenuItem,
    tts_chat: CheckMenuItem,
    moyu: CheckMenuItem,
    autostart: CheckMenuItem,
    visible: MenuItem,
    tray: Option<TrayIcon>,
}

thread_local! {
    static MENUS: RefCell<Option<Menus>> = const { RefCell::new(None) };
    /// 右键菜单弹出期间 muda 借着 Menu，不能增删项
    static POPUP_OPEN: Cell<bool> = const { Cell::new(false) };
}

fn push_id(id: String) {
    MENU_QUEUE.lock().unwrap().push_back(id);
    if let Some(h) = main_hwnd() {
        unsafe {
            let _ = PostMessageW(Some(h), WM_APP_MENU, WPARAM(0), LPARAM(0));
        }
    }
}

fn tray_icon() -> Option<Icon> {
    let png = crate::assets::get("tray.png")?;
    let pm = tiny_skia::Pixmap::decode_png(png).ok()?;
    // tiny-skia 解出来是预乘的，Icon 要直通 RGBA
    let mut rgba = Vec::with_capacity(pm.data().len());
    for p in pm.pixels() {
        let c = p.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Icon::from_rgba(rgba, pm.width(), pm.height()).ok()
}

/// 建菜单和托盘。必须在 UI 线程、主窗口建好之后调用
pub fn init() {
    MenuEvent::set_event_handler(Some(|e: MenuEvent| push_id(e.id.0)));
    TrayIconEvent::set_event_handler(Some(|e: TrayIconEvent| {
        if let TrayIconEvent::Click { button, button_state, .. } = e {
            match (button, button_state) {
                // 左键单击托盘：藏起来 / 叫出来
                (MouseButton::Left, MouseButtonState::Up) => push_id(TRAY_LEFT.into()),
                // 右键按下：菜单马上要弹出来了，先同步勾选状态（回调就在 UI 线程上）
                (MouseButton::Right, MouseButtonState::Down) => refresh(),
                _ => {}
            }
        }
    }));

    let item = |id: &str, text: &str| MenuItem::with_id(id, text, true, None);
    let check = |id: &str, text: &str| CheckMenuItem::with_id(id, text, true, false, None);
    let sep = PredefinedMenuItem::separator;

    let root = Menu::new();
    let pomo_sub = Submenu::new("🍅 番茄钟", true);
    for m in [25, 45, 15] {
        let _ = pomo_sub.append(&item(&format!("pomo:{m}"), &format!("{m} 分钟")));
    }
    let pomo_stop = item("pomo:stop", "🍅 结束番茄钟");

    let costume_sub = Submenu::new("👗 换装", true);
    let costumes: Vec<_> = COSTUMES.iter().map(|(id, name)| (*id, check(&format!("costume:{id}"), name))).collect();
    let outfits: Vec<_> = OUTFITS.iter().map(|(id, name)| (*id, check(&format!("outfit:{id}"), name))).collect();
    for (_, c) in &costumes {
        let _ = costume_sub.append(c);
    }
    let _ = costume_sub.append(&sep());
    for (_, c) in &outfits {
        let _ = costume_sub.append(c);
    }
    let _ = costume_sub.append(&sep());
    let _ = costume_sub.append(&item("outfit:clear", "摘掉配件"));
    // 「🎭 角色」子菜单（角色包）追加在换装末尾
    super::skin_menu::init(&costume_sub);

    let dance_sub = Submenu::new("💃 跳个舞", true);
    for s in &crate::behavior::dance::SCORES {
        let _ = dance_sub.append(&item(&format!("dance:{}", s.id), &format!("{} {}（{} 秒）", s.icon, s.name, s.secs as u32)));
    }
    let _ = dance_sub.append(&sep());
    let _ = dance_sub.append(&item("dance:", "🎲 随便跳一支"));

    let tts = check("tts", "🔊 让哈基米出声");
    let tts_chat = check("ttschat", "🎙️ 只念聊天回复");
    let moyu = check("moyu", "👀 摸鱼监督");
    let autostart = check("autostart", "开机自动启动");
    let settings = Submenu::new("⚙️ 设置", true);
    let _ = settings.append_items(&[
        &item("settings:open", "打开设置文件（API Key、语音、提醒）"),
        &item("settings:reload", "重新加载设置"),
        &tts,
        &tts_chat,
        &moyu,
        &item("forget", "忘掉聊天记录"),
        &item("datadir", "打开数据文件夹"),
        &sep(),
        &autostart,
    ]);
    let visible = item("visible", "🙈 先藏起来");

    let _ = root.append_items(&[
        &item("chat", "💬 和哈基米聊天"),
        &item("feed", "🐟 扔小鱼干（拖到她嘴边也行）"),
        &item("feednow", "🍽️ 直接喂到嘴里"),
        &item("pat", "✋ 摸摸头（滚轮也能撸）"),
        &item("honey", "🍯 喂蜂蜜水（喝完跑得飞快）"),
        &item("game", "🪶 小游戏：逗猫棒"),
        &item("song", "🎵 哈基米之歌"),
        &item("songdir", "📂 放歌的文件夹（把歌拖进去）"),
        &item("status", "📊 看看状态"),
        &item("checkin", "📅 今日签到"),
        &item("weather", "🌦️ 天气播报"),
        &sep(),
        &item("sleep", "💤 去睡觉"),
        &item("walk", "🚶 去溜达"),
        &item("climb", "🧗 爬到当前窗口上"),
        &pomo_sub, // POMO_POS
        &costume_sub,
        &dance_sub,
        &sep(),
        &settings,
        &visible,
        &item("center", "📍 叫她到屏幕中间"),
        &item("quit", "退出"),
    ]);
    // 「📝 备忘录」子菜单插在换装后面
    super::memo_menu::init(&root, costume_sub.id());

    let mut builder = TrayIconBuilder::new()
        .with_menu(Box::new(root.clone()))
        .with_menu_on_left_click(false)
        .with_tooltip("哈基米");
    if let Some(icon) = tray_icon() {
        builder = builder.with_icon(icon);
    }
    let tray = match builder.build() {
        Ok(t) => Some(t),
        Err(e) => {
            crate::core::log::log(&format!("tray failed: {e}"));
            None
        }
    };

    MENUS.with(|m| {
        *m.borrow_mut() = Some(Menus {
            root,
            pomo_sub,
            pomo_stop,
            pomo_running_shown: false,
            costumes,
            outfits,
            tts,
            tts_chat,
            moyu,
            autostart,
            visible,
            tray,
        })
    });
    refresh();
}

/// 按当前配置/状态同步勾选、文字、番茄钟项、托盘提示
pub fn refresh() {
    let cfg = crate::core::config::get();
    let visible = main_hwnd().is_some_and(|h| unsafe { IsWindowVisible(h).as_bool() });
    let pomo_end = POMO_END.load(std::sync::atomic::Ordering::Relaxed);
    let autostart = super::sys::is_autostart();
    MENUS.with(|m| {
        let Ok(mut guard) = m.try_borrow_mut() else { return };
        let Some(m) = guard.as_mut() else { return };
        let costume = if COSTUMES.iter().any(|(id, _)| *id == cfg.costume) { cfg.costume.as_str() } else { "" };
        for (id, c) in &m.costumes {
            c.set_checked(*id == costume);
        }
        for (id, c) in &m.outfits {
            c.set_checked(cfg.outfit.iter().any(|o| o == id));
        }
        m.tts.set_checked(cfg.tts.enabled);
        m.tts_chat.set_checked(cfg.tts.speak == "chat");
        m.moyu.set_checked(cfg.moyu.enabled);
        m.autostart.set_checked(autostart);
        m.visible.set_text(if visible { "🙈 先藏起来" } else { "👀 叫她出来" });

        let running = pomo_end != 0;
        if running {
            let left = timers::minutes_left(pomo_end, now_ms());
            m.pomo_stop.set_text(format!("🍅 结束番茄钟（还剩 {left} 分钟）"));
        }
        if running != m.pomo_running_shown && !POPUP_OPEN.with(|p| p.get()) {
            if running {
                let _ = m.root.remove(&m.pomo_sub);
                let _ = m.root.insert(&m.pomo_stop, POMO_POS);
            } else {
                let _ = m.root.remove(&m.pomo_stop);
                let _ = m.root.insert(&m.pomo_sub, POMO_POS);
            }
            m.pomo_running_shown = running;
        }
        if let Some(t) = &m.tray {
            let _ = t.set_tooltip(Some(if running { "哈基米 · 番茄钟进行中" } else { "哈基米" }));
        }
    });
    super::memo_menu::refresh(POPUP_OPEN.with(|p| p.get()));
    super::skin_menu::refresh(POPUP_OPEN.with(|p| p.get()));
}

/// 在光标处弹出右键菜单（模态，返回时点击已进队列）
pub fn popup() {
    refresh();
    let Some(h) = main_hwnd() else { return };
    let root = MENUS.with(|m| m.borrow().as_ref().map(|m| m.root.clone()));
    let Some(root) = root else { return };
    POPUP_OPEN.with(|p| p.set(true));
    unsafe {
        root.show_context_menu_for_hwnd(h.0 as isize, None);
    }
    POPUP_OPEN.with(|p| p.set(false));
    refresh();
}

/// 删除托盘图标（退出时）
pub fn shutdown() {
    super::memo_menu::shutdown();
    super::skin_menu::shutdown();
    MENUS.with(|m| {
        if let Ok(mut g) = m.try_borrow_mut() {
            *g = None;
        }
    });
}

/// 菜单 id → MenuAction。checked = 勾选项点击后的新状态（muda 已经自己翻转过）
pub fn action_for(id: &str) -> Option<MenuAction> {
    let checked = |f: fn(&Menus) -> &CheckMenuItem| {
        MENUS.with(|m| m.borrow().as_ref().map(|m| f(m).is_checked())).unwrap_or(false)
    };
    let outfit_checked = |oid: &str| {
        MENUS.with(|m| {
            m.borrow().as_ref().and_then(|m| m.outfits.iter().find(|(i, _)| *i == oid).map(|(_, c)| c.is_checked()))
        })
        .unwrap_or(false)
    };
    parse_id(id, |name| match name {
        "tts" => checked(|m| &m.tts),
        "ttschat" => checked(|m| &m.tts_chat),
        "moyu" => checked(|m| &m.moyu),
        "autostart" => checked(|m| &m.autostart),
        other => other.strip_prefix("outfit:").is_some_and(outfit_checked),
    })
}

/// 纯映射（可测试）。checked(id) 给出勾选项的新状态
pub fn parse_id(id: &str, checked: impl Fn(&str) -> bool) -> Option<MenuAction> {
    use MenuAction as A;
    Some(match id {
        "chat" => A::Chat,
        "feed" => A::Feed,
        "feednow" => A::FeedNow,
        "game" => A::Game,
        "checkin" => A::CheckIn,
        "weather" => A::WeatherReport,
        "climb" => A::Climb,
        "pat" => A::Pat,
        "honey" => A::Honey,
        "song" => A::Song,
        "songdir" => A::OpenSongDir,
        "status" => A::Status,
        "sleep" => A::Sleep,
        "walk" => A::Walk,
        "pomo:stop" => A::PomodoroStop,
        "outfit:clear" => A::ClearOutfit,
        "settings:open" => A::OpenSettings,
        "settings:reload" => A::ReloadSettings,
        "tts" => A::Tts(checked(id)),
        "ttschat" => A::TtsChatOnly(checked(id)),
        "moyu" => A::Moyu(checked(id)),
        "forget" => A::ForgetChat,
        "datadir" => A::OpenDataDir,
        "autostart" => A::AutoStart(checked(id)),
        "visible" | TRAY_LEFT => A::ToggleVisible,
        "center" => A::CallToCenter,
        "quit" => A::Quit,
        _ => {
            if let Some(a) = super::memo_menu::parse(id) {
                a
            } else if let Some(a) = super::skin_menu::parse(id) {
                a
            } else if let Some(m) = id.strip_prefix("pomo:") {
                A::PomodoroStart(m.parse().ok()?)
            } else if let Some(d) = id.strip_prefix("dance:") {
                A::Dance(d.to_string())
            } else if let Some(c) = id.strip_prefix("costume:") {
                A::Costume(c.to_string())
            } else {
                let o = id.strip_prefix("outfit:")?;
                A::Outfit(o.to_string(), checked(id))
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MenuAction as A;

    #[test]
    fn ids() {
        let on = |_: &str| true;
        let off = |_: &str| false;
        assert_eq!(parse_id("chat", off), Some(A::Chat));
        assert_eq!(parse_id("pomo:25", off), Some(A::PomodoroStart(25)));
        assert_eq!(parse_id("pomo:stop", off), Some(A::PomodoroStop));
        assert_eq!(parse_id("costume:", off), Some(A::Costume(String::new())));
        assert_eq!(parse_id("costume:jk", off), Some(A::Costume("jk".into())));
        assert_eq!(parse_id("outfit:santa", on), Some(A::Outfit("santa".into(), true)));
        assert_eq!(parse_id("outfit:clear", on), Some(A::ClearOutfit));
        assert_eq!(parse_id("tts", on), Some(A::Tts(true)));
        assert_eq!(parse_id("ttschat", off), Some(A::TtsChatOnly(false)));
        assert_eq!(parse_id("moyu", on), Some(A::Moyu(true)));
        assert_eq!(parse_id("autostart", off), Some(A::AutoStart(false)));
        assert_eq!(parse_id(TRAY_LEFT, off), Some(A::ToggleVisible));
        assert_eq!(parse_id("quit", off), Some(A::Quit));
        assert_eq!(parse_id("dance:disco", off), Some(A::Dance("disco".into())));
        assert_eq!(parse_id("dance:", off), Some(A::Dance(String::new())));
        assert_eq!(parse_id("nope", off), None);
        assert_eq!(parse_id("game", off), Some(A::Game));
        assert_eq!(parse_id("feednow", off), Some(A::FeedNow));
        assert_eq!(parse_id("checkin", off), Some(A::CheckIn));
        assert_eq!(parse_id("weather", off), Some(A::WeatherReport));
        assert_eq!(parse_id("climb", off), Some(A::Climb));
        assert_eq!(parse_id("skin:demo", off), Some(A::Costume("skin:demo".into())));
    }
}
