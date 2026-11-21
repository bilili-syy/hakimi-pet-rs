//! 长期记忆的「脑子」部分：聊天里的「记住…/忘掉…/你记得我什么」识别、后台总结新事实、主动关心的一句话。
//! 数据在 core::memory；用户设置的同一个模型（services::ai::ask_once）。全部同步代码 + 后台线程，不用 async。
//! 记忆内容和 Key 都不写日志。
use crate::core::memo::{Dt, Now};
use crate::core::memory::{self as store, Book, Fact, Kind};
use crate::types::{AppEvent, EventSender, MindEv};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};

/// 聊天里对记忆的指令
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cmd {
    Remember(Fact),
    /// sure = 明确是「忘掉」指令（对不上也要回一句）；false = 「忘记…」开头，可能只是普通聊天
    Forget { query: String, sure: bool },
    /// 想全部忘掉：让主人去菜单里清空（要二次确认）
    ForgetAll,
    Recall,
}

const POLITE: [&str; 8] = ["请你", "麻烦你", "你要", "你得", "帮我", "请", "你", "给我"];

fn strip_polite(t: &str) -> &str {
    let mut t = t.trim();
    for _ in 0..2 {
        for p in POLITE {
            if let Some(r) = t.strip_prefix(p) {
                t = r.trim_start();
            }
        }
    }
    t
}

fn trim_punct(t: &str) -> &str {
    t.trim_matches(|c: char| c.is_whitespace() || "：:，,。.！!？?～~、；;\"“”「」".contains(c))
}

/// 「我」→「主人」，记忆里统一用第三人称
pub fn third_person(t: &str) -> String {
    let t = trim_punct(t);
    let t = t.strip_prefix("我的").map(|r| format!("主人的{r}")).unwrap_or_else(|| t.to_string());
    let t = match t.strip_prefix('我') {
        Some(r) if !r.starts_with('们') => format!("主人{r}"),
        _ => t,
    };
    t.replace("我们", "\u{0}").replace('我', "主人").replace('\u{0}', "我们")
}

/// 主人亲口说的事实 → 猜个类别
pub fn classify(text: &str) -> Fact {
    let t = third_person(text);
    let has = |ks: &[&str]| ks.iter().any(|k| t.contains(k));
    // 「叫我小明」「主人叫小明」「我的名字是…」
    let raw = trim_punct(text);
    for p in ["叫我", "称呼我", "喊我"] {
        if let Some(n) = raw.split(p).nth(1).map(trim_punct).filter(|n| !n.is_empty()) {
            let n = trim_punct(n.trim_end_matches('吧').trim_end_matches("就好").trim_end_matches("就行").trim_end_matches("好了"));
            if !n.is_empty() {
                return Fact::new(Kind::Name, &format!("主人希望被叫作「{n}」"));
            }
        }
    }
    let kind = if has(&["名字", "主人叫"]) {
        Kind::Name
    } else if has(&["生日"]) {
        Kind::Birthday
    } else if has(&["不喜欢", "讨厌", "不吃", "不爱", "过敏", "怕"]) {
        Kind::Dislike
    } else if has(&["喜欢", "爱吃", "爱喝", "爱看", "爱玩", "最爱"]) {
        Kind::Like
    } else if has(&["在忙", "最近", "准备", "考试", "项目", "加班", "复习", "面试", "论文", "搬家", "在学"]) {
        Kind::Busy
    } else if has(&["明天", "后天", "下周", "周一", "周二", "周三", "周四", "周五", "周六", "周日", "号", "月"]) {
        Kind::Event
    } else {
        Kind::Note
    };
    Fact::new(kind, &t)
}

