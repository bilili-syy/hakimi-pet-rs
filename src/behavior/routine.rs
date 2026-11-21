//! 定时提醒、番茄钟、摸鱼监督的反应
use super::*;

impl Behavior {
    // ============================================================ 提醒 / 番茄钟 / 摸鱼
    pub(super) fn on_remind(&mut self, k: RemindKind) {
        if self.chatting || self.waiting || self.is_busy_for_interruption() || self.sense.quiet {
            // 喝水/站立丢了无妨；下班提醒一天只有一次，全屏开会或忙碌时推迟到空闲/退全屏后再补说
            if k == RemindKind::OffWork {
                self.pending_off_work = Some(self.now + 2 * 3_600_000);
                if !self.laters.iter().any(|l| l.1 == Later::OffWorkRetry) {
                    self.later(5_000, Later::OffWorkRetry);
                }
            }
            return;
        }
        if self.state == St::Sleep && k != RemindKind::OffWork { return; }
        if k == RemindKind::OffWork {
            self.pending_off_work = None;
            self.laters.retain(|l| l.1 != Later::OffWorkRetry);
        }
        self.play(Shot::Bounce);
        let l = match k { RemindKind::Water => lines::WATER, RemindKind::StandUp => lines::STAND_UP, RemindKind::OffWork => lines::OFF_WORK };
        self.react(St::Happy, 2500, Some(pick(l)));
    }

    /// 检查是否有推迟的下班提醒可以补说；若刚刚说出返回 true
    pub(super) fn flush_off_work(&mut self) -> bool {
        let Some(deadline) = self.pending_off_work else { return false };
        if self.now > deadline {
            self.pending_off_work = None;
            self.laters.retain(|l| l.1 != Later::OffWorkRetry);
            return false;
        }
        if self.chatting || self.waiting || self.is_busy_for_interruption() || self.sense.quiet {
            if !self.laters.iter().any(|l| l.1 == Later::OffWorkRetry) {
                self.later(5_000, Later::OffWorkRetry);
            }
            return false;
        }
        self.pending_off_work = None;
        self.laters.retain(|l| l.1 != Later::OffWorkRetry);
        self.play(Shot::Bounce);
        self.react(St::Happy, 2500, Some(pick(lines::OFF_WORK)));
        true
    }

    pub(super) fn on_pomodoro(&mut self, p: PomodoroEv) {
        match p {
            PomodoroEv::Start { minutes } => {
                self.pomo_running = true;
                self.react(St::Think, 2500, Some(pickv(lines::pomo_start(minutes))));
            }
            PomodoroEv::Done { .. } => {
                self.pomo_running = false;
                self.play(Shot::Jump);
                self.fx(FxKind::Spark, 7);
                if fastrand::bool() {
                    self.react(St::Smug, 3500, Some(pick(lines::SMUG)));
                } else {
                    self.react(St::Cheer, 3500, Some(pick(lines::POMO_DONE)));
                }
                self.act(Act::Pat);
            }
            PomodoroEv::Cancel => {
                self.pomo_running = false;
                self.say(&pick(lines::POMO_CANCEL), 2600);
            }
        }
    }

    pub(super) fn on_moyu(&mut self, ev: MoyuEv) {
        if self.chatting || self.waiting || self.is_busy_for_interruption() || self.sense.quiet { return; }
        match ev {
            MoyuEv::Back { .. } => {
                self.play(Shot::Bounce);
                self.act(Act::Pat);
                self.react(if fastrand::bool() { St::Smug } else { St::Happy }, 3000, Some(pick(lines::MOYU_BACK)));
            }
            MoyuEv::Nag { level, minutes, what, pomodoro } => {
                let what = lines::moyu_name(&what);
                if self.state == St::Sleep { self.set_state(St::Idle, None); }
                if pomodoro {
                    self.play(Shot::Shake);
                    return self.react(St::Drag, 3000, Some(pickv(lines::moyu_pomo(&what))));
                }
                match level {
                    1 => {
                        self.play(Shot::Bounce);
                        self.fx(FxKind::Sweat, 1);
                        self.react(St::Stare, 3500, Some(pickv(lines::moyu1(&what, minutes))));
                    }
                    2 => {
                        self.play(Shot::Shake);
                        self.react(St::Pout, 3500, Some(pickv(lines::moyu2(&what, minutes))));
                    }
                    _ => {
                        self.play(Shot::Shake);
                        self.fx(FxKind::Anger, 1);
                        self.react(St::Hiss, 4000, Some(pickv(lines::moyu3(&what, minutes))));
                    }
                }
            }
        }
    }
}
