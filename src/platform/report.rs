//! 每周报告图：整理数据 → 画（render/report.rs）→ 存到「图片\哈基米」并复制到剪贴板。
//! 设置窗口「周报」页调这里。另外放「导出聊天记录」（同样是往用户文件夹里存一份东西）。
use crate::render::report;
use std::path::PathBuf;

/// 报告存放的文件夹：图片\哈基米（拿不到「图片」文件夹就放数据目录）
pub fn dir() -> PathBuf {
    known(&windows::Win32::UI::Shell::FOLDERID_Pictures).unwrap_or_else(crate::core::paths::data_dir).join("哈基米")
}

/// 聊天记录导出到：文档\哈基米
pub fn docs_dir() -> PathBuf {
    known(&windows::Win32::UI::Shell::FOLDERID_Documents).unwrap_or_else(crate::core::paths::data_dir).join("哈基米")
}

fn known(id: &windows::core::GUID) -> Option<PathBuf> {
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoTaskMemFree, CoUninitialize};
    use windows::Win32::UI::Shell::{KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    // SAFETY: 确保当前线程已初始化 COM；返回的字符串用完交给 CoTaskMemFree
    unsafe {
        let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).is_ok();
        let res = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok().and_then(|p| {
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const _));
            s.map(PathBuf::from)
        });
        if com {
            CoUninitialize();
        }
        res
    }
}

/// 当前这周的数据 + 主人勾的记忆
pub fn current(memories: Vec<String>) -> report::Report {
    let stats = crate::core::save::load().unwrap_or_default();
    let first_met = if stats.first_met > 0.0 { date_of_ms(stats.first_met) } else { String::new() };
    report::build(&crate::core::usage::week(), &first_met, crate::core::stats::level_name(stats.affection), memories)
}

/// 毫秒时间戳 → 当地日期（用系统时区换算）
fn date_of_ms(ms: f64) -> String {
    let d = date_of_ms_full(ms);
    if d.len() >= 10 { d[..10].to_string() } else { String::new() }
}

/// 毫秒时间戳 → 当地「11-21 14:05」
fn clock_of_ms(ms: f64) -> String {
    let d = date_of_ms_full(ms);
    if d.len() >= 16 { d[5..].to_string() } else { String::new() }
}

/// 毫秒时间戳 → 当地「2026-11-21 14:05」
fn date_of_ms_full(ms: f64) -> String {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    if !ms.is_finite() {
        return String::new();
    }
    let ft100 = (ms.max(0.0) as u64).saturating_mul(10_000).saturating_add(116_444_736_000_000_000);
    let ft = FILETIME { dwLowDateTime: ft100 as u32, dwHighDateTime: (ft100 >> 32) as u32 };
    let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    // SAFETY: 都是本地栈上的结构体
    unsafe {
        if FileTimeToSystemTime(&ft, &mut utc).is_err() || SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_err() {
            return String::new();
        }
    }
    format!("{:04}-{:02}-{:02} {:02}:{:02}", local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute)
}

/// 导出错误分类：Empty = 没有记录，Io = 写盘失败
pub enum ExportErr {
    Empty,
    Io(String),
}

/// 把完整聊天记录导出成 Markdown（文档\哈基米\和哈基米的聊天-日期时间.md），返回路径
pub fn export_chat() -> Result<PathBuf, ExportErr> {
    let items = crate::services::ai::load_log();
    if items.is_empty() {
        return Err(ExportErr::Empty);
    }
    let n = crate::core::memo::Now::get().dt;
    let stamp = format!("{:04}-{:02}-{:02} {:02}:{:02}", n.y, n.mo, n.d, n.h, n.mi);
    let md = crate::services::ai::log_markdown(&items, &stamp, clock_of_ms);
    let d = docs_dir();
    std::fs::create_dir_all(&d).map_err(|e| ExportErr::Io(format!("建文件夹失败：{e}")))?;
    let path = d.join(format!("和哈基米的聊天-{:04}{:02}{:02}-{:02}{:02}.md", n.y, n.mo, n.d, n.h, n.mi));
    // 带 BOM：记事本老版本也认得是 UTF-8
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(md.as_bytes());
    std::fs::write(&path, bytes).map_err(|e| ExportErr::Io(format!("存文件失败：{e}")))?;
    Ok(path)
}

