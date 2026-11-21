//! 养成数值：逐行移植 Electron 版 src/core/stats.js（数值、速率、等级表完全一致）
//! 饱食 hunger、心情 mood、精力 energy：0–100；亲密度 affection：0–∞
//! 公开方法签名是约定，behavior 层直接调用；`*_at(now)` 版本供测试和需要固定时间的调用方使用。
#![allow(dead_code)]
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;

fn clamp(v: f64) -> f64 { 0f64.max(100f64.min(v)) }

/// 每分钟的变化量（stats.js RATE）
pub mod rate {
    /// 醒着约 6.7 小时从满到空
    pub const HUNGER_AWAKE: f64 = -0.25;
    pub const HUNGER_SLEEP: f64 = -0.1;
    /// 醒着约 5.5 小时耗光
    pub const ENERGY_AWAKE: f64 = -0.3;
    /// 睡 40 分钟回满
    pub const ENERGY_SLEEP: f64 = 2.5;
    pub const MOOD_DECAY: f64 = -0.15;
    /// 饿的时候心情掉得更快
    pub const MOOD_HUNGRY_EXTRA: f64 = -0.3;
    pub const MOOD_TIRED_EXTRA: f64 = -0.15;
}
/// 关机期间最多补算 12 小时
pub const OFFLINE_CAP_MIN: f64 = 12.0 * 60.0;

/// 亲密度等级表
pub const LEVELS: [(f64, &str); 6] =
    [(0.0, "初次见面"), (10.0, "眼熟"), (30.0, "朋友"), (80.0, "好朋友"), (200.0, "形影不离"), (500.0, "家人")];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StatsData {
    pub hunger: f64,
    pub mood: f64,
    pub energy: f64,
    pub affection: f64,
    /// 毫秒时间戳
    pub last_tick: f64,
    pub total_feeds: u32,
    pub first_met: f64,
    /// 上次签到的日期 "YYYY-MM-DD"（当地时间），"" = 没签过
    pub checkin_last: String,
    /// 连续签到天数
    pub checkin_streak: u32,
    /// 累计签到天数
    pub checkin_total: u32,
    /// 逗猫棒小游戏最高分
    pub game_best: u32,
    /// 上次早间天气播报的日期 "YYYY-MM-DD"
    pub weather_report_last: String,
    /// 存档里的其它字段原样保留（原版 `{...DEFAULT, ...saved}` 会带着它们写回）
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
impl Default for StatsData {
    fn default() -> Self {
        let now = now_ms();
        Self {
            hunger: 80.0, mood: 80.0, energy: 80.0, affection: 0.0, last_tick: now, total_feeds: 0, first_met: now,
            checkin_last: String::new(), checkin_streak: 0, checkin_total: 0, game_best: 0, weather_report_last: String::new(),
            extra: Map::new(),
        }
    }
}
impl StatsData {
    /// 从 JSON 对象宽松解析：缺的/类型不对的字段用默认值
    pub fn from_value(v: Value) -> Self {
        match v {
            Value::Object(m) => super::config::lenient(m),
            _ => Self::default(),
        }
    }
}

/// stats.js 里 mood() 的返回值
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoodLevel { Starving, Hungry, Exhausted, Sad, Great, Normal }

/// 一次签到的结果
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CheckIn {
    pub streak: u32,
    pub total: u32,
    /// 这次加了多少亲密度
    pub bonus: f64,
}

/// day_number 的反过来：天数 → "YYYY-MM-DD"
pub fn date_of_day(n: i64) -> String {
    let d = super::memo::Dt::from_min(n * 1440);
    format!("{:04}-{:02}-{:02}", d.y, d.mo, d.d)
}

/// 亲密度对应的等级名
pub fn level_name(affection: f64) -> &'static str {
    LEVELS.iter().rev().find(|(min, _)| affection >= *min).map(|x| x.1).unwrap_or(LEVELS[0].1)
}

/// "YYYY-MM-DD" → 自 1970-01-01 起的天数（公历；格式不对返回 None）
pub fn day_number(date: &str) -> Option<i64> {
    let mut it = date.trim().splitn(3, '-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let d: i64 = it.next()?.parse().ok()?;
    if !(1..=99_999).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&d) { return None; }
    // Howard Hinnant 的 days_from_civil
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146097 + doe - 719468)
}

