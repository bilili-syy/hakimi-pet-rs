//! 追加写 hakimi.log：`[2026-11-20T03:04:05.678Z] msg`（和原版 main.js log() 同格式，UTC ISO 时间）。
//! 文件超过 MAX_BYTES 时改名为 hakimi.log.old（覆盖旧的）再从头写，最多占 2×MAX_BYTES。
//! 注意：调用方别把 API Key 之类的敏感信息拼进 msg。
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// 单个日志文件上限
pub const MAX_BYTES: u64 = 1024 * 1024;

struct Sink {
    path: PathBuf,
    file: File,
    bytes: u64,
}

static SINK: Mutex<Option<Sink>> = Mutex::new(None);

fn open_sink(path: PathBuf) -> Option<Sink> {
    let _ = std::fs::create_dir_all(super::paths::data_dir());
    let mut len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if len >= MAX_BYTES {
        let _ = std::fs::rename(&path, path.with_extension("log.old"));
        len = 0;
    }
    let file = std::fs::OpenOptions::new().create(true).append(true).open(&path).ok()?;
    Some(Sink { path, file, bytes: len })
}

pub fn log(msg: &str) {
    let line = format!("[{}] {msg}\n", iso_now());
    {
        let mut g = SINK.lock().unwrap_or_else(|e| e.into_inner());
        let path = super::paths::log_file();
        if g.as_ref().is_some_and(|s| s.path != path || s.bytes >= MAX_BYTES) {
            *g = None;
        }
        if g.is_none() {
            *g = open_sink(path.clone());
        }
        if let Some(s) = g.as_mut() {
            if s.file.write_all(line.as_bytes()).is_ok() {
                s.bytes = s.bytes.saturating_add(line.len() as u64);
            } else {
                *g = open_sink(path);
                if let Some(s) = g.as_mut()
                    && s.file.write_all(line.as_bytes()).is_ok()
                {
                    s.bytes = s.bytes.saturating_add(line.len() as u64);
                }
            }
        }
    }
    #[cfg(debug_assertions)]
    eprintln!("{msg}");
}

/// 调试追踪：设了环境变量 HAKIMI_TRACE=1 才写（念什么、声音从哪来、播放/停止/播完），平时不写
pub fn trace(msg: &str) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("HAKIMI_TRACE").is_some_and(|v| !v.is_empty() && v != "0")) {
        log(&format!("trace {msg}"));
    }
}

/// 当前 UTC 时间，`Date.prototype.toISOString()` 格式
pub fn iso_now() -> String {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    iso(ms)
}

/// 毫秒时间戳 → `YYYY-MM-DDTHH:MM:SS.mmmZ`
pub fn iso(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let milli = ms.rem_euclid(1000);
    let days = secs.div_euclid(86400);
    let sod = secs.rem_euclid(86400);
    // Howard Hinnant 的 civil_from_days
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{milli:03}Z", sod / 3600, sod % 3600 / 60, sod % 60)
}

#[cfg(test)]
mod tests {
    #[test]
    fn iso_format() {
        assert_eq!(super::iso(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(super::iso(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(super::iso(1_763_600_000_123), "2025-11-20T00:53:20.123Z");
    }
}
