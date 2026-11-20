//! 情绪标签：让模型在回复开头写一个 [happy] 这样的标签，这里负责把它认出来并从文字里去掉
//! （气泡和 TTS 都只拿到干净的文字）。没有标签或不认识就按本地关键词猜，再不行 idle。
//!
//! 一次性回复用 [`analyze`]；流式回复（标签可能被拆成 "[ha" + "ppy]"）用 [`TagStream`]。
//! 只做文字处理，不碰聊天流程本身。
use crate::types::Face;

/// 只用现有立绘名
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Emotion { Happy, Love, Shy, Surprised, Cry, Hiss, Think, Dizzy, Stare, Yawn, Idle }

impl Emotion {
    pub const ALL: [Emotion; 11] = [
        Emotion::Happy, Emotion::Love, Emotion::Shy, Emotion::Surprised, Emotion::Cry, Emotion::Hiss,
        Emotion::Think, Emotion::Dizzy, Emotion::Stare, Emotion::Yawn, Emotion::Idle,
    ];

    pub fn label(self) -> &'static str { self.face().stem() }

    pub fn face(self) -> Face {
        match self {
            Emotion::Happy => Face::Happy,
            Emotion::Love => Face::Love,
            Emotion::Shy => Face::Shy,
            Emotion::Surprised => Face::Surprised,
            Emotion::Cry => Face::Cry,
            Emotion::Hiss => Face::Hiss,
            Emotion::Think => Face::Think,
            Emotion::Dizzy => Face::Dizzy,
            Emotion::Stare => Face::Stare,
            Emotion::Yawn => Face::Yawn,
            Emotion::Idle => Face::Idle,
        }
    }

    /// 标签 → 情绪（不分大小写；接受几个常见的近义英文词，模型偶尔不听话）
    pub fn from_label(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        // "emotion: happy" / "mood=happy" 取最后一段
        let s = s.rsplit([':', '=']).next().unwrap_or("").trim();
        if let Some(e) = Self::ALL.iter().find(|e| e.label() == s) {
            return Some(*e);
        }
        Some(match s {
            "joy" | "smile" | "excited" | "glad" => Emotion::Happy,
            "heart" | "loving" | "affection" => Emotion::Love,
            "blush" | "shyness" | "embarrassed" => Emotion::Shy,
            "surprise" | "shock" | "shocked" | "wow" => Emotion::Surprised,
            "sad" | "crying" | "tear" | "tears" => Emotion::Cry,
            "angry" | "mad" | "anger" | "annoyed" => Emotion::Hiss,
            "thinking" | "thoughtful" | "curious" => Emotion::Think,
            "confused" | "giddy" => Emotion::Dizzy,
            "staring" | "speechless" => Emotion::Stare,
            "sleepy" | "tired" | "yawning" => Emotion::Yawn,
            "neutral" | "calm" | "normal" | "none" => Emotion::Idle,
            _ => return None,
        })
    }
}

/// 写进系统提示词的一句
pub const PROMPT: &str = "每条回复的最开头必须先写一个表情标签，只能从这些里选一个：[happy] [love] [shy] [surprised] [cry] [hiss] [think] [dizzy] [stare] [yawn] [idle]（hiss 是生气哈气），例如「[happy]答对啦，快夸我喵！」。标签只写一次，正文里不要再出现方括号标签。";

const OPENS: [char; 3] = ['[', '【', '［'];
const CLOSES: [char; 3] = [']', '】', '］'];
/// 标签里最多多少个字符（再长就不是标签了）
const TAG_MAX: usize = 20;

fn close_of(open: char) -> Option<char> { OPENS.iter().position(|c| *c == open).map(|i| CLOSES[i]) }

/// 看起来像标签：ASCII 字母/空格/_-:= 组成
fn tag_like(inner: &str) -> bool {
    let t = inner.trim();
    !t.is_empty()
        && t.chars().count() <= TAG_MAX
        && t.chars().any(|c| c.is_ascii_alphabetic())
        && t.chars().all(|c| c.is_ascii_alphabetic() || matches!(c, ' ' | '_' | '-' | ':' | '='))
}

