//! 集成：把行为层和渲染层拼成 platform 要的 Handler（负责人：C 路）
use crate::behavior::{Behavior, UiInfo};
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

    fn render(&mut self, scale: f32) -> Option<&Frame> {
        self.behavior.set_scale(scale);
        self.behavior.set_bubble_top(crate::platform::bubble_top_offset());
        let changed = self.renderer.update(&self.behavior.scene, scale);
        // 气泡/状态卡的位置回填给行为层做点击判定（点状态卡=关掉，点气泡不算戳她）
        self.behavior.set_ui(UiInfo {
            bubble: self.renderer.bubble_rect,
            status: self.renderer.status_rect,
            bubble_overflow: self.renderer.bubble_overflow,
        });
        if changed { Some(self.renderer.frame()) } else { None }
    }

    fn quit(&mut self) { self.behavior.quit() }
}
