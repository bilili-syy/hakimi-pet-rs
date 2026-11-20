//! 内嵌资源：assets/ 整个目录编进 exe，单文件分发。
//! 路径用正斜杠、相对 assets/，例如 "sprites/idle.png"、"voice/lines/xxxx.ogg"。
use include_dir::{Dir, include_dir};

static ASSETS: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/assets");

pub fn get(path: &str) -> Option<&'static [u8]> {
    ASSETS.get_file(path).map(|f| f.contents())
}
