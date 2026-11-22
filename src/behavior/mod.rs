//! 行为层（负责人：C 路）：移植 renderer.js 的状态机、台词、互动、提醒/摸鱼反应
//! 原版的 setTimeout/setInterval 全部换成「到期时间」，在 frame() 里统一推进。
pub mod fx;
pub mod lines;
pub mod play;
mod memo;
pub use memo::memo_voices;
mod sense;
mod external;
pub use sense::norm_birthday;
mod emotion;
mod skin;
pub mod beat;
pub mod dance;
mod live;
mod chatvoice;
mod chat;
mod menu;
mod mouse;
mod routine;
mod scene;

use crate::core::stats::{Act, ActResult, MoodLevel, Stats};
use crate::render::{BubbleView, Rect, Scene, StatusView, Xf, ZGlyph, anim};
use crate::types::{
    AppEvent, Button, ChatInputCmd, ChatReply, EventSender, Face, MenuAction, MouseEv, MoyuEv,
    PomodoroEv, RemindKind, SpeakKind, VoiceMood, WinCmd,
};
use fx::{Fx, FxKind};
use std::time::Instant;

// ---------------------------------------------------------------- 状态
/// idle 待机 / walk 溜达 / sit 坐 / sleep 睡 / eat 吃 / happy 开心 / hungry 讨食 / drag 被拎 / fall 掉落 + v0.8 表情态
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum St {
    Idle, Walk, Sit, Sleep, Eat, Happy, Hungry, Think, Drag, Fall,
    Shy, Dizzy, Surprised, Cry, Yawn, Love, Hiss, Stare, Dance, Honey,
    Pout, Smug, Confused, Scared, Starry, Puff,
    Wink, Drool, Sweat, Cheer, Sleepy,
}

impl St {
    /// STATE_FACE
    pub fn face(self) -> Face {
        match self {
            St::Idle | St::Walk => Face::Idle,
            St::Sit => Face::Sit,
            St::Sleep => Face::Sleep,
            St::Eat => Face::Eat,
            St::Happy => Face::Happy,
            St::Hungry => Face::Hungry,
            St::Think => Face::Think,
            St::Drag | St::Fall => Face::Drag,
            St::Shy => Face::Shy,
            St::Dizzy => Face::Dizzy,
            St::Surprised => Face::Surprised,
            St::Cry => Face::Cry,
            St::Yawn => Face::Yawn,
            St::Love => Face::Love,
            St::Hiss => Face::Hiss,
            St::Stare => Face::Stare,
            St::Dance => Face::Dance,
            St::Honey => Face::Honey,
            St::Pout => Face::Pout,
            St::Smug => Face::Smug,
            St::Confused => Face::Confused,
            St::Scared => Face::Scared,
            St::Starry => Face::Starry,
            St::Puff => Face::Puff,
            St::Wink => Face::Wink,
            St::Drool => Face::Drool,
            St::Sweat => Face::Sweat,
            St::Cheer => Face::Cheer,
            St::Sleepy => Face::Sleepy,
        }
    }
    /// VOICE_MOOD
    fn voice_mood(self) -> Option<VoiceMood> {
        Some(match self {
            St::Shy | St::Love | St::Dance | St::Honey | St::Happy | St::Eat | St::Smug | St::Starry | St::Wink | St::Cheer => VoiceMood::Happy,
            St::Cry | St::Dizzy | St::Sit | St::Pout | St::Scared => VoiceMood::Sad,
            St::Yawn | St::Sleep | St::Sleepy => VoiceMood::Sleepy,
            St::Hiss | St::Drag | St::Puff => VoiceMood::Mad,
            St::Surprised | St::Stare | St::Sweat => VoiceMood::Calm,
            St::Hungry | St::Drool => VoiceMood::Hungry,
            St::Think | St::Confused => VoiceMood::Think,
            _ => return None,
        })
    }
}

/// 一次性反馈动画（play(cls)）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shot { Bounce, Squash, Shake, Jump }

impl Shot {
    fn dur(self) -> u64 { if self == Shot::Jump { 750 } else { 500 } }
}

/// #pet 当前生效的动画（CSS 层叠的结果）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PetAnim { Shot(Shot), Chase, Wiggle, Walk(bool), BreatheSlow, Still, Hiss, Dizzy, Breathe }

/// stateTimer 到期要做的事
#[derive(Clone, Copy, Debug, PartialEq)]
enum StateAct {
    Think,
    Set(St, Option<(f64, f64)>),
    DanceEnd,
    /// 聊天回复后：聊天框还开着就 idle 不计时，否则 idle + 计时
    ChatIdle,
}

/// 其余 setTimeout
#[derive(Clone, Debug, PartialEq)]
enum Later {
    LevelUp(&'static str),
    HoneyFast,
    HoneyFastEnd,
    Greet,
    Beg,
    Prewarm,
    /// 语音回答：等某一句合成超时（token）
    ChatVoiceTimeout(u64),
    /// 备忘提醒：点她之后确认「知道了」（等等看是不是双击）
    MemoAck,
    /// 备忘提醒没人理：第 n 次追问
    MemoNag(u8),
    /// 第一次见面的开场白：第 n 句
    Intro(u8),
    /// 下班提醒被全屏安静或忙碌挡住：稍后补说
    OffWorkRetry,
}

/// 透明度过渡（transition: opacity 0.25s）
#[derive(Clone, Copy, Debug)]
struct Fade { on: bool, from: f32, t: u64 }
impl Fade {
    const OFF: Fade = Fade { on: false, from: 0.0, t: 0 };
    fn value(&self, now: u64) -> f32 {
        let p = ((now.saturating_sub(self.t)) as f32 / 250.0).min(1.0);
        let to = if self.on { 1.0 } else { 0.0 };
        if p >= 1.0 { return to; }
        self.from + (to - self.from) * anim::ease(anim::Ease::Ease, p)
    }
    fn set(&mut self, on: bool, now: u64) {
        if self.on == on { return; }
        *self = Fade { on, from: self.value(now), t: now };
    }
    fn moving(&self, now: u64) -> bool { now < self.t + 250 }
}

struct Bubble {
    text: String,
    shown: u64,
    hide_at: Option<u64>,
    /// 估计的展示时长（滚动用）
    dur: u64,
    /// 这条台词对应的语音请求
    token: u64,
    /// 用户用滚轮手动翻到的位置（0..=1），None = 自动慢滚
    manual_scroll: Option<f32>,
}

impl Bubble {
    fn auto_scroll(&self, now: u64) -> f32 {
        let span = self.dur.saturating_sub(3000).max(1000) as f32;
        ((now.saturating_sub(self.shown) as f32 - 1500.0) / span).clamp(0.0, 1.0)
    }

