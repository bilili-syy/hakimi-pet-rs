//! 皮肤 / 角色包：把一个文件夹丢进 `%APPDATA%\hakimi-pet\skins\<id>\`，桌宠就变成你的角色。
//!
//! 每个角色包一个 `skin.json`（字段名是和其它线约定好的，别改名）：
//! ```json
//! {
//!   "name": "小薄荷", "author": "你", "version": "1.0",
//!   "persona": "追加到 AI 聊天 system prompt 末尾的人设",
//!   "scale": 0.9,
//!   "sprites":  { "idle": "idle.png", "happy": "happy.png" },
//!   "overlays": { "blink": "blink.png", "mouthOpen": "mouth.png" },
//!   "voiceDir": "voice",
//!   "lines":    { "poke": ["别戳啦～"], "哈基米上线啦喵～": "小薄荷上线！" }
//! }
//! ```
//! 缺哪个状态 / 哪个文件，就回退到内置哈基米（内置皮肤 id = `hakimi`）。
//!
//! 这里只放「数据」：解析、扫描、当前是哪个皮肤（全局，任何线程可读）。
//! 图片的解码/缩放在 render/skin_loader.rs，菜单在 platform/skin_menu.rs，行为钩子在 behavior/skin.rs。
//! 这里不解码任何图片：启动时只读每个包的 skin.json（几 KB），图片等渲染用到时才按需解码。
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// 内置哈基米的皮肤 id（save.json 里没写 / 写的是它 = 不用角色包）
pub const BUILTIN_ID: &str = "hakimi";
pub const BUILTIN_NAME: &str = "🐱 哈基米（内置）";
/// 角色包清单文件名
pub const MANIFEST: &str = "skin.json";
/// save.json 里记当前皮肤的字段
pub const SAVE_KEY: &str = "skin";
/// 菜单 id 前缀；经 `MenuAction::Costume("skin:…")` 交给行为层（types.rs 冻结，不加新变体）
pub const MENU_PREFIX: &str = "skin:";
/// 「打开皮肤文件夹」（平台层负责打开）
pub const MENU_OPEN: &str = "skin:@open";
/// 「刷新皮肤列表」
pub const MENU_RELOAD: &str = "skin:@reload";
/// 「导入角色包 (.zip)…」
pub const MENU_IMPORT: &str = "skin:@import";
/// 当前支持的 skin.json 清单格式版本
pub const CURRENT_FORMAT_VERSION: u32 = 1;

/// 状态名 = assets/sprites 的文件名（不含 .png，和 types::Face::ALL 一一对应，共 32 个）
pub const STATES: [&str; 32] = [
    "idle", "blink", "happy", "drag", "sit", "sleep", "eat", "hungry", "think", "walk1", "walk2", "shy", "dizzy",
    "surprised", "cry", "yawn", "love", "hiss", "stare", "dance", "honey", "pout", "smug", "confused", "scared",
    "starry", "puff", "wink", "drool", "sweat", "cheer", "sleepy",
];
/// 支持的叠加层
pub const OVERLAYS: [&str; 2] = ["blink", "mouthOpen"];
/// 语音文件可以是这些格式（rodio 解不了 Ogg Opus，角色包请用 Vorbis ogg / wav / mp3 / flac）
pub const VOICE_EXTS: [&str; 4] = ["ogg", "wav", "mp3", "flac"];

/// skin.json 最大多大（再大肯定不是正常的清单）
pub const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
/// 单个语音文件上限
pub const MAX_VOICE_BYTES: u64 = 8 * 1024 * 1024;
/// persona 最多多少字（太长会挤掉聊天上下文）
pub const MAX_PERSONA_CHARS: usize = 2000;
pub const SCALE_MIN: f32 = 0.2;
pub const SCALE_MAX: f32 = 2.0;

// ------------------------------------------------------------------ 清单

/// 角色包专属声线配置（`skin.json` 的 `"tts"` 对象，全部字段可选；只允许引用包内相对路径参考音频，严禁改写外部 URL）
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkinTts {
    /// "clone" | "preset" | "design" | "sovits"
    pub mode: Option<String>,
    /// 预置音色名（如 "蜜桃"、"冰糖"）
    pub voice: Option<String>,
    /// 音色描述 / 情绪风格提示词
    pub style: Option<String>,
    /// 包内参考音频相对路径（用于 clone 或 sovits 模式，经 safe_join 校验）
    pub ref_audio: Option<String>,
    /// GPT-SoVITS 参考音频对应的台词文本
    pub sovits_prompt: Option<String>,
}

impl SkinTts {
    pub fn is_empty(&self) -> bool {
        self.mode.is_none()
            && self.voice.is_none()
            && self.style.is_none()
            && self.ref_audio.is_none()
            && self.sovits_prompt.is_none()
    }
}

