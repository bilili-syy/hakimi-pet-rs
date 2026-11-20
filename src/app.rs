//! 集成：把行为层和渲染层拼成 platform 要的 Handler（负责人：C 路）
use crate::behavior::Behavior;
use crate::render::Renderer;
use crate::types::{AppEvent, EventSender, Frame, Handler, WinCmd};

pub struct App {
    behavior: Behavior,
    renderer: Renderer,
}

impl App {
    pub fn new(tx: EventSender) -> Self {
        crate::services::audio::init(tx.clone());
        Self { behavior: Behavior::new(tx), renderer: Renderer::new() }
    }
}

impl Handler for App {
    fn event(&mut self, ev: AppEvent, out: &mut Vec<WinCmd>) { self.behavior.event(ev, out) }
    fn frame(&mut self, now_ms: u64, out: &mut Vec<WinCmd>) -> u32 { self.behavior.frame(now_ms, out) }
    fn render(&mut self, scale: f32) -> Option<&Frame> { self.renderer.render(&self.behavior.scene, scale) }
    fn quit(&mut self) { self.behavior.quit() }
}
