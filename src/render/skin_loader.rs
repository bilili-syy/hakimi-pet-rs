//! 角色包立绘：按需从磁盘解码当前皮肤的图，缩放进内置立绘的框（200×293 逻辑像素）。
//!
//! 内存：
//! - 只解码「当前皮肤」「当前要画」的那张图；原图解码后立刻缩小并丢掉（和内置立绘一样）。
//! - 缩放好的图进渲染器原有的 SpriteCache（容量固定 12 张），键带 `skin:<版本>:` 前缀。
//! - 换皮肤 / 刷新列表时 core::skin::version() 变了：`sync` 把所有 `skin:` 开头的缓存图立刻释放，
//!   所以新旧两套图不会同时留在内存里。
//! - 超大图（>16MB 文件或边长 >4096）直接拒绝、回退内置，防止一张图吃掉几百 MB。
use super::sprites::{SpriteCache, load_scaled};
use crate::core::skin::{self, SkinInfo};
use crate::types::Face;
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use tiny_skia::{Pixmap, PixmapPaint, Transform};

const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SIDE: u32 = 4096;
/// 缓存键里的标记（glow 的键是 "glow:skin:…"，也一起清）
const KEY_MARK: &str = "skin:";

thread_local! {
    static SEEN: Cell<Option<u64>> = const { Cell::new(None) };
    /// 解码失败过的键（坏图别每帧都重读），换皮肤时清空
    static FAILED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// 每帧开头调用：皮肤版本变了就释放旧皮肤的缓存图。返回 true = 这一帧必须重画
pub fn sync(cache: &mut SpriteCache) -> bool {
    let v = skin::version();
    let changed = SEEN.with(|s| s.replace(Some(v)) != Some(v));
    if changed {
        cache.forget(|k| k.contains(KEY_MARK));
        FAILED.with(|f| f.borrow_mut().clear());
    }
    changed
}

/// 只读 PNG 头拿尺寸，不解码
pub fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" { return None; }
    let be = |i: usize| u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    Some((be(16), be(20)))
}

/// 等比缩放后在 (bw,bh) 框里的位置和大小：贴底、水平居中。返回 (x, y, w, h)
pub fn fit_rect(iw: u32, ih: u32, bw: u32, bh: u32, scale: f32) -> (i32, i32, u32, u32) {
    let s = (bw as f32 / iw.max(1) as f32).min(bh as f32 / ih.max(1) as f32) * scale;
    let w = ((iw as f32 * s).round() as u32).max(1);
    let h = ((ih as f32 * s).round() as u32).max(1);
    ((bw as i32 - w as i32) / 2, bh as i32 - h as i32, w, h)
}

/// 读图 → 等比缩进框（贴底居中，scale 再放大/缩小）→ 返回框大小的透明画布。
/// 输出和内置立绘同样大小，所以后面的变换、光晕、点击判定都不用改。
pub fn load_fit(path: &Path, bw: u32, bh: u32, scale: f32) -> Option<Pixmap> {
    if std::fs::metadata(path).ok()?.len() > MAX_FILE_BYTES { return None; }
    let bytes = std::fs::read(path).ok()?;
    let (iw, ih) = png_size(&bytes)?;
    if iw == 0 || ih == 0 || iw > MAX_SIDE || ih > MAX_SIDE { return None; }
    let (x, y, w, h) = fit_rect(iw, ih, bw, bh, scale);
    let img = load_scaled(&bytes, w, h)?;
    drop(bytes);
    let mut out = Pixmap::new(bw.max(1), bh.max(1))?;
    out.draw_pixmap(x, y, img.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    Some(out)
}

/// 这个表情用皮肤里的哪个状态图。瞬间帧（眨眼、走路两帧）没画就借待机图，免得一闪变回哈基米；
/// 其它没画的状态返回 None，回退内置哈基米。
pub fn state_for(s: &SkinInfo, face: Face) -> Option<&'static str> {
    let stem = face.stem();
    let has = |st: &str| s.sprite_file(st).is_some();
    if has(stem) { return Some(stem); }
    let alias: &[&'static str] = match face {
        Face::Blink => &["idle"],
        Face::Walk1 => &["walk2", "idle"],
        Face::Walk2 => &["walk1", "idle"],
        _ => &[],
    };
    alias.iter().copied().find(|a| has(a))
}

fn key(s: &SkinInfo, what: &str) -> String { format!("{KEY_MARK}{}:{}:{what}", skin::version(), s.id) }

