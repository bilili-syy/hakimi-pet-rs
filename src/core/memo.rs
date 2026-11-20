//! 备忘录：%APPDATA%\hakimi-pet\memo.json 的读写和「下一条什么时候响」的计算。
//! 时间一律按本地时间（GetLocalTime）的「墙上时间」存成 "YYYY-MM-DD HH:MM"，不经过时区换算。
//! 文件：{"version":1,"items":[...]}；每条和整个文件里不认识的字段原样保留，坏掉的条目也原样写回。
//! 全局状态放在一个 Mutex 里：UI 线程（平台层定时、行为层、菜单）和 AI 解析线程都会来读写。
#![allow(dead_code)]
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::sync::Mutex;

/// 到点后多久还算「准点」，超过就当作错过了（睡眠/关机醒来）
pub const LATE_MS: u64 = 2 * 60_000;
/// 「稍后再提醒」的分钟数
pub const SNOOZE_MIN: i64 = 10;

// ---------------------------------------------------------------- 本地时间

/// 本地墙上时间，精确到分钟
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Dt {
    pub y: i32,
    pub mo: u32,
    pub d: u32,
    pub h: u32,
    pub mi: u32,
}

/// 公历日期 → 自 1970-01-01 起的天数（Howard Hinnant 的 days_from_civil）
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((if m <= 2 { y + 1 } else { y }) as i32, m, d)
}

pub fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

impl Dt {
    pub fn new(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Option<Self> {
        let ok = (1..=12).contains(&mo) && d >= 1 && d <= days_in_month(y, mo) && h < 24 && mi < 60;
        ok.then_some(Self { y, mo, d, h, mi })
    }
    /// 自 1970-01-01 00:00 起的分钟数（只用来比较和加减，不代表 UTC）
    pub fn to_min(self) -> i64 {
        days_from_civil(self.y, self.mo, self.d) * 1440 + self.h as i64 * 60 + self.mi as i64
    }
    pub fn from_min(m: i64) -> Self {
        let (y, mo, d) = civil_from_days(m.div_euclid(1440));
        let r = m.rem_euclid(1440);
        Self { y, mo, d, h: (r / 60) as u32, mi: (r % 60) as u32 }
    }
    pub fn add_min(self, m: i64) -> Self { Self::from_min(self.to_min() + m) }
    pub fn add_days(self, n: i64) -> Self { self.add_min(n * 1440) }
    /// 0 = 周日 … 6 = 周六
    pub fn dow(self) -> u32 { (days_from_civil(self.y, self.mo, self.d) + 4).rem_euclid(7) as u32 }
    pub fn date(self) -> (i32, u32, u32) { (self.y, self.mo, self.d) }
    pub fn at(self, h: u32, mi: u32) -> Self { Self { h, mi, ..self } }
    pub fn fmt(self) -> String { format!("{:04}-{:02}-{:02} {:02}:{:02}", self.y, self.mo, self.d, self.h, self.mi) }
    /// "YYYY-MM-DD HH:MM"（也接受 "T" 分隔、带秒、单数字的时/分）
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let (date, time) = s.split_once([' ', 'T'])?;
        let mut dp = date.split('-');
        let y = dp.next()?.parse().ok()?;
        let mo = dp.next()?.parse().ok()?;
        let d = dp.next()?.parse().ok()?;
        let mut tp = time.split(':');
        let h = tp.next()?.trim().parse().ok()?;
        let mi = tp.next()?.trim().parse().ok()?;
        Self::new(y, mo, d, h, mi)
    }
}

/// 现在（本地时间），带秒和毫秒，平台层排定时器要精确到毫秒
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Now {
    pub dt: Dt,
    pub sec: u32,
    pub ms: u32,
}