/// 解析后的 skin.json。解析是宽松的：缺字段 = 默认值；某个字段类型不对只丢这一个字段并记一条 warning。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkinSpec {
    /// 清单规范版本（未写时按 1 处理）
    pub format_version: Option<u32>,
    /// 清单中声明的唯一角色 ID（供平铺 zip 导入命名，可选）
    pub manifest_id: String,
    pub name: String,
    pub author: String,
    pub version: String,
    pub persona: String,
    /// None = 自动缩放（按比例塞进内置 200×293 的框里）
    pub scale: Option<f32>,
    /// 缺失表情是否回退到内置哈基米（默认 false = 只要包里有 idle 就回退到角色自己的 idle，避免闪回哈基米）
    pub fallback_builtin: bool,
    /// 状态名（小写）→ 相对包目录的图片路径
    pub sprites: BTreeMap<String, String>,
    /// "blink" / "mouthOpen" → 图片路径
    pub overlays: BTreeMap<String, String>,
    pub voice_dir: Option<String>,
    /// 角色专属 TTS 声线覆盖
    pub tts: Option<SkinTts>,
    /// 原台词全文 或 台词分类名（小写）→ 可选的替换台词（随机挑一句）
    pub lines: BTreeMap<String, Vec<String>>,
    /// 解析时忽略掉的东西（给日志 / 菜单提示用）
    pub warnings: Vec<String>,
}

impl SkinSpec {
    pub fn format_version_or_default(&self) -> u32 {
        self.format_version.unwrap_or(CURRENT_FORMAT_VERSION)
    }
}

fn str_field(m: &Map<String, Value>, key: &str, warn: &mut Vec<String>) -> String {
    match m.get(key) {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        Some(_) => {
            warn.push(format!("{key} 不是字符串，忽略"));
            String::new()
        }
    }
}

/// 字符串表：值必须是非空字符串，别的跳过
fn path_map(m: &Map<String, Value>, key: &str, lower_keys: bool, warn: &mut Vec<String>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    match m.get(key) {
        None | Some(Value::Null) => {}
        Some(Value::Object(o)) => {
            for (k, v) in o {
                let k = if lower_keys { k.trim().to_ascii_lowercase() } else { k.trim().to_string() };
                match v {
                    Value::String(s) if !s.trim().is_empty() && !k.is_empty() => {
                        out.insert(k, s.trim().to_string());
                    }
                    _ => warn.push(format!("{key}.{k} 不是文件名，忽略")),
                }
            }
        }
        Some(_) => warn.push(format!("{key} 不是对象，忽略")),
    }
    out
}

fn parse_tts(m: &Map<String, Value>, warn: &mut Vec<String>) -> Option<SkinTts> {
    let o = match m.get("tts") {
        None | Some(Value::Null) => return None,
        Some(Value::Object(o)) => o,
        Some(_) => {
            warn.push("tts 不是对象，忽略".into());
            return None;
        }
    };
    let opt = |k: &str, w: &mut Vec<String>| Some(str_field(o, k, w)).filter(|s| !s.is_empty());
    let mode = opt("mode", warn).and_then(|s| {
        let lower = s.to_ascii_lowercase();
        if ["clone", "preset", "design", "sovits"].contains(&lower.as_str()) {
            Some(lower)
        } else {
            warn.push(format!("tts.mode \"{s}\" 不支持（仅限 clone / preset / design / sovits），忽略"));
            None
        }
    });
    let voice = opt("voice", warn);
    let mut style = opt("style", warn);
    if let Some(st) = &mut style
        && st.chars().count() > 300
    {
        *st = st.chars().take(300).collect();
        warn.push("tts.style 超过 300 字，已截断".into());
    }
    let ref_audio = opt("refAudio", warn).or_else(|| opt("sovitsRef", warn));
    let mut sovits_prompt = opt("sovitsPrompt", warn);
    if let Some(sp) = &mut sovits_prompt
        && sp.chars().count() > 200
    {
        *sp = sp.chars().take(200).collect();
    }
    if o.contains_key("sovitsUrl") || o.contains_key("apiUrl") {
        warn.push("tts 不允许由角色包指定外部服务器地址，已忽略".into());
    }
    let out = SkinTts { mode, voice, style, ref_audio, sovits_prompt };
    (!out.is_empty()).then_some(out)
}

