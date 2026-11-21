//! 跳个舞（右键「💃 跳个舞」）。
//!
//! 舞谱 [`Score`] = 默认 BPM + 时长 + 一串按「拍」计长的动作段 [`Step`]，循环跳到时间到。
//! 每段指定：动作（render::dance::Move 的关键帧）、两张表情（每两拍换一次）、朝向、要不要带着窗口左右挪。
//! **加新舞：在 [`SCORES`] 里加一条即可**，菜单自动出现。
//!
//! 时钟：舞步按「拍位置」走。拍位置每帧按当前周期累加；有节拍网格（内置鼓点 = 精确网格，
//! 歌曲 = 后台节拍检测的结果）时，用一个简单的锁相环把拍相位往网格上拉、周期往检测值上滑，
//! 所以检测结果晚到一两秒也不会突然跳拍。歌曲的音量包络决定动作力度。
use super::beat::{self, BeatInfo, Pending};
use super::fx::FxKind;
use super::{Behavior, Shot, St, pick};
use crate::core::stats::Stats;
use crate::render::dance::{self as kf, Move, Pose};
use crate::render::{FxShape, FxSprite, Xf, anim};
use crate::types::{Audio, Face, WinCmd};

/// 跳舞时的帧间隔（≈60fps，和灵动待机一致）；跳完回到原来的空闲帧率
/// 运行时按 config::frame_ms()（设置里的帧率），这里是默认值，测试用
#[allow(dead_code)]
pub const FRAME_MS: u32 = 16;
/// 窗口左右挪一下的距离（逻辑像素）
const NUDGE_PX: f32 = 10.0;
/// 换动作时的混合时长（拍）
const BLEND_BEATS: f64 = 0.25;
/// 估计的出声延迟（开声卡 + 缓冲）
const AUDIO_LATENCY_MS: f64 = 60.0;
/// 特效粒子上限（内存/绘制量封顶）
const MAX_PARTS: usize = 40;
/// 精力低于这个就不跳了
const TIRED_ENERGY: f64 = 12.0;
/// 跳完整支：心情 +MOOD_GAIN，精力 -ENERGY_COST（中途停按比例）
const MOOD_GAIN: f64 = 12.0;
const ENERGY_COST: f64 = 6.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Facing {
    /// 正常朝向
    Front,
    /// 镜像
    Mirror,
    /// 每拍左右翻
    Alt,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Step {
    pub beats: u32,
    pub mv: Move,
    /// 每两拍在这两张表情之间切换
    pub faces: (Face, Face),
    pub facing: Facing,
    /// 这段每两拍带着窗口左右挪一下
    pub nudge: bool,
}

const fn st(beats: u32, mv: Move, a: Face, b: Face, facing: Facing, nudge: bool) -> Step {
    Step { beats, mv, faces: (a, b), facing, nudge }
}

pub struct Score {
    pub id: &'static str,
    pub icon: &'static str,
    pub name: &'static str,
    /// 没有歌（内置鼓点）时的速度
    pub bpm: f32,
    /// 跳多久（秒），有歌没歌都一样
    pub secs: f32,
    pub steps: &'static [Step],
    /// 每拍飘几个音符
    pub notes: u8,
    /// 每个强拍（每 4 拍）冒几个彩色光点，其余拍减半
    pub dots: u8,
    pub intro: &'static [&'static str],
    pub outro: &'static [&'static str],
}

impl Score {
    pub fn loop_beats(&self) -> u32 { self.steps.iter().map(|s| s.beats).sum::<u32>().max(1) }

    /// 拍位置 → (动作段下标, 段内进度 0..1, 已进入本段多少拍)
    pub fn step_at(&self, beat: f64) -> (usize, f32, f64) {
        if self.steps.is_empty() { return (0, 0.0, 0.0); }
        let lb = self.loop_beats() as f64;
        let pos = beat.max(0.0).rem_euclid(lb);
        let mut acc = 0.0;
        for (i, s) in self.steps.iter().enumerate() {
            let len = s.beats.max(1) as f64;
            if pos < acc + len { return (i, ((pos - acc) / len) as f32, pos - acc); }
            acc += len;
        }
        (self.steps.len() - 1, 1.0, 0.0)
    }
}

use Face as F;
use Facing::{Alt, Front, Mirror};
use Move as M;

