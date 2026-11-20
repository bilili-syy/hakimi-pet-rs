//! 用户自己放的歌：%APPDATA%\hakimi-pet\songs\
#![allow(dead_code)]
use std::path::PathBuf;

pub const EXTS: [&str; 6] = ["mp3", "ogg", "wav", "m4a", "flac", "aac"];

/// 打开（必要时先创建）歌曲文件夹
pub fn open_dir() {
    let _ = std::fs::create_dir_all(crate::core::paths::songs_dir());
    // TODO(B)：ShellExecuteW 打开文件夹（或调用 platform 提供的 open_path）
}

/// 随机挑一首 ≤20MB 的支持格式的歌，没有返回 None
pub fn pick() -> Option<PathBuf> { None }
