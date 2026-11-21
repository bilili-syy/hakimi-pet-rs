//! 今天吃了多少「粮食」：云端聊天模型的 token + MiMo 语音的字数（1 字算 1 份）。
//! 本机模型（Ollama / GPT-SoVITS）不花钱，不记。存在数据目录 usage.json，只留最近 30 天。
//! 每日上限（config.dailyFoodCap，0 = 不限）到了：聊天和语音先降级（固定台词 / 内置录音），不直接停。
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Day {
    /// "YYYY-MM-DD"（当地时间）
    pub date: String,
    /// 云端聊天模型 token（接口 usage 给的；没给就按字数估）
    pub tokens: u64,
    /// MiMo 在线合成的字数
    pub tts_chars: u64,
    /// 云端请求次数
    pub requests: u32,
}
impl Day {
    /// 粮食总数：token + 语音字数
    pub fn food(&self) -> u64 { self.tokens + self.tts_chars }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Book {
    /// 最近的在最后
    pub days: Vec<Day>,
}

const KEEP_DAYS: usize = 30;

/// 吃到上限的几成算「快吃撑了」
pub const STUFFED_FRAC: f64 = 0.8;

/// 饱不饱（只看今天用量和上限；上限 0 = 永远正常）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fullness {
    Normal,
    /// 到上限八成：打嗝、拖长尾音
    Stuffed,
    /// 到上限：降级
    Over,
}

pub fn fullness(food: u64, cap: u64) -> Fullness {
    if cap == 0 {
        Fullness::Normal
    } else if food >= cap {
        Fullness::Over
    } else if food as f64 >= cap as f64 * STUFFED_FRAC {
        Fullness::Stuffed
    } else {
        Fullness::Normal
    }
}

impl Book {
    fn day_mut(&mut self, today: &str) -> &mut Day {
        if self.days.last().map(|d| d.date.as_str()) != Some(today) {
            self.days.push(Day { date: today.to_string(), ..Day::default() });
            if self.days.len() > KEEP_DAYS {
                let cut = self.days.len() - KEEP_DAYS;
                self.days.drain(..cut);
            }
        }
        self.days.last_mut().expect("just pushed")
    }
    pub fn add(&mut self, today: &str, tokens: u64, tts_chars: u64) {
        let d = self.day_mut(today);
        d.tokens += tokens;
        d.tts_chars += tts_chars;
        if tokens > 0 {
            d.requests += 1;
        }
    }
    pub fn get(&self, today: &str) -> Day {
        self.days.iter().rev().find(|d| d.date == today).cloned().unwrap_or_else(|| Day { date: today.to_string(), ..Day::default() })
    }
}

/// 聊天接口返回里的 token 用量：OpenAI 兼容 usage.total_tokens / Gemini usageMetadata.totalTokenCount
pub fn tokens_in(json: &serde_json::Value) -> Option<u64> {
    json.pointer("/usage/total_tokens")
        .or_else(|| json.pointer("/usageMetadata/totalTokenCount"))
        .and_then(serde_json::Value::as_u64)
        .filter(|n| *n > 0)
}

/// 没给 usage 时按字数粗估：中文大约 1 字 1 token
pub fn estimate_tokens(chars: usize) -> u64 { (chars as u64).max(1) }

// ------------------------------------------------------------------ 全局（真实文件）

static BOOK: Mutex<Option<Book>> = Mutex::new(None);

fn today_str() -> String {
    let d = crate::core::memo::Now::get().dt;
    format!("{:04}-{:02}-{:02}", d.y, d.mo, d.d)
}

fn path() -> std::path::PathBuf { crate::core::paths::data_dir().join("usage.json") }

fn with<R>(f: impl FnOnce(&mut Book) -> R) -> R {
    let mut g = BOOK.lock().unwrap_or_else(|e| e.into_inner());
    let book = g.get_or_insert_with(|| {
        if cfg!(test) {
            return Book::default();
        }
        std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    });
    f(book)
}

fn save(book: &Book) {
    if cfg!(test) {
        return;
    }
    if let Ok(text) = serde_json::to_string(book) {
        let p = path();
        let tmp = p.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

/// 记一次云端聊天用量
pub fn add_tokens(n: u64) {
    if n == 0 {
        return;
    }
    let today = today_str();
    with(|b| {
        b.add(&today, n, 0);
        save(b);
    });
}

/// 记一次 MiMo 在线合成
pub fn add_tts_chars(n: u64) {
    if n == 0 {
        return;
    }
    let today = today_str();
    with(|b| {
        b.add(&today, 0, n);
        save(b);
    });
}

/// 今天的用量
pub fn today() -> Day {
    let t = today_str();
    with(|b| b.get(&t))
}

/// 按设置里的上限算今天饱不饱
pub fn now_fullness() -> Fullness { fullness(today().food(), crate::core::config::get().daily_food_cap) }

/// 今天吃到上限了（云端聊天 / MiMo 语音该降级了）
pub fn over_cap() -> bool { now_fullness() == Fullness::Over }

/// 设置页显示的一句：今天已用 1.2 万（聊天 1.1 万 token · 语音 800 字）
pub fn summary(d: &Day) -> String {
    fn n(v: u64) -> String {
        if v >= 10_000 { format!("{:.1} 万", v as f64 / 10_000.0) } else { v.to_string() }
    }
    format!("今天已用 {}（聊天 {} token · 语音 {} 字）", n(d.food()), n(d.tokens), n(d.tts_chars))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_roll_and_trim() {
        let mut b = Book::default();
        b.add("2026-11-20", 100, 0);
        b.add("2026-11-21", 50, 0);
        b.add("2026-11-21", 0, 30);
        let d = b.get("2026-11-21");
        assert_eq!((d.tokens, d.tts_chars, d.requests, d.food()), (50, 30, 1, 80));
        assert_eq!(b.get("2026-11-20").tokens, 100);
        assert_eq!(b.get("2026-11-22").food(), 0);
        for i in 0..40 {
            b.add(&format!("2027-01-{:02}", i % 28 + 1), 1, 0);
        }
        assert!(b.days.len() <= KEEP_DAYS);
    }

    #[test]
    fn fullness_levels() {
        assert_eq!(fullness(999_999, 0), Fullness::Normal);
        assert_eq!(fullness(79, 100), Fullness::Normal);
        assert_eq!(fullness(80, 100), Fullness::Stuffed);
        assert_eq!(fullness(100, 100), Fullness::Over);
    }

    #[test]
    fn usage_from_responses() {
        let o: serde_json::Value = serde_json::from_str(r#"{"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#).unwrap();
        assert_eq!(tokens_in(&o), Some(15));
        let g: serde_json::Value = serde_json::from_str(r#"{"usageMetadata":{"totalTokenCount":42}}"#).unwrap();
        assert_eq!(tokens_in(&g), Some(42));
        assert_eq!(tokens_in(&serde_json::json!({})), None);
        assert_eq!(estimate_tokens(0), 1);
    }

    #[test]
    fn summary_text() {
        let d = Day { date: "x".into(), tokens: 11_000, tts_chars: 800, requests: 3 };
        assert_eq!(summary(&d), "今天已用 1.2 万（聊天 1.1 万 token · 语音 800 字）");
    }
}
