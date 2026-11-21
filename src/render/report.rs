//! 每周陪伴报告图：最近 7 天陪了多久、喂了几条小鱼干、聊了几句、最常出现的表情、她记住的事（主人勾选的）。
//! 纯 tiny-skia + GDI 文字（和气泡同一套），画成 PNG 字节。数据整理（build）是纯逻辑，可单测。
use super::shapes::fill_rrect;
use super::text::{TextEngine, blit};
use crate::core::usage::Day;
use tiny_skia::{Pixmap, PixmapPaint, Transform};

pub const W: u32 = 760;
const PAD: f32 = 40.0;

/// 画图要的全部数据
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    /// 「11月15日 – 11月21日」
    pub range: String,
    /// 认识第几天（0 = 不知道）
    pub met_days: i64,
    /// 亲密度等级名
    pub level: String,
    /// (周几, 分钟)，老的在前
    pub days: Vec<(String, u64)>,
    pub total_min: u64,
    pub feeds: u32,
    pub chats: u32,
    /// 最常出现的表情（中文名, 次数）
    pub top_face: Option<(String, u32)>,
    /// 最常出现表情的英文名（挑立绘用）
    pub top_face_key: String,
    /// 主人勾了要放进图里的「她记住的事」
    pub memories: Vec<String>,
    /// 页脚日期 "2026-11-21"
    pub today: String,
}

/// Emotion 英文名 → 中文
pub fn face_name(key: &str) -> &'static str {
    match key {
        "Happy" => "开心",
        "Love" => "喜欢你",
        "Shy" => "害羞",
        "Surprised" => "惊讶",
        "Cry" => "哭哭",
        "Hiss" => "哈气",
        "Think" => "思考",
        "Dizzy" => "晕乎乎",
        "Stare" => "盯——",
        "Yawn" => "打哈欠",
        "Idle" => "发呆",
        "Pout" => "嘟嘴",
        "Smug" => "得意",
        "Confused" => "疑惑",
        "Scared" => "害怕",
        "Starry" => "星星眼",
        "Puff" => "鼓脸",
        _ => "开心",
    }
}

const WEEK: [&str; 7] = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];

fn weekday(date: &str) -> &'static str {
    crate::core::stats::day_number(date).map(|n| WEEK[(n + 4).rem_euclid(7) as usize]).unwrap_or("")
}

fn md(date: &str) -> String {
    let p: Vec<&str> = date.split('-').collect();
    match (p.get(1).and_then(|m| m.parse::<u32>().ok()), p.get(2).and_then(|d| d.parse::<u32>().ok())) {
        (Some(m), Some(d)) => format!("{m}月{d}日"),
        _ => date.to_string(),
    }
}

/// 整理 7 天的记录。first_met 是认识那天 "YYYY-MM-DD"（空 = 不知道）
pub fn build(week: &[Day], first_met: &str, level: &str, memories: Vec<String>) -> Report {
    let today = week.last().map(|d| d.date.clone()).unwrap_or_default();
    let range = match (week.first(), week.last()) {
        (Some(a), Some(b)) => format!("{} – {}", md(&a.date), md(&b.date)),
        _ => String::new(),
    };
    let met_days = match (crate::core::stats::day_number(first_met), crate::core::stats::day_number(&today)) {
        (Some(a), Some(b)) if b >= a => b - a + 1,
        _ => 0,
    };
    let mut faces: std::collections::BTreeMap<String, u32> = Default::default();
    for d in week {
        for (k, v) in &d.faces {
            if *v > 0 {
                let e = faces.entry(k.clone()).or_insert(0);
                *e = e.saturating_add(*v);
            }
        }
    }
    // 次数最多的；一样多按名字排，结果稳定
    let top = faces.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)));
    let total_sec = week.iter().fold(0u64, |acc, d| acc.saturating_add(d.seconds));
    Report {
        range,
        met_days,
        level: level.to_string(),
        days: week.iter().map(|d| (weekday(&d.date).to_string(), d.seconds / 60)).collect(),
        total_min: total_sec / 60,
        feeds: week.iter().fold(0u32, |acc, d| acc.saturating_add(d.feeds)),
        chats: week.iter().fold(0u32, |acc, d| acc.saturating_add(d.chats)),
        top_face: top.as_ref().map(|(k, n)| (face_name(k).to_string(), *n)),
        top_face_key: top.map(|(k, _)| k).unwrap_or_else(|| "Happy".into()),
        memories,
        today,
    }
}

