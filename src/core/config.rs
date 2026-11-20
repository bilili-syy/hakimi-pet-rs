//! config.json（字段名、默认值和 Electron 版 ai.js DEFAULT_CONFIG 完全一致，两版互通）
//! TODO(B)：load 时把缺的字段补齐写回文件（Electron 版行为）；保留 _说明 等未知字段（已用 extra 兜住）
#![allow(dead_code)]
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::sync::RwLock;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Reminders {
    /// 喝水间隔分钟，0 = 关
    pub water: u32,
    pub stand_up: u32,
    /// "18:00"，空 = 关
    pub off_work: String,
}
impl Default for Reminders {
    fn default() -> Self { Self { water: 45, stand_up: 60, off_work: "18:00".into() } }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TtsConfig {
    pub enabled: bool,
    pub api_key: String,
    /// clone / preset / design
    pub mode: String,
    pub voice: String,
    pub style: String,
    /// all / chat / off
    pub speak: String,
    pub volume: f32,
}
impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            api_key: String::new(),
            mode: "clone".into(),
            voice: "冰糖".into(),
            style: "十八岁左右的元气少女猫娘，声音清亮甜美、略带奶音，说话轻快俏皮，句尾喜欢拖长撒娇。".into(),
            speak: "all".into(),
            volume: 0.9,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MoyuConfig {
    pub enabled: bool,
    pub minutes: u32,
    pub pomodoro_minutes: u32,
    pub keywords: Vec<String>,
    pub processes: Vec<String>,
}
impl Default for MoyuConfig {
    fn default() -> Self {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect();
        Self {
            enabled: true,
            minutes: 20,
            pomodoro_minutes: 2,
            keywords: s(&[
                "bilibili", "哔哩哔哩", "youtube", "抖音", "douyin", "tiktok", "快手", "小红书", "微博", "weibo",
                "知乎", "虎扑", "斗鱼", "虎牙", "twitch", "netflix", "爱奇艺", "优酷", "腾讯视频", "芒果tv",
                "淘宝", "京东", "拼多多", "reddit", "twitter", "instagram", "贴吧", "原神", "王者荣耀",
            ]),
            processes: s(&[
                "steam", "steamwebhelper", "genshinimpact", "yuanshen", "leagueclient", "league of legends",
                "douyin", "bilibili", "qqmusic", "cloudmusic",
            ]),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// gemini / openai
    pub provider: String,
    pub api_key: String,
    pub model: String,
    pub base_url: String,
    /// 空 = 跟随系统代理
    pub proxy: String,
    pub user_name: String,
    pub reminders: Reminders,
    pub tts: TtsConfig,
    /// "" / xmas / maid / jk
    pub costume: String,
    /// santa / glasses / scarf
    pub outfit: Vec<String>,
    pub moyu: MoyuConfig,
    /// 未知字段（_说明 之类）原样保留
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            provider: "gemini".into(),
            api_key: String::new(),
            model: "gemini-2.5-flash".into(),
            base_url: String::new(),
            proxy: String::new(),
            user_name: String::new(),
            reminders: Reminders::default(),
            tts: TtsConfig::default(),
            costume: String::new(),
            outfit: Vec::new(),
            moyu: MoyuConfig::default(),
            extra: Map::new(),
        }
    }
}

static CONFIG: RwLock<Option<Config>> = RwLock::new(None);

/// 当前配置（克隆一份）
pub fn get() -> Config {
    if let Some(c) = CONFIG.read().unwrap().as_ref() { return c.clone(); }
    reload()
}

/// 从文件重新读取（缺字段用默认值），写回完整模板，返回新配置
pub fn reload() -> Config {
    let path = super::paths::config_file();
    let c: Config = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    write(&c);
    *CONFIG.write().unwrap() = Some(c.clone());
    c
}

/// 改一个设置并写回文件（菜单开关用）
pub fn update(f: impl FnOnce(&mut Config)) -> Config {
    let mut c = get();
    f(&mut c);
    write(&c);
    *CONFIG.write().unwrap() = Some(c.clone());
    c
}

fn write(c: &Config) {
    let _ = std::fs::create_dir_all(super::paths::data_dir());
    if let Ok(s) = serde_json::to_string_pretty(c) { let _ = std::fs::write(super::paths::config_file(), s); }
}
