//! 新互动（不在原版里）：眼睛跟光标、滚轮撸猫、扔小鱼干、爬窗口、时间/天气反应、签到、逗猫棒小游戏。
//! 纯逻辑（WheelPet / Game / greeting_slot / is_feed_drop）单独放，方便单测；
//! 和状态机打交道的部分是 `impl Behavior`，由 mod.rs 里的几个钩子调用。
use super::{Behavior, Shot, St, StateAct, pick, rand};
use super::fx::FxKind;
use crate::core::stats::{Act, ActResult, MoodLevel};
use crate::render::anim;
use crate::types::{PerchKind, ToyCmd, ToyEv, ToyKind, Weather, WeatherKind, WinCmd};

// ---------------------------------------------------------------- 台词
pub const WHEEL: &[&str] = &["呼噜呼噜……撸得好舒服喵～", "从头顶撸到尾巴尖，嘿嘿", "再撸两下嘛，哈基米不走了", "呼噜噜……要化掉了喵"];
pub const WHEEL_PURR: &[&str] = &["呼噜……", "呼噜噜～", "咕噜咕噜……"];
pub const WHEEL_MAD: &[&str] = &["撸、撸太多了！毛都要起静电了喵！", "够啦够啦！哈基米要秃了！", "哈——！手法太粗暴了喵！"];
pub const FOOD_SEE: &[&str] = &["小鱼干！哈基米来啦——", "闻到香味了喵！", "是给哈基米的吗？！"];
pub const FOOD_GRAB: &[&str] = &["给我给我！喵喵喵！", "拿过来嘛～哈基米张嘴了", "放到哈基米嘴边就好啦～"];
pub const FOOD_DROP: &[&str] = &["掉地上啦，哈基米去捡～", "别跑！小鱼干是哈基米的！"];
pub const FOOD_EAT: &[&str] = &["嗷呜！抓到小鱼干了喵～", "咔嚓咔嚓……好香！", "罐头！今天是什么好日子喵！"];
pub const FOOD_BUSY: &[&str] = &["上一条还没吃到呢喵！", "先让哈基米把这个吃了嘛"];
pub const FOOD_GIVEUP: &[&str] = &["够不着……小鱼干跑掉了喵", "呜，小鱼干不见了"];
pub const PERCH_WIN: &[&str] = &["爬上来啦！这里视野好好喵～", "哈基米占领了这个窗口！", "坐在高处看你干活喵～"];
pub const PERCH_SIDE: &[&str] = &["探头——偷偷看你喵", "屏幕边边好凉快～", "（扒着屏幕边）在干嘛呀？"];
pub const PERCH_SIT: &[&str] = &["这个窗口好暖和喵", "腿晃来晃去～", "不许把哈基米坐的窗口关掉哦"];
pub const PERCH_DOWN: &[&str] = &["跳下去啦——喵！", "看哈基米的完美落地！"];
pub const PERCH_LOST: &[&str] = &["诶诶诶——窗户跑了喵！", "哇啊！谁把窗口拿走了！", "喵——！地板塌了！"];
pub const PERCH_NONE: &[&str] = &["这里没有能爬的窗口喵……", "找不到落脚的地方，哈基米爬不上去"];
pub const LAND_SAFE: &[&str] = &["安全着陆喵！", "十分！哈基米给自己打十分～"];
pub const MORNING: &[&str] = &["早上好喵！今天也要元气满满～", "早安！吃早饭了没有呀？", "太阳晒屁股啦，早上好喵～"];
pub const LUNCH: &[&str] = &["饭点到啦！先去吃午饭喵～", "中午了！干饭干饭，哈基米也要小鱼干", "肚子咕咕叫了吧？去吃午饭喵"];
pub const DINNER: &[&str] = &["晚饭时间到喵！吃点好的犒劳自己～", "天黑啦，记得吃晚饭喵"];
pub const LATE: &[&str] = &["都这么晚了，快去睡觉喵！熬夜会变熊猫眼的", "哈基米都困了……你也早点睡嘛", "深夜了喵，明天再弄吧，身体最重要"];
pub const WEATHER_RAIN: &[&str] = &["外面在下雨喵，出门记得带伞！", "下雨啦～哈基米最讨厌被淋湿了", "滴答滴答……带伞带伞喵！"];
pub const WEATHER_THUNDER: &[&str] = &["打、打雷了喵！哈基米要躲起来", "雷雨天，出门一定带伞喵！"];
pub const WEATHER_SNOW: &[&str] = &["下雪啦！哈基米想去踩雪喵～", "外面在下雪，穿暖和点喵！"];
pub const WEATHER_HOT: &[&str] = &["好热喵……哈基米要化成一摊了", "热热热！记得多喝水，别中暑喵"];
pub const WEATHER_COLD: &[&str] = &["降温了，多穿点衣服喵～"];
pub const WEATHER_NICE: &[&str] = &["今天天气真好喵，想出去晒太阳～", "好天气！适合摸鱼……啊不是，适合干活喵"];
pub const REPORT_WAIT: &[&str] = &["哈基米去窗边看看天气喵……", "等一下，哈基米看看外面～"];
pub const REPORT_FAIL: &[&str] = &["看不到天气……网络好像不通喵"];
pub const CHECKIN_DONE: &[&str] = &["今天已经签过到啦～", "签过啦签过啦，哈基米记着呢"];
pub const GAME_START: &[&str] = &["逗猫棒！哈基米要抓住它喵！", "来呀来呀～看哈基米的猫猫拳！"];
pub const GAME_CATCH: &[&str] = &["抓到啦！", "嘿！到手！", "喵呜！"];
pub const GAME_MISS: &[&str] = &["差一点喵！", "跑得好快！", "再来！"];
pub const GAME_BUSY: &[&str] = &["先把小鱼干吃了再玩喵～"];
pub const GOOD_GAZE: &[&str] = &["盯——（眼睛跟着你的鼠标转）", "你的鼠标在动哦喵", "（偷偷看你）"];

/// 解锁台词：亲密度等级下标 → 偶尔说（2 朋友 / 3 好朋友 / 4 形影不离 / 5 家人）
pub fn affinity_lines(level: usize) -> &'static [&'static str] {
    match level {
        0 | 1 => &[],
        2 => &["和你在一起，时间过得好快喵", "哈基米记住你的样子啦～"],
        3 => &["哈基米把最喜欢的小鱼干分你一半～", "只给你摸肚子哦，别人不行喵"],
        4 => &["你去哪哈基米就去哪！", "今天也想一直待在你旁边喵"],
        _ => &["我们是一家人喵，永远都是！", "有你在，哈基米什么都不怕～"],
    }
}

fn checkin_line(streak: u32, bonus: f64) -> String {
    let b = bonus as u32;
    match streak {
        1 => format!("签到成功！今天也来看哈基米啦，好感 +{b} 喵～"),
        3 => format!("连续签到 3 天！哈基米开始期待每天见到你了，好感 +{b}"),
        7 => format!("连续 7 天！一整个星期都在喵！好感 +{b}"),
        30 => format!("连续 30 天！！哈基米要给你颁个奖喵！好感 +{b}"),
        n if n % 100 == 0 => format!("连续 {n} 天……哈基米感动得要哭了喵！好感 +{b}"),
        n => format!("签到成功！连续 {n} 天喵～好感 +{b}"),
    }
}