/// 135 → "2.3 小时"；40 → "40 分钟"
pub fn duration_text(min: u64) -> String {
    if min >= 60 {
        let h = min as f64 / 60.0;
        if (h - h.round()).abs() < 0.05 { format!("{} 小时", h.round() as u64) } else { format!("{h:.1} 小时") }
    } else {
        format!("{min} 分钟")
    }
}

/// 柱子顶上的小字：90 → "1.5h"；40 → "40m"；0 → ""
fn short_dur(min: u64) -> String {
    match min {
        0 => String::new(),
        m if m >= 60 => {
            let h = m as f64 / 60.0;
            if (h - h.round()).abs() < 0.05 { format!("{}h", h.round() as u64) } else { format!("{h:.1}h") }
        }
        m => format!("{m}m"),
    }
}

// ------------------------------------------------------------------ 画

const BG: u32 = 0xFFF7FA;
const BAND: u32 = 0xFFD9E6;
const CARD: u32 = 0xFFFFFF;
const INK: u32 = 0x4A3440;
const GRAY: u32 = 0x9A8590;
const PINK: u32 = 0xF27BA5;
const BAR_BG: u32 = 0xFBE4EC;

struct Pen<'a> {
    pm: &'a mut Pixmap,
    te: &'a mut TextEngine,
}
impl Pen<'_> {
    fn text(&mut self, s: &str, x: f32, y: f32, px: f32, bold: bool, rgb: u32) {
        if let Some(cov) = self.te.raster(s, px, bold) {
            blit(self.pm, &cov, x.round() as i32, y.round() as i32, rgb, 1.0, None);
        }
    }
    /// 在 [x, x+w] 里居中
    #[allow(clippy::too_many_arguments)]
    fn center(&mut self, s: &str, x: f32, w: f32, y: f32, px: f32, bold: bool, rgb: u32) {
        let tw = self.te.text_width(s, px, bold);
        self.text(s, x + (w - tw) / 2.0, y, px, bold, rgb);
    }
    /// 字号从 px 往下缩，直到放得进 max_w
    fn fit(&mut self, s: &str, max_w: f32, px: f32, bold: bool) -> f32 {
        let mut p = px;
        while p > 12.0 && self.te.text_width(s, p, bold) > max_w {
            p -= 1.0;
        }
        p
    }
    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, c: u32) {
        fill_rrect(self.pm, x, y, w, h, r, c, 1.0, Transform::identity());
    }
}

/// 每条记忆最多几行
const MEMO_LINES: usize = 3;
const MEMO_PX: f32 = 22.0;

