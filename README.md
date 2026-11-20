# 哈基米桌宠 · Rust 版 🐱🦀

[hakimi-pet](https://github.com/bilili-syy/hakimi-pet)（Electron）的纯 Rust 重写。目标：功能一个不少，占用越小越好。

| | Electron 0.8.1 | Rust 目标 |
|---|---|---|
| 内存 | 约 315MB（6 个进程 + PowerShell） | 20–40MB，单进程 |
| 体积 | 安装包约 95MB | 5–10MB 单 exe |

## 技术栈
- 窗口：Win32 分层窗口（UpdateLayeredWindow，逐像素透明，透明处天然点穿）
- 绘制：tiny-skia 软渲染
- 文字：系统微软雅黑
- 托盘/菜单：tray-icon + muda
- 声音：rodio
- 网络：ureq（读系统代理）
- 摸鱼监督：直接调 user32，不再起 PowerShell

## 设置
右键哈基米 →「⚙️ 设置…」（或双击托盘图标）：选 AI 服务商、填 API Key（每家都有「如何获取 API Key？」官网链接和「测试连接」按钮）、语音、开机自启、大小、天气城市、提醒等。

## 数据
继续用 `%APPDATA%\hakimi-pet\`（save.json、config.json、chat.json、tts-cache、songs），和 Electron 版互通。

## 开发
```
cargo run
cargo build --release
```
工具链：stable-x86_64-pc-windows-gnu（不需要装 Visual Studio）。
