//! 聊天窗口：打开、输入、AI 回复
use super::*;

/// 视野短梗彩蛋：只匹配纯玩梗/点歌/喂蜂蜜的短句，避免用户正常喊「哈基米，今天好累」或加备忘被劫持
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EasterEgg {
    Song,
    Honey,
}

pub(super) fn easter_egg(text: &str) -> Option<EasterEgg> {
    let clean: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && !"，。！？、～~,.!?…·：:;；\"'“”‘’（）()".contains(*c))
        .collect();
    if clean.is_empty() {
        return None;
    }
    let len = clean.chars().count();
    let is_question = text.contains('?')
        || text.contains('？')
        || ["吗", "什么", "怎么", "为什么", "几", "哪", "谁", "多少"].iter().any(|q| clean.contains(q));
    if is_question {
        return None;
    }
    if len <= 10 && ["南北绿豆", "曼波", "阿西嘎", "奶龙"].iter().any(|k| clean.contains(k)) {
        return Some(EasterEgg::Song);
    }
    if matches!(clean.as_str(), "哈基米" | "哈吉米" | "唱首歌" | "来首歌" | "唱支歌")
        || (len <= 8
            && (clean.contains("哈基米") || clean.contains("哈吉米"))
            && ["唱", "歌", "跳", "舞", "哼"].iter().any(|k| clean.contains(k)))
    {
        return Some(EasterEgg::Song);
    }
    if matches!(
        clean.as_str(),
        "蜂蜜" | "蜂蜜水" | "喝蜂蜜" | "喝蜂蜜水" | "喂蜂蜜" | "喂蜂蜜水" | "来点蜂蜜" | "给你蜂蜜"
    ) {
        return Some(EasterEgg::Honey);
    }
    None
}

/// 吃撑了说话的样子：打嗝或者尾音拖长（roll 0~1 随机数）
pub(super) fn stuffed_text(text: &str, roll: f64) -> String {
    let t = text.trim_end();
    if t.contains('嗝') {
        return t.to_string();
    }
    if roll < 0.5 {
        format!("{t}…嗝～")
    } else {
        let base = t.trim_end_matches(['～', '~', '。', '！', '!']);
        format!("{base}～～～")
    }
}

/// 这么多新增粮食（token + 语音字数）给饱食度加多少：1000 份 = +1，一次最多 +10
pub(super) fn food_to_hunger(delta: u64) -> f64 { (delta as f64 / 1000.0).min(10.0) }

impl Behavior {
    /// 用量就是粮食：今天新吃了多少就加多少饱食度（跨天数字变小时只重置基准）
    pub(super) fn feed_from_usage(&mut self, food: u64) {
        let seen = self.food_seen.replace(food);
        if let Some(prev) = seen
            && food > prev
        {
            let add = food_to_hunger(food - prev);
            if add > 0.0 {
                self.stats.data.hunger = (self.stats.data.hunger + add).min(100.0);
            }
        }
    }

    // ============================================================ 聊天
    pub(super) fn open_chat(&mut self) {
        if self.state == St::Sleep {
            self.say(&pick(lines::WAKE), 2600);
            self.act(Act::Wake);
        }
        self.chatting = true;
        self.state_timer = None;
        if matches!(self.state, St::Walk | St::Sleep) { self.set_state(St::Idle, None); }
        if self.io {
            self.hold_timer(35_000, StateAct::Think);
        }
        self.cmds.push(WinCmd::ChatInput(ChatInputCmd::Open));
        if self.waiting {
            self.cmds.push(WinCmd::ChatInput(ChatInputCmd::Enable(false)));
        }
        // 新手：还没填聊天 Key，第一次打开聊天就先告诉她去哪儿填
        if !self.key_hint_shown && self.io && crate::core::config::ai_key_missing(&crate::core::config::get()) {
            self.key_hint_shown = true;
            self.say(crate::services::ai::KEY_HINT, 9000);
        }
    }

    /// 输入框里有没有打了字
    pub(super) fn chat_typed(&self) -> bool {
        if self.io { crate::platform::chat_has_text() } else { self.fake_chat_text }
    }

    /// 正在等 AI/看屏吐槽回复，或者聊天框开着且打了字：这时不自己溜达/睡觉，也不被别的动作关掉
    pub(super) fn chat_busy(&self) -> bool { self.waiting || (self.chatting && self.chat_typed()) }

    /// 别的动作来了：空输入框直接收起（不改状态，让那个动作自己定）；有内容就留着
    pub(super) fn yield_chat(&mut self) {
        if !self.chatting || self.chat_busy() { return; }
        self.chatting = false;
        self.cmds.push(WinCmd::ChatInput(ChatInputCmd::Close));
    }

