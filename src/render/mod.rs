//! 渲染层（负责人：C 路「渲染与行为」，集成负责人）
//! tiny-skia 软渲染：立绘 + 尾巴/星星分层动画 + 换装 + 特效 + 气泡 + 状态卡 + zzZ
//! 布局全部按原版 index.html/style.css 的逻辑像素（240×360）算，再整体乘 DPI 缩放。
pub mod anim;
pub mod dance;
pub mod live;
pub mod perf;
pub mod shapes;
pub mod skin_loader;
pub mod sprites;
pub mod text;
pub mod toy;

pub use anim::Xf;
use crate::types::{Face, Frame, WIN_H, WIN_W};
use std::rc::Rc;
use tiny_skia::{BlendMode, FilterQuality, Pixmap, PixmapPaint, Transform};

// ---------------------------------------------------------------- 场景（行为层每帧填）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FxShape {
    Heart,
    /// 0 = ♪，1 = ♫，2 = ♬
    Note(u8),
    Spark,
    Drop,
    Anger,
    Exclaim,
}

/// 一个特效字形。x/y 是元素左上角（#pet 盒子 200×293 内的坐标），size 是字号
#[derive(Clone, Debug, PartialEq)]
pub struct FxSprite {
    pub shape: FxShape,
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub xf: Xf,
    pub color: u32,
}

/// zzZ 的一个字母（窗口坐标，元素左上角）
#[derive(Clone, Debug, PartialEq)]
pub struct ZGlyph {
    pub ch: char,
    pub size: f32,
    pub color: u32,
    pub x: f32,
    pub y: f32,
    pub opacity: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BubbleView {
    pub text: String,
    pub opacity: f32,
    /// 字太多超出 max-height 时的滚动进度 0..1
    pub scroll: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StatusView {
    pub days: u32,
    pub hunger: f32,
    pub mood: f32,
    pub energy: f32,
    pub level: String,
    pub affection: f64,
    pub moyu: Option<u32>,
    /// (连续签到天数, 累计签到天数)
    pub checkin: Option<(u32, u32)>,
    /// 天气一行（如「小雨 12°C」）
    pub weather: Option<String>,
    pub opacity: f32,
}

/// 行为层每帧交给渲染层的场景描述
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub face: Face,
    /// core::costume 的 id（"" / "hoodie" = 原装卫衣立绘，"sailor" = 默认水手服）
    pub costume: String,
    /// santa / glasses（按配置里的顺序叠）
    pub outfits: Vec<String>,
    /// #pet 的动画变换（原点：盒子底边中点）
    pub pet: Xf,
    /// #body 的 scaleX（1 正常，-1 镜像；中间值 = 翻转过渡中）
    pub flip: f32,
    pub tail: Xf,
    pub star: Xf,
    /// 喝了蜂蜜水的金色光晕（drop-shadow）
    pub glow: bool,
    /// 身子朝光标那边歪：头顶水平错切的像素（逻辑像素，向右为正；脚底不动）
    pub lean: f32,
    pub bubble: Option<BubbleView>,
    pub status: Option<StatusView>,
    pub zzz: Vec<ZGlyph>,
    pub fx: Vec<FxSprite>,
    /// 伪 Live2D：呼吸扭曲 + 眨眼/张嘴贴片（全 0 = 静态立绘）
    pub live: live::LiveView,
}

impl Default for Scene {
    fn default() -> Self {
        Self {
            face: Face::Idle,
            costume: String::new(),
            outfits: Vec::new(),
            pet: Xf::ID,
            flip: 1.0,
            tail: Xf::ID,
            star: Xf::ID,
            glow: false,
            lean: 0.0,
            bubble: None,
            status: None,
            zzz: Vec::new(),
            fx: Vec::new(),
            live: live::LiveView::default(),
        }
    }
}

/// 逻辑坐标矩形
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}
impl Rect {
    pub fn contains(&self, x: f32, y: f32) -> bool { x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h }
}

// ---------------------------------------------------------------- 布局常量（style.css）
const PET_X: f32 = 20.0;
const PET_W: f32 = 200.0;
const PET_H: f32 = 293.0;
const PET_Y: f32 = WIN_H as f32 - PET_H;
// 原来 #5a3b2e 在奶白底上偏灰、发虚（电脑上看对比度不够），压深一档
const C_TEXT: u32 = 0x3a2416;
const C_BG: u32 = 0xfffaf0;
/// 气泡/状态卡的边和阴影（原版 #fbbc04 边 + 红绿蓝阴影）
const C_CARD_EDGE: u32 = 0xf2b048;
const C_SHADOW: u32 = 0x7a4a1e;
const C_RED: u32 = 0xea4335;
const C_GREEN: u32 = 0x34a853;
const C_BLUE: u32 = 0x4285f4;
const C_YELLOW: u32 = 0xfbbc04;

/// CSS transform（translate → rotate → scale）绕 origin
pub fn xf_matrix(xf: &Xf, ox: f32, oy: f32) -> Transform {
    Transform::from_translate(ox + xf.tx, oy + xf.ty)
        .pre_concat(Transform::from_rotate(xf.rot))
        .pre_scale(xf.sx, xf.sy)
        .pre_translate(-ox, -oy)
}

#[derive(PartialEq, Eq)]
struct BubbleTextKey {
    scale_bits: u32,
    text: String,
}

#[derive(PartialEq, Eq)]
struct BubbleKey {
    scale_bits: u32,
    text: String,
    scroll_px: i32,
}

#[derive(PartialEq, Eq)]
struct StatusKey {
    scale_bits: u32,
    days: u32,
    hunger: i64,
    mood: i64,
    energy: i64,
    affection_tenth: i64,
    level: String,
    moyu: Option<u32>,
    checkin: Option<(u32, u32)>,
    weather: Option<String>,
}

impl StatusKey {
    fn from(st: &StatusView, scale: f32) -> Self {
        Self {
            scale_bits: scale.to_bits(),
            days: st.days,
            hunger: st.hunger.round().clamp(0.0, 100.0) as i64,
            mood: st.mood.round().clamp(0.0, 100.0) as i64,
            energy: st.energy.round().clamp(0.0, 100.0) as i64,
            affection_tenth: (st.affection * 10.0).round() as i64,
            level: st.level.clone(),
            moyu: st.moyu,
            checkin: st.checkin,
            weather: st.weather.clone(),
        }
    }

    fn matches(&self, st: &StatusView, scale: f32) -> bool {
        self.scale_bits == scale.to_bits()
            && self.days == st.days
            && self.hunger == st.hunger.round().clamp(0.0, 100.0) as i64
            && self.mood == st.mood.round().clamp(0.0, 100.0) as i64
            && self.energy == st.energy.round().clamp(0.0, 100.0) as i64
            && self.affection_tenth == (st.affection * 10.0).round() as i64
            && self.level == st.level
            && self.moyu == st.moyu
            && self.checkin == st.checkin
            && self.weather == st.weather
    }
}

