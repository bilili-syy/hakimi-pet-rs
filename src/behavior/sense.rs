//! 场景互动（v1.1）：平台层 platform/sense.rs 报来的「用户走开/回来、打字很快、鼠标乱晃、全屏、在放音乐」，
//! 以及节日 / 主人生日的问候。只管她怎么反应；忙着的时候（聊天、提醒在响、被拎着、跳舞）一律不打扰。
use super::fx::FxKind;
use super::{Behavior, Shot, St, lines, pick};
use crate::types::SenseEv;

#[derive(Debug, Default)]
pub struct SenseUi {
    /// 全屏程序在前台：安静模式（不念、不主动说话）
    pub quiet: bool,
    quiet_since: u64,
    /// 后台在放音乐：跟着点头
    bob: bool,
    next_bob: u64,
    bob_n: u32,
    music_said: u64,
    /// 今天已经说过的节日（日期）
    festive_day: String,
    /// 测试用「今天」 YYYY-MM-DD
    pub test_today: Option<String>,
    pub test_birthday: Option<String>,
}

/// 节日（公历 + 2026–2028 的农历节日日期表）。返回要说的话
pub fn festival(date: &str) -> Option<&'static [&'static str]> {
    let md = date.get(5..)?;
    Some(match md {
        "01-01" => lines::NEW_YEAR,
        "02-14" => lines::VALENTINE,
        "10-31" => lines::HALLOWEEN,
        "12-24" | "12-25" => lines::XMAS,
        "12-31" => lines::YEAR_END,
        _ => match date {
            "2026-02-17" | "2027-02-06" | "2028-01-26" => lines::SPRING,
            "2026-03-03" | "2027-02-20" | "2028-02-09" => lines::LANTERN,
            "2026-06-19" | "2027-06-09" | "2028-05-28" => lines::DRAGON_BOAT,
            "2026-08-19" | "2027-08-08" | "2028-08-26" => lines::QIXI,
            "2026-09-25" | "2027-09-15" | "2028-10-03" => lines::MID_AUTUMN,
            _ => return None,
        },
    })
}

/// "4-3" / "04-03" / "4月3日" → "04-03"
pub fn norm_birthday(s: &str) -> Option<String> {
    let nums: Vec<u32> = s.split(|c: char| !c.is_ascii_digit()).filter(|x| !x.is_empty()).filter_map(|x| x.parse().ok()).collect();
    let (m, d) = match nums.as_slice() {
        [m, d] => (*m, *d),
        [_, m, d] => (*m, *d),
        _ => return None,
    };
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then(|| format!("{m:02}-{d:02}"))
}

fn today_local() -> String {
    #[cfg(windows)]
    {
        let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay)
    }
    #[cfg(not(windows))]
    String::from("2026-01-01")
}

impl Behavior {
    fn sense_busy(&self) -> bool {
        self.chatting || self.waiting || self.dragging || self.routine.is_some() || self.memo_ringing()
            || matches!(self.state, St::Drag | St::Fall | St::Dance | St::Eat)
    }

    fn sense_today(&self) -> String {
        if let Some(t) = &self.sense.test_today { return t.clone(); }
        if self.io { today_local() } else { String::new() }
    }

    pub(super) fn on_sense(&mut self, ev: SenseEv) {
        self.call(format!("sense:{ev:?}"));
        match ev {
            SenseEv::Fullscreen(on) => self.sense_fullscreen(on),
            SenseEv::Music(on) => {
                self.sense.bob = on;
                self.sense.bob_n = 0;
                if on && !self.sense.quiet && !self.sense_busy() && self.now >= self.sense.music_said {
                    self.sense.music_said = self.now + 20 * 60_000;
                    self.fx(FxKind::Note, 3);
                    self.react(St::Happy, 2200, Some(pick(lines::MUSIC)));
                }
            }
            _ if self.sense.quiet || self.sense_busy() => {}
            SenseEv::UserIdle(1) => {
                if matches!(self.state, St::Idle | St::Walk | St::Sit) {
                    self.fx(FxKind::Note, 1);
                    self.say(&pick(lines::ALONE), 2800);
                    self.react(St::Yawn, 2200, None);
                }
            }
            SenseEv::UserIdle(_) => {
                if matches!(self.state, St::Idle | St::Walk | St::Sit | St::Yawn) {
                    self.set_state(St::Sleep, None);
                }
            }
            SenseEv::UserBack { secs } => {
                if self.state == St::Sleep { self.set_state(St::Idle, None); }
                self.play(Shot::Jump);
                self.fx(FxKind::Heart, 3);
                let line = if secs >= 1800 { pick(lines::WELCOME) } else { pick(&lines::WELCOME[..2]) };
                self.react(St::Starry, 2600, Some(line));
            }
            SenseEv::Typing => {
                if matches!(self.state, St::Idle | St::Walk | St::Sit) {
                    self.play(Shot::Bounce);
                    self.fx(FxKind::Spark, 6);
                    self.react(St::Happy, 2400, Some(pick(lines::TYPING)));
                }
            }
            SenseEv::CursorShake => {
                if matches!(self.state, St::Idle | St::Walk | St::Sit | St::Think | St::Stare) {
                    self.play(Shot::Jump);
                    self.fx(FxKind::Exclaim, 1);
                    self.react(St::Starry, 1800, Some(pick(lines::POUNCE)));
                }
            }
        }
    }

