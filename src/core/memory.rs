//! 长期记忆：%APPDATA%\hakimi-pet\memory.json，存「关于主人的事」（称呼、生日、喜好、最近在忙的事、说过的重要事）。
//! 每条带时间；合并时去重、新的覆盖旧的，最多 MAX_ITEMS 条。写盘原子替换，不认识的字段原样保留。
//! 全局 Mutex，任何线程可调（后台提取线程也会写）。记忆内容不写日志。
use crate::core::memo::{Dt, Now};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::sync::Mutex;

pub const MAX_ITEMS: usize = 60;
/// 放进系统提示词的记忆最多多少字
pub const PROMPT_CHARS: usize = 600;
/// 一条记忆最多多少字
pub const ITEM_CHARS: usize = 60;
/// 「忘掉」过的东西记多少条（防止后台提取又从聊天记录里把它捡回来）
const MAX_FORGOTTEN: usize = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// 怎么称呼主人（只留一条）
    Name,
    /// 生日（只留一条）
    Birthday,
    Like,
    Dislike,
    /// 最近在忙的事（考试、项目、搬家……）
    Busy,
    /// 说过的重要事（某天要做什么、发生了什么）
    Event,
    #[default]
    #[serde(other)]
    Note,
}

impl Kind {
    pub fn parse(s: &str) -> Kind {
        serde_json::from_value(Value::String(s.trim().to_ascii_lowercase())).unwrap_or_default()
    }
    /// 这一类只留一条（新的直接替换旧的）
    pub fn single(self) -> bool { matches!(self, Kind::Name | Kind::Birthday) }
}

/// 一条新事实（还没合并进去）
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fact {
    pub kind: Kind,
    pub text: String,
    /// 主题词（同一主题的新事实覆盖旧的，比如「考试」）
    pub key: String,
}