/// 解析 skin.json 文本。不是 JSON 对象 → Err（这个包整个不认）
pub fn parse(text: &str) -> Result<SkinSpec, String> {
    let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("skin.json 不是合法 JSON：{e}"))?;
    let Value::Object(m) = v else { return Err("skin.json 顶层必须是 { … } 对象".into()) };
    let mut w = Vec::new();
    let format_version = match m.get("formatVersion") {
        None | Some(Value::Null) => None,
        Some(Value::Number(n)) => match n.as_u64() {
            Some(fv) if (1..=999).contains(&fv) => {
                let fv = fv as u32;
                if fv > CURRENT_FORMAT_VERSION {
                    w.push(format!("formatVersion {fv} 高于当前支持的 {CURRENT_FORMAT_VERSION}，部分新字段可能被忽略"));
                }
                Some(fv)
            }
            _ => {
                w.push("formatVersion 必须是正整数，按 1 处理".into());
                None
            }
        },
        Some(_) => {
            w.push("formatVersion 不是整数，按 1 处理".into());
            None
        }
    };
    let manifest_id = str_field(&m, "id", &mut w);
    let name = str_field(&m, "name", &mut w);
    let author = str_field(&m, "author", &mut w);
    let version = str_field(&m, "version", &mut w);
    let mut persona = str_field(&m, "persona", &mut w);
    if persona.chars().count() > MAX_PERSONA_CHARS {
        persona = persona.chars().take(MAX_PERSONA_CHARS).collect();
        w.push(format!("persona 超过 {MAX_PERSONA_CHARS} 字，已截断"));
    }
    let scale = match m.get("scale") {
        None | Some(Value::Null) => None,
        Some(Value::Number(n)) => match n.as_f64() {
            Some(s) if s.is_finite() && s > 0.0 => {
                let c = (s as f32).clamp(SCALE_MIN, SCALE_MAX);
                if (c - s as f32).abs() > f32::EPSILON { w.push(format!("scale {s} 超出 {SCALE_MIN}–{SCALE_MAX}，按 {c}")); }
                Some(c)
            }
            _ => {
                w.push("scale 必须是正数，按自动缩放".into());
                None
            }
        },
        Some(_) => {
            w.push("scale 不是数字，按自动缩放".into());
            None
        }
    };
    let fallback_builtin = match m.get("fallback") {
        None | Some(Value::Null) => false,
        Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "idle" | "" => false,
            "builtin" | "hakimi" => true,
            other => {
                w.push(format!("fallback \"{other}\" 不认识（仅支持 idle / builtin），按 idle 处理"));
                false
            }
        },
        Some(_) => {
            w.push("fallback 不是字符串，按 idle 处理".into());
            false
        }
    };
    let sprites = path_map(&m, "sprites", true, &mut w);
    for k in sprites.keys() {
        if !STATES.contains(&k.as_str()) { w.push(format!("sprites.{k} 不是已知状态（会被忽略）")); }
    }
    let overlays = path_map(&m, "overlays", false, &mut w);
    for k in overlays.keys() {
        if !OVERLAYS.contains(&k.as_str()) { w.push(format!("overlays.{k} 不认识（只支持 blink / mouthOpen）")); }
    }
    let voice_dir = Some(str_field(&m, "voiceDir", &mut w)).filter(|s| !s.is_empty());
    let tts = parse_tts(&m, &mut w);
    let mut lines = BTreeMap::new();
    match m.get("lines") {
        None | Some(Value::Null) => {}
        Some(Value::Object(o)) => {
            for (k, v) in o {
                let alts: Vec<String> = match v {
                    Value::String(s) => vec![s.clone()],
                    Value::Array(a) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
                    _ => Vec::new(),
                };
                let alts: Vec<String> = alts.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                if alts.is_empty() || k.trim().is_empty() {
                    w.push(format!("lines.{k} 没有可用的台词，忽略"));
                } else {
                    lines.insert(line_key(k), alts);
                }
            }
        }
        Some(_) => w.push("lines 不是对象，忽略".into()),
    }
    Ok(SkinSpec {
        format_version,
        manifest_id,
        name,
        author,
        version,
        persona,
        scale,
        fallback_builtin,
        sprites,
        overlays,
        voice_dir,
        tts,
        lines,
        warnings: w,
    })
}

/// 台词表的键：去首尾空白；纯 ASCII 的（分类名）不分大小写
fn line_key(k: &str) -> String {
    let k = k.trim();
    if k.is_ascii() { k.to_ascii_lowercase() } else { k.to_string() }
}