/// 画成图。sprite = 她的立绘 PNG 字节（None = 不画）
/// 按当前服装（含默认水手服 ""/"sailor" 分层与原装卫衣 "hoodie"）和表情拼出周报立绘
pub fn resolve_sprite(costume: &str, face: crate::types::Face) -> Option<Pixmap> {
    let c = crate::core::costume::normalize(costume);
    let layers = super::sprites::layers_for(face, c, &[]);
    let bytes = crate::assets::get(&layers.base).or_else(|| crate::assets::get("sprites/happy.png"))?;
    let mut pm = Pixmap::decode_png(bytes).ok()?;
    if layers.tail
        && let Some(tb) = crate::assets::get(super::sprites::TAIL)
        && let Ok(tp) = Pixmap::decode_png(tb)
    {
        pm.draw_pixmap(0, 0, tp.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    }
    if layers.star
        && let Some(sb) = crate::assets::get(super::sprites::STAR)
        && let Ok(sp) = Pixmap::decode_png(sb)
    {
        pm.draw_pixmap(0, 0, sp.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    }
    Some(pm)
}

#[cfg(test)]
pub fn draw(r: &Report, sprite: Option<&[u8]>, te: &mut TextEngine) -> Option<Pixmap> {
    draw_pixmap(r, sprite.and_then(|b| Pixmap::decode_png(b).ok()), te)
}

pub fn draw_pixmap(r: &Report, sprite: Option<Pixmap>, te: &mut TextEngine) -> Option<Pixmap> {
    let inner = W as f32 - PAD * 2.0;
    // 先排好记忆的行，才知道图多高
    // (是不是一条的第一行, 字)：第一行前面画「·」，后面的行和第一行的字对齐
    let bullet_w = te.text_width("·  ", MEMO_PX, false).ceil();
    let mut memo_lines: Vec<(bool, String)> = Vec::new();
    for m in &r.memories {
        let mut ls = te.wrap(m.trim(), inner - 48.0 - bullet_w, MEMO_PX, false);
        if ls.len() > MEMO_LINES {
            ls.truncate(MEMO_LINES);
            if let Some(last) = ls.last_mut() {
                let mut t: Vec<char> = last.chars().collect();
                t.truncate(t.len().saturating_sub(1));
                *last = t.into_iter().collect::<String>() + "…";
            }
        }
        memo_lines.extend(ls.into_iter().enumerate().map(|(i, l)| (i == 0, l)));
    }
    let line_h = (te.cell_height(MEMO_PX, false) + 6.0).ceil();
    let memo_h = if memo_lines.is_empty() { 0.0 } else { 56.0 + memo_lines.len() as f32 * line_h + 24.0 };
    let chart_top = 380.0;
    let chart_h = 250.0;
    let h = (chart_top + chart_h + 24.0 + memo_h + 60.0).ceil() as u32;

    let mut pm = Pixmap::new(W, h)?;
    pm.fill(super::shapes::rgb(BG, 1.0));
    let mut pen = Pen { pm: &mut pm, te };
    // 顶部粉色带
    pen.rect(0.0, 0.0, W as f32, 200.0, 0.0, BAND);

    // 立绘：右上角，脚踩在卡片上沿
    if let Some(src) = sprite {
        let sh = 236.0;
        let sw = sh * src.width() as f32 / src.height().max(1) as f32;
        if let Some(sp) = super::sprites::scale_pixmap(src, sw.round() as u32, sh as u32) {
            pen.pm.draw_pixmap((W as f32 - PAD - sw + 10.0) as i32, -4, sp.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        }
    }

    let title_px = pen.fit("哈基米的本周陪伴报告", 460.0, 40.0, true);
    pen.text("哈基米的本周陪伴报告", PAD, 46.0, title_px, true, INK);
    pen.text(&r.range, PAD, 108.0, 24.0, false, INK);
    let mut sub = Vec::new();
    if r.met_days > 0 {
        sub.push(format!("认识第 {} 天", r.met_days));
    }
    if !r.level.is_empty() {
        sub.push(format!("亲密度：{}", r.level));
    }
    let sub = sub.join(" · ");
    let sub_px = pen.fit(&sub, 460.0, 20.0, false);
    pen.text(&sub, PAD, 146.0, sub_px, false, GRAY);

    // 四张小卡
    let gap = 16.0;
    let cw = (inner - gap * 3.0) / 4.0;
    let cards: [(String, &str); 4] = [
        (duration_text(r.total_min), "陪你的时间"),
        (format!("{} 条", r.feeds), "吃到的小鱼干"),
        (format!("{} 句", r.chats), "陪你聊的话"),
        (r.top_face.as_ref().map(|f| f.0.clone()).unwrap_or_else(|| "—".into()), "最常出现的表情"),
    ];
    let cy = 222.0;
    for (i, (big, label)) in cards.iter().enumerate() {
        let x = PAD + i as f32 * (cw + gap);
        pen.rect(x, cy, cw, 128.0, 18.0, CARD);
        let px = pen.fit(big, cw - 20.0, 34.0, true);
        pen.center(big, x, cw, cy + 26.0, px, true, PINK);
        let lpx = pen.fit(label, cw - 16.0, 18.0, false);
        pen.center(label, x, cw, cy + 84.0, lpx, false, GRAY);
    }

    // 每天陪了多久
    pen.rect(PAD, chart_top, inner, chart_h, 18.0, CARD);
    pen.text("每天陪你多久", PAD + 24.0, chart_top + 18.0, 22.0, true, INK);
    let max = r.days.iter().map(|d| d.1).max().unwrap_or(0).max(1);
    let n = r.days.len().max(1) as f32;
    let col = (inner - 48.0) / n;
    let bar_w = (col * 0.5).min(44.0);
    let base = chart_top + chart_h - 48.0;
    let top = chart_top + 84.0;
    for (i, (wd, min)) in r.days.iter().enumerate() {
        let cx = PAD + 24.0 + col * i as f32;
        let bx = cx + (col - bar_w) / 2.0;
        pen.rect(bx, top, bar_w, base - top, 8.0, BAR_BG);
        if *min > 0 {
            let bh = ((base - top) * (*min as f32 / max as f32)).max(10.0);
            pen.rect(bx, base - bh, bar_w, bh, 8.0, PINK);
        }
        let s = short_dur(*min);
        if !s.is_empty() {
            let bh = ((base - top) * (*min as f32 / max as f32)).max(10.0);
            pen.center(&s, cx, col, base - bh - 26.0, 16.0, false, GRAY);
        }
        pen.center(wd, cx, col, base + 10.0, 18.0, false, INK);
    }

    // 她记住的事（主人勾的）
    let mut y = chart_top + chart_h + 24.0;
    if !memo_lines.is_empty() {
        pen.rect(PAD, y, inner, memo_h - 24.0, 18.0, CARD);
        pen.text("她记住的事", PAD + 24.0, y + 18.0, 22.0, true, INK);
        let mut ly = y + 56.0;
        for (first, l) in &memo_lines {
            if *first {
                pen.text("·", PAD + 24.0, ly, MEMO_PX, true, PINK);
            }
            pen.text(l, PAD + 24.0 + bullet_w, ly, MEMO_PX, false, INK);
            ly += line_h;
        }
        y += memo_h;
    }
    let foot = format!("hakimi-pet · {}", r.today);
    pen.center(&foot, 0.0, W as f32, y + 14.0, 16.0, false, GRAY);
    Some(pm)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(date: &str, secs: u64, feeds: u32, faces: &[(&str, u32)]) -> Day {
        Day {
            date: date.into(),
            seconds: secs,
            feeds,
            chats: feeds * 2,
            faces: faces.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            ..Day::default()
        }
    }

    fn week() -> Vec<Day> {
        vec![
            day("2026-11-15", 0, 0, &[]),
            day("2026-11-16", 3600, 1, &[("Happy", 2)]),
            day("2026-11-17", 1800, 0, &[("Shy", 1)]),
            day("2026-11-18", 7200, 2, &[("Happy", 1), ("Love", 4)]),
            day("2026-11-19", 600, 0, &[]),
            day("2026-11-20", 0, 0, &[]),
            day("2026-11-21", 5400, 3, &[("Love", 1)]),
        ]
    }

    #[test]
    fn builds_numbers() {
        let r = build(&week(), "2026-11-01", "好朋友", vec![]);
        assert_eq!(r.range, "11月15日 – 11月21日");
        assert_eq!(r.met_days, 21);
        assert_eq!(r.total_min, 310);
        assert_eq!((r.feeds, r.chats), (6, 12));
        assert_eq!(r.top_face, Some(("喜欢你".into(), 5)));
        assert_eq!(r.top_face_key, "Love");
        assert_eq!(r.days[0], ("周日".into(), 0));
        assert_eq!(r.days[6], ("周六".into(), 90));
        let empty = build(&[], "", "", vec![]);
        assert_eq!(empty.top_face, None);
        assert_eq!(empty.met_days, 0);
    }

    #[test]
    fn durations() {
        assert_eq!(duration_text(40), "40 分钟");
        assert_eq!(duration_text(120), "2 小时");
        assert_eq!(duration_text(310), "5.2 小时");
        assert_eq!(short_dur(0), "");
        assert_eq!(short_dur(90), "1.5h");
        assert_eq!(short_dur(45), "45m");
    }

    #[test]
    fn draws_png_and_grows_with_memories() {
        let mut te = TextEngine::new();
        let sprite = crate::assets::get("sprites/happy.png");
        let a = draw(&build(&week(), "2026-11-01", "好朋友", vec![]), sprite, &mut te).expect("pixmap");
        let long = "主人喜欢吃草莓味的冰淇淋，每次加班到很晚都会说想去海边看日出，还养了一只叫团子的橘猫，".repeat(3);
        let b = draw(&build(&week(), "2026-11-01", "好朋友", vec!["主人生日是 4 月 3 日".into(), long]), sprite, &mut te)
            .expect("pixmap");
        assert_eq!(a.width(), W);
        assert!(b.height() > a.height());
        // 长记忆最多 3 行：图不会无限长
        assert!(b.height() < a.height() + 260);
        let png = b.encode_png().expect("png");
        assert!(png.starts_with(b"\x89PNG"));
        if let Ok(dir) = std::env::var("HAKIMI_REPORT_SAMPLE") {
            let _ = std::fs::write(std::path::Path::new(&dir).join("report-sample.png"), &png);
        }
        // 默认水手服（"" / "sailor"）、原装卫衣（"hoodie"）和其他服装都能拿到对应表情立绘，且水手服与卫衣结果不同
        let sailor_love = resolve_sprite("", crate::types::Face::Love).expect("sailor love");
        let hoodie_love = resolve_sprite("hoodie", crate::types::Face::Love).expect("hoodie love");
        let maid_starry = resolve_sprite("maid", crate::types::Face::Starry).expect("maid starry");
        assert_ne!(sailor_love.data(), hoodie_love.data());
        assert_ne!(sailor_love.data(), maid_starry.data());
    }
}
