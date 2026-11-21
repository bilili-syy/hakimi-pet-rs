//! 立绘选择（renderer.js 的 show()/LAYERED/COSTUME_*/OUTFIT_OFFSET）+ 懒解码缓存。
//! 只缓存「已缩放到当前 DPI」的小图，最多 CAP 张；原图解码后立刻缩小并丢掉。
use crate::types::Face;
use std::rc::Rc;
use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};

/// 某个表情要画哪些层
#[derive(Clone, Debug, PartialEq)]
pub struct Layers {
    /// 底图资源路径（相对 assets/）
    pub base: String,
    pub tail: bool,
    pub star: bool,
    /// 配件层的平移（400×587 原图像素）；None = 这个表情不戴配件
    pub outfit_offset: Option<(f32, f32)>,
}

pub const TAIL: &str = "sprites/layered/tail.png";
pub const STAR: &str = "sprites/layered/star.png";

/// 分层表情：(有星星层, 有尾巴层)
fn layered(face: Face) -> Option<(bool, bool)> {
    match face {
        Face::Idle | Face::Blink | Face::Walk1 | Face::Walk2 => Some((true, true)),
        Face::Happy | Face::Eat | Face::Hungry => Some((true, false)),
        _ => None,
    }
}

const COSTUME_FACES: [&str; 8] = ["idle", "happy", "drag", "sit", "sleep", "eat", "think", "hungry"];

fn costume_alias(stem: &str) -> &str {
    match stem {
        "shy" | "love" | "dance" | "smug" | "starry" => "happy",
        "honey" => "eat",
        "dizzy" | "surprised" | "hiss" | "scared" => "drag",
        "cry" | "pout" => "hungry",
        "stare" | "confused" | "puff" => "think",
        "yawn" => "idle",
        s => s,
    }
}

pub fn costume_src(costume: &str, face: Face) -> String {
    let n = costume_alias(face.stem());
    let f = if COSTUME_FACES.contains(&n) { n } else { "idle" };
    format!("outfits/{costume}/{f}.png")
}

pub fn outfit_offset(face: Face) -> Option<(f32, f32)> {
    match face {
        Face::Sit => Some((0.0, 17.0)),
        Face::Sleep | Face::Yawn | Face::Dizzy | Face::Hiss | Face::Dance => None,
        _ => Some((0.0, 0.0)),
    }
}

/// 默认水手服：和原装卫衣一样走分层（尾巴/星星单独动），其余表情按 costume_alias 退到 8 张基本姿势
const SAILOR_FACES: [&str; 4] = ["drag", "sit", "sleep", "think"];

fn sailor_layers(face: Face) -> (String, Option<(bool, bool)>) {
    if let Some(l) = layered(face) {
        return (format!("outfits/sailor/layered/{}.png", face.stem()), Some(l));
    }
    let n = costume_alias(face.stem());
    if SAILOR_FACES.contains(&n) {
        return (format!("outfits/sailor/{n}.png"), None);
    }
    // 别名落到分层的那几张（happy / eat / idle / hungry）
    let f = match n { "happy" => Face::Happy, "eat" => Face::Eat, "hungry" => Face::Hungry, _ => Face::Idle };
    (format!("outfits/sailor/layered/{}.png", f.stem()), layered(f))
}

/// costume：core::costume 的 id。"" / "hoodie" = 原装卫衣（sprites/），"sailor" = 默认水手服，其余 = outfits/<id>/
pub fn layers_for(face: Face, costume: &str, outfits: &[String]) -> Layers {
    let (base, l) = match costume {
        "" | "hoodie" => match layered(face) {
            Some(l) => (format!("sprites/layered/{}.png", face.stem()), Some(l)),
            None => (format!("sprites/{}.png", face.stem()), None),
        },
        "sailor" => sailor_layers(face),
        c => (costume_src(c, face), None),
    };
    let hat = outfits.iter().any(|o| o == "santa");
    Layers {
        base,
        star: l.is_some_and(|l| l.0) && !hat,
        tail: l.is_some_and(|l| l.1),
        outfit_offset: if outfits.is_empty() { None } else { outfit_offset(face) },
    }
}

/// 解码 + 缩放到 (w,h)。先 2× 盒式降采样到接近目标尺寸，再双三次缩放，避免锯齿
pub fn load_scaled(bytes: &[u8], w: u32, h: u32) -> Option<Pixmap> {
    let mut src = Pixmap::decode_png(bytes).ok()?;
    while src.width() >= w * 2 && src.height() >= h * 2 {
        src = halve(&src)?;
    }
    let mut dst = Pixmap::new(w.max(1), h.max(1))?;
    let t = Transform::from_scale(w as f32 / src.width() as f32, h as f32 / src.height() as f32);
    dst.draw_pixmap(0, 0, src.as_ref(), &PixmapPaint { quality: FilterQuality::Bicubic, ..Default::default() }, t, None);
    Some(dst)
}