/// s 以开括号开头：返回 (标签内文, 闭括号之后的字节位置)；还没闭合返回 None
fn leading_tag(s: &str) -> Option<(&str, usize)> {
    let open = s.chars().next()?;
    let close = close_of(open)?;
    let body = &s[open.len_utf8()..];
    for (n, (i, c)) in body.char_indices().enumerate() {
        if n > TAG_MAX + 1 {
            return None;
        }
        // 允许中英括号混用：[happy】
        if c == close || CLOSES.contains(&c) {
            return Some((&body[..i], open.len_utf8() + i + c.len_utf8()));
        }
    }
    None
}

/// 去掉开头连续的标签（认识的和像标签但不认识的都去掉），返回 (第一个认识的情绪, 剩下的文字, 有没有去掉东西)
fn strip_leading(text: &str) -> (Option<Emotion>, &str, bool) {
    let mut s = text.trim_start();
    let mut emo = None;
    let mut any = false;
    while let Some((inner, end)) = leading_tag(s) {
        if !tag_like(inner) {
            break;
        }
        any = true;
        if emo.is_none() {
            emo = Emotion::from_label(inner);
        }
        s = s[end..].trim_start();
    }
    (emo, s, any)
}

/// 正文中间混进来的「认识的」标签也去掉（不认识的方括号内容可能是正文，留着）
fn strip_inline(text: &str) -> (String, Option<Emotion>) {
    let mut out = String::with_capacity(text.len());
    let mut first = None;
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let c = rest.chars().next().unwrap_or(' ');
        if OPENS.contains(&c)
            && let Some((inner, end)) = leading_tag(rest)
            && tag_like(inner)
            && let Some(e) = Emotion::from_label(inner)
        {
            first.get_or_insert(e);
            i += end;
            continue;
        }
        out.push(c);
        i += c.len_utf8();
    }
    (out, first)
}

/// 解析结果
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tagged {
    /// 标签给的情绪（没有/不认识 = None）
    pub tag: Option<Emotion>,
    /// 去掉标签后的文字
    pub text: String,
}

/// 去标签（不猜）
pub fn split(text: &str) -> Tagged {
    let (tag, rest, _) = strip_leading(text);
    let (clean, inline) = strip_inline(rest);
    let clean = clean.trim().to_string();
    Tagged { tag: tag.or(inline), text: clean }
}

/// 去标签 + 决定情绪：标签 → 关键词 → idle。去完是空的给一个「喵～」
pub fn analyze(text: &str) -> (Emotion, String) {
    let t = split(text);
    let text = if t.text.is_empty() { "喵～".to_string() } else { t.text };
    let e = t.tag.unwrap_or_else(|| guess(&text));
    (e, text)
}

/// 本地关键词猜情绪（按优先级）
pub fn guess(text: &str) -> Emotion {
    const TABLE: &[(Emotion, &[&str])] = &[
        (Emotion::Hiss, &["哼", "生气", "气死", "不许", "坏蛋", "讨厌你", "哈——", "哈—", "凶", "不理你"]),
        (Emotion::Cry, &["呜", "难过", "伤心", "哭", "委屈", "好惨", "心疼", "对不起"]),
        (Emotion::Love, &["喜欢你", "爱你", "抱抱", "贴贴", "亲亲", "mua", "最喜欢", "想你"]),
        (Emotion::Shy, &["害羞", "脸红", "不好意思", "人家", "讨厌啦", "才没有", "羞"]),
        (Emotion::Surprised, &["诶？", "欸？", "哇", "真的吗", "居然", "竟然", "吓", "天哪", "什么？", "！？", "?!"]),
        (Emotion::Dizzy, &["晕", "头好乱", "绕晕", "搞不懂", "好复杂"]),
        (Emotion::Yawn, &["困", "哈欠", "好累", "睡觉", "晚安", "犯困"]),
        // 「可能/也许/大概」在普通回答里太常见，不算
        (Emotion::Think, &["让我想想", "想想", "嗯……", "嗯...", "不确定", "思考", "我猜"]),
        (Emotion::Happy, &["哈哈", "嘿嘿", "开心", "太好了", "好耶", "棒", "厉害", "耶", "嘻嘻", "夸我", "~", "～"]),
        (Emotion::Stare, &["盯", "……", "。。。"]),
    ];
    let lower = text.to_lowercase();
    TABLE
        .iter()
        .find(|(_, keys)| keys.iter().any(|k| lower.contains(k)))
        .map(|(e, _)| *e)
        .unwrap_or(Emotion::Idle)
}

