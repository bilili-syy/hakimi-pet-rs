//! 跨模块共享类型和接口 —— 冻结文件。
//! 三路并行开发都依赖它，任何改动先和集成负责人（behavior/render 那一路）商量。
#![allow(dead_code)]

use std::fmt;
use std::sync::Arc;

/// 窗口逻辑尺寸（和 Electron 版一致）。物理像素 = 逻辑 × DPI 缩放
pub const WIN_W: u32 = 240;
pub const WIN_H: u32 = 360;

// ---------------------------------------------------------------- 立绘
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Face {
    Idle, Blink, Happy, Drag, Sit, Sleep, Eat, Hungry, Think, Walk1, Walk2,
    Shy, Dizzy, Surprised, Cry, Yawn, Love, Hiss, Stare, Dance, Honey,
}

impl Face {
    pub const ALL: [Face; 21] = [
        Face::Idle, Face::Blink, Face::Happy, Face::Drag, Face::Sit, Face::Sleep, Face::Eat,
        Face::Hungry, Face::Think, Face::Walk1, Face::Walk2, Face::Shy, Face::Dizzy,
        Face::Surprised, Face::Cry, Face::Yawn, Face::Love, Face::Hiss, Face::Stare,
        Face::Dance, Face::Honey,
    ];
    /// 对应 assets/sprites/<stem>.png
    pub fn stem(self) -> &'static str {
        match self {
            Face::Idle => "idle", Face::Blink => "blink", Face::Happy => "happy", Face::Drag => "drag",
            Face::Sit => "sit", Face::Sleep => "sleep", Face::Eat => "eat", Face::Hungry => "hungry",
            Face::Think => "think", Face::Walk1 => "walk1", Face::Walk2 => "walk2", Face::Shy => "shy",
            Face::Dizzy => "dizzy", Face::Surprised => "surprised", Face::Cry => "cry",
            Face::Yawn => "yawn", Face::Love => "love", Face::Hiss => "hiss", Face::Stare => "stare",
            Face::Dance => "dance", Face::Honey => "honey",
        }
    }
}

// ---------------------------------------------------------------- 语音
/// TTS 情绪（对应 Electron 版 tts.js 的 MOOD_TAG 键）
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VoiceMood { Happy, Sad, Hungry, Sleepy, Mad, Think, Calm }

impl VoiceMood {
    pub fn key(self) -> &'static str {
        match self {
            VoiceMood::Happy => "happy", VoiceMood::Sad => "sad", VoiceMood::Hungry => "hungry",
            VoiceMood::Sleepy => "sleepy", VoiceMood::Mad => "mad", VoiceMood::Think => "think",
            VoiceMood::Calm => "calm",
        }
    }
}

/// 固定台词 or 聊天回复（config.tts.speak == "chat" 时只念 Chat）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpeakKind { Line, Chat }

/// 编码后的音频字节（wav/ogg/mp3…），交给 services::audio 播放
#[derive(Clone)]
pub struct Audio(pub Vec<u8>);
impl fmt::Debug for Audio {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "Audio({} bytes)", self.0.len()) }
}

// ---------------------------------------------------------------- 输入
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button { Left, Right }

/// 鼠标事件。x/y：窗口内逻辑像素（240×360 坐标系，已除以 DPI 缩放）；sx/sy：屏幕物理像素。
/// 分层窗口里 alpha=0 的像素天然点穿，平台层只会收到落在可见像素上的事件。
#[derive(Clone, Copy, Debug)]
pub enum MouseEv {
    Down { button: Button, x: f32, y: f32, sx: i32, sy: i32 },
    Up { button: Button, x: f32, y: f32, sx: i32, sy: i32 },
    /// 鼠标移动（包括按住拖动时；拖动时平台层要 SetCapture，保证移出窗口也能收到）
    Move { x: f32, y: f32, sx: i32, sy: i32 },
    DoubleClick { x: f32, y: f32 },
    Leave,
}