pub static SCORES: [Score; 3] = [
    Score {
        id: "wiggle",
        icon: "🐱",
        name: "小猫摇摆",
        bpm: 100.0,
        secs: 20.0,
        steps: &[
            st(4, M::Sway, F::Dance, F::Happy, Front, false),
            st(2, M::Shimmy, F::Happy, F::Happy, Front, false),
            st(2, M::Bounce, F::Dance, F::Dance, Alt, false),
            st(4, M::Sway, F::Love, F::Shy, Mirror, false),
            st(2, M::Squat, F::Happy, F::Dance, Front, false),
            st(2, M::Pose, F::Shy, F::Shy, Front, false),
        ],
        notes: 1,
        dots: 2,
        intro: &["小猫摇摆～跟着节奏扭起来喵！", "看好啦，哈基米独家小猫摇摆喵～"],
        outro: &["摇完啦～尾巴都甩热了喵 (≧▽≦)", "怎么样怎么样，可爱吧喵？"],
    },
    Score {
        id: "spin",
        icon: "🌀",
        name: "转圈圈",
        bpm: 112.0,
        secs: 22.0,
        steps: &[
            st(2, M::Bounce, F::Happy, F::Dance, Front, false),
            st(2, M::Spin, F::Dance, F::Dance, Front, false),
            st(2, M::Bounce, F::Dance, F::Happy, Mirror, false),
            st(2, M::Spin, F::Love, F::Love, Front, false),
            st(4, M::Slide, F::Walk1, F::Walk2, Alt, true),
            st(2, M::Squat, F::Surprised, F::Happy, Front, false),
            st(2, M::Spin, F::Happy, F::Happy, Front, false),
        ],
        notes: 1,
        dots: 3,
        intro: &["转圈圈～转到头晕也不停喵！", "要转啦要转啦，别眨眼喵～"],
        outro: &["呼…转得星星都在眼前飞喵～", "转完啦！哈基米一点都没晕…才怪喵"],
    },
    Score {
        id: "disco",
        icon: "✨",
        name: "动感蹦迪",
        bpm: 128.0,
        secs: 26.0,
        steps: &[
            st(4, M::Jump, F::Dance, F::Happy, Alt, true),
            st(2, M::Shimmy, F::Love, F::Love, Front, false),
            st(2, M::Spin, F::Dance, F::Dance, Front, false),
            st(4, M::Bounce, F::Happy, F::Dance, Alt, false),
            st(2, M::Slide, F::Stare, F::Dance, Mirror, true),
            st(2, M::Pose, F::Love, F::Happy, Front, false),
        ],
        notes: 2,
        dots: 5,
        intro: &["动感蹦迪时间到！灯光音乐走起喵～", "今天哈基米就是全场最靓的猫喵！"],
        outro: &["蹦迪结束！出了一身猫汗喵～", "嗨完啦，给哈基米鼓个掌喵！"],
    },
];

const STOPPED: &[&str] = &["诶？不跳啦？那哈基米歇会儿喵～", "被你按住了喵…下次再跳给你看！"];
const TOO_TIRED: &[&str] = &["跳不动了喵…让哈基米先歇一会儿嘛～", "腿软软的…今天的舞先欠着喵～"];
const NO_SONG: &str = "（songs 文件夹里还没有歌，先用哈基米牌鼓点喵）";
/// 菜单「🎵 跟着我的歌跳」：随机一支舞，配 songs 文件夹里随机一首歌；别的舞都用自己的内置鼓点
pub const SONG_ID: &str = "song";

/// id → 舞谱；"" 或不认识的 id 随机挑一支
pub fn find(id: &str) -> &'static Score {
    SCORES.iter().find(|s| s.id == id).unwrap_or_else(|| &SCORES[fastrand::usize(..SCORES.len())])
}

/// 跳舞的数值结算（core::stats 的数据）：心情升、精力降，跳完整支 +1 亲密度
pub fn reward(stats: &mut Stats, frac: f64) {
    let f = frac.clamp(0.0, 1.0);
    if f < 0.15 { return; }
    let d = &mut stats.data;
    d.mood = (d.mood + MOOD_GAIN * f).clamp(0.0, 100.0);
    d.energy = (d.energy - ENERGY_COST * f).clamp(0.0, 100.0);
    if f >= 0.9 { d.affection += 1.0; }
}

// ---------------------------------------------------------------- 粒子
const DOT_COLORS: [u32; 7] = [0xff5ea8, 0x4285f4, 0xfbbc04, 0x34a853, 0xa259ff, 0x00c8ff, 0xea4335];

#[derive(Clone, Debug)]
struct Part {
    note: Option<u8>,
    color: u32,
    x: f32,
    y: f32,
    dx: f32,
    dy: f32,
    size: f32,
    born: u64,
    life: u64,
}

fn rnd(a: f32, b: f32) -> f32 { a + fastrand::f32() * (b - a) }

// ---------------------------------------------------------------- 一支舞的运行状态
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cue {
    /// 走到第 n 拍
    Beat(i64),
    /// 带着窗口挪一下（1 右 / -1 左）
    Nudge(i32),
}