/// 流式回复：一块一块喂进来，吐出可以马上显示/念的文字；开头的标签攒齐了再判断。
// 现在的聊天是整条回复；等聊天改成流式（feat/settings-ui 那一路）时直接用它
#[allow(dead_code)]
#[derive(Debug, Default)]
pub struct TagStream {
    buf: String,
    /// 开头的标签已经判断完了
    decided: bool,
    tag: Option<Emotion>,
}

#[allow(dead_code)]
impl TagStream {
    pub fn new() -> Self { Self::default() }

    /// 标签给的情绪（还没判断完或没有标签 = None）
    pub fn tag(&self) -> Option<Emotion> { self.tag }

    /// 喂一块，返回现在可以放出去的文字（可能是空串）
    pub fn push(&mut self, chunk: &str) -> String {
        self.buf.push_str(chunk);
        if !self.decided && !self.decide(false) {
            return String::new();
        }
        self.drain(false)
    }

    /// 流结束：把剩下的全放出来
    pub fn finish(&mut self) -> String {
        if !self.decided {
            self.decide(true);
        }
        self.drain(true)
    }

    /// 处理开头。返回 true = 判断完了
    fn decide(&mut self, end: bool) -> bool {
        loop {
            let t = self.buf.trim_start();
            let Some(c) = t.chars().next() else { return end };
            if !OPENS.contains(&c) {
                break;
            }
            match leading_tag(t) {
                Some((inner, endpos)) if tag_like(inner) => {
                    if self.tag.is_none() {
                        self.tag = Emotion::from_label(inner);
                    }
                    self.buf = t[endpos..].to_string();
                }
                Some(_) => break,
                None => {
                    // 还没闭合：短的话再等等；流结束了还没闭合，像标签就扔掉
                    let body = &t[c.len_utf8()..];
                    let short = body.chars().count() <= TAG_MAX;
                    let maybe = body.trim().is_empty() || tag_like(body);
                    if short && maybe {
                        if end {
                            self.buf.clear();
                            break;
                        }
                        return false;
                    }
                    break;
                }
            }
        }
        self.buf = self.buf.trim_start().to_string();
        self.decided = true;
        true
    }

    /// 放出文字：结尾可能是半截标签的那段先扣着
    fn drain(&mut self, end: bool) -> String {
        let keep_from = if end { self.buf.len() } else { partial_tail(&self.buf) };
        let out: String = self.buf[..keep_from].to_string();
        self.buf.drain(..keep_from);
        let (clean, inline) = strip_inline(&out);
        if self.tag.is_none() {
            self.tag = inline;
        }
        clean
    }
}

