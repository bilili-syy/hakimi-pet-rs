//! 服务层（负责人：B 路「服务与数据」）：网络、AI、TTS、音频、放歌
//! 公开函数签名是约定，别改名改参数；内部实现随意。耗时操作一律放后台线程，结果用 EventSender 送回。
pub mod ai;
pub mod audio;
pub mod emotion;
pub mod memo_parse;
pub mod mind;
pub mod net;
pub mod songs;
pub mod tts;
pub mod update;
pub mod weather;
