//! 语音：移植 Electron 版 src/main/tts.js（MiMo TTS 三种模式、内置台词 ogg、tts-cache 缓存、单请求+429 冷却、预热）
//! 内置台词：assets/voice/lines/<sha1(text) 前16位>.ogg，用 crate::assets::get 读
//! TODO(B)：完整实现
#![allow(dead_code)]
use crate::types::{AppEvent, EventSender, SpeakKind, VoiceMood};

/// 合成/取缓存，完成后 tx.send(AppEvent::Speech { token, audio })。
/// 根据 config.tts 判断：enabled=false / speak=off / speak=chat 且 kind=Line → audio=None。
/// 新请求来了要掐掉旧的在途请求（旧的回 None）。
pub fn speak(tx: &EventSender, token: u64, text: String, mood: Option<VoiceMood>, kind: SpeakKind) {
    let _ = (text, mood, kind);
    tx.send(AppEvent::Speech { token, audio: None });
}

/// 后台慢慢预合成固定台词（最多 300 条），前台说话时让路
pub fn prewarm(items: Vec<(String, Option<VoiceMood>)>) {
    let _ = items;
}
