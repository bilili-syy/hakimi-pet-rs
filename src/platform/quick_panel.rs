//! 奶油风「哈基米快捷面板」（`HakimiQuickPanel`，方案 1）：
//! 右键点击桌宠时在她身旁弹出的圆角快捷卡片，彻底告别多级系统子菜单走钢丝：
//! - 顶部：猫爪标题 + `🎭 角色切换` 下拉胶囊 + `×` 关闭钮
//! - 第一区（2×4 网格，零折叠一键直达）：聊聊天、扔鱼干、喂蜂蜜、逗猫棒、跳支舞、爬窗口、番茄钟、看屏吐槽
//!   （对跳舞/爬窗/番茄钟右键还可展开细分选项）
//! - 第二区（2×5 衣橱胶囊，连点试穿不关面板）：8 套衣服 + 圆眼镜/圣诞帽开关 + 一键摘配饰
//! - 第三区（内嵌双声道音量）：语音滑块 + 音乐滑块 + 一键静音（支持拖拽与滚轮单调，120Hz 实时生效）
//! - 第四区（底部工具条）：备忘(带待办数)、签到、天气、状态、设置、隐藏、更多▾（呼出传统完整菜单兜底）
use super::panel_icons::{PanelIcon, blit_pixmap_to_dc, draw_icon, fill_circle, fill_rrect, hline, rgba};
use super::settings::ui_faces;
use super::sys::wide;
use super::volume::{pick_popup_pos, step_pct};
use crate::core::{config, costume, memo, skin};
use crate::types::{AppEvent, MenuAction};
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use tiny_skia::{Pixmap, Transform};
use tray_icon::menu::{ContextMenu, Menu, MenuItem, PredefinedMenuItem};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_ESCAPE, VK_SPACE,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

const CLASS: PCWSTR = w!("HakimiQuickPanel");
pub const PANEL_W: f32 = 348.0;
pub const PANEL_H: f32 = 364.0;
const PAD_X: f32 = 14.0;
const INNER_W: f32 = PANEL_W - PAD_X * 2.0; // 320.0

const SLIDER_X0: f32 = 76.0;
const SLIDER_X1: f32 = 288.0;
const ROW_VOICE_Y: f32 = 291.0;
const ROW_MUSIC_Y: f32 = 313.0;

const WM_MOUSELEAVE_U32: u32 = 0x02A3;
const WM_DPICHANGED_U32: u32 = 0x02E0;
const WA_INACTIVE_U16: usize = 0;

const fn cref(rgb: u32) -> COLORREF {
    COLORREF(((rgb >> 16) & 0xff) | (rgb & 0x00ff00) | ((rgb & 0xff) << 16))
}

const BG_RGB: u32 = 0xfffbf4;
const BORDER_RGB: u32 = 0xe5cfaf;
const DIVIDER_RGB: u32 = 0xeee0cc;
const TEXT_RGB: u32 = 0x3a2416;
const SUB_RGB: u32 = 0x9a8573;
const TILE_BG: u32 = 0xffffff;
const TILE_BORDER: u32 = 0xead9c3;
const TILE_HOT_BG: u32 = 0xffefd2;
const TILE_HOT_BORDER: u32 = 0xf5a623;
const CHIP_BG: u32 = 0xfff4e5;
const CHIP_HOT_BG: u32 = 0xffe8c7;
const CHIP_BORDER: u32 = 0xe8d4ba;
const ACCENT_RGB: u32 = 0xf5a623;
const ACCENT_HOT_RGB: u32 = 0xe09212;
const TRACK_BG_RGB: u32 = 0xefe3d3;
const MUTED_RGB: u32 = 0xc8b8a6;
const FOOT_BG: u32 = 0xf9f0e2;
const FOOT_HOT_BG: u32 = 0xf3e2c7;
const CLOSE_HOT_BG: u32 = 0xfdecea;
const CLOSE_HOT_FG: u32 = 0xe04b3e;
const CLOSE_IDLE_FG: u32 = 0xa88e7c;

static PANEL_HWND: AtomicIsize = AtomicIsize::new(0);
static PANEL_VISIBLE: AtomicBool = AtomicBool::new(false);

const WARDROBE: [(&str, &str, u32); 9] = [
    ("sailor", "水手服", 0x62aef7),
    ("hoodie", "卫衣", 0xd8c2aa),
    ("pajama", "睡衣", 0xa78bfa),
    ("maid", "女仆装", 0x6b4630),
    ("jk", "JK服", 0x3b82f6),
    ("idol", "偶像服", 0xf472b6),
    ("swim", "泳装", 0x38bdf8),
    ("xmas", "圣诞裙", 0xef5350),
    ("outfit:glasses", "圆眼镜", 0xf5a623),
];