/// 一帧画面要的东西
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DanceFrame {
    pub pose: Pose,
    pub flip: f32,
    pub face: Face,
}

pub struct Routine {
    pub score: &'static Score,
    pub start: u64,
    pub end: u64,
    last: u64,
    /// 拍位置
    beat: f64,
    /// 当前每拍毫秒（往 target_period 平滑靠）
    period: f64,
    target_period: f64,
    /// 已经触发过的最大整拍
    fired: i64,
    /// 节拍网格：(每拍毫秒, 某一拍的绝对时间)
    grid: Option<(f64, f64)>,
    info: Option<BeatInfo>,
    /// 音乐开始的时刻（包络对齐用）
    audio_start: u64,
    pending: Option<Pending>,
    parts: Vec<Part>,
    nudges: u32,
    /// 放的是 songs 里的歌（不是内置鼓点）
    pub song: bool,
    /// 歌放不了/提前放完后已经换成内置鼓点
    pub fallback: bool,
    /// 坐在窗口顶上跳：原地跳，不挪窗口
    pub still: bool,
}

impl Routine {
    pub fn new(score: &'static Score, now: u64) -> Self {
        let period = 60_000.0 / score.bpm as f64;
        Self {
            score,
            start: now,
            end: now + (score.secs * 1000.0) as u64,
            last: now,
            beat: 0.0,
            period,
            target_period: period,
            fired: -1,
            grid: None,
            info: None,
            audio_start: now,
            pending: None,
            parts: Vec::new(),
            nudges: 0,
            song: false,
            fallback: false,
            still: false,
        }
    }

    /// 内置鼓点从 now 开始：网格就是舞谱自己的速度
    pub fn lock_builtin(&mut self, now: u64) {
        let p = 60_000.0 / self.score.bpm as f64;
        self.audio_start = now;
        self.target_period = p;
        self.grid = Some((p, now as f64 + AUDIO_LATENCY_MS));
        self.info = None;
    }

    pub fn set_pending(&mut self, p: Pending) { self.pending = Some(p); }

    /// 采用节拍检测结果
    pub fn adopt(&mut self, info: BeatInfo) {
        if let Some((p, phase)) = info.tempo {
            self.target_period = p;
            self.grid = Some((p, self.audio_start as f64 + phase + AUDIO_LATENCY_MS));
        }
        self.info = Some(info);
    }

    #[cfg(test)]
    pub fn beat(&self) -> f64 { self.beat }
    #[cfg(test)]
    pub fn period(&self) -> f64 { self.period }
    pub fn finished(&self, now: u64) -> bool { now >= self.end }
    pub fn progress(&self, now: u64) -> f64 {
        (now.saturating_sub(self.start) as f64 / (self.end - self.start).max(1) as f64).clamp(0.0, 1.0)
    }
    pub fn nudged(&self) -> bool { self.nudges > 0 }
    /// 挪了奇数次 → 还偏在右边，返回往回挪的方向
    pub fn return_nudge(&self) -> Option<i32> { (self.nudges % 2 == 1).then_some(-1) }

    /// 推进时钟到 now：锁相、触发整拍（生成粒子、挪窗口）
    pub fn update(&mut self, now: u64) -> Vec<Cue> {
        if let Some(p) = &self.pending
            && let Some(r) = p.poll()
        {
            self.pending = None;
            if let Some(info) = r { self.adopt(info); }
        }
        let dt = now.saturating_sub(self.last) as f64;
        self.last = now;
        self.period += (self.target_period - self.period) * (dt / 400.0).min(1.0);
        self.beat += dt / self.period.max(1.0);
        if let Some((p, t0)) = self.grid {
            let target = (now as f64 - t0) / p;
            let d = target.rem_euclid(1.0) - self.beat.rem_euclid(1.0);
            let err = d - d.round();
            self.beat += err * (dt / 250.0).min(1.0);
        }
        let mut cues = Vec::new();
        let cur = self.beat.floor() as i64;
        while self.fired < cur {
            self.fired += 1;
            let n = self.fired;
            cues.push(Cue::Beat(n));
            self.spawn(n, now);
            let (idx, _, into) = self.score.step_at(n as f64 + 1e-6);
            if !self.still && self.score.steps[idx].nudge && (into.round() as i64) % 2 == 0 {
                let dir = if self.nudges.is_multiple_of(2) { 1 } else { -1 };
                self.nudges += 1;
                cues.push(Cue::Nudge(dir));
            }
        }
        self.parts.retain(|p| now < p.born + p.life);
        cues
    }