/// 把包里写的相对路径拼到包目录下。只接受「往下走」的相对路径：
/// 绝对路径、盘符、`..` 一律拒绝，别人分享的包没法读到包外的文件。
pub fn safe_join(dir: &Path, rel: &str) -> Option<PathBuf> {
    let rel = rel.trim().replace('\\', "/");
    if rel.is_empty() || rel.starts_with('/') || rel.contains(':') || rel.chars().any(char::is_control) {
        return None;
    }
    let mut out = dir.to_path_buf();
    let mut any = false;
    for c in Path::new(&rel).components() {
        match c {
            Component::Normal(p) => {
                let s = p.to_str()?;
                let stem = s.split('.').next().unwrap_or(s);
                if is_win_reserved_stem(stem) {
                    return None;
                }
                out.push(p);
                any = true;
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    any.then_some(out)
}

/// 校验单个合法的角色 ID（文件夹名）：不允许路径分隔符、`..`、控制字符、Windows 保留字、`@`/`.` 开头或 `hakimi`
pub fn safe_single_id(raw: &str) -> Option<String> {
    let s = raw.trim().trim_end_matches(['.', ' ']);
    if s.is_empty()
        || s.chars().count() > 64
        || s.starts_with('.')
        || s.starts_with('@')
        || s.eq_ignore_ascii_case(BUILTIN_ID)
        || s.chars().any(|c| "\\/:*?\"<>|".contains(c) || c.is_control())
    {
        return None;
    }
    let stem = s.split('.').next().unwrap_or(s);
    if is_win_reserved_stem(stem) {
        return None;
    }
    let mut comps = Path::new(s).components();
    match (comps.next(), comps.next()) {
        (Some(Component::Normal(_)), None) => Some(s.to_string()),
        _ => None,
    }
}

// ------------------------------------------------------------------ 一个角色包

#[derive(Clone, Debug, PartialEq)]
pub struct SkinInfo {
    /// 文件夹名
    pub id: String,
    pub dir: PathBuf,
    pub spec: SkinSpec,
}

impl SkinInfo {
    pub fn display_name(&self) -> &str { if self.spec.name.is_empty() { &self.id } else { &self.spec.name } }

    /// 0.2–2.0，None = 1
    pub fn scale(&self) -> f32 { self.spec.scale.unwrap_or(1.0) }

    /// 某个状态的图片文件（没写 / 文件不在 → None，调用方回退 idle 或内置）
    pub fn sprite_file(&self, state: &str) -> Option<PathBuf> {
        let rel = self.spec.sprites.get(state)?;
        safe_join(&self.dir, rel).filter(|p| p.is_file())
    }

    pub fn overlay_file(&self, name: &str) -> Option<PathBuf> {
        let rel = self.spec.overlays.get(name)?;
        safe_join(&self.dir, rel).filter(|p| p.is_file())
    }

    pub fn voice_dir(&self) -> Option<PathBuf> { safe_join(&self.dir, self.spec.voice_dir.as_deref()?).filter(|p| p.is_dir()) }

    /// 角色包内的参考音频（仅允许包内相对路径 + 合法音频后缀 + 大小上限）
    pub fn ref_audio_file(&self) -> Option<PathBuf> {
        let rel = self.spec.tts.as_ref()?.ref_audio.as_deref()?;
        let p = safe_join(&self.dir, rel)?;
        let ext = p.extension().and_then(|e| e.to_str())?.to_ascii_lowercase();
        if !VOICE_EXTS.contains(&ext.as_str()) {
            return None;
        }
        let meta = std::fs::metadata(&p).ok()?;
        (meta.is_file() && meta.len() > 0 && meta.len() <= MAX_VOICE_BYTES).then_some(p)
    }

    /// 把角色包的声线配置叠加到用户当前的 TtsConfig 上（保留用户的开关、音量、API Key 与本机服务地址）
    pub fn apply_tts(&self, mut base: crate::core::config::TtsConfig) -> crate::core::config::TtsConfig {
        let Some(st) = &self.spec.tts else { return base };
        if let Some(m) = &st.mode {
            base.mode = m.clone();
        }
        if let Some(v) = &st.voice {
            base.voice = v.clone();
        }
        if let Some(s) = &st.style {
            base.style = s.clone();
        }
        if let Some(p) = &st.sovits_prompt {
            base.sovits_prompt = p.clone();
        }
        if let Some(ref_file) = self.ref_audio_file() {
            let full = ref_file.to_string_lossy().into_owned();
            if base.mode == "sovits" {
                base.sovits_ref = full;
            } else {
                base.clone_ref = full;
            }
        }
        base
    }

    /// 台词语音文件。文件名规则和 assets/voice/lines 一样：`sha1(台词) 前 16 位.ogg`；
    /// 另外也认直接用台词当文件名（`喵？找哈基米有事吗～.wav`，Windows 不能用的 \/:*?"<>| 换成 _）。
    /// `text` 是 tts::clean 之后的台词。
    pub fn voice_file(&self, text: &str) -> Option<PathBuf> {
        let dir = self.voice_dir()?;
        for stem in voice_stems(text) {
            for ext in VOICE_EXTS {
                let p = dir.join(format!("{stem}.{ext}"));
                if p.is_file() { return Some(p); }
            }
        }
        None
    }

    /// 追加到 system prompt 末尾的人设（没写 persona 返回 None）
    pub fn persona_suffix(&self) -> Option<String> {
        if self.spec.persona.is_empty() { return None; }
        Some(format!(
            "【角色包设定，优先级高于上面的外形、名字和口癖】你现在扮演「{}」。{}",
            self.display_name(),
            self.spec.persona
        ))
    }

    /// 台词替换：先按原台词全文找，再按分类名找（分类名由行为层给，比如 "poke"）
    pub fn line_for(&self, text: &str, category: Option<&str>) -> Option<String> {
        let alts = self.spec.lines.get(&line_key(text)).or_else(|| self.spec.lines.get(&line_key(category?)))?;
        Some(alts[fastrand::usize(..alts.len())].clone())
    }
}

pub(crate) fn is_win_reserved_stem(s: &str) -> bool {
    let u = s.to_ascii_uppercase();
    matches!(u.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((u.starts_with("COM") || u.starts_with("LPT")) && u.len() == 4 && matches!(u.as_bytes()[3], b'1'..=b'9'))
}

/// 语音文件名候选：hash 规则、台词原文
pub fn voice_stems(text: &str) -> Vec<String> {
    let hash = sha1_smol::Sha1::from(text.as_bytes()).digest().to_string();
    let mut v = vec![hash[..16].to_string()];
    let plain: String = text.chars().map(|c| if "\\/:*?\"<>|".contains(c) || c.is_control() { '_' } else { c }).collect();
    let plain = plain.trim().trim_end_matches('.').to_string();
    if !plain.is_empty() && plain.chars().count() <= 120 && !is_win_reserved_stem(&plain) { v.push(plain); }
    v
}

/// 读一个角色包目录。没有 skin.json / 解析失败 → Err（包被跳过，原因写日志）
pub fn load_dir(dir: &Path) -> Result<SkinInfo, String> {
    let id = dir.file_name().and_then(|n| n.to_str()).ok_or("文件夹名不是合法的 Unicode")?.to_string();
    let f = dir.join(MANIFEST);
    let meta = std::fs::metadata(&f).map_err(|_| format!("「{id}」里没有 {MANIFEST}"))?;
    if meta.len() > MAX_MANIFEST_BYTES { return Err(format!("「{id}」的 {MANIFEST} 太大了")); }
    let text = std::fs::read_to_string(&f).map_err(|e| format!("「{id}」的 {MANIFEST} 读不了：{e}"))?;
    let spec = parse(&text).map_err(|e| format!("「{id}」：{e}"))?;
    Ok(SkinInfo { id, dir: dir.to_path_buf(), spec })
}

/// 扫描 skins 目录：返回 (能用的包按 id 排序, 跳过的原因)
pub fn scan(root: &Path) -> (Vec<SkinInfo>, Vec<String>) {
    let mut ok = Vec::new();
    let mut bad = Vec::new();
    let Ok(rd) = std::fs::read_dir(root) else { return (ok, bad) };
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_dir() { continue; }
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') { continue; }
        if name.eq_ignore_ascii_case(BUILTIN_ID) || name.starts_with('@') {
            bad.push(format!("「{name}」是保留名字，换个文件夹名吧"));
            continue;
        }
        match load_dir(&p) {
            Ok(s) => ok.push(s),
            Err(e) => bad.push(e),
        }
    }
    ok.sort_by(|a, b| a.id.cmp(&b.id));
    (ok, bad)
}

// ------------------------------------------------------------------ 注册表（纯逻辑，可测试）

#[derive(Default)]
pub struct Registry {
    pub skins: Vec<Arc<SkinInfo>>,
    /// None = 内置哈基米
    pub active: Option<Arc<SkinInfo>>,
    /// 存档里记的 id（包暂时不在也记着，免得一挪文件夹存档就被改掉）
    pub wanted: String,
    pub errors: Vec<String>,
}

impl Registry {
    /// 重新扫描并按 wanted 选中；返回选中的是否是 wanted
    pub fn rescan(&mut self, root: &Path) -> bool {
        let (skins, errors) = scan(root);
        self.skins = skins.into_iter().map(Arc::new).collect();
        self.errors = errors;
        let w = self.wanted.clone();
        self.select(&w)
    }

    /// 选中某个皮肤（`hakimi` / "" = 内置）。找不到 → 回内置，返回 false
    pub fn select(&mut self, id: &str) -> bool {
        self.wanted = if id.is_empty() { BUILTIN_ID.to_string() } else { id.to_string() };
        if self.wanted == BUILTIN_ID {
            self.active = None;
            return true;
        }
        self.active = self.skins.iter().find(|s| s.id == self.wanted).cloned();
        self.active.is_some()
    }

    pub fn active_id(&self) -> &str { self.active.as_ref().map_or(BUILTIN_ID, |s| s.id.as_str()) }

    /// 菜单用：(id, 显示名)，内置在最前
    pub fn list(&self) -> Vec<(String, String)> {
        let mut v = vec![(BUILTIN_ID.to_string(), BUILTIN_NAME.to_string())];
        for s in &self.skins {
            let mut label = s.display_name().to_string();
            if !s.spec.author.is_empty() { label.push_str(&format!("（{}）", s.spec.author)); }
            v.push((s.id.clone(), label));
        }
        v
    }
}

// ------------------------------------------------------------------ 全局（任何线程可调）

static REG: Mutex<Option<Registry>> = Mutex::new(None);
/// 每次换皮肤 / 重新扫描 +1；渲染层据此丢掉旧皮肤的缓存图，菜单据此重建
static VERSION: AtomicU64 = AtomicU64::new(0);

fn with<R>(f: impl FnOnce(&mut Registry) -> R) -> R {
    let mut g = REG.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Registry::default))
}

fn bump() { VERSION.fetch_add(1, Ordering::Relaxed); }

pub fn version() -> u64 { VERSION.load(Ordering::Relaxed) }

pub fn skins_dir() -> PathBuf { super::paths::data_dir().join("skins") }

/// 启动时调一次：扫描 skins 目录，按存档里的 id 选中
pub fn init(saved_id: &str) {
    let root = skins_dir();
    let (ok, n, errs) = with(|r| {
        r.wanted = if saved_id.is_empty() { BUILTIN_ID.into() } else { saved_id.into() };
        let ok = r.rescan(&root);
        (ok, r.skins.len(), r.errors.clone())
    });
    for e in &errs { super::log::log(&format!("skin skipped: {e}")); }
    if !ok { super::log::log(&format!("skin \"{saved_id}\" not found, using built-in")); }
    super::log::log(&format!("skins: {n} found, active {}", active_id()));
    bump();
}

/// 「刷新皮肤列表」：重新扫描（当前皮肤的 skin.json 也重新读）。返回 (包数, 坏包数)
pub fn rescan() -> (usize, usize) {
    let root = skins_dir();
    let r = with(|r| {
        r.rescan(&root);
        for e in &r.errors { super::log::log(&format!("skin skipped: {e}")); }
        (r.skins.len(), r.errors.len())
    });
    bump();
    r
}

/// 换皮肤。成功返回新皮肤（内置 = Ok(None)）；找不到返回 Err 并保持原样
pub fn set_active(id: &str) -> Result<Option<Arc<SkinInfo>>, ()> {
    let r = with(|r| {
        if id != BUILTIN_ID && !r.skins.iter().any(|s| s.id == id) { return Err(()); }
        r.select(id);
        Ok(r.active.clone())
    });
    if r.is_ok() { bump(); }
    r
}

pub fn active() -> Option<Arc<SkinInfo>> { with(|r| r.active.clone()) }

pub fn active_id() -> String { with(|r| r.active_id().to_string()) }

pub fn list() -> Vec<(String, String)> { with(|r| r.list()) }

pub fn errors() -> Vec<String> { with(|r| r.errors.clone()) }

/// 打开前确保目录存在
pub fn ensure_dir() -> PathBuf {
    let d = skins_dir();
    let _ = std::fs::create_dir_all(&d);
    d
}

/// AI 钩子：当前角色的 persona 追加到 system prompt（内置 / 没写 persona 原样返回）
pub fn with_persona(system: String) -> String {
    match active().and_then(|s| s.persona_suffix()) {
        Some(p) => format!("{system}\n{p}"),
        None => system,
    }
}

/// TTS 钩子：当前角色 voiceDir 里这句台词的语音（读不到 / 太大返回 None）
pub fn voice_line(text: &str) -> Option<Vec<u8>> {
    let f = active()?.voice_file(text)?;
    if std::fs::metadata(&f).ok()?.len() > MAX_VOICE_BYTES { return None; }
    std::fs::read(f).ok()
}

/// TTS 配置钩子：把当前角色包的 `tts` 声线设置叠加到全局 TtsConfig 上（内置哈基米原样返回）
pub fn effective_tts(base: crate::core::config::TtsConfig) -> crate::core::config::TtsConfig {
    match active() {
        Some(s) => s.apply_tts(base),
        None => base,
    }
}

/// 台词钩子
pub fn line_override(text: &str, category: Option<&str>) -> Option<String> { active()?.line_for(text, category) }

/// 测试用：直接把某个包设成当前皮肤（不扫描 APPDATA）
#[cfg(test)]
pub fn activate_for_test(info: Option<SkinInfo>) {
    with(|r| {
        r.skins = info.into_iter().map(Arc::new).collect();
        r.active = r.skins.first().cloned();
    });
    bump();
}

// ------------------------------------------------------------------ save.json

/// 存档里的皮肤 id（没有 / 不是字符串 → 内置）
pub fn saved_id(extra: &Map<String, Value>) -> String {
    extra.get(SAVE_KEY).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).unwrap_or(BUILTIN_ID).to_string()
}