/// 识别聊天里的记忆指令。普通聊天返回 None
pub fn parse_command(text: &str) -> Option<Cmd> {
    let t = trim_punct(text);
    if t.is_empty() {
        return None;
    }
    // 你记得我什么 / 你都记得我哪些事 / 你对我了解多少 / 关于我你知道什么
    let recall = ["记得我什么", "记得我哪些", "记得关于我", "记住了我什么", "记住我什么", "对我了解多少", "关于我你知道", "关于我你记得", "你都记得什么", "你记住了什么", "你还记得我吗", "你记得我吗"];
    if recall.iter().any(|k| t.contains(k)) {
        return Some(Cmd::Recall);
    }
    let body = strip_polite(t);
    // 「提醒我/记一下」是备忘录的事
    if body.contains("提醒") || body.starts_with("记一下") || body.starts_with("记下") {
        return None;
    }
    // 记住…
    for p in ["记住", "记好", "记牢", "要记得", "记得"] {
        if let Some(rest) = body.strip_prefix(p) {
            // 「记得吗」「记得我吗」是在问
            let r = trim_punct(rest);
            let asking = text.trim_end().ends_with(['?', '？', '吗', '么', '呢']) || r.contains("什么") || r.contains('哪');
            if r.is_empty() || asking {
                return None;
            }
            let r = r.trim_start_matches('了');
            let r = trim_punct(r.strip_prefix("这件事").or_else(|| r.strip_prefix("这个")).unwrap_or(r));
            return (!r.is_empty()).then(|| Cmd::Remember(classify(r)));
        }
    }
    // 忘掉…
    let all = ["全部", "所有", "一切", "全都", "所有的事", "所有记忆", "全部记忆", "关于我的一切", "关于我的所有事"];
    let forget_sure = ["忘掉", "忘了吧", "别记着", "别记", "不要记得", "不要记住", "不用记得", "删掉记忆里"];
    for p in forget_sure.iter().chain(["忘记"].iter()) {
        if let Some(rest) = body.strip_prefix(p) {
            let r = trim_punct(rest);
            let r = trim_punct(r.trim_end_matches("这件事").trim_end_matches("的事").trim_end_matches('吧'));
            if r.is_empty() {
                return None;
            }
            if all.iter().any(|a| r == *a || r.starts_with(a)) {
                return Some(Cmd::ForgetAll);
            }
            return Some(Cmd::Forget { query: forget_query(r), sure: *p != "忘记" });
        }
    }
    // 把…忘掉/忘了/忘记
    if let Some(rest) = body.strip_prefix('把') {
        for s in ["忘掉吧", "忘掉", "忘了吧", "忘了", "忘记吧", "忘记", "删掉", "删了"] {
            if let Some(q) = rest.strip_suffix(s) {
                let q = trim_punct(q.trim_end_matches("给").trim_end_matches("都"));
                let q = trim_punct(q.trim_end_matches("这件事").trim_end_matches("的事"));
                if q.is_empty() {
                    return None;
                }
                if all.iter().any(|a| q == *a || q.starts_with(a)) {
                    return Some(Cmd::ForgetAll);
                }
                return Some(Cmd::Forget { query: forget_query(q), sure: true });
            }
        }
    }
    None
}

/// 「我喜欢草莓」→「喜欢草莓」（比较时主语会被去掉，这里顺手去一下，回话更自然）
fn forget_query(q: &str) -> String {
    let q = trim_punct(q);
    let q = q.strip_prefix("我").unwrap_or(q);
    trim_punct(q).to_string()
}

// ---------------------------------------------------------------- 台词

pub fn remember_line(f: &Fact) -> String {
    let t = f.text.replace("主人", "你");
    match f.kind {
        Kind::Name => format!("好喵！{}，哈基米记住啦～", t.replace("你希望被叫作", "以后就叫你")),
        Kind::Birthday => format!("记住啦：{t}！到时候要给你唱生日歌喵～"),
        _ => format!("好喵，记住啦：{t}"),
    }
}

pub fn forget_line(gone: &[String], query: &str) -> String {
    if gone.is_empty() {
        return format!("哈基米本来就没记着「{query}」喵……不过以后也不会去记它啦");
    }
    let n = gone.len();
    if n == 1 {
        format!("好……「{}」哈基米已经忘掉了喵", gone[0].replace("主人", "你"))
    } else {
        format!("好……和「{query}」有关的 {n} 件事都忘掉了喵")
    }
}

pub const FORGET_ALL_LINE: &str = "要哈基米全部忘掉？呜……那你右键我「她的记忆」→「清空记忆」，再确认一次才行喵";

pub fn recall_line(items: &[String], total: usize) -> String {
    if items.is_empty() {
        return "哈基米还没记住你什么呢……多跟我说说你的事嘛，喵～".into();
    }
    let list = items.iter().map(|s| s.replace("主人", "你")).collect::<Vec<_>>().join("；");
    if total > items.len() {
        format!("哈基米记得：{list}……还有 {} 件小事，都偷偷记着喵！", total - items.len())
    } else {
        format!("哈基米记得：{list}。都记着呢喵！")
    }
}

