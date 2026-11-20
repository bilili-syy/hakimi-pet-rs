//! save.json：养成数值存档（字段和 Electron 版一致）
//! 读：坏字段只回退那一个字段（原版 `{...DEFAULT, ...saved}` 的效果），整个文件坏了返回 None。
//! 写：先写 save.json.tmp 再 rename（和原版 main.js writeSave 一样），防断电坏档。
#![allow(dead_code)]
use super::stats::StatsData;
use std::io::Write;
use std::path::Path;

pub fn load() -> Option<StatsData> {
    let s = std::fs::read_to_string(super::paths::save_file()).ok()?;
    parse(&s)
}

/// 解析存档文本；不是 JSON 对象返回 None（原版 readSave 返回 null → 全新开始）
pub fn parse(text: &str) -> Option<StatsData> {
    let v: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    if !v.is_object() { return None; }
    Some(StatsData::from_value(v))
}

pub fn store(data: &StatsData) {
    let s = match serde_json::to_string_pretty(data) {
        Ok(s) => s,
        Err(e) => { super::log::log(&format!("save failed {e}")); return; }
    };
    if let Err(e) = write_atomic(&super::paths::save_file(), s.as_bytes()) {
        super::log::log(&format!("save failed {e}"));
    }
}

/// 原子写：同目录写 `<name>.tmp`、刷盘，再 rename 覆盖（Windows 上 std::fs::rename 会替换已有文件）
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path).inspect_err(|_| { let _ = std::fs::remove_file(&tmp); })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces() {
        let dir = std::env::temp_dir().join(format!("hakimi-save-test-{}", std::process::id()));
        let p = dir.join("save.json");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        assert!(!dir.join("save.json.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_electron_save() {
        // 原版 JSON.stringify(Stats.snapshot(), null, 2) 的样子
        let d = parse(r#"{
  "hunger": 63.25,
  "mood": 80,
  "energy": 12.5,
  "affection": 42,
  "lastTick": 1763600000000,
  "totalFeeds": 7,
  "firstMet": 1760000000000,
  "future": "x"
}"#).unwrap();
        assert_eq!(d.hunger, 63.25);
        assert_eq!(d.mood, 80.0);
        assert_eq!(d.affection, 42.0);
        assert_eq!(d.total_feeds, 7);
        assert_eq!(d.last_tick, 1763600000000.0);
        // 写回去：原版能读的字段名 + 未知字段保留
        let out: serde_json::Value = serde_json::from_str(&serde_json::to_string_pretty(&d).unwrap()).unwrap();
        for k in ["hunger", "mood", "energy", "affection", "lastTick", "totalFeeds", "firstMet", "future"] {
            assert!(out.get(k).is_some(), "{k}");
        }
        assert_eq!(out["lastTick"].as_f64(), Some(1763600000000.0));
    }

    #[test]
    fn parse_partial_and_bad() {
        assert!(parse("garbage").is_none());
        assert!(parse("null").is_none());
        let d = parse(r#"{"hunger":null,"mood":"x","affection":5}"#).unwrap();
        assert_eq!(d.hunger, 80.0);
        assert_eq!(d.mood, 80.0);
        assert_eq!(d.affection, 5.0);
    }
}
