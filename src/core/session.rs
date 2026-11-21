//! 运行标记：启动时写 running.lock，正常退出时删掉。
//! 下次启动还看到它，说明上次是闪退、被强制结束或断电——记一行日志方便排查（panic 另有 PANIC 日志）。
use std::path::PathBuf;

fn marker() -> PathBuf { super::paths::data_dir().join("running.lock") }

/// 启动时调用（单实例检查之后）。返回上次是否没有正常退出。
pub fn begin() -> bool {
    let path = marker();
    let crashed = match std::fs::read_to_string(&path) {
        Ok(prev) => {
            super::log::log(&format!("上次没有正常退出（{}），可能是闪退、被强制结束或断电", prev.trim()));
            true
        }
        Err(_) => false,
    };
    let _ = std::fs::create_dir_all(super::paths::data_dir());
    let _ = std::fs::write(&path, format!("pid {} 启动于 {} 版本 {}", std::process::id(), super::log::iso_now(), env!("CARGO_PKG_VERSION")));
    crashed
}

/// 正常退出时调用
pub fn end() {
    let _ = std::fs::remove_file(marker());
}