fn halve(src: &Pixmap) -> Option<Pixmap> {
    let (w, h) = (src.width() / 2, src.height() / 2);
    let mut dst = Pixmap::new(w.max(1), h.max(1))?;
    let sw = src.width() as usize;
    let s = src.data();
    let d = dst.data_mut();
    for y in 0..h as usize {
        for x in 0..w as usize {
            for c in 0..4 {
                let i = |xx: usize, yy: usize| s[(yy * sw + xx) * 4 + c] as u32;
                let v = i(2 * x, 2 * y) + i(2 * x + 1, 2 * y) + i(2 * x, 2 * y + 1) + i(2 * x + 1, 2 * y + 1);
                d[(y * w as usize + x) * 4 + c] = ((v + 2) / 4) as u8;
            }
        }
    }
    Some(dst)
}

/// drop-shadow(0 0 r rgba(..)) 的光晕：按 alpha 着色后模糊。返回 (图, 四周外扩像素)
pub fn make_glow(src: &Pixmap, radius: f32, rgb: u32, alpha: f32) -> Option<(Pixmap, i32)> {
    let sigma = (radius / 2.0).max(0.5);
    let pad = (sigma * 3.0).ceil() as i32;
    let (w, h) = (src.width() as i32 + pad * 2, src.height() as i32 + pad * 2);
    let mut a = vec![0f32; (w * h) as usize];
    let sw = src.width() as i32;
    for y in 0..src.height() as i32 {
        for x in 0..sw {
            a[((y + pad) * w + x + pad) as usize] = src.data()[((y * sw + x) * 4 + 3) as usize] as f32 / 255.0;
        }
    }
    // 三次盒式模糊 ≈ 高斯
    let r = ((sigma * sigma * 4.0 + 1.0).sqrt() as i32 / 2).max(1);
    let mut tmp = vec![0f32; a.len()];
    for _ in 0..3 {
        box_blur(&a, &mut tmp, w, h, r, true);
        box_blur(&tmp, &mut a, w, h, r, false);
    }
    let mut out = Pixmap::new(w as u32, h as u32)?;
    let (cr, cg, cb) = (((rgb >> 16) & 255) as f32, ((rgb >> 8) & 255) as f32, (rgb & 255) as f32);
    for (i, px) in out.data_mut().as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let al = (a[i] * alpha).clamp(0.0, 1.0);
        px[0] = (cr * al) as u8;
        px[1] = (cg * al) as u8;
        px[2] = (cb * al) as u8;
        px[3] = (al * 255.0) as u8;
    }
    Some((out, pad))
}

fn box_blur(src: &[f32], dst: &mut [f32], w: i32, h: i32, r: i32, horiz: bool) {
    let (outer, inner) = if horiz { (h, w) } else { (w, h) };
    let idx = |o: i32, i: i32| if horiz { (o * w + i) as usize } else { (i * w + o) as usize };
    let norm = 1.0 / (2 * r + 1) as f32;
    for o in 0..outer {
        let mut acc = 0.0;
        for i in -r..=r { if (0..inner).contains(&i) { acc += src[idx(o, i)]; } }
        for i in 0..inner {
            dst[idx(o, i)] = acc * norm;
            let add = i + r + 1;
            let sub = i - r;
            if add < inner { acc += src[idx(o, add)]; }
            if sub >= 0 { acc -= src[idx(o, sub)]; }
        }
    }
}

/// 小 LRU：key → 缩放好的图
/// (key, 尺寸, 图, 最近使用)
type Entry = (String, (u32, u32), Rc<Pixmap>, u64);

pub struct SpriteCache {
    cap: usize,
    tick: u64,
    entries: Vec<Entry>,
}

impl SpriteCache {
    pub fn new(cap: usize) -> Self { Self { cap, tick: 0, entries: Vec::new() } }

    pub fn clear(&mut self) { self.entries.clear(); }

    /// 丢掉键满足条件的缓存图（换皮肤时释放旧皮肤的图）
    pub fn forget(&mut self, pred: impl Fn(&str) -> bool) { self.entries.retain(|e| !pred(&e.0)); }

    pub fn get_or(&mut self, key: &str, size: (u32, u32), make: impl FnOnce() -> Option<Pixmap>) -> Option<Rc<Pixmap>> {
        self.tick += 1;
        if let Some(e) = self.entries.iter_mut().find(|e| e.0 == key && e.1 == size) {
            e.3 = self.tick;
            return Some(e.2.clone());
        }
        let p = Rc::new(make()?);
        if self.entries.len() >= self.cap
            && let Some(i) = self.entries.iter().enumerate().min_by_key(|(_, e)| e.3).map(|(i, _)| i)
        {
            self.entries.swap_remove(i);
        }
        self.entries.push((key.to_string(), size, p.clone(), self.tick));
        Some(p)
    }