    fn spawn(&mut self, n: i64, now: u64) {
        let strong = n % 4 == 0;
        for i in 0..self.score.notes {
            self.parts.push(Part {
                note: Some(((n + i as i64) % 3) as u8),
                color: DOT_COLORS[fastrand::usize(..DOT_COLORS.len())],
                x: rnd(30.0, 160.0),
                y: rnd(50.0, 130.0),
                dx: rnd(-30.0, 30.0),
                dy: 0.0,
                size: rnd(14.0, 22.0),
                born: now,
                life: 1800,
            });
        }
        let dots = if strong { self.score.dots } else { self.score.dots / 2 };
        for _ in 0..dots {
            self.parts.push(Part {
                note: None,
                color: DOT_COLORS[fastrand::usize(..DOT_COLORS.len())],
                x: rnd(0.0, 190.0),
                y: rnd(10.0, 250.0),
                dx: rnd(-40.0, 40.0),
                dy: rnd(-40.0, 25.0),
                size: rnd(7.0, 12.0),
                born: now,
                life: 900,
            });
        }
        if self.parts.len() > MAX_PARTS {
            let extra = self.parts.len() - MAX_PARTS;
            self.parts.drain(..extra);
        }
    }

    /// 动作力度：跟着歌的音量包络（没有包络 = 1）
    fn amp(&self, now: u64) -> f32 {
        let Some(info) = &self.info else { return 1.0 };
        let t = now as f64 - self.audio_start as f64 - AUDIO_LATENCY_MS;
        match (info.level(t), info.level(t - beat::ENV_MS as f64)) {
            (Some(a), Some(b)) => 0.7 + 0.55 * (a + b) / 2.0,
            (Some(a), None) => 0.7 + 0.55 * a,
            _ => 1.0,
        }
    }

    fn step_pose(&self, s: &Step, step_p: f32, bi: i64, bp: f32, amp: f32) -> (Pose, f32) {
        let pose = kf::pose(s.mv, step_p, bp, bi.rem_euclid(2) == 1, amp);
        let flip = match s.facing {
            Facing::Front => 1.0,
            Facing::Mirror => -1.0,
            Facing::Alt => kf::facing(true, 1.0, bi, bp),
        };
        (pose, flip)
    }

    /// 当前这一帧的舞姿（换动作混合、开场/收尾过渡）
    pub fn frame(&self, now: u64) -> DanceFrame {
        let b = self.beat.max(0.0);
        let bi = b.floor() as i64;
        let bp = (b - bi as f64) as f32;
        let (idx, step_p, into) = self.score.step_at(b);
        let steps = self.score.steps;
        let s = &steps[idx];
        let amp = self.amp(now);
        let (mut pose, mut flip) = self.step_pose(s, step_p, bi, bp, amp);
        if into < BLEND_BEATS && b >= BLEND_BEATS {
            let prev = &steps[(idx + steps.len() - 1) % steps.len()];
            let (pp, pf) = self.step_pose(prev, 1.0, bi, bp, amp);
            let t = anim::ease(anim::Ease::EaseInOut, (into / BLEND_BEATS) as f32);
            pose = kf::lerp_pose(&pp, &pose, t);
            flip = pf + (flip - pf) * t;
        }
        // 开场 0.3 秒、收尾 0.4 秒从/回到站姿
        let since = now.saturating_sub(self.start) as f32;
        let left = self.end.saturating_sub(now) as f32;
        let k = (since / 300.0).min(left / 400.0).clamp(0.0, 1.0);
        if k < 1.0 {
            let t = anim::ease(anim::Ease::EaseInOut, k);
            pose = kf::lerp_pose(&Pose::ID, &pose, t);
            flip = 1.0 + (flip - 1.0) * t;
        }
        let face = if bi.div_euclid(2) % 2 == 0 { s.faces.0 } else { s.faces.1 };
        DanceFrame { pose, flip, face }
    }

    /// 这一帧的音符和彩色光点（#pet 盒子坐标）
    pub fn sprites(&self, now: u64) -> Vec<FxSprite> {
        let mut out = Vec::with_capacity(self.parts.len());
        for p in &self.parts {
            let t = now.saturating_sub(p.born) as f32 / p.life as f32;
            if t >= 1.0 { continue; }
            let (shape, xf) = match p.note {
                Some(n) => (FxShape::Note(n), anim::fx_rise(t, p.dx)),
                None => {
                    let twinkle = 0.65 + 0.35 * (t * 6.0 * std::f32::consts::PI).sin();
                    let s = 0.6 + 0.6 * (std::f32::consts::PI * t).sin();
                    let xf = Xf {
                        tx: p.dx * t,
                        ty: p.dy * t,
                        sx: s,
                        sy: s,
                        rot: 90.0 * t,
                        opacity: (std::f32::consts::PI * t).sin() * twinkle,
                        ..Xf::ID
                    };
                    (FxShape::Spark, xf)
                }
            };
            out.push(FxSprite { shape, x: p.x, y: p.y, size: p.size, xf, color: p.color });
        }
        out
    }
}

