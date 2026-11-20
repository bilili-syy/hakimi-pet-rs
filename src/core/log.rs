//! 追加写 hakimi.log。TODO(B)：带 ISO 时间戳、文件过大时截断
use std::io::Write;

pub fn log(msg: &str) {
    let _ = std::fs::create_dir_all(super::paths::data_dir());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(super::paths::log_file()) {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let _ = writeln!(f, "[{secs}] {msg}");
    }
    #[cfg(debug_assertions)]
    eprintln!("{msg}");
}
