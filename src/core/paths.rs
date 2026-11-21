//! 数据目录：%APPDATA%\hakimi-pet\（和 Electron 版共用）
//! 切换到自定义角色包 `<id>` 时，聊天记录与长期记忆自动隔离到 `skins-data\<id>\`，内置哈基米保持原路径不变。
use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("hakimi-pet")
}

/// 纯函数：给定数据根目录和当前角色 ID，计算该角色的数据目录（便于单元测试在临时目录下验证隔离逻辑）
pub fn skin_data_dir_in(base: &std::path::Path, id: &str) -> PathBuf {
    if id.is_empty() || id == super::skin::BUILTIN_ID {
        return base.to_path_buf();
    }
    super::skin::safe_single_id(id)
        .map(|safe| base.join("skins-data").join(safe))
        .unwrap_or_else(|| base.to_path_buf())
}

/// 当前角色的专属数据目录：内置哈基米走根目录 `data_dir()`（100% 兼容旧存档）；
/// 自定义角色走 `data_dir()/skins-data/<id>`，互不串记忆、互不串聊天记录，且打包分享 `skins/<id>` 时不会误把私人记忆带出去。
pub fn active_skin_data_dir() -> PathBuf {
    skin_data_dir_in(&data_dir(), &super::skin::active_id())
}

pub fn config_file() -> PathBuf { data_dir().join("config.json") }
pub fn save_file() -> PathBuf { data_dir().join("save.json") }
pub fn chat_file() -> PathBuf { active_skin_data_dir().join("chat.json") }
pub fn tts_cache_dir() -> PathBuf { data_dir().join("tts-cache") }
pub fn songs_dir() -> PathBuf { data_dir().join("songs") }
pub fn log_file() -> PathBuf { data_dir().join("hakimi.log") }
