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
| OpenSettings | 打开设置窗口（platform/settings.rs，按需创建、关掉即销毁） |
| OpenConfigFile / OpenDataDir / OpenSongDir | 打开 config.json / 数据目录 / songs 目录（不存在先建） |
| ReloadSettings | 在 Handler 之后重排提醒定时器、按 moyu.enabled 启停监督、套用「哈基米大小」（scale） |
| Moyu(on) | 在 Handler（它会写 config）之后启停监督 |
| Quit | 调 `Handler::quit` 然后退出 |
| Memo(OpenFile) | 打开 memo.json（不存在先建）；其余 Memo(..) 由行为层调 core::memo 处理 |
| Mind(OpenFile) / Mind(ClearAsk) | 打开 memory.json / 弹框二次确认，确认了再投递 `Mind(Clear)`（行为层清空） |

## 备忘录
- 数据：core/memo.rs（memo.json 读写、到点计算，全局 Mutex，任何线程可调）
- 解析：services/memo_parse.rs（本地中文时间规则 → 不懂再问 AI）
- 定时：平台层 rearm 按 `core::memo::ms_until_next` 排一次性定时器（最多 5 分钟核对一次），tick 里 `core::memo::poll` 到点发 `AppEvent::Memo(MemoEv::Due)`；睡眠醒来/改系统时间（WM_POWERBROADCAST/WM_TIMECHANGE）立刻重排，错过的合并成一次
- 菜单：platform/memo_menu.rs；行为：behavior/memo.rs

其余（Chat、Feed、Pat、Honey、Song、Status、Sleep、Walk、Costume、Outfit、ClearOutfit、Tts、TtsChatOnly、ForgetChat、Dance）只由行为层处理；
其中会改配置的（Costume/Outfit/Tts/TtsChatOnly/Moyu）由行为层调 `core::config::update` 写入，平台层每次弹菜单时读 `core::config::get()` 决定勾选状态。

## 右键菜单（数据驱动）
- 整个菜单在 `platform/menu.rs` 的 `MENU` 表里：顶层只有 聊天 / 喂养▸ / 互动▸ / 娱乐▸ / 工具▸ / 换装▸ / 状态 / 设置… / 藏起来 / 退出。
- 加功能 = 在对应分组里加一行 `Item("id", "文字")`，再在 `parse_id` 里把 id 映射成 `MenuAction`；`table_integrity` 测试会检查 id 唯一、都能映射、顶层 ≤ 10 项。
- 勾选 / 动态文字 / 能不能点由 `checked` / `label` / `enabled` 三个纯函数决定；舞谱和备忘录是运行时生成的子菜单（`Gen("dance")` / `Gen("memo")`）。
- 托盘：同一份菜单；单击 = 藏起来/叫出来（等过双击时间才执行），双击 = 打开设置。
- 自测：`WM_APP+9` 下标 10 = 打开设置窗口、11 = 打开聊天框；`WM_APP+10` = 在光标处弹出右键菜单。

## 设置窗口
- `platform/settings.rs`：原生 Win32 控件 + 临时 comctl32 v6 激活上下文（程序本身没有清单）。三页：AI 聊天 / 语音 / 常规。
- 保存：`core::config::replace` 原子写回（未知字段保留），再走 `settings:reload`。Key 不进日志。
- 「测试连接」= `services::ai::test_connection`（用窗口里没保存的值），「试听」= `services::tts::synth_test`；后台线程跑，PostMessage 回窗口。
- config 新字段：`weatherCity`（空 = 按 IP）、`scale`（0.5~2.0）、`tts.chatVoiceReply`（语音回答，默认开）。

## 语音回答（behavior/chatvoice.rs）
- `tts.chatVoiceReply` 开且 TTS 可用：AI 回复到了先保持「思考中」，按句切开（最多 4 段），第一段合成好才出字并同时开念；
  后面每段在前一段念的时候预合成，念完才出下一段字。某段合成失败或 15 秒没好：剩下的字直接全显示，不出声。
- 关掉：聊天回复只出字、不念（其它台词照旧）。

## 情绪表情 + 长期记忆（feat/memory-emotion）
- 情绪标签：`services/emotion.rs`。系统提示词要求回复以 `[happy]` 等标签开头（只用现有立绘名）；`analyze(text)` 去标签（含【】［］、大小写、正文里混进来的）→ 没有/不认识就按关键词猜 → idle。流式回复用 `TagStream`（标签被拆开也能认，半截标签先扣着）。
  行为层在 `on_chat_reply` 一开始调 `emotion_split`，气泡和 TTS 只拿到干净文字；说话期间换表情 + 小动作，念完（AudioDone）或估算说完后 2 秒回平时。
- 记忆数据：`core/memory.rs`（%APPDATA%\hakimi-pet\memory.json，每条带 created/updated，合并去重、新的覆盖旧的、上限 60、称呼/生日最后才丢；忘掉过的内容记个墓碑，后台提取不再捡回来；未知字段保留）。
- 提示词：`ai::persona` 末尾追加 emotion::PROMPT 和 `core::memory::prompt()`（≤600 字）。
- 提取/关心：`services/mind.rs`。每 4 轮成功聊天后台线程用同一个模型（ai::ask_once）总结 chat.json 里新消息的事实（JSON），合并进去；主动关心每天最多 2 次、间隔 ≥3 小时、9–22 点，只在没开聊天框、没在响提醒、气泡空着时问，结果用 `AppEvent::Mind(MindEv::Care)` 送回。
- 聊天指令：「记住…」立即写入、「忘掉…/把…忘了」删除、「你记得我什么」列几条；行为在 `behavior/emotion.rs`。菜单 `platform/mind_menu.rs`。
- 记忆内容和 Key 不写日志。

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

## 新互动（feat/interactions）
- 行为：`behavior/play.rs`（`Play` 状态 + `impl Behavior` 钩子）。mod.rs 里只有几个钩子调用：`sync_play` / `on_play_timer` / `think_play` / `landed_play` / `maybe_checkin`。新功能尽量也照这个样子单独放文件。
- 平台：`platform/perch.rs`（爬前台窗口：资格判断是纯函数）、`platform/toy.rs`（小道具分层窗口：小鱼干/罐头可拖，逗猫棒点穿跟随光标）。
  滚轮 `WM_MOUSEWHEEL` → `MouseEv::Wheel`；`WinCmd::CursorPoll(ms)` 打开慢速 GetCursorPos 轮询（只在可见时，位置变了才发 `AppEvent::Cursor`）。
  自测钩子：`PostMessage(hwnd, WM_APP+9, i, 0)` = 点 `DEBUG_MENU[i]`。
- 天气：`services/weather.rs`，公开 `refresh(tx)`（后台拉 wttr.in，结果发 `AppEvent::Weather` 并缓存）、`latest() -> Option<WeatherInfo>`、`report_line(&w)`。
- 存档新增字段（save.json，camelCase）：`checkinLast` `checkinStreak` `checkinTotal` `gameBest` `weatherReportLast`；未知字段照旧保留。
- 菜单新增：扔小鱼干（feed）/ 直接喂（feednow）/ 逗猫棒（game）/ 签到（checkin）/ 天气播报（weather）/ 爬窗口（climb）。