impl Now {
    pub fn from_dt(dt: Dt) -> Self { Self { dt, sec: 0, ms: 0 } }
    #[cfg(windows)]
    pub fn get() -> Self {
        // SAFETY: GetLocalTime 只写返回值
        let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        let dt = Dt::new(t.wYear as i32, t.wMonth as u32, t.wDay as u32, t.wHour as u32, t.wMinute as u32)
            .unwrap_or(Dt { y: 2026, mo: 1, d: 1, h: 0, mi: 0 });
        Self { dt, sec: t.wSecond as u32, ms: t.wMilliseconds as u32 }
    }
    #[cfg(not(windows))]
    pub fn get() -> Self {
        let s = crate::core::log::iso_now();
        let n = |a: usize, b: usize| s.get(a..b).and_then(|x| x.parse().ok()).unwrap_or(0);
        let dt = Dt::new(n(0, 4) as i32, n(5, 7), n(8, 10), n(11, 13), n(14, 16)).unwrap_or(Dt { y: 2026, mo: 1, d: 1, h: 0, mi: 0 });
        Self { dt, sec: n(17, 19), ms: 0 }
    }
    /// 自 1970-01-01 本地零点起的毫秒数
    pub fn to_ms(self) -> i64 { self.dt.to_min() * 60_000 + self.sec as i64 * 1000 + self.ms as i64 }
}

// ---------------------------------------------------------------- 数据

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Repeat {
    Daily,
    /// 工作日（周一到周五）
    Workday,
    /// 每周同一天
    Weekly,
    /// 不重复（不认识的值也当成不重复）
    #[default]
    #[serde(other)]
    None,
}

impl Repeat {
    pub fn label(self) -> &'static str {
        match self { Repeat::None => "", Repeat::Daily => "每天", Repeat::Workday => "工作日", Repeat::Weekly => "每周" }
    }
    /// 这个日子能不能响（工作日规则只在周一到周五）
    fn day_ok(self, dt: Dt) -> bool { self != Repeat::Workday || (1..=5).contains(&dt.dow()) }
    /// 严格晚于 after 的下一次（保持 t 的时:分；每周保持星期几）
    pub fn next_after(self, t: Dt, after: Dt) -> Option<Dt> {
        let step = match self { Repeat::None => return None, Repeat::Weekly => 7, _ => 1 };
        let mut n = t;
        if n <= after {
            let days = (after.to_min() - n.to_min()) / 1440;
            n = n.add_days(days / step * step);
        }
        for _ in 0..30 {
            if n > after && self.day_ok(n) { return Some(n); }
            n = n.add_days(step);
        }
        None
    }
    /// 新建时：从 t 起（含 t）第一个合规的日子
    pub fn first_from(self, t: Dt) -> Dt {
        let mut n = t;
        for _ in 0..7 {
            if self.day_ok(n) { return n; }
            n = n.add_days(1);
        }
        t
    }
}