/// 右键菜单 / 托盘菜单的动作。平台层把每个动作都交给 Handler::event，
/// 另外自己执行「平台类」动作（见 docs/ARCHITECTURE.md 的表）。
#[derive(Clone, Debug, PartialEq)]
pub enum MenuAction {
    Chat, Feed, Pat, Honey, Song, OpenSongDir, Status, Sleep, Walk,
    PomodoroStart(u32), PomodoroStop,
    /// 整套衣服："" 原装 / "xmas" / "maid" / "jk"
    Costume(String),
    /// 配件开关：("santa"|"glasses"|"scarf", on)
    Outfit(String, bool),
    ClearOutfit,
    OpenSettings, ReloadSettings,
    Tts(bool), TtsChatOnly(bool), Moyu(bool),
    ForgetChat, OpenDataDir, AutoStart(bool),
    ToggleVisible, CallToCenter, Quit,
}

// ---------------------------------------------------------------- 事件
#[derive(Clone, Debug)]
pub struct ChatReply {
    pub ok: bool,
    /// 成功时是回复；失败时是给用户看的猫娘口吻提示
    pub text: String,
    pub need_key: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemindKind { Water, StandUp, OffWork }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PomodoroEv { Start { minutes: u32 }, Done { minutes: u32 }, Cancel }

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MoyuEv {
    /// level 1..=3；what 是命中的关键词/进程名
    Nag { level: u8, minutes: u32, what: String, pomodoro: bool },
    Back { minutes: u32 },
}

/// 发给 Handler 的所有事件（平台层在 UI 线程上逐个投递）
#[derive(Debug)]
pub enum AppEvent {
    Mouse(MouseEv),
    Menu(MenuAction),
    /// 聊天输入框回车
    ChatSubmit(String),
    /// 聊天输入框 Esc / 失焦关闭
    ChatCancel,
    ChatReply(ChatReply),
    /// tts::speak 的结果；None = 不念（关了/没 key/失败/限流）
    Speech { token: u64, audio: Option<Audio> },
    /// audio 播放结束（正常放完或被 stop）
    AudioDone { token: u64 },
    Remind(RemindKind),
    Pomodoro(PomodoroEv),
    Moyu(MoyuEv),
    /// 掉到地面；big = 摔得重
    Landed { big: bool },
    WalkDone { hit_edge: bool },
    /// 从隐藏变可见、或被叫到屏幕中间（Electron 版的 'peek'）
    Peek,
    /// 用户又双击了一次程序
    SecondInstance,
}

/// 线程安全的事件投递器：服务层工作线程用它把结果送回 UI 线程。
/// 由平台层创建（内部是 PostMessage 唤醒消息循环）。
#[derive(Clone)]
pub struct EventSender(Arc<dyn Fn(AppEvent) + Send + Sync>);
impl EventSender {
    pub fn new(f: impl Fn(AppEvent) + Send + Sync + 'static) -> Self { Self(Arc::new(f)) }
    pub fn send(&self, ev: AppEvent) { (self.0)(ev) }
}
impl fmt::Debug for EventSender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("EventSender") }
}

// ---------------------------------------------------------------- 行为层 → 平台层
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatInputCmd { Open, Close, Enable(bool) }

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WinCmd {
    /// 沿地面水平走 distance 物理像素（dir = -1/1），每 16ms 走 speed 像素；碰边或走完发 WalkDone
    Walk { dir: i32, distance: f32, speed: f32 },
    WalkStop,
    /// 拖拽中：光标在屏幕 (sx, sy)。平台层按最近一次左键 Down 时记下的光标-窗口偏移来摆窗口
    DragMove { sx: i32, sy: i32 },
    /// 松手：带重力掉到工作区底部，落地发 Landed
    DragEnd,
    Show, Hide,
    ChatInput(ChatInputCmd),
    Quit,
}

// ---------------------------------------------------------------- 画面
/// 一帧画面：预乘 alpha 的 BGRA，自上而下，可直接交给 UpdateLayeredWindow
pub struct Frame {
    pub w: u32,
    pub h: u32,
    pub bgra: Vec<u8>,
}

/// 平台层驱动的应用接口（app::App 实现）。全部在 UI 线程调用。
pub trait Handler {
    fn event(&mut self, ev: AppEvent, out: &mut Vec<WinCmd>);
    /// 每帧调用。返回希望多少毫秒后再来一帧（动画中 16~33；完全静止时可以返回 200+ 省电）
    fn frame(&mut self, now_ms: u64, out: &mut Vec<WinCmd>) -> u32;
    /// 画面有变化时返回新帧（物理像素 = WIN_W×scale × WIN_H×scale），没变化返回 None
    fn render(&mut self, scale: f32) -> Option<&Frame>;
    /// 退出前（存档等）
    fn quit(&mut self) {}
}
