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
    /// v1.1 新表情：委屈嘟嘴 / 得意叉腰 / 疑惑歪头 / 害怕发抖 / 星星眼 / 鼓脸生气
    Pout, Smug, Confused, Scared, Starry, Puff,
}

impl Face {
    pub const ALL: [Face; 27] = [
        Face::Idle, Face::Blink, Face::Happy, Face::Drag, Face::Sit, Face::Sleep, Face::Eat,
        Face::Hungry, Face::Think, Face::Walk1, Face::Walk2, Face::Shy, Face::Dizzy,
        Face::Surprised, Face::Cry, Face::Yawn, Face::Love, Face::Hiss, Face::Stare,
        Face::Dance, Face::Honey, Face::Pout, Face::Smug, Face::Confused, Face::Scared,
        Face::Starry, Face::Puff,
    ];
    /// 对应 assets/sprites/<stem>.png
    pub fn stem(self) -> &'static str {
        match self {
            Face::Idle => "idle", Face::Blink => "blink", Face::Happy => "happy", Face::Drag => "drag",
            Face::Sit => "sit", Face::Sleep => "sleep", Face::Eat => "eat", Face::Hungry => "hungry",
            Face::Think => "think", Face::Walk1 => "walk1", Face::Walk2 => "walk2", Face::Shy => "shy",
            Face::Dizzy => "dizzy", Face::Surprised => "surprised", Face::Cry => "cry",
            Face::Yawn => "yawn", Face::Love => "love", Face::Hiss => "hiss", Face::Stare => "stare",
            Face::Dance => "dance", Face::Honey => "honey", Face::Pout => "pout", Face::Smug => "smug",
            Face::Confused => "confused", Face::Scared => "scared", Face::Starry => "starry", Face::Puff => "puff",
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
    /// 滚轮（WM_MOUSEWHEEL，光标在她身上时）。delta：一格 = ±120，向上为正；x/y 同上（窗口内逻辑像素）
    Wheel { delta: i32, x: f32, y: f32 },
}

/// 右键菜单 / 托盘菜单的动作。平台层把每个动作都交给 Handler::event，
/// 另外自己执行「平台类」动作（见 docs/ARCHITECTURE.md 的表）。
#[derive(Clone, Debug, PartialEq)]
pub enum MenuAction {
    Chat, Feed, Pat, Honey, Song, OpenSongDir, Status, Sleep, Walk,
    PomodoroStart(u32), PomodoroStop,
    /// 整套衣服：core::costume::COSTUMES 里的 id（"sailor" 默认 / "hoodie" 原装卫衣 / …；"" 也当默认）
    Costume(String),
    /// 配件开关：("santa"|"glasses", on)
    Outfit(String, bool),
    ClearOutfit,
    /// 打开设置窗口
    OpenSettings,
    /// 用记事本等打开 config.json（设置窗口「编辑设置文件（高级）」）
    OpenConfigFile,
    ReloadSettings,
    Tts(bool), TtsChatOnly(bool), Moyu(bool),
    ForgetChat, OpenDataDir, AutoStart(bool),
    ToggleVisible, CallToCenter, Quit,
    /// 老的「直接喂到嘴里」（Feed 现在是扔一条小鱼干让她去追 / 你拖到她嘴边）
    FeedNow,
    /// 小游戏：逗猫棒
    Game,
    /// 今日签到（第一次互动也会自动签）
    CheckIn,
    /// 天气播报：现在天气、气温、今天最高最低、降雨概率
    WeatherReport,
    /// 爬到当前窗口顶上坐着
    Climb,
    /// 右键「备忘录」子菜单
    Memo(MemoAct),
    /// 跳个舞：舞谱 id（behavior::dance::SCORES），"" = 随机挑一支
    Dance(String),
    /// 右键「她的记忆」子菜单
    Mind(MindAct),
}

// ---------------------------------------------------------------- 事件
/// 天气大类（services::weather 从 wttr.in 的 weatherCode 归类）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeatherKind { Clear, Cloudy, Fog, Rain, Snow, Thunder }

#[derive(Clone, Debug, PartialEq)]
pub struct Weather {
    pub kind: WeatherKind,
    pub temp_c: f32,
    /// 中文描述（如「小雨」），可能为空
    pub desc: String,
    pub city: String,
    /// 今天最高/最低气温（拿不到时 None）
    pub high_c: Option<f32>,
    pub low_c: Option<f32>,
    /// 今天降雨概率（0–100，取全天各时段最大值）
    pub rain_chance: Option<u32>,
}

/// 桌面上的小道具（单独的小分层窗口）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToyKind { Fish, Can, Teaser }

/// 她坐到了哪儿
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PerchKind {
    /// 前台窗口的上沿
    WindowTop,
    /// 屏幕侧边（探出半个身子）
    ScreenSide,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ToyEv {
    /// 用户按住了道具
    Grabbed,
    /// 用户松手：(dx, dy) = 道具中心相对她嘴巴的位置（逻辑像素）
    Dropped { dx: f32, dy: f32 },
    /// 道具掉到地面；dx = 道具中心相对她身体中线（逻辑像素）
    Landed { dx: f32 },
    /// ToyCmd::Chase 走完：ok = 已经走到道具跟前
    Reached { ok: bool },
}

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

/// 备忘录里到点的一条
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoHit {
    pub id: u64,
    pub text: String,
    /// 错过了（睡眠/关机醒来后补的）
    pub late: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemoEv {
    /// 到点了（醒来后错过的好几条会合并成一次）。平台层发
    Due(Vec<MemoHit>),
    /// 本地规则没看懂、交给 AI 后的结果：ok = 已经加进备忘录，text 是她要说的确认/提示
    Reply { ok: bool, text: String },
}

/// 「备忘录」子菜单的动作
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoAct {
    Done(u64),
    Delete(u64),
    /// 稍后 10 分钟再提醒（正在响的那些）
    Snooze,
    OpenFile,
    /// 设置窗口的「备忘录」页
    OpenWindow,
    ClearDone,
}

/// 「她的记忆」子菜单的动作
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MindAct {
    /// 打开 memory.json（平台层打开）
    OpenFile,
    /// 点了「清空记忆…」：平台层弹框二次确认，确认了再发 Clear
    ClearAsk,
    /// 已确认：清空
    Clear,
}

/// 长期记忆的后台结果
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MindEv {
    /// 主动关心：模型给的一句话（开头可能带情绪标签）；None = 这次没什么好说的
    Care(Option<String>),
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
    /// 光标位置（相对窗口左上角的逻辑像素，可在窗口外）。只在 WinCmd::CursorPoll 打开时、位置变了才发
    Cursor { x: f32, y: f32 },
    /// 后台拉到的天气
    Weather(Weather),
    Toy(ToyEv),
    /// WinCmd::Climb 成功跳上去了
    Perched(PerchKind),
    /// WinCmd::Climb 条件不满足（没有合适的窗口），什么都没做
    ClimbFailed,
    /// 坐着的窗口挪走/关了/最小化：平台层已经让她开始下落（之后照常发 Landed）
    PerchLost,
    /// 备忘录：到点提醒 / AI 解析结果
    Memo(MemoEv),
    /// 长期记忆：后台结果
    Mind(MindEv),
    /// 场景感知（platform/sense.rs）
    Sense(SenseEv),
}

/// 场景感知事件：只在状态变化时发
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SenseEv {
    /// 用户没动静了：1 = 3 分钟，2 = 10 分钟
    UserIdle(u8),
    /// 走开 ≥5 分钟后又有输入了（离开了多少秒）
    UserBack { secs: u32 },
    /// 持续快速打字（20 秒里大部分时间在敲键盘）
    Typing,
    /// 鼠标左右猛晃
    CursorShake,
    /// 前台全屏程序（游戏/全屏视频）开始 / 结束
    Fullscreen(bool),
    /// 后台音乐播放器在出声 开始 / 停了
    Music(bool),
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
    /// 光标轮询间隔（毫秒，0 = 关）。平台层只在窗口可见时 GetCursorPos，位置变了才发 AppEvent::Cursor
    CursorPoll(u32),
    /// 跳到前台窗口上沿 / 屏幕侧边。成功发 Perched，不行发 ClimbFailed
    Climb(PerchKind),
    /// 从坐着的地方跳下来（带重力落地，发 Landed）
    Unperch,
    Toy(ToyCmd),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ToyCmd {
    /// 生成道具。Fish/Can：在她身边 side（-1/1）方向 offset 逻辑像素处、她头顶高度出现，然后往下掉（可以被拖）；
    /// Teaser：跟着光标走、点不到（点穿）
    Spawn { kind: ToyKind, side: i32, offset: f32 },
    /// 让道具往下掉到地面（发 Landed）
    Fall,
    /// 她朝道具走过去（每 16ms 走 speed 物理像素），走到发 Reached
    Chase { speed: f32 },
    Remove,
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