// ---------------------------------------------------------------- 行为层接入
impl Behavior {
    /// 菜单「跳个舞」
    pub(super) fn start_routine(&mut self, id: &str) {
        let use_song = id == SONG_ID;
        let score = find(if use_song { "" } else { id });
        // 跟逗猫棒一样：正在玩/输入框里打了字/等 AI 回复时不开跳
        if self.play.game.is_some() || self.chat_busy() || self.waiting { return; }
        self.yield_chat();
        if self.stats.data.energy < TIRED_ENERGY {
            self.play(Shot::Shake);
            self.say(&pick(TOO_TIRED), 2600);
            return self.set_state(St::Sit, Some((8000.0, 12000.0)));
        }
        self.cancel_routine();
        self.stop_song();
        self.stop_voice();
        self.dance = None;
        self.chase = None;
        self.set_state(St::Dance, None);
        let now = self.now;
        let mut r = Routine::new(score, now);
        r.still = self.play.perch.is_some();
        let path = if self.io && use_song { crate::services::songs::pick() } else { None };
        let tok = self.next_token();
        self.song = Some(tok);
        let secs = score.secs.ceil() as u32 + 1;
        let has_song = path.is_some();
        match path {
            Some(p) => {
                r.song = true;
                self.call(format!("dance-song:{}", score.id));
                if self.io {
                    r.set_pending(beat::analyze_file(p.clone(), secs.min(30)));
                    let vol = crate::core::config::get().tts.volume;
                    crate::services::audio::play_file(tok, p, vol, secs);
                }
            }
            None => {
                r.lock_builtin(now);
                self.call(format!("dance-beat:{}", score.id));
                self.play_builtin_beat(tok, score.bpm, score.secs);
            }
        }
        self.routine = Some(r);
        let mut line = pick(score.intro);
        if use_song && !has_song { line.push_str(NO_SONG); }
        // 放着音乐：只出气泡不念（只有一个声道）
        self.say(&line, 3200);
    }

    fn play_builtin_beat(&mut self, tok: u64, bpm: f32, secs: f32) {
        if !self.io { return; }
        let vol = crate::core::config::get().tts.volume * 0.8;
        crate::services::audio::play_music_mem(tok, Audio(beat::synth_wav(bpm, secs + 0.5)), vol, secs.ceil() as u32 + 2);
    }

    /// 每帧推进（frame_at 里调）
    pub(super) fn dance_tick(&mut self) {
        let now = self.now;
        let Some(r) = self.routine.as_mut() else { return };
        if r.finished(now) { return self.finish_routine(false); }
        let cues = r.update(now);
        for c in cues {
            if let Cue::Nudge(dir) = c {
                self.cmds.push(WinCmd::Walk { dir, distance: NUDGE_PX * self.scale, speed: 1.2 * self.scale });
            }
        }
    }

    /// 把舞姿盖到场景上（build_scene 末尾调）
    pub(super) fn apply_dance_scene(&mut self) {
        let Some(r) = &self.routine else { return };
        let now = self.now;
        let f = r.frame(now);
        self.scene.pet = f.pose.pet;
        self.scene.tail = f.pose.tail;
        self.scene.star = f.pose.star;
        self.scene.flip = f.flip;
        self.scene.face = f.face;
        self.scene.fx.extend(r.sprites(now));
    }

    /// 跳完或被点停：结算 + 收尾台词
    pub(super) fn finish_routine(&mut self, by_click: bool) {
        let Some(r) = self.routine.take() else { return };
        let frac = r.progress(self.now);
        self.wrap_up(&r);
        reward(&mut self.stats, frac);
        self.save();
        self.call(format!("dance-end:{}:{}", r.score.id, if by_click { "click" } else { "done" }));
        if by_click {
            self.react(St::Surprised, 2200, Some(pick(STOPPED)));
        } else {
            self.react(St::Love, 2800, Some(pick(r.score.outro)));
            self.fx(FxKind::Heart, 4);
            self.fx(FxKind::Spark, 6);
        }
    }

    /// 被别的事打断（拖拽、喂食、提醒……）：安静收场，按跳了多少结算
    pub(super) fn cancel_routine(&mut self) {
        let Some(r) = self.routine.take() else { return };
        let frac = r.progress(self.now);
        self.wrap_up(&r);
        reward(&mut self.stats, frac);
        self.call(format!("dance-end:{}:cancel", r.score.id));
    }