const FOOTER_WIDTHS: [f32; 7] = [48.0, 41.0, 41.0, 41.0, 41.0, 41.0, 43.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    None,
    RoleBtn,
    CloseBtn,
    Action(usize),
    ClearOutfit,
    Wardrobe(usize),
    MuteBtn,
    VoiceSlider,
    MusicSlider,
    Footer(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragSlider {
    None,
    Voice,
    Music,
}

struct PanelState {
    hwnd: HWND,
    scale: f32,
    font: HFONT,
    bold: HFONT,
    title_bold: HFONT,
    small: HFONT,
    small_bold: HFONT,
    hover: Part,
    drag: DragSlider,
    tracking_leave: bool,
    dirty_disk: bool,
}

thread_local! {
    static PANEL: RefCell<Option<PanelState>> = const { RefCell::new(None) };
    /// 弹出附属原生子菜单期间置 true，忽略 WA_INACTIVE
    static SUBMENU_OPEN: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn hwnd() -> Option<HWND> {
    let h = PANEL_HWND.load(Ordering::Relaxed);
    (h != 0).then_some(HWND(h as *mut _))
}

pub fn is_visible() -> bool {
    PANEL_VISIBLE.load(Ordering::Relaxed)
}

pub fn slider_pos_to_pct(lx: f32) -> u8 {
    let span = (SLIDER_X1 - SLIDER_X0).max(1.0);
    (((lx - SLIDER_X0) / span).clamp(0.0, 1.0) * 100.0).round() as u8
}

fn role_rect() -> [f32; 4] {
    [184.0, 11.0, 118.0, 24.0]
}

fn close_rect() -> [f32; 4] {
    [308.0, 11.0, 26.0, 24.0]
}

fn action_rect(idx: usize) -> [f32; 4] {
    let (col, row) = (idx % 4, idx / 4);
    let (cw, ch, gx, gy) = (75.5, 50.0, 6.0, 6.0);
    [PAD_X + col as f32 * (cw + gx), 66.0 + row as f32 * (ch + gy), cw, ch]
}

fn clear_outfit_rect() -> [f32; 4] {
    [282.0, 176.0, 52.0, 18.0]
}

fn wardrobe_rect(idx: usize) -> [f32; 4] {
    let (ph, gx, gy) = (23.0, 5.0, 5.0);
    if idx < 5 {
        let pw = 60.0;
        [PAD_X + idx as f32 * (pw + gx), 197.0, pw, ph]
    } else {
        let col = idx - 5;
        let pw = 76.25;
        [PAD_X + col as f32 * (pw + gx), 197.0 + ph + gy, pw, ph]
    }
}

fn mute_rect() -> [f32; 4] {
    [268.0, 258.0, 66.0, 21.0]
}

fn footer_rect(idx: usize) -> [f32; 4] {
    let mut x = PAD_X;
    for w in FOOTER_WIDTHS.iter().take(idx) {
        x += *w + 4.0;
    }
    [x, 335.0, FOOTER_WIDTHS[idx], 22.0]
}

fn in_rect(lx: f32, ly: f32, r: [f32; 4]) -> bool {
    lx >= r[0] && lx <= r[0] + r[2] && ly >= r[1] && ly <= r[1] + r[3]
}

fn in_slider(lx: f32, ly: f32, cy: f32) -> bool {
    (SLIDER_X0 - 9.0..=SLIDER_X1 + 9.0).contains(&lx) && (cy - 10.5..=cy + 10.5).contains(&ly)
}

/// 纯逻辑命中检测（可单测）
pub fn part_at(lx: f32, ly: f32, has_outfit: bool) -> Part {
    if in_rect(lx, ly, close_rect()) {
        return Part::CloseBtn;
    }
    if in_rect(lx, ly, role_rect()) {
        return Part::RoleBtn;
    }
    for i in 0..8 {
        if in_rect(lx, ly, action_rect(i)) {
            return Part::Action(i);
        }
    }
    if has_outfit && in_rect(lx, ly, clear_outfit_rect()) {
        return Part::ClearOutfit;
    }
    for i in 0..WARDROBE.len() {
        if in_rect(lx, ly, wardrobe_rect(i)) {
            return Part::Wardrobe(i);
        }
    }
    if in_rect(lx, ly, mute_rect()) {
        return Part::MuteBtn;
    }
    if in_slider(lx, ly, ROW_VOICE_Y) {
        return Part::VoiceSlider;
    }
    if in_slider(lx, ly, ROW_MUSIC_Y) {
        return Part::MusicSlider;
    }
    for i in 0..7 {
        if in_rect(lx, ly, footer_rect(i)) {
            return Part::Footer(i);
        }
    }
    Part::None
}

fn dpi_scale(anchor: HWND) -> f32 {
    let d = unsafe {
        let wd = if !anchor.is_invalid() { GetDpiForWindow(anchor) } else { 0 };
        if wd > 0 { wd } else { GetDpiForSystem() }
    };
    (if d == 0 { 96 } else { d }) as f32 / 96.0
}

unsafe fn create_fonts(s: f32) -> (HFONT, HFONT, HFONT, HFONT, HFONT) {
    let (body_face, bold_face) = ui_faces();
    let mk = |size: f32, bold: bool| unsafe {
        let fw = wide(if bold { bold_face } else { body_face });
        CreateFontW(
            -(size * s).round() as i32,
            0,
            0,
            0,
            if bold { FW_BOLD.0 as i32 } else { FW_NORMAL.0 as i32 },
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            FF_DONTCARE.0 as u32,
            PCWSTR(fw.as_ptr()),
        )
    };
    (mk(12.2, false), mk(12.2, true), mk(14.5, true), mk(11.3, false), mk(11.3, true))
}

unsafe fn delete_fonts(fs: (HFONT, HFONT, HFONT, HFONT, HFONT)) {
    for f in [fs.0, fs.1, fs.2, fs.3, fs.4] {
        if !f.is_invalid() {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(f.0));
            }
        }
    }
}

unsafe fn apply_round_rgn(hwnd: HWND, w: i32, h: i32, s: f32) {
    let rad = (16.0 * s).round() as i32;
    unsafe {
        let rgn = CreateRoundRectRgn(0, 0, w + 1, h + 1, rad, rad);
        if !rgn.is_invalid() {
            let _ = SetWindowRgn(hwnd, Some(rgn), true);
        }
    }
}

fn ensure_created() -> Option<HWND> {
    if let Some(h) = PANEL.with(|c| c.borrow().as_ref().map(|p| p.hwnd))
        && unsafe { IsWindow(Some(h)).as_bool() }
    {
        return Some(h);
    }
    let owner = super::main_hwnd().unwrap_or_default();
    let s = dpi_scale(owner);
    unsafe {
        let hinst = GetModuleHandleW(None).ok()?;
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinst.into(),
            lpszClassName: CLASS,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let w = (PANEL_W * s).round() as i32;
        let h = (PANEL_H * s).round() as i32;
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            CLASS,
            w!("哈基米快捷面板"),
            WS_POPUP,
            0,
            0,
            w,
            h,
            if owner.is_invalid() { None } else { Some(owner) },
            None,
            Some(hinst.into()),
            None,
        )
        .ok()?;
        apply_round_rgn(hwnd, w, h, s);
        let (font, bold, title_bold, small, small_bold) = create_fonts(s);
        PANEL_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
        PANEL.with(|c| {
            *c.borrow_mut() = Some(PanelState {
                hwnd,
                scale: s,
                font,
                bold,
                title_bold,
                small,
                small_bold,
                hover: Part::None,
                drag: DragSlider::None,
                tracking_leave: false,
                dirty_disk: false,
            });
        });
        Some(hwnd)
    }
}