    /// 资源图，缩放到 (w,h)
    pub fn asset(&mut self, path: &str, w: u32, h: u32) -> Option<Rc<Pixmap>> {
        self.get_or(path, (w, h), || load_scaled(crate::assets::get(path)?, w, h))
    }

    #[cfg(test)]
    pub fn len(&self) -> usize { self.entries.len() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_plain() {
        let l = layers_for(Face::Idle, "", &[]);
        assert_eq!(l.base, "sprites/layered/idle.png");
        assert!(l.tail && l.star);
        assert_eq!(l.outfit_offset, None);
        let l = layers_for(Face::Happy, "", &[]);
        assert!(l.star && !l.tail);
        let l = layers_for(Face::Shy, "", &[]);
        assert_eq!(l.base, "sprites/shy.png");
        assert!(!l.star && !l.tail);
    }

    #[test]
    fn layers_costume_and_outfit() {
        let l = layers_for(Face::Love, "maid", &["glasses".into()]);
        assert_eq!(l.base, "outfits/maid/happy.png");
        assert!(!l.star && !l.tail);
        assert_eq!(l.outfit_offset, Some((0.0, 0.0)));
        assert_eq!(layers_for(Face::Walk1, "jk", &[]).base, "outfits/jk/idle.png");
        assert_eq!(layers_for(Face::Sit, "", &["glasses".into()]).outfit_offset, Some((0.0, 17.0)));
        assert_eq!(layers_for(Face::Sleep, "", &["glasses".into()]).outfit_offset, None);
        // 戴圣诞帽时藏星星
        assert!(!layers_for(Face::Idle, "", &["santa".into()]).star);
    }

    #[test]
    fn all_sprite_assets_exist() {
        for f in Face::ALL {
            let l = layers_for(f, "", &[]);
            assert!(crate::assets::get(&l.base).is_some(), "{}", l.base);
            for (c, _) in crate::core::costume::COSTUMES {
                let l = layers_for(f, c, &[]);
                assert!(crate::assets::get(&l.base).is_some(), "{c} {}", l.base);
            }
        }
        for p in [TAIL, STAR, "outfits/santa.png", "outfits/glasses.png"] {
            assert!(crate::assets::get(p).is_some(), "{p}");
        }
    }

    #[test]
    fn layers_sailor_default() {
        // 默认水手服保留尾巴/星星分层动画
        let l = layers_for(Face::Idle, "sailor", &[]);
        assert_eq!(l.base, "outfits/sailor/layered/idle.png");
        assert!(l.tail && l.star);
        assert_eq!(layers_for(Face::Walk2, "sailor", &[]).base, "outfits/sailor/layered/walk2.png");
        let l = layers_for(Face::Happy, "sailor", &[]);
        assert!(l.star && !l.tail);
        // 别名：love → happy（分层），stare → think，yawn → idle（分层）
        let l = layers_for(Face::Love, "sailor", &[]);
        assert_eq!(l.base, "outfits/sailor/layered/happy.png");
        assert!(l.star && !l.tail);
        assert_eq!(layers_for(Face::Stare, "sailor", &[]).base, "outfits/sailor/think.png");
        assert_eq!(layers_for(Face::Sleep, "sailor", &[]).base, "outfits/sailor/sleep.png");
        assert!(layers_for(Face::Yawn, "sailor", &[]).tail);
        assert!(!layers_for(Face::Idle, "sailor", &["santa".into()]).star);
        // 原装卫衣："" 和 "hoodie" 一样
        assert_eq!(layers_for(Face::Idle, "hoodie", &[]), layers_for(Face::Idle, "", &[]));
        assert_eq!(layers_for(Face::Shy, "hoodie", &[]).base, "sprites/shy.png");
        assert_eq!(layers_for(Face::Cry, "swim", &[]).base, "outfits/swim/hungry.png");
    }

    #[test]
    fn new_outfit_sprites_are_400x587() {
        for (c, _) in crate::core::costume::COSTUMES {
            for f in Face::ALL {
                let p = layers_for(f, c, &[]).base;
                let png = Pixmap::decode_png(crate::assets::get(&p).unwrap()).unwrap();
                assert_eq!((png.width(), png.height()), (400, 587), "{p}");
            }
        }
    }

    #[test]
    fn cache_evicts() {
        let mut c = SpriteCache::new(2);
        for k in ["a", "b", "c"] {
            c.get_or(k, (1, 1), || Pixmap::new(1, 1));
        }
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn decode_and_scale() {
        let p = SpriteCache::new(2).asset("sprites/idle.png", 200, 293).unwrap();
        assert_eq!((p.width(), p.height()), (200, 293));
        assert!(p.data().as_chunks::<4>().0.iter().any(|px| px[3] == 255));
    }
}
