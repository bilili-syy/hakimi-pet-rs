//! 用户自己放的歌：%APPDATA%\hakimi-pet\songs\
//! 原版 main.js 的 song-pick / openSongDir。播放走 services::audio::play_file（边读边解码，不进内存）。
#![allow(dead_code)]
use std::path::{Path, PathBuf};

pub const EXTS: [&str; 6] = ["mp3", "ogg", "wav", "m4a", "flac", "aac"];
/// 超过 20MB 的不放（原版限制）
pub const MAX_BYTES: u64 = 20 * 1024 * 1024;

/// 打开（必要时先创建）歌曲文件夹
pub fn open_dir() {
    let dir = crate::core::paths::songs_dir();
    let _ = std::fs::create_dir_all(&dir);
    open_in_shell(&dir);
}

#[cfg(windows)]
fn open_in_shell(p: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{PCWSTR, w};
    let wide: Vec<u16> = p.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    // SAFETY: wide 以 0 结尾，调用期间一直有效
    unsafe { ShellExecuteW(None, w!("open"), PCWSTR(wide.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
}
#[cfg(not(windows))]
fn open_in_shell(_p: &Path) {}

/// 扩展名是不是支持的格式（大小写不敏感）
pub fn supported(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// 随机挑一首 ≤20MB 的支持格式的歌，没有返回 None
pub fn pick() -> Option<PathBuf> { pick_in(&crate::core::paths::songs_dir()) }

pub fn pick_in(dir: &Path) -> Option<PathBuf> {
    let files: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .filter(|p| supported(p))
        .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.len() <= MAX_BYTES))
        .collect();
    if files.is_empty() { return None; }
    Some(files[fastrand::usize(..files.len())].clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_filters() {
        let dir = std::env::temp_dir().join(format!("hakimi-songs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(pick_in(&dir).is_none());
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        assert!(pick_in(&dir).is_none());
        std::fs::write(dir.join("b.MP3"), b"x").unwrap();
        assert_eq!(pick_in(&dir).unwrap().file_name().unwrap(), "b.MP3");
        assert!(supported(Path::new("x.flac")) && supported(Path::new("x.M4A")) && !supported(Path::new("x")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
