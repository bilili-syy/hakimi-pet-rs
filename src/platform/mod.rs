//! 平台层（负责人：A 路「窗口与系统」）：Win32 分层窗口、消息循环、鼠标、拖拽/下落/溜达物理、
//! 托盘与右键菜单、聊天输入框、单实例、开机自启、摸鱼监督、提醒与番茄钟。
//! 公开函数签名是约定，别改名改参数；内部实现随意。
#![allow(dead_code)]
use crate::types::{EventSender, Handler};

/// 创建窗口、托盘，跑消息循环直到退出。
/// make 拿到 EventSender（内部 PostMessage 到 UI 线程）后构造 Handler。
pub fn run(make: impl FnOnce(EventSender) -> Box<dyn Handler>) {
    // TODO(A)：真正的实现。现在只是让程序能编译运行。
    let tx = EventSender::new(|_ev| {});
    let mut h = make(tx);
    h.quit();
}

/// 番茄钟剩余分钟，没开返回 None
pub fn pomodoro_left() -> Option<u32> { None }

/// 今天摸鱼了几分钟；摸鱼监督关闭时返回 None
pub fn moyu_today() -> Option<u32> { None }

/// 用资源管理器 / 默认程序打开文件或文件夹（ShellExecuteW）
pub fn open_path(path: &std::path::Path) { let _ = path; }
