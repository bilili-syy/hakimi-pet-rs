//! 天气：后台线程 GET wttr.in `?format=j1`（免 key，按出口 IP 定位），走 services::net（系统代理）。
//! 公开接口：`refresh(tx)` 后台拉一次（结果用 AppEvent::Weather 送回，并缓存）；`latest()` 取最近一次结果。
//! 失败/离线什么都不发（安静失败）。同一时间最多一个请求。
use crate::types::{AppEvent, EventSender, Weather, WeatherKind};
use serde_json::Value;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

pub type WeatherInfo = Weather;

const URL: &str = "https://wttr.in/?format=j1&lang=zh";
static BUSY: AtomicBool = AtomicBool::new(false);
/// (结果, 拿到时的 epoch 毫秒)
static LATEST: Mutex<Option<(WeatherInfo, u64)>> = Mutex::new(None);

fn epoch_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// 最近一次拿到的天气（3 小时以上的旧数据不算）
pub fn latest() -> Option<WeatherInfo> {
    let g = LATEST.lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref().filter(|(_, t)| epoch_ms().saturating_sub(*t) < 3 * 3600 * 1000).map(|(w, _)| w.clone())
}

/// 后台拉一次天气（已经在拉就忽略）
pub fn refresh(tx: &EventSender) {
    if BUSY.swap(true, Ordering::AcqRel) {
        return;
    }
    let tx = tx.clone();
    let r = std::thread::Builder::new().name("weather".into()).stack_size(256 * 1024).spawn(move || {
        let w = crate::services::net::get_text(URL, 20, 512 * 1024)
            .ok()
            .filter(|(st, _)| *st == 200)
            .and_then(|(_, body)| parse(&body));
        match &w {
            Some(w) => crate::core::log::log(&format!(
                "weather {:?} {}C {:?}/{:?} rain {:?}% {}",
                w.kind, w.temp_c, w.high_c, w.low_c, w.rain_chance, w.desc
            )),
            None => crate::core::log::log("weather unavailable"),
        }
        if let Some(w) = &w {
            *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = Some((w.clone(), epoch_ms()));
        }
        BUSY.store(false, Ordering::Release);
        if let Some(w) = w {
            tx.send(AppEvent::Weather(w));
        }
    });
    if r.is_err() {
        BUSY.store(false, Ordering::Release);
    }
}

/// wttr.in weatherCode → 大类（worldweatheronline 天气代码）
pub fn kind_of(code: u32) -> WeatherKind {
    match code {
        113 => WeatherKind::Clear,
        116 | 119 | 122 => WeatherKind::Cloudy,
        143 | 248 | 260 => WeatherKind::Fog,
        200 | 386 | 389 | 392 | 395 => WeatherKind::Thunder,
        179 | 182 | 185 | 227 | 230 | 317 | 320 | 323 | 326 | 329 | 332 | 335 | 338 | 350 | 362 | 365 | 368
        | 371 | 374 | 377 => WeatherKind::Snow,
        176 | 263 | 266 | 281 | 284 | 293 | 296 | 299 | 302 | 305 | 308 | 311 | 314 | 353 | 356 | 359 => {
            WeatherKind::Rain
        }
        _ => WeatherKind::Cloudy,
    }
}

/// 没有中文描述时的兜底
pub fn kind_name(k: WeatherKind) -> &'static str {
    match k {
        WeatherKind::Clear => "晴",
        WeatherKind::Cloudy => "多云",
        WeatherKind::Fog => "有雾",
        WeatherKind::Rain => "下雨",
        WeatherKind::Snow => "下雪",
        WeatherKind::Thunder => "雷雨",
    }
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

fn first_value(v: &Value) -> Option<String> {
    v.get(0)?.get("value")?.as_str().map(|s| s.trim().to_string())
}