// ---------------------------------------------------------------- 后台提取

/// 同一时间只跑一个提取 / 一个关心请求
static EXTRACTING: AtomicBool = AtomicBool::new(false);
static CARING: AtomicBool = AtomicBool::new(false);

/// 提取用的提示词（附上已有的记忆，让模型只输出新的或有变化的）
pub fn extract_prompt(now: Dt, known: &Book) -> String {
    let mut known_txt: String = known.ranked().iter().take(40).map(|i| format!("\n- {}", i.text)).collect();
    if known_txt.is_empty() {
        known_txt = "\n（还没有）".into();
    }
    format!(
        "你是桌宠「哈基米」的记忆整理助手。现在是 {}。下面会给你一段用户和哈基米的最新对话。\n\
         请找出值得长期记住的、关于用户本人的新事实：称呼、生日、喜好、讨厌的东西、最近在忙的事、说过的重要安排或经历。\n\
         只记关于用户的事，不记哈基米说的话、不记闲聊和一次性的问题；用第三人称「主人」写，每条不超过 30 字；带上具体日期（把「明天」「下周三」换算成几月几日）。\n\
         已经记住的：{known_txt}\n\
         已经记住且没变化的不要重复输出；有变化的输出新的版本（同一主题用同一个 key）。\n\
         只输出 JSON 数组，不要别的文字，没有就输出 []。格式：\
         [{{\"kind\":\"name|birthday|like|dislike|busy|event|note\",\"text\":\"主人……\",\"key\":\"主题词\"}}]",
        now.fmt()
    )
}

/// 模型明确表示「没有新事实」的常见纯文本回复（避免因不是 JSON 数组而反复重试）
fn is_explicit_empty_reply(s: &str) -> bool {
    let clean = s
        .trim()
        .trim_matches(|c: char| matches!(c, '`' | '"' | '\'' | '。' | '.' | '!' | '！' | '~' | '～'))
        .trim();
    matches!(
        clean.to_ascii_lowercase().as_str(),
        "none" | "null" | "无" | "没有" | "没有新的" | "没有新事实" | "没有新信息" | "暂无" | "无新事实" | "无新信息"
    )
}

/// 解析模型给的 JSON（容忍 ```json 包裹、前后废话）；区分「合法空结果」与「格式损坏/截断」
pub fn parse_facts_strict(reply: &str) -> Result<Vec<Fact>, ()> {
    let s = reply.trim();
    // 尝试找一段合法 JSON 数组，避免正文前面有普通 `[...]` 导致外层切片解析失败
    let mut parsed: Option<Vec<Value>> = None;
    for (a, _) in s.match_indices('[') {
        for (b, _) in s.rmatch_indices(']') {
            if b <= a { break; }
            if let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(&s[a..=b]) {
                parsed = Some(arr);
                break;
            }
        }
        if parsed.is_some() { break; }
    }
    let Some(arr) = parsed else {
        return if is_explicit_empty_reply(s) { Ok(Vec::new()) } else { Err(()) };
    };
    Ok(arr
        .iter()
        .filter_map(|v| {
            let text = v.get("text").and_then(Value::as_str)?.trim();
            if text.is_empty() {
                return None;
            }
            let kind = Kind::parse(v.get("kind").and_then(Value::as_str).unwrap_or("note"));
            let key = v.get("key").and_then(Value::as_str).unwrap_or("").trim();
            Some(Fact { kind, text: text.to_string(), key: key.chars().take(12).collect() })
        })
        .take(10)
        .collect())
}

/// 解析模型给的 JSON（兼容旧签名，测试用）
#[cfg(test)]
pub fn parse_facts(reply: &str) -> Vec<Fact> {
    parse_facts_strict(reply).unwrap_or_default()
}