/// 签到的纯逻辑：今天签过返回 None；昨天签过 → 连续 +1，否则从 1 重来
pub fn next_streak(last: &str, today: &str, streak: u32) -> Option<u32> {
    let t = day_number(today)?;
    match day_number(last) {
        Some(l) if l == t => None,
        Some(l) if l == t - 1 => Some(streak.saturating_add(1).max(1)),
        _ => Some(1),
    }
}

/// 签到加的亲密度：2 + 连续天数（最多 +7）
pub fn checkin_bonus(streak: u32) -> f64 { 2.0 + streak.min(7) as f64 }

/// act() 的种类
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Act { Feed, Pat, Poke, Angry, Drag, Chat, Wake }

/// act() 的结果（目前只有 feed 会返回 Full）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActResult { Ok, Full }

pub struct Stats {
    pub data: StatsData,
    /// 每种互动上次发生的时间（ms），用于「连续摸效果递减」
    last_act: HashMap<Act, f64>,
}

impl Stats {
    /// 载入存档并做离线补算；返回实际离开的分钟数（用于打招呼）
    pub fn load(saved: Option<StatsData>) -> (Self, f64) { Self::load_at(saved, now_ms()) }

    pub fn load_at(saved: Option<StatsData>, now: f64) -> (Self, f64) {
        let mut s = saved.unwrap_or_default();
        // 离线补算：关掉期间按「睡着」的速率算，哈基米在你不在时会睡觉
        let last = if s.last_tick != 0.0 && !s.last_tick.is_nan() { s.last_tick } else { now };
        let away = ((now - last) / 60000.0).max(0.0);
        let mins = away.min(OFFLINE_CAP_MIN);
        if mins > 1.0 {
            s.hunger = clamp(s.hunger + rate::HUNGER_SLEEP * mins);
            s.energy = clamp(s.energy + rate::ENERGY_SLEEP * mins);
            s.mood = clamp(s.mood + rate::MOOD_DECAY * mins * 0.5);
        }
        s.last_tick = now;
        (Self { data: s, last_act: HashMap::new() }, away)
    }

    /// 按距上次的时间流逝更新（单次最多算 5 分钟）
    pub fn tick(&mut self, sleeping: bool) { self.tick_at(sleeping, now_ms()) }

    pub fn tick_at(&mut self, sleeping: bool, now: f64) {
        let s = &mut self.data;
        let mins = ((now - s.last_tick) / 60000.0).clamp(0.0, 5.0);
        s.last_tick = now;
        s.hunger = clamp(s.hunger + if sleeping { rate::HUNGER_SLEEP } else { rate::HUNGER_AWAKE } * mins);
        s.energy = clamp(s.energy + if sleeping { rate::ENERGY_SLEEP } else { rate::ENERGY_AWAKE } * mins);
        let mut dm = rate::MOOD_DECAY;
        if s.hunger < 30.0 { dm += rate::MOOD_HUNGRY_EXTRA; }
        if s.energy < 20.0 { dm += rate::MOOD_TIRED_EXTRA; }
        s.mood = clamp(s.mood + dm * mins);
    }

    pub fn act(&mut self, kind: Act) -> ActResult { self.act_at(kind, now_ms()) }

    pub fn act_at(&mut self, kind: Act, now: f64) -> ActResult {
        let since = now - self.last_act.get(&kind).copied().unwrap_or(0.0);
        self.last_act.insert(kind, now);
        let s = &mut self.data;
        match kind {
            Act::Feed => {
                if s.hunger >= 95.0 { s.mood = clamp(s.mood - 2.0); return ActResult::Full; }
                s.hunger = clamp(s.hunger + 25.0);
                s.mood = clamp(s.mood + 6.0);
                s.affection += 1.0;
                s.total_feeds += 1;
            }
            Act::Pat => {
                s.mood = clamp(s.mood + if since > 20000.0 { 8.0 } else { 2.0 }); // 连续摸效果递减
                if since > 20000.0 { s.affection += 2.0; }
            }
            Act::Poke => {
                s.mood = clamp(s.mood + 1.0);
                if since > 60000.0 { s.affection += 1.0; }
            }
            Act::Angry => s.mood = clamp(s.mood - 10.0),
            Act::Drag => s.mood = clamp(s.mood - 3.0),
            Act::Chat => {
                s.mood = clamp(s.mood + 3.0);
                if since > 30000.0 { s.affection += 1.0; }
            }
            Act::Wake => s.mood = clamp(s.mood - 5.0),
        }
        ActResult::Ok
    }

