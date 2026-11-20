//! 渲染层（负责人：C 路「渲染与行为」，集成负责人）
//! tiny-skia 软渲染：立绘 + 尾巴/星星分层动画 + 换装 + 特效 + 气泡 + 状态卡 + zzZ
#![allow(dead_code)]
use crate::types::{Face, Frame, WIN_H, WIN_W};

/// 行为层每帧交给渲染层的场景描述
#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub face: Option<Face>,
}

pub struct Renderer {
    frame: Frame,
    dirty: bool,
}

impl Renderer {
    pub fn new() -> Self {
        Self { frame: Frame { w: 0, h: 0, bgra: Vec::new() }, dirty: true }
    }
    pub fn render(&mut self, scene: &Scene, scale: f32) -> Option<&Frame> {
        let _ = scene;
        let w = (WIN_W as f32 * scale).round() as u32;
        let h = (WIN_H as f32 * scale).round() as u32;
        if !self.dirty && self.frame.w == w && self.frame.h == h { return None; }
        self.frame = Frame { w, h, bgra: vec![0; (w * h * 4) as usize] };
        self.dirty = false;
        Some(&self.frame)
    }
}