fn place_near_pet(hwnd: HWND) {
    let Some(pet) = super::main_hwnd() else { return };
    let mut pr = RECT::default();
    if unsafe { GetWindowRect(pet, &mut pr) }.is_err() {
        return;
    }
    let s = dpi_scale(pet);
    PANEL.with(|c| {
        if let Some(p) = c.borrow_mut().as_mut()
            && (p.scale - s).abs() > 0.01
        {
            let old = (p.font, p.bold, p.title_bold, p.small, p.small_bold);
            p.scale = s;
            let nf = unsafe { create_fonts(s) };
            p.font = nf.0;
            p.bold = nf.1;
            p.title_bold = nf.2;
            p.small = nf.3;
            p.small_bold = nf.4;
            unsafe { delete_fonts(old) };
        }
    });
    let w = (PANEL_W * s).round() as i32;
    let h = (PANEL_H * s).round() as i32;
    let wa = super::work_area_of(pet);
    let gap = (8.0 * s).round() as i32;
    let (x, y) = pick_popup_pos(pr, (w, h), wa, gap);
    unsafe {
        apply_round_rgn(hwnd, w, h, s);
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
    }
}

/// 右键桌宠：已开则收起，未开则收起迷你音量条并弹出快捷面板
pub fn toggle() {
    if is_visible() {
        hide();
    } else {
        open();
    }
}