    fn wrap_up(&mut self, r: &Routine) {
        if r.nudged() {
            self.cmds.push(WinCmd::WalkStop);
            if let Some(dir) = r.return_nudge() {
                self.cmds.push(WinCmd::Walk { dir, distance: NUDGE_PX * self.scale, speed: 1.2 * self.scale });
            }
        }
        self.stop_song();
    }

    /// 音乐提前停了（歌放不了/比舞短）：还没跳到 80% 就换内置鼓点接着跳
    pub(super) fn dance_music_done(&mut self) {
        let now = self.now;
        let Some(r) = self.routine.as_mut() else { return };
        if !r.song || r.fallback || r.progress(now) >= 0.8 { return; }
        r.fallback = true;
        r.song = false;
        r.pending = None;
        let left = (r.end.saturating_sub(now) as f32 / 1000.0).max(0.5);
        let bpm = r.score.bpm;
        // 鼓点按舞谱速度重新对拍；拍位置由锁相环平滑拉过去，不会跳拍
        r.lock_builtin(now);
        let tok = self.next_token();
        self.song = Some(tok);
        self.call("dance-beat-fallback".into());
        self.play_builtin_beat(tok, bpm, left);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AppEvent, Button, EventSender, MenuAction, MouseEv};

    fn mk() -> Behavior {
        fastrand::seed(11);
        let (s, _) = Stats::load(None);
        Behavior::with(EventSender::new(|_| {}), s, false, 0.0, String::new(), Vec::new(), false)
    }

    fn ev(b: &mut Behavior, e: AppEvent, now: u64) -> Vec<WinCmd> {
        let mut out = Vec::new();
        b.event_at(e, now, &mut out);
        out
    }

    /// 每 25ms 一帧跑到 until，返回最后一帧的间隔和所有窗口命令
    fn run(b: &mut Behavior, from: u64, until: u64) -> (u32, Vec<WinCmd>) {
        let mut all = Vec::new();
        let mut ms = 0;
        let mut t = from;
        while t <= until {
            let mut out = Vec::new();
            ms = b.frame_at(t, &mut out);
            all.extend(out);
            t += 25;
        }
        (ms, all)
    }

    #[test]
    fn scores_are_well_formed() {
        let mut ids = Vec::new();
        for s in &SCORES {
            assert!((15.0..=30.0).contains(&s.secs), "{} lasts {}s", s.id, s.secs);
            assert!(!s.steps.is_empty() && s.loop_beats() >= 8);
            assert!(!s.intro.is_empty() && !s.outro.is_empty());
            assert!(s.intro.iter().chain(s.outro).all(|l| l.contains('喵')));
            assert!(!ids.contains(&s.id));
            ids.push(s.id);
            assert_eq!(find(s.id).id, s.id);
        }
        assert!(SCORES.len() >= 3);
        // 随机挑一次再比（原来在闭包里每次重新随机，约 30% 概率误报）
        let picked = find("").id;
        assert!(SCORES.iter().any(|s| s.id == picked));
    }

    #[test]
    fn step_lookup_by_beat() {
        let s = &SCORES[0]; // 4,2,2,4,2,2
        assert_eq!(s.loop_beats(), 16);
        assert_eq!(s.step_at(0.0).0, 0);
        let (i, p, into) = s.step_at(5.0);
        assert_eq!(i, 1);
        assert!((p - 0.5).abs() < 1e-6 && (into - 1.0).abs() < 1e-9);
        assert_eq!(s.step_at(15.99).0, 5);
        assert_eq!(s.step_at(16.0).0, 0); // 循环
        assert_eq!(s.step_at(16.0 + 8.5).0, 3);
    }

    #[test]
    fn builtin_beat_fires_every_beat_on_time() {
        let s = &SCORES[2]; // 128 BPM
        let mut r = Routine::new(s, 1000);
        r.lock_builtin(1000);
        let mut beats = Vec::new();
        let mut t = 1000;
        while t < 1000 + 10_000 {
            for c in r.update(t) { if let Cue::Beat(n) = c { beats.push((n, t)); } }
            t += 25;
        }
        let period = 60_000.0 / 128.0;
        // 0..=21 拍，不重不漏
        assert!(beats.windows(2).all(|w| w[1].0 == w[0].0 + 1));
        assert_eq!(beats[0].0, 0);
        assert!((beats.len() as f64 - 10_000.0 / period).abs() <= 1.5);
        // 每拍落在网格上（帧间隔 25ms 以内）
        for (n, t) in beats.iter().skip(2) {
            let ideal = 1000.0 + AUDIO_LATENCY_MS + *n as f64 * period;
            assert!((*t as f64 - ideal).abs() <= 26.0, "beat {n} at {t}, ideal {ideal}");
        }
    }