fn game_end_line(score: u32, best: bool) -> String {
    let s = match score {
        0 => "逗猫棒太狡猾了喵……一次都没抓到".to_string(),
        1..=3 => format!("抓到 {score} 次！再来一局嘛喵～"),
        4..=7 => format!("抓到 {score} 次！哈基米的猫猫拳厉害吧～"),
        _ => format!("{score} 次！哈基米是逗猫棒冠军喵！"),
    };
    if best && score > 0 { format!("{s}（新纪录！）") } else { s }
}

// ---------------------------------------------------------------- 纯逻辑
/// 滚轮撸猫的结果
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelOut {
    None,
    /// 在冷却里：小呼噜，不加数值
    Purr,
    /// 撸到了：等同摸头
    Pet,
    /// 撸太多：炸毛
    Annoyed,
}

/// 滚轮撸猫：每滚 3 格算撸一次，撸一次后 3 秒冷却（期间只呼噜）；5 秒里滚了 30 格以上就炸毛，8 秒内不理
#[derive(Default, Debug)]
pub struct WheelPet {
    recent: Vec<(u64, u32)>,
    acc: u32,
    cool_until: u64,
    annoyed_until: u64,
}

impl WheelPet {
    pub const PET_NOTCHES: u32 = 3;
    pub const COOLDOWN: u64 = 3000;
    pub const WINDOW: u64 = 5000;
    pub const TOO_MUCH: u32 = 30;
    pub const SULK: u64 = 8000;

    pub fn on(&mut self, now: u64, delta: i32) -> WheelOut {
        if now < self.annoyed_until { return WheelOut::None; }
        // 精确滚轮可能一次只给几十：按 120 = 1 格折算，至少算 1
        let notches = (delta.unsigned_abs() / 120).max(1);
        self.recent.retain(|(t, _)| now.saturating_sub(*t) < Self::WINDOW);
        self.recent.push((now, notches));
        let total: u32 = self.recent.iter().map(|(_, n)| n).sum();
        if total >= Self::TOO_MUCH {
            self.recent.clear();
            self.acc = 0;
            self.annoyed_until = now + Self::SULK;
            return WheelOut::Annoyed;
        }
        self.acc += notches;
        if self.acc < Self::PET_NOTCHES { return WheelOut::None; }
        self.acc = 0;
        if now >= self.cool_until {
            self.cool_until = now + Self::COOLDOWN;
            WheelOut::Pet
        } else {
            WheelOut::Purr
        }
    }
}

/// 一天里的几个打招呼时段
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot { Morning, Lunch, Dinner, Late }

impl Slot {
    fn bit(self) -> u8 { 1 << self as u8 }
    fn lines(self) -> &'static [&'static str] {
        match self { Slot::Morning => MORNING, Slot::Lunch => LUNCH, Slot::Dinner => DINNER, Slot::Late => LATE }
    }
}

/// 当前时刻落在哪个时段（早上 6:30–9:59、午饭 11:30–12:59、晚饭 17:30–18:59、深夜 23:00–1:59）
pub fn greeting_slot(hour: u16, minute: u16) -> Option<Slot> {
    let m = hour as u32 * 60 + minute as u32;
    match m {
        390..=599 => Some(Slot::Morning),
        690..=779 => Some(Slot::Lunch),
        1050..=1139 => Some(Slot::Dinner),
        1380.. | 0..=119 => Some(Slot::Late),
        _ => None,
    }
}

/// 松手时小鱼干在不在她嘴边（dx/dy：道具中心相对嘴巴的逻辑像素）
pub fn is_feed_drop(dx: f32, dy: f32) -> bool {
    (dx * dx + dy * dy).sqrt() < 80.0 || (dx.abs() < 85.0 && dy > -60.0 && dy < 150.0)
}

/// 光标 → (身子错切像素, 歪头角度)。光标相对头部中心 (dx, dy)，400 像素外或就在头上时不看
pub fn gaze(dx: f32, dy: f32) -> (f32, f32) {
    let d = (dx * dx + dy * dy).sqrt();
    if !(40.0..=400.0).contains(&d) { return (0.0, 0.0); }
    // 越近歪得越明显，但有上限
    let k = (dx / 400.0).clamp(-1.0, 1.0);
    let lean = (k * 30.0).clamp(-14.0, 14.0);
    let tilt = (k * 9.0).clamp(-4.0, 4.0);
    (lean, tilt)
}

/// 逗猫棒小游戏要她做的事
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GameAct {
    None,
    Jump { caught: bool },
    Walk { dir: i32, dist: f32 },
}

/// 逗猫棒：光标就是毛球。毛球进到她头顶附近就扑一下，晃得慢容易被抓到
#[derive(Debug)]
pub struct Game {
    pub end: u64,
    pub score: u32,
    pub jumps: u32,
    last_jump: u64,
    last_walk: u64,
    last: Option<(f32, f32, u64)>,
    /// 毛球速度（逻辑像素/秒，平滑过）
    pub speed: f32,
}

impl Game {
    pub const DURATION: u64 = 25_000;
    pub const JUMP_GAP: u64 = 900;
    /// 她头顶中心（窗口逻辑坐标）
    pub const HEAD: (f32, f32) = (120.0, 100.0);

    pub fn new(now: u64) -> Self {
        Self { end: now + Self::DURATION, score: 0, jumps: 0, last_jump: 0, last_walk: 0, last: None, speed: 0.0 }
    }

    pub fn in_reach(x: f32, y: f32) -> bool {
        let dx = x - Self::HEAD.0;
        dx.abs() < 100.0 && y > -60.0 && y < 230.0
    }

    /// 毛球移动到 (x, y)（窗口逻辑坐标）；roll ∈ [0,1) 决定抓没抓到
    pub fn on_cursor(&mut self, now: u64, x: f32, y: f32, roll: f64) -> GameAct {
        if let Some((lx, ly, lt)) = self.last {
            let dt = now.saturating_sub(lt).max(1) as f32 / 1000.0;
            let v = ((x - lx).powi(2) + (y - ly).powi(2)).sqrt() / dt;
            self.speed = self.speed * 0.5 + v.min(5000.0) * 0.5;
        }
        self.last = Some((x, y, now));
        if now >= self.end { return GameAct::None; }
        if Self::in_reach(x, y) {
            if now.saturating_sub(self.last_jump) < Self::JUMP_GAP { return GameAct::None; }
            self.last_jump = now;
            self.jumps += 1;
            let chance = if self.speed < 300.0 { 0.9 } else if self.speed < 900.0 { 0.6 } else if self.speed < 1800.0 { 0.3 } else { 0.1 };
            let caught = roll < chance;
            if caught { self.score += 1; }
            return GameAct::Jump { caught };
        }
        let dx = x - Self::HEAD.0;
        if dx.abs() > 110.0 && dx.abs() < 900.0 && y > -300.0 && now.saturating_sub(self.last_walk) > 1200 {
            self.last_walk = now;
            return GameAct::Walk { dir: dx.signum() as i32, dist: (dx.abs() - 60.0).min(220.0) };
        }
        GameAct::None
    }