    fn scroll_at(&self, now: u64, overflow: bool) -> f32 {
        if !overflow {
            return 0.0;
        }
        self.manual_scroll.unwrap_or_else(|| self.auto_scroll(now))
    }
}

struct Down { sx: i32, sy: i32, lx: Option<i32>, dir: i32, flips: Vec<u64> }

#[derive(Default)]
struct Rub { last: f32, dir: i32, flips: Vec<u64>, cool: u64 }

struct Dance { next_beat: u64, beats: u32, flip: bool }

/// 行为层能看到的渲染结果（由 app 在每次 render 后回填）
#[derive(Clone, Copy, Debug, Default)]
pub struct UiInfo {
    pub bubble: Option<Rect>,
    pub status: Option<Rect>,
    pub bubble_overflow: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Zone { Pet, Bubble, Status }

const IDLE_FRAME_MS: u32 = 200;
const SLEEP_FRAME_MS: u32 = 100;
const ANIM_FRAME_MS: u32 = 33;
/// 默认（60fps）时拖动的帧间隔；运行时按 config::frame_ms()，这里留给测试作参照
#[allow(dead_code)]
const DRAG_FRAME_MS: u32 = 16;
const PET_TOP: f32 = 360.0 - 293.0;

fn rand(a: f64, b: f64) -> f64 { a + fastrand::f64() * (b - a) }
fn pick(arr: &[&str]) -> String { arr[fastrand::usize(..arr.len())].to_string() }
fn pickv(v: Vec<String>) -> String { let i = fastrand::usize(..v.len()); v.into_iter().nth(i).unwrap_or_default() }

/// 当地时间的小时（凌晨 0~6 点算夜里）
fn local_hour() -> u16 {
    #[cfg(windows)]
    unsafe { windows::Win32::System::SystemInformation::GetLocalTime().wHour }
    #[cfg(not(windows))]
    12
}

pub struct Behavior {
    tx: EventSender,
    pub scene: Scene,
    /// false = 不碰磁盘/网络/音频（单元测试）
    io: bool,
    /// 测试用：记录对外调用
    pub calls: Vec<String>,
    t0: Instant,
    now: u64,
    scale: f32,
    ui: UiInfo,
    cmds: Vec<WinCmd>,

    stats: Stats,
    saved_existed: bool,
    /// 第一次见面（没有存档）：开场多说几句，教怎么玩、去哪填 Key
    intro: bool,
    away_min: f64,
    next_tick: u64,

    pub state: St,
    face: Face,
    walk_dir: i32,
    walk_frame: usize,
    next_walk_frame: u64,
    state_timer: Option<(u64, StateAct)>,
    laters: Vec<(u64, Later)>,
    next_blink: u64,
    /// 伪 Live2D：眨眼排程、口型包络、呼吸相位（behavior/live.rs）
    live: live::LiveState,

    shot: Option<(Shot, u64)>,
    chase: Option<u64>,
    dragging_cls: bool,
    pet_anim: (PetAnim, u64),
    mood_cls: MoodLevel,
    flip_target: bool,
    flip_from: f32,
    flip_t: u64,
    sleep_since: Option<u64>,
    fx: Fx,
    fast_until: u64,
    glow: bool,
    dance: Option<Dance>,
    /// 右键「跳个舞」的整支舞（behavior/dance.rs）
    routine: Option<dance::Routine>,
    /// 跟着外面在放的歌跳：后台节拍线程的结果槽
    listen: Option<beat::LiveFeed>,
    /// 上次看到的今日粮食数（token 用量喂饱食度用；None = 还没看过）
    food_seen: Option<u64>,

    bubble: Option<Bubble>,
    bubble_fade: Fade,
    bubble_hover: bool,
    bubble_top: f32,
    last_bubble_text: String,
    status: Option<StatusView>,
    status_hide_at: Option<u64>,
    status_fade: Fade,

    down: Option<Down>,
    dragging: bool,
    shaken: bool,
    land_shaken: bool,
    rub: Rub,
    pokes: Vec<u64>,
    angry_until: u64,
    last_beg: u64,

    chatting: bool,
    /// 测试用：io=false 时假装输入框里有字（真机读 platform::chat_has_text）
    pub(crate) fake_chat_text: bool,
    /// 测试用：io=false 时假装的语音音量（真机读 services::audio::level）
    pub(crate) fake_level: f32,
    waiting: bool,
    /// 当前正在等待的异步请求序号（0 = 没有），用于精准丢弃 Esc 取消后的旧回复
    active_chat_req: u64,
    /// 测试兼容（io=false 时外部直接注入无序号 ChatReply）：待丢弃的已取消回复个数
    drop_replies: u32,
    pomo_running: bool,

    token_seq: u64,
    voice_req: u64,
    voice_playing: Option<u64>,
    song: Option<u64>,

    costume: String,
    outfits: Vec<String>,

    /// 新互动（behavior/play.rs）
    play: play::Play,
    memo: memo::MemoUi,
    sense: sense::SenseUi,
    /// 外部事件（忙的时候先存着）
    ext: external::ExtUi,
    /// 情绪表情 + 长期记忆（behavior/emotion.rs）
    mind: emotion::MindUi,
    /// 语音回答进行中（behavior/chatvoice.rs）
    cv: Option<chatvoice::ChatVoice>,
    /// 测试用：io=false 时假装「语音回答」开着且 TTS 可用 / 用户关了语音回答
    pub(crate) fake_chat_voice: bool,
    pub(crate) fake_chat_voice_off: bool,
    /// 测试用：io=false 时模拟的 (语音音量%, 伴奏音量%, 是否静音)
    pub(crate) sim_vol: (u8, u8, bool),
    /// 这次运行里已经提醒过「去设置里填 Key」
    key_hint_shown: bool,
    /// 被全屏安静/忙碌挡住的下班提醒截止时刻（过期后不再补说）
    pending_off_work: Option<u64>,
}

impl Behavior {
    pub fn new(tx: EventSender) -> Self {
        let saved = crate::core::save::load();
        let existed = saved.is_some();
        let (stats, away) = Stats::load(saved);
        let cfg = crate::core::config::get();
        let mut b = Self::with(tx, stats, existed, away, cfg.costume, cfg.outfit, true);
        // 角色包：扫描 skins 目录，按 save.json 的 skin 字段选中（只读清单，图片用到才解码）
        crate::core::skin::init(&crate::core::skin::saved_id(&b.stats.data.extra));
        b.save();
        b
    }

