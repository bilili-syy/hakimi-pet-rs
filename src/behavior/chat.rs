//! 聊天窗口：打开、输入、AI 回复
use super::*;

impl Behavior {
    // ============================================================ 聊天
    pub(super) fn open_chat(&mut self) {
        if self.state == St::Sleep {
            self.say(&pick(lines::WAKE), 2600);
            self.act(Act::Wake);
        }
        self.chatting = true;
        self.state_timer = None;
        if matches!(self.state, St::Walk | St::Sleep) { self.set_state(St::Idle, None); }
        self.cmds.push(WinCmd::ChatInput(ChatInputCmd::Open));
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

    /// 聊天框开着且「有内容」：打了字，或者正在等 AI 回复。这时不自己溜达/睡觉，也不被别的动作关掉
    pub(super) fn chat_busy(&self) -> bool { self.chatting && (self.waiting || self.chat_typed()) }

    /// 别的动作来了：空输入框直接收起（不改状态，让那个动作自己定）；有内容就留着
    pub(super) fn yield_chat(&mut self) {
        if !self.chatting || self.chat_busy() { return; }
        self.chatting = false;
        self.cmds.push(WinCmd::ChatInput(ChatInputCmd::Close));
    }

    pub(super) fn close_chat(&mut self) {
        self.chatting = false;
        if !self.waiting { self.set_state(St::Idle, Some((5000.0, 9000.0))); }
    }

    pub(super) fn on_chat_submit(&mut self, text: String) {
        let text = text.trim().to_string();
        if text.is_empty() || self.waiting { return; }
        // 哈基米梗彩蛋：不走 AI，直接唱跳
        if ["哈基米", "哈吉米", "南北绿豆", "曼波", "阿西嘎", "奶龙"].iter().any(|k| text.contains(k)) {
            self.act(Act::Chat);
            return self.play_song();
        }
        if text.contains("蜂蜜") {
            self.act(Act::Chat);
            return self.drink_honey();
        }
        // 「提醒我…」「记一下…」「我有什么待办」
        // 「记住…」「忘掉…」「你记得我什么」
        if self.mind_chat(&text) { return; }
        if self.memo_chat(&text) { return; }
        self.waiting = true;
        self.cmds.push(WinCmd::ChatInput(ChatInputCmd::Enable(false)));
        self.set_state(St::Think, None);
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
        if self.io { crate::services::ai::chat(&self.tx, text, ctx); }
    }

    pub(super) fn on_chat_reply(&mut self, r: ChatReply) {
        // 去掉情绪标签（气泡和 TTS 只拿干净文字）
        let (r, emo) = self.emotion_split(r);
        // 语音回答：先合成，字和声音一起出来（chatvoice.rs）；这期间保持思考中
        if r.ok && self.chat_voice_on() {
            self.cv_start(&r.text, emo);
            if self.cv.is_some() {
                // 第一段出字时再换表情（emotion_cv_reveal），整段念完 2 秒后回平时
                self.emotion_cv_pending(emo, r.text.chars().count());
                self.mind_after_reply(true);
                return;
            }
        }
        self.waiting = false;
        self.cmds.push(WinCmd::ChatInput(ChatInputCmd::Enable(true)));
        if r.ok {
            self.act(Act::Chat);
            self.play(Shot::Bounce);
            self.set_state(St::Happy, None);
            self.emotion_face(emo);
        } else {
            self.play(Shot::Shake);
            self.set_state(if r.need_key { St::Sit } else { St::Drag }, None);
        }
        // 字多就多停一会儿：2.5 秒起，每字 +0.15 秒，最多 20 秒
        let ms = (2500 + r.text.encode_utf16().count() as u64 * 150).min(20_000);
        // 语音回答关了：聊天回复只出字
        let voice = !(r.ok && self.chat_voice_disabled_by_user());
        self.say_opts(&r.text, ms, voice, if r.ok { SpeakKind::Chat } else { SpeakKind::Line });
        self.hold_timer(2500, StateAct::ChatIdle);
        self.emotion_hold(emo);
        self.mind_after_reply(r.ok);
    }
}
