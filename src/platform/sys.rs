//! 系统杂项：单实例、开机自启（HKCU\...\Run）、ShellExecute 打开路径、宽字符串工具。
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, ERROR_SUCCESS, GetLastError, HANDLE, WPARAM, LPARAM};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::Win32::System::StationsAndDesktops::{GetThreadDesktop, GetUserObjectInformationW, UOI_NAME};
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentThreadId};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, SW_SHOWNORMAL};
use windows::core::{HSTRING, PCWSTR, w};

pub const WINDOW_CLASS: PCWSTR = w!("HakimiPetRsWindow");
/// 单实例锁名前缀；后面拼上当前桌面名（见 single_instance）
const MUTEX_PREFIX: &str = "Local\\HakimiPetRs.SingleInstance";
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("HakimiPet");

/// 以 NUL 结尾的 UTF-16
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 抢单实例锁。抢到返回 Some(句柄，进程存活期间别关)；已有实例则通知它（发 msg）并返回 None。
///
/// 锁按「会话 + 桌面」区分：Local\ 只到会话这一级，而 FindWindowW 只找得到同一桌面上的窗口。
/// 以前另一个桌面（比如服务/远程工具的隐藏桌面）上残留一只，用户在自己桌面上双击就会被当成第二实例
/// 静默退出，桌面上什么都看不到。现在不同桌面各管各的。
pub fn single_instance(msg: u32) -> Option<HANDLE> {
    let name = wide(&format!("{MUTEX_PREFIX}.{}", desktop_name().unwrap_or_default()));
    unsafe {
        let h = CreateMutexW(None, false, PCWSTR(name.as_ptr())).ok()?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            // 等第一个实例窗口建好（它可能也刚启动）
            for _ in 0..20 {
                if let Ok(w) = FindWindowW(WINDOW_CLASS, PCWSTR::null())
                    && !w.0.is_null()
                {
                    let _ = PostMessageW(Some(w), msg, WPARAM(0), LPARAM(0));
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            crate::core::log::log("second instance: existing pet window not found on this desktop");
            return None;
        }
        Some(h)
    }
}

/// 当前线程所在桌面的名字（一般是 "Default"）
fn desktop_name() -> Option<String> {
    unsafe {
        let d = GetThreadDesktop(GetCurrentThreadId()).ok()?;
        let mut buf = [0u16; 256];
        let mut need = 0u32;
        GetUserObjectInformationW(
            HANDLE(d.0),
            UOI_NAME,
            Some(buf.as_mut_ptr() as *mut _),
            (buf.len() * 2) as u32,
            Some(&mut need),
        )
        .ok()?;
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..n]).replace('\\', "_"))
    }
}

fn exe_cmd() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(format!("\"{}\"", exe.display()))
}

/// 开机自启是否开着（Run 里有我们的项且指向当前 exe）
pub fn is_autostart() -> bool {
    let Some(want) = exe_cmd() else { return false };
    unsafe {
        let mut buf = [0u16; 1024];
        let mut size = (buf.len() * 2) as u32;
        let r = RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            RUN_VALUE,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        );
        if r != ERROR_SUCCESS {
            return false;
        }
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..n]).eq_ignore_ascii_case(&want)
    }
}

pub fn set_autostart(on: bool) {
    unsafe {
        let r = if on {
            let Some(cmd) = exe_cmd() else { return };
            let v = wide(&cmd);
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                RUN_VALUE,
                REG_SZ.0,
                Some(v.as_ptr() as *const _),
                (v.len() * 2) as u32,
            )
        } else {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE)
        };
        if r != ERROR_SUCCESS {
            crate::core::log::log(&format!("autostart {on} failed: {:?}", r));
        }
    }
}

/// 用资源管理器 / 默认程序打开文件或文件夹
pub fn open_path(path: &std::path::Path) {
    let p = HSTRING::from(path.as_os_str());
    unsafe {
        ShellExecuteW(None, w!("open"), &p, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}