    /// 不读磁盘的构造（测试和 new 共用）
    pub fn with(tx: EventSender, stats: Stats, saved_existed: bool, away_min: f64, costume: String, outfits: Vec<String>, io: bool) -> Self {
        let costume = crate::core::costume::normalize(&costume).to_string();
        let mut b = Self {
            tx,
            scene: Scene::default(),
            io,
            calls: Vec::new(),
            t0: Instant::now(),
            now: 0,
            scale: 1.0,
            ui: UiInfo::default(),
            cmds: Vec::new(),
            stats,
            saved_existed,
            intro: !saved_existed && io,
            away_min,
            next_tick: 20_000,
            state: St::Idle,
            face: Face::Idle,
            walk_dir: 1,
            walk_frame: 0,
            next_walk_frame: 0,
            state_timer: None,
            laters: Vec::new(),
            next_blink: 0,
            live: live::LiveState::new(0),
            shot: None,
            chase: None,
            dragging_cls: false,
            pet_anim: (PetAnim::Breathe, 0),
            mood_cls: MoodLevel::Normal,
            flip_target: false,
            flip_from: 1.0,
            flip_t: 0,
            sleep_since: None,
            fx: Fx::default(),
            fast_until: 0,
            glow: false,
            dance: None,
            routine: None,
            listen: None,
            food_seen: None,
            bubble: None,
            bubble_fade: Fade::OFF,
            bubble_hover: false,
            bubble_top: 0.0,
            last_bubble_text: String::new(),
            status: None,
            status_hide_at: None,
            status_fade: Fade::OFF,
            down: None,
            dragging: false,
            shaken: false,
            land_shaken: false,
            rub: Rub::default(),
            pokes: Vec::new(),
            angry_until: 0,
            last_beg: 0,
            chatting: false,
            fake_chat_text: false,
            fake_level: 0.0,
            waiting: false,
            active_chat_req: 0,
            drop_replies: 0,
            pomo_running: false,
            token_seq: 0,
            voice_req: 0,
            voice_playing: None,
            song: None,
            costume,
            outfits,
            play: play::Play::default(),
            cv: None,
            fake_chat_voice: false,
            fake_chat_voice_off: false,
            sim_vol: (90, 80, false),
            key_hint_shown: false,
            pending_off_work: None,
            memo: memo::MemoUi::default(),
            sense: sense::SenseUi::default(),
            ext: external::ExtUi::default(),
            mind: emotion::MindUi::default(),
        };
        b.mood_cls = b.stats.mood();
        b.next_blink = b.live.blink.next;
        b.later(600, Later::Greet);
        b.later(5000, Later::Prewarm);
        b.build_scene();
        b
    }

    fn clock(&self) -> u64 { self.t0.elapsed().as_millis() as u64 }

    pub fn set_scale(&mut self, s: f32) { if s.is_finite() && s > 0.1 { self.scale = s; } }
    pub fn set_ui(&mut self, ui: UiInfo) { self.ui = ui; }
    pub fn set_bubble_top(&mut self, top: f32) {
        let top = if top.is_finite() && top > 0.0 { top } else { 0.0 };
        self.bubble_top = top;
        self.scene.bubble_top = top;
    }

    // ============================================================ Handler 入口
    pub fn event(&mut self, ev: AppEvent, out: &mut Vec<WinCmd>) {
        let now = self.clock().max(self.now);
        self.event_at(ev, now, out);
    }

    pub fn frame(&mut self, now_ms: u64, out: &mut Vec<WinCmd>) -> u32 {
        let _ = now_ms; // 用自己的单调时钟，事件和帧共用同一时间基准
        let now = self.clock().max(self.now);
        self.frame_at(now, out)
    }

    pub fn quit(&mut self) {
        self.cancel_routine();
        if self.state == St::Dance {
            self.dance = None;
            self.stop_song();
        }
        self.save();
        if self.io { crate::services::audio::stop(); }
    }

