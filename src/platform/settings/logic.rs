//! 设置窗口的纯逻辑（服务商预设、识别、校验、时间解析），有单元测试
use super::*;

// ================================================================ 纯逻辑（可测）

/// AI 服务商预设。gemini 走原生接口，其余都是 OpenAI 兼容（provider = "openai" + baseUrl）
pub struct Preset {
    pub name: &'static str,
    pub provider: &'static str,
    pub base: &'static str,
    pub models: &'static [&'static str],
    /// 「如何获取 API Key？」打开的官网页面（都 curl 验证过能打开）；空 = 自定义，只显示说明
    pub key_url: &'static str,
    pub note: &'static str,
}

pub const PRESETS: &[Preset] = &[
    Preset {
        name: "Google Gemini",
        provider: "gemini",
        base: "",
        models: &["gemini-2.5-flash", "gemini-flash-latest", "gemini-3.5-flash-lite", "gemini-3.8-flash"],
        key_url: "https://aistudio.google.com/apikey",
        note: "有免费额度，够日常聊天。国内需要开代理（下面「代理」可以填）。接口地址留空就行。",
    },
    Preset {
        name: "DeepSeek（深度求索）",
        provider: "openai",
        base: "https://api.deepseek.com/v1",
        models: &["deepseek-flash", "deepseek-v4-pro"],
        key_url: "https://platform.deepseek.com/api_keys",
        note: "国内直连，不用代理；按用量付费，要先在平台充一点钱。",
    },
    Preset {
        name: "通义千问（阿里云百炼）",
        provider: "openai",
        base: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        models: &["qwen-flash", "qwen-plus", "qwen3.8-flash", "qwen3.7-plus"],
        key_url: "https://bailian.console.aliyun.com/cn-beijing/settings/api-key",
        note: "国内直连；新用户一般有免费额度（在百炼控制台领取）。",
    },
    Preset {
        name: "Kimi（月之暗面）",
        provider: "openai",
        base: "https://api.moonshot.cn/v1",
        models: &["kimi-k3", "kimi-k3.1-pro"],
        key_url: "https://platform.kimi.com/console/api-keys",
        note: "国内直连；按用量付费。",
    },
    Preset {
        name: "智谱 GLM",
        provider: "openai",
        base: "https://open.bigmodel.cn/api/paas/v4",
        models: &["glm-4.7-flash", "glm-5.3-flash", "glm-5.3"],
        key_url: "https://open.bigmodel.cn/usercenter/apikeys",
        note: "国内直连；glm-4.7-flash 免费。注意地址结尾是 /v4，不是 /v1。",
    },
    Preset {
        name: "OpenAI",
        provider: "openai",
        base: "https://api.openai.com/v1",
        models: &["gpt-5-mini", "gpt-4.1-mini", "gpt-4o-mini"],
        key_url: "https://platform.openai.com/api-keys",
        note: "需要海外付费账户；国内要开代理。",
    },
    Preset {
        name: "自定义（OpenAI 兼容接口）",
        provider: "openai",
        base: "",
        models: &[],
        key_url: "",
        note: "填服务商给的 OpenAI 兼容接口地址（一般以 /v1 结尾），再填 API Key 和模型名，都在服务商网站的控制台里能找到。",
    },
];

pub(super) const CUSTOM: usize = PRESETS.len() - 1;

pub(super) fn norm_url(s: &str) -> String {
    s.trim().trim_end_matches('/').to_ascii_lowercase()
}

/// 按 config 的 provider + baseUrl 认出是哪个预设
pub fn detect_preset(provider: &str, base_url: &str) -> usize {
    if provider != "openai" {
        return 0; // gemini（原版只认 openai，其余都当 gemini）
    }
    let b = norm_url(base_url);
    if b.is_empty() {
        return PRESETS.iter().position(|p| p.name == "OpenAI").unwrap_or(CUSTOM);
    }
    PRESETS.iter().position(|p| p.provider == "openai" && !p.base.is_empty() && norm_url(p.base) == b).unwrap_or(CUSTOM)
}

/// 切换服务商时接口地址怎么变：换到预设就填预设的；换到自定义时，原来是预设地址就清空让用户填，自己填的保留
pub fn switch_base(old: usize, new: usize, current: &str) -> String {
    let p = &PRESETS[new];
    if new != CUSTOM {
        return p.base.to_string();
    }
    let was_preset = old != CUSTOM && norm_url(current) == norm_url(PRESETS[old].base);
    if was_preset { String::new() } else { current.trim().to_string() }
}

