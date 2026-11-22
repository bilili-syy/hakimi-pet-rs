//! 右键菜单的动作和换装
use super::*;

impl Behavior {
    // ============================================================ 菜单
    pub(super) fn on_menu(&mut self, a: MenuAction) {
        if self.io { crate::core::log::trace(&format!("menu {a:?}")); }
        // 让她做事的菜单项：输入框开着但没打字就顺手关掉；打了字就留着（框跟着她走）
        if matches!(
            a,
            MenuAction::Feed | MenuAction::Pat | MenuAction::Honey | MenuAction::Song | MenuAction::Sleep
                | MenuAction::Walk | MenuAction::Costume(_) | MenuAction::Outfit(..) | MenuAction::ClearOutfit
                | MenuAction::Dance(_)
        ) {
            self.yield_chat();
        }
        match a {
            MenuAction::Feed => self.toss_food(),
            MenuAction::Game => self.start_game(),
            MenuAction::CheckIn => self.do_checkin(true),
            MenuAction::WeatherReport => self.weather_report(true),
            MenuAction::Climb => self.start_climb(crate::types::PerchKind::WindowTop, true),
            MenuAction::FeedNow => {
                let was_starving = self.stats.data.hunger < 15.0;
                if self.act(Act::Feed) == ActResult::Full {
                    self.play(Shot::Shake);
                    self.say(&pick(lines::FULL), 2600);
                    return;
                }
                if was_starving {
                    self.react(St::Love, 2800, Some(pick(lines::FEED_STARVING)));
                    self.fx(FxKind::Heart, 6);
                } else {
                    self.react(St::Eat, 2600, Some(pick(lines::FEED)));
                    self.fx(FxKind::Heart, 2);
                }
            }
            MenuAction::Pat => {
                self.act(Act::Pat);
                if self.state == St::Sleep {
                    self.play(Shot::Squash);
                    self.say(lines::PAT_SLEEPING, 2600);
                    return;
                }
                let l = if fastrand::bool() { lines::SHY } else { lines::PAT };
                self.react(St::Shy, 2200, Some(pick(l)));
                self.fx(FxKind::Heart, 5);
            }
            MenuAction::Honey => self.drink_honey(),
            MenuAction::Song => self.play_song(),
            MenuAction::Dance(id) => self.start_routine(&id),
            MenuAction::Sleep => self.go_sleep(),
            MenuAction::Walk => self.go_walk(),
            MenuAction::Status => self.show_status(),
            MenuAction::Chat => self.open_chat(),
            MenuAction::ChatStop => self.chat_stop(),
            MenuAction::CopyBubble => {
                if self.last_bubble_text.trim().is_empty() {
                    self.show_bubble("还没有可以复制的话喵～".into(), Some(self.now + 2000), 2000, 0, false);
                } else {
                    let t = self.last_bubble_text.clone();
                    self.call(format!("copy:{t}"));
                    let ok = if self.io { crate::platform::copy_text(&t) } else { true };
                    let msg = if ok { "刚才那句话复制到剪贴板啦喵～" } else { "剪贴板正忙，没复制上喵…" };
                    self.show_bubble(msg.into(), Some(self.now + 2200), 2200, 0, false);
                }
            }
            MenuAction::Costume(id) => {
                // 「🎭 角色」子菜单（skin:…）
                if self.on_skin_menu(&id) { return; }
                let id = crate::core::costume::normalize(&id).to_string();
                if id == self.costume { return; }
                if self.io { let v = crate::core::costume::config_value(&id); crate::core::config::update(|c| c.costume = v); }
                self.costume = id;
                self.outfit_changed(true, false);
            }
            MenuAction::Outfit(id, on) => {
                let mut l: Vec<String> = self.outfits.iter().filter(|x| **x != id).cloned().collect();
                if on { l.push(id); }
                self.set_outfits(l);
            }
            MenuAction::ClearOutfit => self.set_outfits(Vec::new()),
            MenuAction::ReloadSettings => {
                let (c, o) = if self.io {
                    let cfg = crate::core::config::reload();
                    // 设置窗口里关了声音或勾了总静音：正在念的停掉，并中止未念完的语音回答队列
                    if !cfg.tts.enabled || cfg.tts.muted {
                        self.stop_voice();
                        self.cv_disable_tts();
                    }
                    (cfg.costume, cfg.outfit)
                } else {
                    (self.costume.clone(), self.outfits.clone())
                };
                self.costume = crate::core::costume::normalize(&c).to_string();
                self.outfits = o;
                self.say(&pick(lines::RELOADED), 2600);
            }
            MenuAction::Tts(on) => {
                if self.io { crate::core::config::update(|c| c.tts.enabled = on); }
                if !on {
                    self.stop_voice();
                    self.cv_disable_tts();
                }
            }
            MenuAction::TtsChatOnly(on) => {
                if self.io { crate::core::config::update(|c| c.tts.speak = if on { "chat".into() } else { "all".into() }); }
            }
            MenuAction::Moyu(on) => {
                if self.io { crate::core::config::update(|c| c.moyu.enabled = on); }
                self.say(&pick(if on { lines::MOYU_ON } else { lines::MOYU_OFF }), 2600);
            }
            MenuAction::Memo(m) => self.on_memo_menu(m),
            MenuAction::Mind(m) => self.on_mind_menu(m),
            // 平台层打开设置窗口的周报页
            MenuAction::Report | MenuAction::ExportChat | MenuAction::Roast => {}
            MenuAction::ForgetChat => {
                if self.io { crate::services::ai::clear_history(); }
                self.play(Shot::Shake);
                self.say(&pick(lines::FORGOT), 2600);
            }
            MenuAction::ToggleVisible => {
                // 藏起来时顺手停掉正在跳的舞、外放、语音队列和置顶玩具，免得窗口隐藏还在背后出鼓点/悬浮玩具
                self.cleanup_for_hide();
            }
            MenuAction::CallToCenter => {
                self.reset_aerial_flags();
            }
            MenuAction::VolumePopup => {
                self.call("vol:popup".into());
            }
            MenuAction::VolumeMute(on) => {
                let (vp, mp, mu) = self.mutate_volume(false, |_, _, m| *m = on);
                if !self.memo_ringing() && self.voice_playing.is_none() && self.cv.is_none() {
                    let msg = if mu {
                        "🔇 已开启桌宠静音喵～".to_string()
                    } else {
                        format!("🔊 声音恢复啦喵（语音 {vp}% · 音乐 {mp}%）")
                    };
                    self.show_bubble(msg, Some(self.now + 2200), 2200, 0, false);
                }
            }
            MenuAction::VolumePreset(pct) => {
                let p = pct.min(100);
                let (vp, mp, _) = self.mutate_volume(false, |v, m, mu| {
                    *v = p;
                    *m = p;
                    *mu = p == 0;
                });
                if !self.memo_ringing() && self.voice_playing.is_none() && self.cv.is_none() {
                    let msg = format!("🔊 音量已调到 {vp}%（伴奏 {mp}%）喵～");
                    self.show_bubble(msg, Some(self.now + 2000), 2000, 0, false);
                }
            }
            // 平台层自己处理：番茄钟（会发 AppEvent::Pomodoro）、叫到中间（还会发 Peek）、
            // 打开文件/文件夹、自启、退出（会调 Handler::quit）
            MenuAction::PomodoroStart(_) | MenuAction::PomodoroStop | MenuAction::OpenSongDir | MenuAction::OpenSettings
            | MenuAction::OpenConfigFile
            | MenuAction::OpenDataDir | MenuAction::AutoStart(_)
            | MenuAction::Quit => {}
        }
    }

    pub(super) fn set_outfits(&mut self, l: Vec<String>) {
        if l == self.outfits { return; }
        let added = l.len() > self.outfits.len();
        if self.io { let v = l.clone(); crate::core::config::update(|c| c.outfit = v); }
        self.outfits = l;
        self.outfit_changed(false, added);
    }

    pub(super) fn outfit_changed(&mut self, costume_changed: bool, added: bool) {
        self.play(Shot::Bounce);
        let l = if costume_changed {
            pick(lines::costume_line(&self.costume))
        } else if added {
            pick(lines::OUTFIT_ON)
        } else {
            pick(lines::OUTFIT_OFF)
        };
        self.react(St::Happy, 2500, Some(l));
    }
}
