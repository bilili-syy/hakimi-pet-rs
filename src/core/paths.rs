//! 数据目录：%APPDATA%\hakimi-pet\（和 Electron 版共用）
use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("hakimi-pet")
}
pub fn config_file() -> PathBuf { data_dir().join("config.json") }
pub fn save_file() -> PathBuf { data_dir().join("save.json") }
pub fn chat_file() -> PathBuf { data_dir().join("chat.json") }
pub fn tts_cache_dir() -> PathBuf { data_dir().join("tts-cache") }
pub fn songs_dir() -> PathBuf { data_dir().join("songs") }
pub fn log_file() -> PathBuf { data_dir().join("hakimi.log") }