    pub(super) fn close_chat(&mut self) {
        self.chatting = false;
        if !self.waiting
            && self.routine.is_none()
            && !matches!(self.state, St::Dance | St::Honey | St::Sleep | St::Drag | St::Fall)
        {
            self.set_state(St::Idle, Some((5000.0, 9000.0)));
        }
    }

    pub(super) fn on_chat_submit(&mut self, text: String) {
        let text = text.trim().to_string();
        if text.is_empty() || self.waiting { return; }
        // 「提醒我…」「记一下…」「我有什么待办」
        // 「记住…」「忘掉…」「你记得我什么」——优先于彩蛋，避免「哈基米提醒我…」或「提醒我买蜂蜜」被劫持
        if self.mind_chat(&text) { return; }
        if self.memo_chat(&text) { return; }
        // 哈基米短梗彩蛋：不走 AI，直接唱跳或喝蜂蜜水
        match easter_egg(&text) {
            Some(EasterEgg::Song) => {
                self.act(Act::Chat);
                return self.play_song();
            }
            Some(EasterEgg::Honey) => {
                self.act(Act::Chat);
                return self.drink_honey();
            }
            None => {}
        }
        // 本地明确动作口令（换装/配饰/跳舞/番茄钟/打开白名单工具）：免等云端，离线无 Key 也能直接执行
        if let Some((act, hint)) = crate::services::actions::parse_local_direct(&text) {
            self.act(Act::Chat);
            if !hint.is_empty() {
                self.say(hint, 2600);
            }
            self.run_chat_acts(vec![act], false);
            return;
        }
        let req_id = self.begin_chat_wait();
        self.cmds.push(WinCmd::ChatInput(ChatInputCmd::Enable(false)));
        if !self.dragging && !matches!(self.state, St::Drag | St::Fall) {
            self.set_state(St::Think, None);
        }
        self.say_opts(&pick(lines::THINK), 30_000, false, SpeakKind::Line);
        let d = &self.stats.data;
        let ctx = crate::services::ai::ChatCtx {
            hunger: d.hunger.round() as u32,
            mood: d.mood.round() as u32,
            energy: d.energy.round() as u32,
            level: self.stats.level().1.to_string(),
            days: self.stats.days(),
        };
        self.call(format!("chat:{text}"));
        if self.io { crate::services::ai::chat(&self.tx, req_id, text, ctx); }
    }

    pub(super) fn begin_chat_wait(&mut self) -> u64 {
        self.waiting = true;
        let req_id = crate::services::ai::next_req_id();
        self.active_chat_req = req_id;
        req_id
    }

    /// 看屏吐槽：主人确认过的截图。和聊天一样等回复（Esc 也能取消），回复走 on_chat_reply_id
    pub(super) fn on_roast(&mut self, png: Vec<u8>) {
        if self.waiting { return; }
        let req_id = self.begin_chat_wait();
        self.act(Act::Chat);
        self.cmds.push(WinCmd::ChatInput(ChatInputCmd::Enable(false)));
        if !self.dragging && !matches!(self.state, St::Drag | St::Fall) && self.play.game.is_none() {
            self.set_state(St::Think, None);
        }
        if !self.memo_ringing() {
            self.say_opts(&pick(lines::ROAST_LOOK), 30_000, false, SpeakKind::Line);
        }
        let d = &self.stats.data;
        let ctx = crate::services::ai::ChatCtx {
            hunger: d.hunger.round() as u32,
            mood: d.mood.round() as u32,
            energy: d.energy.round() as u32,
            level: self.stats.level().1.to_string(),
            days: self.stats.days(),
        };
        self.call(format!("roast:{}", png.len()));
        if self.io { crate::services::ai::roast(&self.tx, req_id, png, ctx); }
    }

    /// 等回复时按 Esc：不等了。回复到了丢掉、不进历史；正在合成第一段语音的也停
    pub(super) fn chat_stop(&mut self) {
        if !self.waiting {
            return self.close_chat();
        }
        self.waiting = false;
        self.active_chat_req = 0;
        if self.cv.is_some() {
            self.cv = None;
            self.emotion_cv_clear();
        } else if self.io {
            crate::services::ai::cancel_current();
        } else {
            self.drop_replies = self.drop_replies.saturating_add(1);
        }
        self.call("chat:stop".into());
        self.cmds.push(WinCmd::ChatInput(ChatInputCmd::Enable(true)));
        if !self.dragging && !matches!(self.state, St::Drag | St::Fall) && self.play.game.is_none() {
            self.set_state(St::Idle, None);
        }
        if !self.memo_ringing() {
            self.say_opts(&pick(lines::CHAT_STOP), 2200, false, SpeakKind::Line);
        }
    }

