//! 托盘图标 + 右键菜单（右键 & 托盘共用一份菜单）。
//!
//! 菜单结构全在下面的 `MENU` 表里（数据驱动）：顶层只放常用的几项，其余按用途分组成子菜单。
//! 以后加功能：在对应分组里加一行 `Item("id", "文字")`，再在 `parse_id` 里把 id 映射成 MenuAction。
//! 勾选状态 / 动态文字 / 能不能点，由 `checked` / `label` / `enabled` 三个纯函数按当前状态给出（可测试）。
//! 原来菜单里的那些配置开关（出声、只念聊天、摸鱼监督、开机自启…）都搬进了设置窗口（platform/settings.rs）。
//!
//! 菜单对象只在 UI 线程（thread_local）；点击经 MenuEvent 回调放进队列再 PostMessage 给主窗口。
use super::{MENU_QUEUE, WM_APP_MENU, main_hwnd, pomodoro_left};
use crate::types::MenuAction;
use std::cell::{Cell, RefCell};
use tray_icon::menu::{CheckMenuItem, ContextMenu, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
use windows::Win32::UI::WindowsAndMessaging::{IsWindowVisible, KillTimer, PostMessageW, SetTimer};

pub const OUTFITS: [(&str, &str); 2] = [("santa", "🎅 圣诞帽"), ("glasses", "👓 圆眼镜")];
/// 托盘左键单击的伪菜单 id
pub const TRAY_LEFT: &str = "tray:left";
/// 托盘左键单击要等双击时间过了才算（双击 = 打开设置），用主窗口上的这个定时器
pub(crate) const TRAY_CLICK_TIMER: usize = 7;

// ================================================================ 菜单表

/// 菜单表的一行
pub enum Node {
    /// 普通项 (id, 文字)
    Item(&'static str, &'static str),
    /// 勾选项 (id, 文字)，勾没勾由 `checked` 决定
    Check(&'static str, &'static str),
    Sep,
    /// 子菜单 (文字, 内容)
    Sub(&'static str, &'static [Node]),
    /// 运行时生成的子菜单："dance"（舞谱表）/ "memo"（备忘录，平台层 memo_menu 管）/ "skin"（角色包，skin_menu 管）
    Gen(&'static str),
}
use Node::{Check, Gen, Item, Sep, Sub};

const FEED: &[Node] = &[
    Item("feed", "🐟 扔小鱼干（拖到她嘴边也行）"),
    Item("feednow", "🍽️ 直接喂到嘴里"),
    Item("honey", "🍯 喂蜂蜜水（喝完跑得飞快）"),
];
const PLAY: &[Node] = &[
    Item("pat", "✋ 摸摸头（滚轮也能撸）"),
    Item("climb", "🧗 爬到当前窗口上"),
    Item("walk", "🚶 去溜达"),
    Item("sleep", "💤 去睡觉"),
    Sep,
    Item("center", "📍 叫她到屏幕中间"),
];
const FUN: &[Node] = &[
    Gen("dance"),
    Item("song", "🎵 哈基米之歌"),
    Item("game", "🪶 小游戏：逗猫棒"),
    Sep,
    Item("songdir", "📂 放歌的文件夹（把歌拖进去）"),
];
pub const POMO_TITLE: &str = "🍅 番茄钟";
const POMO: &[Node] = &[
    Item("pomo:25", "25 分钟"),
    Item("pomo:45", "45 分钟"),
    Item("pomo:15", "15 分钟"),
    Sep,
    Item("pomo:stop", "结束番茄钟"),
];
/// 「她的记忆」（platform/mind_menu.rs 把 id 映射成 MenuAction::Mind）
pub const MIND_TITLE: &str = "🧠 她的记忆";
const MIND: &[Node] = &[
    Item("mind:open", "📂 打开 memory.json"),
    Item("mind:clear", "🗑️ 清空记忆…"),
];
const TOOLS: &[Node] = &[
    Gen("memo"),
    Sub(MIND_TITLE, MIND),
    Sub(POMO_TITLE, POMO),
    Item("checkin", "📅 今日签到"),
    Item("weather", "🌦️ 天气播报"),
    Item("report", "📊 本周陪伴报告"),
    Item("chatexport", "💬 导出聊天记录"),
];
const COSTUME: &[Node] = &[
    // 和 core::costume::COSTUMES 一一对应（table_integrity 会检查）
    Check("costume:sailor", "🌈 奶白水手服（默认）"),
    Check("costume:hoodie", "🤍 原装卫衣"),
    Check("costume:pajama", "🌙 猫耳睡衣"),
    Check("costume:idol", "🎤 偶像演出服"),
    Check("costume:swim", "🏖️ 泳装"),
    Check("costume:xmas", "🎄 圣诞小红裙"),
    Check("costume:maid", "🎀 女仆装"),
    Check("costume:jk", "🏫 JK 水手服"),
    Sep,
    Check("outfit:santa", "🎅 圣诞帽"),
    Check("outfit:glasses", "👓 圆眼镜"),
    Sep,
    Item("outfit:clear", "摘掉配件"),
    Sep,
    // 「🎭 角色」：角色包列表 + 打开皮肤文件夹 + 刷新（platform/skin_menu.rs）
    Gen("skin"),
];

/// 整个右键 / 托盘菜单
pub const MENU: &[Node] = &[
    Item("chat", "💬 和哈基米聊天"),
    Sub("🍽️ 喂养", FEED),
    Sub("🤲 互动", PLAY),
    Sub("🎮 娱乐", FUN),
    Sub("🧰 工具", TOOLS),
    Sub("👗 换装", COSTUME),
    Item("status", "📊 看看状态"),
    Sep,
    Item("settings", "⚙️ 设置…"),
    Item("visible", "🙈 先藏起来"),
    Item("quit", "退出"),
];

/// 菜单上看得到的动态状态
#[derive(Clone, Debug, Default)]
pub struct Ctx {
    pub costume: String,
    pub outfits: Vec<String>,
    pub visible: bool,
    /// 番茄钟还剩几分钟（没开 = None）
    pub pomo_left: Option<u32>,
}

/// 勾选项现在该不该勾
pub fn checked(id: &str, c: &Ctx) -> bool {
    if let Some(k) = id.strip_prefix("costume:") {
        return crate::core::costume::normalize(k) == crate::core::costume::normalize(&c.costume);
    }
    if let Some(o) = id.strip_prefix("outfit:") {
        return c.outfits.iter().any(|x| x == o);
    }
    false
}

/// 要换文字的项（None = 用表里的）
pub fn label(id: &str, c: &Ctx) -> Option<String> {
    match id {
        "visible" => Some(if c.visible { "🙈 先藏起来" } else { "👀 叫她出来" }.into()),
        "pomo:stop" => Some(match c.pomo_left {
            Some(m) => format!("结束番茄钟（还剩 {m} 分钟）"),
            None => "结束番茄钟".into(),
        }),
        POMO_TITLE => Some(match c.pomo_left {
            Some(m) => format!("🍅 番茄钟（还剩 {m} 分钟）"),
            None => POMO_TITLE.into(),
        }),
        _ => None,
    }
}

/// 能不能点（番茄钟开着时只能结束，没开时只能开始）
pub fn enabled(id: &str, c: &Ctx) -> bool {
    if id == "pomo:stop" {
        return c.pomo_left.is_some();
    }
    if id.starts_with("pomo:") {
        return c.pomo_left.is_none();
    }
    true
}

/// 运行时生成的子菜单里的 (id, 文字)
fn gen_items(kind: &str) -> (String, Vec<(String, String)>) {
    match kind {
        "dance" => {
            let mut v: Vec<(String, String)> = crate::behavior::dance::SCORES
                .iter()
                .map(|s| (format!("dance:{}", s.id), format!("{} {}（{} 秒）", s.icon, s.name, s.secs as u32)))
                .collect();
            v.push((String::new(), String::new())); // 分隔线
            v.push(("dance:".into(), "🎲 随便跳一支".into()));
            v.push((format!("dance:{}", crate::behavior::dance::SONG_ID), "🎵 跟着我的歌跳（songs 文件夹）".into()));
            v.push((format!("dance:{}", crate::behavior::dance::LISTEN_ID), "🎧 跟着正在放的歌跳".into()));
            ("💃 跳个舞".into(), v)
        }
        _ => (String::new(), Vec::new()),
    }
}

/// 表里所有能点的 id（含运行时生成的舞谱项；备忘录条目是动态的，不在内）
pub fn all_ids() -> Vec<String> {
    fn walk(nodes: &[Node], out: &mut Vec<String>) {
        for n in nodes {
            match n {
                Item(id, _) | Check(id, _) => out.push(id.to_string()),
                Sub(_, kids) => walk(kids, out),
                Gen(k) => out.extend(gen_items(k).1.into_iter().map(|x| x.0).filter(|id| !id.is_empty())),
                Sep => {}
            }
        }
    }
    let mut v = Vec::new();
    walk(MENU, &mut v);
    v
}

// ================================================================ 建菜单 / 刷新

enum Handle {
    Item(MenuItem),
    Check(CheckMenuItem),
    Sub(Submenu),
}

struct Menus {
    root: Menu,
    /// (id 或子菜单标题, 句柄)
    items: Vec<(String, Handle)>,
    tray: Option<TrayIcon>,
}

thread_local! {
    static MENUS: RefCell<Option<Menus>> = const { RefCell::new(None) };
    /// 右键菜单弹出期间 muda 借着 Menu，不能增删项
    static POPUP_OPEN: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn push_id(id: String) {
    MENU_QUEUE.lock().unwrap_or_else(|e| e.into_inner()).push_back(id);
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

/// 往 Menu 或 Submenu 里加一项
trait Container {
    fn add(&self, item: &dyn IsMenuItem);
}
impl Container for Menu {
    fn add(&self, item: &dyn IsMenuItem) {
        let _ = self.append(item);
    }
}
impl Container for Submenu {
    fn add(&self, item: &dyn IsMenuItem) {
        let _ = self.append(item);
    }
}

fn build(nodes: &[Node], into: &dyn Container, items: &mut Vec<(String, Handle)>) {
    for n in nodes {
        match n {
            Item(id, text) => {
                let it = MenuItem::with_id(*id, *text, true, None);
                into.add(&it);
                items.push((id.to_string(), Handle::Item(it)));
            }
            Check(id, text) => {
                let it = CheckMenuItem::with_id(*id, *text, true, false, None);
                into.add(&it);
                items.push((id.to_string(), Handle::Check(it)));
            }
            Sep => into.add(&PredefinedMenuItem::separator()),
            Sub(title, kids) => {
                let s = Submenu::new(*title, true);
                build(kids, &s, items);
                into.add(&s);
                items.push((title.to_string(), Handle::Sub(s)));
            }
            Gen("memo") => into.add(&super::memo_menu::init()),
            Gen("skin") => into.add(&super::skin_menu::init()),
            Gen(kind) => {
                let (title, list) = gen_items(kind);
                let s = Submenu::new(title, true);
                for (id, text) in list {
                    if id.is_empty() {
                        let _ = s.append(&PredefinedMenuItem::separator());
                    } else {
                        let it = MenuItem::with_id(id.clone(), text, true, None);
                        let _ = s.append(&it);
                        items.push((id, Handle::Item(it)));
                    }
                }
                into.add(&s);
            }
        }
    }
}

/// 建菜单和托盘。必须在 UI 线程、主窗口建好之后调用
pub fn init() {
    MenuEvent::set_event_handler(Some(|e: MenuEvent| push_id(e.id.0)));
    TrayIconEvent::set_event_handler(Some(|e: TrayIconEvent| match e {
        TrayIconEvent::Click { button, button_state, .. } => match (button, button_state) {
            // 左键单击托盘：藏起来 / 叫出来。等双击时间过了才执行，免得双击时先藏再出
            (MouseButton::Left, MouseButtonState::Up) => {
                if let Some(h) = main_hwnd() {
                    unsafe {
                        SetTimer(Some(h), TRAY_CLICK_TIMER, GetDoubleClickTime(), None);
                    }
                }
            }
            // 右键按下：菜单马上要弹出来了，先同步勾选状态（回调就在 UI 线程上）
            (MouseButton::Right, MouseButtonState::Down) => refresh(),
            _ => {}
        },
        // 双击托盘 = 打开设置
        TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } => {
            if let Some(h) = main_hwnd() {
                unsafe {
                    let _ = KillTimer(Some(h), TRAY_CLICK_TIMER);
                }
            }
            push_id("settings".into());
        }
        _ => {}
    }));

    let root = Menu::new();
    let mut items = Vec::new();
    build(MENU, &root, &mut items);

    let mut builder = TrayIconBuilder::new()
        .with_menu(Box::new(root.clone()))
        .with_menu_on_left_click(false)
        .with_tooltip("哈基米（双击打开设置）");
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

    MENUS.with(|m| *m.borrow_mut() = Some(Menus { root, items, tray }));
    refresh();
}

/// 托盘单击的定时器到了（主窗口 WM_TIMER 调）
pub(crate) fn tray_click_timer() {
    if let Some(h) = main_hwnd() {
        unsafe {
            let _ = KillTimer(Some(h), TRAY_CLICK_TIMER);
        }
    }
    push_id(TRAY_LEFT.into());
}

fn ctx_now() -> Ctx {
    let cfg = crate::core::config::get();
    Ctx {
        costume: cfg.costume,
        outfits: cfg.outfit,
        visible: main_hwnd().is_some_and(|h| unsafe { IsWindowVisible(h).as_bool() }),
        pomo_left: pomodoro_left(),
    }
}

/// 按当前配置/状态同步勾选、文字、能不能点、托盘提示
pub fn refresh() {
    let c = ctx_now();
    MENUS.with(|m| {
        let Ok(guard) = m.try_borrow() else { return };
        let Some(m) = guard.as_ref() else { return };
        for (id, h) in &m.items {
            match h {
                Handle::Check(it) => it.set_checked(checked(id, &c)),
                Handle::Item(it) => {
                    if let Some(t) = label(id, &c) {
                        it.set_text(t);
                    }
                    it.set_enabled(enabled(id, &c));
                }
                Handle::Sub(s) => {
                    if let Some(t) = label(id, &c) {
                        s.set_text(t);
                    }
                }
            }
        }
        if let Some(t) = &m.tray {
            let _ = t.set_tooltip(Some(if c.pomo_left.is_some() { "哈基米 · 番茄钟进行中" } else { "哈基米（双击打开设置）" }));
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
    let now_checked = |id: &str| {
        MENUS
            .with(|m| {
                m.borrow().as_ref().and_then(|m| {
                    m.items.iter().find(|(i, _)| i == id).and_then(|(_, h)| match h {
                        Handle::Check(c) => Some(c.is_checked()),
                        _ => None,
                    })
                })
            })
            .unwrap_or(false)
    };
    parse_id(id, now_checked)
}

/// 纯映射（可测试）。checked(id) 给出勾选项的新状态。
/// 除了菜单表里的 id，还认设置窗口按钮用的几个（settings:file / settings:reload / forget / datadir）
/// 和老的开关 id（tts / ttschat / moyu / autostart，菜单里已经没有了，留着兼容）。
pub fn parse_id(id: &str, checked: impl Fn(&str) -> bool) -> Option<MenuAction> {
    use MenuAction as A;
    Some(match id {
        "chat" => A::Chat,
        "feed" => A::Feed,
        "feednow" => A::FeedNow,
        "game" => A::Game,
        "checkin" => A::CheckIn,
        "weather" => A::WeatherReport,
        "report" => A::Report,
        "chatexport" => A::ExportChat,
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
        "settings" | "settings:open" => A::OpenSettings,
        "settings:file" => A::OpenConfigFile,
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
            } else if let Some(a) = super::mind_menu::parse(id) {
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
        assert_eq!(parse_id("settings", off), Some(A::OpenSettings));
        assert_eq!(parse_id("settings:file", off), Some(A::OpenConfigFile));
        assert_eq!(parse_id("settings:reload", off), Some(A::ReloadSettings));
    }

    /// 菜单表完整性：id 不重复、都能映射成动作、顶层够短、每个动作都还点得到
    #[test]
    fn table_integrity() {
        let ids = all_ids();
        let mut seen = std::collections::HashSet::new();
        for id in &ids {
            assert!(seen.insert(id.clone()), "duplicate menu id {id}");
            assert!(parse_id(id, |_| false).is_some(), "menu id {id} has no action");
        }
        // 顶层（不算分隔线）不超过 10 项
        let top = MENU.iter().filter(|n| !matches!(n, Sep)).count();
        assert!(top <= 10, "top level has {top} entries");
        // 换装表和 COSTUMES / OUTFITS 一致
        for (c, _) in crate::core::costume::COSTUMES {
            assert!(ids.contains(&format!("costume:{c}")));
        }
        for (o, _) in OUTFITS {
            assert!(ids.contains(&format!("outfit:{o}")));
        }
        // 舞谱都在
        for s in crate::behavior::dance::SCORES.iter() {
            assert!(ids.contains(&format!("dance:{}", s.id)));
        }
        // 菜单里能直接点到的动作
        let actions: Vec<A> = ids.iter().filter_map(|id| parse_id(id, |_| false)).collect();
        let must = [
            A::Chat, A::Feed, A::FeedNow, A::Honey, A::Pat, A::Climb, A::Walk, A::Sleep, A::CallToCenter,
            A::Song, A::Game, A::OpenSongDir, A::Dance(String::new()), A::PomodoroStart(25), A::PomodoroStop,
            A::CheckIn, A::WeatherReport, A::ClearOutfit, A::Status, A::OpenSettings, A::ToggleVisible, A::Quit,
        ];
        for a in must {
            assert!(actions.contains(&a), "{a:?} not reachable from the menu");
        }
        // 搬进设置窗口的那些（设置窗口按钮发这些 id）
        for id in ["settings:file", "settings:reload", "forget", "datadir"] {
            assert!(parse_id(id, |_| false).is_some());
        }
        // 番茄钟子菜单标题能被 label 改写
        assert!(MENU.iter().any(|n| matches!(n, Sub(_, k) if k.iter().any(|x| matches!(x, Sub(t, _) if *t == POMO_TITLE)))));
    }

    #[test]
    fn dynamic_state() {
        let mut c = Ctx { costume: "maid".into(), outfits: vec!["santa".into()], visible: true, pomo_left: None };
        assert!(checked("costume:maid", &c));
        assert!(!checked("costume:", &c));
        assert!(checked("outfit:santa", &c));
        assert!(!checked("outfit:glasses", &c));
        c.costume = "bogus".into();
        assert!(checked("costume:", &c));
        // 老配置的 "" = 默认水手服；卫衣要单独选
        c.costume = String::new();
        assert!(checked("costume:sailor", &c) && !checked("costume:hoodie", &c));
        c.costume = "hoodie".into();
        assert!(checked("costume:hoodie", &c) && !checked("costume:sailor", &c));
        assert_eq!(label("visible", &c).unwrap(), "🙈 先藏起来");
        assert!(enabled("pomo:25", &c) && !enabled("pomo:stop", &c));
        c.pomo_left = Some(12);
        c.visible = false;
        assert_eq!(label("visible", &c).unwrap(), "👀 叫她出来");
        assert_eq!(label("pomo:stop", &c).unwrap(), "结束番茄钟（还剩 12 分钟）");
        assert_eq!(label(POMO_TITLE, &c).unwrap(), "🍅 番茄钟（还剩 12 分钟）");
        assert!(!enabled("pomo:25", &c) && enabled("pomo:stop", &c));
        assert_eq!(label("chat", &c), None);
    }
}