pub fn open() {
    super::volume::hide();
    let Some(hwnd) = ensure_created() else { return };
    place_near_pet(hwnd);
    PANEL_VISIBLE.store(true, Ordering::Relaxed);
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

pub fn sync_if_open() {
    if !is_visible() {
        return;
    }
    if let Some(h) = hwnd() {
        unsafe {
            let _ = InvalidateRect(Some(h), None, false);
        }
    }
}

pub fn follow_pet() {
    if !is_visible() {
        return;
    }
    let dragging = PANEL.with(|c| c.borrow().as_ref().is_some_and(|p| p.drag != DragSlider::None));
    if dragging {
        return;
    }
    if let Some(h) = hwnd() {
        place_near_pet(h);
    }
}

pub fn hide() {
    flush_if_dirty();
    PANEL_VISIBLE.store(false, Ordering::Relaxed);
    PANEL.with(|c| {
        if let Some(p) = c.borrow_mut().as_mut() {
            p.drag = DragSlider::None;
            p.hover = Part::None;
        }
    });
    if let Some(h) = hwnd() {
        unsafe {
            let _ = ShowWindow(h, SW_HIDE);
        }
    }
}

pub fn shutdown() {
    flush_if_dirty();
    PANEL_VISIBLE.store(false, Ordering::Relaxed);
    let old = PANEL.with(|c| c.borrow_mut().take());
    PANEL_HWND.store(0, Ordering::Relaxed);
    if let Some(p) = old {
        unsafe {
            if IsWindow(Some(p.hwnd)).as_bool() {
                let _ = DestroyWindow(p.hwnd);
            }
            delete_fonts((p.font, p.bold, p.title_bold, p.small, p.small_bold));
        }
    }
}

fn flush_if_dirty() {
    let dirty = PANEL.with(|c| {
        c.borrow_mut()
            .as_mut()
            .map(|p| std::mem::replace(&mut p.dirty_disk, false))
            .unwrap_or(false)
    });
    if dirty {
        config::flush();
        super::menu::refresh();
        super::settings::sync_external_volume();
    }
}

fn repaint_self() {
    if let Some(h) = hwnd() {
        unsafe {
            let _ = InvalidateRect(Some(h), None, false);
        }
    }
}

fn active_role_short() -> String {
    match skin::active() {
        None => "哈基米".into(),
        Some(s) => {
            let name = s.display_name();
            let mut out: String = name.chars().take(5).collect();
            if name.chars().count() > 5 {
                out.push('…');
            }
            out
        }
    }
}

/// 生成抗锯齿图形与矢量图标层（纯内存 `Pixmap`，同时支持无窗口单测验证）
fn render_shapes_pixmap(
    size: (u32, u32),
    s: f32,
    hover: Part,
    drag: DragSlider,
    cfg: &config::Config,
    custom_skin: bool,
    pomo_running: bool,
) -> Option<Pixmap> {
    let (w, h) = size;
    let mut pm = Pixmap::new(w.max(1), h.max(1))?;
    pm.fill(rgba(BG_RGB, 1.0));
    let xf = Transform::from_scale(s, s);

    // 卡片外边框
    fill_rrect(&mut pm, [0.0, 0.0, PANEL_W, PANEL_H], 15.0, BG_RGB, Some((BORDER_RGB, 1.6)), xf);

    // 标题栏：猫爪图标 + 角色胶囊 + 关闭钮
    draw_icon(&mut pm, PanelIcon::Paw, 25.0, 23.0, s, 1.0);

    let rr = role_rect();
    let role_hot = hover == Part::RoleBtn;
    fill_rrect(
        &mut pm,
        rr,
        12.0,
        if role_hot { CHIP_HOT_BG } else { CHIP_BG },
        Some((CHIP_BORDER, 1.0)),
        xf,
    );
    draw_icon(&mut pm, PanelIcon::RoleMask, rr[0] + 16.0, rr[1] + rr[3] * 0.5, s, 0.85);

    let cr = close_rect();
    if hover == Part::CloseBtn {
        fill_rrect(&mut pm, cr, 7.0, CLOSE_HOT_BG, None, xf);
    }

    hline(&mut pm, PAD_X, PANEL_W - PAD_X, 43.0, DIVIDER_RGB, 1.0, xf);

    // 第一区：2×4 常用互动网格
    let icons = [
        PanelIcon::Chat,
        PanelIcon::Fish,
        PanelIcon::Honey,
        PanelIcon::Teaser,
        PanelIcon::Dance,
        PanelIcon::Climb,
        PanelIcon::Pomo,
        PanelIcon::Roast,
    ];
    for (i, ico) in icons.iter().enumerate() {
        let r = action_rect(i);
        let hot = hover == Part::Action(i);
        let active_pomo = i == 6 && pomo_running;
        let (fill_c, border_c, bw) = if hot {
            (TILE_HOT_BG, TILE_HOT_BORDER, 1.8)
        } else if active_pomo {
            (0xfff2ee, 0xef5350, 1.5)
        } else {
            (TILE_BG, TILE_BORDER, 1.0)
        };
        fill_rrect(&mut pm, r, 10.0, fill_c, Some((border_c, bw)), xf);
        draw_icon(&mut pm, *ico, r[0] + r[2] * 0.5, r[1] + 17.5, s, 1.0);
    }

    // 第二区：2×5 衣橱胶囊
    if !cfg.outfit.is_empty() {
        let clr = clear_outfit_rect();
        let chot = hover == Part::ClearOutfit;
        fill_rrect(&mut pm, clr, 9.0, if chot { CHIP_HOT_BG } else { FOOT_BG }, None, xf);
    }
    let cur_costume = costume::normalize(&cfg.costume);
    for (i, (key, _, dot_rgb)) in WARDROBE.iter().enumerate() {
        let r = wardrobe_rect(i);
        let active = if let Some(acc) = key.strip_prefix("outfit:") {
            cfg.outfit.iter().any(|x| x == acc)
        } else {
            !custom_skin && cur_costume == *key
        };
        let hot = hover == Part::Wardrobe(i);
        let (fill_c, stroke_c, dot_c) = if active {
            (if hot { ACCENT_HOT_RGB } else { ACCENT_RGB }, None, 0xffffff)
        } else {
            (
                if hot { CHIP_HOT_BG } else { CHIP_BG },
                Some((CHIP_BORDER, 1.0)),
                *dot_rgb,
            )
        };
        fill_rrect(&mut pm, r, 11.5, fill_c, stroke_c, xf);
        fill_circle(&mut pm, r[0] + 10.5, r[1] + r[3] * 0.5, 3.3, dot_c, None, xf);
    }

    hline(&mut pm, PAD_X, PANEL_W - PAD_X, 254.0, DIVIDER_RGB, 1.0, xf);

    // 第三区：独立音量
    let muted = cfg.tts.muted;
    let mr = mute_rect();
    let mhot = hover == Part::MuteBtn;
    let (m_fill, m_stroke) = if muted {
        (if mhot { ACCENT_HOT_RGB } else { ACCENT_RGB }, None)
    } else {
        (if mhot { CHIP_HOT_BG } else { CHIP_BG }, Some((CHIP_BORDER, 1.0)))
    };
    fill_rrect(&mut pm, mr, 10.5, m_fill, m_stroke, xf);

    draw_icon(&mut pm, PanelIcon::Speaker(muted), 23.0, ROW_VOICE_Y, s, 0.84);
    draw_icon(&mut pm, PanelIcon::MusicNote(muted), 23.0, ROW_MUSIC_Y, s, 0.84);

    let draw_bar = |pm: &mut Pixmap, row_y: f32, pct: u8, active: bool| {
        let th = 5.6;
        let span = SLIDER_X1 - SLIDER_X0;
        fill_rrect(pm, [SLIDER_X0, row_y - th * 0.5, span, th], 2.8, TRACK_BG_RGB, None, xf);
        let fill_w = span * (pct as f32 / 100.0).clamp(0.0, 1.0);
        let bar_c = if muted { MUTED_RGB } else { ACCENT_RGB };
        if fill_w >= 2.0 {
            fill_rrect(pm, [SLIDER_X0, row_y - th * 0.5, fill_w.max(th), th], 2.8, bar_c, None, xf);
        }
        let rad = if active { 7.2 } else { 6.3 };
        fill_circle(pm, SLIDER_X0 + fill_w, row_y, rad, 0xffffff, Some((bar_c, 2.0)), xf);
    };
    draw_bar(
        &mut pm,
        ROW_VOICE_Y,
        cfg.tts.voice_pct(),
        drag == DragSlider::Voice || hover == Part::VoiceSlider,
    );
    draw_bar(
        &mut pm,
        ROW_MUSIC_Y,
        cfg.tts.music_pct(),
        drag == DragSlider::Music || hover == Part::MusicSlider,
    );

    hline(&mut pm, PAD_X, PANEL_W - PAD_X, 329.0, DIVIDER_RGB, 1.0, xf);

    // 第四区：底部 7 个工具胶囊
    for i in 0..7 {
        let fr = footer_rect(i);
        let fhot = hover == Part::Footer(i);
        fill_rrect(&mut pm, fr, 8.0, if fhot { FOOT_HOT_BG } else { FOOT_BG }, None, xf);
    }

    Some(pm)
}

unsafe fn draw_txt(hdc: HDC, r: [f32; 4], s: f32, text: &str, color: u32, flags: DRAW_TEXT_FORMAT) {
    let mut rc = RECT {
        left: (r[0] * s).round() as i32,
        top: (r[1] * s).round() as i32,
        right: ((r[0] + r[2]) * s).round() as i32,
        bottom: ((r[1] + r[3]) * s).round() as i32,
    };
    let w = wide(text);
    unsafe {
        SetTextColor(hdc, cref(color));
        DrawTextW(hdc, &mut w.clone()[..w.len() - 1], &mut rc, flags | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
    }
}

unsafe fn paint(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let (w, h) = ((rc.right - rc.left).max(1), (rc.bottom - rc.top).max(1));

        let mem_dc = CreateCompatibleDC(Some(hdc));
        let mem_bmp = CreateCompatibleBitmap(hdc, w, h);
        let old_bmp = SelectObject(mem_dc, HGDIOBJ(mem_bmp.0));

        let (s, font, bold, title_bold, small, small_bold, hover, drag) = PANEL.with(|c| {
            c.borrow()
                .as_ref()
                .map(|p| (p.scale, p.font, p.bold, p.title_bold, p.small, p.small_bold, p.hover, p.drag))
                .unwrap_or((
                    1.0,
                    HFONT::default(),
                    HFONT::default(),
                    HFONT::default(),
                    HFONT::default(),
                    HFONT::default(),
                    Part::None,
                    DragSlider::None,
                ))
        });

        let cfg = config::get();
        let custom_skin = skin::active().is_some();
        let pomo_left = super::pomodoro_left();
        let chat_busy = super::chat::is_busy();
        let pending_memos = memo::pending().len();

        if let Some(pm) = render_shapes_pixmap((w as u32, h as u32), s, hover, drag, &cfg, custom_skin, pomo_left.is_some()) {
            blit_pixmap_to_dc(mem_dc, &pm);
        }

        SetBkMode(mem_dc, TRANSPARENT);

        // 标题与角色名
        let old_f = SelectObject(mem_dc, HGDIOBJ(title_bold.0));
        draw_txt(mem_dc, [38.0, 9.0, 144.0, 28.0], s, "哈基米快捷面板", TEXT_RGB, DT_LEFT);

        SelectObject(mem_dc, HGDIOBJ(small_bold.0));
        let role_txt = format!("{} ▾", active_role_short());
        let rr = role_rect();
        draw_txt(mem_dc, [rr[0] + 24.0, rr[1], rr[2] - 28.0, rr[3]], s, &role_txt, TEXT_RGB, DT_CENTER);

        // 右上角 ×
        let cr = close_rect();
        let close_fg = if hover == Part::CloseBtn { CLOSE_HOT_FG } else { CLOSE_IDLE_FG };
        let pen = CreatePen(PS_SOLID, (1.7 * s).round().max(1.0) as i32, cref(close_fg));
        let old_pen = SelectObject(mem_dc, HGDIOBJ(pen.0));
        let cx = ((cr[0] + cr[2] * 0.5) * s).round() as i32;
        let cy = ((cr[1] + cr[3] * 0.5) * s).round() as i32;
        let d = (4.2 * s).round() as i32;
        let _ = MoveToEx(mem_dc, cx - d, cy - d, None);
        let _ = LineTo(mem_dc, cx + d + 1, cy + d + 1);
        let _ = MoveToEx(mem_dc, cx + d, cy - d, None);
        let _ = LineTo(mem_dc, cx - d - 1, cy + d + 1);
        SelectObject(mem_dc, old_pen);
        let _ = DeleteObject(HGDIOBJ(pen.0));

        // 第一区分组名 + 8 格文字
        SelectObject(mem_dc, HGDIOBJ(small_bold.0));
        draw_txt(mem_dc, [PAD_X, 47.0, INNER_W, 16.0], s, "常用互动（一键直达 · 右键跳舞/番茄钟选档）", SUB_RGB, DT_LEFT);

        let pomo_label = match pomo_left {
            Some(m) => format!("专注 {m}m"),
            None => "番茄钟".into(),
        };
        let roast_label = if !cfg.screen_roast { "看屏(开启)" } else { "看屏吐槽" };
        let tile_labels: [&str; 8] = [
            if chat_busy { "停止等待" } else { "聊聊天" },
            "扔鱼干",
            "喂蜂蜜",
            "逗猫棒",
            "跳支舞",
            "爬窗口",
            &pomo_label,
            roast_label,
        ];
        for (i, lbl) in tile_labels.iter().enumerate() {
            let r = action_rect(i);
            let hot = hover == Part::Action(i);
            let active_pomo = i == 6 && pomo_left.is_some();
            SelectObject(mem_dc, HGDIOBJ(if hot || active_pomo { bold.0 } else { font.0 }));
            let fg = if hot {
                0xc4720a
            } else if active_pomo {
                0xd63b36
            } else {
                TEXT_RGB
            };
            draw_txt(mem_dc, [r[0], r[1] + 29.0, r[2], 19.0], s, lbl, fg, DT_CENTER);
        }

        // 第二区：衣橱试穿
        SelectObject(mem_dc, HGDIOBJ(small_bold.0));
        draw_txt(mem_dc, [PAD_X, 177.0, 260.0, 16.0], s, "衣橱试穿（连点直接换装，不关面板）", SUB_RGB, DT_LEFT);
        if !cfg.outfit.is_empty() {
            let clr = clear_outfit_rect();
            SelectObject(mem_dc, HGDIOBJ(small.0));
            draw_txt(mem_dc, clr, s, "摘配饰", SUB_RGB, DT_CENTER);
        }

        let cur_costume = costume::normalize(&cfg.costume);
        for (i, (key, lbl, _)) in WARDROBE.iter().enumerate() {
            let r = wardrobe_rect(i);
            let active = if let Some(acc) = key.strip_prefix("outfit:") {
                cfg.outfit.iter().any(|x| x == acc)
            } else {
                !custom_skin && cur_costume == *key
            };
            SelectObject(mem_dc, HGDIOBJ(if active { small_bold.0 } else { small.0 }));
            draw_txt(
                mem_dc,
                [r[0] + 15.0, r[1], r[2] - 17.0, r[3]],
                s,
                lbl,
                if active { 0xffffff } else { TEXT_RGB },
                DT_CENTER,
            );
        }

        // 第三区：独立音量
        SelectObject(mem_dc, HGDIOBJ(small_bold.0));
        draw_txt(mem_dc, [PAD_X, 260.0, 120.0, 18.0], s, "独立音量", SUB_RGB, DT_LEFT);

        let muted = cfg.tts.muted;
        let mr = mute_rect();
        SelectObject(mem_dc, HGDIOBJ(if muted { small_bold.0 } else { small.0 }));
        draw_txt(
            mem_dc,
            mr,
            s,
            if muted { "恢复出声" } else { "一键静音" },
            if muted { 0xffffff } else { TEXT_RGB },
            DT_CENTER,
        );

        SelectObject(mem_dc, HGDIOBJ(font.0));
        let v_fg = if muted { SUB_RGB } else { TEXT_RGB };
        draw_txt(mem_dc, [35.0, ROW_VOICE_Y - 10.0, 38.0, 20.0], s, "语音", v_fg, DT_LEFT);
        draw_txt(mem_dc, [35.0, ROW_MUSIC_Y - 10.0, 38.0, 20.0], s, "音乐", v_fg, DT_LEFT);

        SelectObject(mem_dc, HGDIOBJ(bold.0));
        let vp = format!("{}%", cfg.tts.voice_pct());
        let mp = format!("{}%", cfg.tts.music_pct());
        draw_txt(mem_dc, [292.0, ROW_VOICE_Y - 10.0, 42.0, 20.0], s, &vp, v_fg, DT_RIGHT);
        draw_txt(mem_dc, [292.0, ROW_MUSIC_Y - 10.0, 42.0, 20.0], s, &mp, v_fg, DT_RIGHT);

        // 第四区：底部工具胶囊
        SelectObject(mem_dc, HGDIOBJ(small.0));
        let memo_lbl = if pending_memos > 0 {
            format!("备忘({})", pending_memos.min(99))
        } else {
            "备忘".into()
        };
        let foot_labels = [memo_lbl.as_str(), "签到", "天气", "状态", "设置", "隐藏", "更多▾"];
        for (i, fl) in foot_labels.iter().enumerate() {
            let fr = footer_rect(i);
            let fhot = hover == Part::Footer(i);
            SelectObject(mem_dc, HGDIOBJ(if fhot { small_bold.0 } else { small.0 }));
            draw_txt(mem_dc, fr, s, fl, if fhot { TEXT_RGB } else { 0x6a5240 }, DT_CENTER);
        }

        SelectObject(mem_dc, old_f);
        let _ = BitBlt(hdc, 0, 0, w, h, Some(mem_dc), 0, 0, SRCCOPY);
        SelectObject(mem_dc, old_bmp);
        let _ = DeleteObject(HGDIOBJ(mem_bmp.0));
        let _ = DeleteDC(mem_dc);
        let _ = EndPaint(hwnd, &ps);
    }
}

fn apply_slider_drag(kind: DragSlider, lx: f32, commit_disk: bool) {
    let pct = slider_pos_to_pct(lx);
    let val = pct as f32 / 100.0;
    let upd = |c: &mut config::Config| {
        match kind {
            DragSlider::Voice => c.tts.volume = val,
            DragSlider::Music => c.tts.music_volume = val,
            DragSlider::None => {}
        }
        if pct > 0 && c.tts.muted {
            c.tts.muted = false;
        }
    };
    if commit_disk {
        config::update(upd);
        super::menu::refresh();
        super::settings::sync_external_volume();
    } else {
        config::update_in_memory(upd);
        PANEL.with(|c| {
            if let Some(p) = c.borrow_mut().as_mut() {
                p.dirty_disk = true;
            }
        });
    }
    crate::services::audio::apply_config_volume();
    repaint_self();
}

fn toggle_mute() {
    let c = config::update(|cfg| cfg.tts.muted = !cfg.tts.muted);
    if c.tts.muted {
        crate::services::tts::cancel_active();
    }
    crate::services::audio::apply_config_volume();
    super::menu::refresh();
    super::settings::sync_external_volume();
    repaint_self();
}

fn popup_temp_menu(hwnd: HWND, items: &[(&str, &str)]) {
    let m = Menu::new();
    for (id, text) in items {
        if id.is_empty() {
            let _ = m.append(&PredefinedMenuItem::separator());
        } else {
            let _ = m.append(&MenuItem::with_id(*id, *text, true, None));
        }
    }
    SUBMENU_OPEN.with(|s| s.set(true));
    unsafe {
        m.show_context_menu_for_hwnd(hwnd.0 as isize, None);
    }
    SUBMENU_OPEN.with(|s| s.set(false));
}

fn handle_left_click(hwnd: HWND, part: Part) {
    match part {
        Part::None | Part::VoiceSlider | Part::MusicSlider => {}
        Part::CloseBtn => hide(),
        Part::RoleBtn => {
            SUBMENU_OPEN.with(|s| s.set(true));
            super::skin_menu::popup(hwnd);
            SUBMENU_OPEN.with(|s| s.set(false));
            repaint_self();
        }
        Part::Action(idx) => match idx {
            0 => {
                hide();
                super::push_menu_id("chat");
            }
            1 => {
                hide();
                super::push_menu_id("feed");
            }
            2 => {
                hide();
                super::push_menu_id("honey");
            }
            3 => {
                hide();
                super::push_menu_id("game");
            }
            4 => {
                hide();
                super::push_menu_id("dance:");
            }
            5 => {
                hide();
                super::push_menu_id("climb");
            }
            6 => {
                if super::pomodoro_left().is_some() {
                    super::push_menu_id("pomo:stop");
                    repaint_self();
                } else {
                    hide();
                    super::push_menu_id("pomo:25");
                }
            }
            7 => {
                let cfg = config::get();
                if !cfg.screen_roast {
                    hide();
                    super::settings::open(2);
                } else if let Some(why) = super::roast::blocked() {
                    hide();
                    super::post_event(AppEvent::RoastNote(format!("看屏吐槽现在用不了喵（{why}）")));
                } else {
                    hide();
                    super::push_menu_id("roast");
                }
            }
            _ => {}
        },
        Part::ClearOutfit => {
            super::trigger_action(MenuAction::ClearOutfit);
            repaint_self();
        }
        Part::Wardrobe(idx) => {
            let (key, _, _) = WARDROBE[idx];
            if let Some(acc) = key.strip_prefix("outfit:") {
                let on = !config::get().outfit.iter().any(|x| x == acc);
                super::trigger_action(MenuAction::Outfit(acc.to_string(), on));
            } else {
                if skin::active().is_some() {
                    super::trigger_action(MenuAction::Costume("skin:hakimi".into()));
                }
                super::trigger_action(MenuAction::Costume(key.to_string()));
            }
            repaint_self();
        }
        Part::MuteBtn => toggle_mute(),
        Part::Footer(idx) => match idx {
            0 => {
                hide();
                super::settings::open(3);
            }
            1 => {
                hide();
                super::push_menu_id("checkin");
            }
            2 => {
                hide();
                super::push_menu_id("weather");
            }
            3 => {
                hide();
                super::push_menu_id("status");
            }
            4 => {
                hide();
                super::push_menu_id("settings");
            }
            5 => {
                hide();
                super::push_menu_id("visible");
            }
            6 => {
                hide();
                super::menu::popup();
            }
            _ => {}
        },
    }
}

fn handle_right_click(hwnd: HWND, part: Part) {
    match part {
        Part::Action(1) => {
            hide();
            super::push_menu_id("feednow");
        }
        Part::Action(4) => {
            let mut list: Vec<(String, String)> = crate::behavior::dance::SCORES
                .iter()
                .map(|s| (format!("dance:{}", s.id), format!("{} {}（{} 秒）", s.icon, s.name, s.secs as u32)))
                .collect();
            list.push((String::new(), String::new()));
            list.push(("dance:".into(), "🎲 随便跳一支".into()));
            list.push((format!("dance:{}", crate::behavior::dance::SONG_ID), "🎵 跟着我的歌跳（songs 文件夹）".into()));
            list.push((format!("dance:{}", crate::behavior::dance::LISTEN_ID), "🎧 跟着正在放的歌跳".into()));
            list.push(("song".into(), "🎤 唱哈基米之歌".into()));
            list.push(("songdir".into(), "📂 打开放歌的文件夹".into()));
            let refs: Vec<(&str, &str)> = list.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
            popup_temp_menu(hwnd, &refs);
            hide();
        }
        Part::Action(5) => {
            popup_temp_menu(
                hwnd,
                &[
                    ("climb", "🧗 爬到当前窗口上"),
                    ("center", "📍 叫她到屏幕中间"),
                    ("walk", "🚶 去溜达"),
                    ("sleep", "💤 去睡觉"),
                ],
            );
            hide();
        }
        Part::Action(6) => {
            popup_temp_menu(
                hwnd,
                &[
                    ("pomo:15", "🍅 15 分钟"),
                    ("pomo:25", "🍅 25 分钟"),
                    ("pomo:45", "🍅 45 分钟"),
                    ("", ""),
                    ("pomo:stop", "⏹ 结束番茄钟"),
                ],
            );
            hide();
        }
        Part::Footer(3) => {
            hide();
            super::settings::open(4);
        }
        _ => {
            hide();
            super::menu::popup();
        }
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let coords = || {
        let s = PANEL.with(|c| c.borrow().as_ref().map(|p| p.scale).unwrap_or(1.0));
        let cx = (lp.0 & 0xffff) as u16 as i16 as f32 / s;
        let cy = ((lp.0 >> 16) & 0xffff) as u16 as i16 as f32 / s;
        (cx, cy)
    };

    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            unsafe { paint(hwnd) };
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let clickable = PANEL.with(|c| {
                c.borrow()
                    .as_ref()
                    .is_some_and(|p| p.hover != Part::None || p.drag != DragSlider::None)
            });
            unsafe {
                let cur = LoadCursorW(None, if clickable { IDC_HAND } else { IDC_ARROW }).unwrap_or_default();
                SetCursor(Some(cur));
            }
            LRESULT(1)
        }
        WM_MOUSEMOVE => {
            let (lx, ly) = coords();
            let has_outfit = !config::get().outfit.is_empty();
            let drag = PANEL.with(|c| {
                let mut g = c.borrow_mut();
                let Some(p) = g.as_mut() else { return DragSlider::None };
                if !p.tracking_leave {
                    p.tracking_leave = true;
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    unsafe {
                        let _ = TrackMouseEvent(&mut tme);
                    }
                }
                let next_hover = part_at(lx, ly, has_outfit);
                if next_hover != p.hover {
                    p.hover = next_hover;
                    unsafe {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                }
                p.drag
            });
            if drag != DragSlider::None {
                apply_slider_drag(drag, lx, false);
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE_U32 => {
            PANEL.with(|c| {
                if let Some(p) = c.borrow_mut().as_mut() {
                    p.tracking_leave = false;
                    if p.hover != Part::None {
                        p.hover = Part::None;
                        unsafe {
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }
                    }
                }
            });
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let (lx, ly) = coords();
            let has_outfit = !config::get().outfit.is_empty();
            let hit = part_at(lx, ly, has_outfit);
            match hit {
                Part::VoiceSlider => {
                    PANEL.with(|c| {
                        if let Some(p) = c.borrow_mut().as_mut() {
                            p.drag = DragSlider::Voice;
                        }
                    });
                    unsafe {
                        let _ = SetCapture(hwnd);
                    }
                    apply_slider_drag(DragSlider::Voice, lx, false);
                }
                Part::MusicSlider => {
                    PANEL.with(|c| {
                        if let Some(p) = c.borrow_mut().as_mut() {
                            p.drag = DragSlider::Music;
                        }
                    });
                    unsafe {
                        let _ = SetCapture(hwnd);
                    }
                    apply_slider_drag(DragSlider::Music, lx, false);
                }
                other => handle_left_click(hwnd, other),
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let (lx, _) = coords();
            let drag = PANEL.with(|c| {
                c.borrow_mut()
                    .as_mut()
                    .map(|p| std::mem::replace(&mut p.drag, DragSlider::None))
                    .unwrap_or(DragSlider::None)
            });
            if drag != DragSlider::None {
                unsafe {
                    let _ = ReleaseCapture();
                }
                apply_slider_drag(drag, lx, true);
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            let (lx, ly) = coords();
            let has_outfit = !config::get().outfit.is_empty();
            let hit = part_at(lx, ly, has_outfit);
            handle_right_click(hwnd, hit);
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            PANEL.with(|c| {
                if let Some(p) = c.borrow_mut().as_mut() {
                    p.drag = DragSlider::None;
                }
            });
            flush_if_dirty();
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp.0 >> 16) & 0xffff) as u16 as i16 as i32;
            let mut pt = POINT {
                x: (lp.0 & 0xffff) as u16 as i16 as i32,
                y: ((lp.0 >> 16) & 0xffff) as u16 as i16 as i32,
            };
            unsafe {
                let _ = ScreenToClient(hwnd, &mut pt);
            }
            let s = PANEL.with(|c| c.borrow().as_ref().map(|p| p.scale).unwrap_or(1.0));
            let ly = pt.y as f32 / s;
            // 在音乐滑块区域滚轮单调音乐，在语音滑块区域单调语音，在其它区域同时调两路
            let target = if (ROW_MUSIC_Y - 11.0..=ROW_MUSIC_Y + 14.0).contains(&ly) {
                DragSlider::Music
            } else if (ROW_VOICE_Y - 14.0..=ROW_VOICE_Y + 11.0).contains(&ly) {
                DragSlider::Voice
            } else {
                DragSlider::None
            };
            config::update(|cfg| match target {
                DragSlider::Music => {
                    let next = step_pct(cfg.tts.music_pct(), delta);
                    cfg.tts.music_volume = next as f32 / 100.0;
                    if delta > 0 && next > 0 {
                        cfg.tts.muted = false;
                    }
                }
                DragSlider::Voice => {
                    let next = step_pct(cfg.tts.voice_pct(), delta);
                    cfg.tts.volume = next as f32 / 100.0;
                    if delta > 0 && next > 0 {
                        cfg.tts.muted = false;
                    }
                }
                DragSlider::None => {
                    let nv = step_pct(cfg.tts.voice_pct(), delta);
                    let nm = step_pct(cfg.tts.music_pct(), delta);
                    cfg.tts.volume = nv as f32 / 100.0;
                    cfg.tts.music_volume = nm as f32 / 100.0;
                    if delta > 0 && (nv > 0 || nm > 0) {
                        cfg.tts.muted = false;
                    }
                }
            });
            crate::services::audio::apply_config_volume();
            super::menu::refresh();
            super::settings::sync_external_volume();
            repaint_self();
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let vk = wp.0 as u16;
            if vk == VK_ESCAPE.0 {
                hide();
            } else if vk == VK_SPACE.0 || vk == b'M' as u16 {
                toggle_mute();
            }
            LRESULT(0)
        }
        WM_ACTIVATE => {
            if (wp.0 & 0xffff) == WA_INACTIVE_U16 && is_visible() && !SUBMENU_OPEN.with(|s| s.get()) {
                hide();
            }
            LRESULT(0)
        }
        WM_DPICHANGED_U32 => {
            if is_visible() {
                place_near_pet(hwnd);
                repaint_self();
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_test_covers_all_panel_zones_without_overlap() {
        // 顶部按钮
        assert_eq!(part_at(240.0, 22.0, false), Part::RoleBtn);
        assert_eq!(part_at(320.0, 22.0, false), Part::CloseBtn);

        // 8 个互动格子都能准确命中
        for i in 0..8 {
            let r = action_rect(i);
            assert_eq!(part_at(r[0] + r[2] * 0.5, r[1] + r[3] * 0.5, false), Part::Action(i));
        }

        // 摘配饰仅在有配饰时可点
        let clr = clear_outfit_rect();
        assert_eq!(part_at(clr[0] + 10.0, clr[1] + 8.0, false), Part::None);
        assert_eq!(part_at(clr[0] + 10.0, clr[1] + 8.0, true), Part::ClearOutfit);

        // 9 个衣橱胶囊都可命中，两行总宽均严格对齐 320px
        let last_r0 = wardrobe_rect(4);
        let last_r1 = wardrobe_rect(8);
        assert!((last_r0[0] + last_r0[2] - (PANEL_W - PAD_X)).abs() < 0.01);
        assert!((last_r1[0] + last_r1[2] - (PANEL_W - PAD_X)).abs() < 0.01);
        for i in 0..WARDROBE.len() {
            let r = wardrobe_rect(i);
            assert_eq!(part_at(r[0] + r[2] * 0.5, r[1] + r[3] * 0.5, true), Part::Wardrobe(i));
        }

        // 音量滑块与静音钮
        let mr = mute_rect();
        assert_eq!(part_at(mr[0] + 20.0, mr[1] + 10.0, false), Part::MuteBtn);
        assert_eq!(part_at((SLIDER_X0 + SLIDER_X1) * 0.5, ROW_VOICE_Y, false), Part::VoiceSlider);
        assert_eq!(part_at((SLIDER_X0 + SLIDER_X1) * 0.5, ROW_MUSIC_Y, false), Part::MusicSlider);
        assert_eq!(slider_pos_to_pct(SLIDER_X0), 0);
        assert_eq!(slider_pos_to_pct((SLIDER_X0 + SLIDER_X1) * 0.5), 50);
        assert_eq!(slider_pos_to_pct(SLIDER_X1), 100);

        // 底部 7 个工具胶囊总宽严格对齐 320px
        let last_foot = footer_rect(6);
        assert!((last_foot[0] + last_foot[2] - (PANEL_W - PAD_X)).abs() < 0.01);
        for i in 0..7 {
            let r = footer_rect(i);
            assert_eq!(part_at(r[0] + r[2] * 0.5, r[1] + r[3] * 0.5, false), Part::Footer(i));
        }
    }

    #[test]
    fn shapes_and_icons_render_headlessly_at_100_and_150_dpi() {
        let mut cfg = config::Config {
            costume: "maid".into(),
            outfit: vec!["glasses".into()],
            ..Default::default()
        };
        let pm1 = render_shapes_pixmap((348, 364), 1.0, Part::Action(4), DragSlider::None, &cfg, false, false).expect("100% dpi");
        assert_eq!((pm1.width(), pm1.height()), (348, 364));

        cfg.tts.muted = true;
        let pm2 = render_shapes_pixmap((522, 546), 1.5, Part::Wardrobe(3), DragSlider::Voice, &cfg, false, true).expect("150% dpi");
        assert_eq!((pm2.width(), pm2.height()), (522, 546));
    }
}