impl Fact {
    pub fn new(kind: Kind, text: &str) -> Self { Self { kind, text: text.into(), key: String::new() } }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: u64,
    #[serde(default)]
    pub kind: Kind,
    pub text: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
    /// 第一次记下的本地时间 "YYYY-MM-DD HH:MM"
    #[serde(default)]
    pub created: String,
    /// 最后一次更新的本地时间
    #[serde(default)]
    pub updated: String,
    /// "user" = 主人亲口让记的；"ai" = 聊天里总结出来的
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 主动关心的计数（一天最多 2 次）
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Care {
    #[serde(default)]
    pub day: String,
    #[serde(default)]
    pub count: u32,
    /// 上一次的本地时间 "YYYY-MM-DD HH:MM"
    #[serde(default)]
    pub last: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Book {
    pub items: Vec<Item>,
    /// 解析不了的条目，原样写回
    pub bad: Vec<Value>,
    pub care: Care,
    /// 后台提取已经看到聊天记录的哪一刻（chat.json 里消息的 t，毫秒）
    pub extracted_at: u64,
    /// 忘掉过的内容（归一化后）
    pub forgotten: Vec<String>,
    /// 文件顶层不认识的字段
    pub extra: Map<String, Value>,
}

/// 归一化：去空白标点、小写、把「主人/我/用户」去掉，用来比较是不是同一件事
pub fn norm(s: &str) -> String {
    let mut t = s.to_lowercase();
    for w in ["主人", "用户", "我"] {
        t = t.replace(w, "");
    }
    t.chars().filter(|c| c.is_alphanumeric()).collect()
}

fn clip(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= n { s.to_string() } else { s.chars().take(n).collect() }
}

impl Item {
    fn same_as(&self, f: &Fact) -> bool {
        if f.kind.single() && self.kind == f.kind {
            return true;
        }
        if !f.key.is_empty() && !self.key.is_empty() && f.key.trim().eq_ignore_ascii_case(self.key.trim()) {
            return true;
        }
        let (a, b) = (norm(&self.text), norm(&f.text));
        if a.is_empty() || b.is_empty() {
            return false;
        }
        a == b || (a.chars().count() >= 4 && b.chars().count() >= 4 && (a.contains(&b) || b.contains(&a)))
    }
    fn pinned(&self) -> bool { self.kind.single() }
}

impl Book {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim_start_matches('\u{feff}').trim();
        if text.is_empty() {
            return Some(Self::default());
        }
        let v: Value = serde_json::from_str(text).ok()?;
        let (arr, mut o) = match v {
            Value::Array(a) => (a, Map::new()),
            Value::Object(mut o) => {
                let a = match o.remove("items") { Some(Value::Array(a)) => a, Some(_) => return None, None => vec![] };
                (a, o)
            }
            _ => return None,
        };
        o.remove("version");
        let care = o.remove("care").and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
        let extracted_at = o.remove("extractedAt").and_then(|v| v.as_u64()).unwrap_or(0);
        let forgotten = o
            .remove("forgotten")
            .and_then(|v| serde_json::from_value::<Vec<String>>(v).ok())
            .unwrap_or_default();
        let mut b = Self { care, extracted_at, forgotten, extra: o, ..Self::default() };
        for v in arr {
            match serde_json::from_value::<Item>(v.clone()) {
                Ok(i) if !i.text.trim().is_empty() => b.items.push(i),
                _ => b.bad.push(v),
            }
        }
        Some(b)
    }

    pub fn to_json(&self) -> String {
        let mut o = Map::new();
        o.insert("version".into(), Value::from(1));
        let mut items: Vec<Value> = self.items.iter().filter_map(|i| serde_json::to_value(i).ok()).collect();
        items.extend(self.bad.iter().cloned());
        o.insert("items".into(), Value::Array(items));
        if let Ok(c) = serde_json::to_value(&self.care) {
            o.insert("care".into(), c);
        }
        o.insert("extractedAt".into(), Value::from(self.extracted_at));
        o.insert("forgotten".into(), Value::from(self.forgotten.clone()));
        for (k, v) in &self.extra {
            o.entry(k.clone()).or_insert_with(|| v.clone());
        }
        serde_json::to_string_pretty(&Value::Object(o)).unwrap_or_else(|_| "{}".into())
    }

    fn next_id(&self) -> u64 {
        let bad_max = self.bad.iter().filter_map(|v| v.get("id").and_then(Value::as_u64)).max().unwrap_or(0);
        self.items.iter().map(|i| i.id).max().unwrap_or(0).max(bad_max) + 1
    }

    fn is_forgotten(&self, text: &str) -> bool {
        let n = norm(text);
        !n.is_empty() && self.forgotten.iter().any(|f| !f.is_empty() && (n.contains(f.as_str()) || f.contains(&n)))
    }

    /// 合并新事实：同一件事/同一主题/只留一条的类别 → 新的覆盖旧的；超过上限先丢最久没更新的（称呼和生日最后才丢）。
    /// user = true：主人亲口说的，会解除「忘掉过」的标记；false（AI 总结的）：忘掉过的不再捡回来。
    /// 返回实际改动的条数
    pub fn merge(&mut self, facts: &[Fact], now: Dt, user: bool) -> usize {
        let stamp = now.fmt();
        let mut changed = 0;
        for f in facts {
            let text = clip(&f.text, ITEM_CHARS);
            if norm(&text).is_empty() {
                continue;
            }
            if user {
                let n = norm(&text);
                self.forgotten.retain(|x| !(n.contains(x.as_str()) || x.contains(&n)));
            } else if self.is_forgotten(&text) {
                continue;
            }
            let fact = Fact { text: text.clone(), ..f.clone() };
            // 同一件事可能对上好几条旧的：留第一条更新，其余删掉
            let hits: Vec<usize> = self.items.iter().enumerate().filter(|(_, i)| i.same_as(&fact)).map(|(k, _)| k).collect();
            if let Some(&first) = hits.first() {
                let it = &mut self.items[first];
                let unchanged = it.text == text && it.kind == f.kind && (f.key.is_empty() || it.key == f.key);
                it.text = text;
                it.kind = f.kind;
                if !f.key.is_empty() {
                    it.key = f.key.trim().to_string();
                }
                it.updated = stamp.clone();
                if user {
                    it.source = "user".into();
                }
                for &k in hits[1..].iter().rev() {
                    self.items.remove(k);
                }
                if !unchanged || hits.len() > 1 {
                    changed += 1;
                }
                // 最新的放最后
                let it = self.items.remove(first);
                self.items.push(it);
            } else {
                let id = self.next_id();
                self.items.push(Item {
                    id,
                    kind: f.kind,
                    text,
                    key: f.key.trim().to_string(),
                    created: stamp.clone(),
                    updated: stamp.clone(),
                    source: if user { "user".into() } else { "ai".into() },
                    extra: Map::new(),
                });
                changed += 1;
            }
        }
        changed + self.enforce_cap()
    }

    /// 超过上限：丢最久没更新的（先丢普通的，称呼/生日最后）
    fn enforce_cap(&mut self) -> usize {
        let mut n = 0;
        while self.items.len() > MAX_ITEMS {
            let pick = |pinned: bool| {
                self.items
                    .iter()
                    .enumerate()
                    .filter(|(_, i)| i.pinned() == pinned)
                    .min_by(|a, b| a.1.updated.cmp(&b.1.updated).then(a.0.cmp(&b.0)))
                    .map(|(k, _)| k)
            };
            let Some(k) = pick(false).or_else(|| pick(true)) else { break };
            self.items.remove(k);
            n += 1;
        }
        n
    }

    /// 忘掉和 query 有关的条目，返回被删掉的文字
    pub fn forget(&mut self, query: &str) -> Vec<String> {
        let q = norm(query);
        if q.is_empty() {
            return vec![];
        }
        let mut gone = vec![];
        self.items.retain(|i| {
            let n = norm(&i.text);
            let hit = (!n.is_empty() && (n.contains(&q) || (q.chars().count() >= 2 && q.contains(&n))))
                || (!i.key.is_empty() && norm(&i.key) == q);
            if hit {
                gone.push(i.text.clone());
            }
            !hit
        });
        if !self.forgotten.contains(&q) {
            self.forgotten.push(q);
        }
        let over = self.forgotten.len().saturating_sub(MAX_FORGOTTEN);
        self.forgotten.drain(..over);
        gone
    }

    /// 清空记忆（之前的聊天记录也不再提取）
    pub fn clear(&mut self, now_ms: u64) {
        self.items.clear();
        self.bad.clear();
        self.forgotten.clear();
        self.extracted_at = self.extracted_at.max(now_ms);
    }

    /// 排序：称呼、生日在前，其余最近更新的在前
    pub fn ranked(&self) -> Vec<&Item> {
        let mut v: Vec<&Item> = self.items.iter().collect();
        v.sort_by(|a, b| {
            b.pinned().cmp(&a.pinned()).then(b.kind.eq(&Kind::Name).cmp(&a.kind.eq(&Kind::Name))).then(b.updated.cmp(&a.updated))
        });
        v
    }

    /// 放进系统提示词的一段（≤ max 字）。没有记忆返回空串
    pub fn prompt_block(&self, max: usize) -> String {
        if self.items.is_empty() {
            return String::new();
        }
        let mut out = String::from("你记得的关于用户的事（括号里是记下的日期；自然地用上，别一次全说出来，没记过的别编）：");
        let mut len = out.chars().count();
        if len >= max {
            return String::new();
        }
        for it in self.ranked() {
            let date = it.updated.get(5..10).unwrap_or("");
            let line = if date.is_empty() { format!("\n- {}", it.text) } else { format!("\n- {}（{}）", it.text, date) };
            let n = line.chars().count();
            if len + n > max {
                break;
            }
            out.push_str(&line);
            len += n;
        }
        out
    }

    /// 「你记得我什么」：挑几条
    pub fn recall(&self, n: usize) -> Vec<String> {
        self.ranked().into_iter().take(n).map(|i| clip(&i.text, 24)).collect()
    }

    /// 今天还能不能主动关心（一天最多 limit 次，两次至少隔 gap_min 分钟）
    pub fn care_ok(&self, now: Dt, limit: u32, gap_min: i64) -> bool {
        let today = now.fmt()[..10].to_string();
        if self.care.day == today && self.care.count >= limit {
            return false;
        }
        match Dt::parse(&self.care.last) {
            Some(t) => now.to_min() - t.to_min() >= gap_min || now.to_min() < t.to_min(),
            None => true,
        }
    }

    pub fn care_mark(&mut self, now: Dt) {
        let today = now.fmt()[..10].to_string();
        if self.care.day != today {
            self.care.day = today;
            self.care.count = 0;
        }
        self.care.count += 1;
        self.care.last = now.fmt();
    }
}

// ---------------------------------------------------------------- 全局存储

struct Store {
    path: PathBuf,
    book: Book,
    mtime: Option<std::time::SystemTime>,
    checked_at: std::time::Instant,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);
static CLEAR_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn clear_gen() -> u64 { CLEAR_GEN.load(std::sync::atomic::Ordering::SeqCst) }

pub fn file() -> PathBuf { super::paths::active_skin_data_dir().join("memory.json") }

fn mtime_of(p: &std::path::Path) -> Option<std::time::SystemTime> { std::fs::metadata(p).and_then(|m| m.modified()).ok() }

fn read_file_at(p: &std::path::Path) -> Book {
    let Ok(s) = std::fs::read_to_string(p) else { return Book::default() };
    match Book::parse(&s) {
        Some(b) => b,
        None => {
            // 整个文件坏了：留个备份再从空的开始
            let mut bak = p.as_os_str().to_owned();
            bak.push(".broken");
            let _ = std::fs::copy(p, PathBuf::from(bak));
            super::log::log("memory.json unreadable, backed up to memory.json.broken");
            Book::default()
        }
    }
}

impl Store {
    fn open(p: PathBuf) -> Self {
        let book = read_file_at(&p);
        let mtime = mtime_of(&p);
        Self { path: p, book, mtime, checked_at: std::time::Instant::now() }
    }