fn is_false(b: &bool) -> bool { !*b }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: u64,
    pub text: String,
    /// 提醒时间（本地 "YYYY-MM-DD HH:MM"）；null = 只是记一下，不提醒。重复的条目这里是「下一次」
    #[serde(default)]
    pub time: Option<String>,
    #[serde(default)]
    pub repeat: Repeat,
    #[serde(default)]
    pub done: bool,
    /// 创建时间（本地 "YYYY-MM-DD HH:MM"）
    #[serde(default)]
    pub created: String,
    /// 「稍后再提醒」到哪一刻
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snooze: Option<String>,
    /// 不重复的条目已经响过、等用户点「知道了」
    #[serde(default, skip_serializing_if = "is_false")]
    pub fired: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Item {
    pub fn time_dt(&self) -> Option<Dt> { self.time.as_deref().and_then(Dt::parse) }
    fn snooze_dt(&self) -> Option<Dt> { self.snooze.as_deref().and_then(Dt::parse) }
    /// 下一次该响的时刻
    pub fn due(&self) -> Option<Dt> {
        if self.done { return None; }
        let t = if self.fired && self.repeat == Repeat::None { None } else { self.time_dt() };
        match (t, self.snooze_dt()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
    /// 菜单/列表里显示的时间：今天 "15:00"，别的日子 "11/21 09:00"，重复 "每天 22:00"
    pub fn when_label(&self, today: Dt) -> String {
        let Some(t) = self.snooze_dt().or(self.time_dt()) else { return "随手记".into() };
        let hm = format!("{:02}:{:02}", t.h, t.mi);
        match self.repeat {
            Repeat::Daily | Repeat::Workday => format!("{} {hm}", self.repeat.label()),
            Repeat::Weekly => format!("每{} {hm}", weekday_name(t.dow())),
            Repeat::None if t.date() == today.date() => hm,
            Repeat::None if t.date() == today.add_days(1).date() => format!("明天 {hm}"),
            Repeat::None => format!("{}/{} {hm}", t.mo, t.d),
        }
    }
}

pub fn weekday_name(dow: u32) -> &'static str {
    ["周日", "周一", "周二", "周三", "周四", "周五", "周六"][(dow % 7) as usize]
}

/// 到点的一条（类型在 types.rs，平台层直接塞进 AppEvent::Memo）
pub use crate::types::MemoHit as Hit;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Book {
    pub items: Vec<Item>,
    /// 解析不了的条目，原样写回
    pub bad: Vec<Value>,
    /// 文件顶层不认识的字段
    pub extra: Map<String, Value>,
}

impl Book {
    /// 解析 memo.json（也接受直接是数组的旧格式）。整个文件不是 JSON 返回 None
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim_start_matches('\u{feff}').trim();
        if text.is_empty() { return Some(Self::default()); }
        let v: Value = serde_json::from_str(text).ok()?;
        let (arr, extra) = match v {
            Value::Array(a) => (a, Map::new()),
            Value::Object(mut o) => {
                let a = match o.remove("items") { Some(Value::Array(a)) => a, Some(_) => return None, None => vec![] };
                o.remove("version");
                (a, o)
            }
            _ => return None,
        };
        let mut b = Self { extra, ..Self::default() };
        for v in arr {
            match serde_json::from_value::<Item>(v.clone()) {
                Ok(i) => b.items.push(i),
                Err(_) => b.bad.push(v),
            }
        }
        Some(b)
    }

    pub fn to_json(&self) -> String {
        let mut o = Map::new();
        o.insert("version".into(), Value::from(1));
        let mut items: Vec<Value> = self.items.iter().filter_map(|i| serde_json::to_value(i).ok()).collect();
        items.extend(self.bad.iter().cloned());
        o.insert("items".into(), Value::Array(items));
        for (k, v) in &self.extra {
            o.entry(k.clone()).or_insert_with(|| v.clone());
        }
        serde_json::to_string_pretty(&Value::Object(o)).unwrap_or_else(|_| "{}".into())
    }

    fn next_id(&self) -> u64 {
        let bad_max = self.bad.iter().filter_map(|v| v.get("id").and_then(Value::as_u64)).max().unwrap_or(0);
        self.items.iter().map(|i| i.id).max().unwrap_or(0).max(bad_max) + 1
    }

    pub fn add(&mut self, text: &str, time: Option<Dt>, repeat: Repeat, now: Dt) -> Item {
        let repeat = if time.is_none() { Repeat::None } else { repeat };
        let time = time.map(|t| if repeat == Repeat::None { t } else { repeat.first_from(t) });
        let item = Item {
            id: self.next_id(),
            text: text.trim().to_string(),
            time: time.map(Dt::fmt),
            repeat,
            done: false,
            created: now.fmt(),
            snooze: None,
            fired: false,
            extra: Map::new(),
        };
        self.items.push(item.clone());
        item
    }

    /// 备忘录窗口里改一条：内容/时间/重复都换掉，重新算「没响过」
    pub fn update(&mut self, id: u64, text: &str, time: Option<Dt>, repeat: Repeat) -> bool {
        let Some(i) = self.get_mut(id) else { return false };
        let repeat = if time.is_none() { Repeat::None } else { repeat };
        i.text = text.trim().to_string();
        i.time = time.map(|t| if repeat == Repeat::None { t } else { repeat.first_from(t) }).map(Dt::fmt);
        i.repeat = repeat;
        i.done = false;
        i.fired = false;
        i.snooze = None;
        true
    }

    pub fn get(&self, id: u64) -> Option<&Item> { self.items.iter().find(|i| i.id == id) }
    fn get_mut(&mut self, id: u64) -> Option<&mut Item> { self.items.iter_mut().find(|i| i.id == id) }

    /// 最早的到期时刻
    pub fn next_due(&self) -> Option<Dt> { self.items.iter().filter_map(Item::due).min() }

    /// 到点的条目：不重复的标记 fired，重复的排到下一次（错过好几次也只响一次）
    pub fn poll(&mut self, now: Now) -> Vec<Hit> {
        let now_ms = now.to_ms();
        let mut hits = vec![];
        for it in &mut self.items {
            let Some(due) = it.due() else { continue };
            if due.to_min() * 60_000 > now_ms { continue; }
            let late = (now_ms - due.to_min() * 60_000) as u64 > LATE_MS;
            hits.push(Hit { id: it.id, text: it.text.clone(), late });
            it.snooze = None;
            match it.repeat {
                Repeat::None => it.fired = true,
                r => {
                    if let Some(t) = it.time_dt()
                        && t <= now.dt
                    {
                        it.time = r.next_after(t, now.dt).map(Dt::fmt);
                    }
                }
            }
        }
        hits
    }

    /// 点她 = 知道了：不重复的标完成
    pub fn ack(&mut self, ids: &[u64]) -> bool {
        let mut changed = false;
        for &id in ids {
            if let Some(i) = self.get_mut(id)
                && i.repeat == Repeat::None
                && !i.done
            {
                i.done = true;
                i.fired = false;
                changed = true;
            }
        }
        changed
    }

    /// 稍后再提醒
    pub fn snooze(&mut self, ids: &[u64], now: Dt, minutes: i64) -> bool {
        let at = now.add_min(minutes).fmt();
        let mut changed = false;
        for &id in ids {
            if let Some(i) = self.get_mut(id)
                && !i.done
            {
                i.snooze = Some(at.clone());
                changed = true;
            }
        }
        changed
    }

    /// 菜单「完成」：重复的也一起停掉
    pub fn complete(&mut self, id: u64) -> bool {
        match self.get_mut(id) {
            Some(i) if !i.done => {
                i.done = true;
                i.fired = false;
                i.snooze = None;
                true
            }
            _ => false,
        }
    }

    pub fn delete(&mut self, id: u64) -> bool {
        let n = self.items.len();
        self.items.retain(|i| i.id != id);
        n != self.items.len()
    }

    pub fn clear_done(&mut self) -> usize {
        let n = self.items.len();
        self.items.retain(|i| !i.done);
        n - self.items.len()
    }

    /// 没完成的，按时间排（随手记的排最后）
    pub fn pending(&self) -> Vec<Item> {
        let mut v: Vec<Item> = self.items.iter().filter(|i| !i.done).cloned().collect();
        v.sort_by_key(|i| (i.snooze_dt().or(i.time_dt()).is_none(), i.snooze_dt().or(i.time_dt()), i.id));
        v
    }

    /// 今天还要做的：(今天有时间的, 随手记的)
    pub fn today(&self, now: Dt) -> (Vec<Item>, Vec<Item>) {
        let mut timed = vec![];
        let mut loose = vec![];
        for i in self.pending() {
            match i.snooze_dt().or(i.time_dt()) {
                Some(t) if t.date() == now.date() => timed.push(i),
                // 今天已经响过但还没点「知道了」的也算
                Some(t) if t < now && i.fired => timed.push(i),
                None => loose.push(i),
                _ => {}
            }
        }
        (timed, loose)
    }
}