    pub fn event_at(&mut self, ev: AppEvent, now: u64, out: &mut Vec<WinCmd>) {
        self.now = now;
        self.run_timers();
        match ev {
            AppEvent::Mouse(m) => {
                if matches!(
                    m,
                    MouseEv::Down { .. } | MouseEv::Up { .. } | MouseEv::DoubleClick { .. } | MouseEv::Wheel { .. } | MouseEv::CtrlWheel { .. }
                ) {
                    self.sense_touch();
                }
                self.on_mouse(m)
            }
            AppEvent::Menu(a) => {
                self.sense_touch();
                self.on_menu(a)
            }
            AppEvent::ChatSubmit(t) => {
                self.sense_touch();
                self.on_chat_submit(t)
            }
            AppEvent::ChatCancel => self.close_chat(),
            AppEvent::ChatStop => self.chat_stop(),
            AppEvent::ScreenRoast(png) => self.on_roast(png),
            AppEvent::RoastNote(t) => {
                if !self.memo_ringing() {
                    self.play(Shot::Shake);
                    self.fx(FxKind::Sweat, 1);
                    self.set_state(St::Sweat, None);
                    self.say_opts(&t, 3200, false, SpeakKind::Line);
                    self.hold_timer(2400, StateAct::Set(St::Idle, Some((5000.0, 9000.0))));
                }
            }
            AppEvent::ChatExported(res) => {
                if !self.memo_ringing() {
                    let l = match res {
                        Ok(()) => "聊天记录导出好啦，在「文档\\哈基米」里～",
                        Err(true) => "还没有聊天记录可以导出喵",
                        Err(false) => "导出聊天记录失败了喵…看看磁盘或权限？",
                    };
                    self.say_opts(l, 3200, false, SpeakKind::Line);
                }
            }
            AppEvent::ChatReply(r) => self.on_chat_reply_id(0, r),
            AppEvent::ChatReplyId(req_id, r) => self.on_chat_reply_id(req_id, r),
            AppEvent::Speech { token, audio } if self.cv_wants(token) => self.cv_on_speech(token, audio),
            AppEvent::Speech { token, audio } => {
                self.emotion_speech(token, audio.is_some());
                if token != self.voice_req || token == 0 { return self.flush(out); }
                let Some(a) = audio else { return self.flush(out) };
                let same = self.bubble.as_ref().is_some_and(|b| b.token == token) && self.bubble_fade.on;
                if !same { return self.flush(out); }
                self.call(format!("play:{token}"));
                if self.io {
                    let vol = crate::core::config::get().tts.voice_vol();
                    crate::services::audio::play(token, a, vol);
                }
                self.voice_playing = Some(token);
                // 气泡至少留到念完再多 1 秒（AudioDone 时定），最多 60 秒兜底
                if let Some(b) = &mut self.bubble { b.hide_at = Some(now + 60_000); }
            }
            AppEvent::AudioDone { token } => {
                self.emotion_audio_done(token);
                let tail = self.memo.bubble_tail(token, now);
                if self.voice_playing == Some(token) {
                    self.voice_playing = None;
                    if let Some(b) = &mut self.bubble
                        && b.token == token
                        && self.bubble_fade.on
                    {
                        b.hide_at = Some(now + tail);
                    }
                }
                // 跳个舞：音乐提前停了就换内置鼓点接着跳
                if self.song == Some(token) && self.routine.is_some() {
                    self.song = None;
                    self.dance_music_done();
                }
                if self.song == Some(token) {
                    self.song = None;
                    if self.state == St::Dance { self.end_dance(); }
                }
                self.memo_audio_done(token);
                self.cv_on_audio_done(token);
                self.emotion_cv_done();
            }
            AppEvent::Remind(k) => self.on_remind(k),
            AppEvent::Pomodoro(p) => self.on_pomodoro(p),
            AppEvent::Moyu(m) => self.on_moyu(m),
            AppEvent::Landed { big } => self.on_landed(big),
            AppEvent::WalkDone { hit_edge } => {
                let pouncing = std::mem::take(&mut self.sense.pouncing);
                if self.state == St::Walk {
                    if pouncing {
                        self.play(Shot::Bounce);
                        self.fx(FxKind::Spark, 5);
                        self.react(St::Starry, 1600, None);
                    } else {
                        if hit_edge { self.say(&pick(lines::EDGE), 2000); }
                        self.set_state(St::Idle, Some((4000.0, 9000.0)));
                    }
                }
            }
            AppEvent::Peek => {
                self.reset_aerial_flags();
                let line = (!self.memo_ringing()).then(|| pick(lines::PEEK));
                self.react(St::Happy, 1500, line);
            }
            AppEvent::SecondInstance => {
                self.reset_aerial_flags();
                self.cmds.push(WinCmd::Show);
                let line = (!self.memo_ringing()).then(|| pick(lines::PEEK));
                self.react(St::Happy, 1500, line);
            }
            AppEvent::Cursor { x, y } => self.on_cursor(x, y),
            AppEvent::Weather(w) => self.on_weather(w),
            AppEvent::Toy(t) => self.on_toy(t),
            AppEvent::Perched(k) => self.on_perched(k),
            AppEvent::ClimbFailed => self.on_climb_failed(),
            AppEvent::PerchLost => self.on_perch_lost(),
            AppEvent::Memo(m) => self.on_memo(m),
            AppEvent::Mind(m) => self.on_mind(m),
            AppEvent::Sense(e) => self.on_sense(e),
            AppEvent::External(e) => self.on_external(e),
        }
        self.sync_play();
        self.build_scene();
        self.flush(out);
    }

    pub fn frame_at(&mut self, now: u64, out: &mut Vec<WinCmd>) -> u32 {
        self.now = now;
        self.run_timers();
        self.sense_frame();
        self.external_frame();
        self.dance_tick();
        self.sync_play();
        self.build_scene();
        self.flush(out);
        self.next_interval()
    }

    fn flush(&mut self, out: &mut Vec<WinCmd>) { out.append(&mut self.cmds); }

    fn call(&mut self, s: String) {
        if self.io { crate::core::log::trace(&s) } else if self.calls.len() < 500 { self.calls.push(s) }
    }

    fn save(&mut self) { if self.io { crate::core::save::store(&self.stats.data); } }

    fn later(&mut self, ms: u64, l: Later) { self.laters.push((self.now + ms, l)); }

    fn hold_timer(&mut self, ms: u64, a: StateAct) { self.state_timer = Some((self.now + ms, a)); }

    fn next_token(&mut self) -> u64 { self.token_seq += 1; self.token_seq }

    // ============================================================ 计时器
    fn run_timers(&mut self) {
        let now = self.now;
        for _ in 0..64 {
            // 找最早到期的一个
            let mut best: Option<(u64, u8)> = None;
            let mut consider = |due: u64, k: u8| if due <= now && best.is_none_or(|(d, _)| due < d) { best = Some((due, k)); };
            if let Some((d, _)) = self.state_timer { consider(d, 0); }
            if let Some(d) = self.laters.iter().map(|l| l.0).min() { consider(d, 1); }
            consider(self.next_blink, 2);
            consider(self.next_tick, 3);
            if self.state == St::Walk { consider(self.next_walk_frame, 4); }
            if let Some(d) = &self.dance { consider(d.next_beat, 5); }
            if let Some(h) = self.bubble.as_ref().and_then(|b| b.hide_at) { consider(h, 6); }
            if let Some(h) = self.status_hide_at { consider(h, 7); }
            if let Some(d) = self.play.next_due() { consider(d, 8); }
            let Some((due, k)) = best else { break };
            // 计时器按「到期那一刻」执行（帧间隔可能比计时器粗）
            self.now = due;
            match k {
                0 => {
                    if let Some((_, a)) = self.state_timer.take() { self.on_state_timer(a); }
                }
                1 => {
                    if let Some(i) = self.laters.iter().enumerate().min_by_key(|(_, l)| l.0).map(|(i, _)| i) {
                        let (_, l) = self.laters.remove(i);
                        self.on_later(l);
                    }
                }
                2 => self.on_blink(now),
                3 => self.on_tick(now),
                // 走路换帧跟着整体动画的颠步走（live.rs）
                4 => self.walk_tick(now),
                5 => self.on_dance_beat(),
                6 => {
                    // 光标正指着气泡读字时暂停消失（用最新光标坐标兜底校验，防透明区未发 Move 导致卡住），移开后再留一会儿
                    let over = self.bubble_hover
                        && self.ui.bubble.is_some_and(|r| {
                            self.play.cursor.is_none_or(|(cx, cy, _)| r.contains(cx, cy))
                        });
                    if over && !self.memo_ringing() {
                        if let Some(b) = &mut self.bubble { b.hide_at = Some(now + 1800); }
                    } else {
                        self.bubble_hover = false;
                        if let Some(b) = &mut self.bubble { b.hide_at = None; }
                        self.bubble_fade.set(false, due);
                    }
                }
                7 => {
                    self.status_hide_at = None;
                    self.status_fade.set(false, due);
                }
                8 => self.on_play_timer(),
                _ => break,
            }
        }
        self.now = now;
    }