    /// 奖励：亲密度（最多 +8）和心情
    pub fn reward(&self) -> (f64, f64) { ((self.score.min(8)) as f64, 4.0 + self.score.min(10) as f64 * 2.0) }
}

// ---------------------------------------------------------------- 状态
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoodPhase { Falling, Held, Chasing, Ground }

#[derive(Debug)]
pub struct Food {
    pub kind: ToyKind,
    pub phase: FoodPhase,
    pub tries: u32,
    pub expire: u64,
}

pub struct Play {
    /// 最近一次光标位置（窗口逻辑坐标）和时间
    pub cursor: Option<(f32, f32, u64)>,
    cursor_moved: u64,
    lean_from: (f32, f32),
    lean_to: (f32, f32),
    lean_t: u64,
    poll_sent: u32,
    glance_until: Option<u64>,
    next_glance: u64,
    pub wheel: WheelPet,
    pub food: Option<Food>,
    pub perch: Option<PerchKind>,
    climbing: Option<u64>,
    climb_manual: bool,
    pub jumping_down: bool,
    pub weather: Option<Weather>,
    weather_at: u64,
    /// 等天气回来再播报：(是不是手动点的, 开始等的时间)
    report_pending: Option<(bool, u64)>,
    weather_said: Option<(WeatherKind, i32, u64)>,
    next_weather: u64,
    greeted: (String, u8),
    checkin_at: Option<u64>,
    pub game: Option<Game>,
    /// 测试用：固定「今天」和「现在几点」
    pub fake_today: Option<String>,
    pub fake_hm: Option<(u16, u16)>,
}

impl Default for Play {
    fn default() -> Self {
        Self {
            cursor: None,
            cursor_moved: 0,
            lean_from: (0.0, 0.0),
            lean_to: (0.0, 0.0),
            lean_t: 0,
            poll_sent: 0,
            glance_until: None,
            next_glance: 20_000,
            wheel: WheelPet::default(),
            food: None,
            perch: None,
            climbing: None,
            climb_manual: false,
            jumping_down: false,
            weather: None,
            weather_at: 0,
            report_pending: None,
            weather_said: None,
            next_weather: 15_000,
            greeted: (String::new(), 0),
            checkin_at: None,
            game: None,
            fake_today: None,
            fake_hm: None,
        }
    }
}

const LEAN_MS: u64 = 220;

impl Play {
    pub fn lean_at(&self, now: u64) -> (f32, f32) {
        let p = (now.saturating_sub(self.lean_t) as f32 / LEAN_MS as f32).min(1.0);
        let e = anim::ease(anim::Ease::EaseOut, p);
        (self.lean_from.0 + (self.lean_to.0 - self.lean_from.0) * e, self.lean_from.1 + (self.lean_to.1 - self.lean_from.1) * e)
    }
    pub fn lean_moving(&self, now: u64) -> bool { now < self.lean_t + LEAN_MS && self.lean_from != self.lean_to }
    fn set_lean(&mut self, to: (f32, f32), now: u64) {
        // 半个像素以内不动，免得光标抖一下就重画
        if (to.0 - self.lean_to.0).abs() < 0.5 && (to.1 - self.lean_to.1).abs() < 0.2 { return; }
        self.lean_from = self.lean_at(now);
        self.lean_to = to;
        self.lean_t = now;
    }

    /// 下一个要处理的到期时间
    pub fn next_due(&self) -> Option<u64> {
        [
            self.glance_until,
            self.checkin_at,
            self.game.as_ref().map(|g| g.end),
            self.food.as_ref().map(|f| f.expire),
            self.climbing,
            self.report_pending.map(|(_, t)| t + 30_000),
            Some(self.next_weather),
        ]
        .into_iter()
        .flatten()
        .min()
    }
}

fn today_string() -> String {
    #[cfg(windows)]
    {
        let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay)
    }
    #[cfg(not(windows))]
    {
        "2026-01-01".to_string()
    }
}

fn local_hm() -> (u16, u16) {
    #[cfg(windows)]
    {
        let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        (t.wHour, t.wMinute)
    }
    #[cfg(not(windows))]
    {
        (12, 0)
    }
}

// ---------------------------------------------------------------- 状态机钩子
impl Behavior {
    fn today(&self) -> String { self.play.fake_today.clone().unwrap_or_else(today_string) }
    fn hm(&self) -> (u16, u16) { self.play.fake_hm.unwrap_or_else(local_hm) }

    /// 能不能被小事打扰（打招呼/天气）
    fn free_for_chatter(&self) -> bool {
        !self.chatting && !self.waiting && self.play.game.is_none() && !self.memo_ringing()
            && matches!(self.state, St::Idle | St::Sit | St::Walk)
    }

    /// 能不能看光标
    fn gaze_ok(&self) -> bool {
        !self.dragging && matches!(self.state, St::Idle | St::Sit | St::Hungry | St::Stare | St::Happy | St::Shy | St::Love | St::Think)
    }

    /// 每次事件/帧之后：光标轮询开关、眼神跟随
    pub(super) fn sync_play(&mut self) {
        let now = self.now;
        let want = if self.play.game.is_some() {
            33
        } else if self.gaze_ok() {
            match self.play.cursor {
                Some((x, y, _)) if ((x - 120.0).powi(2) + (y - 150.0).powi(2)).sqrt() < 600.0 => 100,
                _ => 250,
            }
        } else {
            0
        };
        if want != self.play.poll_sent {
            self.play.poll_sent = want;
            self.cmds.push(WinCmd::CursorPoll(want));
        }
        let target = match self.play.cursor {
            Some((x, y, _)) if self.gaze_ok() && self.play.game.is_none() => gaze(x - 120.0, y - 150.0),
            _ => (0.0, 0.0),
        };
        self.play.set_lean(target, now);
    }

    pub(super) fn play_animating(&self) -> bool {
        self.play.lean_moving(self.now) || self.play.game.is_some()
    }

    /// 光标位置（平台轮询 or 鼠标在她身上移动）
    pub(super) fn on_cursor(&mut self, x: f32, y: f32) {
        let now = self.now;
        let moved = self.play.cursor.is_none_or(|(lx, ly, _)| (lx - x).abs() > 3.0 || (ly - y).abs() > 3.0);
        if moved { self.play.cursor_moved = now; }
        self.play.cursor = Some((x, y, now));
        if self.play.game.is_some() { return self.game_cursor(x, y); }
        // 偶尔偷看：光标停在附近 2.5 秒以上
        let near = ((x - 120.0).powi(2) + (y - 150.0).powi(2)).sqrt() < 260.0;
        if near
            && self.state == St::Idle
            && self.face == crate::types::Face::Idle
            && now.saturating_sub(self.play.cursor_moved) > 2500
            && now >= self.play.next_glance
            && self.play.glance_until.is_none()
        {
            self.show(crate::types::Face::Stare);
            self.play.glance_until = Some(now + 1600);
            self.play.next_glance = now + rand(15_000.0, 40_000.0) as u64;
            if fastrand::f64() < 0.3 && !self.bubble_fade.on {
                self.say_opts(&pick(GOOD_GAZE), 1800, false, crate::types::SpeakKind::Line);
            }
        }
    }

