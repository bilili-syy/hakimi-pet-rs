//! AI 聊天：移植 Electron 版 src/main/ai.js（persona 提示词、Gemini/OpenAI 兼容、20 条记忆、错误文案）
//! TODO(B)：完整实现
#![allow(dead_code)]
use crate::types::{AppEvent, ChatReply, EventSender};

/// 拼 persona 用的当前状态
#[derive(Clone, Debug)]
pub struct ChatCtx {
    pub hunger: u32,
    pub mood: u32,
    pub energy: u32,
    /// 亲密度等级名
    pub level: String,
    pub days: u32,
}

/// 后台线程发请求，完成后 tx.send(AppEvent::ChatReply(..))
pub fn chat(tx: &EventSender, text: String, ctx: ChatCtx) {
    let _ = (text, ctx);
    tx.send(AppEvent::ChatReply(ChatReply {
        ok: false,
        need_key: true,
        text: "哈基米的大脑还没接上喵……右键我选「设置」，把 Gemini API Key 填进去就能聊天啦！".into(),
    }));
}

/// 清空 chat.json
pub fn clear_history() {}