    #[test]
    fn phase_locks_onto_late_detection() {
        // 舞谱 100 BPM 先跳着，1.5 秒后检测结果到：120 BPM，第一拍在 170ms
        let mut r = Routine::new(&SCORES[0], 0);
        let p = Pending::default();
        r.set_pending(p.clone());
        let mut t = 0;
        let mut last = -1;
        while t < 1500 {
            for c in r.update(t) { if let Cue::Beat(n) = c { last = n; } }
            t += 25;
        }
        assert!((r.period() - 600.0).abs() < 1e-6);
        let info = BeatInfo { tempo: Some((500.0, 170.0)), confidence: 3.0, env: vec![255; 200] };
        p.put(Some(info));
        while t < 6000 {
            for c in r.update(t) {
                if let Cue::Beat(n) = c { assert_eq!(n, last + 1, "no repeated/skipped beats"); last = n; }
            }
            t += 25;
        }
        assert!((r.period() - 500.0).abs() < 0.5, "period {}", r.period());
        let target = (t as f64 - 25.0 - 170.0 - AUDIO_LATENCY_MS) / 500.0;
        let d = target.rem_euclid(1.0) - r.beat().rem_euclid(1.0);
        assert!((d - d.round()).abs() < 0.03, "phase error {d}");
        // 包络满格 → 力度最大
        assert!((r.amp(t) - 1.25).abs() < 1e-3);
    }

    #[test]
    fn nudges_come_in_pairs() {
        let mut r = Routine::new(&SCORES[1], 0); // 转圈圈：第 8~11 拍 Slide 带挪窗
        r.lock_builtin(0);
        let mut nudges = Vec::new();
        let mut t = 0;
        while t < 7000 {
            for c in r.update(t) { if let Cue::Nudge(d) = c { nudges.push(d); } }
            t += 25;
        }
        assert_eq!(nudges, vec![1, -1]);
        assert_eq!(r.return_nudge(), None);
        r.nudges = 3;
        assert_eq!(r.return_nudge(), Some(-1));
    }

    #[test]
    fn particles_are_capped_and_expire() {
        let mut r = Routine::new(&SCORES[2], 0);
        r.lock_builtin(0);
        let mut t = 0;
        while t < 8000 {
            r.update(t);
            assert!(r.parts.len() <= MAX_PARTS);
            t += 25;
        }
        let sp = r.sprites(t);
        assert!(sp.iter().any(|s| matches!(s.shape, FxShape::Note(_))));
        assert!(sp.iter().any(|s| s.shape == FxShape::Spark));
        assert!(r.sprites(t + 2000).is_empty(), "all faded out");
    }

    fn near_id(p: &Pose) -> bool {
        [p.pet, p.tail, p.star].iter().all(|x| {
            [x.tx - Xf::ID.tx, x.ty, x.rot, x.sx - 1.0, x.sy - 1.0, x.opacity - 1.0, x.bright - 1.0].iter().all(|v| v.abs() < 1e-4)
        })
    }

    #[test]
    fn frame_eases_in_and_out() {
        let mut r = Routine::new(&SCORES[0], 0);
        r.lock_builtin(0);
        r.update(0);
        assert!(near_id(&r.frame(0).pose));
        assert_eq!(r.frame(0).flip, 1.0);
        let mut t = 0;
        while t < 2000 { r.update(t); t += 25; }
        assert!(!near_id(&r.frame(t).pose));
        let end = r.end;
        r.update(end);
        assert!(near_id(&r.frame(end).pose));
        assert_eq!(r.frame(end).flip, 1.0);
    }

    #[test]
    fn reward_moves_stats() {
        let (mut s, _) = Stats::load(None);
        s.data.mood = 50.0;
        s.data.energy = 50.0;
        reward(&mut s, 1.0);
        assert_eq!((s.data.mood, s.data.energy, s.data.affection), (62.0, 44.0, 1.0));
        reward(&mut s, 0.1); // 刚开始就停：不算
        assert_eq!(s.data.mood, 62.0);
        reward(&mut s, 0.5);
        assert_eq!((s.data.mood, s.data.energy, s.data.affection), (68.0, 41.0, 1.0));
    }