    fn next_due(&self) -> Option<u64> {
        let mut best = self.next_blink.min(self.next_tick);
        if let Some((d, _)) = self.state_timer { best = best.min(d); }
        for l in &self.laters { best = best.min(l.0); }
        if self.state == St::Walk { best = best.min(self.next_walk_frame); }
        if let Some(d) = &self.dance { best = best.min(d.next_beat); }
        if let Some(h) = self.bubble.as_ref().and_then(|b| b.hide_at) { best = best.min(h); }
        if let Some(h) = self.status_hide_at { best = best.min(h); }
        if let Some(d) = self.play.next_due() { best = best.min(d); }
        Some(best)
    }

    fn animating(&self) -> bool {
        let now = self.now;
        self.shot.is_some_and(|(s, t)| now < t + s.dur())
            || self.play_animating()
            || self.chase.is_some_and(|t| now < t + 1800)
            || self.dragging_cls
            || matches!(self.state, St::Walk | St::Dance | St::Fall)
            || (self.state == St::Hiss && now < self.pet_anim.1 + 1000)
            || (self.state == St::Dizzy && now < self.pet_anim.1 + 3000)
            || !self.fx.is_empty()
            || self.bubble_fade.moving(now)
            || self.status_fade.moving(now)
            || now < self.flip_t + 200
            || (self.ui.bubble_overflow && self.bubble_fade.on && self.bubble.as_ref().is_some_and(|b| b.manual_scroll.is_none()))
    }

    fn next_interval(&self) -> u32 {
        // 跳舞、拖动、灵动待机都按设置里的帧率（默认 60）
        if self.routine.is_some() { return crate::core::config::frame_ms(); }
        let base = if self.dragging_cls { crate::core::config::frame_ms() }
            else if self.animating() { ANIM_FRAME_MS }
            else if self.state == St::Sleep { SLEEP_FRAME_MS }
            else { IDLE_FRAME_MS };
        let base = base.min(self.live_interval());
        match self.next_due() {
            Some(d) if d > self.now => base.min((d - self.now).min(u32::MAX as u64) as u32).max(1),
            Some(_) => 1,
            None => base,
        }
    }

    fn on_state_timer(&mut self, a: StateAct) {
        match a {
            StateAct::Think => self.think(),
            StateAct::Set(s, hold) => self.set_state(s, hold),
            StateAct::DanceEnd => self.end_dance(),
            StateAct::ChatIdle => {
                if self.chatting {
                    self.set_state(St::Idle, self.io.then_some((25_000.0, 35_000.0)));
                } else {
                    self.set_state(St::Idle, Some((5000.0, 9000.0)));
                }
            }
        }
    }

    /// 当前是否正忙于高优先级状态（真实拖拽/下落/备忘响铃/小游戏/跳舞）
    /// 注意：不检查 St::Drag（该枚举也被番茄钟摸鱼/重摔表情复用），物理拖拽以 self.dragging 为准
    pub(super) fn is_busy_for_interruption(&self) -> bool {
        self.dragging
            || self.state == St::Fall
            || self.memo_ringing()
            || self.routine.is_some()
            || self.play.game.is_some()
    }

    fn on_later(&mut self, l: Later) {
        match l {
            Later::LevelUp(name) => {
                if self.dragging || matches!(self.state, St::Drag | St::Fall) || self.memo_ringing() {
                    return;
                }
                self.react(St::Starry, 3000, Some(pickv(lines::level_up(name))));
                self.fx(FxKind::Spark, 8);
                self.fx(FxKind::Heart, 4);
            }
            Later::HoneyFast => {
                self.fast_until = self.now + 60_000;
                self.glow = true;
                self.later(60_000, Later::HoneyFastEnd);
                if self.is_busy_for_interruption() || self.chatting || self.waiting {
                    return;
                }
                self.say(&pick(lines::FAST), 2000);
                self.fx(FxKind::Spark, 6);
                self.go_walk();
            }
            Later::HoneyFastEnd => {
                if self.now >= self.fast_until {
                    self.glow = false;
                    if !self.memo_ringing() && !self.sense.quiet {
                        self.say(&pick(lines::SLOW), 2000);
                    }
                }
            }
            Later::Greet if self.intro => {
                self.react(St::Starry, 3500, Some(lines::INTRO[0].to_string()));
                self.fx(FxKind::Heart, 4);
                self.later(5000, Later::Intro(1));
            }
            Later::Intro(n) => {
                let n = n as usize;
                // 第三句教填 Key：已经填了就不说
                let has_key = !self.io || !crate::core::config::ai_key_missing(&crate::core::config::get());
                if n >= lines::INTRO.len() || (n == lines::INTRO.len() - 1 && has_key && self.io) {
                    self.intro = false;
                    return;
                }
                if self.is_busy_for_interruption() || self.chatting {
                    self.later(3000, Later::Intro(n as u8));
                    return;
                }
                self.react(St::Happy, 4500, Some(lines::INTRO[n].to_string()));
                self.later(5500, Later::Intro(n as u8 + 1));
            }
            Later::Greet => {
                if self.is_busy_for_interruption() || self.sense.quiet {
                    return;
                }
                // 先判节日与深夜，避免节日/深夜问候白白消耗早/午/晚问候槽位
                let fest = self.festive_line();
                let line = if let Some(f) = fest.clone() {
                    f
                } else if Self::is_night() {
                    pick(lines::NIGHT)
                } else if let Some(l) = self.greet_slot_line() {
                    l
                } else if self.saved_existed && self.away_min >= 60.0 {
                    pickv(lines::back(&lines::fmt_away(self.away_min)))
                } else {
                    pick(lines::HELLO)
                };
                let st = if fest.is_some() || (self.saved_existed && self.away_min >= 60.0) { St::Starry } else { St::Happy };
                self.react(st, 1500, Some(line));
                if self.stats.data.hunger < 30.0 { self.later(3000, Later::Beg); }
            }
            Later::Beg => {
                if !self.is_busy_for_interruption() && !self.chatting && !self.waiting && !self.sense.quiet {
                    self.go_beg();
                }
            }
            Later::Prewarm => {
                let items = lines::prewarm_items();
                self.call(format!("prewarm:{}", items.len()));
                if self.io { crate::services::tts::prewarm(items); }
            }
            Later::ChatVoiceTimeout(t) => self.cv_timeout(t),
            Later::MemoAck => self.memo_ack(),
            Later::MemoNag(n) => self.memo_nag(n),
            Later::OffWorkRetry => { self.flush_off_work(); }
        }
    }