    pub(super) fn on_play_timer(&mut self) {
        let now = self.now;
        if self.play.glance_until.is_some_and(|t| t <= now) {
            self.play.glance_until = None;
            if self.face == crate::types::Face::Stare && self.state == St::Idle { self.show(crate::types::Face::Idle); }
        }
        if self.play.checkin_at.is_some_and(|t| t <= now) {
            self.play.checkin_at = None;
            self.do_checkin(false);
        }
        if self.play.game.as_ref().is_some_and(|g| g.end <= now) { self.end_game(); }
        if self.play.food.as_ref().is_some_and(|f| f.expire <= now) {
            let held = self.play.food.as_ref().is_some_and(|f| f.phase == FoodPhase::Held);
            if held {
                if let Some(f) = &mut self.play.food { f.expire = now + 30_000; }
            } else {
                self.play.food = None;
                self.cmds.push(WinCmd::Toy(ToyCmd::Remove));
                if self.state == St::Walk { self.set_state(St::Idle, Some((4000.0, 8000.0))); }
            }
        }
        if self.play.report_pending.is_some_and(|(_, t)| now.saturating_sub(t) >= 30_000) {
            // 等了 30 秒天气还没回来：手动点的说一声，自动的下次再试
            let (manual, _) = self.play.report_pending.take().unwrap_or((false, 0));
            if manual { self.say(&pick(REPORT_FAIL), 2400); }
        }
        if self.play.climbing.is_some_and(|t| t <= now) {
            // 平台一直没回话：当没爬成
            self.play.climbing = None;
        }
        if self.play.next_weather <= now {
            self.play.next_weather = now + 90 * 60_000;
            self.call("weather".into());
            if self.io { crate::services::weather::refresh(&self.tx); }
        }
    }

    // ------------------------------------------------------------ 滚轮
    pub(super) fn on_wheel(&mut self, delta: i32) {
        if self.chat_busy() || matches!(self.state, St::Drag | St::Fall) || self.play.game.is_some() { return; }
        self.yield_chat();
        match self.play.wheel.on(self.now, delta) {
            WheelOut::None => {}
            WheelOut::Purr => {
                self.fx(FxKind::Heart, 1);
                if self.bubble.is_none() || !self.bubble_fade.on {
                    self.say_opts(&pick(WHEEL_PURR), 1200, false, crate::types::SpeakKind::Line);
                }
            }
            WheelOut::Pet => {
                self.act(Act::Pat);
                if self.state == St::Sleep {
                    self.say(super::lines::PAT_SLEEPING, 2600);
                    self.fx(FxKind::Heart, 3);
                    return;
                }
                let st = if self.stats.mood() == MoodLevel::Great && fastrand::bool() { St::Love } else { St::Shy };
                self.react(st, 2200, Some(pick(WHEEL)));
                self.fx(FxKind::Heart, 4);
            }
            WheelOut::Annoyed => {
                self.act(Act::Angry);
                self.play(Shot::Shake);
                self.set_state(St::Hiss, None);
                self.say(&pick(WHEEL_MAD), 2600);
                self.fx(FxKind::Anger, 1);
                self.hold_timer(2200, StateAct::Set(St::Idle, Some((5000.0, 10000.0))));
            }
        }
    }

    // ------------------------------------------------------------ 喂食
    /// 菜单「喂小鱼干」：在她旁边扔一条，她会去追；也可以拖到她嘴边
    pub(super) fn toss_food(&mut self) {
        if self.play.game.is_some() { return; }
        self.yield_chat();
        if self.play.food.is_some() {
            self.say(&pick(FOOD_BUSY), 2000);
            return;
        }
        if self.state == St::Sleep {
            self.act(Act::Wake);
            self.say(&pick(super::lines::WAKE), 2000);
        }
        let kind = if fastrand::f64() < 0.25 { ToyKind::Can } else { ToyKind::Fish };
        let side = if fastrand::bool() { -1 } else { 1 };
        self.play.food = Some(Food { kind, phase: FoodPhase::Falling, tries: 0, expire: self.now + 60_000 });
        self.cmds.push(WinCmd::Toy(ToyCmd::Spawn { kind, side, offset: rand(150.0, 260.0) as f32 }));
        self.play(Shot::Bounce);
        self.set_state(St::Hungry, None);
        self.say(&pick(FOOD_SEE), 2200);
        self.fx(FxKind::Exclaim, 1);
    }

    fn chase_food(&mut self) {
        let Some(f) = &mut self.play.food else { return };
        f.phase = FoodPhase::Chasing;
        if self.play.perch == Some(PerchKind::WindowTop) {
            // 先跳下来，落地后再追
            if let Some(f) = &mut self.play.food { f.phase = FoodPhase::Ground; }
            return self.jump_down();
        }
        self.set_state(St::Walk, None);
        self.cmds.push(WinCmd::Toy(ToyCmd::Chase { speed: 3.2 * self.scale }));
    }

    fn eat_food(&mut self) {
        let Some(f) = self.play.food.take() else { return };
        self.cmds.push(WinCmd::Toy(ToyCmd::Remove));
        if self.state == St::Walk { self.set_state(St::Idle, None); }
        let was_starving = self.stats.data.hunger < 15.0;
        if self.act(Act::Feed) == ActResult::Full {
            self.play(Shot::Shake);
            self.say(&pick(super::lines::FULL), 2600);
            return self.set_state(St::Idle, Some((4000.0, 8000.0)));
        }
        if was_starving {
            self.react(St::Love, 2800, Some(pick(super::lines::FEED_STARVING)));
            self.fx(FxKind::Heart, 6);
        } else {
            let line = if f.kind == ToyKind::Can { FOOD_EAT[2].to_string() } else { pick(&FOOD_EAT[..2]) };
            self.react(St::Eat, 2600, Some(line));
            self.fx(FxKind::Heart, 3);
        }
    }

    pub(super) fn on_toy(&mut self, ev: ToyEv) {
        if self.play.game.is_some() { return; }
        if self.play.food.is_none() {
            // 道具已经不归我们管了（吃掉/超时），收掉
            if matches!(ev, ToyEv::Landed { .. } | ToyEv::Dropped { .. }) { self.cmds.push(WinCmd::Toy(ToyCmd::Remove)); }
            return;
        }
        match ev {
            ToyEv::Grabbed => {
                if let Some(f) = &mut self.play.food { f.phase = FoodPhase::Held; f.expire = self.now + 60_000; }
                if matches!(self.state, St::Drag | St::Fall) { return; }
                if self.state == St::Walk { self.set_state(St::Hungry, None); }
                if self.state != St::Hungry { self.set_state(St::Hungry, None); }
                self.play(Shot::Bounce);
                self.say(&pick(FOOD_GRAB), 2200);
            }
            ToyEv::Dropped { dx, dy } => {
                if is_feed_drop(dx, dy) && !matches!(self.state, St::Drag | St::Fall) && self.play.perch != Some(PerchKind::WindowTop) {
                    self.eat_food();
                } else {
                    if let Some(f) = &mut self.play.food { f.phase = FoodPhase::Falling; }
                    self.cmds.push(WinCmd::Toy(ToyCmd::Fall));
                    if !matches!(self.state, St::Drag | St::Fall) { self.say(&pick(FOOD_DROP), 1800); }
                }
            }
            ToyEv::Landed { .. } => {
                if let Some(f) = &mut self.play.food { f.phase = FoodPhase::Ground; }
                if matches!(self.state, St::Drag | St::Fall) || self.chatting { return; }
                self.chase_food();
            }
            ToyEv::Reached { ok } => {
                if ok { return self.eat_food(); }
                let tries = self.play.food.as_mut().map(|f| { f.tries += 1; f.tries }).unwrap_or(9);
                if tries < 3 {
                    self.chase_food();
                } else {
                    self.play.food = None;
                    self.cmds.push(WinCmd::Toy(ToyCmd::Remove));
                    self.react(St::Cry, 2200, Some(pick(FOOD_GIVEUP)));
                }
            }
        }
    }

