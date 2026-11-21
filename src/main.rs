// 哈基米桌宠 · Rust 版 —— 入口
// 模块结构见 docs/ARCHITECTURE.md；跨模块类型在 types.rs
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod assets;
mod behavior;
mod core;
mod platform;
mod render;
mod services;
mod types;

fn main() {
    core::log::log(&format!("start {}", env!("CARGO_PKG_VERSION")));
    // release 是 windows 子系统，panic 没有控制台可写，以前闪退什么都不留；记进日志方便排查
    std::panic::set_hook(Box::new(|info| {
        let bt = std::backtrace::Backtrace::force_capture();
        core::log::log(&format!("PANIC {info}\n{bt}"));
    }));
    core::config::reload();
    platform::run(|tx| Box::new(app::App::new(tx)));
}