/// 写进存档的其它字段里（StatsData.extra 会原样写回 save.json，别的字段不动）
pub fn stamp(extra: &mut Map<String, Value>, id: &str) { extra.insert(SAVE_KEY.into(), Value::String(id.to_string())); }

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hakimi-skin-{}-{name}-{}", std::process::id(), fastrand::u32(..)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(p: &Path, s: &[u8]) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    #[test]
    fn parse_full() {
        let s = parse(r#"{
            "name": "小薄荷", "author": "阿绿", "version": 2,
            "persona": "你是薄荷味的猫。", "scale": 0.8,
            "sprites": { "Idle": "idle.png", "happy": "图/开心.png" },
            "overlays": { "blink": "blink.png", "mouthOpen": "mouth.png" },
            "voiceDir": "voice",
            "lines": { "poke": ["别戳啦", "嗯？"], "哈基米上线啦喵～": "小薄荷上线！", "x": 3 }
        }"#).unwrap();
        assert_eq!(s.name, "小薄荷");
        assert_eq!(s.version, "2");
        assert_eq!(s.scale, Some(0.8));
        assert_eq!(s.sprites.get("idle").map(String::as_str), Some("idle.png"));
        assert_eq!(s.sprites.get("happy").map(String::as_str), Some("图/开心.png"));
        assert_eq!(s.overlays.len(), 2);
        assert_eq!(s.voice_dir.as_deref(), Some("voice"));
        assert_eq!(s.lines["poke"].len(), 2);
        assert_eq!(s.lines["哈基米上线啦喵～"], vec!["小薄荷上线！".to_string()]);
        assert!(!s.lines.contains_key("x"));
        assert_eq!(s.warnings.len(), 1, "{:?}", s.warnings);
    }

    #[test]
    fn parse_missing_and_wrong_types() {
        // 全空：合法，全部回退
        let s = parse("{}").unwrap();
        assert_eq!(s, SkinSpec::default());
        // BOM + 只有名字
        assert_eq!(parse("\u{feff}{\"name\":\"a\"}").unwrap().name, "a");
        // 类型不对的字段只丢那一个
        let s = parse(r#"{"name":["x"],"scale":"big","sprites":{"idle":5,"sit":"sit.png","bogus":"b.png"},"overlays":[],"lines":"no","persona":"p"}"#).unwrap();
        assert_eq!(s.name, "");
        assert_eq!(s.scale, None);
        assert_eq!(s.sprites.get("sit").map(String::as_str), Some("sit.png"));
        assert!(!s.sprites.contains_key("idle"));
        assert!(s.overlays.is_empty() && s.lines.is_empty());
        assert_eq!(s.persona, "p");
        assert!(s.warnings.len() >= 5, "{:?}", s.warnings);
        // scale 夹到范围内
        assert_eq!(parse(r#"{"scale":9}"#).unwrap().scale, Some(SCALE_MAX));
        assert_eq!(parse(r#"{"scale":-1}"#).unwrap().scale, None);
        // persona 截断
        let long = format!(r#"{{"persona":"{}"}}"#, "喵".repeat(MAX_PERSONA_CHARS + 50));
        assert_eq!(parse(&long).unwrap().persona.chars().count(), MAX_PERSONA_CHARS);
    }

    #[test]
    fn parse_broken() {
        assert!(parse("").is_err());
        assert!(parse("{ name: 'x' ").is_err());
        assert!(parse("[1,2]").is_err());
        assert!(parse("null").is_err());
        assert!(parse("\"skin\"").is_err());
    }

    #[test]
    fn safe_paths() {
        let d = Path::new("base");
        assert_eq!(safe_join(d, "a/b.png"), Some(d.join("a").join("b.png")));
        assert_eq!(safe_join(d, "a\\b.png"), Some(d.join("a").join("b.png")));
        assert_eq!(safe_join(d, "./图片/待机.png"), Some(d.join("图片").join("待机.png")));
        assert_eq!(safe_join(d, "../x.png"), None);
        assert_eq!(safe_join(d, "a/../../x.png"), None);
        assert_eq!(safe_join(d, "/etc/passwd"), None);
        assert_eq!(safe_join(d, "\\\\server\\x.png"), None);
        assert_eq!(safe_join(d, ""), None);
        assert_eq!(safe_join(d, "."), None);
        #[cfg(windows)]
        assert_eq!(safe_join(d, "C:\\x.png"), None);
    }

    #[test]
    fn scan_chinese_paths_and_fallback() {
        let root = tmp("中文 路径");
        let mint = root.join("小薄荷");
        write(&mint.join(MANIFEST), r#"{"name":"小薄荷","sprites":{"idle":"图/待机.png","happy":"不存在.png"},"voiceDir":"语音","persona":"薄荷味"}"#.as_bytes());
        write(&mint.join("图").join("待机.png"), b"png");
        write(&mint.join("语音").join(format!("{}.wav", voice_stems("你好喵")[0])), b"RIFF");
        write(&mint.join("语音").join("早上好_.ogg"), b"OggS");
        write(&root.join("坏包").join(MANIFEST), b"{oops");
        std::fs::create_dir_all(root.join("空文件夹")).unwrap();
        write(&root.join("hakimi").join(MANIFEST), b"{}");
        write(&root.join("readme.txt"), b"not a dir");
        let (ok, bad) = scan(&root);
        assert_eq!(ok.len(), 1, "{bad:?}");
        assert_eq!(bad.len(), 3, "{bad:?}");
        let s = &ok[0];
        assert_eq!(s.id, "小薄荷");
        assert_eq!(s.display_name(), "小薄荷");
        // 有图的状态 → 包里的文件；没写的状态 / 写了但文件不在 → None（回退内置）
        assert_eq!(s.sprite_file("idle"), Some(mint.join("图").join("待机.png")));
        assert_eq!(s.sprite_file("happy"), None);
        assert_eq!(s.sprite_file("sleep"), None);
        assert_eq!(s.overlay_file("blink"), None);
        assert_eq!(s.scale(), 1.0);
        // 语音：hash 文件名、台词原文文件名（? 换成 _）、没有的回退
        assert!(s.voice_file("你好喵").is_some());
        assert!(s.voice_file("早上好?").is_some_and(|p| p.ends_with("早上好_.ogg")));
        assert!(s.voice_file("没录这句").is_none());
        assert!(s.persona_suffix().unwrap().contains("小薄荷") && s.persona_suffix().unwrap().ends_with("薄荷味"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn voice_hash_matches_bundled_rule() {
        // 和 assets/voice/lines 的命名一致（sha1 前 16 位）
        assert_eq!(voice_stems("喵？找哈基米有事吗～")[0], "2b60dc85bb8a2964");
        assert_eq!(voice_stems("a/b:c")[1], "a_b_c");
    }

    #[test]
    fn registry_select_and_fallback() {
        let root = tmp("reg");
        write(&root.join("demo").join(MANIFEST), br#"{"name":"Demo","author":"me"}"#);
        let mut r = Registry { wanted: "demo".into(), ..Default::default() };
        assert!(r.rescan(&root));
        assert_eq!(r.active_id(), "demo");
        assert_eq!(r.list(), vec![(BUILTIN_ID.into(), BUILTIN_NAME.into()), ("demo".into(), "Demo（me）".into())]);
        assert!(r.select(BUILTIN_ID));
        assert_eq!(r.active_id(), BUILTIN_ID);
        assert!(r.select(""));
        assert!(r.active.is_none());
        // 存档里的包不在了：用内置，但记着 wanted
        assert!(!r.select("gone"));
        assert_eq!(r.active_id(), BUILTIN_ID);
        assert_eq!(r.wanted, "gone");
        // 刷新时当前包被删了 → 回内置
        r.select("demo");
        std::fs::remove_dir_all(root.join("demo")).unwrap();
        assert!(!r.rescan(&root));
        assert!(r.active.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn line_overrides() {
        let s = SkinInfo {
            id: "m".into(),
            dir: PathBuf::from("m"),
            spec: parse(r#"{"lines":{"POKE":["戳我干嘛"],"哈基米上线啦喵～":"薄荷上线"}}"#).unwrap(),
        };
        assert_eq!(s.line_for("哈基米上线啦喵～", None).as_deref(), Some("薄荷上线"));
        assert_eq!(s.line_for("喵？找哈基米有事吗～", Some("poke")).as_deref(), Some("戳我干嘛"));
        assert_eq!(s.line_for("喵？找哈基米有事吗～", None), None);
        assert_eq!(s.line_for("别的", Some("feed")), None);
        assert!(s.persona_suffix().is_none());
    }

    #[test]
    fn save_field_roundtrip() {
        // 老存档没有 skin 字段 → 内置；写回时别的字段（包括未知的）都在
        let mut d = crate::core::save::parse(r#"{"hunger":50,"future":"x"}"#).unwrap();
        assert_eq!(saved_id(&d.extra), BUILTIN_ID);
        stamp(&mut d.extra, "小薄荷");
        let out: Value = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(out["skin"], "小薄荷");
        assert_eq!(out["future"], "x");
        assert_eq!(out["hunger"], 50.0);
        let back = crate::core::save::parse(&out.to_string()).unwrap();
        assert_eq!(saved_id(&back.extra), "小薄荷");
        // 类型不对 → 内置
        let d = crate::core::save::parse(r#"{"skin":5}"#).unwrap();
        assert_eq!(saved_id(&d.extra), BUILTIN_ID);
    }

    #[test]
    fn states_match_all_faces_and_manifest_v1_tts() {
        assert_eq!(STATES.len(), crate::types::Face::ALL.len());
        for f in crate::types::Face::ALL {
            assert!(STATES.contains(&f.stem()), "missing state in skin::STATES: {}", f.stem());
        }
        let d = tmp("v1tts");
        write(&d.join("voice").join("ref.mp3"), b"ID3fake");
        let spec = parse(r#"{
            "formatVersion": 1,
            "id": "mint_cat",
            "name": "小薄荷",
            "fallback": "builtin",
            "sprites": { "idle": "idle.png", "wink": "wink.png", "sleepy": "sleepy.png" },
            "tts": {
                "mode": "clone",
                "voice": "蜜桃",
                "style": "清冷少女音",
                "refAudio": "voice/ref.mp3",
                "sovitsPrompt": "早安喵",
                "sovitsUrl": "http://evil.example/tts"
            }
        }"#).unwrap();
        assert_eq!(spec.format_version, Some(1));
        assert_eq!(spec.manifest_id, "mint_cat");
        assert!(spec.fallback_builtin);
        assert!(spec.sprites.contains_key("wink") && spec.sprites.contains_key("sleepy"));
        assert!(spec.warnings.iter().any(|w| w.contains("外部服务器地址")), "{:?}", spec.warnings);
        let info = SkinInfo { id: "mint_cat".into(), dir: d.clone(), spec };
        let eff = info.apply_tts(crate::core::config::TtsConfig::default());
        assert_eq!(eff.voice, "蜜桃");
        assert_eq!(eff.style, "清冷少女音");
        assert!(eff.clone_ref.ends_with("ref.mp3"), "clone_ref={}", eff.clone_ref);
        assert_eq!(eff.sovits_url, "http://127.0.0.1:9880", "sovitsUrl must never be overridden by skin");

        // 路径穿越的 refAudio 必须被忽略
        let bad_spec = parse(r#"{"tts":{"mode":"sovits","refAudio":"../secret.wav"}}"#).unwrap();
        let bad_info = SkinInfo { id: "x".into(), dir: d.clone(), spec: bad_spec };
        let eff2 = bad_info.apply_tts(crate::core::config::TtsConfig::default());
        assert!(eff2.sovits_ref.is_empty());

        // 高版本 formatVersion 只记 warning，不拒读
        let future = parse(r#"{"formatVersion": 2, "name": "未来角色"}"#).unwrap();
        assert_eq!(future.format_version_or_default(), 2);
        assert!(future.warnings.iter().any(|w| w.contains("formatVersion 2")));

        // safe_single_id 校验
        assert_eq!(safe_single_id("小薄荷"), Some("小薄荷".into()));
        assert_eq!(safe_single_id("hakimi"), None);
        assert_eq!(safe_single_id("@open"), None);
        assert_eq!(safe_single_id(".hidden"), None);
        assert_eq!(safe_single_id("CON"), None);
        assert_eq!(safe_single_id("a/b"), None);
        assert_eq!(safe_single_id(".."), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