    #[test]
    fn menu_dance_runs_and_finishes() {
        let mut b = mk();
        run(&mut b, 0, 3000);
        b.stats.data.mood = 50.0;
        b.stats.data.energy = 60.0;
        ev(&mut b, AppEvent::Menu(MenuAction::Dance("spin".into())), 3000);
        assert_eq!(b.state, St::Dance);
        assert!(b.routine.is_some() && b.song.is_some());
        assert!(b.calls.iter().any(|c| c == "dance-beat:spin"));
        assert!(b.scene.bubble.as_ref().unwrap().text.contains('喵'));
        // 放音乐时不念台词
        assert!(!b.calls.iter().any(|c| c.starts_with("speak:转圈圈")));
        let (ms, cmds) = run(&mut b, 3025, 3000 + 10_000);
        assert_eq!(ms, FRAME_MS);
        assert!(cmds.iter().any(|c| matches!(c, WinCmd::Walk { .. })), "nudges the window");
        assert!(!b.scene.fx.is_empty());
        let faces: Vec<Face> = (0..40).map(|i| { run(&mut b, 13_000 + i * 250, 13_000 + i * 250); b.scene.face }).collect();
        assert!(faces.iter().any(|f| *f != faces[0]), "expression changes");
        run(&mut b, 23_000, 3000 + 22_000 + 50);
        assert!(b.routine.is_none());
        assert_eq!(b.state, St::Love);
        assert!(b.song.is_none());
        assert!(b.calls.iter().any(|c| c == "dance-end:spin:done"));
        assert!(b.stats.data.mood > 55.0 && b.stats.data.energy < 60.0);
        // 收尾台词会念（歌已停）
        assert!(b.calls.iter().rev().take(5).any(|c| c.starts_with("speak:")));
        // 特效放完回到省电帧率
        run(&mut b, 25_100, 31_000);
        assert!(!b.animating());
        let (ms, _) = run(&mut b, 31_025, 31_025);
        // 舞蹈和灵动待机现在都是 ≈60fps，这里只确认已经不走舞蹈的固定帧了
        assert!(b.routine.is_none() && ms <= super::super::IDLE_FRAME_MS);
    }

    #[test]
    fn click_stops_and_drag_interrupts() {
        let mut b = mk();
        run(&mut b, 0, 3000);
        ev(&mut b, AppEvent::Menu(MenuAction::Dance("wiggle".into())), 3000);
        run(&mut b, 3025, 8000);
        let down = MouseEv::Down { button: Button::Left, x: 120.0, y: 250.0, sx: 500, sy: 500 };
        let up = MouseEv::Up { button: Button::Left, x: 120.0, y: 250.0, sx: 500, sy: 500 };
        ev(&mut b, AppEvent::Mouse(down), 8000);
        ev(&mut b, AppEvent::Mouse(up), 8050);
        assert!(b.routine.is_none());
        assert_eq!(b.state, St::Surprised);
        assert!(b.calls.iter().any(|c| c == "dance-end:wiggle:click"));

        ev(&mut b, AppEvent::Menu(MenuAction::Dance("disco".into())), 12_000);
        run(&mut b, 12_025, 14_000);
        ev(&mut b, AppEvent::Mouse(down), 14_000);
        ev(&mut b, AppEvent::Mouse(MouseEv::Move { x: 0.0, y: 0.0, sx: 530, sy: 520 }), 14_020);
        assert!(b.routine.is_none());
        assert_eq!(b.state, St::Drag);
        assert!(b.calls.iter().any(|c| c == "dance-end:disco:cancel"));
        assert!(b.song.is_none());
    }

    #[test]
    fn song_ending_early_falls_back_to_builtin_beat() {
        let mut b = mk();
        run(&mut b, 0, 3000);
        ev(&mut b, AppEvent::Menu(MenuAction::Dance("wiggle".into())), 3000);
        // 假装放的是歌
        b.routine.as_mut().unwrap().song = true;
        let tok = b.song.unwrap();
        ev(&mut b, AppEvent::AudioDone { token: tok }, 5000);
        assert_eq!(b.state, St::Dance, "keeps dancing");
        assert!(b.calls.iter().any(|c| c == "dance-beat-fallback"));
        let tok2 = b.song.unwrap();
        assert_ne!(tok, tok2);
        // 鼓点也停了：没有第二次兜底，继续按内部时钟跳完
        ev(&mut b, AppEvent::AudioDone { token: tok2 }, 6000);
        assert_eq!(b.state, St::Dance);
        assert!(b.song.is_none());
        run(&mut b, 6025, 3000 + 20_000 + 50);
        assert_eq!(b.state, St::Love);
    }

    #[test]
    fn too_tired_to_dance() {
        let mut b = mk();
        run(&mut b, 0, 3000);
        b.stats.data.energy = 5.0;
        ev(&mut b, AppEvent::Menu(MenuAction::Dance(String::new())), 3000);
        assert!(b.routine.is_none());
        assert_eq!(b.state, St::Sit);
    }
}