    // ------------------------------------------------------------ 爬窗口
    /// think() 开头调用；返回 true = 已经决定了下一步
    pub(super) fn think_play(&mut self) -> bool {
        if self.play.game.is_some() { return true; }
        if let Some(f) = &self.play.food {
            if f.phase == FoodPhase::Ground || f.phase == FoodPhase::Chasing {
                self.chase_food();
            }
            // 小鱼干下落中或被主人捏在手里时也别走开/睡着
            return true;
        }
        let r = fastrand::f64();
        match self.play.perch {
            Some(PerchKind::WindowTop) => {
                if r < 0.3 {
                    self.jump_down();
                } else {
                    if fastrand::f64() < 0.3 { self.say(&pick(PERCH_SIT), 2400); }
                    self.set_state(St::Sit, Some((12000.0, 25000.0)));
                }
                true
            }
            Some(PerchKind::ScreenSide) => {
                self.play.perch = None;
                false
            }
            None => {
                if self.state != St::Idle || self.play.climbing.is_some() || self.play.food.is_some() { return false; }
                let kind = if r < 0.10 { PerchKind::WindowTop } else if r < 0.14 { PerchKind::ScreenSide } else { return false };
                self.start_climb(kind, false);
                true
            }
        }
    }

    pub(super) fn start_climb(&mut self, kind: PerchKind, manual: bool) {
        if matches!(self.state, St::Drag | St::Fall) || self.play.game.is_some() { return; }
        if self.state == St::Walk { self.set_state(St::Idle, None); }
        self.play.climbing = Some(self.now + 5000);
        self.play.climb_manual = manual;
        self.cmds.push(WinCmd::Climb(kind));
        self.play(Shot::Bounce);
        self.show(crate::types::Face::Happy);
        // 平台没回话的兜底：5 秒后照常思考
        self.hold_timer(5000, StateAct::Think);
    }

    pub(super) fn jump_down(&mut self) {
        self.cancel_routine();
        self.set_flip(false);
        self.sleep_since = None;
        self.play.perch = None;
        self.play.jumping_down = true;
        self.cmds.push(WinCmd::Unperch);
        self.state = St::Fall;
        self.state_timer = None;
        self.show(crate::types::Face::Happy);
        self.play(Shot::Bounce);
        self.say(&pick(PERCH_DOWN), 1800);
    }

    pub(super) fn on_perched(&mut self, kind: PerchKind) {
        self.play.climbing = None;
        self.play.perch = Some(kind);
        self.play(Shot::Squash);
        match kind {
            PerchKind::WindowTop => {
                self.set_state(St::Sit, Some((12000.0, 25000.0)));
                self.say(&pick(PERCH_WIN), 2400);
            }
            PerchKind::ScreenSide => {
                self.set_state(St::Stare, Some((6000.0, 12000.0)));
                self.say(&pick(PERCH_SIDE), 2400);
            }
        }
    }

    pub(super) fn on_climb_failed(&mut self) {
        self.play.climbing = None;
        if std::mem::take(&mut self.play.climb_manual) {
            self.play(Shot::Shake);
            self.say(&pick(PERCH_NONE), 2400);
        }
        self.set_state(St::Idle, Some((4000.0, 9000.0)));
    }

    pub(super) fn on_perch_lost(&mut self) {
        self.cancel_routine();
        self.set_flip(false);
        self.sleep_since = None;
        self.play.perch = None;
        self.play.jumping_down = false;
        self.dragging_cls = false;
        self.land_shaken = false;
        self.state = St::Fall;
        self.state_timer = None;
        self.show(crate::types::Face::Surprised);
        self.play(Shot::Shake);
        self.fx(FxKind::Exclaim, 1);
        self.say(&pick(PERCH_LOST), 2400);
    }

    /// 落地时：主动跳下来的不喊疼。返回 true = 已处理
    pub(super) fn landed_play(&mut self) -> bool {
        self.play.perch = None;
        if !std::mem::take(&mut self.play.jumping_down) { return false; }
        self.play(Shot::Squash);
        if fastrand::f64() < 0.5 { self.say(&pick(LAND_SAFE), 1800); }
        self.set_state(St::Idle, Some((3000.0, 6000.0)));
        if self.play.food.as_ref().is_some_and(|f| f.phase == FoodPhase::Ground) { self.chase_food(); }
        true
    }

    /// 开始被拖：从窗口上被拎下来了
    pub(super) fn drag_started_play(&mut self) {
        self.play.perch = None;
        self.play.jumping_down = false;
        if let Some(f) = &mut self.play.food && f.phase == FoodPhase::Chasing { f.phase = FoodPhase::Ground; }
    }

    // ------------------------------------------------------------ 时间 & 天气
    /// 每 20 秒（on_tick）：到了饭点/早上/深夜就说一句（每个时段每天一次）
    pub(super) fn tick_play(&mut self) {
        if !self.io && self.play.fake_hm.is_none() { return; }
        let (h, m) = self.hm();
        // 每天 7 点后第一次活动：早间天气播报（存档记日期，一天一次）
        if h >= 7 && self.play.report_pending.is_none() && self.free_for_chatter() && self.stats.data.weather_report_last != self.today() {
            return self.weather_report(false);
        }
        let Some(slot) = greeting_slot(h, m) else { return };
        let today = self.today();
        if self.play.greeted.0 != today { self.play.greeted = (today, 0); }
        if self.play.greeted.1 & slot.bit() != 0 || !self.free_for_chatter() { return; }
        self.play.greeted.1 |= slot.bit();
        self.time_greet(slot);
    }

    /// 启动打招呼时如果正好在某个时段，用时段台词，并记下今天这个时段已经说过
    pub(super) fn greet_slot_line(&mut self) -> Option<String> {
        if !self.io && self.play.fake_hm.is_none() { return None; }
        let (h, m) = self.hm();
        let slot = greeting_slot(h, m)?;
        let today = self.today();
        if self.play.greeted.0 != today { self.play.greeted = (today, 0); }
        self.play.greeted.1 |= slot.bit();
        Some(pick(slot.lines()))
    }

    fn time_greet(&mut self, slot: Slot) {
        let line = pick(slot.lines());
        match slot {
            Slot::Late => {
                self.say(&line, 3000);
                self.react(St::Yawn, 2200, None);
            }
            Slot::Lunch | Slot::Dinner => {
                self.play(Shot::Bounce);
                self.react(St::Hungry, 2600, Some(line));
            }
            Slot::Morning => self.react(St::Happy, 2200, Some(line)),
        }
    }