    // 眨眼（on_blink）：2~6 秒一次、偶尔连眨两下，用眼部贴片淡入淡出 → behavior/live.rs

    /// 每 20 秒结算一次数值，并处理「饿了主动讨食」「睡饱自己醒」
    fn on_tick(&mut self, real_now: u64) {
        // 用真实当前时间续排，防止电脑休眠醒来后补跑上千次 tick 和上千次 save.json 写盘
        self.next_tick = real_now.max(self.now) + 20_000;
        self.stats.tick(self.state == St::Sleep);
        if self.io {
            crate::core::usage::add_seconds(20);
            let food = crate::core::usage::today().food();
            self.feed_from_usage(food);
        }
        self.save();
        let (hunger, energy) = (self.stats.data.hunger, self.stats.data.energy);
        let free = matches!(self.state, St::Idle | St::Sit | St::Walk);
        if self.state == St::Sleep && energy >= 100.0 && hunger >= 15.0 {
            self.say(&pick(lines::WAKE_UP), 2600);
            self.play(Shot::Bounce);
            self.set_state(St::Idle, Some((5000.0, 9000.0)));
        } else if self.state == St::Sleep && hunger < 15.0 {
            self.say(lines::WAKE_HUNGRY, 2600);
            self.go_beg();
        } else if free && hunger < 30.0 && self.now.saturating_sub(self.last_beg) > 120_000 {
            self.go_beg();
        } else {
            self.tick_play();
        }
        self.tick_mind();
    }

    // ============================================================ 基本动作
    fn show(&mut self, f: Face) { self.face = f; }

    fn set_state(&mut self, next: St, hold: Option<(f64, f64)>) {
        if self.io && next != self.state { crate::core::log::trace(&format!("state {:?} -> {next:?}", self.state)); }
        if next != St::Dance { self.cancel_routine(); }
        if next != St::Walk { self.sense.pouncing = false; }
        if self.state == St::Walk && next != St::Walk { self.cmds.push(WinCmd::WalkStop); }
        let prev = self.state;
        self.state = next;
        self.state_timer = None;
        self.set_flip(next == St::Walk && self.walk_dir < 0);
        if next == St::Sleep { if prev != St::Sleep || self.sleep_since.is_none() { self.sleep_since = Some(self.now); } } else { self.sleep_since = None; }
        self.show(next.face());
        if next == St::Walk && prev != St::Walk {
            self.walk_frame = 0;
            self.next_walk_frame = self.now + 160;
        }
        self.mood_cls = self.stats.mood();
        if let Some((lo, hi)) = hold { self.hold_timer(rand(lo, hi) as u64, StateAct::Think); }
    }

    fn set_flip(&mut self, on: bool) {
        if self.flip_target == on { return; }
        self.flip_from = self.flip_value();
        self.flip_target = on;
        self.flip_t = self.now;
    }

    fn flip_value(&self) -> f32 {
        let to = if self.flip_target { -1.0 } else { 1.0 };
        let p = (self.now.saturating_sub(self.flip_t) as f32 / 200.0).min(1.0);
        if p >= 1.0 { return to; }
        self.flip_from + (to - self.flip_from) * anim::ease(anim::Ease::Ease, p)
    }

    fn play(&mut self, s: Shot) { self.shot = Some((s, self.now)); }

    /// 互动打断当前状态，表演完回到待机再继续自主行为
    fn react(&mut self, st: St, ms: u64, line: Option<String>) {
        self.play(if st == St::Eat { Shot::Bounce } else { Shot::Squash });
        self.set_state(st, None);
        if let Some(l) = line { self.say(&l, 2600); }
        self.hold_timer(ms, StateAct::Set(St::Idle, Some((5000.0, 10000.0))));
    }

    /// 互动并检查亲密度是否升级
    fn act(&mut self, kind: Act) -> ActResult {
        let before = self.stats.level().0;
        self.maybe_checkin(kind);
        let r = self.stats.act(kind);
        if self.io && kind == Act::Feed && r != ActResult::Full {
            crate::core::usage::add_feed();
        }
        let (idx, name) = self.stats.level();
        if idx > before { self.later(2700, Later::LevelUp(name)); }
        self.save();
        r
    }

    fn is_night() -> bool { local_hour() < 6 }

    // ============================================================ 气泡 & 语音
    pub(super) fn show_bubble(&mut self, text: String, hide_at: Option<u64>, dur: u64, token: u64, remember: bool) {
        if remember && !text.trim().is_empty() {
            self.last_bubble_text = text.clone();
            if self.io {
                crate::platform::set_has_copyable_bubble(true);
            }
        }
        self.bubble = Some(Bubble { text, shown: self.now, hide_at, dur, token, manual_scroll: None });
        self.bubble_fade.set(true, self.now);
    }

    pub(super) fn dismiss_bubble(&mut self) {
        self.bubble_hover = false;
        if let Some(b) = &mut self.bubble {
            b.hide_at = None;
        }
        self.bubble_fade.set(false, self.now);
        if !self.memo_voice_busy(0) {
            self.stop_voice();
        }
    }

    fn say(&mut self, text: &str, ms: u64) { self.say_opts(text, ms, true, SpeakKind::Line); }

    fn say_opts(&mut self, text: &str, ms: u64, voice: bool, kind: SpeakKind) {
        self.cv_interrupted();
        // 角色包的 lines 替换固定台词（聊天回复不换）
        let over = if kind == SpeakKind::Line && self.io { skin::line_override(text) } else { None };
        let text = over.as_deref().unwrap_or(text);
        let token = if voice { self.next_token() } else { 0 };
        self.show_bubble(text.to_string(), Some(self.now + ms), ms, token, true);
        if voice {
            self.voice(text, kind, token);
        } else if !self.memo_voice_busy(0) {
            self.stop_voice();
        }
    }

    /// 只换气泡、不出声，也不掐正在念的台词（小游戏里抓到/扑空的短句，别把开场白打断）
    fn say_bubble(&mut self, text: &str, ms: u64) {
        self.show_bubble(text.to_string(), Some(self.now + ms), ms, 0, true);
    }