// ---------------------------------------------------------------- 全局（带文件）

struct Store {
    book: Book,
    /// 上次读/写文件时的修改时间，用来发现用户手改了 memo.json
    mtime: Option<std::time::SystemTime>,
    checked_at: std::time::Instant,
    /// 正在响、还没点掉的
    ringing: Vec<u64>,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);
static VERSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub fn file() -> PathBuf { super::paths::data_dir().join("memo.json") }

fn mtime() -> Option<std::time::SystemTime> { std::fs::metadata(file()).and_then(|m| m.modified()).ok() }

fn read_file() -> Book {
    let p = file();
    let Ok(s) = std::fs::read_to_string(&p) else { return Book::default() };
    match Book::parse(&s) {
        Some(b) => b,
        None => {
            // 整个文件坏了：留个备份再从空的开始，绝不默默覆盖用户的东西
            let mut bak = p.as_os_str().to_owned();
            bak.push(".broken");
            let _ = std::fs::copy(&p, PathBuf::from(bak));
            super::log::log("memo.json unreadable, backed up to memo.json.broken");
            Book::default()
        }
    }
}

fn bump() { VERSION.fetch_add(1, std::sync::atomic::Ordering::Relaxed); }

/// 数据变了就 +1（菜单据此决定要不要重建）
pub fn version() -> u64 { VERSION.load(std::sync::atomic::Ordering::Relaxed) }