    /// 天气播报：有新鲜天气就直接说，没有就去拉，回来再说
    pub(super) fn weather_report(&mut self, manual: bool) {
        let fresh = self.play.weather.is_some() && self.now.saturating_sub(self.play.weather_at) < 3_600_000;
        // 后台缓存里可能有更新的（别的地方刚拉过）
        let w = if fresh { self.play.weather.clone() } else if self.io { crate::services::weather::latest() } else { None };
        let Some(w) = w else {
            if self.play.report_pending.is_none() {
                self.play.report_pending = Some((manual, self.now));
                self.call("weather".into());
                if self.io { crate::services::weather::refresh(&self.tx); }
            }
            if manual { self.react(St::Think, 2000, Some(pick(REPORT_WAIT))); }
            return;
        };
        if !manual {
            self.stats.data.weather_report_last = self.today();
            self.save();
        }
        let line = crate::services::weather::report_line(&w);
        let ms = (3000 + line.encode_utf16().count() as u64 * 120).min(12_000);
        self.play(Shot::Bounce);
        self.set_state(St::Happy, None);
        self.say(&line, ms);
        self.hold_timer(ms.min(6000), StateAct::Set(St::Idle, Some((5000.0, 10000.0))));
        match w.kind {
            WeatherKind::Rain | WeatherKind::Thunder => self.fx(FxKind::Rain, 8),
            WeatherKind::Snow => self.fx(FxKind::Snow, 8),
            WeatherKind::Clear => self.fx(FxKind::Spark, 6),
            _ => {}
        }
    }

    pub(super) fn on_weather(&mut self, w: Weather) {
        let now = self.now;
        self.play.weather_at = now;
        if let Some((manual, _)) = self.play.report_pending.take() {
            self.play.weather = Some(w);
            if manual || self.free_for_chatter() { return self.weather_report(manual); }
            return;
        }
        let hot = w.temp_c >= 30.0;
        let cold = w.temp_c <= 5.0;
        let bucket = if hot { 1 } else if cold { -1 } else { 0 };
        self.play.weather = Some(w.clone());
        // 同一种天气 3 小时内不重复念叨
        let notable = matches!(w.kind, WeatherKind::Rain | WeatherKind::Thunder | WeatherKind::Snow) || hot || cold
            || (w.kind == WeatherKind::Clear && (18.0..=26.0).contains(&w.temp_c));
        let repeat = self.play.weather_said.is_some_and(|(k, b, t)| k == w.kind && b == bucket && now.saturating_sub(t) < 3 * 3_600_000)
            || (self.play.weather_said.is_some() && w.kind == WeatherKind::Clear && !hot && !cold);
        if !notable || repeat || !self.free_for_chatter() { return; }
        self.play.weather_said = Some((w.kind, bucket, now));
        match w.kind {
            WeatherKind::Rain => { self.react(St::Stare, 2600, Some(pick(WEATHER_RAIN))); self.fx(FxKind::Rain, 8); }
            WeatherKind::Thunder => { self.play(Shot::Shake); self.react(St::Surprised, 2600, Some(pick(WEATHER_THUNDER))); self.fx(FxKind::Rain, 8); }
            WeatherKind::Snow => { self.react(St::Happy, 2600, Some(pick(WEATHER_SNOW))); self.fx(FxKind::Snow, 8); }
            _ if hot => { self.react(St::Dizzy, 2600, Some(pick(WEATHER_HOT))); self.fx(FxKind::Sweat, 1); }
            _ if cold => { self.react(St::Shy, 2600, Some(pick(WEATHER_COLD))); }
            _ => { self.react(St::Happy, 2200, Some(pick(WEATHER_NICE))); self.fx(FxKind::Spark, 6); }
        }
    }

    pub(super) fn weather_text(&self) -> Option<String> {
        let w = self.play.weather.as_ref()?;
        let desc = if w.desc.is_empty() {
            match w.kind {
                WeatherKind::Clear => "晴", WeatherKind::Cloudy => "多云", WeatherKind::Fog => "雾",
                WeatherKind::Rain => "雨", WeatherKind::Snow => "雪", WeatherKind::Thunder => "雷雨",
            }.to_string()
        } else {
            w.desc.chars().take(8).collect()
        };
        Some(format!("{desc} {}°C", w.temp_c.round() as i32))
    }

    // ------------------------------------------------------------ 签到 & 好感
    /// act() 里调用：今天第一次互动 → 稍后签到（等这次的反应说完）
    pub(super) fn maybe_checkin(&mut self, kind: Act) {
        if !matches!(kind, Act::Feed | Act::Pat | Act::Poke | Act::Chat) || self.play.checkin_at.is_some() { return; }
        // 单元测试不碰真实日期（除非测试自己给了「今天」）
        if !self.io && self.play.fake_today.is_none() { return; }
        if self.stats.checked_in(&self.today()) { return; }
        self.play.checkin_at = Some(self.now + 2900);
    }

    pub(super) fn do_checkin(&mut self, manual: bool) {
        // 自动签到别打断正在追小鱼干/玩游戏/被拎着：过几秒再来
        let busy = self.play.game.is_some()
            || self.play.food.is_some()
            || self.chatting
            || matches!(self.state, St::Walk | St::Drag | St::Fall | St::Eat);
        if !manual && busy {
            self.play.checkin_at = Some(self.now + 4000);
            return;
        }
        let today = self.today();
        let before = self.stats.level().0;
        let Some(c) = self.stats.checkin(&today) else {
            if manual {
                let n = self.stats.data.checkin_streak;
                self.say(&format!("{}连续 {n} 天喵～", pick(CHECKIN_DONE)), 2600);
                self.show_status();
            }
            return;
        };
        self.save();
        if self.chatting || self.waiting {
            self.fx(FxKind::Spark, 6);
        } else {
            self.react(St::Love, 2600, Some(checkin_line(c.streak, c.bonus)));
            self.fx(FxKind::Spark, 8);
            self.fx(FxKind::Heart, 3);
        }
        let (idx, name) = self.stats.level();
        if idx > before { self.later(2900, super::Later::LevelUp(name)); }
        if manual { self.show_status(); }
    }

    /// 戳她时偶尔说的解锁台词（亲密度够了才有）
    pub(super) fn affinity_line(&self) -> Option<String> {
        let l = affinity_lines(self.stats.level().0);
        (!l.is_empty() && fastrand::f64() < 0.25).then(|| pick(l))
    }

    // ------------------------------------------------------------ 逗猫棒
    pub(super) fn start_game(&mut self) {
        if self.play.game.is_some() || self.chat_busy() || self.waiting { return; }
        self.yield_chat();
        if self.play.food.is_some() {
            self.say(&pick(GAME_BUSY), 2200);
            return;
        }
        if self.state == St::Sleep { self.act(Act::Wake); }
        if self.play.perch == Some(PerchKind::WindowTop) { self.jump_down(); }
        self.play.game = Some(Game::new(self.now));
        self.cmds.push(WinCmd::Toy(ToyCmd::Spawn { kind: ToyKind::Teaser, side: 1, offset: 0.0 }));
        if self.state != St::Fall { self.set_state(St::Happy, None); }
        self.play(Shot::Bounce);
        self.say(&pick(GAME_START), 2400);
        self.sync_play();
    }