    /// 气泡先出，声音随后到；新台词会打断旧的
    fn voice(&mut self, text: &str, kind: SpeakKind, token: u64) {
        if self.memo_voice_busy(token) || self.sense_mute(token) { return; }
        self.stop_voice();
        if self.routine.is_some() { return; } // 跳舞时不念（内置鼓点走语音声道）；放歌时照念，audio 混音、压低音乐
        self.voice_req = token;
        let mood = self.voice_mood();
        self.call(format!("speak:{text}"));
        if self.io { crate::services::tts::speak(&self.tx, token, text.to_string(), Some(mood), kind); }
    }

    fn stop_voice(&mut self) {
        if self.voice_req != 0 && self.io {
            crate::services::tts::cancel_active();
        }
        self.voice_req = 0;
        if let Some(tok) = self.voice_playing.take() {
            self.call("stop".into());
            if self.io { crate::services::audio::stop_token(tok); }
            self.emotion_audio_done(tok);
            let ringing = self.memo_ringing();
            if let Some(b) = &mut self.bubble
                && b.token == tok
                && self.bubble_fade.on
                && !ringing
            {
                b.hide_at = Some(self.now + 1500);
            }
        }
    }

    pub(super) fn reset_aerial_flags(&mut self) {
        self.play.perch = None;
        self.play.climbing = None;
        self.play.jumping_down = false;
        self.sense.pounce_after_land = false;
        self.dragging = false;
        self.dragging_cls = false;
        self.land_shaken = false;
        self.shaken = false;
    }

    pub(super) fn cleanup_for_hide(&mut self) {
        self.cancel_routine();
        if self.state == St::Dance {
            self.end_dance();
        }
        if !self.memo_voice_busy(0) {
            self.stop_voice();
        }
        self.cv_disable_tts();
        if self.play.game.take().is_some() || self.play.food.take().is_some() {
            self.cmds.push(WinCmd::Toy(crate::types::ToyCmd::Remove));
        }
        self.sense.pounce_after_land = false;
    }

    fn voice_mood(&self) -> VoiceMood {
        if let Some(m) = self.state.voice_mood() { return m; }
        let d = &self.stats.data;
        if d.mood < 30.0 { VoiceMood::Sad } else if d.energy < 15.0 { VoiceMood::Sleepy } else { VoiceMood::Calm }
    }

    // ============================================================ 状态卡
    fn show_status(&mut self) {
        let moyu = if self.io { crate::platform::moyu_today() } else { None };
        let d = &self.stats.data;
        self.status = Some(StatusView {
            days: self.stats.days(),
            hunger: d.hunger as f32,
            mood: d.mood as f32,
            energy: d.energy as f32,
            level: self.stats.level().1.to_string(),
            affection: d.affection,
            moyu,
            checkin: Some((self.stats.data.checkin_streak, self.stats.data.checkin_total)),
            weather: self.weather_text(),
            opacity: 1.0,
        });
        self.status_fade.set(true, self.now);
        self.status_hide_at = Some(self.now + 5000);
    }

    // ============================================================ 自主行为
    /// 空闲时自己决定下一步做什么，受数值影响
    fn think(&mut self) {
        if matches!(self.state, St::Drag | St::Fall) || self.chat_busy() { return; }
        if self.chatting && self.io && crate::platform::chat_is_focused() {
            self.hold_timer(25_000, StateAct::Think);
            return;
        }
        // 备忘提醒还响着：原地等你点她，不换动作不说闲话（不然 60 秒的提醒气泡十几秒就被盖掉）
        if self.memo_ringing() { return self.set_state(St::Idle, Some((5000.0, 9000.0))); }
        self.yield_chat();
        if self.think_play() { return; }
        let m = self.stats.mood();
        let energy = self.stats.data.energy;
        let r = fastrand::f64();
        if matches!(m, MoodLevel::Hungry | MoodLevel::Starving) {
            if r < 0.6 { return self.go_beg(); }
            return self.set_state(St::Sit, Some((10000.0, 20000.0)));
        }
        if m == MoodLevel::Exhausted { return self.go_sleep(); }
        if self.state == St::Sit {
            let sleep_chance = if Self::is_night() { 0.6 } else if energy < 40.0 { 0.5 } else { 0.2 };
            if r < sleep_chance { return self.go_sleep(); }
            return self.set_state(St::Idle, Some((6000.0, 12000.0)));
        }
        // 彩蛋：心情好时偶尔追头顶的小星星，原地转圈
        if m == MoodLevel::Great && self.state == St::Idle && r < 0.08 { return self.go_chase(); }
        // 丰富的待机微表情与小动作：犯困点头、伸懒腰、馋嘴流口水、俏皮眨眼、举爪打气、得意叉腰、歪头好奇、哼歌、盯——
        if self.state == St::Idle && r < 0.34 {
            let f = fastrand::f64();
            let hunger = self.stats.data.hunger;
            if f < 0.16 && (energy < 55.0 || Self::is_night()) {
                self.say(&pick(lines::SLEEPY_NOD), 2200);
                return self.react(St::Sleepy, 2400, None);
            }
            if f < 0.28 && energy < 75.0 {
                self.say(&pick(lines::YAWN), 1800);
                return self.react(St::Yawn, 1800, None);
            }
            if f < 0.42 && hunger < 68.0 {
                self.fx(FxKind::Spark, 3);
                self.say(&pick(lines::DROOL), 2400);
                return self.react(St::Drool, 2400, None);
            }
            if f < 0.56 {
                self.fx(FxKind::Heart, 2);
                self.say(&pick(lines::WINK), 2200);
                return self.react(St::Wink, 2000, None);
            }
            if f < 0.68 && m == MoodLevel::Great {
                self.play(Shot::Jump);
                self.fx(FxKind::Spark, 6);
                self.say(&pick(lines::CHEER), 2200);
                return self.react(St::Cheer, 2200, None);
            }
            if f < 0.78 && m == MoodLevel::Great {
                return self.go_dance(4200, Some(pick(lines::DANCE)));
            }
            if f < 0.89 {
                self.fx(FxKind::Question, 1);
                self.say(&pick(lines::IDLE_CURIOUS), 2200);
                return self.react(St::Confused, 2200, None);
            }
            self.say(&pick(lines::STARE), 2200);
            return self.react(St::Stare, 2400, None);
        }
        let walk_chance = match m { MoodLevel::Great => 0.55, MoodLevel::Sad => 0.2, _ => 0.45 };
        if r < walk_chance { return self.go_walk(); }
        if r < walk_chance + 0.25 {
            if m == MoodLevel::Sad && fastrand::f64() < 0.5 { self.say(&pick(lines::SAD), 2600); }
            else if fastrand::f64() < 0.3 { self.say(&pick(lines::SIT), 2600); }
            return self.set_state(St::Sit, Some((15000.0, 30000.0)));
        }
        if (Self::is_night() || energy < 25.0) && r < 0.9 { return self.go_sleep(); }
        self.set_state(St::Idle, Some((6000.0, 14000.0)));
    }