struct BubbleText {
    key: BubbleTextKey,
    lines: Vec<(Option<text::Coverage>, f32)>,
    cell: f32,
}

/// (资源路径, 尺寸, 裁好的图, 在缩放图里的 x, y)
type Crop = (&'static str, (u32, u32), Rc<Pixmap>, f32, f32);

struct Layer<K> {
    key: K,
    pm: Pixmap,
    /// 设备像素位置
    x: i32,
    y: i32,
    rect: Rect,
}

fn clamp_scale(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.1 { scale.min(8.0) } else { 1.0 }
}

pub struct Renderer {
    pm: Option<Pixmap>,
    frame: Frame,
    last: Option<(Scene, f32)>,
    sprites: sprites::SpriteCache,
    skin: skin_loader::SkinCache,
    warper: live::Warper,
    patches: live::PatchCache,
    /// HAKIMI_TRACE=1 时的帧耗时/帧率/CPU 统计
    perf: perf::Perf,
    /// 尾巴/星星图层裁到不透明部分（整张画布大多是透明的，画小图省一大半像素）
    crops: Vec<Crop>,
    text: text::TextEngine,
    bubble: Option<Layer<BubbleKey>>,
    bubble_text: Option<BubbleText>,
    status: Option<Layer<StatusKey>>,
    glyphs: Vec<((char, u32), Rc<text::Coverage>)>,
    pub bubble_rect: Option<Rect>,
    pub status_rect: Option<Rect>,
    /// 气泡文字超出 max-height（行为层据此自动滚动）
    pub bubble_overflow: bool,
}

impl Default for Renderer {
    fn default() -> Self { Self::new() }
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            pm: None,
            frame: Frame { w: 0, h: 0, bgra: Vec::new() },
            last: None,
            sprites: sprites::SpriteCache::new(12),
            skin: skin_loader::SkinCache::default(),
            warper: live::Warper::default(),
            patches: live::PatchCache::default(),
            perf: perf::Perf::new(),
            crops: Vec::new(),
            text: text::TextEngine::new(),
            bubble: None,
            bubble_text: None,
            status: None,
            glyphs: Vec::new(),
            bubble_rect: None,
            status_rect: None,
            bubble_overflow: false,
        }
    }

    /// 画面有变化才返回新帧
    #[allow(dead_code)]
    pub fn render(&mut self, scene: &Scene, scale: f32) -> Option<&Frame> {
        if self.update(scene, scale) { Some(&self.frame) } else { None }
    }

    /// 画一帧到内部缓冲；没变化返回 false
    pub fn update(&mut self, scene: &Scene, scale: f32) -> bool {
        let t0 = self.perf.begin();
        let drawn = self.update_inner(scene, scale);
        self.perf.end(t0, drawn, scene.live.warping(), scale);
        drawn
    }

    fn update_inner(&mut self, scene: &Scene, scale: f32) -> bool {
        let scale = clamp_scale(scale);
        // 换了角色包：旧皮肤的图已释放，这一帧必须重画
        if self.skin.sync(&mut self.sprites) { self.last = None; }
        if let Some((s, sc)) = &self.last
            && s == scene
            && *sc == scale
        {
            return false;
        }
        self.draw(scene, scale);
        match &mut self.last {
            Some((s, sc)) => {
                s.clone_from(scene);
                *sc = scale;
            }
            None => self.last = Some((scene.clone(), scale)),
        }
        let Some(pm) = self.pm.as_ref() else { return false };
        let (w, h) = (pm.width(), pm.height());
        self.frame.w = w;
        self.frame.h = h;
        self.frame.bgra.resize((w * h * 4) as usize, 0);
        for (d, s) in self.frame.bgra.as_chunks_mut::<4>().0.iter_mut().zip(pm.data().as_chunks::<4>().0.iter()) {
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
            d[3] = s[3];
        }
        true
    }

    pub fn frame(&self) -> &Frame { &self.frame }

    /// 只画到内部 Pixmap（测试用）
    pub fn draw(&mut self, scene: &Scene, scale: f32) {
        let scale = clamp_scale(scale);
        self.skin.sync(&mut self.sprites);
        let w = (WIN_W as f32 * scale).round() as u32;
        let h = (WIN_H as f32 * scale).round() as u32;
        if self.pm.as_ref().is_none_or(|p| p.width() != w || p.height() != h) {
            self.pm = Pixmap::new(w, h);
            self.sprites.clear();
            self.patches.clear();
            self.crops.clear();
            self.bubble = None;
            self.bubble_text = None;
            self.status = None;
            self.glyphs.clear();
        }
        let Some(mut pm) = self.pm.take() else { return };
        pm.fill(tiny_skia::Color::TRANSPARENT);
        let root = Transform::from_scale(scale, scale);

        self.draw_zzz(&mut pm, scene, scale);
        self.draw_pet(&mut pm, scene, scale, root);
        self.draw_fx(&mut pm, scene, root);
        self.draw_bubble(&mut pm, scene, scale);
        self.draw_status(&mut pm, scene, scale);
        self.pm = Some(pm);
    }

    #[allow(dead_code)]
    pub fn pixmap(&self) -> Option<&Pixmap> { self.pm.as_ref() }

    fn draw_pet(&mut self, pm: &mut Pixmap, scene: &Scene, scale: f32, root: Transform) {
        let (sw, sh) = ((PET_W * scale).round() as u32, (PET_H * scale).round() as u32);
        // 快速路径：没有旋转/缩放/歪头时，立绘贴底对齐到整像素直接拷进窗口（不再二次双线性 → 高倍缩放不发虚，也更省）
        let snap = snapped_origin(scene, scale, sw, sh);
        let mirror = scene.flip < 0.0;
        let body = match snap {
            Some((x, y)) => {
                // 盒子坐标 → 设备像素（缩放 = 立绘缩放图 / 盒子），镜像绕盒子中线
                Transform::from_row(sw as f32 / PET_W, 0.0, 0.0, sh as f32 / PET_H, x as f32, y as f32)
                    .pre_translate(PET_W / 2.0, 0.0)
                    .pre_scale(scene.flip.signum(), 1.0)
                    .pre_translate(-PET_W / 2.0, 0.0)
            }
            None => {
                let pet = root.pre_concat(xf_matrix(&scene.pet, PET_X + PET_W / 2.0, WIN_H as f32));
                // 歪头看光标：以脚底为轴水平错切，头顶偏 lean 像素
                let pet = if scene.lean.abs() > 0.01 {
                    let k = -scene.lean / PET_H;
                    pet.pre_concat(Transform::from_row(1.0, 0.0, k, 1.0, -k * WIN_H as f32, 0.0))
                } else {
                    pet
                };
                let cx = PET_X + PET_W / 2.0;
                let cy = PET_Y + PET_H / 2.0;
                pet.pre_translate(cx, cy).pre_scale(scene.flip, 1.0).pre_translate(-cx, -cy).pre_translate(PET_X, PET_Y)
            }
        };
        let to_box = |p: &Pixmap| Transform::from_scale(PET_W / p.width() as f32, PET_H / p.height() as f32);
        let paint = PixmapPaint { quality: FilterQuality::Bilinear, ..Default::default() };

        let layers = sprites::layers_for(scene.face, &scene.costume, &scene.outfits);
        // 角色包：当前皮肤有这个表情就用它（不画哈基米专属的尾巴/星星/配件），没有就回退内置
        let skin = self.skin.base(&mut self.sprites, scene.face, sw, sh);
        let layers = match &skin { Some((k, _)) => skin_loader::strip_layers(layers, k), None => layers };
        let skinned = skin.is_some();
        let base = skin.map(|s| s.1).or_else(|| self.sprites.asset(&layers.base, sw, sh));
        if let Some(base) = &base {
            if scene.glow {
                let key = format!("glow:{}", layers.base);
                let glow = self.sprites.get_or(&key, (sw, sh), || {
                    let (g, pad) = sprites::make_glow(base, 4.0 * scale, 0xfbbc04, 0.9)?;
                    // 把外扩量编码进宽度差里，画的时候再算回来
                    let _ = pad;
                    Some(g)
                });
                if let Some(g) = glow {
                    let pad = (g.width().saturating_sub(base.width()) / 2) as i32;
                    // 光晕跟着呼吸扭曲一起动（以前是静止的一圈，身子起伏时会露馅）
                    match snap {
                        Some((x, y)) => self.warper.warp_blit(&g, &scene.live, PET_H, pad as u32, pm, x - pad, y - pad, mirror, 1.0),
                        None => {
                            let t = body.pre_concat(to_box(base)).pre_translate(-pad as f32, -pad as f32);
                            if scene.live.warping()
                                && let Some(w) = self.warper.warp_padded(&g, &scene.live, PET_H, pad as u32)
                            {
                                pm.draw_pixmap(0, 0, w.as_ref(), &paint, t, None);
                            } else {
                                pm.draw_pixmap(0, 0, g.as_ref().as_ref(), &paint, t, None);
                            }
                        }
                    }
                }
            }
            let m = body.pre_concat(to_box(base));
            // 表情交叉淡化：新表情画满，旧表情按 prev_alpha 盖在上面（重叠处 = 线性混合，旧的独有部分淡出）
            let prev = match scene.live.prev_face {
                Some(pf) if scene.live.prev_alpha > 0.003 && !skinned => {
                    let pl = sprites::layers_for(pf, &scene.costume, &scene.outfits);
                    if pl.base == layers.base { None } else { self.sprites.asset(&pl.base, sw, sh) }
                }
                _ => None,
            };
            for (img, op) in [(Some(base.clone()), 1.0), (prev, scene.live.prev_alpha)] {
                let Some(img) = img else { continue };
                if let Some((x, y)) = snap {
                    self.warper.warp_blit(&img, &scene.live, PET_H, 0, pm, x, y, mirror, op);
                    continue;
                }
                let p = PixmapPaint { opacity: op, ..paint };
                if scene.live.warping()
                    && let Some(w) = self.warper.warp(&img, &scene.live, PET_H)
                {
                    pm.draw_pixmap(0, 0, w.as_ref(), &p, m, None);
                } else {
                    pm.draw_pixmap(0, 0, img.as_ref().as_ref(), &p, m, None);
                }
            }
            // 眨眼 / 张嘴贴片：脸那一块随扭曲整体平移
            let lv = &scene.live;
            if (lv.blink > 0.003 || lv.mouth > 0.003) && skinned {
                // 角色包：用包里和底图同尺寸的 blink / mouthOpen 叠加层，不盖哈基米的贴片
                let head = body.pre_translate(0.0, -live::row_lift(lv, live::FACE_V));
                for (name, a) in [("blink", lv.blink), ("mouthOpen", lv.mouth)] {
                    if a > 0.003 && let Some(o) = self.skin.overlay(&mut self.sprites, name, sw, sh) {
                        let pp = PixmapPaint { opacity: a.min(1.0), ..paint };
                        pm.draw_pixmap(0, 0, o.as_ref().as_ref(), &pp, head.pre_concat(to_box(&o)), None);
                    }
                }
            } else if lv.blink > 0.003 || lv.mouth > 0.003 {
                let pt = self.patches.get(scene.face, &scene.costume, (sw, sh));
                let head = body.pre_translate(0.0, -live::row_lift(lv, live::FACE_V)).pre_concat(to_box(base));
                for (p, a) in [(&pt.blink, lv.blink), (&pt.mouth, lv.mouth)] {
                    if let Some(p) = p && a > 0.003 {
                        let pp = PixmapPaint { opacity: a.min(1.0), ..paint };
                        pm.draw_pixmap(0, 0, p.pm.as_ref(), &pp, head.pre_translate(p.x as f32, p.y as f32), None);
                    }
                }
            }
        }
        let full_box = Transform::from_scale(PET_W / sw as f32, PET_H / sh as f32);
        if layers.tail
            && let Some((t, ox, oy)) = self.cropped(sprites::TAIL, sw, sh)
        {
            let (dx, dy) = live::offset_at(&scene.live, 0.6, 0.797);
            let m = body.pre_translate(dx, dy).pre_concat(xf_matrix(&scene.tail, 0.6 * PET_W, 0.797 * PET_H)).pre_concat(full_box).pre_translate(ox, oy);
            pm.draw_pixmap(0, 0, t.as_ref().as_ref(), &paint, m, None);
        }
        if layers.star
            && let Some((s, ox, oy)) = self.cropped(sprites::STAR, sw, sh)
        {
            let lift = live::row_lift(&scene.live, 0.123);
            let m = body.pre_translate(0.0, -lift).pre_concat(xf_matrix(&scene.star, 0.395 * PET_W, 0.123 * PET_H)).pre_concat(full_box).pre_translate(ox, oy);
            pm.draw_pixmap(0, 0, s.as_ref().as_ref(), &paint, m, None);
            if scene.star.bright > 1.001 {
                // brightness(b) ≈ 再叠一层 (b-1) 的加色
                let p = PixmapPaint { opacity: (scene.star.bright - 1.0).min(1.0), blend_mode: BlendMode::Plus, quality: FilterQuality::Bilinear };
                pm.draw_pixmap(0, 0, s.as_ref().as_ref(), &p, m, None);
            }
        }
        if let Some((ox, oy)) = layers.outfit_offset {
            for id in &scene.outfits {
                let path = format!("outfits/{id}.png");
                if let Some(o) = self.sprites.asset(&path, sw, sh) {
                    if let Some((x, y)) = snap {
                        let (dx, dy) = ((ox / 400.0 * sw as f32).round() as i32, (oy / 587.0 * sh as f32).round() as i32);
                        self.warper.warp_blit(&o, &scene.live, PET_H, 0, pm, x + if mirror { -dx } else { dx }, y + dy, mirror, 1.0);
                        continue;
                    }
                    let m = body.pre_translate(ox / 400.0 * PET_W, oy / 587.0 * PET_H).pre_concat(to_box(&o));
                    if scene.live.warping()
                        && let Some(w) = self.warper.warp(&o, &scene.live, PET_H)
                    {
                        pm.draw_pixmap(0, 0, w.as_ref(), &paint, m, None);
                    } else {
                        pm.draw_pixmap(0, 0, o.as_ref().as_ref(), &paint, m, None);
                    }
                }
            }
        }
    }

    /// 图层裁到不透明部分：(小图, 在缩放图里的 x, y)
    fn cropped(&mut self, path: &'static str, w: u32, h: u32) -> Option<(Rc<Pixmap>, f32, f32)> {
        if let Some(c) = self.crops.iter().find(|c| c.0 == path && c.1 == (w, h)) { return Some((c.2.clone(), c.3, c.4)); }
        let full = self.sprites.asset(path, w, h)?;
        let (fw, fh) = (full.width() as usize, full.height() as usize);
        let d = full.data();
        let (mut x0, mut y0, mut x1, mut y1) = (fw, fh, 0, 0);
        for y in 0..fh {
            for x in 0..fw {
                if d[(y * fw + x) * 4 + 3] > 0 { x0 = x0.min(x); y0 = y0.min(y); x1 = x1.max(x); y1 = y1.max(y); }
            }
        }
        let (c, off) = if x1 < x0 {
            (full.clone(), (0.0, 0.0))
        } else {
            // 四周留 1px 透明，双线性采样边缘不被截断
            let (x0, y0, x1, y1) = (x0.saturating_sub(1), y0.saturating_sub(1), (x1 + 1).min(fw - 1), (y1 + 1).min(fh - 1));
            let r = tiny_skia::IntRect::from_ltrb(x0 as i32, y0 as i32, x1 as i32 + 1, y1 as i32 + 1)?;
            (Rc::new(full.clone_rect(r)?), (x0 as f32, y0 as f32))
        };
        self.crops.push((path, (w, h), c.clone(), off.0, off.1));
        Some((c, off.0, off.1))
    }

    fn draw_fx(&mut self, pm: &mut Pixmap, scene: &Scene, root: Transform) {
        if scene.fx.is_empty() { return; }
        let pet = root.pre_concat(xf_matrix(&scene.pet, PET_X + PET_W / 2.0, WIN_H as f32)).pre_translate(PET_X, PET_Y);
        for f in &scene.fx {
            if f.xf.opacity <= 0.003 { continue; }
            // 元素盒：宽约 1em，高 = 行高（YaHei normal ≈ 1.32em），变换原点在盒中心
            let (bw, bh) = (f.size * 0.9, f.size * 1.32);
            // 跟着立绘一起镜像（转身过程中 flip 在 -1..1 之间平滑变化），眼泪/汗滴才贴得住脸
            let (cx, cy) = (PET_W / 2.0 + (f.x + bw / 2.0 - PET_W / 2.0) * scene.flip, f.y + bh / 2.0);
            let t = pet.pre_concat(xf_matrix(&f.xf, cx, cy)).pre_translate(cx, cy).pre_scale(f.size, f.size);
            shapes::draw_glyph(pm, f.shape, f.color, f.xf.opacity, t, f.size);
        }
    }

    fn glyph(&mut self, ch: char, px: f32) -> Option<Rc<text::Coverage>> {
        let k = (ch, (px * 4.0) as u32);
        if let Some((_, c)) = self.glyphs.iter().find(|(kk, _)| *kk == k) { return Some(c.clone()); }
        let c = Rc::new(self.text.raster(&ch.to_string(), px, true)?);
        if self.glyphs.len() > 8 { self.glyphs.remove(0); }
        self.glyphs.push((k, c.clone()));
        Some(c)
    }

    fn draw_zzz(&mut self, pm: &mut Pixmap, scene: &Scene, scale: f32) {
        for z in &scene.zzz {
            if z.opacity <= 0.003 { continue; }
            if let Some(c) = self.glyph(z.ch, z.size * scale) {
                text::blit(pm, &c, ((150.0 + z.x) * scale).round() as i32, ((120.0 + z.y) * scale).round() as i32, z.color, z.opacity, None);
            }
        }
    }

    // ------------------------------------------------------------ 气泡
    fn draw_bubble(&mut self, pm: &mut Pixmap, scene: &Scene, scale: f32) {
        let Some(b) = &scene.bubble else {
            self.bubble_rect = None;
            self.bubble_overflow = false;
            self.bubble = None;
            self.bubble_text = None;
            return;
        };
        if b.opacity <= 0.003 || b.text.is_empty() {
            self.bubble_rect = None;
            self.bubble_overflow = false;
            self.bubble = None;
            self.bubble_text = None;
            return;
        }
        let s = scale;
        let px = 14.0 * s;
        let lh = 14.0 * 1.35 * s;
        let content_w = 200.0 * s;
        let sbits = s.to_bits();
        if self.bubble_text.as_ref().is_none_or(|t| t.key.scale_bits != sbits || t.key.text != b.text) {
            let lines = self.text.wrap(&b.text, content_w, px, false);
            let cell = self.text.cell_height(px, false);
            let mut out = Vec::new();
            for line in &lines {
                let lw = self.text.text_width(line, px, false);
                out.push((self.text.raster(line, px, false), 12.0 * s + (content_w - lw) / 2.0));
            }
            self.bubble_text = Some(BubbleText { key: BubbleTextKey { scale_bits: sbits, text: b.text.clone() }, lines: out, cell });
        }
        let Some(bt) = self.bubble_text.as_ref() else { return };
        let full_h = bt.lines.len() as f32 * lh;
        let content_h = full_h.min(128.0 * s);
        self.bubble_overflow = full_h > content_h + 0.5;
        let scroll_px = (b.scroll.clamp(0.0, 1.0) * (full_h - content_h).max(0.0)).round() as i32;
        let need_rebuild = self.bubble.as_ref().is_none_or(|l| l.key.scale_bits != sbits || l.key.scroll_px != scroll_px || l.key.text != b.text);
        if need_rebuild {
            let key = BubbleKey { scale_bits: sbits, text: b.text.clone(), scroll_px };
            self.bubble = self.build_bubble_from_cache(scroll_px as f32, full_h, content_h, lh, s)
                .map(|(pm, rect)| Layer { key, pm, x: (8.0 * s).round() as i32, y: 0, rect });
        }
        if let Some(l) = &self.bubble {
            let p = PixmapPaint { opacity: b.opacity, ..Default::default() };
            pm.draw_pixmap(l.x, l.y, l.pm.as_ref(), &p, Transform::identity(), None);
            self.bubble_rect = Some(l.rect);
        }
    }

    /// 气泡：left/right 8px、border 2、padding 6/10 → 内容宽 200；14px 行高 1.35；内容最多 128 高
    fn build_bubble_from_cache(&self, scroll_px: f32, _full_h: f32, content_h: f32, lh: f32, s: f32) -> Option<(Pixmap, Rect)> {
        let bt = self.bubble_text.as_ref()?;
        let box_w = 224.0 * s;
        let box_h = content_h + 16.0 * s;
        let mut pm = Pixmap::new(box_w.ceil() as u32 + 1, (box_h + 6.0 * s).ceil() as u32 + 1)?;
        draw_card(&mut pm, box_w, box_h, s, 1.0);
        let (top, bottom) = ((8.0 * s).round() as i32, (8.0 * s + content_h).round() as i32);
        for (i, (cov, x)) in bt.lines.iter().enumerate() {
            let y = 8.0 * s + i as f32 * lh - scroll_px;
            if y + lh < top as f32 || y > bottom as f32 { continue; }
            if let Some(c) = cov {
                let clip = Some((0, top, pm.width() as i32, bottom));
                text::blit(&mut pm, c, x.round() as i32, (y + (lh - bt.cell) / 2.0).round() as i32, C_TEXT, 1.0, clip);
            }
        }
        Some((pm, Rect { x: 8.0, y: 0.0, w: 224.0, h: box_h / s }))
    }

    // ------------------------------------------------------------ 状态卡
    fn draw_status(&mut self, pm: &mut Pixmap, scene: &Scene, scale: f32) {
        let Some(st) = &scene.status else {
            self.status_rect = None;
            self.status = None;
            return;
        };
        if st.opacity <= 0.003 {
            self.status_rect = None;
            self.status = None;
            return;
        }
        if self.status.as_ref().is_none_or(|l| !l.key.matches(st, scale)) {
            let key = StatusKey::from(st, scale);
            self.status = self.build_status(st, scale).map(|(pm, rect)| Layer { key, pm, x: (14.0 * scale).round() as i32, y: (46.0 * scale).round() as i32, rect });
        }
        if let Some(l) = &self.status {
            let p = PixmapPaint { opacity: st.opacity, ..Default::default() };
            pm.draw_pixmap(l.x, l.y, l.pm.as_ref(), &p, Transform::identity(), None);
            self.status_rect = Some(l.rect);
        }
    }

    /// 状态卡：top 46、左右 14、border 2、padding 8/10 → 内容宽 188；12px
    fn build_status(&mut self, st: &StatusView, s: f32) -> Option<(Pixmap, Rect)> {
        let px = 12.0 * s;
        let lh = self.text.cell_height(px, false).max(px * 1.2);
        let lhb = self.text.cell_height(px, true).max(px * 1.2);
        let box_w = 212.0 * s;
        let rows = 4 + st.moyu.is_some() as usize + st.checkin.is_some() as usize + st.weather.is_some() as usize;
        let content_h = lhb + 4.0 * s + rows as f32 * lh + (rows as f32 - 1.0) * 3.0 * s + 3.0 * s;
        let box_h = content_h + 20.0 * s;
        let mut pm = Pixmap::new(box_w.ceil() as u32 + 1, (box_h + 6.0 * s).ceil() as u32 + 1)?;
        draw_card(&mut pm, box_w, box_h, s, 0.96);
        let x0 = 12.0 * s;
        let cw = 188.0 * s;
        let mut y = 10.0 * s;
        // 标题
        let title = format!("哈基米 · 认识第 {} 天", st.days);
        let tw = self.text.text_width(&title, px, true);
        self.text_at(&mut pm, &title, x0 + (cw - tw) / 2.0, y, lhb, px, true, C_TEXT);
        y += lhb + 4.0 * s;
        let bars = [("饱食", st.hunger, C_YELLOW), ("心情", st.mood, C_RED), ("精力", st.energy, C_GREEN)];
        for (label, v, color) in bars {
            self.text_at(&mut pm, label, x0, y, lh, px, false, C_TEXT);
            let bx = x0 + 34.0 * s;
            let bw = cw - 34.0 * s - 30.0 * s;
            let by = y + (lh - 8.0 * s) / 2.0;
            let id = Transform::identity();
            shapes::fill_rrect(&mut pm, bx, by, bw, 8.0 * s, 4.0 * s, 0xf1e6d6, 1.0, id);
            let clamped = v.round().clamp(0.0, 100.0);
            let fw = bw * (clamped / 100.0);
            if fw > 0.5 { shapes::fill_rrect(&mut pm, bx, by, fw, 8.0 * s, 4.0 * s, color, 1.0, id); }
            let n = format!("{}", clamped as i64);
            let nw = self.text.text_width(&n, px, false);
            self.text_at(&mut pm, &n, x0 + cw - nw, y, lh, px, false, C_TEXT);
            y += lh + 3.0 * s;
        }
        self.text_at(&mut pm, "亲密", x0, y, lh, px, false, C_TEXT);
        self.text_at(&mut pm, &st.level, x0 + 34.0 * s, y, lh, px, true, C_BLUE);
        let n = if (st.affection - st.affection.round()).abs() < 1e-3 {
            format!("{}", st.affection.round() as i64)
        } else {
            format!("{:.1}", st.affection)
        };
        let nw = self.text.text_width(&n, px, false);
        self.text_at(&mut pm, &n, x0 + cw - nw, y, lh, px, false, C_TEXT);
        y += lh + 3.0 * s;
        if let Some((streak, total)) = st.checkin {
            self.text_at(&mut pm, "签到", x0, y, lh, px, false, C_TEXT);
            self.text_at(&mut pm, &format!("连续 {streak} 天"), x0 + 34.0 * s, y, lh, px, true, C_GREEN);
            let n = format!("累计 {total}");
            let nw = self.text.text_width(&n, px, false);
            self.text_at(&mut pm, &n, x0 + cw - nw, y, lh, px, false, C_TEXT);
            y += lh + 3.0 * s;
        }
        if let Some(w) = &st.weather {
            self.text_at(&mut pm, "天气", x0, y, lh, px, false, C_TEXT);
            self.text_at(&mut pm, w, x0 + 34.0 * s, y, lh, px, true, C_BLUE);
            y += lh + 3.0 * s;
        }
        if let Some(m) = st.moyu {
            self.text_at(&mut pm, "摸鱼", x0, y, lh, px, false, C_TEXT);
            self.text_at(&mut pm, &format!("今天 {m} 分钟"), x0 + 34.0 * s, y, lh, px, true, C_RED);
        }
        let _ = y;
        Some((pm, Rect { x: 14.0, y: 46.0, w: 212.0, h: box_h / s }))
    }

    #[allow(clippy::too_many_arguments)]
    fn text_at(&mut self, pm: &mut Pixmap, s: &str, x: f32, y: f32, lh: f32, px: f32, bold: bool, color: u32) {
        let cell = self.text.cell_height(px, bold);
        if let Some(c) = self.text.raster(s, px, bold) {
            text::blit(pm, &c, x.round() as i32, (y + (lh - cell) / 2.0).round() as i32, color, 1.0, None);
        }
    }
}

