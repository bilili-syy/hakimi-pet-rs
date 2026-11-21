//! 鼠标互动：点哪里、拖、甩、撸、戳、摔下来
use super::*;

impl Behavior {
    // ============================================================ 鼠标
    pub(super) fn zone(&self, x: f32, y: f32) -> Zone {
        if self.ui.status.is_some_and(|r| r.contains(x, y)) { return Zone::Status; }
        if self.ui.bubble.is_some_and(|r| r.contains(x, y)) { return Zone::Bubble; }
        Zone::Pet
    }

    pub(super) fn on_mouse(&mut self, m: MouseEv) {
        match m {
            MouseEv::Down { button: Button::Left, x, y, sx, sy } => match self.zone(x, y) {
                Zone::Status => { self.status_hide_at = None; self.status_fade.set(false, self.now); }
                Zone::Bubble => {}
                Zone::Pet => { self.down = Some(Down { sx, sy, lx: None, dir: 0, flips: Vec::new() }); self.dragging = false; }
            },
            MouseEv::Wheel { delta, x, y } => {
                if self.zone(x, y) == Zone::Pet { self.on_wheel(delta); }
            }
            MouseEv::Move { x, y, sx, sy } => {
                if self.down.is_none() { self.on_cursor(x, y); }
                if self.down.is_some() { self.on_drag_move(sx, sy); } else if self.zone(x, y) == Zone::Pet { self.on_rub(x, y); }
            }
            MouseEv::Up { button: Button::Left, .. } => {
                if self.down.is_none() { return; }
                if self.dragging {
                    self.dragging_cls = false;
                    self.state = St::Fall;
                    self.land_shaken = self.shaken;
                    self.shaken = false;
                    self.cmds.push(WinCmd::DragEnd);
                } else {
                    self.on_poke();
                }
                self.down = None;
                self.dragging = false;
            }
            // 备忘提醒响着时双击 = 稍后再提醒，否则开聊天
            MouseEv::DoubleClick { x, y } if self.zone(x, y) == Zone::Pet && !self.memo_double_click() => self.open_chat(),
            _ => {}
        }
    }

    pub(super) fn on_drag_move(&mut self, sx: i32, sy: i32) {
        let s = self.scale;
        let Some(d) = &self.down else { return };
        let dist = (((sx - d.sx) as f32).powi(2) + ((sy - d.sy) as f32).powi(2)).sqrt();
        if !self.dragging && dist > 4.0 * s {
            self.yield_chat();
            self.dragging = true;
            self.dragging_cls = true;
            self.act(Act::Drag);
            self.drag_started_play();
            self.set_state(St::Drag, None);
            self.say(&pick(lines::DRAG), 1500);
        }
        if !self.dragging { return; }
        self.cmds.push(WinCmd::DragMove { sx, sy });
        // 甩来甩去会晕：统计 1.5 秒内左右换向次数
        let now = self.now;
        let Some(d) = self.down.as_mut() else { return };
        let dx = sx - d.lx.unwrap_or(sx);
        d.lx = Some(sx);
        if (dx.abs() as f32) > 6.0 * s {
            let dir = dx.signum();
            if dir != d.dir {
                d.dir = dir;
                d.flips.retain(|t| now - t < 1500);
                d.flips.push(now);
            }
            if d.flips.len() >= 6 { self.shaken = true; }
        }
    }

    /// 摸头不用点：鼠标在她头上来回蹭几下就算
    pub(super) fn on_rub(&mut self, x: f32, y: f32) {
        if self.chatting { return; }
        if y - PET_TOP > 293.0 * 0.38 { return; }
        let dx = x - self.rub.last;
        self.rub.last = x;
        if dx.abs() < 3.0 { return; }
        let d = dx.signum() as i32;
        let now = self.now;
        if d != self.rub.dir {
            self.rub.dir = d;
            self.rub.flips.retain(|t| now - t < 2000);
            self.rub.flips.push(now);
        }
        if self.rub.flips.len() >= 6 && now > self.rub.cool && !matches!(self.state, St::Drag | St::Fall) {
            self.rub.flips.clear();
            self.rub.cool = now + 5000;
            self.act(Act::Pat);
            if self.state == St::Sleep {
                self.say(lines::PAT_SLEEPING, 2600);
                self.fx(FxKind::Heart, 3);
                return;
            }
            self.react(St::Shy, 2200, Some(pick(lines::RUB)));
            self.fx(FxKind::Heart, 5);
        }
    }

