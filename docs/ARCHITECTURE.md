# 架构

原版（Electron）在 `D:\download\luleme2\hakimi-pet`（只读参考，别改）。行为、数值、文案、配置字段一律以原版为准。

## 线程模型
- **UI 线程**：Win32 消息循环（platform），窗口、托盘、菜单、输入框都在这个线程。每帧调用 `Handler::frame` → `Handler::render` → `UpdateLayeredWindow`。
- **工作线程**：services 里的网络请求、TTS、音频播放。结果通过 `EventSender` 送回 UI 线程，平台层再调 `Handler::event`。
- 不用 async 运行时，不用 tokio。

## 模块
```
src/
  main.rs        入口                                   C
  types.rs       跨模块类型和 Handler 接口（冻结）       C
  assets.rs      include_dir 内嵌 assets/                C
  app.rs         Handler 实现：behavior + render 拼起来  C
  platform/      Win32 窗口/消息循环/托盘/菜单/输入框/单实例/自启/摸鱼监督/提醒/番茄钟   A
  core/          paths、log、config、save、stats                                     B
  services/      net(代理)、ai、tts、audio、songs                                     B
  render/        tiny-skia 绘制                                                       C
  behavior/      状态机、台词、互动                                                   C
```

## 原版 → Rust 对照
| 原版 | Rust |
|---|---|
| src/main/main.js 窗口、拖拽、下落、溜达、托盘、菜单、自启、提醒、番茄钟 | platform/ |
| src/main/watch.js（PowerShell） | platform/watch.rs（直接调 user32，不起子进程） |
| src/main/ai.js | core/config.rs + services/ai.rs + services/net.rs |
| src/main/tts.js | services/tts.rs |
| main.js 的 song-pick / 渲染进程 Audio | services/songs.rs + services/audio.rs |
| src/core/stats.js | core/stats.rs |
| src/renderer/renderer.js | behavior/ |
| src/renderer/style.css + index.html | render/ |

## 菜单动作谁来执行
平台层把**每个** `MenuAction` 都先交给 `Handler::event`，然后**自己**再执行下面这些：

| 动作 | 平台层额外做什么 |
|---|---|
| ToggleVisible / CallToCenter | 显示/隐藏、挪到工作区 62% 处地面；变可见时发 `AppEvent::Peek` |
| PomodoroStart(m) / PomodoroStop | 计时，发 `AppEvent::Pomodoro(..)`，刷新菜单文字 |
| AutoStart(on) | 写 HKCU\…\Run |
| OpenSettings / OpenDataDir / OpenSongDir | 打开 config.json / 数据目录 / songs 目录（不存在先建） |
| ReloadSettings | 在 Handler 之后重排提醒定时器、按 moyu.enabled 启停监督 |
| Moyu(on) | 在 Handler（它会写 config）之后启停监督 |
| Quit | 调 `Handler::quit` 然后退出 |

其余（Chat、Feed、Pat、Honey、Song、Status、Sleep、Walk、Costume、Outfit、ClearOutfit、Tts、TtsChatOnly、ForgetChat、Dance）只由行为层处理；
其中会改配置的（Costume/Outfit/Tts/TtsChatOnly/Moyu）由行为层调 `core::config::update` 写入，平台层每次弹菜单时读 `core::config::get()` 决定勾选状态。

## 跳个舞
- 舞谱在 `behavior/dance.rs` 的 `SCORES`（BPM、时长、按拍计长的动作段），加一条菜单就自动多一支舞。
- 动作关键帧在 `render/dance.rs`（纯函数）；节拍检测和内置鼓点在 `behavior/beat.rs`（后台线程边解码边算能量，不加依赖）。
- 跳舞时帧间隔 25ms，结束后回到原来的空闲帧率。

## 约定
- `types.rs` 冻结；各模块 `mod.rs`/公开函数签名是约定，改之前先说。
- 各路只改自己目录下的文件。`Cargo.toml` 依赖已经预先加好；真要加依赖，在 PR 描述里写明，由集成方合并时处理。
- 每个 PR 前：`cargo build` 和 `cargo test` 通过，`cargo clippy` 无新警告。
- 工具链：stable-x86_64-pc-windows-gnu。
- 内存目标：总常驻 < 40MB。别把所有立绘一次性解码，别把大文件读进内存转 base64。