/// 解析 j1 JSON
pub fn parse(body: &str) -> Option<WeatherInfo> {
    let v: Value = serde_json::from_str(body).ok()?;
    let cur = v.get("current_condition")?.get(0)?;
    let temp = num(cur.get("temp_C")?)? as f32;
    let code = num(cur.get("weatherCode")?)? as u32;
    let desc = cur.get("lang_zh").and_then(first_value).or_else(|| cur.get("weatherDesc").and_then(first_value)).unwrap_or_default();
    let city = v
        .get("nearest_area")
        .and_then(|a| a.get(0))
        .and_then(|a| a.get("areaName"))
        .and_then(first_value)
        .unwrap_or_default();
    let today = v.get("weather").and_then(|w| w.get(0));
    let high_c = today.and_then(|t| t.get("maxtempC")).and_then(num).map(|x| x as f32);
    let low_c = today.and_then(|t| t.get("mintempC")).and_then(num).map(|x| x as f32);
    let rain_chance = today
        .and_then(|t| t.get("hourly"))
        .and_then(|h| h.as_array())
        .and_then(|h| h.iter().filter_map(|x| x.get("chanceofrain").and_then(num)).map(|x| x.clamp(0.0, 100.0) as u32).max());
    Some(Weather { kind: kind_of(code), temp_c: temp, desc, city, high_c, low_c, rain_chance })
}

/// 播报台词：「上海现在小雨，12°C，今天 9~15°C，降雨概率 80%……」+ 一句叮嘱
pub fn report_line(w: &WeatherInfo) -> String {
    let desc = if w.desc.is_empty() { kind_name(w.kind).to_string() } else { w.desc.clone() };
    let mut s = String::new();
    if !w.city.is_empty() {
        s.push_str(&w.city);
    }
    s.push_str(&format!("现在{desc}，{}°C", w.temp_c.round() as i32));
    if let (Some(lo), Some(hi)) = (w.low_c, w.high_c) {
        s.push_str(&format!("，今天 {}~{}°C", lo.round() as i32, hi.round() as i32));
    }
    if let Some(r) = w.rain_chance {
        s.push_str(&format!("，降雨概率 {r}%"));
    }
    s.push('。');
    let rainy = matches!(w.kind, WeatherKind::Rain | WeatherKind::Thunder) || w.rain_chance.is_some_and(|r| r >= 50);
    let tip = if rainy {
        "出门记得带伞喵！"
    } else if w.kind == WeatherKind::Snow {
        "路滑，走慢点喵～"
    } else if w.high_c.unwrap_or(w.temp_c) >= 30.0 {
        "好热，多喝水别中暑喵！"
    } else if w.low_c.unwrap_or(w.temp_c) <= 5.0 {
        "好冷，多穿点衣服喵！"
    } else {
        "哈基米播报完毕喵～"
    };
    s.push_str(tip);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = r#"{"current_condition":[{"temp_C":"12","weatherCode":"296","weatherDesc":[{"value":"Light rain"}],"lang_zh":[{"value":"小雨"}]}],
        "nearest_area":[{"areaName":[{"value":"Shanghai"}]}],
        "weather":[{"maxtempC":"15","mintempC":"9","hourly":[{"chanceofrain":"20"},{"chanceofrain":"85"},{"chanceofrain":"x"}]}]}"#;

    #[test]
    fn parses_j1() {
        let w = parse(BODY).unwrap();
        assert_eq!(w.kind, WeatherKind::Rain);
        assert_eq!(w.temp_c, 12.0);
        assert_eq!(w.desc, "小雨");
        assert_eq!(w.city, "Shanghai");
        assert_eq!((w.high_c, w.low_c, w.rain_chance), (Some(15.0), Some(9.0), Some(85)));
        assert!(parse("<html>").is_none());
        assert!(parse(r#"{"current_condition":[]}"#).is_none());
        // 没有 weather 段也能用
        let w = parse(r#"{"current_condition":[{"temp_C":3,"weatherCode":113}]}"#).unwrap();
        assert_eq!((w.kind, w.high_c, w.rain_chance), (WeatherKind::Clear, None, None));
        assert_eq!(kind_of(338), WeatherKind::Snow);
        assert_eq!(kind_of(389), WeatherKind::Thunder);
    }

    #[test]
    fn report() {
        let l = report_line(&parse(BODY).unwrap());
        assert_eq!(l, "Shanghai现在小雨，12°C，今天 9~15°C，降雨概率 85%。出门记得带伞喵！");
        let w = parse(r#"{"current_condition":[{"temp_C":"20","weatherCode":"113","lang_zh":[{"value":""}]}]}"#).unwrap();
        assert_eq!(report_line(&w), "现在晴，20°C。哈基米播报完毕喵～");
    }
}