    pub(super) fn on_chat_reply(&mut self, r: ChatReply) {
        self.on_chat_reply_id(0, r);
    }

    pub(super) fn on_chat_reply_id(&mut self, req_id: u64, mut r: ChatReply) {
        if req_id != 0 {
            if req_id != self.active_chat_req {
                self.call("chat:dropped".into());
                return;
            }
            self.active_chat_req = 0;
        } else if self.drop_replies > 0 {
            self.drop_replies -= 1;
            self.call("chat:dropped".into());
            return;
        }
        // 先剥离受控动作标签 [do:...]，再剥离情绪标签（气泡和 TTS 只拿干净文字）
        let acts = if r.ok {
            let (clean_txt, extracted) = crate::services::actions::extract(&r.text);
            r.text = clean_txt;
            extracted
        } else {
            Vec::new()
        };
        let (mut r, emo) = self.emotion_split(r);
        // 若模型只返回了 [do:...] 标签没有正文，直接按本地动作模式执行（带默认动作台词）
        if r.ok && r.text.trim().is_empty() && !acts.is_empty() {
            self.waiting = false;
            self.cmds.push(WinCmd::ChatInput(ChatInputCmd::Enable(true)));
            self.act(Act::Chat);
            self.run_chat_acts(acts, false);
            self.mind_after_reply(true);
            return;
        }
        // 今天快吃撑了：打嗝、拖长尾音
        if r.ok && self.io && crate::core::usage::now_fullness() != crate::core::usage::Fullness::Normal {
            r.text = stuffed_text(&r.text, fastrand::f64());
        }
        if r.ok && self.io {
            crate::core::usage::add_chat();
        }
        // 语音回答：先合成，字和声音一起出来（chatvoice.rs）；这期间保持思考中
        if r.ok && self.chat_voice_on() {
            self.cv_start(&r.text, emo);
            if self.cv.is_some() {
                // 第一段出字时再换表情（emotion_cv_reveal），整段念完 2 秒后回平时
                self.emotion_cv_pending(emo, r.text.chars().count());
                if !acts.is_empty() {
                    self.run_chat_acts(acts, true);
                }
                self.mind_after_reply(true);
                return;
            }
        }
        self.waiting = false;
        self.cmds.push(WinCmd::ChatInput(ChatInputCmd::Enable(true)));
        let busy_phys = self.dragging || matches!(self.state, St::Drag | St::Fall);
        if r.ok {
            self.act(Act::Chat);
            if !busy_phys {
                self.play(Shot::Bounce);
                self.set_state(St::Happy, None);
                self.emotion_face(emo);
            }
        } else if !busy_phys {
            self.play(Shot::Shake);
            self.set_state(if r.need_key { St::Sit } else { St::Drag }, None);
        }
        // 未填 API Key 时，直接帮主人弹出「AI 聊天」设置页，免得再绕一圈找右键菜单
        if r.need_key {
            self.cmds.push(WinCmd::OpenSettingsPage(0));
        }
        // 字多就多停一会儿：2.5 秒起，每字 +0.15 秒，最多 20 秒
        let ms = (2500 + r.text.encode_utf16().count() as u64 * 150).min(20_000);
        // 语音回答关了：聊天回复只出字
        let voice = !(r.ok && self.chat_voice_disabled_by_user());
        // 主人在等这句回复：全屏安静模式下也念；但若备忘提醒正响着，保留 90 秒提醒气泡不被覆盖
        self.sense_touch();
        if self.memo_ringing() {
            if !r.text.trim().is_empty() {
                self.last_bubble_text = r.text.clone();
                if self.io {
                    crate::platform::set_has_copyable_bubble(true);
                }
            }
        } else {
            self.say_opts(&r.text, ms, voice, if r.ok { SpeakKind::Chat } else { SpeakKind::Line });
        }
        if !busy_phys {
            self.hold_timer(2500, StateAct::ChatIdle);
            self.emotion_hold(emo);
        }
        if !acts.is_empty() {
            self.run_chat_acts(acts, true);
        }
        self.mind_after_reply(r.ok);
    }