/// 资源管理器里打开文件夹并选中这个文件
pub fn reveal(path: &std::path::Path) {
    use std::os::windows::process::CommandExt;
    let arg = format!("/select,\"{}\"", path.display());
    let _ = std::process::Command::new("explorer.exe").raw_arg(arg).spawn();
}

/// 画好、存盘、复制。成功返回 (文件路径, 是否已成功复制到剪贴板)
pub fn make(memories: Vec<String>) -> Result<(PathBuf, bool), String> {
    let r = current(memories);
    let costume = crate::core::config::get().costume;
    let face = crate::services::emotion::Emotion::ALL
        .iter()
        .find(|e| format!("{e:?}") == r.top_face_key)
        .map(|e| e.face())
        .unwrap_or(crate::types::Face::Happy);
    let sprite = report::resolve_sprite(&costume, face);
    let mut te = crate::render::text::TextEngine::new();
    let pm = report::draw_pixmap(&r, sprite, &mut te).ok_or("画图失败")?;
    let png = pm.encode_png().map_err(|e| format!("编码 PNG 失败：{e}"))?;
    let d = dir();
    std::fs::create_dir_all(&d).map_err(|e| format!("建文件夹失败：{e}"))?;
    let path = d.join(format!("哈基米周报-{}.png", r.today));
    std::fs::write(&path, &png).map_err(|e| format!("存图失败：{e}"))?;
    let copied = match copy_to_clipboard(&pm) {
        Ok(()) => true,
        Err(e) => {
            crate::core::log::log(&format!("report clipboard: {e}"));
            false
        }
    };
    Ok((path, copied))
}

/// 把图（预乘 RGBA）转成 CF_DIB 放进剪贴板，微信/QQ 里直接粘贴
pub fn copy_to_clipboard(pm: &tiny_skia::Pixmap) -> Result<(), String> {
    use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    const CF_DIB: u32 = 8;
    let dib = dib_bytes(pm);
    // SAFETY: 标准剪贴板流程；SetClipboardData 成功后内存归系统，失败才自己释放
    unsafe {
        let h: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, dib.len()).map_err(|e| e.to_string())?;
        let p = GlobalLock(h) as *mut u8;
        if p.is_null() {
            let _ = GlobalFree(Some(h));
            return Err("GlobalLock".into());
        }
        std::ptr::copy_nonoverlapping(dib.as_ptr(), p, dib.len());
        let _ = GlobalUnlock(h);
        if OpenClipboard(None).is_err() {
            let _ = GlobalFree(Some(h));
            return Err("OpenClipboard".into());
        }
        let _ = EmptyClipboard();
        let r = SetClipboardData(CF_DIB, Some(HANDLE(h.0)));
        let _ = CloseClipboard();
        if r.is_err() {
            let _ = GlobalFree(Some(h));
            return Err("SetClipboardData".into());
        }
    }
    Ok(())
}

/// BITMAPINFOHEADER + 自下而上的 BGRA（图是不透明底，直接去预乘也安全）
pub fn dib_bytes(pm: &tiny_skia::Pixmap) -> Vec<u8> {
    let (w, h) = (pm.width() as usize, pm.height() as usize);
    let mut out = Vec::with_capacity(40 + w * h * 4);
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&((w * h * 4) as u32).to_le_bytes());
    out.extend_from_slice(&[0u8; 16]);
    let data = pm.data();
    for y in (0..h).rev() {
        for x in 0..w {
            let i = (y * w + x) * 4;
            out.extend_from_slice(&[data[i + 2], data[i + 1], data[i], data[i + 3]]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dib_layout() {
        let mut pm = tiny_skia::Pixmap::new(2, 2).unwrap();
        pm.fill(tiny_skia::Color::from_rgba8(10, 20, 30, 255));
        let d = dib_bytes(&pm);
        assert_eq!(d.len(), 40 + 16);
        assert_eq!(&d[0..4], &40u32.to_le_bytes());
        assert_eq!(&d[40..44], &[30, 20, 10, 255]);
    }

    #[test]
    fn clock_of_ms_shape() {
        let s = clock_of_ms(1_795_262_400_000.0);
        assert_eq!(s.len(), 11, "{s}");
        assert_eq!(&s[2..3], "-");
    }

    #[test]
    fn local_date_of_ms() {
        // 2026-11-21 12:00 UTC：不管哪个时区都还在 11-21 或相邻一天
        let s = date_of_ms(1_795_262_400_000.0);
        assert!(s.starts_with("2026-11-2"), "{s}");
    }
}