/// chat.json 里 t > since 的消息拼成对话文本（去掉情绪标签）；返回 (文本, 最新的 t)
pub fn dialogue_since(history: &[Value], since: u64) -> (String, u64) {
    let mut out = String::new();
    let mut last = since;
    for m in history {
        let t = m.get("t").and_then(Value::as_u64).unwrap_or(0);
        if t <= since {
            continue;
        }
        last = last.max(t);
        let text = m.get("text").and_then(Value::as_str).unwrap_or("");
        let user = m.get("role").and_then(Value::as_str) == Some("user");
        let text = if user { text.to_string() } else { super::emotion::split(text).text };
        out.push_str(if user { "用户：" } else { "哈基米：" });
        out.push_str(&text.chars().take(300).collect::<String>());
        out.push('\n');
    }
    (out, last)
}

/// 后台总结一次新事实并合并进 memory.json（已经有一个在跑就跳过）
pub fn extract_in_background() {
    if EXTRACTING.swap(true, Ordering::SeqCst) {
        return;
    }
    let ok = std::thread::Builder::new().name("hakimi-mind".into()).spawn(|| {
        extract_blocking();
        EXTRACTING.store(false, Ordering::SeqCst);
    });
    if ok.is_err() {
        EXTRACTING.store(false, Ordering::SeqCst);
    }
}

const MAX_PARSE_FAILS: u32 = 3;
static EXTRACT_PARSE_FAILS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn record_parse_fail(counter: &std::sync::atomic::AtomicU32) -> bool {
    let n = counter.fetch_add(1, Ordering::Relaxed) + 1;
    if n >= MAX_PARSE_FAILS {
        counter.store(0, Ordering::Relaxed);
        true
    } else {
        false
    }
}

fn extract_blocking() {
    if crate::core::config::ai_key_missing(&crate::core::config::get()) {
        return;
    }
    let clear_gen = store::clear_gen();
    let book = store::snapshot();
    let history = crate::services::ai::load_history();
    let (dialogue, last) = dialogue_since(&history, book.extracted_at);
    if dialogue.trim().is_empty() {
        return;
    }
    match crate::services::ai::ask_once_max(&extract_prompt(Now::get().dt, &book), &dialogue, 1024) {
        Ok(r) => match parse_facts_strict(&r) {
            Ok(facts) => {
                EXTRACT_PARSE_FAILS.store(0, Ordering::Relaxed);
                let n = store::merge_ai_if(&facts, last, clear_gen);
                crate::core::log::log(&format!("mind: extracted {} facts, {n} changed", facts.len()));
            }
            Err(()) => {
                // 偶发截断先重试；连续失败 3 次则推进游标，防止不按 JSON 格式回复的模型无限重发
                if record_parse_fail(&EXTRACT_PARSE_FAILS) {
                    let _ = store::merge_ai_if(&[], last, clear_gen);
                    crate::core::log::log("mind: 3 consecutive invalid extract replies, advancing cursor");
                } else {
                    crate::core::log::log("mind: extract reply not valid json array, will retry");
                }
            }
        },
        // 只记一句，不带服务器返回的内容（可能回显提示词里的记忆）
        Err(_) => crate::core::log::log("mind: extract request failed"),
    }
}

// ---------------------------------------------------------------- 主动关心

pub fn care_prompt(now: Dt, book: &Book) -> String {
    let list: String = book.ranked().iter().take(30).map(|i| format!("\n- {}（记于 {}）", i.text, i.updated)).collect();
    format!(
        "你是桌宠猫娘「哈基米」，自称哈基米，句尾常带「喵」，简体中文口语。现在是 {}（{}）。你记得这些关于主人的事：{list}\n\
         从里面挑一件此刻最适合主动关心的事（比如今天/最近的考试、面试、安排、生日、在忙的事），对主人说一句关心的话，不超过 30 个字，不用 Markdown。\n\
         回复开头加一个表情标签：[happy] [love] [shy] [surprised] [cry] [think] [stare] [idle] 之一。\n\
         如果没有哪件适合现在提（都太久远或已经过去很久），只输出 NONE。",
        now.fmt(),
        crate::core::memo::weekday_name(now.dow())
    )
}

/// 记忆里有没有值得关心的（避免没东西还白问一次模型）
pub fn has_care_topic(book: &Book) -> bool {
    book.items.iter().any(|i| matches!(i.kind, Kind::Busy | Kind::Event | Kind::Birthday))
}

