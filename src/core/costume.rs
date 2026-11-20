//! 整套衣服（换装）清单：菜单、配置读写、渲染层共用一份。
//!
//! 配置里的 `costume` 字段和 Electron 版互通：
//! - `""`：默认衣服。以前是卫衣，现在默认是奶白水手服（`sailor`）；
//!   老的 config.json 都是 `""`（原版会把完整模板写回去），所以老用户没主动选过的会换上新默认。
//! - `"hoodie"`：原装卫衣（Electron 版不认识这个值，会退回它自己的默认＝卫衣，看起来一样）。
//! - `"sailor"`：也认，等同 `""`；写回时统一写 `""`。
//! - 其余 id 原样；不认识的值当默认。

/// (内部 id, 菜单文字)。顺序就是菜单顺序；第一个是默认
pub const COSTUMES: [(&str, &str); 8] = [
    ("sailor", "🌈 奶白水手服（默认）"),
    ("hoodie", "🤍 原装卫衣"),
    ("pajama", "🌙 猫耳睡衣"),
    ("idol", "🎤 偶像演出服"),
    ("swim", "🏖️ 泳装"),
    ("xmas", "🎄 圣诞小红裙"),
    ("maid", "🎀 女仆装"),
    ("jk", "🏫 JK 水手服"),
];

pub const DEFAULT: &str = "sailor";

/// 配置值 / 菜单 id → 内部 id
pub fn normalize(v: &str) -> &'static str {
    match v.trim() {
        "" => DEFAULT,
        v => COSTUMES.iter().find(|(id, _)| *id == v).map(|(id, _)| *id).unwrap_or(DEFAULT),
    }
}

/// 内部 id → 写进 config.json 的值（默认衣服写 ""，保持和原版互通）
pub fn config_value(id: &str) -> String {
    let id = normalize(id);
    if id == DEFAULT { String::new() } else { id.to_string() }
}

/// 穿的是不是「本体」衣服（默认水手服或原装卫衣）：天冷自动围围巾之类只在这两套上做
pub fn is_base(id: &str) -> bool { matches!(normalize(id), "sailor" | "hoodie") }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_configs_still_work() {
        // 老配置的 "" = 当时的默认 → 现在的默认水手服
        assert_eq!(normalize(""), "sailor");
        // 老用户主动选过的保留
        for c in ["xmas", "maid", "jk"] { assert_eq!(normalize(c), c); }
        assert_eq!(normalize("hoodie"), "hoodie");
        assert_eq!(normalize("sailor"), "sailor");
        assert_eq!(normalize(" jk "), "jk");
        // 乱填的当默认
        assert_eq!(normalize("kimono"), "sailor");
        assert_eq!(normalize("HOODIE"), "sailor");
    }

    #[test]
    fn config_round_trip() {
        assert_eq!(config_value("sailor"), "");
        assert_eq!(config_value(""), "");
        assert_eq!(config_value("hoodie"), "hoodie");
        assert_eq!(config_value("swim"), "swim");
        assert_eq!(config_value("bogus"), "");
        for (id, _) in COSTUMES { assert_eq!(normalize(&config_value(id)), id); }
        // 用 config 模块完整解析一遍
        let c = crate::core::config::parse(r#"{"costume":""}"#);
        assert_eq!(normalize(&c.costume), "sailor");
        let c = crate::core::config::parse(r#"{"costume":"maid"}"#);
        assert_eq!(normalize(&c.costume), "maid");
        let c = crate::core::config::parse(r#"{}"#);
        assert_eq!(normalize(&c.costume), "sailor");
    }

    #[test]
    fn base_outfits() {
        assert!(is_base("") && is_base("sailor") && is_base("hoodie"));
        assert!(!is_base("idol") && !is_base("xmas"));
    }
}