/// 皮肤底图。返回 (缓存键, 图)；None = 这个表情回退内置。
/// 眨眼：没有 blink 立绘但有 blink 叠加层 → 待机图 + 叠加层合成一张。
pub fn base_of(cache: &mut SpriteCache, s: &SkinInfo, face: Face, w: u32, h: u32) -> Option<(String, Rc<Pixmap>)> {
    let st = state_for(s, face)?;
    let scale = s.scale();
    let blink_ov = if face == Face::Blink && st != "blink" { s.overlay_file("blink") } else { None };
    let k = key(s, if blink_ov.is_some() { "idle+blink" } else { st });
    let file = s.sprite_file(st)?;
    let fkey = format!("{k}@{w}x{h}");
    if FAILED.with(|f| f.borrow().contains(&fkey)) { return None; }
    let pm = cache.get_or(&k, (w, h), || {
        let mut base = load_fit(&file, w, h, scale)?;
        if let Some(ov) = &blink_ov
            && let Some(o) = load_fit(ov, w, h, scale)
        {
            base.draw_pixmap(0, 0, o.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        }
        Some(base)
    });
    let Some(pm) = pm else {
        if !cfg!(test) { crate::core::log::log(&format!("skin image failed: {}", file.display())); }
        FAILED.with(|f| f.borrow_mut().push(fkey));
        return None;
    };
    Some((k, pm))
}

/// 叠加层（blink / mouthOpen），和底图同样缩放对齐。给呼吸/眨眼/张嘴那条线用
#[allow(dead_code)]
pub fn overlay_of(cache: &mut SpriteCache, s: &SkinInfo, name: &str, w: u32, h: u32) -> Option<Rc<Pixmap>> {
    let file = s.overlay_file(name)?;
    let scale = s.scale();
    cache.get_or(&key(s, &format!("ov:{name}")), (w, h), || load_fit(&file, w, h, scale))
}

/// 渲染器钩子：当前皮肤这一帧的底图（内置哈基米 / 回退时 None）
pub fn base(cache: &mut SpriteCache, face: Face, w: u32, h: u32) -> Option<(String, Rc<Pixmap>)> {
    let s = skin::active()?;
    base_of(cache, &s, face, w, h)
}

/// 当前皮肤的叠加层
#[allow(dead_code)]
pub fn overlay(cache: &mut SpriteCache, name: &str, w: u32, h: u32) -> Option<Rc<Pixmap>> {
    let s = skin::active()?;
    overlay_of(cache, &s, name, w, h)
}

/// 用了皮肤底图的表情：不画哈基米专属的尾巴/星星分层和配件（位置是按哈基米对的）
pub fn strip_layers(mut l: super::sprites::Layers, key: &str) -> super::sprites::Layers {
    l.base = key.to_string();
    l.tail = false;
    l.star = false;
    l.outfit_offset = None;
    l
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::skin::{MANIFEST, load_dir};
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hakimi-skinld-{}-{name}-{}", std::process::id(), fastrand::u32(..)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// w×h 的不透明纯色 PNG
    fn png(path: &Path, w: u32, h: u32, rgba: [u8; 4]) {
        let mut p = Pixmap::new(w, h).unwrap();
        p.fill(tiny_skia::Color::from_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        p.save_png(path).unwrap();
    }

    fn alpha(p: &Pixmap, x: u32, y: u32) -> u8 { p.pixel(x, y).unwrap().alpha() }

    #[test]
    fn fit_math() {
        // 一样比例：铺满
        assert_eq!(fit_rect(400, 586, 200, 293, 1.0), (0, 0, 200, 293));
        // 方图：按宽缩，贴底
        assert_eq!(fit_rect(100, 100, 200, 293, 1.0), (0, 93, 200, 200));
        // scale 0.5：居中贴底
        assert_eq!(fit_rect(100, 100, 200, 293, 0.5), (50, 193, 100, 100));
        // 又高又瘦：按高缩
        assert_eq!(fit_rect(100, 1000, 200, 293, 1.0).3, 293);
    }

    #[test]
    fn png_header() {
        let d = tmp("hdr");
        let f = d.join("a.png");
        png(&f, 7, 9, [1, 2, 3, 255]);
        assert_eq!(png_size(&std::fs::read(&f).unwrap()), Some((7, 9)));
        assert_eq!(png_size(b"GIF89a................."), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn load_and_fallback_with_chinese_path() {
        let d = tmp("角色").join("小薄荷");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(MANIFEST), r#"{"scale":0.5,"sprites":{"idle":"图/待机.png","happy":"缺.png"},"overlays":{"blink":"眨眼.png"}}"#).unwrap();
        png(&d.join("图").join("待机.png"), 100, 100, [0, 200, 100, 255]);
        // 叠加层：同尺寸，只有上半部分不透明
        let mut ov = Pixmap::new(100, 100).unwrap();
        ov.fill_rect(tiny_skia::Rect::from_xywh(0.0, 0.0, 100.0, 50.0).unwrap(), &tiny_skia::Paint::default(), Transform::identity(), None);
        ov.save_png(d.join("眨眼.png")).unwrap();
        // 坏图
        std::fs::write(d.join("缺.png"), b"not a png").unwrap();
        let s = load_dir(&d).unwrap();
        let mut c = SpriteCache::new(4);

        // 待机：100×100 等比缩进 200×293 再 ×0.5 → 100×100，贴底居中
        let (k, p) = base_of(&mut c, &s, Face::Idle, 200, 293).unwrap();
        assert!(k.starts_with("skin:") && k.ends_with(":idle"));
        assert_eq!((p.width(), p.height()), (200, 293));
        assert_eq!(alpha(&p, 100, 290), 255);
        assert_eq!(alpha(&p, 100, 150), 0);
        assert_eq!(alpha(&p, 10, 290), 0);

        // 眨眼：没 blink 立绘 → 借待机 + 叠加 blink
        assert_eq!(state_for(&s, Face::Blink), Some("idle"));
        let (k, _) = base_of(&mut c, &s, Face::Blink, 200, 293).unwrap();
        assert!(k.ends_with("idle+blink"));
        // 走路帧也借待机
        assert_eq!(state_for(&s, Face::Walk2), Some("idle"));
        // happy 的文件是坏的 / sleep 没写 → 回退内置
        assert!(base_of(&mut c, &s, Face::Happy, 200, 293).is_none());
        assert!(base_of(&mut c, &s, Face::Sleep, 200, 293).is_none());
        // 叠加层同样对齐：上半不透明部分落在贴底的 100×100 区域的上半
        let o = overlay_of(&mut c, &s, "blink", 200, 293).unwrap();
        assert_eq!(alpha(&o, 100, 200), 255);
        assert_eq!(alpha(&o, 100, 280), 0);
        assert!(overlay_of(&mut c, &s, "mouthOpen", 200, 293).is_none());
        let _ = std::fs::remove_dir_all(d.parent().unwrap());
    }

    #[test]
    fn switching_frees_old_images() {
        let mut c = SpriteCache::new(12);
        c.get_or("sprites/idle.png", (1, 1), || Pixmap::new(1, 1));
        c.get_or("skin:1:a:idle", (1, 1), || Pixmap::new(1, 1));
        c.get_or("glow:skin:1:a:idle", (1, 1), || Pixmap::new(1, 1));
        SEEN.with(|s| s.set(Some(u64::MAX)));
        assert!(sync(&mut c));
        // 皮肤图（含光晕）都释放，内置的留着
        assert_eq!(c.len(), 1);
        assert!(!sync(&mut c));
    }

    /// 手动看效果：HAKIMI_SNAP=skin.png cargo test skin_snapshot -- --ignored
    /// 用 examples/skins/demo 画 待机 / 眨眼(叠加层) / 开心 / 坐（没画 → 回退哈基米）
    #[test]
    #[ignore]
    fn skin_snapshot() {
        let out = std::env::var("HAKIMI_SNAP").unwrap_or_else(|_| "skin.png".into());
        let demo = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples").join("skins").join("demo");
        crate::core::skin::activate_for_test(Some(load_dir(&demo).unwrap()));
        let mut r = crate::render::Renderer::new();
        let faces = [Face::Idle, Face::Blink, Face::Happy, Face::Sit];
        let mut sheet = Pixmap::new(240 * faces.len() as u32, 360).unwrap();
        for (i, f) in faces.iter().enumerate() {
            let sc = crate::render::Scene { face: *f, ..Default::default() };
            r.draw(&sc, 1.0);
            sheet.draw_pixmap(240 * i as i32, 0, r.pixmap().unwrap().as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        }
        crate::core::skin::activate_for_test(None);
        sheet.save_png(&out).unwrap();
    }

    #[test]
    fn rejects_huge_header() {
        let d = tmp("huge");
        let f = d.join("big.png");
        png(&f, 4, 4, [0, 0, 0, 255]);
        let mut b = std::fs::read(&f).unwrap();
        b[16..20].copy_from_slice(&100_000u32.to_be_bytes());
        std::fs::write(&f, &b).unwrap();
        assert!(load_fit(&f, 200, 293, 1.0).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }
}
