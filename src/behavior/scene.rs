//! 每帧把状态画成 Scene（宠物姿态、尾巴星星、气泡、状态卡）
use super::*;

impl Behavior {
    // ============================================================ 场景
    pub(super) fn current_pet_anim(&self) -> PetAnim {
        let now = self.now;
        if let Some((s, t)) = self.shot && now < t + s.dur() { return PetAnim::Shot(s); }
        if self.chase.is_some_and(|t| now < t + 1800) { return PetAnim::Chase; }
        if self.dragging_cls { return PetAnim::Wiggle; }
        match self.state {
            St::Walk => PetAnim::Walk(now < self.fast_until && self.glow),
            St::Sleep => PetAnim::BreatheSlow,
            St::Dance => PetAnim::Still,
            St::Hiss => PetAnim::Hiss,
            St::Dizzy => PetAnim::Dizzy,
            _ => PetAnim::Breathe,
        }
    }

    pub(super) fn pet_xf(&mut self) -> Xf {
        let want = self.current_pet_anim();
        if want != self.pet_anim.0 {
            // 一次性动画用自己的起点（play() 的时刻），循环动画从切换时重新开始
            let start = match want {
                PetAnim::Shot(_) => self.shot.map(|s| s.1).unwrap_or(self.now),
                PetAnim::Chase => self.chase.unwrap_or(self.now),
                _ => self.now,
            };
            self.pet_anim = (want, start);
        }
        let e = self.now.saturating_sub(self.pet_anim.1) as f32;
        let run = |dur: f32, n: Option<u32>, f: fn(f32) -> Xf| anim::cycle(e, dur, n).map(f).unwrap_or(Xf::ID);
        match want {
            PetAnim::Shot(Shot::Bounce) => run(450.0, Some(1), anim::bounce),
            PetAnim::Shot(Shot::Squash) => run(400.0, Some(1), anim::squash),
            PetAnim::Shot(Shot::Shake) => run(400.0, Some(1), anim::shake),
            PetAnim::Shot(Shot::Jump) => run(700.0, Some(1), anim::jump),
            PetAnim::Chase => run(600.0, Some(3), anim::chase),
            PetAnim::Wiggle => run(350.0, None, anim::wiggle),
            PetAnim::Walk(fast) => run(if fast { 250.0 } else { 500.0 }, None, anim::walk),
            PetAnim::BreatheSlow => run(4000.0, None, anim::breathe_slow),
            PetAnim::Still => Xf::ID,
            PetAnim::Hiss => run(250.0, Some(4), anim::shake),
            PetAnim::Dizzy => run(600.0, Some(5), anim::wiggle),
            PetAnim::Breathe => run(2600.0, None, anim::breathe),
        }
    }

    pub(super) fn tail_star(&self) -> (Xf, Xf) {
        let now = self.now as f64;
        let m = self.mood_cls;
        let sad = m == MoodLevel::Sad;
        let tired = m == MoodLevel::Exhausted;
        let p = |dur: f32| ((now % dur as f64) / dur as f64) as f32;
        let tail = if self.state == St::Happy {
            anim::tail_wag(p(450.0))
        } else if sad || tired {
            anim::tail_droop(p(4000.0))
        } else {
            let dur = if m == MoodLevel::Great { 1300.0 } else if self.state == St::Walk { 900.0 } else { 2400.0 };
            anim::tail_sway(p(dur))
        };
        let star = if sad {
            anim::star_droop(p(4000.0))
        } else if matches!(self.state, St::Happy | St::Eat) {
            anim::star_spin(p(1200.0))
        } else {
            anim::star_bob(p(3200.0))
        };
        (tail, star)
    }

    pub(super) fn build_scene(&mut self) {
        let now = self.now;
        let live = self.live_view();
        let mut pet = self.pet_xf();
        if self.live_replaces_breathe() && matches!(self.pet_anim.0, PetAnim::Breathe | PetAnim::BreatheSlow) { pet = Xf::ID; }
        let mut pet = self.blend_pet(pet);
        let (lean, tilt) = self.play.lean_at(now);
        pet.rot += tilt;
        let (tail, star) = self.tail_star();
        let bubble = self.bubble.as_ref().and_then(|b| {
            let op = self.bubble_fade.value(now);
            if op <= 0.0 { return None; }
            // 超出 max-height 时慢慢往下滚（原版可以用滚轮，这里自动滚）
            let scroll = if self.ui.bubble_overflow {
                let span = b.dur.saturating_sub(3000).max(1000) as f32;
                ((now.saturating_sub(b.shown) as f32 - 1500.0) / span).clamp(0.0, 1.0)
            } else { 0.0 };
            let scroll = (scroll * 100.0).round() / 100.0;
            Some(BubbleView { text: b.text.clone(), opacity: op, scroll })
        });
        let status = self.status.as_ref().and_then(|s| {
            let op = self.status_fade.value(now);
            (op > 0.0).then(|| StatusView { opacity: op, ..s.clone() })
        });
        self.scene.zzz.clear();
        if let (St::Sleep, Some(since)) = (self.state, self.sleep_since) {
            const Z: [(char, f32, u32); 3] = [('z', 14.0, 0x4285f4), ('z', 18.0, 0x34a853), ('Z', 24.0, 0xea4335)];
            for (i, (ch, size, color)) in Z.iter().enumerate() {
                let t = now as i64 - since as i64 - i as i64 * 1000;
                if t < 0 { continue; }
                let xf = anim::zfloat((t % 3000) as f32 / 3000.0);
                self.scene.zzz.push(ZGlyph { ch: *ch, size: *size, color: *color, x: xf.tx, y: xf.ty, opacity: xf.opacity });
            }
        }
        let flip = self.flip_value();
        self.scene.face = self.face;
        if self.scene.costume != self.costume {
            self.scene.costume.clone_from(&self.costume);
        }
        if self.scene.outfits != self.outfits {
            self.scene.outfits.clone_from(&self.outfits);
        }
        self.scene.pet = pet;
        self.scene.flip = flip;
        self.scene.tail = tail;
        self.scene.star = star;
        self.scene.glow = self.glow;
        self.scene.lean = lean;
        self.scene.bubble = bubble;
        self.scene.status = status;
        self.scene.fx = self.fx.sprites(now);
        self.scene.live = live;
        self.apply_dance_scene();
    }
}
