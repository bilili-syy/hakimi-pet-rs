//! 提醒（喝水/站立/下班）和番茄钟的时间逻辑。纯计算，便于测试；由 UI 线程的定时器驱动。
use crate::core::config::Reminders as RemCfg;
use crate::types::RemindKind;

const MIN: u64 = 60_000;
/// 下班提醒的检查间隔（原版 setInterval 20s）
const OFF_CHECK: u64 = 20_000;

/// 本地时间（GetLocalTime）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTime {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    /// 0 = 周日
    pub dow: u16,
    pub hour: u16,
    pub minute: u16,
}

impl LocalTime {
    pub fn day_key(&self) -> (u16, u16, u16) {
        (self.year, self.month, self.day)
    }
    #[cfg(windows)]
    pub fn now() -> Self {
        let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        Self { year: t.wYear, month: t.wMonth, day: t.wDay, dow: t.wDayOfWeek, hour: t.wHour, minute: t.wMinute }
    }
}

/// 解析 "18:00"（原版正则 ^\d{1,2}:\d{2}$）
pub fn parse_hm(s: &str) -> Option<(u16, u16)> {
    let (h, m) = s.split_once(':')?;
    let ok = |x: &str, lo: usize, hi: usize| (lo..=hi).contains(&x.len()) && x.bytes().all(|b| b.is_ascii_digit());
    if !ok(h, 1, 2) || !ok(m, 2, 2) {
        return None;
    }
    Some((h.parse().ok()?, m.parse().ok()?))
}

#[derive(Debug, Default)]
pub struct Reminders {
    water_every: u64,
    water_next: Option<u64>,
    stand_every: u64,
    stand_next: Option<u64>,
    off: Option<(u16, u16)>,
    off_next: u64,
    off_fired_on: Option<(u16, u16, u16)>,
}

impl Reminders {
    /// 按配置（重新）排定。now = 毫秒
    pub fn schedule(c: &RemCfg, now: u64) -> Self {
        let water_every = c.water as u64 * MIN;
        let stand_every = c.stand_up as u64 * MIN;
        let off = parse_hm(&c.off_work);
        Self {
            water_every,
            water_next: (c.water > 0).then_some(now + water_every),
            stand_every,
            // 第一次多等 7 分钟，避免和喝水提醒撞在一起
            stand_next: (c.stand_up > 0).then_some(now + stand_every + 7 * MIN),
            off,
            off_next: now + OFF_CHECK,
            off_fired_on: None,
        }
    }

    /// 重新载入配置：保留今天已响过的下班提醒日期，避免同一分钟内重发
    pub fn reschedule(&mut self, c: &RemCfg, now: u64) {
        let fired = self.off_fired_on;
        *self = Self::schedule(c, now);
        self.off_fired_on = fired;
    }

    pub fn next_deadline(&self) -> Option<u64> {
        [self.water_next, self.stand_next, self.off.map(|_| self.off_next)].into_iter().flatten().min()
    }

    /// 到点的提醒
    pub fn poll(&mut self, now: u64, lt: LocalTime) -> Vec<RemindKind> {
        let mut out = vec![];
        if let Some(t) = self.water_next
            && now >= t
        {
            out.push(RemindKind::Water);
            self.water_next = Some(next_after(t, self.water_every, now));
        }
        if let Some(t) = self.stand_next
            && now >= t
        {
            out.push(RemindKind::StandUp);
            self.stand_next = Some(next_after(t, self.stand_every, now));
        }
        if let Some((h, m)) = self.off
            && now >= self.off_next
        {
            self.off_next = next_after(self.off_next, OFF_CHECK, now);
            let weekday = (1..=5).contains(&lt.dow);
            if weekday && self.off_fired_on != Some(lt.day_key()) && lt.hour == h && lt.minute == m {
                self.off_fired_on = Some(lt.day_key());
                out.push(RemindKind::OffWork);
            }
        }
        out
    }
}