    /// 切换角色后路径变化时自动重读对应角色的记忆本，并递增代数丢弃跨角色的后台提取
    fn sync_path(&mut self, cur: PathBuf) {
        if self.path != cur {
            CLEAR_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.book = read_file_at(&cur);
            self.mtime = mtime_of(&cur);
            self.path = cur;
            self.checked_at = std::time::Instant::now();
        } else if self.checked_at.elapsed().as_secs() >= 5 {
            self.checked_at = std::time::Instant::now();
            let m = mtime_of(&self.path);
            if m != self.mtime {
                self.book = read_file_at(&self.path);
                self.mtime = m;
            }
        }
    }
}

fn with<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    let mut g = STORE.lock().unwrap_or_else(|e| e.into_inner());
    let cur = file();
    match g.as_mut() {
        Some(s) => {
            s.sync_path(cur);
            f(s)
        }
        None => {
            let mut s = Store::open(cur);
            let r = f(&mut s);
            *g = Some(s);
            r
        }
    }
}

fn save(s: &mut Store) {
    let json = s.book.to_json();
    if let Err(e) = super::save::write_atomic(&s.path, json.as_bytes()) {
        super::log::log(&format!("memory write failed {e}"));
    }
    s.mtime = mtime_of(&s.path);
}

/// 改数据，f 返回 (结果, 有没有变)；变了就写盘
pub fn edit<R>(f: impl FnOnce(&mut Book) -> (R, bool)) -> R {
    with(|s| {
        let (r, changed) = f(&mut s.book);
        if changed {
            save(s);
        }
        r
    })
}

