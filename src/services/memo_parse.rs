//! 聊天里的「提醒我…/记一下…」识别：先用本地规则解析中文时间，解析不了再让 AI 提取成 JSON。
//! 纯函数部分（parse / confirm_line / today_line / parse_ai）不碰时钟和磁盘，测试里直接给「现在」。
#![allow(dead_code)]
use crate::core::memo::{Dt, Item, Now, Repeat, weekday_name};
use crate::services::ai::ChatCtx;
use crate::types::{AppEvent, EventSender, MemoEv};
use serde_json::Value;

/// 解析出来的一条备忘
#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    pub text: String,
    /// None = 只是记一下
    pub time: Option<Dt>,
    pub repeat: Repeat,
    /// 「X 分钟后」这类相对时间（确认台词用）
    pub rel_min: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Parsed {
    /// 是备忘，时间也解析出来了
    Memo(Draft),
    /// 问「我有什么待办」
    Query,
    /// 像是要设提醒，但时间没看懂 → 交给 AI
    Unsure,
    /// 普通聊天
    NotMemo,
}

/// 内容为空时（「半小时后叫我」）用的说法
const DEFAULT_WHAT: &str = "起来";

// ---------------------------------------------------------------- 扫描

#[derive(Clone, Copy, Debug, PartialEq)]
enum Day {
    Offset(i64),
    /// dow 0=周日；next = 「下周X」
    Week { dow: u32, next: bool },
    Date { mo: Option<u32>, d: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Period { Dawn, Morning, Forenoon, Noon, Afternoon, Dusk, Evening, Night }

impl Period {
    fn default_hour(self) -> u32 {
        match self {
            Period::Dawn => 6,
            Period::Morning => 8,
            Period::Forenoon => 9,
            Period::Noon => 12,
            Period::Afternoon => 15,
            Period::Dusk => 18,
            Period::Evening => 20,
            Period::Night => 23,
        }
    }
    /// 12 小时制的钟点 → (24 小时制, 要不要往后挪一天)
    fn adjust(self, h: u32) -> (u32, i64) {
        match self {
            Period::Dawn => (h % 12, 0),
            Period::Morning | Period::Forenoon => (h, 0),
            Period::Noon => (if h <= 3 { h + 12 } else { h }, 0),
            Period::Afternoon | Period::Dusk => (if h < 12 { h + 12 } else { h }, 0),
            Period::Evening => match h {
                12 => (0, 1),
                1..=4 => (h, 1),
                5..=11 => (h + 12, 0),
                _ => (h, 0),
            },
            Period::Night => match h {
                12 => (0, 1),
                0..=5 => (h, 1),
                6..=11 => (h + 12, 0),
                _ => (h, 0),
            },
        }
    }
}

#[derive(Debug, Default)]
struct Feat {
    rel: Option<i64>,
    /// 「等会儿」「一会儿」这种说不准的
    vague: bool,
    day: Option<Day>,
    period: Option<Period>,
    clock: Option<(u32, u32)>,
    repeat: Option<Repeat>,
    rep_dow: Option<u32>,
    /// 提醒我 / 叫我 / 别忘了
    trigger: bool,
    /// 记得
    soft: bool,
    /// 记一下 / 帮我记
    note: bool,
    /// 第一个时间词出现的位置
    first_time: Option<usize>,
}

struct Scan {
    c: Vec<char>,
    used: Vec<bool>,
}

fn cn_digit(c: char) -> Option<u32> {
    Some(match c {
        '零' | '〇' => 0,
        '一' => 1,
        '二' | '两' => 2,
        '三' => 3,
        '四' => 4,
        '五' => 5,
        '六' => 6,
        '七' => 7,
        '八' => 8,
        '九' => 9,
        _ => return None,
    })
}

fn dow_char(c: char) -> Option<u32> {
    Some(match c {
        '一' => 1,
        '二' => 2,
        '三' => 3,
        '四' => 4,
        '五' => 5,
        '六' => 6,
        '日' | '天' | '七' => 0,
        _ => return None,
    })
}

/// 周一=0 … 周日=6
fn mon_index(dow: u32) -> i64 { ((dow + 6) % 7) as i64 }

impl Scan {
    fn new(s: &str) -> Self {
        let c: Vec<char> = s
            .chars()
            .filter_map(|c| match c {
                '０'..='９' => char::from_u32(c as u32 - '０' as u32 + '0' as u32),
                '：' => Some(':'),
                c if c.is_whitespace() => None,
                c => Some(c),
            })
            .collect();
        let used = vec![false; c.len()];
        Self { c, used }
    }

    fn at(&self, i: usize) -> Option<char> { self.c.get(i).copied() }

    fn starts(&self, i: usize, pat: &str) -> bool {
        pat.chars().enumerate().all(|(k, p)| self.at(i + k) == Some(p))
    }

    /// 从 i 开始匹配列表里最先命中的词，返回 (词序号, 长度)
    fn word(&self, i: usize, words: &[&str]) -> Option<(usize, usize)> {
        words.iter().enumerate().find(|(_, w)| self.starts(i, w)).map(|(k, w)| (k, w.chars().count()))
    }

    /// 阿拉伯数字或中文数字（到 99），返回 (值, 长度)
    fn num(&self, i: usize) -> Option<(u32, usize)> {
        let digits = self.c.get(i..).unwrap_or(&[]).iter().take_while(|c| c.is_ascii_digit()).count();
        if digits > 0 {
            let v = self.c[i..i + digits].iter().fold(0u32, |v, c| v.saturating_mul(10).saturating_add(*c as u32 - '0' as u32));
            return Some((v, digits));
        }
        // 中文：[一-九]?十[一-九]? 或单个数字
        let a = self.at(i).and_then(cn_digit);
        if self.at(i) == Some('十') {
            let b = self.at(i + 1).and_then(cn_digit);
            return Some((10 + b.unwrap_or(0), if b.is_some() { 2 } else { 1 }));
        }
        let a = a?;
        if self.at(i + 1) == Some('十') {
            let b = self.at(i + 2).and_then(cn_digit);
            return Some((a * 10 + b.unwrap_or(0), if b.is_some() { 3 } else { 2 }));
        }
        Some((a, 1))
    }

    fn mark(&mut self, i: usize, n: usize) {
        for u in self.used.iter_mut().skip(i).take(n) {
            *u = true;
        }
    }

    /// 「后 / 以后 / 之后」
    fn after_suffix(&self, i: usize) -> usize {
        self.word(i, &["以后", "之后", "后"]).map(|(_, n)| n).unwrap_or(0)
    }

    /// 相对时间：半小时后 / 10分钟后 / 两个小时后 / 一个半小时后 / 1小时20分钟后 / 过10分钟 / 一刻钟后
    fn relative(&self, i: usize) -> Option<(i64, usize)> {
        let pre = if self.at(i) == Some('过') { 1 } else { 0 };
        let mut j = i + pre;
        if let Some((_, n)) = self.word(j, &["半个小时", "半个钟头", "半小时"]) {
            j += n;
            let s = self.after_suffix(j);
            return (s > 0 || pre > 0).then_some((30, j + s - i));
        }
        if self.starts(j, "一刻钟") {
            j += 3;
            let s = self.after_suffix(j);
            return (s > 0 || pre > 0).then_some((15, j + s - i));
        }
        let (v, n) = self.num(j)?;
        j += n;
        let mut total: i64;
        if let Some((_, n)) = self.word(j, &["个半小时", "个半钟头", "个小时", "个钟头", "小时", "钟头"]) {
            let half = self.starts(j, "个半");
            total = v as i64 * 60 + if half { 30 } else { 0 };
            j += n;
            if !half && self.starts(j, "半") {
                total += 30;
                j += 1;
            } else if let Some((m, mn)) = self.num(j)
                && let Some((_, un)) = self.word(j + mn, &["分钟", "分"])
            {
                total += m as i64;
                j += mn + un;
            }
        } else if let Some((_, un)) = self.word(j, &["分钟", "分"]) {
            total = v as i64;
            j += un;
        } else {
            return None;
        }
        let s = self.after_suffix(j);
        (s > 0 || pre > 0).then_some((total, j + s - i))
    }

    /// 钟点：3点 / 三点半 / 9点一刻 / 8点45 / 8点45分 / 10点整 / 3点钟 / 15:30
    fn clock(&self, i: usize) -> Option<((u32, u32), usize)> {
        let (h, n) = self.num(i)?;
        let mut j = i + n;
        if self.at(j) == Some(':') {
            let (m, mn) = self.num(j + 1).filter(|(_, mn)| *mn == 2)?;
            return (h <= 24 && m < 60).then_some(((h, m), n + 1 + mn));
        }
        if !matches!(self.at(j), Some('点' | '时')) || h > 24 {
            return None;
        }
        // 「一点点」「两点儿」不是钟点
        if matches!(self.at(j + 1), Some('点' | '儿')) {
            return None;
        }
        // 「几时」「何时」不会走到这；「小时」由 relative 先吃掉
        j += 1;
        let mut m = 0;
        if self.starts(j, "钟") {
            j += 1;
        } else if self.starts(j, "半") {
            m = 30;
            j += 1;
        } else if self.starts(j, "一刻") {
            m = 15;
            j += 2;
        } else if self.starts(j, "三刻") {
            m = 45;
            j += 2;
        } else if self.starts(j, "整") {
            j += 1;
        } else if let Some((mm, mn)) = self.num(j)
            && mm < 60
            // 「3点1刻」也认；「3点5分钟后」不是钟点
            && !self.starts(j + mn, "分钟")
        {
            m = mm;
            j += mn;
            if self.starts(j, "分") {
                j += 1;
            }
        }
        Some(((h, m), j - i))
    }

    fn weekday(&self, i: usize) -> Option<(Day, usize)> {
        let (next, p) = match self.word(i, &["下个", "下", "这个", "这", "本"]) {
            Some((k, n)) => (k <= 1, n),
            None => (false, 0),
        };
        let (_, w) = self.word(i + p, &["星期", "礼拜", "周"])?;
        let dow = self.at(i + p + w).and_then(dow_char)?;
        Some((Day::Week { dow, next }, p + w + 1))
    }

    fn date(&self, i: usize) -> Option<(Day, usize)> {
        let (a, n) = self.num(i)?;
        let j = i + n;
        if self.at(j) == Some('月') {
            let (d, dn) = self.num(j + 1)?;
            let k = j + 1 + dn;
            let s = if matches!(self.at(k), Some('号' | '日')) { 1 } else { 0 };
            return ((1..=12).contains(&a) && (1..=31).contains(&d)).then_some((Day::Date { mo: Some(a), d }, n + 1 + dn + s));
        }
        if matches!(self.at(j), Some('号' | '日')) && (1..=31).contains(&a) {
            return Some((Day::Date { mo: None, d: a }, n + 1));
        }
        None
    }

    fn scan(&mut self) -> Feat {
        let mut f = Feat::default();
        // 记一下：只认开头
        if let Some((_, n)) = self.word(0, &["帮我记一下", "帮我记下", "帮我记", "记一下", "记下来", "记下", "记住", "备忘一下", "备忘"]) {
            f.note = true;
            self.mark(0, n);
            if matches!(self.at(n), Some(':' | ',' | '，' | '、')) {
                self.mark(n, 1);
            }
        }
        let mut i = 0;
        while i < self.c.len() {
            if self.used[i] {
                i += 1;
                continue;
            }
            let start = i;
            let mut time_word = true;
            let mut n = 0;
            if let Some((k, len)) = self.word(i, &["每个工作日", "工作日", "周一到周五", "每天", "每日", "天天", "每晚", "每早"]) {
                f.repeat = Some(if k <= 2 { Repeat::Workday } else { Repeat::Daily });
                if k == 6 {
                    f.period = Some(Period::Evening);
                } else if k == 7 {
                    f.period = Some(Period::Morning);
                }
                n = len;
            } else if let Some((_, len)) = self.word(i, &["每个星期", "每个礼拜", "每个周", "每星期", "每礼拜", "每周"]) {
                f.repeat = Some(Repeat::Weekly);
                n = len;
                if let Some(d) = self.at(i + len).and_then(dow_char) {
                    f.rep_dow = Some(d);
                    n += 1;
                }
            } else if let Some((m, len)) = self.relative(i) {
                f.rel = Some(m);
                n = len;
            } else if let Some((_, len)) = self.word(i, &["等一会儿", "等会儿", "等会", "一会儿", "待会儿", "待会", "过会儿", "晚点", "回头"]) {
                f.vague = true;
                n = len;
            } else if let Some((k, len)) = self.word(i, &["大后天", "后天", "明天", "明早", "明晚", "明儿", "今天", "今早", "今晚", "今儿", "今夜"]) {
                const OFF: [i64; 11] = [3, 2, 1, 1, 1, 1, 0, 0, 0, 0, 0];
                f.day = Some(Day::Offset(OFF[k]));
                match k {
                    3 | 7 => f.period = Some(Period::Morning),
                    4 | 8 | 10 => f.period = Some(Period::Evening),
                    _ => {}
                }
                n = len;
            } else if let Some((d, len)) = self.weekday(i) {
                f.day = Some(d);
                n = len;
            } else if let Some((p, len)) = self.word(i, &["凌晨", "早上", "早晨", "清晨", "上午", "中午", "下午", "傍晚", "晚上", "夜晚", "晚间", "夜里", "半夜"]) {
                const P: [Period; 13] = [
                    Period::Dawn, Period::Morning, Period::Morning, Period::Morning, Period::Forenoon, Period::Noon,
                    Period::Afternoon, Period::Dusk, Period::Evening, Period::Evening, Period::Evening, Period::Night,
                    Period::Night,
                ];
                f.period = Some(P[p]);
                n = len;
            } else if let Some((hm, len)) = self.clock(i) {
                f.clock = Some(hm);
                n = len;
            } else if let Some((d, len)) = self.date(i) {
                f.day = Some(d);
                n = len;
            } else {
                time_word = false;
                if let Some((_, len)) = self.word(i, &["提醒我一下", "提醒一下", "提醒我", "叫我", "喊我", "别忘了", "别忘记", "提醒"]) {
                    f.trigger = true;
                    n = len;
                } else if let Some((_, len)) = self.word(i, &["记得"]) {
                    f.soft = true;
                    n = len;
                }
            }
            if n > 0 {
                if time_word && f.first_time.is_none() {
                    f.first_time = Some(start);
                }
                self.mark(i, n);
                i += n;
            } else {
                i += 1;
            }
        }
        f
    }

    /// 没被时间词/触发词吃掉的部分，去掉两头的虚词和标点
    fn content(&self) -> String {
        let s: String = self.c.iter().zip(&self.used).filter(|(_, u)| !**u).map(|(c, _)| *c).collect();
        clean_content(&s)
    }
}

fn clean_content(s: &str) -> String {
    const PUNCT: &[char] = &['，', ',', '。', '.', '！', '!', '？', '?', ':', '、', '～', '~', '…', '；', ';', '"', '“', '”', '「', '」'];
    let mut s = s.trim_matches(PUNCT).to_string();
    loop {
        let before = s.len();
        for p in ["我要", "我得", "我该", "我去", "的时候", "到时候", "的", "要", "该", "再", "一下", "就"] {
            if let Some(r) = s.strip_prefix(p)
                && !r.is_empty()
            {
                s = r.to_string();
            }
        }
        for p in ["一下", "哦", "喔", "喵", "吧", "啊", "呀", "呢", "嘛", "哈", "的事", "这件事"] {
            if let Some(r) = s.strip_suffix(p)
                && !r.is_empty()
            {
                s = r.to_string();
            }
        }
        s = s.trim_matches(PUNCT).to_string();
        if s.len() == before {
            break;
        }
    }
    s
}

fn is_question(s: &str) -> bool {
    ["？", "?", "吗", "什么", "怎么", "几点", "为什么", "多少", "哪", "啥", "如何", "是不是"].iter().any(|q| s.contains(q))
}

fn is_query(s: &str) -> bool {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if s.starts_with("记") || s.starts_with("帮我记") {
        return false;
    }
    if (s.contains("待办") || s.contains("备忘录") || s.contains("备忘")) && (is_question(&s) || s.chars().count() <= 6) {
        return true;
    }
    ["有什么事", "有啥事", "有什么安排", "有啥安排", "有什么提醒", "有哪些提醒", "要做什么", "有什么要做", "有哪些事", "还有什么事"]
        .iter()
        .any(|k| s.contains(k))
}

// ---------------------------------------------------------------- 解析

pub fn parse(text: &str, now: Dt) -> Parsed {
    if is_query(text) {
        return Parsed::Query;
    }
    let mut sc = Scan::new(text);
    if sc.c.is_empty() {
        return Parsed::NotMemo;
    }
    let f = sc.scan();
    let what = sc.content();
    let question = is_question(text);
    let has_clock = f.clock.is_some() || f.rel.is_some();
    let leads_with_time = f.first_time == Some(0) && has_clock;

    let wants = f.note || f.trigger || (f.soft && (has_clock || f.day.is_some())) || (leads_with_time && !question && !what.is_empty());
    if !wants {
        return Parsed::NotMemo;
    }
    if question && !f.note {
        return if f.trigger { Parsed::Unsure } else { Parsed::NotMemo };
    }
    let resolved = resolve(&f, now);
    let what = if what.is_empty() {
        if f.note {
            return Parsed::NotMemo;
        }
        DEFAULT_WHAT.to_string()
    } else {
        what
    };
    match resolved {
        Ok(Some((t, repeat))) => Parsed::Memo(Draft { text: what, time: Some(t), repeat, rel_min: f.rel }),
        // 「记一下：买猫粮」：不定时
        Ok(None) if f.note => Parsed::Memo(Draft { text: what, time: None, repeat: Repeat::None, rel_min: None }),
        _ => Parsed::Unsure,
    }
}

/// 特征 → 具体时刻。Ok(None) = 没说时间；Err = 说了但没看懂
fn resolve(f: &Feat, now: Dt) -> Result<Option<(Dt, Repeat)>, ()> {
    if let Some(r) = f.rel {
        return if (1..=60 * 24 * 30).contains(&r) { Ok(Some((now.add_min(r), Repeat::None))) } else { Err(()) };
    }
    if f.vague && f.clock.is_none() {
        return Err(());
    }
    if f.clock.is_none() && f.period.is_none() && f.day.is_none() && f.repeat.is_none() {
        return Ok(None);
    }
    let repeat = f.repeat.unwrap_or(Repeat::None);
    let today = now.at(0, 0);

    // 时:分
    let explicit_day = !matches!(f.day, None | Some(Day::Offset(0)));
    let (mut h, m, mut extra) = match (f.clock, f.period) {
        (Some((h, m)), Some(p)) => {
            let (h, e) = p.adjust(h);
            (h, m, e)
        }
        (Some((h, m)), None) => (h, m, 0),
        (None, Some(p)) => (p.default_hour(), 0, 0),
        (None, None) => (9, 0, 0),
    };
    if h == 24 {
        h = 0;
        extra += 1;
    }
    if h > 23 || m > 59 {
        return Err(());
    }
    // 没说上午下午的 1~11 点
    let ambiguous = f.clock.is_some() && f.period.is_none() && (1..=11).contains(&h);

    // 日期
    let date = match f.day {
        None if repeat == Repeat::Weekly => {
            let dow = f.rep_dow.unwrap_or(now.dow());
            today.add_days((dow as i64 - now.dow() as i64).rem_euclid(7))
        }
        None | Some(Day::Offset(0)) => today,
        Some(Day::Offset(n)) => today.add_days(n),
        Some(Day::Week { dow, next }) => {
            let days = if next {
                7 - mon_index(now.dow()) + mon_index(dow)
            } else {
                (dow as i64 - now.dow() as i64).rem_euclid(7)
            };
            today.add_days(days)
        }
        Some(Day::Date { mo, d }) => {
            let (mut y, mut mo2) = (now.y, mo.unwrap_or(now.mo));
            let mut cand = Dt::new(y, mo2, d, 0, 0);
            if cand.is_none_or(|c| c < today) {
                if mo.is_some() {
                    y += 1;
                } else {
                    mo2 += 1;
                    if mo2 > 12 {
                        mo2 = 1;
                        y += 1;
                    }
                }
                cand = Dt::new(y, mo2, d, 0, 0);
            }
            cand.ok_or(())?
        }
    };

    if ambiguous && repeat == Repeat::None && !explicit_day {
        // 今天的 h 点、h+12 点里最近的将来；都过了就明天（1~5 点按下午算）
        let a = date.at(h, m).add_days(extra);
        let b = date.at(h + 12, m).add_days(extra);
        let t = if a > now {
            a
        } else if b > now {
            b
        } else {
            date.add_days(1 + extra).at(if h < 6 { h + 12 } else { h }, m)
        };
        return Ok(Some((t, repeat)));
    }
    if ambiguous && h < 6 {
        h += 12;
    }
    let mut t = date.at(h, m).add_days(extra);
    if repeat != Repeat::None {
        t = if t <= now { repeat.next_after(t, now).ok_or(())? } else { repeat.first_from(t) };
        return Ok(Some((t, repeat)));
    }
    if t <= now {
        // 过了时间：没指定哪天的顺延到明天，「周X」顺延到下周
        t = match f.day {
            None | Some(Day::Offset(0)) => t.add_days(1),
            Some(Day::Week { next: false, .. }) => t.add_days(7),
            _ => return Err(()),
        };
    }
    Ok(Some((t, repeat)))
}

// ---------------------------------------------------------------- 台词

fn period_name(h: u32) -> &'static str {
    match h {
        0..=5 => "凌晨",
        6..=9 => "早上",
        10..=11 => "上午",
        12 => "中午",
        13..=17 => "下午",
        _ => "晚上",
    }
}

/// 「下午 3 点半」「晚上 10 点」「早上 9 点 15 分」
pub fn hm_phrase(t: Dt) -> String {
    if t.h == 0 && t.mi == 0 {
        return "半夜 12 点".into();
    }
    let h12 = if t.h > 12 { t.h - 12 } else { t.h };
    let m = match t.mi {
        0 => String::new(),
        30 => "半".into(),
        m => format!(" {m} 分"),
    };
    format!("{} {h12} 点{m}", period_name(t.h))
}

/// 「明天早上 9 点」「周三晚上 7 点」「12 月 25 日上午 9 点」
pub fn when_phrase(t: Dt, now: Dt) -> String {
    let days = (t.at(0, 0).to_min() - now.at(0, 0).to_min()) / 1440;
    let day = match days {
        0 => String::new(),
        1 => "明天".into(),
        2 => "后天".into(),
        3..=6 => {
            let next = mon_index(t.dow()) <= mon_index(now.dow());
            format!("{}{}", if next { "下" } else { "" }, weekday_name(t.dow()))
        }
        _ => format!("{} 月 {} 日", t.mo, t.d),
    };
    format!("{day}{}", hm_phrase(t))
}

pub fn rel_phrase(r: i64) -> String {
    match r {
        30 => "半小时后".into(),
        r if r < 60 => format!("{r} 分钟后"),
        r if r % 60 == 0 => format!("{} 小时后", r / 60),
        r if r % 60 == 30 => format!("{} 个半小时后", r / 60),
        r => format!("{} 小时 {} 分钟后", r / 60, r % 60),
    }
}

/// 加好之后她的确认：「好喵，下午 3 点叫你开会」
pub fn confirm_line(d: &Draft, now: Dt) -> String {
    let what = &d.text;
    let Some(t) = d.time else { return format!("好喵，记下了：{what}") };
    if let Some(r) = d.rel_min {
        return format!("好喵，{}叫你{what}", rel_phrase(r));
    }
    match d.repeat {
        Repeat::None => format!("好喵，{}叫你{what}", when_phrase(t, now)),
        Repeat::Daily => format!("好喵，每天{}提醒你{what}", hm_phrase(t)),
        Repeat::Workday => format!("好喵，每个工作日{}提醒你{what}", hm_phrase(t)),
        Repeat::Weekly => format!("好喵，每{}{}提醒你{what}", weekday_name(t.dow()), hm_phrase(t)),
    }
}

fn hm(i: &Item) -> String {
    match i.snooze.as_deref().and_then(Dt::parse).or(i.time_dt()) {
        Some(t) => format!("{:02}:{:02}", t.h, t.mi),
        None => String::new(),
    }
}

fn join_names(v: Vec<String>, max: usize) -> String {
    let n = v.len();
    let mut s = v.into_iter().take(max).collect::<Vec<_>>().join("、");
    if n > max {
        s.push_str(&format!(" 等 {n} 件"));
    }
    s
}

/// 「我有什么待办」的回答
pub fn today_line(timed: &[Item], loose: &[Item]) -> String {
    let t = join_names(timed.iter().map(|i| format!("{} {}", hm(i), i.text)).collect(), 4);
    let l = join_names(loose.iter().map(|i| i.text.clone()).collect(), 3);
    match (timed.is_empty(), loose.is_empty()) {
        (true, true) => "今天没有待办喵，可以安心摸鱼～".into(),
        (false, true) => format!("今天还有：{t}，别忘了喵"),
        (true, false) => format!("今天没有定时的事，随手记里还有：{l}喵"),
        (false, false) => format!("今天还有：{t}；随手记里还有：{l}喵"),
    }
}

// ---------------------------------------------------------------- AI 兜底

pub fn ai_prompt(now: Dt) -> String {
    format!(
        "你是时间解析器。判断用户这句话是不是要你设置提醒或者记一条备忘。只输出一个 JSON 对象，不要任何别的文字：\
{{\"memo\":true或false,\"text\":\"要做的事（简短，不含时间和「提醒我」）\",\"time\":\"YYYY-MM-DD HH:MM\"或null,\"repeat\":\"none\"|\"daily\"|\"workday\"|\"weekly\"}}。\
现在是本地时间 {}（{}）。「等会儿/一会儿」按 10 分钟后算；没说上午下午时取最近的将来时刻；只是记一下不需要提醒时 time 为 null。",
        now.fmt(),
        weekday_name(now.dow())
    )
}

/// AI 返回的 JSON → Draft；不是备忘或者看不懂返回 None
pub fn parse_ai(reply: &str, now: Dt) -> Option<Draft> {
    let s = reply.get(reply.find('{')?..=reply.rfind('}')?)?;
    let v: Value = serde_json::from_str(s).ok()?;
    if v.get("memo").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let text = clean_content(v.get("text").and_then(Value::as_str)?.trim());
    if text.is_empty() {
        return None;
    }
    let repeat = match v.get("repeat").and_then(Value::as_str) {
        Some("daily") => Repeat::Daily,
        Some("workday") => Repeat::Workday,
        Some("weekly") => Repeat::Weekly,
        _ => Repeat::None,
    };
    let mut time = match v.get("time") {
        Some(Value::String(t)) if !t.trim().is_empty() => Some(Dt::parse(t)?),
        _ => None,
    };
    if let Some(t) = time {
        time = Some(if repeat != Repeat::None {
            if t <= now { repeat.next_after(t, now)? } else { repeat.first_from(t) }
        } else if t <= now {
            // 模型偶尔给出今天已经过去的时刻：顺延一天，再不行就不要了
            Some(t.add_days(1)).filter(|t| *t > now)?
        } else {
            t
        });
    }
    let repeat = if time.is_some() { repeat } else { Repeat::None };
    Some(Draft { text, time, repeat, rel_min: None })
}

const NO_KEY_LINE: &str = "这个时间哈基米没听懂喵……换个说法？比如「30 分钟后提醒我喝水」「明天 9 点提醒我交报告」";

/// 本地规则没看懂：后台线程问 AI。是备忘就加上并发 Memo(Reply)，不是就当普通聊天发 ChatReply
pub fn route_ai(tx: &EventSender, text: String, ctx: ChatCtx) {
    let worker_tx = tx.clone();
    let spawned = std::thread::Builder::new().name("hakimi-memo".into()).spawn(move || {
        let now = Now::get().dt;
        if crate::core::config::get().api_key.is_empty() {
            worker_tx.send(AppEvent::Memo(MemoEv::Reply { ok: false, text: NO_KEY_LINE.into() }));
            return;
        }
        let draft = match crate::services::ai::ask_once(&ai_prompt(now), &text) {
            Ok(r) => parse_ai(&r, now),
            Err(e) => {
                crate::core::log::log(&format!("memo ai error {e}"));
                None
            }
        };
        match draft {
            Some(d) => {
                crate::core::memo::add(&d.text, d.time, d.repeat);
                worker_tx.send(AppEvent::Memo(MemoEv::Reply { ok: true, text: confirm_line(&d, now) }));
            }
            None => worker_tx.send(AppEvent::ChatReply(crate::services::ai::chat_blocking(&text, &ctx))),
        }
    });
    if spawned.is_err() {
        tx.send(AppEvent::Memo(MemoEv::Reply { ok: false, text: NO_KEY_LINE.into() }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-11-20 周五 12:41
    fn now() -> Dt { Dt::new(2026, 11, 20, 12, 41).unwrap() }
    fn dt(mo: u32, d: u32, h: u32, mi: u32) -> Dt { Dt::new(2026, mo, d, h, mi).unwrap() }

    fn memo(s: &str) -> Draft {
        match parse(s, now()) {
            Parsed::Memo(d) => d,
            other => panic!("{s} → {other:?}"),
        }
    }

    fn check(s: &str, what: &str, t: Option<Dt>, r: Repeat) {
        let d = memo(s);
        assert_eq!((d.text.as_str(), d.time, d.repeat), (what, t, r), "{s}");
    }

    #[test]
    fn numbers() {
        let n = |s: &str| Scan::new(s).num(0);
        assert_eq!(n("12点"), Some((12, 2)));
        assert_eq!(n("十"), Some((10, 1)));
        assert_eq!(n("十二"), Some((12, 2)));
        assert_eq!(n("二十"), Some((20, 2)));
        assert_eq!(n("二十三"), Some((23, 3)));
        assert_eq!(n("四十五分"), Some((45, 3)));
        assert_eq!(n("两"), Some((2, 1)));
        assert_eq!(n("１５"), Some((15, 2)));
        assert_eq!(n("点"), None);
    }

    #[test]
    fn user_examples() {
        use Repeat::*;
        check("提醒我 3 点开会", "开会", Some(dt(11, 20, 15, 0)), None);
        check("半小时后叫我喝药", "喝药", Some(dt(11, 20, 13, 11)), None);
        check("明天早上 9 点交报告", "交报告", Some(dt(11, 21, 9, 0)), None);
        check("每天 22 点提醒我洗漱", "洗漱", Some(dt(11, 20, 22, 0)), Daily);
        check("记一下：买猫粮", "买猫粮", Option::None, None);
    }

    #[test]
    fn relative_times() {
        use Repeat::None;
        check("10分钟后提醒我关火", "关火", Some(dt(11, 20, 12, 51)), None);
        check("两个小时后叫我收衣服", "收衣服", Some(dt(11, 20, 14, 41)), None);
        check("一个半小时后提醒我出门", "出门", Some(dt(11, 20, 14, 11)), None);
        check("1小时20分钟后提醒我拿外卖", "拿外卖", Some(dt(11, 20, 14, 1)), None);
        check("过20分钟提醒我喝水", "喝水", Some(dt(11, 20, 13, 1)), None);
        check("一刻钟后提醒我看锅", "看锅", Some(dt(11, 20, 12, 56)), None);
        // 跨天
        check("12小时后提醒我续费", "续费", Some(dt(11, 21, 0, 41)), None);
        check("半小时后叫我", DEFAULT_WHAT, Some(dt(11, 20, 13, 11)), None);
    }

    #[test]
    fn clock_times() {
        use Repeat::None;
        check("下午三点半提醒我给妈妈打电话", "给妈妈打电话", Some(dt(11, 20, 15, 30)), None);
        check("提醒我晚上8点看直播", "看直播", Some(dt(11, 20, 20, 0)), None);
        check("中午1点提醒我午休", "午休", Some(dt(11, 20, 13, 0)), None);
        check("提醒我九点一刻开会", "开会", Some(dt(11, 20, 21, 15)), None);
        check("八点四十五提醒我出门", "出门", Some(dt(11, 20, 20, 45)), None);
        check("3点开会", "开会", Some(dt(11, 20, 15, 0)), None);
        check("提醒我15:30去取快递", "去取快递", Some(dt(11, 20, 15, 30)), None);
    }

    #[test]
    fn rolls_over_to_tomorrow() {
        use Repeat::None;
        // 已经过了的时刻顺延到明天
        check("提醒我上午10点开会", "开会", Some(dt(11, 21, 10, 0)), None);
        check("今天早上8点提醒我晨跑", "晨跑", Some(dt(11, 21, 8, 0)), None);
        check("提醒我12:30吃饭", "吃饭", Some(dt(11, 21, 12, 30)), None);
        check("今晚12点提醒我睡觉", "睡觉", Some(dt(11, 21, 0, 0)), None);
        check("凌晨2点提醒我看球", "看球", Some(dt(11, 21, 2, 0)), None);
        check("晚上1点提醒我关电脑", "关电脑", Some(dt(11, 21, 1, 0)), None);
    }

    #[test]
    fn days_and_weekdays() {
        use Repeat::None;
        check("明晚九点提醒我倒垃圾", "倒垃圾", Some(dt(11, 21, 21, 0)), None);
        check("后天下午2点提醒我去医院", "去医院", Some(dt(11, 22, 14, 0)), None);
        check("大后天提醒我还书", "还书", Some(dt(11, 23, 9, 0)), None);
        check("明天提醒我交报告", "交报告", Some(dt(11, 21, 9, 0)), None);
        check("周一早上9点提醒我开周会", "开周会", Some(dt(11, 23, 9, 0)), None);
        check("下周五提醒我交房租", "交房租", Some(dt(11, 27, 9, 0)), None);
        check("周五晚上8点提醒我看电影", "看电影", Some(dt(11, 20, 20, 0)), None);
        check("周五上午10点提醒我开会", "开会", Some(dt(11, 27, 10, 0)), None);
        check("星期天下午提醒我打扫卫生", "打扫卫生", Some(dt(11, 22, 15, 0)), None);
        check("12月25日提醒我买礼物", "买礼物", Some(dt(12, 25, 9, 0)), None);
        check("3号提醒我还信用卡", "还信用卡", Some(dt(12, 3, 9, 0)), None);
        check("提醒我明天下午开会", "开会", Some(dt(11, 21, 15, 0)), None);
        check("明天3点提醒我取件", "取件", Some(dt(11, 21, 15, 0)), None);
        check("记一下明天10点体检", "体检", Some(dt(11, 21, 10, 0)), None);
    }

    #[test]
    fn repeats() {
        use Repeat::*;
        check("每天早上7点半叫我起床", "起床", Some(dt(11, 21, 7, 30)), Daily);
        check("工作日早上9点提醒我打卡", "打卡", Some(dt(11, 23, 9, 0)), Workday);
        check("每个工作日下午6点提醒我下班", "下班", Some(dt(11, 20, 18, 0)), Workday);
        check("每周三晚上7点提醒我上瑜伽课", "上瑜伽课", Some(dt(11, 25, 19, 0)), Weekly);
        check("每晚11点提醒我睡觉", "睡觉", Some(dt(11, 20, 23, 0)), Daily);
        check("每周五提醒我写周报", "写周报", Some(dt(11, 27, 9, 0)), Weekly);
    }

    #[test]
    fn not_memos_and_queries() {
        assert_eq!(parse("我有什么待办", now()), Parsed::Query);
        assert_eq!(parse("今天有什么事要做", now()), Parsed::Query);
        assert_eq!(parse("备忘录", now()), Parsed::Query);
        for s in ["今天天气怎么样", "你好呀", "我有点困", "一点点就好", "明天去公园玩吧", "现在几点了", "我记得你喜欢小鱼干", ""] {
            assert_eq!(parse(s, now()), Parsed::NotMemo, "{s}");
        }
        for s in ["等会儿提醒我喝水", "提醒我喝水", "你能提醒我3点开会吗"] {
            assert_eq!(parse(s, now()), Parsed::Unsure, "{s}");
        }
    }

    #[test]
    fn confirm_lines() {
        let c = |s: &str| confirm_line(&memo(s), now());
        assert_eq!(c("提醒我 3 点开会"), "好喵，下午 3 点叫你开会");
        assert_eq!(c("半小时后叫我喝药"), "好喵，半小时后叫你喝药");
        assert_eq!(c("明天早上 9 点交报告"), "好喵，明天早上 9 点叫你交报告");
        assert_eq!(c("每天 22 点提醒我洗漱"), "好喵，每天晚上 10 点提醒你洗漱");
        assert_eq!(c("记一下：买猫粮"), "好喵，记下了：买猫粮");
        assert_eq!(c("工作日早上9点提醒我打卡"), "好喵，每个工作日早上 9 点提醒你打卡");
        assert_eq!(c("每周三晚上7点提醒我上瑜伽课"), "好喵，每周三晚上 7 点提醒你上瑜伽课");
        assert_eq!(c("下周五提醒我交房租"), "好喵，11 月 27 日早上 9 点叫你交房租");
        assert_eq!(c("周一早上9点提醒我开周会"), "好喵，下周一早上 9 点叫你开周会");
        assert_eq!(c("一个半小时后提醒我出门"), "好喵，1 个半小时后叫你出门");
        assert_eq!(c("提醒我九点一刻开会"), "好喵，晚上 9 点 15 分叫你开会");
    }

    #[test]
    fn today_answer() {
        let mut b = crate::core::memo::Book::default();
        assert_eq!(today_line(&[], &[]), "今天没有待办喵，可以安心摸鱼～");
        b.add("开会", Some(dt(11, 20, 15, 0)), Repeat::None, now());
        b.add("洗漱", Some(dt(11, 20, 22, 0)), Repeat::Daily, now());
        b.add("买猫粮", None, Repeat::None, now());
        let (t, l) = b.today(now());
        assert_eq!(today_line(&t, &l), "今天还有：15:00 开会、22:00 洗漱；随手记里还有：买猫粮喵");
        assert_eq!(today_line(&t, &[]), "今天还有：15:00 开会、22:00 洗漱，别忘了喵");
    }

    #[test]
    fn ai_json() {
        let d = parse_ai("```json\n{\"memo\":true,\"text\":\"喝水\",\"time\":\"2026-11-20 12:51\",\"repeat\":\"none\"}\n```", now()).unwrap();
        assert_eq!((d.text.as_str(), d.time, d.repeat), ("喝水", Some(dt(11, 20, 12, 51)), Repeat::None));
        let d = parse_ai(r#"{"memo":true,"text":"买猫粮","time":null,"repeat":"daily"}"#, now()).unwrap();
        assert_eq!((d.time, d.repeat), (None, Repeat::None));
        // 已经过了的时刻顺延一天
        let d = parse_ai(r#"{"memo":true,"text":"开会","time":"2026-11-20 10:00","repeat":"none"}"#, now()).unwrap();
        assert_eq!(d.time, Some(dt(11, 21, 10, 0)));
        let d = parse_ai(r#"{"memo":true,"text":"打卡","time":"2026-11-21 09:00","repeat":"workday"}"#, now()).unwrap();
        assert_eq!(d.time, Some(dt(11, 23, 9, 0)));
        assert!(parse_ai(r#"{"memo":false}"#, now()).is_none());
        assert!(parse_ai("不是 JSON", now()).is_none());
        assert!(parse_ai(r#"{"memo":true,"text":"x","time":"明天"}"#, now()).is_none());
        assert!(ai_prompt(now()).contains("2026-11-20 12:41（周五）"));
    }
}