fn with<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    let mut g = STORE.lock().unwrap_or_else(|e| e.into_inner());
    let s = g.get_or_insert_with(|| Store {
        book: read_file(),
        mtime: mtime(),
        checked_at: std::time::Instant::now(),
        ringing: vec![],
    });
    // 用户可能手改了文件：最多 5 秒看一次修改时间
    if s.checked_at.elapsed().as_secs() >= 5 {
        s.checked_at = std::time::Instant::now();
        let m = mtime();
        if m != s.mtime {
            s.book = read_file();
            s.mtime = m;
            bump();
        }
    }
    f(s)
}

fn save(s: &mut Store) {
    let json = s.book.to_json();
    if let Err(e) = super::save::write_atomic(&file(), json.as_bytes()) {
        super::log::log(&format!("memo write failed {e}"));
    }
    s.mtime = mtime();
    bump();
}

/// 改数据并在有变化时写盘
fn edit<R>(f: impl FnOnce(&mut Book) -> (R, bool)) -> R {
    with(|s| {
        let (r, changed) = f(&mut s.book);
        if changed { save(s); }
        r
    })
}

pub fn add(text: &str, time: Option<Dt>, repeat: Repeat) -> Item {
    let now = Now::get().dt;
    edit(|b| (b.add(text, time, repeat, now), true))
}

/// 到点的条目（平台层每次定时器到期时调）
pub fn poll(now: Now) -> Vec<Hit> {
    with(|s| {
        if s.book.next_due().is_none_or(|d| d.to_min() * 60_000 > now.to_ms()) {
            return vec![];
        }
        let hits = s.book.poll(now);
        if !hits.is_empty() {
            for h in &hits {
                if !s.ringing.contains(&h.id) { s.ringing.push(h.id); }
            }
            save(s);
        }
        hits
    })
}

/// 距离下一条提醒还有多少毫秒（没有返回 None；已经过了返回 0）
pub fn ms_until_next(now: Now) -> Option<u64> {
    with(|s| s.book.next_due()).map(|d| (d.to_min() * 60_000 - now.to_ms()).max(0) as u64)
}

pub fn ringing() -> Vec<u64> { with(|s| s.ringing.clone()) }

fn unring(s: &mut Store, ids: &[u64]) {
    let before = s.ringing.len();
    s.ringing.retain(|i| !ids.contains(i));
    if s.ringing.len() != before { bump(); }
}

/// 点她 = 知道了
pub fn ack(ids: &[u64]) {
    with(|s| {
        unring(s, ids);
        if s.book.ack(ids) { save(s); }
    })
}

/// 稍后 10 分钟再提醒
pub fn snooze(ids: &[u64]) {
    let now = Now::get().dt;
    with(|s| {
        unring(s, ids);
        if s.book.snooze(ids, now, SNOOZE_MIN) { save(s); }
    })
}

pub fn complete(id: u64) -> bool {
    with(|s| {
        unring(s, &[id]);
        let c = s.book.complete(id);
        if c { save(s); }
        c
    })
}

pub fn delete(id: u64) -> bool {
    with(|s| {
        unring(s, &[id]);
        let c = s.book.delete(id);
        if c { save(s); }
        c
    })
}