    pub(super) fn on_poke(&mut self) {
        self.yield_chat();
        if self.memo_poke() { return; }
        if self.routine.is_some() { return self.finish_routine(true); }
        if self.state == St::Sleep {
            self.act(Act::Wake);
            self.play(Shot::Shake);
            self.say(&pick(lines::WAKE), 2600);
            return self.set_state(St::Sit, Some((8000.0, 15000.0)));
        }
        let now = self.now;
        self.pokes.retain(|t| now - t < 3000);
        self.pokes.push(now);
        if self.pokes.len() >= 5 {
            self.pokes.clear();
            self.act(Act::Angry);
            self.play(Shot::Shake);
            // 一半炸毛哈气，一半鼓脸赌气
            let puff = fastrand::bool();
            self.set_state(if puff { St::Puff } else { St::Hiss }, None);
            let l = if puff { lines::PUFF } else if fastrand::bool() { lines::HISS } else { lines::POKE_MAD };
            self.say(&pick(l), 2600);
            self.fx(FxKind::Anger, 1);
            self.hold_timer(2200, StateAct::Set(St::Idle, Some((5000.0, 10000.0))));
            return;
        }
        self.act(Act::Poke);
        if self.pomo_running && fastrand::bool() {
            let left = if self.io { crate::platform::pomodoro_left() } else { Some(25) };
            if let Some(left) = left { self.say(&pickv(lines::pomo_focus(left)), 2600); }
            self.play(Shot::Squash);
            return;
        }
        let m = self.stats.mood();
        match m {
            MoodLevel::Hungry | MoodLevel::Starving => {
                self.play(Shot::Bounce);
                self.say(&pick(lines::POKE_HUNGRY), 2600);
                return self.set_state(St::Hungry, Some((8000.0, 12000.0)));
            }
            MoodLevel::Exhausted => {
                self.play(Shot::Squash);
                self.say(&pick(lines::POKE_TIRED), 2600);
                return self.set_state(St::Sit, Some((8000.0, 12000.0)));
            }
            MoodLevel::Sad => {
                if fastrand::f64() < 0.4 {
                    self.play(Shot::Shake);
                    self.fx(FxKind::Tear, 4);
                    return self.react(St::Cry, 2600, Some(pick(lines::CRY)));
                }
                self.play(Shot::Squash);
                if fastrand::bool() {
                    return self.react(St::Pout, 2600, Some(pick(lines::POUT)));
                }
                self.say(&pick(lines::POKE_SAD), 2600);
                return self.set_state(St::Sit, Some((8000.0, 12000.0)));
            }
            _ => {}
        }
        if self.state == St::Walk && fastrand::f64() < 0.6 {
            self.play(Shot::Bounce);
            self.fx(FxKind::Exclaim, 1);
            return self.react(St::Surprised, 1400, Some(pick(lines::SURPRISED)));
        }
        self.play(Shot::Bounce);
        let great = m == MoodLevel::Great;
        if fastrand::f64() < 0.3 {
            if great { self.fx(FxKind::Heart, 3) } else { self.fx(FxKind::Spark, 6) }
        }
        if let Some(l) = self.affinity_line() { return self.react(St::Love, 2000, Some(l)); }
        if fastrand::f64() < 0.15 { return self.react(St::Confused, 1600, Some(pick(lines::CONFUSED))); }
        let l = if great && fastrand::bool() { lines::POKE_GREAT } else { lines::POKE };
        self.react(St::Happy, 1200, Some(pick(l)));
    }

    pub(super) fn on_landed(&mut self, big: bool) {
        if self.landed_play() { return; }
        self.play(Shot::Squash);
        if self.land_shaken {
            self.land_shaken = false;
            self.say(&pick(lines::DIZZY), 3000);
            self.set_state(St::Dizzy, None);
            self.fx(FxKind::Dizzy, 1);
            self.hold_timer(3200, StateAct::Set(St::Idle, Some((4000.0, 8000.0))));
            return;
        }
        if big {
            let scared = fastrand::bool();
            self.say(&pick(if scared { lines::SCARED } else { lines::LAND_BIG }), 2600);
            self.set_state(if scared { St::Scared } else { St::Drag }, None);
            if scared { self.play(Shot::Shake); self.fx(FxKind::Sweat, 1); }
            self.hold_timer(if scared { 2200 } else { 1200 }, StateAct::Set(St::Idle, Some((4000.0, 8000.0))));
        } else {
            self.set_state(St::Idle, Some((4000.0, 8000.0)));
        }
    }
}