/// 周期任务的下一次时间（睡眠/卡顿后不补发一串，直接跳到将来）
fn next_after(t: u64, every: u64, now: u64) -> u64 {
    let every = every.max(1);
    let mut n = t + every;
    if n <= now {
        n += (now - n) / every * every + every;
    }
    n
}

#[derive(Clone, Copy, Debug)]
pub struct Pomodoro {
    pub end: u64,
    pub minutes: u32,
}

impl Pomodoro {
    pub fn start(minutes: u32, now: u64) -> Self {
        Self { end: now + minutes as u64 * MIN, minutes }
    }
}

/// 剩余分钟（原版 Math.max(0, Math.round((end - now) / 60000))）
pub fn minutes_left(end: u64, now: u64) -> u32 {
    (end.saturating_sub(now) as f64 / MIN as f64).round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lt(dow: u16, h: u16, m: u16) -> LocalTime {
        LocalTime { year: 2026, month: 11, day: 20, dow, hour: h, minute: m }
    }

    #[test]
    fn hm() {
        assert_eq!(parse_hm("18:00"), Some((18, 0)));
        assert_eq!(parse_hm("9:05"), Some((9, 5)));
        assert_eq!(parse_hm(""), None);
        assert_eq!(parse_hm("18:0"), None);
        assert_eq!(parse_hm("118:00"), None);
        assert_eq!(parse_hm("a:00"), None);
    }

    #[test]
    fn water_and_stand() {
        let c = RemCfg::default(); // 45 / 60 / 18:00
        let mut r = Reminders::schedule(&c, 0);
        assert!(r.poll(44 * MIN, lt(5, 10, 0)).is_empty());
        assert_eq!(r.poll(45 * MIN, lt(5, 10, 0)), vec![RemindKind::Water]);
        assert!(r.poll(66 * MIN, lt(5, 10, 0)).is_empty());
        assert_eq!(r.poll(67 * MIN, lt(5, 10, 0)), vec![RemindKind::StandUp]);
        assert_eq!(r.poll(90 * MIN, lt(5, 10, 0)), vec![RemindKind::Water]);
        assert_eq!(r.poll(127 * MIN, lt(5, 10, 0)), vec![RemindKind::StandUp]);
    }

    #[test]
    fn zero_disables() {
        let c = RemCfg { water: 0, stand_up: 0, off_work: String::new(), ..Default::default() };
        let r = Reminders::schedule(&c, 0);
        assert_eq!(r.next_deadline(), None);
    }

    #[test]
    fn off_work_once_per_weekday() {
        let c = RemCfg { water: 0, stand_up: 0, off_work: "18:00".into(), ..Default::default() };
        let mut r = Reminders::schedule(&c, 0);
        assert!(r.poll(20_000, lt(5, 17, 59)).is_empty());
        assert_eq!(r.poll(40_000, lt(5, 18, 0)), vec![RemindKind::OffWork]);
        assert!(r.poll(60_000, lt(5, 18, 0)).is_empty());
        // 周六不提醒
        let mut r = Reminders::schedule(&c, 0);
        assert!(r.poll(20_000, lt(6, 18, 0)).is_empty());
    }

    #[test]
    fn no_burst_after_sleep() {
        let c = RemCfg { water: 45, stand_up: 0, off_work: String::new(), ..Default::default() };
        let mut r = Reminders::schedule(&c, 0);
        assert_eq!(r.poll(1000 * MIN, lt(1, 1, 0)), vec![RemindKind::Water]);
        assert!(r.poll(1001 * MIN, lt(1, 1, 0)).is_empty());
        assert!(r.next_deadline().unwrap() > 1000 * MIN);
    }

    #[test]
    fn pomo_left() {
        let p = Pomodoro::start(25, 0);
        assert_eq!(minutes_left(p.end, 0), 25);
        assert_eq!(minutes_left(p.end, 24 * MIN + 29_000), 1);
        assert_eq!(minutes_left(p.end, 30 * MIN), 0);
    }
}
