//! 养成数值：移植 Electron 版 src/core/stats.js（数值、速率、等级表必须一致）
//! TODO(B)：完整实现 + 单元测试。下面的签名是约定，behavior 层直接调用。
#![allow(dead_code)]
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StatsData {
    pub hunger: f64,
    pub mood: f64,
    pub energy: f64,
    pub affection: f64,
    /// 毫秒时间戳
    pub last_tick: f64,
    pub total_feeds: u32,
    pub first_met: f64,
}
impl Default for StatsData {
    fn default() -> Self {
        let now = now_ms();
        Self { hunger: 80.0, mood: 80.0, energy: 80.0, affection: 0.0, last_tick: now, total_feeds: 0, first_met: now }
    }
}

/// stats.js 里 mood() 的返回值
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoodLevel { Starving, Hungry, Exhausted, Sad, Great, Normal }

/// act() 的种类
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Act { Feed, Pat, Poke, Angry, Drag, Chat, Wake }

/// act() 的结果（目前只有 feed 会返回 Full）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActResult { Ok, Full }

pub struct Stats {
    pub data: StatsData,
}

impl Stats {
    /// 载入存档并做离线补算；返回实际离开的分钟数（用于打招呼）
    pub fn load(saved: Option<StatsData>) -> (Self, f64) {
        (Self { data: saved.unwrap_or_default() }, 0.0)
    }
    /// 按距上次的时间流逝更新（单次最多算 5 分钟）
    pub fn tick(&mut self, sleeping: bool) { let _ = sleeping; }
    pub fn act(&mut self, kind: Act) -> ActResult { let _ = kind; ActResult::Ok }
    /// (等级下标, 等级名)：初次见面/眼熟/朋友/好朋友/形影不离/家人
    pub fn level(&self) -> (usize, &'static str) { (0, "初次见面") }
    pub fn mood(&self) -> MoodLevel { MoodLevel::Normal }
    /// 认识第几天（从 1 开始）
    pub fn days(&self) -> u32 { 1 }
}

pub fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0)
}