pub fn update(id: u64, text: &str, time: Option<Dt>, repeat: Repeat) -> bool {
    with(|s| {
        unring(s, &[id]);
        let c = s.book.update(id, text, time, repeat);
        if c { save(s); }
        c
    })
}

pub fn clear_done() -> usize { edit(|b| { let n = b.clear_done(); (n, n > 0) }) }

pub fn pending() -> Vec<Item> { with(|s| s.book.pending()) }

pub fn today(now: Dt) -> (Vec<Item>, Vec<Item>) { with(|s| s.book.today(now)) }

pub fn get(id: u64) -> Option<Item> { with(|s| s.book.get(id).cloned()) }

/// 打开前确保文件存在
pub fn ensure_file() -> PathBuf {
    let p = file();
    if !p.exists() {
        with(save);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Dt { Dt::new(y, mo, d, h, mi).unwrap() }

    #[test]
    fn calendar_math() {
        let t = dt(2026, 11, 20, 12, 41);
        assert_eq!(t.dow(), 5); // 2026-11-20 是周五
        assert_eq!(dt(1970, 1, 1, 0, 0).to_min(), 0);
        assert_eq!(Dt::from_min(t.to_min()), t);
        assert_eq!(t.add_days(12).date(), (2026, 12, 2));
        assert_eq!(dt(2026, 12, 31, 23, 50).add_min(20), dt(2027, 1, 1, 0, 10));
        assert_eq!(dt(2028, 2, 28, 9, 0).add_days(1).date(), (2028, 2, 29));
        assert!(Dt::new(2026, 2, 29, 0, 0).is_none());
        assert_eq!(Dt::parse("2026-11-20 09:05"), Some(dt(2026, 11, 20, 9, 5)));
        assert_eq!(Dt::parse("2026-11-20T9:05:30"), Some(dt(2026, 11, 20, 9, 5)));
        assert_eq!(Dt::parse("2026-11-20"), None);
        assert_eq!(dt(2026, 1, 2, 3, 4).fmt(), "2026-01-02 03:04");
    }

    #[test]
    fn repeat_rules() {
        let fri = dt(2026, 11, 20, 22, 0);
        assert_eq!(Repeat::Daily.next_after(fri, fri), Some(dt(2026, 11, 21, 22, 0)));
        // 周五之后的下一个工作日是周一
        assert_eq!(Repeat::Workday.next_after(fri, fri), Some(dt(2026, 11, 23, 22, 0)));
        assert_eq!(Repeat::Weekly.next_after(fri, fri), Some(dt(2026, 11, 27, 22, 0)));
        // 错过很多天：直接跳到将来那一次
        assert_eq!(Repeat::Daily.next_after(fri, dt(2026, 12, 5, 23, 0)), Some(dt(2026, 12, 6, 22, 0)));
        assert_eq!(Repeat::Weekly.next_after(fri, dt(2026, 12, 5, 23, 0)), Some(dt(2026, 12, 11, 22, 0)));
        assert_eq!(Repeat::None.next_after(fri, fri), None);
        assert_eq!(Repeat::Workday.first_from(dt(2026, 11, 21, 9, 0)), dt(2026, 11, 23, 9, 0));
    }

    #[test]
    fn file_roundtrip_keeps_unknown() {
        let s = r#"{"version":1,"owner":"me","items":[
            {"id":3,"text":"开会","time":"2026-11-20 15:00","repeat":"none","done":false,"created":"2026-11-20 12:00","color":"red"},
            {"id":"bad"},
            {"id":4,"text":"洗漱","time":"2026-11-20 22:00","repeat":"fortnightly","done":false,"created":""}
        ]}"#;
        let b = Book::parse(s).unwrap();
        assert_eq!(b.items.len(), 2);
        assert_eq!(b.bad.len(), 1);
        assert_eq!(b.items[0].extra["color"], "red");
        assert_eq!(b.items[1].repeat, Repeat::None);
        let out: Value = serde_json::from_str(&b.to_json()).unwrap();
        assert_eq!(out["owner"], "me");
        assert_eq!(out["items"][0]["color"], "red");
        assert_eq!(out["items"][2]["id"], "bad");
        assert!(Book::parse("[]").unwrap().items.is_empty());
        assert!(Book::parse("").unwrap().items.is_empty());
        assert!(Book::parse("{not json").is_none());
        let mut b = b;
        assert_eq!(b.add("x", None, Repeat::Daily, dt(2026, 1, 1, 0, 0)).id, 5);
    }

    #[test]
    fn poll_fires_once_and_merges_missed() {
        let now = dt(2026, 11, 20, 12, 0);
        let mut b = Book::default();
        let a = b.add("开会", Some(dt(2026, 11, 20, 15, 0)), Repeat::None, now);
        let w = b.add("喝药", Some(dt(2026, 11, 20, 15, 30)), Repeat::None, now);
        let d = b.add("洗漱", Some(dt(2026, 11, 20, 22, 0)), Repeat::Daily, now);
        b.add("买猫粮", None, Repeat::None, now);
        assert_eq!(b.next_due(), Some(dt(2026, 11, 20, 15, 0)));
        assert!(b.poll(Now::from_dt(dt(2026, 11, 20, 14, 59))).is_empty());
        let h = b.poll(Now { dt: dt(2026, 11, 20, 15, 0), sec: 3, ms: 0 });
        assert_eq!(h, vec![Hit { id: a.id, text: "开会".into(), late: false }]);
        assert!(b.poll(Now::from_dt(dt(2026, 11, 20, 15, 1))).is_empty());
        // 睡了一晚上：喝药和洗漱一起补，洗漱排到下一天
        let h = b.poll(Now::from_dt(dt(2026, 11, 21, 8, 0)));
        assert_eq!(h.len(), 2);
        assert!(h.iter().all(|x| x.late));
        assert_eq!(b.get(d.id).unwrap().time.as_deref(), Some("2026-11-21 22:00"));
        assert!(b.get(w.id).unwrap().fired);
        assert_eq!(b.next_due(), Some(dt(2026, 11, 21, 22, 0)));
    }

    #[test]
    fn ack_snooze_complete() {
        let now = dt(2026, 11, 20, 12, 0);
        let mut b = Book::default();
        let a = b.add("开会", Some(dt(2026, 11, 20, 15, 0)), Repeat::None, now);
        let d = b.add("洗漱", Some(dt(2026, 11, 20, 22, 0)), Repeat::Daily, now);
        b.poll(Now::from_dt(dt(2026, 11, 20, 15, 0)));
        assert!(b.snooze(&[a.id], dt(2026, 11, 20, 15, 0), 10));
        assert_eq!(b.next_due(), Some(dt(2026, 11, 20, 15, 10)));
        assert_eq!(b.poll(Now::from_dt(dt(2026, 11, 20, 15, 10))).len(), 1);
        assert!(b.ack(&[a.id]));
        assert!(b.get(a.id).unwrap().done);
        // 重复的点「知道了」不算完成
        assert!(!b.ack(&[d.id]));
        assert!(b.complete(d.id));
        assert_eq!(b.next_due(), None);
        assert_eq!(b.clear_done(), 2);
        assert!(b.items.is_empty());
    }

    #[test]
    fn today_list_and_labels() {
        let now = dt(2026, 11, 20, 12, 0);
        let mut b = Book::default();
        b.add("洗漱", Some(dt(2026, 11, 20, 22, 0)), Repeat::Daily, now);
        b.add("开会", Some(dt(2026, 11, 20, 15, 0)), Repeat::None, now);
        b.add("交报告", Some(dt(2026, 11, 21, 9, 0)), Repeat::None, now);
        b.add("买猫粮", None, Repeat::None, now);
        let (t, l) = b.today(now);
        assert_eq!(t.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(), ["开会", "洗漱"]);
        assert_eq!(l[0].text, "买猫粮");
        let p = b.pending();
        assert_eq!(p[0].when_label(now), "15:00");
        assert_eq!(p[1].when_label(now), "每天 22:00");
        assert_eq!(p[2].when_label(now), "明天 09:00");
        assert_eq!(p[3].when_label(now), "随手记");
    }
}