    /// 今日签到；今天已经签过返回 None
    pub fn checkin(&mut self, today: &str) -> Option<CheckIn> {
        let s = &mut self.data;
        let streak = next_streak(&s.checkin_last, today, s.checkin_streak)?;
        let bonus = checkin_bonus(streak);
        s.checkin_last = today.to_string();
        s.checkin_streak = streak;
        s.checkin_total = s.checkin_total.saturating_add(1);
        s.affection += bonus;
        s.mood = clamp(s.mood + 5.0);
        Some(CheckIn { streak, total: s.checkin_total, bonus })
    }

    /// 今天签过没有
    pub fn checked_in(&self, today: &str) -> bool { self.data.checkin_last == today }

    /// 小游戏/摸摸等额外奖励：直接加亲密度和心情
    pub fn reward(&mut self, affection: f64, mood: f64) {
        self.data.affection += affection.max(0.0);
        self.data.mood = clamp(self.data.mood + mood);
    }

    /// (等级下标, 等级名)：初次见面/眼熟/朋友/好朋友/形影不离/家人
    pub fn level(&self) -> (usize, &'static str) {
        let mut out = (0, LEVELS[0].1);
        for (i, (min, name)) in LEVELS.iter().enumerate() {
            if self.data.affection >= *min { out = (i, name); }
        }
        out
    }

    pub fn mood(&self) -> MoodLevel {
        let s = &self.data;
        if s.hunger < 15.0 { return MoodLevel::Starving; }
        if s.hunger < 30.0 { return MoodLevel::Hungry; }
        if s.energy < 15.0 { return MoodLevel::Exhausted; }
        if s.mood < 25.0 { return MoodLevel::Sad; }
        if s.mood > 75.0 { return MoodLevel::Great; }
        MoodLevel::Normal
    }

    /// 认识第几天（从 1 开始）
    pub fn days(&self) -> u32 { self.days_at(now_ms()) }

    pub fn days_at(&self, now: f64) -> u32 {
        let first = if self.data.first_met != 0.0 && !self.data.first_met.is_nan() { self.data.first_met } else { now };
        let d = ((now - first) / 86400000.0).floor() + 1.0;
        d.max(0.0) as u32
    }
}