    /// 执行聊天解析出来的动作列表。`preserve_bubble = true` 表示 AI 已经给了自己的回复气泡和语音，动作只改状态/配置不覆盖气泡
    pub(super) fn run_chat_acts(&mut self, acts: Vec<crate::services::actions::ChatAct>, preserve_bubble: bool) {
        use crate::services::actions::ChatAct as A;
        for act in acts {
            match act {
                A::Costume(id) => {
                    let norm = crate::core::costume::normalize(&id).to_string();
                    self.call(format!("tool:costume:{norm}"));
                    if norm != self.costume {
                        if self.io {
                            let v = crate::core::costume::config_value(&norm);
                            crate::core::config::update(|c| c.costume = v);
                        }
                        self.costume = norm;
                        if preserve_bubble {
                            self.play(Shot::Bounce);
                        } else {
                            self.outfit_changed(true, false);
                        }
                    } else if !preserve_bubble {
                        self.play(Shot::Bounce);
                        self.say(&pick(lines::costume_line(&self.costume)), 2500);
                    }
                }
                A::Outfit(id, on) => {
                    self.call(format!("tool:outfit:{id}:{on}"));
                    let mut l: Vec<String> = self.outfits.iter().filter(|x| **x != id).cloned().collect();
                    if on {
                        l.push(id);
                    }
                    if preserve_bubble {
                        if l != self.outfits {
                            if self.io {
                                let v = l.clone();
                                crate::core::config::update(|c| c.outfit = v);
                            }
                            self.outfits = l;
                            self.play(Shot::Bounce);
                        }
                    } else {
                        self.set_outfits(l);
                    }
                }
                A::ClearOutfit => {
                    self.call("tool:outfit:clear".into());
                    if preserve_bubble {
                        if !self.outfits.is_empty() {
                            if self.io {
                                crate::core::config::update(|c| c.outfit.clear());
                            }
                            self.outfits.clear();
                            self.play(Shot::Bounce);
                        }
                    } else {
                        self.set_outfits(Vec::new());
                    }
                }
                A::PomodoroStart(mins) => {
                    self.call(format!("tool:pomo:{mins}"));
                    if preserve_bubble {
                        self.pomo_running = true;
                        if self.io {
                            crate::platform::start_pomodoro_quiet(mins);
                        }
                    } else if self.io {
                        crate::platform::push_menu_id(&format!("pomo:{mins}"));
                    } else {
                        self.on_pomodoro(PomodoroEv::Start { minutes: mins });
                    }
                }
                A::PomodoroStop => {
                    self.call("tool:pomo:stop".into());
                    if preserve_bubble {
                        self.pomo_running = false;
                        if self.io {
                            crate::platform::stop_pomodoro_quiet();
                        }
                    } else if self.io {
                        crate::platform::push_menu_id("pomo:stop");
                    } else {
                        self.on_pomodoro(PomodoroEv::Cancel);
                    }
                }
                A::Memo { text, time, repeat } => {
                    self.call(format!(
                        "tool:memo:{}|{}|{:?}",
                        text,
                        time.map(crate::core::memo::Dt::fmt).unwrap_or_default(),
                        repeat
                    ));
                    if self.io {
                        crate::core::memo::add(&text, time, repeat);
                    }
                }
                A::OpenTarget(target) => {
                    self.call(format!("tool:open:{target}"));
                    if self.io {
                        crate::platform::request_tool_open(&target);
                    }
                }
                A::Dance(id) => {
                    self.call(format!("tool:dance:{id}"));
                    let prev_bubble = preserve_bubble.then(|| self.bubble.as_ref().map(|b| (b.text.clone(), b.hide_at, b.dur, b.token))).flatten();
                    self.start_routine(&id);
                    if let Some((txt, hide, dur, tok)) = prev_bubble {
                        self.show_bubble(txt, hide, dur, tok, false);
                    }
                }
                A::Song => {
                    self.call("tool:song".into());
                    let prev_bubble = preserve_bubble.then(|| self.bubble.as_ref().map(|b| (b.text.clone(), b.hide_at, b.dur, b.token))).flatten();
                    self.play_song();
                    if let Some((txt, hide, dur, tok)) = prev_bubble {
                        self.show_bubble(txt, hide, dur, tok, false);
                    }
                }
                A::Weather => {
                    self.call("tool:weather".into());
                    self.weather_report(true);
                }
                A::Sleep => {
                    self.call("tool:sleep".into());
                    if preserve_bubble {
                        self.set_state(St::Yawn, None);
                        self.hold_timer(2200, StateAct::Set(St::Sleep, Some((60000.0, 150000.0))));
                    } else {
                        self.go_sleep();
                    }
                }
                A::Walk => {
                    self.call("tool:walk".into());
                    if preserve_bubble {
                        if self.play.perch != Some(crate::types::PerchKind::WindowTop) {
                            self.play.perch = None;
                            self.walk_dir = if fastrand::bool() { -1 } else { 1 };
                            self.set_state(St::Walk, None);
                            let dist = rand(160.0, 420.0) as f32 * self.scale;
                            self.cmds.push(WinCmd::Walk { dir: self.walk_dir, distance: dist, speed: 1.6 * self.scale });
                        } else {
                            self.jump_down();
                        }
                    } else {
                        self.go_walk();
                    }
                }
                A::Honey => {
                    self.call("tool:honey".into());
                    if preserve_bubble {
                        if self.act(Act::Feed) != ActResult::Full {
                            self.set_state(St::Honey, None);
                            self.fx(FxKind::Honey, 6);
                            self.later(2700, Later::HoneyFast);
                            self.hold_timer(2600, StateAct::Set(St::Idle, Some((5000.0, 9000.0))));
                        }
                    } else {
                        self.drink_honey();
                    }
                }
                A::Status => {
                    self.call("tool:status".into());
                    self.show_status();
                }
                A::Game => {
                    self.call("tool:game".into());
                    let prev_bubble = preserve_bubble.then(|| self.bubble.as_ref().map(|b| (b.text.clone(), b.hide_at, b.dur, b.token))).flatten();
                    self.start_game();
                    if let Some((txt, hide, dur, tok)) = prev_bubble {
                        self.show_bubble(txt, hide, dur, tok, false);
                    }
                }
                A::Climb => {
                    self.call("tool:climb".into());
                    self.start_climb(crate::types::PerchKind::WindowTop, true);
                }
                A::VolumeSet { channel, percent } => {
                    use crate::services::actions::VolumeChan as C;
                    let p = percent.min(100);
                    self.call(format!("tool:vol:set:{channel:?}:{p}"));
                    let (vp, mp, mu) = self.mutate_volume(!preserve_bubble, |v, m, muted| {
                        match channel {
                            C::All => {
                                *v = p;
                                *m = p;
                            }
                            C::Voice => *v = p,
                            C::Music => *m = p,
                        }
                        if p > 0 {
                            *muted = false;
                        } else if matches!(channel, C::All) {
                            *muted = true;
                        }
                    });
                    if !preserve_bubble {
                        self.play(Shot::Squash);
                        let msg = if mu {
                            "好哒，哈基米先安静一会儿喵～（🔇 已静音）".to_string()
                        } else {
                            format!("好喵，音量调好啦～（语音 {vp}% · 音乐 {mp}%）")
                        };
                        self.say_opts(&msg, 2600, !mu && vp > 0, SpeakKind::Line);
                    }
                }
                A::VolumeDelta { channel, delta } => {
                    use crate::services::actions::VolumeChan as C;
                    self.call(format!("tool:vol:delta:{channel:?}:{delta}"));
                    let d = delta as i32;
                    let (vp, mp, mu) = self.mutate_volume(!preserve_bubble, |v, m, muted| {
                        if *muted && d > 0 {
                            *muted = false;
                        }
                        match channel {
                            C::All => {
                                *v = (*v as i32 + d).clamp(0, 100) as u8;
                                *m = (*m as i32 + d).clamp(0, 100) as u8;
                            }
                            C::Voice => *v = (*v as i32 + d).clamp(0, 100) as u8,
                            C::Music => *m = (*m as i32 + d).clamp(0, 100) as u8,
                        }
                    });
                    if !preserve_bubble {
                        self.play(Shot::Squash);
                        let msg = format!("好哒，现在语音 {vp}% · 音乐 {mp}% 喵～");
                        self.say_opts(&msg, 2600, !mu && vp > 0, SpeakKind::Line);
                    }
                }
                A::VolumeMute(on) => {
                    self.call(format!("tool:vol:mute:{on}"));
                    let (vp, mp, mu) = self.mutate_volume(false, |_, _, muted| *muted = on);
                    if !preserve_bubble {
                        self.play(Shot::Squash);
                        if mu {
                            self.say_opts("好哒，哈基米先不吭声啦喵～（🔇 已静音）", 2600, false, SpeakKind::Line);
                        } else {
                            let msg = format!("哇，又可以开口说话啦喵！（语音 {vp}% · 音乐 {mp}%）");
                            self.say_opts(&msg, 2600, vp > 0, SpeakKind::Line);
                        }
                    }
                }
                A::VolumePanel => {
                    self.call("tool:vol:panel".into());
                    if self.io {
                        crate::platform::open_volume_popup();
                    }
                    if !preserve_bubble {
                        self.play(Shot::Bounce);
                        self.say("音量调节条给你打开啦喵，随手拖就行～", 2600);
                    }
                }
            }
        }
    }
}