pub fn snapshot() -> Book { with(|s| s.book.clone()) }

pub fn count() -> usize { with(|s| s.book.items.len()) }

/// 后台提取的结果（如果期间发生过清空记忆，或 seen_until 已落后于 extracted_at，则丢弃）
pub fn merge_ai_if(facts: &[Fact], seen_until: u64, expected_gen: u64) -> usize {
    edit(|b| {
        if clear_gen() != expected_gen || (seen_until > 0 && seen_until <= b.extracted_at) {
            return (0, false);
        }
        let n = b.merge(facts, Now::get().dt, false);
        let moved = seen_until > b.extracted_at;
        if moved {
            b.extracted_at = seen_until;
        }
        (n, n > 0 || moved)
    })
}

/// 后台提取的结果
#[allow(dead_code)]
pub fn merge_ai(facts: &[Fact], seen_until: u64) -> usize {
    merge_ai_if(facts, seen_until, clear_gen())
}

pub fn clear() {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    edit(|b| {
        CLEAR_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        b.clear(now);
        ((), true)
    })
}

pub fn prompt() -> String { with(|s| s.book.prompt_block(PROMPT_CHARS)) }

/// 文件不存在先建一个（菜单「打开 memory.json」）
pub fn ensure_file() -> PathBuf {
    let p = file();
    if !p.exists() {
        with(save);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(d: u32, h: u32) -> Dt { Dt::new(2026, 11, d, h, 0).unwrap() }
    fn f(kind: Kind, t: &str) -> Fact { Fact::new(kind, t) }

    #[test]
    fn merge_dedups_and_overrides() {
        let mut b = Book::default();
        assert_eq!(b.merge(&[f(Kind::Like, "主人喜欢吃草莓")], at(1, 9), false), 1);
        // 同一件事（只差标点/主语）不重复
        assert_eq!(b.merge(&[f(Kind::Like, "喜欢吃草莓！")], at(2, 9), false), 1);
        assert_eq!(b.items.len(), 1);
        assert_eq!(b.items[0].text, "喜欢吃草莓！");
        assert_eq!(b.items[0].created, "2026-11-01 09:00");
        assert_eq!(b.items[0].updated, "2026-11-02 09:00");
        // 完全一样再来一次：不算改动
        assert_eq!(b.merge(&[f(Kind::Like, "喜欢吃草莓！")], at(2, 9), false), 0);
        // 包含关系 = 同一件事，新的覆盖旧的
        b.merge(&[f(Kind::Like, "主人喜欢吃草莓蛋糕")], at(3, 9), false);
        assert_eq!(b.items.len(), 1);
        assert_eq!(b.items[0].text, "主人喜欢吃草莓蛋糕");
    }

    #[test]
    fn single_kinds_and_keys_replace() {
        let mut b = Book::default();
        b.merge(&[f(Kind::Name, "主人叫小明"), f(Kind::Birthday, "生日是 3 月 5 日")], at(1, 9), true);
        b.merge(&[f(Kind::Name, "主人希望被叫作明明")], at(2, 9), true);
        assert_eq!(b.items.iter().filter(|i| i.kind == Kind::Name).count(), 1);
        assert!(b.items.iter().any(|i| i.text == "主人希望被叫作明明"));
        let exam = |t: &str| Fact { kind: Kind::Busy, text: t.into(), key: "考试".into() };
        b.merge(&[exam("主人下周三考高数")], at(3, 9), false);
        b.merge(&[exam("主人的高数考试改到周五了")], at(4, 9), false);
        let busy: Vec<_> = b.items.iter().filter(|i| i.kind == Kind::Busy).collect();
        assert_eq!(busy.len(), 1);
        assert_eq!(busy[0].text, "主人的高数考试改到周五了");
        assert_eq!(b.items.len(), 3);
    }

    #[test]
    fn cap_drops_oldest_keeps_pinned() {
        let mut b = Book::default();
        b.merge(&[f(Kind::Name, "主人叫阿猫")], at(1, 0), true);
        for i in 0..70u32 {
            b.merge(&[f(Kind::Note, &format!("第{i}件事情"))], at(1 + i / 24, i % 24), false);
        }
        assert_eq!(b.items.len(), MAX_ITEMS);
        assert!(b.items.iter().any(|i| i.kind == Kind::Name), "称呼不该被挤掉");
        assert!(!b.items.iter().any(|i| i.text == "第0件事情"));
        assert!(b.items.iter().any(|i| i.text == "第69件事情"));
        // ids 不重复
        let mut ids: Vec<u64> = b.items.iter().map(|i| i.id).collect();
        ids.dedup();
        assert_eq!(ids.len(), MAX_ITEMS);
    }

    #[test]
    fn forget_and_tombstone() {
        let mut b = Book::default();
        b.merge(&[f(Kind::Like, "主人喜欢草莓"), f(Kind::Busy, "主人在准备考研")], at(1, 9), false);
        let gone = b.forget("草莓");
        assert_eq!(gone, vec!["主人喜欢草莓".to_string()]);
        assert_eq!(b.items.len(), 1);
        // AI 总结又提到草莓：不捡回来
        assert_eq!(b.merge(&[f(Kind::Like, "喜欢草莓")], at(2, 9), false), 0);
        // 主人亲口让记：可以
        assert_eq!(b.merge(&[f(Kind::Like, "我喜欢草莓")], at(2, 10), true), 1);
        assert!(b.forgotten.is_empty());
        assert!(b.forget("不存在的事").is_empty());
    }

    #[test]
    fn prompt_block_limit_and_order() {
        let mut b = Book::default();
        assert_eq!(b.prompt_block(600), "");
        for i in 0..50u32 {
            b.merge(&[f(Kind::Event, &format!("这是一条比较长的记忆内容编号{i}，用来测试字数上限"))], at(1, 0).add_min(i as i64), false);
        }
        b.merge(&[f(Kind::Name, "主人叫小明")], at(1, 0), true);
        let p = b.prompt_block(PROMPT_CHARS);
        assert!(p.chars().count() <= PROMPT_CHARS);
        let first = p.lines().nth(1).unwrap();
        assert!(first.starts_with("- 主人叫小明"), "{first}");
        // 最近的在前
        assert!(p.lines().nth(2).unwrap().contains("编号49"));
        assert!(p.contains("（11-01）"));
    }

    #[test]
    fn json_roundtrip_keeps_unknown() {
        let src = r#"{"version":1,"items":[{"id":3,"kind":"like","text":"喜欢猫","created":"2026-11-01 09:00","updated":"2026-11-01 09:00","mood":"x"},{"id":9,"text":""},{"weird":true}],"care":{"day":"2026-11-01","count":1,"last":"2026-11-01 10:00","z":1},"extractedAt":42,"theme":"dark"}"#;
        let b = Book::parse(src).unwrap();
        assert_eq!(b.items.len(), 1);
        assert_eq!(b.bad.len(), 2);
        assert_eq!(b.extracted_at, 42);
        assert_eq!(b.care.count, 1);
        let out = b.to_json();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["items"][0]["mood"], "x");
        assert_eq!(v["care"]["z"], 1);
        assert_eq!(v["items"].as_array().unwrap().len(), 3);
        assert_eq!(Book::parse(&out).unwrap(), b);
        assert!(Book::parse("not json").is_none());
        assert_eq!(Book::parse("").unwrap(), Book::default());
        assert_eq!(Kind::parse("BUSY"), Kind::Busy);
        assert_eq!(Kind::parse("whatever"), Kind::Note);
    }

    #[test]
    fn care_limits() {
        let mut b = Book::default();
        assert!(b.care_ok(at(5, 9), 2, 180));
        b.care_mark(at(5, 9));
        assert!(!b.care_ok(at(5, 10), 2, 180), "间隔太短");
        assert!(b.care_ok(at(5, 13), 2, 180));
        b.care_mark(at(5, 13));
        assert!(!b.care_ok(at(5, 20), 2, 180), "一天最多两次");
        assert!(b.care_ok(at(6, 9), 2, 180));
        b.care_mark(at(6, 9));
        assert_eq!(b.care.count, 1);
    }

    #[test]
    fn clear_moves_extract_mark() {
        let mut b = Book::default();
        b.merge(&[f(Kind::Like, "喜欢猫")], at(1, 9), true);
        b.clear(1000);
        assert!(b.items.is_empty());
        assert_eq!(b.extracted_at, 1000);
        assert_eq!(b.recall(5).len(), 0);
    }

    #[test]
    fn per_skin_store_isolates_memories_and_bumps_clear_gen() {
        let root = std::env::temp_dir().join(format!("hakimi-mem-iso-{}-{}", std::process::id(), fastrand::u32(..)));
        let _ = std::fs::remove_dir_all(&root);
        let hakimi_mem = super::super::paths::skin_data_dir_in(&root, "hakimi").join("memory.json");
        let mint_mem = super::super::paths::skin_data_dir_in(&root, "小薄荷").join("memory.json");
        assert_eq!(hakimi_mem, root.join("memory.json"), "内置哈基米保持根目录兼容");
        assert_eq!(mint_mem, root.join("skins-data").join("小薄荷").join("memory.json"));

        let mut st = Store::open(hakimi_mem.clone());
        st.book.merge(&[f(Kind::Like, "主人喜欢小鱼干")], at(1, 9), true);
        save(&mut st);

        let g0 = clear_gen();
        st.sync_path(mint_mem.clone());
        assert!(clear_gen() > g0, "切换角色应递增代数，阻止旧角色的后台记忆提取串入新角色");
        assert!(st.book.items.is_empty(), "新角色初始记忆应为空");

        st.book.merge(&[f(Kind::Like, "主人喜欢薄荷糖")], at(1, 10), true);
        save(&mut st);

        // 切回哈基米：各自记忆互不覆盖
        st.sync_path(hakimi_mem);
        assert_eq!(st.book.items.len(), 1);
        assert!(st.book.items[0].text.contains("小鱼干"));

        st.sync_path(mint_mem);
        assert_eq!(st.book.items.len(), 1);
        assert!(st.book.items[0].text.contains("薄荷糖"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
