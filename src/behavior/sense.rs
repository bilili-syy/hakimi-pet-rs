//! 场景互动（v1.1）：平台层 platform/sense.rs 报来的「用户走开/回来、打字很快、鼠标乱晃、全屏、在放音乐」，
//! 以及节日 / 主人生日的问候。只管她怎么反应；忙着的时候（聊天、提醒在响、被拎着、跳舞）一律不打扰。
use super::fx::FxKind;
use super::{Behavior, Shot, St, lines, pick};
use crate::types::SenseEv;

/// 安静模式下，主人点她之后多久内的回应照样出声
const TOUCH_VOICE_MS: u64 = 2500;

#[derive(Debug, Default)]
pub struct SenseUi {
    /// 全屏程序在前台：安静模式（不念、不主动说话）
    pub quiet: bool,
    quiet_since: u64,
    /// 主人最近一次直接找她（点、摸、滚轮、右键菜单、发聊天）的时刻：安静模式下这之后一小会儿照样出声
    pub touched: u64,
    /// 后台在放音乐：跟着点头
    bob: bool,
    next_bob: u64,
    bob_n: u32,
    music_said: u64,
    /// 正在朝晃动的鼠标猛扑过去（跑到位后星星眼收尾）
    pub pouncing: bool,
    /// 从窗口顶上跳下来落地后接着扑鼠标
    pub pounce_after_land: bool,
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
        self.chatting
            || self.waiting
            || self.dragging
            || self.routine.is_some()
            || self.memo_ringing()
            || self.play.game.is_some()
            || self.play.food.is_some()
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
                    self.play(Shot::Jump);
                    self.fx(FxKind::Spark, 7);
                    self.react(St::Cheer, 2400, Some(pick(lines::TYPING)));
                }
            }
            SenseEv::CursorShake => {
                if matches!(self.state, St::Idle | St::Walk | St::Sit | St::Think | St::Stare) {
                    self.pounce_cursor();
                }
            }
        }
    }

    /// 晃鼠标：朝光标所在的水平方向小跑扑过去（如果正坐在窗口顶上，先跳下来再扑）
    pub(super) fn pounce_cursor(&mut self) {
        if self.play.perch == Some(crate::types::PerchKind::WindowTop) {
            self.sense.pounce_after_land = true;
            self.fx(FxKind::Exclaim, 1);
            self.say(&pick(lines::POUNCE), 2600);
            return self.jump_down();
        }
        let dx = self
            .play
            .cursor
            .map(|(x, _, _)| x - 120.0)
            .unwrap_or_else(|| if fastrand::bool() { 260.0 } else { -260.0 });
        if dx.abs() < 35.0 {
            self.play(Shot::Jump);
            self.fx(FxKind::Exclaim, 1);
            self.react(St::Starry, 1800, Some(pick(lines::POUNCE)));
            return;
        }
        self.walk_dir = if dx >= 0.0 { 1 } else { -1 };
        self.set_state(St::Walk, None);
        self.sense.pouncing = true;
        self.fx(FxKind::Exclaim, 1);
        self.say(&pick(lines::POUNCE), 2600);
        let dist = dx.abs().clamp(90.0, 1400.0) * self.scale;
        self.cmds.push( crate::types::WinCmd::Walk {
            dir: self.walk_dir,
            distance: dist,
            speed: 5.5 * self.scale,
        });
    }

    fn sense_fullscreen(&mut self, on: bool) {
        if on == self.sense.quiet { return; }
        self.sense.quiet = on;
        if on {
            self.sense.quiet_since = self.now;
            // 进全屏安静（默认还会隐藏窗口）时停掉跳舞、外放、语音队列和置顶玩具，免得在后台出声或 60~120fps 跟拍空转
            self.cleanup_for_hide();
            if matches!(self.state, St::Idle | St::Walk) { self.set_state(St::Sit, None); }
        } else if !self.flush_off_work() && self.now.saturating_sub(self.sense.quiet_since) >= 5 * 60_000 && !self.sense_busy() {
            self.react(St::Smug, 2400, Some(pick(lines::FS_OFF)));
        }
    }

    /// 安静模式（全屏程序在前台，比如游戏）只挡她自己冒出来的话：提醒照念；
    /// 主人刚点了她 / 用了菜单 / 聊天回复到了，回应照念——是主人自己找的，不算打扰
    pub(super) fn sense_mute(&self, token: u64) -> bool {
        if !self.sense.quiet || self.memo_token(token) { return false; }
        !(self.sense.touched > 0 && self.now.saturating_sub(self.sense.touched) <= TOUCH_VOICE_MS)
    }

    /// 记一下「主人直接找她了」
    pub(super) fn sense_touch(&mut self) { self.sense.touched = self.now.max(1); }

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

    /// 开机问候时：今天是节日 / 主人生日就先说这个（一天一次；直播隐私保护开启且正在推流时暂缓喊出真实生日）
    pub(super) fn festive_line(&mut self) -> Option<String> {
        let today = self.sense_today();
        if today.len() < 10 || self.sense.festive_day == today { return None; }
        let bday = match &self.sense.test_birthday {
            Some(b) => Some(b.clone()),
            None if self.io => norm_birthday(&crate::core::config::get().birthday),
            None => None,
        };
        if bday.as_deref() == today.get(5..) {
            if self.stream_privacy_active() {
                // 直播中不报真实生日；若有公共节日可正常说节日，否则留到下播后再祝贺生日
                let pub_fest = festival(&today).map(pick);
                if pub_fest.is_some() {
                    self.sense.festive_day = today;
                }
                return pub_fest;
            }
            self.sense.festive_day = today;
            return Some(pick(lines::BIRTHDAY));
        }
        let line = festival(&today).map(pick);
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
    fn quiet_mode_still_answers_when_touched() {
        use crate::types::{Button, MouseEv};
        let mut b = mk();
        tick(&mut b, 3000);
        ev(&mut b, AppEvent::Sense(SenseEv::Fullscreen(true)), 4000);
        // 自己冒出来的话：不念
        b.say("自言自语喵", 2000);
        assert!(!b.calls.iter().any(|c| c == "speak:自言自语喵"));
        // 主人点了她：回应照念
        ev(&mut b, AppEvent::Mouse(MouseEv::Down { button: Button::Left, x: 120.0, y: 250.0, sx: 500, sy: 500 }), 5000);
        b.say("被点到了喵", 2000);
        assert!(b.calls.iter().any(|c| c == "speak:被点到了喵"));
        // 过了一会儿又是自己说话：还是不念
        tick(&mut b, 9000);
        b.say("又自言自语喵", 2000);
        assert!(!b.calls.iter().any(|c| c == "speak:又自言自语喵"));
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

    #[test]
    fn cursor_shake_sprints_toward_cursor() {
        let mut b = mk();
        tick(&mut b, 3000);
        b.set_state(St::Idle, None);
        ev(&mut b, AppEvent::Cursor { x: 520.0, y: 200.0 }, 4000);
        let out = ev(&mut b, AppEvent::Sense(SenseEv::CursorShake), 4010);
        assert_eq!(b.state, St::Walk);
        assert!(b.sense.pouncing);
        let walk = out.iter().find_map(|c| match c {
            WinCmd::Walk { dir, distance, speed } => Some((*dir, *distance, *speed)),
            _ => None,
        });
        let (dir, dist, speed) = walk.expect("should emit Walk toward cursor");
        assert_eq!(dir, 1);
        assert!((dist - 400.0).abs() < 1.0);
        assert!(speed > 5.0);
        // 扑到鼠标位置后，蹦一下切星星眼
        ev(&mut b, AppEvent::WalkDone { hit_edge: false }, 4800);
        assert_eq!(b.state, St::Starry);
        assert!(!b.sense.pouncing);
    }
}