    fn go_walk(&mut self) {
        if self.play.perch == Some(crate::types::PerchKind::WindowTop) { return self.jump_down(); }
        self.play.perch = None;
        self.walk_dir = if fastrand::bool() { -1 } else { 1 };
        self.set_state(St::Walk, None);
        let fast = self.now < self.fast_until;
        if !fast && fastrand::f64() < 0.3 { self.say(&pick(lines::WALK), 2000); }
        if self.stats.mood() == MoodLevel::Great && fastrand::f64() < 0.5 { self.fx(FxKind::Note, 3); }
        let (dist, speed) = if fast { (rand(400.0, 900.0), 5.0) } else { (rand(120.0, 420.0), 1.6) };
        self.cmds.push(WinCmd::Walk { dir: self.walk_dir, distance: dist as f32 * self.scale, speed: speed * self.scale });
    }

    fn go_sleep(&mut self) {
        let l = if self.stats.data.energy < 15.0 { lines::TIRED } else { lines::SLEEPY };
        self.say(&pick(l), 2200);
        self.set_state(St::Yawn, None);
        self.hold_timer(1800, StateAct::Set(St::Sleep, Some((60000.0, 150000.0))));
    }

    fn go_chase(&mut self) {
        self.set_state(St::Happy, None);
        self.say(&pick(lines::CHASE), 2400);
        self.chase = Some(self.now);
        self.hold_timer(2000, StateAct::Set(St::Idle, Some((6000.0, 10000.0))));
    }

    fn go_beg(&mut self) {
        self.last_beg = self.now;
        self.play(Shot::Bounce);
        let l = if self.stats.data.hunger < 15.0 { lines::STARVING } else { lines::HUNGRY };
        self.say(&pick(l), 3500);
        self.set_state(St::Hungry, Some((8000.0, 14000.0)));
    }

    /// 哈基米之歌：原地跳舞（舞步图左右翻着跳），飘音符
    fn go_dance(&mut self, ms: u64, line: Option<String>) {
        // 若之前正在跳 Routine（如实时跟歌跳），set_state(St::Dance) 不会自动取消，这里先停干净
        self.cancel_routine();
        self.set_state(St::Dance, None);
        let l = line.unwrap_or_else(|| pick(lines::SONG));
        self.say(&l, ms);
        self.dance = Some(Dance { next_beat: self.now + 420, beats: 0, flip: false });
        self.hold_timer(ms, StateAct::DanceEnd);
    }

    fn on_dance_beat(&mut self) {
        if self.state != St::Dance {
            self.dance = None;
            self.set_flip(false);
            self.stop_song();
            return;
        }
        let now = self.now;
        let Some(d) = &mut self.dance else { return };
        d.flip = !d.flip;
        d.beats += 1;
        d.next_beat = if d.next_beat + 1000 < now { now + 420 } else { d.next_beat + 420 };
        let (flip, beats) = (d.flip, d.beats);
        self.set_flip(flip);
        self.play(Shot::Bounce);
        if beats % 2 == 1 { self.fx(FxKind::Note, 2); }
    }

    fn end_dance(&mut self) {
        self.dance = None;
        self.set_flip(false);
        self.stop_song();
        self.set_state(St::Idle, Some((5000.0, 9000.0)));
    }

    fn stop_song(&mut self) {
        if let Some(tok) = self.song.take() {
            self.call("stop".into());
            if self.io { crate::services::audio::stop_token(tok); }
        }
    }

    /// 放 songs 文件夹里的歌，边放边跳（最多 60 秒）；没放歌就用她自己的声音哼
    fn play_song(&mut self) {
        self.stop_song();
        let path = if self.io { crate::services::songs::pick() } else { None };
        let Some(path) = path else { return self.go_dance(7000, None) };
        let tok = self.next_token();
        self.song = Some(tok);
        // 不知道歌多长：先按最长 60 秒跳，放完（AudioDone）就停
        self.go_dance(60_000, Some(lines::SONG_PLAYING.to_string()));
        let vol = crate::core::config::get().tts.music_vol();
        crate::services::audio::play_file(tok, path, vol, 60);
    }

    /// 统一修改桌宠语音/伴奏音量与静音标记，并实时同步正在播放的音频与 UI
    pub(super) fn mutate_volume(
        &mut self,
        show_osd: bool,
        f: impl FnOnce(&mut u8, &mut u8, &mut bool),
    ) -> (u8, u8, bool) {
        if self.io {
            let c = crate::core::config::update(|cfg| {
                let mut vp = cfg.tts.voice_pct();
                let mut mp = cfg.tts.music_pct();
                let mut mu = cfg.tts.muted;
                f(&mut vp, &mut mp, &mut mu);
                cfg.tts.volume = vp.min(100) as f32 / 100.0;
                cfg.tts.music_volume = mp.min(100) as f32 / 100.0;
                cfg.tts.muted = mu;
            });
            if c.tts.muted {
                self.stop_voice();
                self.cv_disable_tts();
            }
            crate::services::audio::apply_config_volume();
            crate::platform::sync_volume_ui();
            if show_osd {
                crate::platform::open_volume_osd();
            }
            (c.tts.voice_pct(), c.tts.music_pct(), c.tts.muted)
        } else {
            let (mut vp, mut mp, mut mu) = self.sim_vol;
            f(&mut vp, &mut mp, &mut mu);
            vp = vp.min(100);
            mp = mp.min(100);
            self.sim_vol = (vp, mp, mu);
            if mu {
                self.stop_voice();
                self.cv_disable_tts();
            }
            self.call(format!("vol:{vp}:{mp}:{}", if mu { "mute" } else { "on" }));
            (vp, mp, mu)
        }
    }

    /// 蜂蜜水：「舔了蜂蜜，脚步就会变快」——喝完一分钟内走路飞快
    fn drink_honey(&mut self) {
        if self.act(Act::Feed) == ActResult::Full {
            self.play(Shot::Shake);
            self.say(&pick(lines::FULL), 2600);
            return;
        }
        self.react(St::Honey, 2600, Some(pick(lines::HONEY)));
        self.fx(FxKind::Honey, 6);
        self.later(2700, Later::HoneyFast);
    }

    fn fx(&mut self, k: FxKind, n: usize) { self.fx.spawn(k, n, self.now); }

}

#[cfg(test)]
mod tests;
