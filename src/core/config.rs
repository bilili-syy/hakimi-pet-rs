//! config.json（字段名、默认值和 Electron 版 ai.js DEFAULT_CONFIG 完全一致，两版互通）
//!
//! 载入规则照搬 ai.js loadConfig：
//! `{ ...DEFAULT_CONFIG, ...saved, reminders: {...}, tts: {...}, moyu: {...} }`——顶层浅合并，
//! 三个子对象再各自浅合并；未知字段（`_说明`、用户自己加的键）原样保留；载入后把完整模板写回文件。
//! 比原版多做一点防护：某个字段类型不对（比如 `"water": "45"`）只把这一个字段换回默认值，
//! 不会因为一个字段把整份设置清空。
#![allow(dead_code)]
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::sync::RwLock;
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Reminders {
    /// 喝水间隔分钟，0 = 关
    pub water: u32,
    pub stand_up: u32,
    /// "18:00"，空 = 关
    pub off_work: String,
    /// 未知字段原样保留
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
impl Default for Reminders {
    fn default() -> Self { Self { water: 45, stand_up: 60, off_work: "18:00".into(), extra: Map::new() } }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TtsConfig {
    pub enabled: bool,
    pub api_key: String,
    /// clone / preset / design / sovits（本机 GPT-SoVITS）
    pub mode: String,
    pub voice: String,
    pub style: String,
    /// all / chat / off
    pub speak: String,
    pub volume: f32,
    /// 语音回答：聊天回复等语音合成好了再和文字一起出来（关了 = 文字立刻出、聊天回复不念）
    pub chat_voice_reply: bool,
    /// 本机 GPT-SoVITS api_v2 地址（mode = sovits 时用）
    pub sovits_url: String,
    /// GPT-SoVITS 参考音频（服务那台机器上的路径）；空 = 用内置的哈基米录音（导出到数据目录）
    pub sovits_ref: String,
    /// 参考音频里说的话；空 = 无参考文本模式
    pub sovits_prompt: String,
    /// 未知字段原样保留
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
impl TtsConfig {
    /// 有没有办法在线合成：本机 GPT-SoVITS 不要 Key，MiMo 要
    pub fn can_synth(&self) -> bool { self.mode == "sovits" || !self.api_key.trim().is_empty() }
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
            chat_voice_reply: true,
            sovits_url: "http://127.0.0.1:9880".into(),
            sovits_ref: String::new(),
            sovits_prompt: String::new(),
            extra: Map::new(),
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
    /// 未知字段原样保留
    #[serde(flatten)]
    pub extra: Map<String, Value>,
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
            extra: Map::new(),
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
    /// 主人生日「月-日」（如 04-03），那天开机她会先祝你生日快乐；空 = 不知道
    pub birthday: String,
    pub reminders: Reminders,
    pub tts: TtsConfig,
    /// "" = 默认水手服 / hoodie / pajama / idol / swim / xmas / maid / jk（见 core::costume）
    pub costume: String,
    /// santa / glasses
    pub outfit: Vec<String>,
    pub moyu: MoyuConfig,
    /// 天气城市（wttr.in 认的名字，如「北京」「Shanghai」）；空 = 按出口 IP 自动定位
    pub weather_city: String,
    /// 哈基米大小倍数（乘在系统 DPI 缩放上），1.0 = 标准；载入时夹到 0.5~2.0
    pub scale: f32,
    /// 动画帧率上限：30 / 60 / 120（载入时就近取这三档）
    pub fps: u32,
    /// 全屏程序（游戏、全屏视频）在前台时自动藏起来，退出全屏再出来
    pub fullscreen_hide: bool,
    /// 每天最多吃多少「粮食」（云端聊天 token + MiMo 语音字数）；0 = 不限。到了先降级，不直接停
    pub daily_food_cap: u64,
    /// 看屏吐槽：Ctrl+Alt+K 截前台窗口给她看（默认关；截图只在内存里，确认后发给聊天模型）
    pub screen_roast: bool,
    /// 聊天「打开程序」安全白名单（键=口令/别名，值=本机 .exe 绝对路径；打开前仍会弹确认框，禁止命令行解释器）
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub allowed_apps: std::collections::BTreeMap<String, String>,
    /// 未知字段（_说明 之类）原样保留
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Electron 版 DEFAULT_CONFIG 里的说明字段（原样，用户第一次打开设置文件时能看到）
const DOC_FIELDS: [(&str, &str); 8] = [
    ("_说明", "填好 apiKey 后保存，右键哈基米「重新加载设置」即可生效。provider 可选 gemini 或 openai（任何 OpenAI 兼容接口）。"),
    ("_baseUrl说明", "gemini 留空即可；openai 兼容接口填到 /v1 为止，例如 https://api.openai.com/v1"),
    ("_proxy说明", "留空=跟随系统代理；也可填 http://127.0.0.1:7890 这样的地址"),
    ("_userName说明", "哈基米怎么称呼你，留空就叫「你」"),
    ("_reminders说明", "喝水/起身提醒间隔（分钟，0=关闭）；offWork 下班提醒时间，留空=关闭"),
    ("_outfit说明", "换装：costume 是整套衣服（空=默认奶白水手服，hoodie=原装卫衣，pajama=猫耳睡衣，idol=偶像演出服，swim=泳装，xmas=圣诞小红裙，maid=女仆装，jk=JK水手服）；outfit 是配件，可同时戴多个（santa=圣诞帽、glasses=圆眼镜）。右键「换装」里点就行"),
    ("_moyu说明", "摸鱼监督：前台窗口是视频/社交/购物/游戏（keywords 匹配窗口标题，processes 匹配进程名）连续 minutes 分钟就来催你；番茄钟期间 pomodoroMinutes 分钟就催。只在本机判断，不记录窗口标题。enabled=false 关闭"),
    ("_tts说明", "语音用小米 MiMo TTS。apiKey 填 MiMo 开放平台的 Key；mode: clone=固定哈基米声线（默认）/ preset=预置音色(voice 填 冰糖、茉莉)/ design=按 style 文字现设计；speak: all=所有气泡都念 / chat=只念聊天回复 / off；volume 0~1"),
];

impl Default for Config {
    fn default() -> Self {
        let mut extra = Map::new();
        for (k, v) in DOC_FIELDS { extra.insert(k.into(), Value::String(v.into())); }
        Self {
            provider: "gemini".into(),
            api_key: String::new(),
            model: "gemini-2.5-flash".into(),
            base_url: String::new(),
            proxy: String::new(),
            user_name: String::new(),
            birthday: String::new(),
            reminders: Reminders::default(),
            tts: TtsConfig::default(),
            costume: String::new(),
            outfit: Vec::new(),
            moyu: MoyuConfig::default(),
            weather_city: String::new(),
            scale: 1.0,
            fps: 60,
            fullscreen_hide: true,
            daily_food_cap: 0,
            screen_roast: false,
            allowed_apps: std::collections::BTreeMap::new(),
            extra,
        }
    }
}

/// 把一个 JSON 对象转成 T：整体能转就直接转；否则逐个字段试，类型不对的字段换回默认值。
pub(crate) fn lenient<T: DeserializeOwned + Serialize + Default>(obj: Map<String, Value>) -> T {
    if let Ok(t) = serde_json::from_value::<T>(Value::Object(obj.clone())) { return t; }
    let defaults = match serde_json::to_value(T::default()) { Ok(Value::Object(m)) => m, _ => Map::new() };
    let mut fixed = Map::new();
    for (k, v) in obj {
        let mut probe = Map::new();
        probe.insert(k.clone(), v.clone());
        if serde_json::from_value::<T>(Value::Object(probe)).is_ok() {
            fixed.insert(k, v);
        } else if let Some(d) = defaults.get(&k) {
            fixed.insert(k, d.clone());
        }
    }
    serde_json::from_value(Value::Object(fixed)).unwrap_or_default()
}

/// 子对象浅合并：`{ ...DEFAULT.x, ...(saved.x || {}) }`。saved.x 不是对象就当没有。
fn sub<T: DeserializeOwned + Serialize + Default>(saved: Option<&Value>) -> Value {
    let obj = match saved { Some(Value::Object(m)) => m.clone(), _ => Map::new() };
    // T 的 serde(default) 会把缺的字段补成默认值，相当于先铺默认再盖上 saved
    serde_json::to_value(lenient::<T>(obj)).unwrap_or(Value::Null)
}

/// 按 ai.js loadConfig 的规则把文件内容（已解析的 JSON）和默认值合并
pub fn merge(saved: &Value) -> Config {
    let empty = Map::new();
    let saved = saved.as_object().unwrap_or(&empty);
    let mut obj = match serde_json::to_value(Config::default()) { Ok(Value::Object(m)) => m, _ => Map::new() };
    for (k, v) in saved { obj.insert(k.clone(), v.clone()); }
    obj.insert("reminders".into(), sub::<Reminders>(saved.get("reminders")));
    obj.insert("tts".into(), sub::<TtsConfig>(saved.get("tts")));
    obj.insert("moyu".into(), sub::<MoyuConfig>(saved.get("moyu")));
    let mut c: Config = lenient(obj);
    c.scale = clamp_scale(c.scale);
    c.fps = clamp_fps(c.fps);
    c
}

/// 帧率就近取 30 / 60 / 120
pub fn clamp_fps(f: u32) -> u32 {
    match f { 0..=44 => 30, 45..=89 => 60, _ => 120 }
}

/// 当前帧率对应的帧间隔（ms）。渲染热路径每帧都要问，不能每次克隆整份配置，所以单独存一份。
static FRAME_MS: AtomicU32 = AtomicU32::new(16);

pub fn frame_ms() -> u32 { FRAME_MS.load(Ordering::Relaxed) }

fn publish(c: &Config) {
    let ms = match clamp_fps(c.fps) { 30 => 33, 60 => 16, _ => 8 };
    FRAME_MS.store(ms, Ordering::Relaxed);
}

/// 大小倍数夹到 0.5~2.0（NaN / 0 当 1.0）
pub fn clamp_scale(s: f32) -> f32 {
    if s.is_finite() && s > 0.0 { s.clamp(0.5, 2.0) } else { 1.0 }
}

/// 没填聊天 API Key（新手第一次打开聊天时提示去设置）。本机 / 局域网的 OpenAI 兼容接口（Ollama、LM Studio）不需要 Key
pub fn ai_key_missing(c: &Config) -> bool { c.api_key.trim().is_empty() && !ai_is_local(c) }

/// 聊天模型跑在本机 / 局域网（Ollama 等）：不要 Key、不走代理、超时放宽
pub fn ai_is_local(c: &Config) -> bool { c.provider == "openai" && is_local_url(&c.base_url) }

/// URL 指向本机或局域网：localhost / 127.x / ::1 / 10.x / 172.16-31.x / 192.168.x / 169.254.x / fc00::/7 / fe80:: / *.local
pub fn is_local_url(url: &str) -> bool {
    let rest = url.trim().split_once("://").map(|(_, r)| r).unwrap_or(url.trim());
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
    let host = if let Some(h) = host_port.strip_prefix('[') {
        h.split(']').next().unwrap_or("")
    } else {
        host_port.split(':').next().unwrap_or("")
    };
    if host.is_empty() { return false; }
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") || lower.ends_with(".local") { return true; }
    let ip_str = host.split('%').next().unwrap_or(host);
    let check_v4 = |v4: std::net::Ipv4Addr| v4.is_loopback() || v4.is_private() || v4.is_link_local();
    match ip_str.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => check_v4(v4),
        Ok(std::net::IpAddr::V6(v6)) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return check_v4(v4);
            }
            let seg = v6.segments()[0];
            v6.is_loopback() || (seg & 0xfe00) == 0xfc00 || (seg & 0xffc0) == 0xfe80
        }
        Err(_) => false,
    }
}

/// 把文件文本解析成配置（坏文件 = 全默认，和原版一致）
pub fn parse(text: &str) -> Config {
    let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap_or(Value::Null);
    merge(&v)
}

static CONFIG: RwLock<Option<Config>> = RwLock::new(None);

/// 当前配置（克隆一份）
pub fn get() -> Config {
    if let Some(c) = CONFIG.read().unwrap_or_else(|e| e.into_inner()).as_ref() { return c.clone(); }
    reload()
}

/// 从文件读取（不加锁，供 reload/update 持有写锁时复用）
fn load_from_disk() -> Config {
    let path = super::paths::config_file();
    let c = match std::fs::read_to_string(&path) {
        Ok(s) => {
            if !s.trim().is_empty() && serde_json::from_str::<Value>(s.trim_start_matches('\u{feff}')).is_err() {
                // 原版这里会直接用默认模板覆盖；多留一份备份，免得手滑写坏 JSON 丢掉 Key
                let _ = std::fs::write(path.with_extension("json.bak"), &s);
                super::log::log("config.json 不是合法 JSON，已备份为 config.json.bak 并恢复默认");
            }
            parse(&s)
        }
        Err(_) => Config::default(),
    };
    write(&c);
    publish(&c);
    c
}

/// 从文件重新读取（缺字段用默认值），写回完整模板，返回新配置
pub fn reload() -> Config {
    let mut guard = CONFIG.write().unwrap_or_else(|e| e.into_inner());
    let c = load_from_disk();
    *guard = Some(c.clone());
    c
}

/// 改一个设置并写回文件（菜单开关用）
pub fn update(f: impl FnOnce(&mut Config)) -> Config {
    let mut guard = CONFIG.write().unwrap_or_else(|e| e.into_inner());
    let mut c = match guard.as_ref() {
        Some(c) => c.clone(),
        None => load_from_disk(),
    };
    f(&mut c);
    c.scale = clamp_scale(c.scale);
    c.fps = clamp_fps(c.fps);
    write(&c);
    publish(&c);
    *guard = Some(c.clone());
    c
}

/// 设置窗口「保存」：整份替换（c 是从 get() 拿的副本改出来的，未知字段都还在），原子写回文件。
/// 返回写盘是否成功。
pub fn replace(mut c: Config) -> bool {
    c.scale = clamp_scale(c.scale);
    c.fps = clamp_fps(c.fps);
    let mut guard = CONFIG.write().unwrap_or_else(|e| e.into_inner());
    let ok = write(&c);
    publish(&c);
    *guard = Some(c);
    ok
}

/// 序列化成和原版一样的 2 空格缩进 JSON
pub fn to_json(c: &Config) -> String {
    serde_json::to_string_pretty(c).unwrap_or_default()
}

fn write(c: &Config) -> bool {
    let s = to_json(c);
    if s.is_empty() { return false; }
    if let Err(e) = super::save::write_atomic(&super::paths::config_file(), s.as_bytes()) {
        super::log::log(&format!("config write failed {e}"));
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_original() {
        let c = Config::default();
        assert_eq!(c.provider, "gemini");
        assert_eq!(c.model, "gemini-2.5-flash");
        assert_eq!((c.reminders.water, c.reminders.stand_up, c.reminders.off_work.as_str()), (45, 60, "18:00"));
        assert!(c.tts.enabled);
        assert_eq!(c.tts.mode, "clone");
        assert_eq!(c.tts.voice, "冰糖");
        assert_eq!(c.tts.speak, "all");
        assert!((c.tts.volume - 0.9).abs() < 1e-6);
        assert!(c.moyu.enabled);
        assert_eq!((c.moyu.minutes, c.moyu.pomodoro_minutes), (20, 2));
        assert_eq!(c.moyu.keywords.len(), 29);
        assert_eq!(c.moyu.processes.len(), 10);
        assert_eq!(c.costume, "");
        assert!(c.outfit.is_empty());
        assert!(c.extra.contains_key("_说明"));
    }

    #[test]
    fn camel_case_field_names() {
        let v = serde_json::to_value(Config::default()).unwrap();
        for k in ["provider", "apiKey", "model", "baseUrl", "proxy", "userName", "reminders", "tts", "costume", "outfit", "moyu", "_说明", "_tts说明"] {
            assert!(v.get(k).is_some(), "missing {k}");
        }
        assert!(v["reminders"].get("standUp").is_some());
        assert!(v["reminders"].get("offWork").is_some());
        assert!(v["tts"].get("apiKey").is_some());
        assert!(v["moyu"].get("pomodoroMinutes").is_some());
    }

    #[test]
    fn merge_keeps_user_values_and_unknown_fields() {
        let c = parse(r#"{"apiKey":"k1","provider":"openai","myField":1,"_说明":"自定义",
            "reminders":{"water":0},"tts":{"apiKey":"t1","volume":1,"custom":true},"moyu":{"enabled":false},"outfit":["santa"]}"#);
        assert_eq!(c.api_key, "k1");
        assert_eq!(c.provider, "openai");
        assert_eq!(c.model, "gemini-2.5-flash");
        assert_eq!(c.reminders.water, 0);
        assert_eq!(c.reminders.stand_up, 60);
        assert_eq!(c.tts.api_key, "t1");
        assert!((c.tts.volume - 1.0).abs() < 1e-6);
        assert_eq!(c.tts.mode, "clone");
        assert!(!c.moyu.enabled);
        assert_eq!(c.moyu.minutes, 20);
        assert_eq!(c.outfit, vec!["santa".to_string()]);
        assert_eq!(c.extra.get("myField"), Some(&Value::from(1)));
        assert_eq!(c.extra.get("_说明"), Some(&Value::from("自定义")));
        assert_eq!(c.tts.extra.get("custom"), Some(&Value::Bool(true)));
        assert!(c.extra.contains_key("_moyu说明"));
        // 写回再读，值不变
        let again = parse(&to_json(&c));
        assert_eq!(to_json(&again), to_json(&c));
    }

    #[test]
    fn round_trip_keeps_unknown_fields_everywhere() {
        // 用户手写的未知字段（顶层和三个子对象里）+ 新字段，经过「读 → 设置窗口改几项 → 写」都要原样保留
        let text = r#"{"apiKey":"k","futureFlag":{"a":[1,2]},"_说明":"自定义说明",
            "reminders":{"water":30,"snooze":5},"tts":{"apiKey":"t","chatVoiceReply":false,"pitch":1.5},
            "moyu":{"minutes":15,"whitelist":["code"]},"weatherCity":"杭州","scale":1.2}"#;
        let mut c = parse(text);
        assert_eq!(c.weather_city, "杭州");
        assert!((c.scale - 1.2).abs() < 1e-6);
        assert!(!c.tts.chat_voice_reply);
        // 设置窗口改了几个已知字段
        c.model = "deepseek-flash".into();
        c.tts.volume = 0.5;
        c.reminders.water = 0;
        let out: Value = serde_json::from_str(&to_json(&c)).unwrap();
        assert_eq!(out["futureFlag"], serde_json::json!({"a":[1,2]}));
        assert_eq!(out["_说明"], "自定义说明");
        assert_eq!(out["reminders"]["snooze"], 5);
        assert_eq!(out["reminders"]["water"], 0);
        assert_eq!(out["tts"]["pitch"], 1.5);
        assert_eq!(out["tts"]["chatVoiceReply"], false);
        assert_eq!(out["moyu"]["whitelist"], serde_json::json!(["code"]));
        assert_eq!(out["weatherCity"], "杭州");
        assert_eq!(out["model"], "deepseek-flash");
        assert_eq!(out["apiKey"], "k");
        // 再读一遍完全一样
        assert_eq!(to_json(&parse(&to_json(&c))), to_json(&c));
    }

    #[test]
    fn new_fields_default_and_clamp() {
        let c = parse("{}");
        assert!(c.tts.chat_voice_reply);
        assert_eq!(c.weather_city, "");
        assert!((c.scale - 1.0).abs() < 1e-6);
        assert!((parse(r#"{"scale":9}"#).scale - 2.0).abs() < 1e-6);
        assert!((parse(r#"{"scale":0}"#).scale - 1.0).abs() < 1e-6);
        assert!((parse(r#"{"scale":"big"}"#).scale - 1.0).abs() < 1e-6);
        assert!(ai_key_missing(&c));
        assert!(!ai_key_missing(&parse(r#"{"apiKey":"x"}"#)));
        // 本机 Ollama 不要 Key；gemini 填本机地址也还是要 Key
        assert!(!ai_key_missing(&parse(r#"{"provider":"openai","baseUrl":"http://127.0.0.1:11434/v1"}"#)));
        assert!(ai_key_missing(&parse(r#"{"provider":"gemini","baseUrl":"http://127.0.0.1:11434/v1"}"#)));
        assert!(ai_key_missing(&parse(r#"{"provider":"openai","baseUrl":"https://api.deepseek.com/v1"}"#)));
    }

    #[test]
    fn bad_field_only_resets_that_field() {
        let c = parse(r#"{"apiKey":"keep","model":123,"reminders":{"water":"x","standUp":30},"tts":null,"outfit":"santa"}"#);
        assert_eq!(c.api_key, "keep");
        assert_eq!(c.model, "gemini-2.5-flash");
        assert_eq!(c.reminders.water, 45);
        assert_eq!(c.reminders.stand_up, 30);
        assert_eq!(c.tts.mode, "clone");
        assert!(c.outfit.is_empty());
    }

    #[test]
    fn garbage_gives_defaults() {
        let c = parse("{not json");
        assert_eq!(c.provider, "gemini");
        let c = parse("\u{feff}{\"apiKey\":\"bom\"}");
        assert_eq!(c.api_key, "bom");
    }

    #[test]
    fn local_urls() {
        for u in ["http://127.0.0.1:11434/v1", "http://localhost:1234/v1", "http://[::1]:11434", "http://192.168.1.20:11434/v1",
            "http://10.0.0.5/v1", "http://172.20.1.1:8080", "http://my-pc.local:11434", "http://user:pw@127.0.0.1:1/v1"] {
            assert!(is_local_url(u), "{u}");
        }
        for u in ["https://api.openai.com/v1", "https://localhost.example.com/v1", "http://172.32.0.1", "http://8.8.8.8", "", "https://192.168.evil.com"] {
            assert!(!is_local_url(u), "{u}");
        }
    }
}