/// AI 页校验
pub fn validate_ai(preset: usize, base: &str, model: &str) -> Option<String> {
    let b = base.trim();
    if preset == CUSTOM && b.is_empty() {
        return Some("选了「自定义」就要填接口地址（一般以 /v1 结尾）".into());
    }
    if !b.is_empty() && !(b.starts_with("http://") || b.starts_with("https://")) {
        return Some("接口地址要以 http:// 或 https:// 开头".into());
    }
    if model.trim().is_empty() {
        return Some("模型名不能空着（可以从下拉里选）".into());
    }
    None
}

/// "45" → 45；空 = 0；超范围/不是数字 = None
pub fn parse_minutes(s: &str, max: u32) -> Option<u32> {
    let t = s.trim();
    if t.is_empty() {
        return Some(0);
    }
    t.parse::<u32>().ok().filter(|v| *v <= max)
}

/// 下班时间："18:00" / "8:5" → "08:05"；空 / "0" = ""（关）；不合法 = None
pub fn parse_clock(s: &str) -> Option<String> {
    let t = s.trim().replace('：', ":");
    if t.is_empty() || t == "0" {
        return Some(String::new());
    }
    let (h, m) = t.split_once(':')?;
    let (h, m): (u32, u32) = (h.trim().parse().ok()?, m.trim().parse().ok()?);
    (h < 24 && m < 60).then(|| format!("{h:02}:{m:02}"))
}

/// 最接近的大小档位
/// 备忘录时间："18:00" "9:30" "18点" "9点半" "18：00"
pub fn parse_hm(s: &str) -> Option<(u32, u32)> {
    let s = s.trim().replace(['：', '点', '.'], ":");
    let s = s.trim_end_matches(':');
    let (h, m) = s.split_once(':').unwrap_or((s, "0"));
    let m = m.trim().trim_end_matches('分');
    let h: u32 = h.trim().parse().ok()?;
    let m: u32 = match m {
        "" => 0,
        "半" => 30,
        m => m.parse().ok()?,
    };
    (h < 24 && m < 60).then_some((h, m))
}

/// 备忘录日期："2026-11-21" "2026/11/21" "11-21" "11/21" "11月21日"（只写月日、已过了就算明年）
pub(super) fn parse_day(s: &str, now: Dt) -> Option<Dt> {
    let s = s.replace(['/', '.', '年', '月'], "-").replace(['日', '号'], "");
    let n: Vec<u32> = s.split('-').map(str::trim).filter(|x| !x.is_empty()).map(|x| x.parse().ok()).collect::<Option<_>>()?;
    let (y, mo, d) = match n[..] {
        [y, mo, d] => (y as i32, mo, d),
        [mo, d] => (now.y, mo, d),
        _ => return None,
    };
    let t = Dt::new(y, mo, d, 0, 0)?;
    if n.len() == 2 && t.date() < now.date() { Dt::new(y + 1, mo, d, 0, 0) } else { Some(t) }
}

/// 备忘录表单 → 提醒时刻。都空 = 随手记（None）；只填时间 = 今天（过了就明天）；重复的已过时刻排到下一次
pub fn parse_when(date: &str, time: &str, repeat: Repeat, now: Dt) -> Result<Option<Dt>, String> {
    let (date, time) = (date.trim(), time.trim());
    if time.is_empty() {
        if date.is_empty() {
            return Ok(None);
        }
        return Err("填了日期，也要填几点（比如 18:00）".into());
    }
    let (h, mi) = parse_hm(time).ok_or_else(|| "「时间」要填 18:00 这样的".to_string())?;
    let day = match date {
        "" | "今天" => None,
        "明天" => Some(now.add_days(1)),
        "后天" => Some(now.add_days(2)),
        d => Some(parse_day(d, now).ok_or_else(|| "「日期」要填 2026-11-21、11-21 或 明天".to_string())?),
    };
    let mut t = day.unwrap_or(now).at(h, mi);
    if t <= now {
        if repeat != Repeat::None {
            t = repeat.next_after(t, now).unwrap_or(t);
        } else if day.is_none() {
            t = t.add_days(1);
        } else {
            return Err("这个时间已经过了".into());
        }
    }
    Ok(Some(t))
}

pub fn nearest_scale(s: f32) -> usize {
    SCALES
        .iter()
        .enumerate()
        .min_by(|a, b| (a.1.0 - s).abs().total_cmp(&(b.1.0 - s).abs()))
        .map(|x| x.0)
        .unwrap_or(2)
}