/// 后台问一句关心的话；有就发 AppEvent::Mind(MindEv::Care(text))（text 带情绪标签，行为层再去掉）
pub fn care_in_background(tx: &EventSender) -> bool {
    if CARING.swap(true, Ordering::SeqCst) {
        return false;
    }
    let tx = tx.clone();
    let ok = std::thread::Builder::new().name("hakimi-care".into()).spawn(move || {
        let book = store::snapshot();
        let line = if crate::core::config::ai_key_missing(&crate::core::config::get()) || !has_care_topic(&book) {
            None
        } else {
            match crate::services::ai::ask_once(&care_prompt(Now::get().dt, &book), "说一句吧") {
                Ok(r) => care_reply(&r),
                Err(_) => {
                    crate::core::log::log("mind: care request failed");
                    None
                }
            }
        };
        CARING.store(false, Ordering::SeqCst);
        tx.send(AppEvent::Mind(MindEv::Care(line)));
    });
    if ok.is_err() {
        CARING.store(false, Ordering::SeqCst);
        return false;
    }
    true
}

/// 模型的关心回复：NONE / 空 / 太长 → None
pub fn care_reply(r: &str) -> Option<String> {
    let t = r.trim().trim_matches('"').trim();
    let clean = super::emotion::split(t).text;
    if clean.is_empty() || clean.to_ascii_uppercase().starts_with("NONE") || clean.chars().count() > 80 {
        return None;
    }
    Some(t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rem(t: &str) -> Fact {
        match parse_command(t) {
            Some(Cmd::Remember(f)) => f,
            other => panic!("{t}: {other:?}"),
        }
    }

    #[test]
    fn remember_commands() {
        let f = rem("记住：我下周三考高数");
        assert_eq!(f.text, "主人下周三考高数");
        assert_eq!(f.kind, Kind::Event);
        assert_eq!(rem("帮我记住，我喜欢吃草莓").kind, Kind::Like);
        assert_eq!(rem("记住我喜欢吃草莓").text, "主人喜欢吃草莓");
        assert_eq!(rem("请记住我不吃香菜").kind, Kind::Dislike);
        assert_eq!(rem("你要记得我的生日是10月10日").kind, Kind::Birthday);
        assert_eq!(rem("你要记得我的生日是10月10日").text, "主人的生日是10月10日");
        let n = rem("记住，以后叫我小明就好");
        assert_eq!(n.kind, Kind::Name);
        assert_eq!(n.text, "主人希望被叫作「小明」");
        assert_eq!(rem("记住我最近在准备考研").kind, Kind::Busy);
        assert_eq!(rem("记住我们下周去爬山").text, "我们下周去爬山");
    }

    #[test]
    fn not_commands() {
        for t in ["记得我喜欢什么？", "记得吗", "你记得我吗？", "提醒我 3 点开会", "记一下：买猫粮", "记住了吗", "今天好累", "我忘记带钥匙了", "你好"] {
            assert!(matches!(parse_command(t), None | Some(Cmd::Recall)), "{t}: {:?}", parse_command(t));
        }
        assert_eq!(parse_command("你记得我吗？"), Some(Cmd::Recall));
        assert_eq!(parse_command("记住了吗"), None);
    }

    #[test]
    fn forget_and_recall_commands() {
        assert_eq!(parse_command("忘掉我喜欢草莓这件事"), Some(Cmd::Forget { query: "喜欢草莓".into(), sure: true }));
        assert_eq!(parse_command("把考研的事忘了吧"), Some(Cmd::Forget { query: "考研".into(), sure: true }));
        assert_eq!(parse_command("忘记草莓"), Some(Cmd::Forget { query: "草莓".into(), sure: false }));
        assert_eq!(parse_command("别记着我生日了"), Some(Cmd::Forget { query: "生日了".into(), sure: true }));
        assert_eq!(parse_command("忘掉所有的事"), Some(Cmd::ForgetAll));
        assert_eq!(parse_command("把关于我的一切都忘掉"), Some(Cmd::ForgetAll));
        assert_eq!(parse_command("你记得我什么"), Some(Cmd::Recall));
        assert_eq!(parse_command("你都记得我哪些事？"), Some(Cmd::Recall));
        assert_eq!(parse_command("关于我你知道什么"), Some(Cmd::Recall));
    }

    #[test]
    fn third_person_rules() {
        assert_eq!(third_person("我喜欢猫"), "主人喜欢猫");
        assert_eq!(third_person("我的猫叫咪咪"), "主人的猫叫咪咪");
        assert_eq!(third_person("妈妈说我胖了"), "妈妈说主人胖了");
        assert_eq!(third_person("我们周末去露营"), "我们周末去露营");
    }

    #[test]
    fn lines() {
        assert_eq!(remember_line(&Fact::new(Kind::Like, "主人喜欢草莓")), "好喵，记住啦：你喜欢草莓");
        assert!(remember_line(&Fact::new(Kind::Name, "主人希望被叫作「小明」")).contains("以后就叫你「小明」"));
        assert!(forget_line(&[], "草莓").contains("本来就没记着「草莓」"));
        assert_eq!(forget_line(&["主人喜欢草莓".into()], "草莓"), "好……「你喜欢草莓」哈基米已经忘掉了喵");
        assert!(recall_line(&[], 0).contains("还没记住"));
        let l = recall_line(&["主人叫小明".into(), "主人喜欢草莓".into()], 5);
        assert_eq!(l, "哈基米记得：你叫小明；你喜欢草莓……还有 3 件小事，都偷偷记着喵！");
    }

    #[test]
    fn facts_from_model() {
        let r = "```json\n[{\"kind\":\"busy\",\"text\":\"主人在准备11月25日的高数考试\",\"key\":\"考试\"},{\"kind\":\"???\",\"text\":\"主人养了一只橘猫\"},{\"text\":\"\"}]\n```";
        let f = parse_facts(r);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].kind, Kind::Busy);
        assert_eq!(f[0].key, "考试");
        assert_eq!(f[1].kind, Kind::Note);
        assert!(parse_facts("[]").is_empty());
        assert!(parse_facts("没有新的").is_empty());
        assert!(parse_facts("[oops").is_empty());
        // parse_facts_strict：空数组和明确「没有新的/NONE」算合法空结果；截断/损坏算 Err
        assert_eq!(parse_facts_strict("[]"), Ok(vec![]));
        assert_eq!(parse_facts_strict("没有新的。"), Ok(vec![]));
        assert_eq!(parse_facts_strict("NONE"), Ok(vec![]));
        assert!(parse_facts_strict("[oops").is_err());
        assert!(parse_facts_strict("今天天气不错").is_err());
        // 连续 3 次解析失败才触发推进游标
        let c = std::sync::atomic::AtomicU32::new(0);
        assert!(!record_parse_fail(&c));
        assert!(!record_parse_fail(&c));
        assert!(record_parse_fail(&c));
        assert!(!record_parse_fail(&c));
        let with_prefix = "[note] 以下是结果：\n[{\"kind\":\"like\",\"text\":\"主人喜欢喝拿铁\"}]";
        let ok = parse_facts_strict(with_prefix).unwrap();
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].text, "主人喜欢喝拿铁");
    }

    #[test]
    fn dialogue_window() {
        let h = vec![
            json!({"role":"user","text":"旧的","t":5}),
            json!({"role":"user","text":"我下周考试","t":10}),
            json!({"role":"model","text":"[think]要加油喵","t":11}),
        ];
        let (d, last) = dialogue_since(&h, 5);
        assert_eq!(d, "用户：我下周考试\n哈基米：要加油喵\n");
        assert_eq!(last, 11);
        assert_eq!(dialogue_since(&h, 11), (String::new(), 11));
    }

    #[test]
    fn care_replies() {
        assert_eq!(care_reply("[think]今天考试考得怎么样喵？").as_deref(), Some("[think]今天考试考得怎么样喵？"));
        assert_eq!(care_reply("NONE"), None);
        assert_eq!(care_reply("[idle]none"), None);
        assert_eq!(care_reply("  "), None);
        let mut b = Book::default();
        assert!(!has_care_topic(&b));
        b.merge(&[Fact::new(Kind::Busy, "主人在准备考研")], Dt::new(2026, 11, 1, 9, 0).unwrap(), true);
        assert!(has_care_topic(&b));
        let p = care_prompt(Dt::new(2026, 11, 20, 9, 0).unwrap(), &b);
        assert!(p.contains("主人在准备考研（记于 2026-11-01 09:00）"));
        assert!(p.contains("NONE"));
        let e = extract_prompt(Dt::new(2026, 11, 20, 9, 0).unwrap(), &b);
        assert!(e.contains("- 主人在准备考研"));
    }
}
