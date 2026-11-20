// 哈基米桌宠 · Rust 版 —— 入口
// 模块归属见 docs/TASKS.md；跨模块类型在 types.rs（冻结）
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
    core::config::reload();
    platform::run(|tx| Box::new(app::App::new(tx)));
}
