//! 音频播放（rodio）。同一时间只放一个声音：新的 play 会先停掉旧的（旧的发 AudioDone）。
//! TODO(B)：完整实现。rodio 的输出流放在一个常驻后台线程里，按需懒初始化。
#![allow(dead_code)]
use crate::types::{AppEvent, Audio, EventSender};
use std::path::PathBuf;

/// 启动时调用一次，保存 tx 用来发 AudioDone
pub fn init(tx: EventSender) { let _ = tx; }

/// 播放编码好的音频（wav/ogg），volume 0~1；放完发 AppEvent::AudioDone { token }
pub fn play(token: u64, audio: Audio, volume: f32) {
    let _ = (token, audio, volume);
}

/// 播放本地歌曲文件（mp3/ogg/wav/m4a/flac/aac），最多 max_secs 秒；放完或到时发 AudioDone
pub fn play_file(token: u64, path: PathBuf, volume: f32, max_secs: u32) {
    let _ = (token, path, volume, max_secs);
}

/// 停止当前声音
pub fn stop() {}

/// 当前是否在出声（行为层用来判断气泡要不要多留一会儿）
pub fn is_playing() -> bool { false }