    fn sense_fullscreen(&mut self, on: bool) {
        if on == self.sense.quiet { return; }
        self.sense.quiet = on;
        if on {
            self.sense.quiet_since = self.now;
            self.stop_voice();
            if matches!(self.state, St::Idle | St::Walk) { self.set_state(St::Sit, None); }
        } else if !self.flush_off_work() && self.now.saturating_sub(self.sense.quiet_since) >= 5 * 60_000 && !self.sense_busy() {
            self.react(St::Smug, 2400, Some(pick(lines::FS_OFF)));
        }
    }

    /// 安静模式下只念提醒
    pub(super) fn sense_mute(&self, token: u64) -> bool { self.sense.quiet && !self.memo_token(token) }

    /// 每帧：放音乐时跟着点头（每 0.7 秒一下，偶尔冒音符）
    pub(super) fn sense_frame(&mut self) {
        if !self.sense.bob || self.sense.quiet || self.state != St::Idle || self.sense_busy() || self.now < self.sense.next_bob {
            return;
        }
        self.sense.next_bob = self.now + 700;
        self.sense.bob_n += 1;
        self.play(if self.sense.bob_n.is_multiple_of(2) { Shot::Bounce } else { Shot::Squash });
        if self.sense.bob_n.is_multiple_of(9) { self.fx(FxKind::Note, 1); }
    }

    /// 开机问候时：今天是节日 / 主人生日就先说这个（一天一次）
    pub(super) fn festive_line(&mut self) -> Option<String> {
        let today = self.sense_today();
        if today.len() < 10 || self.sense.festive_day == today { return None; }
        let bday = match &self.sense.test_birthday {
            Some(b) => Some(b.clone()),
            None if self.io => norm_birthday(&crate::core::config::get().birthday),
            None => None,
        };
        let line = if bday.as_deref() == today.get(5..) { Some(pick(lines::BIRTHDAY)) } else { festival(&today).map(pick) };
        if line.is_some() { self.sense.festive_day = today; }
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stats::Stats;
    use crate::types::{AppEvent, EventSender, WinCmd};

    fn mk() -> Behavior {
        fastrand::seed(7);
        let (s, _) = Stats::load(None);
        Behavior::with(EventSender::new(|_| {}), s, false, 0.0, String::new(), Vec::new(), false)
    }
    fn ev(b: &mut Behavior, e: AppEvent, now: u64) -> Vec<WinCmd> {
        let mut out = Vec::new();
        b.event_at(e, now, &mut out);
        out
    }
    fn tick(b: &mut Behavior, now: u64) {
        let mut out = Vec::new();
        b.frame_at(now, &mut out);
    }

    #[test]
    fn dates() {
        assert!(festival("2027-02-06").is_some());
        assert!(festival("2026-12-25").is_some());
        assert!(festival("2026-11-20").is_none());
        assert_eq!(norm_birthday("4月3日").as_deref(), Some("04-03"));
        assert_eq!(norm_birthday("1995-04-03").as_deref(), Some("04-03"));
        assert_eq!(norm_birthday("13-01"), None);
        assert_eq!(norm_birthday(""), None);
    }

    #[test]
    fn reactions() {
        let mut b = mk();
        tick(&mut b, 3000);
        b.set_state(St::Idle, None);
        ev(&mut b, AppEvent::Sense(SenseEv::UserBack { secs: 900 }), 5000);
        assert_eq!(b.state, St::Starry);
        b.set_state(St::Idle, None);
        ev(&mut b, AppEvent::Sense(SenseEv::Fullscreen(true)), 9000);
        assert!(b.sense.quiet);
        ev(&mut b, AppEvent::Sense(SenseEv::CursorShake), 9500);
        assert_ne!(b.state, St::Starry, "全屏时不打扰");
        ev(&mut b, AppEvent::Sense(SenseEv::Fullscreen(false)), 9000 + 6 * 60_000);
        assert_eq!(b.state, St::Smug);
        b.set_state(St::Idle, None);
        ev(&mut b, AppEvent::Sense(SenseEv::UserIdle(2)), 9000 + 7 * 60_000);
        assert_eq!(b.state, St::Sleep);
    }

    #[test]
    fn birthday_greets_once() {
        let mut b = mk();
        b.sense.test_today = Some("2027-04-03".into());
        b.sense.test_birthday = Some("04-03".into());
        assert!(b.festive_line().is_some_and(|l| lines::BIRTHDAY.contains(&l.as_str())));
        assert!(b.festive_line().is_none());
        b.sense.test_today = Some("2027-02-06".into());
        assert!(b.festive_line().is_some_and(|l| lines::SPRING.contains(&l.as_str())));
    }
}
