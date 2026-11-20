//! save.json：养成数值存档（字段和 Electron 版一致）
//! TODO(B)：先写 .tmp 再 rename，防断电坏档
use super::stats::StatsData;

pub fn load() -> Option<StatsData> {
    let s = std::fs::read_to_string(super::paths::save_file()).ok()?;
    serde_json::from_str(&s).ok()
}

pub fn store(data: &StatsData) {
    let _ = (data,);
    // TODO(B)
}