    fn game_cursor(&mut self, x: f32, y: f32) {
        if matches!(self.state, St::Drag | St::Fall) { return; }
        let roll = fastrand::f64();
        let Some(g) = &mut self.play.game else { return };
        match g.on_cursor(self.now, x, y, roll) {
            GameAct::None => {}
            GameAct::Jump { caught } => {
                if self.state == St::Walk { self.set_state(St::Happy, None); }
                self.play(Shot::Jump);
                if caught {
                    self.set_state(St::Happy, None);
                    self.fx(FxKind::Heart, 2);
                    self.say_bubble(&pick(GAME_CATCH), 900);
                } else {
                    self.set_state(St::Surprised, None);
                    if fastrand::f64() < 0.5 { self.say_bubble(&pick(GAME_MISS), 900); }
                }
            }
            GameAct::Walk { dir, dist } => {
                if self.play.perch.is_some() || self.state == St::Walk { return; }
                self.walk_dir = dir;
                self.set_state(St::Walk, None);
                self.cmds.push(WinCmd::Walk { dir, distance: dist * self.scale, speed: 4.0 * self.scale });
            }
        }
    }

    fn end_game(&mut self) {
        let Some(g) = self.play.game.take() else { return };
        self.cmds.push(WinCmd::Toy(ToyCmd::Remove));
        let best = g.score > self.stats.data.game_best;
        if best { self.stats.data.game_best = g.score; }
        let before = self.stats.level().0;
        let (aff, mood) = g.reward();
        self.stats.reward(aff, mood);
        self.save();
        let (idx, name) = self.stats.level();
        if idx > before { self.later(3200, super::Later::LevelUp(name)); }
        if matches!(self.state, St::Drag | St::Fall) { return; }
        let st = if g.score == 0 { St::Cry } else if g.score >= 4 { St::Love } else { St::Happy };
        self.react(st, 3000, Some(game_end_line(g.score, best)));
        if g.score > 0 { self.fx(FxKind::Heart, g.score.min(6) as usize); }
        self.sync_play();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_pets_with_cooldown() {
        let mut w = WheelPet::default();
        assert_eq!(w.on(0, 120), WheelOut::None);
        assert_eq!(w.on(100, -120), WheelOut::None);
        assert_eq!(w.on(200, 120), WheelOut::Pet);
        // 冷却中只呼噜
        for t in [300, 400] { assert_eq!(w.on(t, 120), WheelOut::None); }
        assert_eq!(w.on(500, 120), WheelOut::Purr);
        // 冷却过了又能撸
        for t in [6000, 6100] { assert_eq!(w.on(t, 120), WheelOut::None); }
        assert_eq!(w.on(6200, 120), WheelOut::Pet);
        // 精确滚轮的小 delta 也算一格
        let mut w = WheelPet::default();
        for t in 0..2 { w.on(t * 10, 30); }
        assert_eq!(w.on(30, 30), WheelOut::Pet);
    }

    #[test]
    fn wheel_too_much_annoys_then_sulks() {
        let mut w = WheelPet::default();
        let mut outs = Vec::new();
        for i in 0..30 { outs.push(w.on(i * 100, 120)); }
        assert_eq!(*outs.last().unwrap(), WheelOut::Annoyed);
        assert_eq!(outs.iter().filter(|o| **o == WheelOut::Annoyed).count(), 1);
        // 生气 8 秒内不理
        assert_eq!(w.on(3000, 120 * 5), WheelOut::None);
        assert_eq!(w.on(2900 + WheelPet::SULK + 200, 360), WheelOut::Pet);
    }

    #[test]
    fn slots() {
        assert_eq!(greeting_slot(7, 0), Some(Slot::Morning));
        assert_eq!(greeting_slot(6, 29), None);
        assert_eq!(greeting_slot(12, 0), Some(Slot::Lunch));
        assert_eq!(greeting_slot(13, 0), None);
        assert_eq!(greeting_slot(18, 30), Some(Slot::Dinner));
        assert_eq!(greeting_slot(23, 30), Some(Slot::Late));
        assert_eq!(greeting_slot(1, 59), Some(Slot::Late));
        assert_eq!(greeting_slot(2, 0), None);
    }

    #[test]
    fn feed_drop_zone() {
        assert!(is_feed_drop(0.0, 0.0));
        assert!(is_feed_drop(40.0, 60.0));
        assert!(is_feed_drop(-70.0, 120.0));
        assert!(!is_feed_drop(200.0, 0.0));
        assert!(!is_feed_drop(0.0, -150.0));
    }

    #[test]
    fn gaze_limits() {
        assert_eq!(gaze(500.0, 0.0), (0.0, 0.0));
        assert_eq!(gaze(10.0, 10.0), (0.0, 0.0));
        let (l, t) = gaze(200.0, -50.0);
        assert!(l > 0.0 && l <= 14.0 && t > 0.0 && t <= 4.0);
        let (l, _) = gaze(-390.0, 0.0);
        assert_eq!(l, -14.0);
    }

    #[test]
    fn game_jumps_and_scores() {
        let mut g = Game::new(0);
        // 远处：走过去
        assert!(matches!(g.on_cursor(1500, 500.0, 100.0, 0.0), GameAct::Walk { dir: 1, .. }));
        // 慢慢靠近头顶：扑，roll 小 → 抓到
        g.speed = 0.0;
        g.last = None;
        assert_eq!(g.on_cursor(2000, 130.0, 50.0, 0.1), GameAct::Jump { caught: true });
        // 间隔太短不扑
        assert_eq!(g.on_cursor(2100, 131.0, 50.0, 0.1), GameAct::None);
        // 晃得飞快：大概率扑空
        assert_eq!(g.on_cursor(3000, 60.0, 20.0, 0.1), GameAct::Jump { caught: true });
        g.on_cursor(3050, 200.0, 20.0, 0.9);
        let r = g.on_cursor(4000, 40.0, 20.0, 0.7);
        assert_eq!(r, GameAct::Jump { caught: false });
        assert_eq!(g.score, 2);
        assert_eq!(g.jumps, 3);
        // 结束后不再动
        assert_eq!(g.on_cursor(Game::DURATION + 1, 120.0, 50.0, 0.0), GameAct::None);
        assert_eq!(g.reward().0, 2.0);
    }

    // ---- 状态机里的整套流程（io=false）
    use crate::core::stats::Stats;
    use crate::types::{AppEvent, Button, EventSender, MenuAction, MouseEv};

    fn mkb() -> Behavior {
        fastrand::seed(3);
        let (s, _) = Stats::load(None);
        Behavior::with(EventSender::new(|_| {}), s, false, 0.0, String::new(), Vec::new(), false)
    }
    fn ev(b: &mut Behavior, e: AppEvent, now: u64) -> Vec<WinCmd> {
        let mut out = Vec::new();
        b.event_at(e, now, &mut out);
        out
    }
    fn frame(b: &mut Behavior, now: u64) -> Vec<WinCmd> {
        let mut out = Vec::new();
        b.frame_at(now, &mut out);
        out
    }

    #[test]
    fn toss_chase_and_eat_food() {
        let mut b = mkb();
        b.stats.data.hunger = 40.0;
        frame(&mut b, 3000);
        let out = ev(&mut b, AppEvent::Menu(MenuAction::Feed), 3000);
        assert!(out.iter().any(|c| matches!(c, WinCmd::Toy(ToyCmd::Spawn { .. }))));
        assert_eq!(b.state, St::Hungry);
        let out = ev(&mut b, AppEvent::Toy(ToyEv::Landed { dx: 200.0 }), 3600);
        assert!(out.iter().any(|c| matches!(c, WinCmd::Toy(ToyCmd::Chase { .. }))));
        assert_eq!(b.state, St::Walk);
        let out = ev(&mut b, AppEvent::Toy(ToyEv::Reached { ok: true }), 5000);
        assert!(out.contains(&WinCmd::Toy(ToyCmd::Remove)));
        assert_eq!(b.state, St::Eat);
        assert_eq!(b.stats.data.total_feeds, 1);
        // 拖到嘴边直接吃
        ev(&mut b, AppEvent::Menu(MenuAction::Feed), 9000);
        ev(&mut b, AppEvent::Toy(ToyEv::Grabbed), 9100);
        let out = ev(&mut b, AppEvent::Toy(ToyEv::Dropped { dx: 10.0, dy: 10.0 }), 9500);
        assert!(out.contains(&WinCmd::Toy(ToyCmd::Remove)));
        assert_eq!(b.stats.data.total_feeds, 2);
        // 老的直接喂还在
        ev(&mut b, AppEvent::Menu(MenuAction::FeedNow), 15000);
        assert_eq!(b.stats.data.total_feeds, 3);
    }

    #[test]
    fn wheel_petting_raises_mood() {
        let mut b = mkb();
        b.stats.data.mood = 50.0;
        frame(&mut b, 3000);
        for i in 0..3 { ev(&mut b, AppEvent::Mouse(MouseEv::Wheel { delta: 120, x: 120.0, y: 200.0 }), 3000 + i * 50); }
        assert!(matches!(b.state, St::Shy | St::Love));
        assert!(b.stats.data.mood > 50.0);
    }

    #[test]
    fn perch_and_fall() {
        let mut b = mkb();
        frame(&mut b, 3000);
        let out = ev(&mut b, AppEvent::Menu(MenuAction::Climb), 3000);
        assert!(out.contains(&WinCmd::Climb(PerchKind::WindowTop)));
        ev(&mut b, AppEvent::Perched(PerchKind::WindowTop), 3800);
        assert_eq!(b.state, St::Sit);
        // 坐在窗口上被叫去溜达：先跳下来
        let out = ev(&mut b, AppEvent::Menu(MenuAction::Walk), 4000);
        assert!(out.contains(&WinCmd::Unperch));
        ev(&mut b, AppEvent::Landed { big: true }, 4800);
        assert_ne!(b.state, St::Fall);
        // 窗口被关：掉下去
        ev(&mut b, AppEvent::Perched(PerchKind::WindowTop), 9000);
        ev(&mut b, AppEvent::PerchLost, 9500);
        assert_eq!(b.state, St::Fall);
        ev(&mut b, AppEvent::Landed { big: false }, 10000);
        assert_eq!(b.state, St::Idle);
        // 没窗口可爬
        ev(&mut b, AppEvent::Menu(MenuAction::Climb), 20000);
        ev(&mut b, AppEvent::ClimbFailed, 20010);
        assert_eq!(b.state, St::Idle);
    }

    #[test]
    fn weather_report_waits_for_data() {
        let mut b = mkb();
        frame(&mut b, 3000);
        b.calls.clear();
        ev(&mut b, AppEvent::Menu(MenuAction::WeatherReport), 3000);
        assert!(b.calls.iter().any(|c| c == "weather"));
        let w = Weather { kind: WeatherKind::Rain, temp_c: 12.0, desc: "小雨".into(), city: String::new(), high_c: Some(15.0), low_c: Some(9.0), rain_chance: Some(80) };
        ev(&mut b, AppEvent::Weather(w), 4000);
        let text = b.scene.bubble.as_ref().map(|x| x.text.clone()).unwrap_or_default();
        assert!(text.contains("12°C") && text.contains("80%"), "{text}");
        // 有新鲜数据：直接播报，不再请求
        frame(&mut b, 20000); // 定时刷新（第 15 秒那次）先跑掉
        b.calls.clear();
        ev(&mut b, AppEvent::Menu(MenuAction::WeatherReport), 20000);
        assert!(!b.calls.iter().any(|c| c == "weather"));
    }

    #[test]
    fn first_interaction_checks_in() {
        let mut b = mkb();
        b.play.fake_today = Some("2026-11-20".into());
        frame(&mut b, 3000);
        ev(&mut b, AppEvent::Mouse(MouseEv::Down { button: Button::Left, x: 120.0, y: 250.0, sx: 0, sy: 0 }), 3000);
        ev(&mut b, AppEvent::Mouse(MouseEv::Up { button: Button::Left, x: 120.0, y: 250.0, sx: 0, sy: 0 }), 3010);
        frame(&mut b, 6500);
        assert_eq!(b.stats.data.checkin_streak, 1);
        assert_eq!(b.stats.data.checkin_last, "2026-11-20");
    }

    #[test]
    fn teaser_game_runs_and_ends() {
        let mut b = mkb();
        frame(&mut b, 3000);
        let out = ev(&mut b, AppEvent::Menu(MenuAction::Game), 3000);
        assert!(out.contains(&WinCmd::Toy(ToyCmd::Spawn { kind: ToyKind::Teaser, side: 1, offset: 0.0 })));
        assert!(out.contains(&WinCmd::CursorPoll(33)));
        ev(&mut b, AppEvent::Cursor { x: 120.0, y: 60.0 }, 4000);
        assert!(b.shot.is_some_and(|(s, _)| s == Shot::Jump));
        let out = frame(&mut b, 3000 + Game::DURATION + 10);
        assert!(out.contains(&WinCmd::Toy(ToyCmd::Remove)));
        assert!(b.play.game.is_none());
    }

    #[test]
    fn teaser_catch_does_not_cut_opening_line() {
        let mut b = mkb();
        frame(&mut b, 3000);
        ev(&mut b, AppEvent::Menu(MenuAction::Game), 3000);
        let tok = b.voice_req;
        assert!(tok != 0);
        ev(&mut b, AppEvent::Speech { token: tok, audio: Some(crate::types::Audio(vec![1, 2, 3])) }, 3100);
        assert_eq!(b.voice_playing, Some(tok));
        b.calls.clear();
        // 开场白还在念，毛球就进了扑击范围：扑一下、换气泡，但不能把开场白掐掉
        ev(&mut b, AppEvent::Cursor { x: 120.0, y: 60.0 }, 3500);
        assert!(b.shot.is_some_and(|(s, _)| s == Shot::Jump));
        assert!(!b.calls.iter().any(|c| c == "stop"), "{:?}", b.calls);
        assert_eq!(b.voice_playing, Some(tok));
    }
}