/// 结尾有没闭合、像标签的「[hap」：返回它的起点（没有 = 全长）
#[allow(dead_code)]
fn partial_tail(s: &str) -> usize {
    if let Some((i, c)) = s.char_indices().rev().find(|(_, c)| OPENS.contains(c) || CLOSES.contains(c)) {
        if CLOSES.contains(&c) {
            return s.len();
        }
        let body = &s[i + c.len_utf8()..];
        if body.chars().count() <= TAG_MAX && (body.is_empty() || tag_like(body)) {
            return i;
        }
    }
    s.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(chunks: &[&str]) -> (Option<Emotion>, String) {
        let mut s = TagStream::new();
        let mut out = String::new();
        for c in chunks {
            out.push_str(&s.push(c));
        }
        out.push_str(&s.finish());
        (s.tag(), out)
    }

    #[test]
    fn basic_tags() {
        assert_eq!(analyze("[happy]答对啦，快夸我喵！"), (Emotion::Happy, "答对啦，快夸我喵！".into()));
        assert_eq!(analyze("  [shy] 才、才没有喵"), (Emotion::Shy, "才、才没有喵".into()));
        for e in Emotion::ALL {
            assert_eq!(split(&format!("[{}]喵", e.label())).tag, Some(e));
        }
    }

    #[test]
    fn case_and_brackets() {
        assert_eq!(split("[HAPPY]好耶").tag, Some(Emotion::Happy));
        assert_eq!(split("[Hiss]哼").tag, Some(Emotion::Hiss));
        assert_eq!(split("【cry】呜呜").tag, Some(Emotion::Cry));
        assert_eq!(split("【cry】呜呜").text, "呜呜");
        assert_eq!(split("［love］最喜欢你了").tag, Some(Emotion::Love));
        assert_eq!(split("[ think ]嗯").tag, Some(Emotion::Think));
        assert_eq!(split("[happy】混着用").text, "混着用");
        assert_eq!(split("[emotion: surprised]诶？").tag, Some(Emotion::Surprised));
        assert_eq!(split("[angry]不理你了").tag, Some(Emotion::Hiss));
    }

    #[test]
    fn unknown_or_missing_tag_falls_back() {
        // 不认识的标签也要去掉（不能念出来），然后按关键词猜
        let (e, t) = analyze("[banana]哈哈哈，好开心喵");
        assert_eq!(t, "哈哈哈，好开心喵");
        assert_eq!(e, Emotion::Happy);
        assert_eq!(analyze("呜呜，被骂了喵"), (Emotion::Cry, "呜呜，被骂了喵".into()));
        assert_eq!(analyze("今天是星期五喵").0, Emotion::Idle);
        assert_eq!(analyze("[xyz]今天是星期五").0, Emotion::Idle);
        // 正文里的中文方括号内容不是标签，留着
        assert_eq!(split("书名是【三体】喵").text, "书名是【三体】喵");
        assert_eq!(split("[1] 第一条").text, "[1] 第一条");
        // 只有标签
        assert_eq!(analyze("[happy]"), (Emotion::Happy, "喵～".into()));
    }

    #[test]
    fn inline_and_multiple_tags() {
        let t = split("[happy][love]好喵");
        assert_eq!(t.tag, Some(Emotion::Happy));
        assert_eq!(t.text, "好喵");
        let t = split("好的喵[shy]");
        assert_eq!(t.tag, Some(Emotion::Shy));
        assert_eq!(t.text, "好的喵");
        let t = split("答案是 42 [Happy] 对吧");
        assert_eq!(t.text, "答案是 42  对吧");
    }

    #[test]
    fn guess_keywords() {
        assert_eq!(guess("哼，不许这样"), Emotion::Hiss);
        assert_eq!(guess("诶？真的吗"), Emotion::Surprised);
        assert_eq!(guess("好困，想睡觉了"), Emotion::Yawn);
        assert_eq!(guess("让我想想喵"), Emotion::Think);
        assert_eq!(guess("最喜欢你啦"), Emotion::Love);
        assert_eq!(guess("人家才没有脸红"), Emotion::Shy);
        assert_eq!(guess("头好晕"), Emotion::Dizzy);
    }

    #[test]
    fn stream_split_tag() {
        assert_eq!(stream(&["[ha", "ppy]答对", "啦喵"]), (Some(Emotion::Happy), "答对啦喵".into()));
        assert_eq!(stream(&["[", "h", "a", "p", "p", "y", "]", "好", "耶"]), (Some(Emotion::Happy), "好耶".into()));
        assert_eq!(stream(&["【", "Cry", "】呜"]), (Some(Emotion::Cry), "呜".into()));
        assert_eq!(stream(&[" ", " [SHY", "] 才没有"]), (Some(Emotion::Shy), "才没有".into()));
        assert_eq!(stream(&["没有标签", "的回复"]), (None, "没有标签的回复".into()));
        assert_eq!(stream(&["[banana]", "哈哈"]), (None, "哈哈".into()));
        assert_eq!(stream(&["【三体】", "是本书"]), (None, "【三体】是本书".into()));
        // 结尾的认识标签被拆开
        assert_eq!(stream(&["好的喵[sh", "y]"]), (Some(Emotion::Shy), "好的喵".into()));
        // 流断在半截标签上：扔掉
        assert_eq!(stream(&["[hap"]), (None, "".into()));
    }

    #[test]
    fn stream_emits_early() {
        let mut s = TagStream::new();
        assert_eq!(s.push("[lo"), "");
        assert_eq!(s.push("ve]最"), "最");
        assert_eq!(s.tag(), Some(Emotion::Love));
        assert_eq!(s.push("喜欢"), "喜欢");
        // 没标签的直接放
        let mut s = TagStream::new();
        assert_eq!(s.push("你好"), "你好");
        // 开头半截括号后面跟着中文 = 不是标签
        let mut s = TagStream::new();
        assert_eq!(s.push("【注意"), "【注意");
        assert_eq!(s.push("】别迟到"), "】别迟到");
    }
}