pub fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: f64 = 1_763_600_000_000.0;
    const MIN: f64 = 60000.0;

    fn data(h: f64, m: f64, e: f64, a: f64) -> StatsData {
        StatsData { hunger: h, mood: m, energy: e, affection: a, last_tick: T0, total_feeds: 0, first_met: T0, extra: Map::new(), ..Default::default() }
    }
    fn approx(a: f64, b: f64) { assert!((a - b).abs() < 1e-9, "{a} != {b}"); }

    #[test]
    fn default_values() {
        let d = StatsData::default();
        assert_eq!((d.hunger, d.mood, d.energy, d.affection, d.total_feeds), (80.0, 80.0, 80.0, 0.0, 0));
    }

    #[test]
    fn load_offline_catch_up() {
        // 离开 100 分钟：按睡着速率
        let (s, away) = Stats::load_at(Some(data(80.0, 80.0, 50.0, 0.0)), T0 + 100.0 * MIN);
        approx(away, 100.0);
        approx(s.data.hunger, 80.0 - 0.1 * 100.0);
        approx(s.data.energy, 100.0); // 50 + 250 → clamp
        approx(s.data.mood, 80.0 - 0.15 * 100.0 * 0.5);
        approx(s.data.last_tick, T0 + 100.0 * MIN);
    }

    #[test]
    fn load_caps_at_12h_and_ignores_short() {
        let (s, away) = Stats::load_at(Some(data(80.0, 80.0, 50.0, 0.0)), T0 + 2000.0 * MIN);
        approx(away, 2000.0);
        approx(s.data.hunger, 80.0 - 0.1 * 720.0);
        approx(s.data.mood, 80.0 - 0.15 * 720.0 * 0.5);
        // 1 分钟以内不补算
        let (s, _) = Stats::load_at(Some(data(80.0, 80.0, 50.0, 0.0)), T0 + 0.5 * MIN);
        approx(s.data.hunger, 80.0);
        // lastTick=0 当作刚刚
        let mut d = data(80.0, 80.0, 50.0, 0.0);
        d.last_tick = 0.0;
        let (s, away) = Stats::load_at(Some(d), T0);
        approx(away, 0.0);
        approx(s.data.hunger, 80.0);
    }

    #[test]
    fn tick_awake_and_asleep() {
        let (mut s, _) = Stats::load_at(Some(data(80.0, 80.0, 80.0, 0.0)), T0);
        s.tick_at(false, T0 + 2.0 * MIN);
        approx(s.data.hunger, 79.5);
        approx(s.data.energy, 79.4);
        approx(s.data.mood, 79.7);
        s.tick_at(true, T0 + 4.0 * MIN);
        approx(s.data.hunger, 79.3);
        approx(s.data.energy, 84.4);
        approx(s.data.mood, 79.4);
    }

    #[test]
    fn tick_caps_5_min_and_extra_decay() {
        let (mut s, _) = Stats::load_at(Some(data(20.0, 50.0, 10.0, 0.0)), T0);
        s.tick_at(false, T0 + 60.0 * MIN); // 只算 5 分钟
        approx(s.data.hunger, 20.0 - 1.25);
        approx(s.data.energy, 10.0 - 1.5);
        approx(s.data.mood, 50.0 + (-0.15 - 0.3 - 0.15) * 5.0);
        approx(s.data.last_tick, T0 + 60.0 * MIN);
    }

    #[test]
    fn feed_and_full() {
        let (mut s, _) = Stats::load_at(Some(data(50.0, 50.0, 50.0, 0.0)), T0);
        assert_eq!(s.act_at(Act::Feed, T0), ActResult::Ok);
        approx(s.data.hunger, 75.0);
        approx(s.data.mood, 56.0);
        approx(s.data.affection, 1.0);
        assert_eq!(s.data.total_feeds, 1);
        assert_eq!(s.act_at(Act::Feed, T0 + 1.0), ActResult::Ok);
        approx(s.data.hunger, 100.0);
        approx(s.data.mood, 62.0);
        assert_eq!(s.act_at(Act::Feed, T0 + 2.0), ActResult::Full);
        approx(s.data.mood, 60.0);
        approx(s.data.affection, 2.0);
        assert_eq!(s.data.total_feeds, 2);
    }

    #[test]
    fn pat_diminishing() {
        let (mut s, _) = Stats::load_at(Some(data(50.0, 50.0, 50.0, 0.0)), T0);
        s.act_at(Act::Pat, T0);
        approx(s.data.mood, 58.0);
        approx(s.data.affection, 2.0);
        s.act_at(Act::Pat, T0 + 10_000.0); // 20 秒内
        approx(s.data.mood, 60.0);
        approx(s.data.affection, 2.0);
        s.act_at(Act::Pat, T0 + 10_000.0 + 20_001.0);
        approx(s.data.mood, 68.0);
        approx(s.data.affection, 4.0);
    }

    #[test]
    fn other_acts() {
        let (mut s, _) = Stats::load_at(Some(data(50.0, 50.0, 50.0, 0.0)), T0);
        s.act_at(Act::Poke, T0);
        approx(s.data.mood, 51.0);
        approx(s.data.affection, 1.0);
        s.act_at(Act::Poke, T0 + 60_000.0); // 不是 > 60000
        approx(s.data.affection, 1.0);
        s.act_at(Act::Chat, T0);
        approx(s.data.mood, 55.0);
        approx(s.data.affection, 2.0);
        s.act_at(Act::Chat, T0 + 30_000.0);
        approx(s.data.affection, 2.0);
        s.act_at(Act::Angry, T0);
        approx(s.data.mood, 48.0);
        s.act_at(Act::Drag, T0);
        approx(s.data.mood, 45.0);
        s.act_at(Act::Wake, T0);
        approx(s.data.mood, 40.0);
        let (mut s, _) = Stats::load_at(Some(data(50.0, 3.0, 50.0, 0.0)), T0);
        s.act_at(Act::Angry, T0);
        approx(s.data.mood, 0.0);
    }

    #[test]
    fn levels() {
        let lv = |a: f64| Stats::load_at(Some(data(50.0, 50.0, 50.0, a)), T0).0.level();
        assert_eq!(lv(0.0), (0, "初次见面"));
        assert_eq!(lv(9.0), (0, "初次见面"));
        assert_eq!(lv(10.0), (1, "眼熟"));
        assert_eq!(lv(30.0), (2, "朋友"));
        assert_eq!(lv(79.0), (2, "朋友"));
        assert_eq!(lv(80.0), (3, "好朋友"));
        assert_eq!(lv(200.0), (4, "形影不离"));
        assert_eq!(lv(10_000.0), (5, "家人"));
    }

    #[test]
    fn moods() {
        let m = |h: f64, mo: f64, e: f64| Stats::load_at(Some(data(h, mo, e, 0.0)), T0).0.mood();
        assert_eq!(m(14.9, 90.0, 90.0), MoodLevel::Starving);
        assert_eq!(m(29.9, 90.0, 90.0), MoodLevel::Hungry);
        assert_eq!(m(30.0, 90.0, 14.0), MoodLevel::Exhausted);
        assert_eq!(m(30.0, 24.0, 15.0), MoodLevel::Sad);
        assert_eq!(m(30.0, 76.0, 15.0), MoodLevel::Great);
        assert_eq!(m(30.0, 75.0, 15.0), MoodLevel::Normal);
        assert_eq!(m(30.0, 25.0, 15.0), MoodLevel::Normal);
    }

    #[test]
    fn day_numbers() {
        assert_eq!(day_number("1970-01-01"), Some(0));
        assert_eq!(day_number("2026-03-01").unwrap() - day_number("2026-02-28").unwrap(), 1);
        assert_eq!(day_number("2024-03-01").unwrap() - day_number("2024-02-28").unwrap(), 2);
        assert_eq!(day_number("2027-01-01").unwrap() - day_number("2026-12-31").unwrap(), 1);
        assert_eq!(day_number(""), None);
        assert_eq!(day_number("2026-13-01"), None);
    }

    #[test]
    fn checkin_streak() {
        assert_eq!(next_streak("", "2026-11-20", 0), Some(1));
        assert_eq!(next_streak("2026-11-19", "2026-11-20", 3), Some(4));
        assert_eq!(next_streak("2026-11-20", "2026-11-20", 3), None);
        assert_eq!(next_streak("2026-11-17", "2026-11-20", 3), Some(1));
        assert_eq!(next_streak("2026-12-31", "2027-01-01", 9), Some(10));
        assert_eq!(next_streak("garbage", "2026-11-20", 9), Some(1));
        let (mut s, _) = Stats::load_at(Some(data(50.0, 50.0, 50.0, 0.0)), T0);
        let c = s.checkin("2026-11-19").unwrap();
        assert_eq!((c.streak, c.total), (1, 1));
        approx(c.bonus, 3.0);
        assert!(s.checkin("2026-11-19").is_none());
        assert!(s.checked_in("2026-11-19"));
        let c = s.checkin("2026-11-20").unwrap();
        assert_eq!((c.streak, c.total), (2, 2));
        approx(s.data.affection, 3.0 + 4.0);
        // 连续 30 天封顶 +9
        approx(checkin_bonus(30), 9.0);
    }

    #[test]
    fn checkin_fields_roundtrip() {
        let mut d = data(50.0, 50.0, 50.0, 0.0);
        d.checkin_last = "2026-11-20".into();
        d.checkin_streak = 5;
        d.game_best = 7;
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v["checkinLast"], "2026-11-20");
        assert_eq!(v["checkinStreak"], 5);
        assert_eq!(v["gameBest"], 7);
        let back = StatsData::from_value(v);
        assert_eq!(back.checkin_streak, 5);
        // 老存档没有这些字段：默认值
        let old = StatsData::from_value(serde_json::json!({"hunger": 10, "checkinStreak": "bad"}));
        assert_eq!(old.checkin_streak, 0);
        assert_eq!(old.hunger, 10.0);
    }

    #[test]
    fn days() {
        let (s, _) = Stats::load_at(Some(data(50.0, 50.0, 50.0, 0.0)), T0);
        assert_eq!(s.days_at(T0), 1);
        assert_eq!(s.days_at(T0 + 86_399_999.0), 1);
        assert_eq!(s.days_at(T0 + 86_400_000.0), 2);
        assert_eq!(s.days_at(T0 + 10.5 * 86_400_000.0), 11);
    }

    #[test]
    fn clock_backward_does_not_reverse_decay() {
        let (s, away) = Stats::load_at(Some(data(60.0, 60.0, 60.0, 0.0)), T0 - 30.0 * MIN);
        approx(away, 0.0);
        approx(s.data.hunger, 60.0);
        approx(s.data.energy, 60.0);
        approx(s.data.mood, 60.0);
        let (mut s, _) = Stats::load_at(Some(data(60.0, 60.0, 60.0, 0.0)), T0);
        s.tick_at(false, T0 - 10.0 * MIN);
        approx(s.data.hunger, 60.0);
        approx(s.data.energy, 60.0);
        approx(s.data.mood, 60.0);
    }
}
