//! 行为层（负责人：C 路）：移植 renderer.js 的状态机、台词、互动、提醒/摸鱼反应
#![allow(dead_code)]
use crate::render::Scene;
use crate::types::{AppEvent, EventSender, WinCmd};

pub struct Behavior {
    tx: EventSender,
    pub scene: Scene,
}

impl Behavior {
    pub fn new(tx: EventSender) -> Self { Self { tx, scene: Scene::default() } }
    pub fn event(&mut self, ev: AppEvent, out: &mut Vec<WinCmd>) {
        if let AppEvent::Menu(crate::types::MenuAction::Quit) = ev { out.push(WinCmd::Quit); }
    }
    pub fn frame(&mut self, now_ms: u64, out: &mut Vec<WinCmd>) -> u32 { let _ = (now_ms, out); 200 }
    pub fn quit(&mut self) {}
}