/// 立绘的变换是不是「纯平移（可水平镜像）」：是就返回立绘缩放图在窗口里贴底、居中、对齐整像素的左上角（设备像素）。
/// 翻转过渡中、旋转、缩放、歪头时返回 None，走通用的 tiny-skia 变换
fn snapped_origin(scene: &Scene, scale: f32, sw: u32, sh: u32) -> Option<(i32, i32)> {
    let p = &scene.pet;
    let plain = p.rot.abs() < 1e-3 && (p.sx - 1.0).abs() < 1e-4 && (p.sy - 1.0).abs() < 1e-4;
    if !plain || scene.lean.abs() > 0.01 || (scene.flip.abs() - 1.0).abs() > 1e-4 { return None; }
    let x = ((PET_X + p.tx) * scale + (PET_W * scale - sw as f32) / 2.0).round() as i32;
    let y = ((WIN_H as f32 + p.ty) * scale).round() as i32 - sh as i32;
    Some((x, y))
}

/// 卡片：圆角 12、2px 暖橙边、#fffaf0 底、下方一层柔和的浅棕阴影（和设置窗口同一套奶油/橙色调）。
/// 原版是红/绿/蓝三层错开的彩色 box-shadow，缩小后像花边，2026-11-20 按用户要求换掉。
fn draw_card(pm: &mut Pixmap, w: f32, h: f32, s: f32, bg_alpha: f32) {
    let id = Transform::identity();
    let r = 12.0 * s;
    // 由外到内三层半透明叠出一个软阴影（画布下方只多留了 6px）
    for (inset, dy, a) in [(2.0, 5.5, 0.06), (1.0, 4.0, 0.08), (0.5, 2.5, 0.12)] {
        shapes::fill_rrect(pm, inset * s, dy * s, w - 2.0 * inset * s, h, r, C_SHADOW, a, id);
    }
    shapes::fill_rrect(pm, 0.0, 0.0, w, h, r, C_CARD_EDGE, 1.0, id);
    // 内底：先挖空再填，半透明背景下边框不透出来
    if let Some(p) = shapes::rrect(2.0 * s, 2.0 * s, w - 4.0 * s, h - 4.0 * s, r - 2.0 * s) {
        let mut clear = shapes::paint(0, 1.0);
        clear.blend_mode = BlendMode::Clear;
        pm.fill_path(&p, &clear, tiny_skia::FillRule::Winding, id, None);
        pm.fill_path(&p, &shapes::paint(C_BG, bg_alpha), tiny_skia::FillRule::Winding, id, None);
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    #[test]
    fn transform_origin() {
        // 绕底边中点缩放：原点不动
        let m = xf_matrix(&Xf { sx: 1.5, sy: 0.5, ..Xf::ID }, 120.0, 360.0);
        let mut p = [tiny_skia::Point::from_xy(120.0, 360.0)];
        m.map_points(&mut p);
        assert!((p[0].x - 120.0).abs() < 1e-3 && (p[0].y - 360.0).abs() < 1e-3);
        let m = xf_matrix(&Xf { ty: -18.0, ..Xf::ID }, 0.0, 0.0);
        let mut p = [tiny_skia::Point::from_xy(5.0, 5.0)];
        m.map_points(&mut p);
        assert!((p[0].y + 13.0).abs() < 1e-3);
    }

    #[test]
    fn renders_and_skips_unchanged() {
        let mut r = Renderer::new();
        let mut sc = Scene::default();
        sc.bubble = Some(BubbleView { text: "哈基米上线啦喵～".into(), opacity: 1.0, scroll: 0.0 });
        let f = r.render(&sc, 1.5).expect("first frame");
        assert_eq!((f.w, f.h), (360, 540));
        assert!(f.bgra.as_chunks::<4>().0.iter().any(|p| p[3] == 255));
        assert!(r.render(&sc, 1.5).is_none());
        sc.pet.ty = -3.0;
        assert!(r.render(&sc, 1.5).is_some());
        assert!(r.bubble_rect.is_some());
    }

    /// 手动看效果：HAKIMI_SNAP=out.png cargo test snapshot -- --ignored
    #[test]
    #[ignore]
    fn snapshot() {
        let out = std::env::var("HAKIMI_SNAP").unwrap_or_else(|_| "snap.png".into());
        let mut r = Renderer::new();
        let mut sc = Scene::default();
        sc.outfits = vec!["glasses".into()];
        sc.glow = true;
        sc.star.bright = 1.2;
        sc.tail.rot = 6.0;
        sc.bubble = Some(BubbleView { text: "哈基米～哈基米～哈基米～哈基米哦南北绿豆～ 阿西嘎～阿西嘎～哈呀库奶龙～曼波！🎵 Hello world, this is a long English sentence.".into(), opacity: 1.0, scroll: 0.0 });
        sc.zzz = vec![ZGlyph { ch: 'z', size: 14.0, color: C_BLUE, x: 5.0, y: -10.0, opacity: 1.0 }, ZGlyph { ch: 'Z', size: 24.0, color: C_RED, x: 20.0, y: -40.0, opacity: 0.7 }];
        let shapes = [FxShape::Heart, FxShape::Note(0), FxShape::Note(1), FxShape::Note(2), FxShape::Spark, FxShape::Drop, FxShape::Anger, FxShape::Exclaim];
        sc.fx = shapes.iter().enumerate().map(|(i, &sh)| FxSprite { shape: sh, x: 10.0 + i as f32 * 22.0, y: 150.0, size: 20.0, xf: Xf::ID, color: [C_RED, C_BLUE, C_GREEN, C_YELLOW][i % 4] }).collect();
        sc.status = Some(StatusView { days: 12, hunger: 66.0, mood: 80.0, energy: 33.0, level: "好朋友".into(), affection: 88.0, moyu: Some(23), checkin: Some((3, 10)), weather: Some("小雨 12°C".into()), opacity: 1.0 });
        r.draw(&sc, 1.5);
        r.pixmap().unwrap().save_png(&out).unwrap();
        let mut sc2 = Scene::default();
        sc2.face = Face::Sit;
        sc2.costume = "maid".into();
        sc2.outfits = vec!["santa".into()];
        sc2.pet = Xf { rot: -6.0, ..Xf::ID };
        sc2.flip = -1.0;
        sc2.bubble = Some(BubbleView { text: "欢迎回来，主人喵～".into(), opacity: 0.8, scroll: 0.0 });
        r.draw(&sc2, 1.0);
        r.pixmap().unwrap().save_png(out.replace(".png", "2.png")).unwrap();
    }

    /// 手动看贴片对位：HAKIMI_SNAP=dir cargo test live_snapshot -- --ignored
    /// 每个有贴片的表情出一张：左 原图 / 中 闭眼 / 右 张嘴（脸部裁剪放大）
    #[test]
    #[ignore]
    fn live_snapshot() {
        let dir = std::env::var("HAKIMI_SNAP").unwrap_or_else(|_| ".".into());
        let mut r = Renderer::new();
        let faces = [(Face::Idle, ""), (Face::Happy, ""), (Face::Think, ""), (Face::Shy, ""), (Face::Love, ""), (Face::Surprised, ""), (Face::Hungry, ""), (Face::Stare, ""), (Face::Walk1, ""),
            (Face::Idle, "maid"), (Face::Love, "maid"), (Face::Think, "maid"), (Face::Hungry, "maid"), (Face::Idle, "xmas"), (Face::Think, "jk")];
        let scale = 3.0;
        let (cx, cy, cw, ch) = ((PET_X + 40.0) * scale, (PET_Y + 70.0) * scale, 110.0 * scale, 75.0 * scale);
        let mut sheet = Pixmap::new((cw * 3.0) as u32, (ch * faces.len() as f32) as u32).unwrap();
        sheet.fill(tiny_skia::Color::WHITE);
        for (row, (f, costume)) in faces.iter().enumerate() {
            for (col, (b, m, d)) in [(0.0, 0.0, 0.0), (1.0, 0.0, 1.0), (0.0, 1.0, 1.0)].iter().enumerate() {
                let mut sc = Scene::default();
                sc.face = *f;
                sc.costume = costume.to_string();
                sc.live = live::LiveView { depth: *d, breath: 0.4, bob: 0.25, sway: 0.25, blink: *b, mouth: *m, sleep: false, ..Default::default() };
                r.draw(&sc, scale);
                let src = r.pixmap().unwrap();
                let crop = src.clone_rect(tiny_skia::IntRect::from_xywh(cx as i32, cy as i32, cw as u32, ch as u32).unwrap()).unwrap();
                sheet.draw_pixmap((col as f32 * cw) as i32, (row as f32 * ch) as i32, crop.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
            }
        }
        sheet.save_png(format!("{dir}/live_sheet.png")).unwrap();
        // 扭曲幅度：吸气顶点 vs 呼气、发梢左右
        let mut sc = Scene::default();
        let mut strip = Pixmap::new((WIN_W as f32 * 2.0 * 3.0) as u32, (WIN_H as f32 * 2.0) as u32).unwrap();
        strip.fill(tiny_skia::Color::WHITE);
        for (i, (br, sw)) in [(0.0, 0.25), (0.4, 0.75), (0.4, 0.25)].iter().enumerate() {
            sc.live = live::LiveView { depth: 1.0, breath: *br, bob: 0.0, sway: *sw, blink: 0.0, mouth: 0.0, sleep: false, ..Default::default() };
            r.draw(&sc, 2.0);
            strip.draw_pixmap((i as f32 * WIN_W as f32 * 2.0) as i32, 0, r.pixmap().unwrap().as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        }
        strip.save_png(format!("{dir}/live_warp.png")).unwrap();
    }

    #[test]
    fn live_overlays_draw_and_change_frame() {
        let mut r = Renderer::new();
        let mut sc = Scene::default();
        r.draw(&sc, 1.0);
        let a = r.pixmap().unwrap().clone();
        sc.live.blink = 1.0;
        r.draw(&sc, 1.0);
        let b = r.pixmap().unwrap().clone();
        assert_ne!(a.data(), b.data(), "blink patch drew nothing");
        // 只改了脸那一块：脚底一行不变
        let w = a.width() as usize;
        let foot = (a.height() as usize - 20) * w * 4;
        assert_eq!(&a.data()[foot..foot + w * 4], &b.data()[foot..foot + w * 4]);
        // 扭曲开着但在呼气末、头不点：头顶和脚底逐像素不变（只有发梢/衣摆在动）
        sc.live = live::LiveView { depth: 1.0, ..Default::default() };
        r.draw(&sc, 1.0);
        let c = r.pixmap().unwrap().clone();
        let row = |p: &Pixmap, y: f32| { let y = y as usize * w * 4; p.data()[y..y + w * 4].to_vec() };
        for y in [PET_Y + 30.0, PET_Y + 60.0, WIN_H as f32 - 20.0, WIN_H as f32 - 5.0] {
            assert_eq!(row(&a, y), row(&c, y), "row {y}");
        }
        // 吸气顶点：头整体上移
        sc.live.breath = 0.4;
        r.draw(&sc, 1.0);
        let d = r.pixmap().unwrap().clone();
        assert_ne!(row(&a, PET_Y + 60.0), row(&d, PET_Y + 60.0));
        assert_eq!(row(&a, WIN_H as f32 - 5.0), row(&d, WIN_H as f32 - 5.0));
    }

    /// 手动测性能：cargo test --release bench -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench() {
        let mut r = Renderer::new();
        let mut sc = Scene::default();
        sc.outfits = vec!["glasses".into()];
        sc.bubble = Some(BubbleView { text: "哈基米～哈基米～哈基米～哈基米哦南北绿豆～".into(), opacity: 1.0, scroll: 0.0 });
        let t = std::time::Instant::now();
        let n = 300;
        for i in 0..n {
            let p = i as f32 / 60.0;
            sc.pet = anim::breathe(p.fract());
            sc.tail = anim::tail_sway(p.fract());
            sc.star = anim::star_bob(p.fract());
            sc.face = if i % 50 < 2 { Face::Blink } else { Face::Idle };
            r.update(&sc, 1.5);
        }
        eprintln!("avg frame {:?}", t.elapsed() / n);
        // 伪 Live2D：扭曲 + 贴片
        let t = std::time::Instant::now();
        for i in 0..n {
            let p = i as f32 / 60.0;
            sc.face = Face::Idle;
            sc.pet = Xf::ID;
            sc.live = live::LiveView { depth: 1.0, breath: p.fract(), bob: (p * 0.8).fract(), sway: (p * 0.7).fract(), blink: if i % 50 < 3 { 1.0 } else { 0.0 }, mouth: (i % 4) as f32 / 3.0, sleep: false, ..Default::default() };
            r.update(&sc, 1.5);
        }
        eprintln!("avg live frame {:?}", t.elapsed() / n);
        // 桌面上的常见情况：100% 缩放、没有气泡/配件
        for scale in [1.0f32, 1.25] {
            let mut r = Renderer::new();
            let mut sc = Scene::default();
            let t = std::time::Instant::now();
            for i in 0..n {
                sc.pet = anim::breathe((i as f32 / 13.0).fract());
                sc.tail = anim::tail_sway((i as f32 / 40.0).fract());
                sc.star = anim::star_bob((i as f32 / 50.0).fract());
                r.update(&sc, scale);
            }
            let old = t.elapsed() / n;
            let t = std::time::Instant::now();
            sc.pet = Xf::ID;
            for i in 0..n {
                let p = i as f32 / 60.0;
                sc.tail = anim::tail_sway((i as f32 / 40.0).fract());
                sc.star = anim::star_bob((i as f32 / 50.0).fract());
                sc.live = live::LiveView { depth: 1.0, breath: p.fract(), bob: (p * 0.8).fract(), sway: (p * 0.7).fract(), blink: 0.0, mouth: 0.0, sleep: false, ..Default::default() };
                r.update(&sc, scale);
            }
            eprintln!("scale {scale}: old idle frame {old:?}, live idle frame {:?}", t.elapsed() / n);
        }
        std::thread::sleep(std::time::Duration::from_secs(4));
    }
    #[test]
    fn snapped_pet_is_pixel_exact_at_fractional_dpi() {
        // 125%/150%/175%：立绘缩放图原样落到整像素上，不再二次双线性（以前高倍缩放发虚）
        for scale in [1.25f32, 1.5, 1.75, 2.5] {
            let mut r = Renderer::new();
            let sc = Scene::default();
            r.draw(&sc, scale);
            let (sw, sh) = ((PET_W * scale).round() as u32, (PET_H * scale).round() as u32);
            let base = r.sprites.asset(&sprites::layers_for(sc.face, &sc.costume, &[]).base, sw, sh).unwrap();
            let (x, y) = snapped_origin(&sc, scale, sw, sh).unwrap();
            let pm = r.pixmap().unwrap();
            let pw = pm.width() as usize;
            // 取立绘中间一块（避开尾巴/星星图层）逐像素比对
            let mut same = 0;
            let mut total = 0;
            for yy in (sh as usize / 3)..(sh as usize / 2) {
                for xx in (sw as usize * 2 / 5)..(sw as usize * 3 / 5) {
                    let s = &base.data()[(yy * sw as usize + xx) * 4..][..4];
                    if s[3] != 255 { continue; }
                    total += 1;
                    let d = &pm.data()[((y as usize + yy) * pw + x as usize + xx) * 4..][..4];
                    if s == d { same += 1; }
                }
            }
            assert!(total > 100 && same == total, "scale {scale}: {same}/{total}");
            // 脚底贴窗口底边
            assert_eq!(y + sh as i32, (WIN_H as f32 * scale).round() as i32);
        }
        // 转着/歪头时走通用变换
        let mut sc = Scene::default();
        sc.pet.rot = 3.0;
        assert!(snapped_origin(&sc, 1.25, 250, 366).is_none());
        sc.pet.rot = 0.0;
        sc.flip = 0.4;
        assert!(snapped_origin(&sc, 1.25, 250, 366).is_none());
        sc.flip = -1.0;
        assert!(snapped_origin(&sc, 1.25, 250, 366).is_some());
    }

    #[test]
    fn mirrored_snap_matches_general_path() {
        // 镜像快速路径和 tiny-skia 通用路径画出来的位置一致（只差采样）
        let mut r = Renderer::new();
        let mut sc = Scene::default();
        sc.flip = -1.0;
        r.draw(&sc, 1.0);
        let a = r.pixmap().unwrap().clone();
        sc.pet.rot = 0.0005; // 极小旋转也走快速路径（阈值内）
        r.draw(&sc, 1.0);
        let b = r.pixmap().unwrap().clone();
        assert_eq!(a.data(), b.data());
        // 镜像后左右对调：不透明像素的水平重心落在另一侧
        let sc2 = Scene::default();
        r.draw(&sc2, 1.0);
        let c = r.pixmap().unwrap().clone();
        let cx = |p: &Pixmap| {
            let w = p.width() as usize;
            let (mut s, mut n) = (0f64, 0f64);
            for (i, px) in p.data().as_chunks::<4>().0.iter().enumerate() { if px[3] > 128 { s += (i % w) as f64; n += 1.0; } }
            s / n
        };
        let mid = (PET_X + PET_W / 2.0) as f64;
        assert!((cx(&a) - mid) * (cx(&c) - mid) < 0.0 || (cx(&a) - cx(&c)).abs() > 0.5);
    }

    #[test]
    fn crossfade_mixes_previous_face() {
        let mut r = Renderer::new();
        let mut sc = Scene::default();
        sc.face = Face::Think;
        r.draw(&sc, 1.0);
        let think = r.pixmap().unwrap().clone();
        sc.face = Face::Idle;
        r.draw(&sc, 1.0);
        let idle = r.pixmap().unwrap().clone();
        sc.live.prev_face = Some(Face::Think);
        sc.live.prev_alpha = 0.5;
        r.draw(&sc, 1.0);
        let mix = r.pixmap().unwrap().clone();
        // 两张不一样的像素上，混合结果落在两者之间
        // （尾巴/星星图层画在上面，那几处不参与混合，所以按比例判断）
        let (mut checked, mut between) = (0, 0);
        for ((a, b), m) in idle.data().as_chunks::<4>().0.iter().zip(think.data().as_chunks::<4>().0).zip(mix.data().as_chunks::<4>().0) {
            if a[3] == 255 && b[3] == 255 && (a[0] as i32 - b[0] as i32).abs() > 60 {
                checked += 1;
                let (lo, hi) = (a[0].min(b[0]) as i32 + 10, a[0].max(b[0]) as i32 - 10);
                if (lo..=hi).contains(&(m[0] as i32)) { between += 1; }
            }
        }
        assert!(checked > 20 && between * 10 > checked * 8, "{between}/{checked}");
        // 同一张底图（换装别名）不叠
        sc.costume = "maid".into();
        sc.face = Face::Happy;
        sc.live.prev_face = Some(Face::Love);
        r.draw(&sc, 1.0);
        let a = r.pixmap().unwrap().clone();
        sc.live.prev_face = None;
        r.draw(&sc, 1.0);
        assert_eq!(a.data(), r.pixmap().unwrap().data());
    }

    #[test]
    fn glow_follows_breathing_warp() {
        let mut r = Renderer::new();
        let mut sc = Scene::default();
        sc.glow = true;
        sc.live = live::LiveView { depth: 1.0, breath: 0.0, ..Default::default() };
        r.draw(&sc, 1.0);
        let a = r.pixmap().unwrap().clone();
        sc.live.breath = 0.4;
        r.draw(&sc, 1.0);
        let b = r.pixmap().unwrap().clone();
        // 头顶上方一行（只有光晕，没有立绘）：吸气时光晕跟着上移 → 这一行变了
        let w = a.width() as usize;
        let top = (0..a.height() as usize).find(|&y| a.data()[y * w * 4..(y + 1) * w * 4].as_chunks::<4>().0.iter().any(|p| p[3] > 0 && p[3] < 200)).unwrap();
        let row = |p: &Pixmap, y: usize| p.data()[y * w * 4..(y + 1) * w * 4].to_vec();
        assert_ne!(row(&a, top + 2), row(&b, top + 2));
        // 脚底的光晕不动
        let foot = a.height() as usize - 3;
        assert_eq!(row(&a, foot), row(&b, foot));
    }
}
