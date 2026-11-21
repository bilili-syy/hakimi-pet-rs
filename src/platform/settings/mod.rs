//! 设置窗口：原生 Win32 控件（不用 webview / GUI 库）。点「⚙️ 设置…」时才创建，关掉就整个销毁，不常驻。
//!
//! - 左边分类列表（AI 聊天 / 语音 / 常规 / 备忘录），右边是对应的一页控件；底部「恢复默认」「保存」「取消」。
//! - 「备忘录」页例外：增删改直接写 memo.json（core::memo），不等底部「保存」。
//! - 只在保存时写 config.json：拿 `config::get()` 的副本改已知字段（未知字段原样留着），原子写回，
//!   然后走和菜单「重新加载设置」同一条路（`settings:reload`）让改动马上生效。
//! - 「测试连接」「试听」在后台线程跑，结果 PostMessage 回窗口。Key 不进日志。
//! - comctl32 v6 外观：程序没有清单，临时用一个激活上下文（ActCtx）创建控件。
//! - 纯逻辑（服务商预设、识别、校验）放在文件末尾，有单元测试。
use super::sys::wide;
use crate::core::config::{self, Config};
use crate::core::memo::{Dt, Repeat};
use std::cell::{Cell, RefCell};
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};
use windows::Win32::Foundation::{COLORREF, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DeleteObject, FF_DONTCARE,
    FW_BOLD, FW_NORMAL, GetMonitorInfoW, HBRUSH, HDC, HFONT, HGDIOBJ, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromWindow, OUT_DEFAULT_PRECIS, SetBkColor, SetTextColor,
};
use windows::Win32::System::ApplicationInstallationAndServicing::{ACTCTXW, ActivateActCtx, CreateActCtxW, DeactivateActCtx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{ICC_BAR_CLASSES, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIcon, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyWindow, GetDlgItem, GetWindowTextLengthW,
    GetWindowTextW, HICON, HMENU, IDC_ARROW, IDC_HAND, IDYES, IDNO, IsDialogMessageW, IsWindow, LoadCursorW,
    MB_ICONQUESTION, MB_ICONWARNING, MB_OK, MB_YESNO, MB_YESNOCANCEL, MSG, MessageBoxW, PostMessageW, RegisterClassExW,
    SW_HIDE, SW_RESTORE, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW, SetCursor, SetForegroundWindow, SetWindowPos,
    SetWindowTextW, ShowWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_NCDESTROY,
    WM_SETFONT, WM_SETICON, WNDCLASSEXW, WS_CAPTION, WS_CHILD, WS_CLIPCHILDREN, WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU,
};
use windows::core::{PCWSTR, w};

mod logic;
mod memo_page;
mod paint;
pub use logic::*;
use memo_page::*;
use paint::*;

const CLASS: PCWSTR = w!("HakimiPetRsSettings");
/// 客户区逻辑尺寸（96 DPI 下的像素）
const W: f32 = 540.0;
const H: f32 = 560.0;
/// 后台结果回窗口
const WM_RESULT: u32 = WM_APP + 20;
/// 试听用的音频 token（行为层不认识它，AudioDone 会被忽略）
const TEST_TOKEN: u64 = u64::MAX - 7;

// ---- 原始 Win32 常量（windows crate 里类型五花八门，这里统一成 u32）
const WS_TABSTOP: u32 = 0x0001_0000;
const WS_VSCROLL: u32 = 0x0020_0000;
const ES_MULTILINE: u32 = 0x4;
const ES_PASSWORD: u32 = 0x20;
const ES_AUTOVSCROLL: u32 = 0x40;
const ES_AUTOHSCROLL: u32 = 0x80;
const ES_NUMBER: u32 = 0x2000;
const BS_DEFPUSHBUTTON: u32 = 0x1;
const BS_OWNERDRAW: u32 = 0xB;
const LBS_OWNERDRAWFIXED: u32 = 0x10;
const LBS_HASSTRINGS: u32 = 0x40;
const WM_CTLCOLOREDIT: u32 = 0x133;
const WM_CTLCOLORLISTBOX: u32 = 0x134;
const WM_DRAWITEM: u32 = 0x2B;
const WM_ERASEBKGND: u32 = 0x14;
const EN_SETFOCUS: u32 = 0x100;
const EN_KILLFOCUS: u32 = 0x200;
const ODS_SELECTED: u32 = 0x1;
const ODS_DISABLED: u32 = 0x4;
const ODT_LISTBOX: u32 = 2;
const ODT_BUTTON: u32 = 4;
const BS_AUTOCHECKBOX: u32 = 0x3;
const CBS_DROPDOWN: u32 = 0x2;
const CBS_DROPDOWNLIST: u32 = 0x3;
const CBS_AUTOHSCROLL: u32 = 0x40;
const CBS_HASSTRINGS: u32 = 0x200;
const SS_NOTIFY: u32 = 0x100;
const SS_CENTERIMAGE: u32 = 0x200;
const LBS_NOTIFY: u32 = 0x1;
const LBS_NOINTEGRALHEIGHT: u32 = 0x100;
const TBS_NOTICKS: u32 = 0x10;
const CB_ADDSTRING: u32 = 0x143;
const CB_GETCURSEL: u32 = 0x147;
const CB_RESETCONTENT: u32 = 0x14B;
const CB_SETCURSEL: u32 = 0x14E;
const CBN_SELCHANGE: u32 = 1;
const LB_ADDSTRING: u32 = 0x180;
const LB_RESETCONTENT: u32 = 0x184;
const WM_TIMER: u32 = 0x113;
const LB_SETCURSEL: u32 = 0x186;
const LB_GETCURSEL: u32 = 0x188;
const LB_SETITEMHEIGHT: u32 = 0x1A0;
const LBN_SELCHANGE: u32 = 1;
const BM_GETCHECK: u32 = 0xF0;
const BM_SETCHECK: u32 = 0xF1;
const EM_SETPASSWORDCHAR: u32 = 0xCC;
/// 密码框的遮挡字符（U+2022 •）
const PASSWORD_DOT: usize = 0x2022;
const EM_LIMITTEXT: u32 = 0xC5;
const EM_SETCUEBANNER: u32 = 0x1501;
const TBM_GETPOS: u32 = 0x400;
const TBM_SETPOS: u32 = 0x405;
const TBM_SETRANGE: u32 = 0x406;
const WM_HSCROLL: u32 = 0x114;
const WM_CTLCOLORSTATIC: u32 = 0x138;
const WM_CTLCOLORBTN: u32 = 0x135;
const WM_SETCURSOR: u32 = 0x20;
const WM_DPICHANGED: u32 = 0x2E0;
const IDOK: i32 = 1;
const IDCANCEL: i32 = 2;

// ---- 控件 id
const ID_LIST: i32 = 100;
const ID_PROVIDER: i32 = 110;
const ID_KEY: i32 = 111;
const ID_KEY_SHOW: i32 = 112;
const ID_KEY_LINK: i32 = 113;
const ID_MODEL: i32 = 114;
const ID_BASE: i32 = 115;
const ID_PROXY: i32 = 116;
const ID_USER: i32 = 117;
const ID_TEST: i32 = 118;
const ID_TEST_RESULT: i32 = 119;
const ID_NOTE: i32 = 120;
const ID_TTS_ON: i32 = 130;
const ID_CHAT_VOICE: i32 = 131;
const ID_SPEAK: i32 = 132;
const ID_TTS_KEY: i32 = 133;
const ID_TTS_KEY_SHOW: i32 = 134;
const ID_TTS_LINK: i32 = 135;
const ID_MODE: i32 = 136;
const ID_VOICE: i32 = 137;
const ID_STYLE: i32 = 138;
const ID_VOLUME: i32 = 139;
const ID_VOLUME_LABEL: i32 = 140;
const ID_PLAY: i32 = 141;
const ID_PLAY_RESULT: i32 = 142;
const ID_AUTOSTART: i32 = 150;
const ID_FS_HIDE: i32 = 166;
const ID_SCALE: i32 = 151;
const ID_FPS: i32 = 164;
const ID_BIRTHDAY: i32 = 165;
const ID_CITY: i32 = 152;
const ID_MOYU: i32 = 153;
const ID_MOYU_MIN: i32 = 154;
const ID_MOYU_POMO: i32 = 155;
const ID_WATER: i32 = 156;
const ID_STAND: i32 = 157;
const ID_OFFWORK: i32 = 158;
const ID_DATADIR: i32 = 160;
const ID_FORGET: i32 = 161;
const ID_RAWFILE: i32 = 162;
const ID_RELOAD: i32 = 163;
const ID_DEFAULTS: i32 = 170;
const ID_UPDATE: i32 = 171;
const ID_UPDATE_RESULT: i32 = 172;
const ID_MEMO_LIST: i32 = 180;
const ID_MEMO_TEXT: i32 = 181;
const ID_MEMO_DATE: i32 = 182;
const ID_MEMO_TIME: i32 = 183;
const ID_MEMO_REPEAT: i32 = 184;
const ID_MEMO_NEW: i32 = 185;
const ID_MEMO_SAVE: i32 = 186;
const ID_MEMO_DONE: i32 = 187;
const ID_MEMO_DEL: i32 = 188;
const ID_MEMO_CLEAR: i32 = 189;
const ID_MEMO_MSG: i32 = 190;
/// 灰色说明文字
const ID_HINT0: i32 = 200;
/// 粗体标题
const ID_TITLE0: i32 = 300;
/// 普通标签
const ID_LABEL0: i32 = 400;

const PAGES: [&str; 4] = ["AI 聊天", "语音", "常规", "备忘录"];
const REPEATS: [(Repeat, &str); 4] = [(Repeat::None, "不重复"), (Repeat::Daily, "每天"), (Repeat::Workday, "工作日"), (Repeat::Weekly, "每周")];
const SPEAK: [(&str, &str); 3] = [("all", "所有气泡都念"), ("chat", "只念聊天回复"), ("off", "都不念")];
const MODES: [(&str, &str); 3] =
    [("clone", "哈基米本人的声音（推荐）"), ("preset", "MiMo 预置音色"), ("design", "按下面的文字描述现设计")];
const VOICES: [&str; 2] = ["冰糖", "茉莉"];
const SCALES: [(f32, &str); 5] =
    [(0.7, "小（70%）"), (0.85, "偏小（85%）"), (1.0, "标准（100%）"), (1.2, "偏大（120%）"), (1.5, "大（150%）")];
const FPS: [(u32, &str); 3] = [(30, "30"), (60, "60"), (120, "120")];
/// MiMo 开放平台 API Keys 页（curl 200）
pub const MIMO_KEY_URL: &str = "https://platform.xiaomimimo.com/#/console/api-keys";

// 配色和桌宠气泡一套：奶油底、深棕字、橙色强调
const TEXT: COLORREF = rgb(0x3a, 0x24, 0x16);
const GREY: COLORREF = rgb(0x9a, 0x85, 0x73);
const LINK: COLORREF = rgb(0xd9, 0x82, 0x2b);
const OK_GREEN: COLORREF = rgb(0x2e, 0x8b, 0x4a);
const ERR_RED: COLORREF = rgb(0xd0, 0x3b, 0x2f);
/// 右侧内容区底色
const BG: COLORREF = rgb(0xff, 0xfb, 0xf4);
/// 左侧导航栏底色
const SIDE: COLORREF = rgb(0xfb, 0xee, 0xdc);
/// 输入框底色
const FIELD: COLORREF = rgb(0xff, 0xff, 0xff);
/// 输入框/分隔线描边
const LINE: COLORREF = rgb(0xea, 0xd9, 0xc3);
/// 强调色（选中的导航、主按钮、聚焦的输入框）
const ACCENT: COLORREF = rgb(0xf5, 0xa6, 0x23);
const ACCENT_DOWN: COLORREF = rgb(0xdc, 0x8c, 0x10);
/// 次按钮
const BTN: COLORREF = rgb(0xff, 0xf1, 0xdc);
const BTN_DOWN: COLORREF = rgb(0xf6, 0xe0, 0xc2);
/// 左侧导航栏宽度（逻辑像素）
const SIDE_W: f32 = 128.0;
/// 底部按钮区上面那条分隔线
const FOOT_Y: f32 = H - 62.0;

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

/// 一个控件：逻辑坐标（DPI 变了重排用）+ 属于哪一页（0 = 每页都显示）
struct Ctl {
    h: HWND,
    r: [f32; 4],
    page: u8,
}

struct Dlg {
    hwnd: HWND,
    scale: f32,
    font: HFONT,
    bold: HFONT,
    link: HFONT,
    /// 导航栏（未选中 / 选中）
    nav: HFONT,
    nav_bold: HFONT,
    icon: Option<HICON>,
    ctls: Vec<Ctl>,
    /// 自绘的输入框圆角外框：(逻辑矩形, 里面那个 Edit, 页)
    frames: Vec<([f32; 4], HWND, u8)>,
    page: u8,
    /// 打开时的配置（取消/判断有没有改动用）
    orig: Config,
    orig_autostart: bool,
    /// 当前服务商预设（PRESETS 下标）
    preset: usize,
    /// 后台任务代号：窗口关掉/再点一次后旧结果作废
    test_gen: u64,
    play_gen: u64,
    update_gen: u64,
    /// 备忘录页：列表第 i 行是哪条、选中的那条、上次刷新时的数据版本
    memo_ids: Vec<u64>,
    memo_sel: Option<u64>,
    memo_ver: u64,
}

thread_local! {
    static DLG: RefCell<Option<Dlg>> = const { RefCell::new(None) };
    /// 白底画刷（WM_CTLCOLOR* 用；窗口销毁时删）
    static BRUSH: Cell<isize> = const { Cell::new(0) };
    /// 导航栏 / 输入框底色画刷
    static SIDE_BRUSH: Cell<isize> = const { Cell::new(0) };
    static FIELD_BRUSH: Cell<isize> = const { Cell::new(0) };
    /// 「测试连接」「试听」结果颜色：0 灰 / 1 绿 / -1 红
    static RESULT_COLOR: Cell<[i8; 2]> = const { Cell::new([0, 0]) };
}
static HWND_ATOMIC: AtomicIsize = AtomicIsize::new(0);
static GEN: AtomicU64 = AtomicU64::new(1);
/// (控件 id, 代号, 成功?, 文字)
static RESULTS: Mutex<Vec<(i32, u64, bool, String)>> = Mutex::new(Vec::new());
/// 测试连接时从本机模型服务读到的模型表：(代号, 模型名们, 选中的)
static FOUND_MODELS: Mutex<Option<(u64, Vec<String>, String)>> = Mutex::new(None);
static ACTCTX: AtomicIsize = AtomicIsize::new(0);

fn hinst() -> HINSTANCE {
    unsafe { GetModuleHandleW(None).map(|m| m.into()).unwrap_or_default() }
}

/// 设置窗口开着吗
pub fn is_open() -> bool {
    HWND_ATOMIC.load(Ordering::Relaxed) != 0
}

/// 主消息循环里先给设置窗口一次机会（Tab 切换焦点、回车=保存、Esc=取消）
pub fn dialog_message(msg: &MSG) -> bool {
    let h = HWND_ATOMIC.load(Ordering::Relaxed);
    if h == 0 {
        return false;
    }
    unsafe { IsDialogMessageW(HWND(h as *mut _), msg).as_bool() }
}

/// 打开设置窗口（已经开着就恢复并切到对应页提到前台）。page：0 AI / 1 语音 / 2 常规 / 3 备忘录
pub fn open(page: u8) {
    let h = HWND_ATOMIC.load(Ordering::Relaxed);
    if h != 0 {
        let hwnd = HWND(h as *mut _);
        unsafe {
            if IsWindow(Some(hwnd)).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
                let _ = SetForegroundWindow(hwnd);
                with(|d| d.show_page(page.min(3)));
                return;
            }
        }
        HWND_ATOMIC.store(0, Ordering::Relaxed);
    }
    if let Err(e) = create(page) {
        crate::core::log::log(&format!("settings window failed: {e}"));
    }
}

/// 主程序退出时关闭已打开的设置窗口
pub fn close_now() {
    let h = HWND_ATOMIC.load(Ordering::Acquire);
    if h != 0 {
        let hwnd = HWND(h as *mut _);
        unsafe {
            if IsWindow(Some(hwnd)).as_bool() {
                let _ = DestroyWindow(hwnd);
            }
        }
    }
}

/// comctl32 v6 的激活上下文（没清单的程序拿到新外观的标准做法）。失败就用老外观，不影响功能
fn actctx() -> Option<HANDLE> {
    let h = ACTCTX.load(Ordering::Relaxed);
    if h != 0 {
        return Some(HANDLE(h as *mut _));
    }
    const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
<dependency><dependentAssembly><assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/></dependentAssembly></dependency>
</assembly>"#;
    let path = std::env::temp_dir().join("hakimi-pet-comctl6.manifest");
    if std::fs::read_to_string(&path).ok().as_deref() != Some(MANIFEST) {
        std::fs::write(&path, MANIFEST).ok()?;
    }
    let p = wide(&path.to_string_lossy());
    let ctx = ACTCTXW { cbSize: std::mem::size_of::<ACTCTXW>() as u32, lpSource: PCWSTR(p.as_ptr()), ..Default::default() };
    let h = unsafe { CreateActCtxW(&ctx) }.ok()?;
    ACTCTX.store(h.0 as isize, Ordering::Relaxed);
    Some(h)
}

fn create(page: u8) -> windows::core::Result<()> {
    let ctx = actctx();
    let mut cookie = 0usize;
    let activated = ctx.is_some_and(|c| unsafe { ActivateActCtx(Some(c), &mut cookie).is_ok() });
    let r = create_inner(page);
    if activated {
        unsafe {
            let _ = DeactivateActCtx(0, cookie);
        }
    }
    r
}

fn create_inner(page: u8) -> windows::core::Result<()> {
    unsafe {
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_BAR_CLASSES | ICC_STANDARD_CLASSES,
        };
        let _ = InitCommonControlsEx(&icc);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst(),
            lpszClassName: CLASS,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            // 背景在 WM_ERASEBKGND 里自己画（导航栏、输入框圆角框、分隔线）
            hbrBackground: HBRUSH::default(),
            ..Default::default()
        };
        RegisterClassExW(&wc); // 第二次打开时已注册，失败无所谓

        // 用宠物所在显示器的 DPI，窗口放在那块屏幕中间
        let pet = super::main_hwnd();
        let dpi = pet.map(|h| GetDpiForWindow(h)).filter(|d| *d > 0).unwrap_or(96);
        let scale = dpi as f32 / 96.0;
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
        let mut rc = RECT { left: 0, top: 0, right: (W * scale).round() as i32, bottom: (H * scale).round() as i32 };
        let _ = AdjustWindowRectExForDpi(&mut rc, style, false, WINDOW_EX_STYLE(0), dpi);
        let (ww, wh) = (rc.right - rc.left, rc.bottom - rc.top);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let mon = MonitorFromWindow(pet.unwrap_or_default(), MONITOR_DEFAULTTONEAREST);
        let wa = if GetMonitorInfoW(mon, &mut mi).as_bool() {
            mi.rcWork
        } else {
            RECT { left: 0, top: 0, right: 1920, bottom: 1080 }
        };
        let x = wa.left + ((wa.right - wa.left) - ww).max(0) / 2;
        let y = wa.top + ((wa.bottom - wa.top) - wh).max(0) / 2;

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            w!("哈基米设置"),
            style,
            x,
            y,
            ww,
            wh,
            None,
            None,
            Some(hinst()),
            None,
        )?;
        HWND_ATOMIC.store(hwnd.0 as isize, Ordering::Relaxed);
        BRUSH.with(|b| b.set(CreateSolidBrush(BG).0 as isize));
        SIDE_BRUSH.with(|b| b.set(CreateSolidBrush(SIDE).0 as isize));
        FIELD_BRUSH.with(|b| b.set(CreateSolidBrush(FIELD).0 as isize));
        style_caption(hwnd);
        RESULT_COLOR.with(|c| c.set([0, 0]));

        let cfg = config::get();
        let mut d = Dlg {
            hwnd,
            scale,
            font: HFONT::default(),
            bold: HFONT::default(),
            link: HFONT::default(),
            nav: HFONT::default(),
            nav_bold: HFONT::default(),
            icon: make_icon(),
            ctls: Vec::new(),
            frames: Vec::new(),
            page: 0,
            orig: cfg.clone(),
            orig_autostart: super::sys::is_autostart(),
            preset: 0,
            test_gen: 0,
            play_gen: 0,
            update_gen: 0,
            memo_ids: Vec::new(),
            memo_sel: None,
            memo_ver: 0,
        };
        let _ = d.make_fonts();
        if let Some(i) = d.icon {
            SendMessageW(hwnd, WM_SETICON, Some(WPARAM(0)), Some(LPARAM(i.0 as isize)));
            SendMessageW(hwnd, WM_SETICON, Some(WPARAM(1)), Some(LPARAM(i.0 as isize)));
        }
        d.build();
        d.fill(&cfg, d.orig_autostart);
        d.show_page(page.min(3));
        DLG.with(|c| *c.borrow_mut() = Some(d));
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        Ok(())
    }
}

/// tray.png → 窗口图标（解码完就丢，只留 HICON）
fn make_icon() -> Option<HICON> {
    let png = crate::assets::get("tray.png")?;
    let pm = tiny_skia::Pixmap::decode_png(png).ok()?;
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let mut bgra = Vec::with_capacity(pm.data().len());
    for p in pm.pixels() {
        let c = p.demultiply();
        bgra.extend_from_slice(&[c.blue(), c.green(), c.red(), c.alpha()]);
    }
    // AND 掩码全 0（透明度由 alpha 决定），每行按 16 位对齐
    let mask = vec![0u8; (((w + 15) / 16 * 2) * h) as usize];
    unsafe { CreateIcon(Some(hinst()), w, h, 1, 32, mask.as_ptr(), bgra.as_ptr()).ok() }
}

// ---------------------------------------------------------------- 控件小工具

fn ctl(hwnd: HWND, id: i32) -> HWND {
    unsafe { GetDlgItem(Some(hwnd), id).unwrap_or_default() }
}

fn get_text(h: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h).max(0) as usize;
        let mut buf = vec![0u16; n + 1];
        let got = GetWindowTextW(h, &mut buf).max(0) as usize;
        String::from_utf16_lossy(&buf[..got.min(n)])
    }
}

fn set_text(h: HWND, s: &str) {
    let w = wide(s);
    unsafe {
        let _ = SetWindowTextW(h, PCWSTR(w.as_ptr()));
    }
}

fn send(h: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    unsafe { SendMessageW(h, msg, Some(WPARAM(wp)), Some(LPARAM(lp))).0 }
}

fn checked(h: HWND) -> bool {
    send(h, BM_GETCHECK, 0, 0) == 1
}

fn set_checked(h: HWND, on: bool) {
    send(h, BM_SETCHECK, on as usize, 0);
}

fn combo_fill(h: HWND, items: &[&str]) {
    send(h, CB_RESETCONTENT, 0, 0);
    for s in items {
        let w = wide(s);
        send(h, CB_ADDSTRING, 0, w.as_ptr() as isize);
    }
}

fn combo_sel(h: HWND) -> usize {
    send(h, CB_GETCURSEL, 0, 0).max(0) as usize
}

fn combo_set(h: HWND, i: usize) {
    send(h, CB_SETCURSEL, i, 0);
}

fn cue(h: HWND, s: &str) {
    let w = wide(s);
    send(h, EM_SETCUEBANNER, 1, w.as_ptr() as isize);
}

fn enable(h: HWND, on: bool) {
    unsafe {
        let _ = EnableWindow(h, on);
    }
}

fn repaint(h: HWND) {
    unsafe {
        let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(h), None, true);
    }
}

fn msgbox(owner: HWND, text: &str, flags: windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE) -> i32 {
    let t = wide(text);
    unsafe { MessageBoxW(Some(owner), PCWSTR(t.as_ptr()), w!("哈基米设置"), flags).0 }
}

impl Dlg {
    fn px(&self, v: f32) -> i32 {
        (v * self.scale).round() as i32
    }

    /// 先创建新字体并替换，返回旧字体列表（由调用方在所有控件 WM_SETFONT 之后再 DeleteObject，避免 UAF）
    fn make_fonts(&mut self) -> [HFONT; 5] {
        let old = [self.font, self.bold, self.link, self.nav, self.nav_bold];
        unsafe {
            let (body, bold) = ui_faces();
            let mk = |size: f32, weight: i32, underline: u32| {
                let face = if weight >= FW_BOLD.0 as i32 { bold } else { body };
                let fw: Vec<u16> = face.encode_utf16().chain(Some(0)).collect();
                CreateFontW(
                    -(size * self.scale).round() as i32,
                    0,
                    0,
                    0,
                    weight,
                    0,
                    underline,
                    0,
                    DEFAULT_CHARSET,
                    OUT_DEFAULT_PRECIS,
                    CLIP_DEFAULT_PRECIS,
                    CLEARTYPE_QUALITY,
                    FF_DONTCARE.0 as u32,
                    PCWSTR(fw.as_ptr()),
                )
            };
            self.font = mk(13.5, FW_NORMAL.0 as i32, 0);
            self.bold = mk(20.0, FW_BOLD.0 as i32, 0);
            self.link = mk(13.0, FW_NORMAL.0 as i32, 0);
            self.nav = mk(14.5, FW_NORMAL.0 as i32, 0);
            self.nav_bold = mk(14.5, FW_BOLD.0 as i32, 0);
        }
        old
    }

    fn delete_fonts(fonts: [HFONT; 5]) {
        unsafe {
            for f in fonts {
                if !f.is_invalid() {
                    let _ = DeleteObject(HGDIOBJ(f.0));
                }
            }
        }
    }

    fn font_for(&self, id: i32) -> HFONT {
        if id == ID_KEY_LINK || id == ID_TTS_LINK {
            self.link
        } else if (ID_TITLE0..ID_TITLE0 + 10).contains(&id) {
            self.bold
        } else {
            self.font
        }
    }

    /// 建一个子控件。r = 逻辑 [x, y, w, h]
    #[allow(clippy::too_many_arguments)]
    fn add(&mut self, page: u8, class: PCWSTR, text: &str, style: u32, ex: u32, id: i32, r: [f32; 4]) -> HWND {
        let t = wide(text);
        let h = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(ex),
                class,
                PCWSTR(t.as_ptr()),
                WS_CHILD | WINDOW_STYLE(style),
                self.px(r[0]),
                self.px(r[1]),
                self.px(r[2]),
                self.px(r[3]),
                Some(self.hwnd),
                Some(HMENU(id as isize as *mut _)),
                Some(hinst()),
                None,
            )
            .unwrap_or_default()
        };
        send(h, WM_SETFONT, self.font_for(id).0 as usize, 0);
        self.ctls.push(Ctl { h, r, page });
        h
    }

    fn label(&mut self, page: u8, text: &str, r: [f32; 4]) {
        let id = ID_LABEL0 + self.ctls.len() as i32;
        self.add(page, w!("STATIC"), text, SS_CENTERIMAGE, 0, id, r);
    }

    fn hint(&mut self, page: u8, text: &str, r: [f32; 4]) {
        let id = ID_HINT0 + self.ctls.len() as i32 % 100;
        self.add(page, w!("STATIC"), text, 0, 0, id, r);
    }

    fn title(&mut self, page: u8, text: &str) {
        self.add(page, w!("STATIC"), text, 0, 0, ID_TITLE0 + page as i32, [X0, 14.0, 360.0, 26.0]);
    }

    /// 输入框：没有系统边框，父窗口画圆角外框，Edit 缩在框里面
    fn edit(&mut self, page: u8, id: i32, style: u32, r: [f32; 4]) -> HWND {
        let multi = style & ES_MULTILINE != 0;
        let inner = if multi { [r[0] + 7.0, r[1] + 4.0, r[2] - 10.0, r[3] - 7.0] } else { [r[0] + 9.0, r[1] + 4.0, r[2] - 16.0, r[3] - 8.0] };
        let auto = if multi { 0 } else { ES_AUTOHSCROLL };
        let h = self.add(page, w!("EDIT"), "", WS_TABSTOP | auto | style, 0, id, inner);
        // 系统默认的 ● 在思源黑体里又大又挤，换成小圆点 •
        if style & ES_PASSWORD != 0 { send(h, EM_SETPASSWORDCHAR, PASSWORD_DOT, 0); }
        self.frames.push(([r[0], r[1] - 2.0, r[2], r[3] + 4.0], h, page));
        h
    }

    /// 普通按钮改成自绘（圆角、主按钮橙色）；复选框保持系统样式
    fn button(&mut self, page: u8, id: i32, text: &str, style: u32, r: [f32; 4]) -> HWND {
        let style = if style == 0 || style == BS_DEFPUSHBUTTON { BS_OWNERDRAW } else { style };
        self.add(page, w!("BUTTON"), text, WS_TABSTOP | style, 0, id, r)
    }

    fn combo(&mut self, page: u8, id: i32, editable: bool, r: [f32; 4]) -> HWND {
        let kind = if editable { CBS_DROPDOWN | CBS_AUTOHSCROLL } else { CBS_DROPDOWNLIST };
        // 组合框的高度含下拉列表
        self.add(page, w!("COMBOBOX"), "", WS_TABSTOP | WS_VSCROLL | CBS_HASSTRINGS | kind, 0, id, [r[0], r[1], r[2], 260.0])
    }

    fn build(&mut self) {
        let list = self.add(
            0,
            w!("LISTBOX"),
            "",
            WS_TABSTOP | LBS_NOTIFY | LBS_NOINTEGRALHEIGHT | LBS_OWNERDRAWFIXED | LBS_HASSTRINGS,
            0,
            ID_LIST,
            [10.0, 18.0, SIDE_W - 20.0, H - 36.0],
        );
        for p in PAGES {
            let w = wide(p);
            send(list, LB_ADDSTRING, 0, w.as_ptr() as isize);
        }
        send(list, LB_SETITEMHEIGHT, 0, self.px(40.0) as isize);

        // ---------------- AI 聊天
        let p = 1;
        self.title(p, "AI 聊天");
        self.label(p, "服务商", [X0, 52.0, LW, 26.0]);
        let prov = self.combo(p, ID_PROVIDER, false, [FX, 52.0, FW, 0.0]);
        combo_fill(prov, &PRESETS.iter().map(|x| x.name).collect::<Vec<_>>());
        // 服务商说明（灰字，3 行以内）
        self.add(p, w!("STATIC"), "", 0, 0, ID_NOTE, [FX, 82.0, FW, 58.0]);
        self.label(p, "API Key", [X0, 144.0, LW, 26.0]);
        let key = self.edit(p, ID_KEY, ES_PASSWORD, [FX, 144.0, FW - 64.0, 26.0]);
        send(key, EM_LIMITTEXT, 300, 0);
        self.button(p, ID_KEY_SHOW, "显示", BS_AUTOCHECKBOX, [FX + FW - 56.0, 144.0, 58.0, 26.0]);
        self.add(p, w!("STATIC"), "如何获取 API Key？（点这里打开官网）", SS_NOTIFY, 0, ID_KEY_LINK, [FX, 174.0, FW, 22.0]);
        self.label(p, "模型", [X0, 202.0, LW, 26.0]);
        self.combo(p, ID_MODEL, true, [FX, 202.0, FW, 0.0]);
        self.label(p, "接口地址", [X0, 238.0, LW, 26.0]);
        let base = self.edit(p, ID_BASE, 0, [FX, 238.0, FW, 26.0]);
        cue(base, "留空 = 用官方地址");
        self.label(p, "代理", [X0, 274.0, LW, 26.0]);
        let proxy = self.edit(p, ID_PROXY, 0, [FX, 274.0, FW, 26.0]);
        cue(proxy, "留空 = 跟随系统代理");
        self.label(p, "怎么称呼你", [X0, 310.0, LW, 26.0]);
        let user = self.edit(p, ID_USER, 0, [FX, 310.0, FW, 26.0]);
        cue(user, "留空就叫「你」");
        self.button(p, ID_TEST, "测试连接", 0, [FX, 352.0, 110.0, 30.0]);
        self.hint(p, "不用先保存，直接试", [FX + 118.0, 352.0, FW - 118.0, 30.0]);
        self.add(p, w!("STATIC"), "", 0, 0, ID_TEST_RESULT, [FX, 390.0, FW, 64.0]);

        // ---------------- 语音
        let p = 2;
        self.title(p, "语音");
        self.button(p, ID_TTS_ON, "让哈基米出声", BS_AUTOCHECKBOX, [X0, 50.0, 360.0, 24.0]);
        self.button(p, ID_CHAT_VOICE, "语音回答：聊天回复等声音准备好，再和文字一起出来", BS_AUTOCHECKBOX, [X0, 76.0, 380.0, 24.0]);
        self.label(p, "念哪些话", [X0, 106.0, LW, 26.0]);
        let speak = self.combo(p, ID_SPEAK, false, [FX, 106.0, FW, 0.0]);
        combo_fill(speak, &SPEAK.map(|x| x.1));
        self.label(p, "MiMo Key", [X0, 142.0, LW, 26.0]);
        let tkey = self.edit(p, ID_TTS_KEY, ES_PASSWORD, [FX, 142.0, FW - 64.0, 26.0]);
        send(tkey, EM_LIMITTEXT, 300, 0);
        self.button(p, ID_TTS_KEY_SHOW, "显示", BS_AUTOCHECKBOX, [FX + FW - 56.0, 142.0, 58.0, 26.0]);
        self.add(p, w!("STATIC"), "如何获取 MiMo API Key？（国内直连）", SS_NOTIFY, 0, ID_TTS_LINK, [FX, 172.0, FW, 22.0]);
        self.label(p, "声线", [X0, 200.0, LW, 26.0]);
        let mode = self.combo(p, ID_MODE, false, [FX, 200.0, FW, 0.0]);
        combo_fill(mode, &MODES.map(|x| x.1));
        self.label(p, "预置音色", [X0, 236.0, LW, 26.0]);
        let voice = self.combo(p, ID_VOICE, true, [FX, 236.0, FW, 0.0]);
        combo_fill(voice, &VOICES);
        self.label(p, "声音描述", [X0, 272.0, LW, 26.0]);
        // 三行高：不带滚动条，写长了用滚轮/方向键翻
        self.edit(p, ID_STYLE, ES_MULTILINE | ES_AUTOVSCROLL, [FX, 272.0, FW, 78.0]);
        self.label(p, "音量", [X0, 360.0, LW, 30.0]);
        let vol = self.add(p, w!("msctls_trackbar32"), "", WS_TABSTOP | TBS_NOTICKS, 0, ID_VOLUME, [FX - 6.0, 360.0, FW - 54.0, 30.0]);
        send(vol, TBM_SETRANGE, 1, (100 << 16) as isize);
        self.add(p, w!("STATIC"), "90%", SS_CENTERIMAGE, 0, ID_VOLUME_LABEL, [FX + FW - 52.0, 360.0, 52.0, 30.0]);
        self.button(p, ID_PLAY, "试听", 0, [FX, 400.0, 110.0, 30.0]);
        self.add(p, w!("STATIC"), "", 0, 0, ID_PLAY_RESULT, [FX + 118.0, 400.0, FW - 118.0, 44.0]);
        self.hint(p, "没填 Key 时只能念内置的固定台词；AI 聊天回复要有 Key 才能出声。", [X0, 452.0, W - X0 - 16.0, 40.0]);

        // ---------------- 常规
        let p = 3;
        self.title(p, "常规");
        self.button(p, ID_AUTOSTART, "开机自动启动", BS_AUTOCHECKBOX, [X0, 50.0, 130.0, 24.0]);
        self.button(p, ID_FS_HIDE, "全屏（游戏/视频）时自动隐藏", BS_AUTOCHECKBOX, [X0 + 140.0, 50.0, 260.0, 24.0]);
        self.label(p, "哈基米大小", [X0, 82.0, LW, 26.0]);
        let sc = self.combo(p, ID_SCALE, false, [FX, 82.0, 170.0, 0.0]);
        combo_fill(sc, &SCALES.map(|x| x.1));
        self.label(p, "帧率", [FX + 184.0, 82.0, 36.0, 26.0]);
        let fps = self.combo(p, ID_FPS, false, [FX + 222.0, 82.0, FW - 222.0, 0.0]);
        combo_fill(fps, &FPS.map(|x| x.1));
        self.label(p, "天气城市", [X0, 118.0, LW, 26.0]);
        let city = self.edit(p, ID_CITY, 0, [FX, 118.0, 170.0, 26.0]);
        cue(city, "留空 = 自动定位");
        self.label(p, "主人生日", [FX + 184.0, 118.0, 64.0, 26.0]);
        let bd = self.edit(p, ID_BIRTHDAY, 0, [FX + 250.0, 118.0, FW - 250.0, 26.0]);
        cue(bd, "如 4-3");
        self.button(p, ID_MOYU, "摸鱼监督（看视频/刷社交太久会来催你）", BS_AUTOCHECKBOX, [X0, 156.0, 360.0, 24.0]);
        self.label(p, "连续摸鱼", [X0 + 18.0, 186.0, 70.0, 26.0]);
        self.edit(p, ID_MOYU_MIN, ES_NUMBER, [X0 + 88.0, 186.0, 46.0, 26.0]);
        self.label(p, "分钟就催，番茄钟里", [X0 + 140.0, 186.0, 132.0, 26.0]);
        self.edit(p, ID_MOYU_POMO, ES_NUMBER, [X0 + 274.0, 186.0, 46.0, 26.0]);
        self.label(p, "分钟", [X0 + 326.0, 186.0, 40.0, 26.0]);
        self.label(p, "喝水提醒：每", [X0, 226.0, 88.0, 26.0]);
        self.edit(p, ID_WATER, ES_NUMBER, [X0 + 88.0, 226.0, 46.0, 26.0]);
        self.label(p, "分钟　起身提醒：每", [X0 + 140.0, 226.0, 132.0, 26.0]);
        self.edit(p, ID_STAND, ES_NUMBER, [X0 + 274.0, 226.0, 46.0, 26.0]);
        self.label(p, "分钟", [X0 + 326.0, 226.0, 40.0, 26.0]);
        self.label(p, "下班提醒", [X0, 262.0, 88.0, 26.0]);
        let off = self.edit(p, ID_OFFWORK, 0, [X0 + 88.0, 262.0, 70.0, 26.0]);
        cue(off, "18:00");
        self.hint(p, "填 0 或留空 = 不提醒。番茄钟在右键「🧰 工具」里开。", [X0 + 166.0, 262.0, W - X0 - 182.0, 40.0]);
        self.button(p, ID_DATADIR, "打开数据文件夹", 0, [X0, 318.0, 170.0, 30.0]);
        self.button(p, ID_FORGET, "清空聊天记录", 0, [X0 + 182.0, 318.0, 170.0, 30.0]);
        self.button(p, ID_RAWFILE, "编辑设置文件（高级）", 0, [X0, 356.0, 170.0, 30.0]);
        self.button(p, ID_RELOAD, "重新读取设置文件", 0, [X0 + 182.0, 356.0, 170.0, 30.0]);
        self.hint(p, "高级：摸鱼关键词、进程名等在设置文件里改，改完点「重新读取设置文件」。", [X0, 394.0, W - X0 - 16.0, 40.0]);
        self.button(p, ID_UPDATE, "检查更新", 0, [X0, 438.0, 170.0, 30.0]);
        self.add(p, w!("STATIC"), &format!("当前版本 v{}", crate::services::update::CURRENT), SS_CENTERIMAGE, 0, ID_UPDATE_RESULT, [X0 + 182.0, 438.0, W - X0 - 198.0, 30.0]);

        // ---------------- 备忘录（改动马上写进 memo.json，不用点「保存」）
        let p = 4;
        self.title(p, "备忘录");
        self.hint(p, "这一页改动马上生效。聊天里说「提醒我…」也能加。", [X0, 44.0, W - X0 - 16.0, 20.0]);
        let mw = W - X0 - 16.0;
        let list = self.add(p, w!("LISTBOX"), "", WS_TABSTOP | WS_VSCROLL | LBS_NOTIFY | LBS_NOINTEGRALHEIGHT, 0, ID_MEMO_LIST, [X0 + 6.0, 72.0, mw - 10.0, 164.0]);
        self.frames.push(([X0, 68.0, mw, 172.0], list, p));
        self.label(p, "内容", [X0, 252.0, LW, 26.0]);
        let t = self.edit(p, ID_MEMO_TEXT, 0, [FX, 252.0, FW, 26.0]);
        send(t, EM_LIMITTEXT, 200, 0);
        cue(t, "比如：喝水、开会、给妈妈打电话");
        self.label(p, "日期", [X0, 288.0, LW, 26.0]);
        let d = self.edit(p, ID_MEMO_DATE, 0, [FX, 288.0, 120.0, 26.0]);
        cue(d, "今天 / 11-21");
        self.label(p, "时间", [FX + 132.0, 288.0, 40.0, 26.0]);
        let tm = self.edit(p, ID_MEMO_TIME, 0, [FX + 172.0, 288.0, FW - 172.0, 26.0]);
        cue(tm, "18:00");
        self.label(p, "重复", [X0, 324.0, LW, 26.0]);
        let rp = self.combo(p, ID_MEMO_REPEAT, false, [FX, 324.0, 120.0, 0.0]);
        combo_fill(rp, &REPEATS.map(|x| x.1));
        combo_set(rp, 0);
        self.hint(p, "时间空着 = 不提醒", [FX + 132.0, 328.0, FW - 132.0, 22.0]);
        let bw = (mw - 30.0) / 4.0;
        for (i, (id, s)) in [(ID_MEMO_NEW, "新建"), (ID_MEMO_SAVE, "添加这条"), (ID_MEMO_DONE, "完成"), (ID_MEMO_DEL, "删除")].into_iter().enumerate() {
            self.button(p, id, s, 0, [X0 + i as f32 * (bw + 10.0), 366.0, bw, 30.0]);
        }
        self.button(p, ID_MEMO_CLEAR, "清空已完成", 0, [X0, 406.0, 110.0, 30.0]);
        self.add(p, w!("STATIC"), "", SS_CENTERIMAGE, 0, ID_MEMO_MSG, [X0 + 122.0, 406.0, mw - 122.0, 30.0]);

        // ---------------- 底部按钮（每页都有）
        let y = H - 50.0;
        self.button(0, ID_DEFAULTS, "恢复默认", 0, [X0, y, 100.0, 32.0]);
        self.button(0, IDOK, "保存", BS_DEFPUSHBUTTON, [W - 216.0, y, 96.0, 32.0]);
        self.button(0, IDCANCEL, "取消", 0, [W - 112.0, y, 96.0, 32.0]);
    }

    fn c(&self, id: i32) -> HWND {
        ctl(self.hwnd, id)
    }

    /// 把配置填进控件
    fn fill(&mut self, c: &Config, autostart: bool) {
        self.preset = detect_preset(&c.provider, &c.base_url);
        combo_set(self.c(ID_PROVIDER), self.preset);
        set_text(self.c(ID_KEY), &c.api_key);
        set_text(self.c(ID_BASE), &c.base_url);
        set_text(self.c(ID_PROXY), &c.proxy);
        set_text(self.c(ID_USER), &c.user_name);
        self.models_for_preset(&c.model);
        self.update_ai_hints();

        set_checked(self.c(ID_TTS_ON), c.tts.enabled);
        set_checked(self.c(ID_CHAT_VOICE), c.tts.chat_voice_reply);
        combo_set(self.c(ID_SPEAK), SPEAK.iter().position(|x| x.0 == c.tts.speak).unwrap_or(0));
        set_text(self.c(ID_TTS_KEY), &c.tts.api_key);
        combo_set(self.c(ID_MODE), MODES.iter().position(|x| x.0 == c.tts.mode).unwrap_or(0));
        set_text(self.c(ID_VOICE), &c.tts.voice);
        set_text(self.c(ID_STYLE), &c.tts.style);
        let v = (c.tts.volume.clamp(0.0, 1.0) * 100.0).round() as isize;
        send(self.c(ID_VOLUME), TBM_SETPOS, 1, v);
        self.update_volume_label();
        self.update_tts_enable();

        set_checked(self.c(ID_AUTOSTART), autostart);
        set_checked(self.c(ID_FS_HIDE), c.fullscreen_hide);
        combo_set(self.c(ID_SCALE), nearest_scale(c.scale));
        combo_set(self.c(ID_FPS), FPS.iter().position(|x| x.0 == crate::core::config::clamp_fps(c.fps)).unwrap_or(1));
        set_text(self.c(ID_CITY), &c.weather_city);
        set_text(self.c(ID_BIRTHDAY), &c.birthday);
        set_checked(self.c(ID_MOYU), c.moyu.enabled);
        set_text(self.c(ID_MOYU_MIN), &c.moyu.minutes.to_string());
        set_text(self.c(ID_MOYU_POMO), &c.moyu.pomodoro_minutes.to_string());
        set_text(self.c(ID_WATER), &c.reminders.water.to_string());
        set_text(self.c(ID_STAND), &c.reminders.stand_up.to_string());
        set_text(self.c(ID_OFFWORK), &c.reminders.off_work);
        self.update_moyu_enable();
    }

    /// 按当前预设填模型下拉；keep 是要保留的模型名（在列表里或是自定义的都留着）
    fn models_for_preset(&self, keep: &str) {
        let p = &PRESETS[self.preset];
        let h = self.c(ID_MODEL);
        combo_fill(h, p.models);
        let model = if keep.trim().is_empty() { p.models.first().copied().unwrap_or("") } else { keep };
        match p.models.iter().position(|m| *m == model) {
            Some(i) => combo_set(h, i),
            None => set_text(h, model),
        }
    }

    fn update_ai_hints(&self) {
        let p = &PRESETS[self.preset];
        let link = self.c(ID_KEY_LINK);
        unsafe {
            let _ = ShowWindow(link, if !p.key_url.is_empty() && self.page == 1 { SW_SHOW } else { SW_HIDE });
        }
        set_text(self.c(ID_NOTE), p.note);
        let local = preset_is_local(self.preset);
        set_text(link, if local { "还没装 Ollama？（点这里打开下载页）" } else { "如何获取 API Key？（点这里打开官网）" });
        cue(self.c(ID_KEY), if local { "本机模型不用填" } else { "" });
        let base = self.c(ID_BASE);
        cue(base, if p.provider == "gemini" { "留空 = 用官方地址（一般不用改）" } else { "https://…/v1" });
    }

    fn update_volume_label(&self) {
        let v = send(self.c(ID_VOLUME), TBM_GETPOS, 0, 0);
        set_text(self.c(ID_VOLUME_LABEL), &format!("{v}%"));
    }

    fn update_tts_enable(&self) {
        let on = checked(self.c(ID_TTS_ON));
        for id in [ID_CHAT_VOICE, ID_SPEAK, ID_MODE, ID_VOICE, ID_STYLE, ID_VOLUME] {
            enable(self.c(id), on);
        }
        let mode = MODES[combo_sel(self.c(ID_MODE)).min(2)].0;
        enable(self.c(ID_VOICE), on && mode == "preset");
        enable(self.c(ID_STYLE), on && mode != "clone");
        repaint(self.hwnd);
    }

    fn update_moyu_enable(&self) {
        let on = checked(self.c(ID_MOYU));
        enable(self.c(ID_MOYU_MIN), on);
        enable(self.c(ID_MOYU_POMO), on);
        repaint(self.hwnd);
    }

    fn show_page(&mut self, idx: u8) {
        self.page = idx + 1;
        send(self.c(ID_LIST), LB_SETCURSEL, idx as usize, 0);
        for c in &self.ctls {
            let show = c.page == 0 || c.page == self.page;
            unsafe {
                let _ = ShowWindow(c.h, if show { SW_SHOW } else { SW_HIDE });
            }
        }
        // 备忘录页不走「保存」，「恢复默认」藏起来免得误会；只在备忘录页开 1 秒刷新定时器
        unsafe {
            let _ = ShowWindow(self.c(ID_DEFAULTS), if self.page == 4 { SW_HIDE } else { SW_SHOW });
            if self.page == 4 {
                windows::Win32::UI::WindowsAndMessaging::SetTimer(Some(self.hwnd), 1, 1000, None);
            } else {
                let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(Some(self.hwnd), 1);
            }
        }
        if self.page == 4 {
            self.memo_refresh();
        }
        // 自定义服务商没有官网链接
        self.update_ai_hints();
        repaint(self.hwnd);
    }

    // ---------------- 备忘录页
    /// 从控件读回配置（在打开时的副本上改，未知字段都还在）。校验失败返回 (页, 提示)
    fn collect(&self) -> Result<(Config, bool), (u8, String)> {
        let mut c = config::get();
        // 别人（菜单开关、换装）在设置窗口开着时改的字段以文件为准；设置窗口只覆盖自己管的字段
        let p = &PRESETS[self.preset];
        c.provider = p.provider.into();
        c.api_key = get_text(self.c(ID_KEY)).trim().to_string();
        c.base_url = get_text(self.c(ID_BASE)).trim().to_string();
        c.model = get_text(self.c(ID_MODEL)).trim().to_string();
        c.proxy = get_text(self.c(ID_PROXY)).trim().to_string();
        c.user_name = get_text(self.c(ID_USER)).trim().to_string();
        if let Some(e) = validate_ai(self.preset, &c.base_url, &c.model) {
            return Err((0, e));
        }

        c.tts.enabled = checked(self.c(ID_TTS_ON));
        c.tts.chat_voice_reply = checked(self.c(ID_CHAT_VOICE));
        c.tts.speak = SPEAK[combo_sel(self.c(ID_SPEAK)).min(2)].0.into();
        c.tts.api_key = get_text(self.c(ID_TTS_KEY)).trim().to_string();
        c.tts.mode = MODES[combo_sel(self.c(ID_MODE)).min(2)].0.into();
        let voice = get_text(self.c(ID_VOICE)).trim().to_string();
        c.tts.voice = if voice.is_empty() { "冰糖".into() } else { voice };
        let style = get_text(self.c(ID_STYLE)).trim().to_string();
        if !style.is_empty() {
            c.tts.style = style;
        }
        c.tts.volume = send(self.c(ID_VOLUME), TBM_GETPOS, 0, 0).clamp(0, 100) as f32 / 100.0;

        let autostart = checked(self.c(ID_AUTOSTART));
        c.fullscreen_hide = checked(self.c(ID_FS_HIDE));
        c.scale = SCALES[combo_sel(self.c(ID_SCALE)).min(SCALES.len() - 1)].0;
        c.fps = FPS[combo_sel(self.c(ID_FPS)).min(FPS.len() - 1)].0;
        c.weather_city = get_text(self.c(ID_CITY)).trim().to_string();
        let bd = get_text(self.c(ID_BIRTHDAY));
        c.birthday = crate::behavior::norm_birthday(&bd).unwrap_or_default();
        c.moyu.enabled = checked(self.c(ID_MOYU));
        let num = |id: i32, name: &str, max: u32| -> Result<u32, (u8, String)> {
            parse_minutes(&get_text(self.c(id)), max).ok_or((2, format!("「{name}」要填 0~{max} 的整数")))
        };
        c.moyu.minutes = num(ID_MOYU_MIN, "连续摸鱼分钟", 600)?.max(1);
        c.moyu.pomodoro_minutes = num(ID_MOYU_POMO, "番茄钟里催的分钟", 600)?.max(1);
        c.reminders.water = num(ID_WATER, "喝水提醒", 1440)?;
        c.reminders.stand_up = num(ID_STAND, "起身提醒", 1440)?;
        c.reminders.off_work = parse_clock(&get_text(self.c(ID_OFFWORK)))
            .ok_or((2, "「下班提醒」要填 18:00 这样的时间，留空 = 不提醒".to_string()))?;
        Ok((c, autostart))
    }

    fn dirty(&self) -> bool {
        match self.collect() {
            Ok((c, a)) => a != self.orig_autostart || differs(&self.orig, &c),
            Err(_) => true,
        }
    }

    fn relayout(&mut self, scale: f32) {
        self.scale = scale;
        let old_fonts = self.make_fonts();
        for c in &self.ctls {
            let id = unsafe { windows::Win32::UI::WindowsAndMessaging::GetDlgCtrlID(c.h) };
            // 先换新字体再挪位置（SetWindowPos 会触发重绘），最后统一销毁旧字体
            send(c.h, WM_SETFONT, self.font_for(id).0 as usize, 0);
            let r = c.r;
            unsafe {
                let _ = SetWindowPos(
                    c.h,
                    None,
                    self.px(r[0]),
                    self.px(r[1]),
                    self.px(r[2]),
                    self.px(r[3]),
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
        send(self.c(ID_LIST), LB_SETITEMHEIGHT, 0, self.px(40.0) as isize);
        Self::delete_fonts(old_fonts);
        repaint(self.hwnd);
    }
}

/// 只比较设置窗口管的字段（菜单开关等别处改的不算「有改动」）
fn differs(a: &Config, b: &Config) -> bool {
    let pick = |c: &Config| {
        (
            c.provider.clone(),
            c.api_key.clone(),
            c.base_url.clone(),
            c.model.clone(),
            c.proxy.clone(),
            c.user_name.clone(),
            format!("{:?}{:?}{:?}{:?}{:?}{:?}{}", c.tts.enabled, c.tts.chat_voice_reply, c.tts.speak, c.tts.api_key, c.tts.mode, c.tts.voice, c.tts.style),
            ((c.tts.volume * 100.0).round() as i32, (c.scale * 100.0).round() as i32, crate::core::config::clamp_fps(c.fps)),
            (c.weather_city.clone(), c.birthday.clone()),
            (c.moyu.enabled, c.moyu.minutes, c.moyu.pomodoro_minutes, c.reminders.water, c.reminders.stand_up),
            c.reminders.off_work.clone(),
            c.fullscreen_hide,
        )
    };
    pick(a) != pick(b)
}

const X0: f32 = 140.0;
const LW: f32 = 84.0;
const FX: f32 = 228.0;
const FW: f32 = W - FX - 18.0;

fn with<R>(f: impl FnOnce(&mut Dlg) -> R) -> Option<R> {
    DLG.with(|c| {
        let mut g = c.try_borrow_mut().ok()?;
        g.as_mut().map(f)
    })
}

/// 菜单动作（走主窗口的菜单队列，和点菜单一样）
fn menu(id: &str) {
    super::menu::push_id(id.to_string());
}

fn on_command(hwnd: HWND, id: i32, code: u32) {
    match (id, code) {
        (ID_LIST, LBN_SELCHANGE) => {
            with(|d| {
                let i = send(d.c(ID_LIST), LB_GETCURSEL, 0, 0).clamp(0, 3) as u8;
                d.show_page(i);
            });
        }
        (ID_PROVIDER, CBN_SELCHANGE) => {
            with(|d| {
                let old = d.preset;
                let new = combo_sel(d.c(ID_PROVIDER)).min(PRESETS.len() - 1);
                if old == new {
                    return;
                }
                d.preset = new;
                let base = get_text(d.c(ID_BASE));
                set_text(d.c(ID_BASE), &switch_base(old, new, &base));
                // 换了服务商：模型换成新服务商的第一个（自定义保留原来填的）
                let keep = if PRESETS[new].models.is_empty() { get_text(d.c(ID_MODEL)) } else { String::new() };
                d.models_for_preset(&keep);
                d.update_ai_hints();
                set_text(d.c(ID_TEST_RESULT), "");
            });
        }
        (ID_KEY_SHOW, 0) | (ID_TTS_KEY_SHOW, 0) => {
            with(|d| {
                let (edit, chk) = if id == ID_KEY_SHOW { (ID_KEY, ID_KEY_SHOW) } else { (ID_TTS_KEY, ID_TTS_KEY_SHOW) };
                let e = d.c(edit);
                send(e, EM_SETPASSWORDCHAR, if checked(d.c(chk)) { 0 } else { PASSWORD_DOT }, 0);
                unsafe {
                    let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(e), None, true);
                }
            });
        }
        (ID_KEY_LINK, 0) => {
            if let Some(url) = with(|d| PRESETS[d.preset].key_url)
                && !url.is_empty()
            {
                super::sys::open_url(url);
            }
        }
        (ID_TTS_LINK, 0) => super::sys::open_url(MIMO_KEY_URL),
        (ID_TEST, 0) => start_test(),
        (ID_UPDATE, 0) => start_update(hwnd),
        (ID_PLAY, 0) => start_play(),
        (ID_TTS_ON, 0) | (ID_MODE, CBN_SELCHANGE) => {
            with(|d| d.update_tts_enable());
        }
        (ID_MOYU, 0) => {
            with(|d| d.update_moyu_enable());
        }
        (ID_DATADIR, 0) => menu("datadir"),
        (ID_RAWFILE, 0) => menu("settings:file"),
        (ID_FORGET, 0) => {
            if msgbox(hwnd, "确定清空和哈基米的聊天记录吗？清空后她就不记得之前聊过什么了。", MB_YESNO | MB_ICONQUESTION) == IDYES.0 {
                menu("forget");
            }
        }
        (ID_RELOAD, 0) => {
            let c = config::reload();
            let a = super::sys::is_autostart();
            with(|d| {
                d.orig = c.clone();
                d.orig_autostart = a;
                d.fill(&c, a);
            });
            menu("settings:reload");
        }
        (ID_DEFAULTS, 0) => {
            if msgbox(hwnd, "把这里的设置都恢复成默认值吗？（API Key 会保留；点「保存」后才生效）", MB_YESNO | MB_ICONQUESTION) == IDYES.0 {
                with(|d| {
                    let cur = d.collect().map(|x| x.0).unwrap_or_else(|_| d.orig.clone());
                    let def = Config {
                        api_key: cur.api_key,
                        tts: config::TtsConfig { api_key: cur.tts.api_key, ..Default::default() },
                        ..Default::default()
                    };
                    d.fill(&def, false);
                });
            }
        }
        (ID_MEMO_LIST, LBN_SELCHANGE) => {
            with(|d| d.memo_pick());
        }
        (ID_MEMO_NEW, 0) => {
            with(|d| {
                d.memo_clear_form();
                d.memo_msg("");
                unsafe {
                    let _ = SetFocus(Some(d.c(ID_MEMO_TEXT)));
                }
            });
        }
        (ID_MEMO_SAVE, 0) => memo_save_ui(hwnd),
        (ID_MEMO_DONE, 0) => {
            if let Some(Some(id)) = with(|d| d.memo_sel) {
                // 走菜单同一条路：她会说一句「划掉啦」；50ms 后立刻刷新列表免得等 1 秒
                menu(&format!("memo:done:{id}"));
                with(|d| {
                    d.memo_clear_form();
                    d.memo_msg("✔ 划掉啦");
                });
                unsafe { windows::Win32::UI::WindowsAndMessaging::SetTimer(Some(hwnd), 2, 50, None); }
            }
        }
        (ID_MEMO_DEL, 0) => {
            if let Some(Some(id)) = with(|d| d.memo_sel)
                && msgbox(hwnd, "删掉这条备忘吗？", MB_YESNO | MB_ICONQUESTION) == IDYES.0
            {
                menu(&format!("memo:del:{id}"));
                with(|d| {
                    d.memo_clear_form();
                    d.memo_msg("已删除");
                });
                unsafe { windows::Win32::UI::WindowsAndMessaging::SetTimer(Some(hwnd), 2, 50, None); }
            }
        }
        (ID_MEMO_CLEAR, 0) => {
            menu("memo:clear");
            with(|d| d.memo_msg("已清掉做完的"));
            unsafe { windows::Win32::UI::WindowsAndMessaging::SetTimer(Some(hwnd), 2, 50, None); }
        }
        (IDOK, 0) => {
            // 备忘录页在输入框里按回车 = 添加/保存这条，不是保存设置并关窗
            let in_memo = with(|d| {
                let f = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() };
                d.page == 4 && [ID_MEMO_TEXT, ID_MEMO_DATE, ID_MEMO_TIME].iter().any(|i| d.c(*i) == f)
            })
            .unwrap_or(false);
            if in_memo { memo_save_ui(hwnd) } else { save(hwnd) }
        }
        (IDCANCEL, 0) => close(hwnd, false),
        _ => {}
    }
}

fn save(hwnd: HWND) {
    let Some(r) = with(|d| d.collect()) else { return };
    let (c, autostart) = match r {
        Ok(x) => x,
        Err((page, msg)) => {
            with(|d| d.show_page(page));
            msgbox(hwnd, &msg, MB_OK | MB_ICONWARNING);
            return;
        }
    };
    let orig_auto = with(|d| d.orig_autostart).unwrap_or(autostart);
    if !config::replace(c) {
        msgbox(hwnd, "保存失败：写不进设置文件（%APPDATA%\\hakimi-pet\\config.json）。", MB_OK | MB_ICONWARNING);
        return;
    }
    if autostart != orig_auto {
        super::sys::set_autostart(autostart);
    }
    // 和菜单「重新加载设置」同一条路：重排提醒、启停摸鱼监督、套用大小、她说一句「设置换好了」
    menu("settings:reload");
    destroy(hwnd);
}

/// 关窗口。ask = 有改动时问一下要不要保存
fn close(hwnd: HWND, ask: bool) {
    if ask && with(|d| d.dirty()).unwrap_or(false) {
        let r = msgbox(hwnd, "有修改还没保存，要保存吗？", MB_YESNOCANCEL | MB_ICONQUESTION);
        if r == IDYES.0 {
            return save(hwnd);
        }
        if r != IDNO.0 {
            return;
        }
    }
    destroy(hwnd);
}

fn destroy(hwnd: HWND) {
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

fn start_update(hwnd: HWND) {
    // 已经查到新版本：再点一次就打开下载页
    if UPDATE_FOUND.load(Ordering::Relaxed) {
        super::sys::open_url(crate::services::update::RELEASES_URL);
        return;
    }
    let Some(tag) = with(|d| {
        d.update_gen = GEN.fetch_add(1, Ordering::Relaxed);
        set_text(d.c(ID_UPDATE_RESULT), "正在检查…");
        enable(d.c(ID_UPDATE), false);
        d.update_gen
    }) else {
        return;
    };
    let h = hwnd.0 as isize;
    let spawned = std::thread::Builder::new().name("hakimi-update".into()).spawn(move || {
        let cur = crate::services::update::CURRENT;
        let (ok, text) = match crate::services::update::check() {
            Ok((t, true)) => {
                UPDATE_FOUND.store(true, Ordering::Relaxed);
                (true, format!("有新版本 {t}！再点一次去下载"))
            }
            Ok((_, false)) => (true, format!("v{cur} 已经是最新版")),
            Err(e) => (false, format!("✘ {e}")),
        };
        post_result(h, ID_UPDATE_RESULT, tag, ok, text);
    });
    if spawned.is_err() {
        post_result(h, ID_UPDATE_RESULT, tag, false, "✘ 开不了后台线程".into());
    }
}

/// 查到过新版本（按钮变成「去下载」）
static UPDATE_FOUND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn start_test() {
    let Some((hwnd, params, tag)) = with(|d| {
        d.test_gen = GEN.fetch_add(1, Ordering::Relaxed);
        let p = crate::services::ai::TestParams {
            provider: PRESETS[d.preset].provider.into(),
            base_url: get_text(d.c(ID_BASE)),
            api_key: get_text(d.c(ID_KEY)),
            model: get_text(d.c(ID_MODEL)),
            proxy: get_text(d.c(ID_PROXY)),
        };
        set_text(d.c(ID_TEST_RESULT), "正在连接…");
        RESULT_COLOR.with(|c| c.set([0, c.get()[1]]));
        enable(d.c(ID_TEST), false);
        (d.hwnd.0 as isize, p, d.test_gen)
    }) else {
        return;
    };
    let spawned = std::thread::Builder::new().name("hakimi-settings-test".into()).spawn(move || {
        let mut params = params;
        let mut extra = String::new();
        // 本机 / 局域网的服务（Ollama 等）：先列出装了哪些模型，模型没填或填的没装就换成第一个
        if params.provider == "openai" && crate::core::config::is_local_url(&params.base_url) {
            match crate::services::ai::list_models(&params) {
                Ok(list) if list.is_empty() => {
                    return post_result(hwnd, ID_TEST_RESULT, tag, false, "✘ 服务开着，但还没有模型：命令行运行 ollama pull qwen3:8b 再试".into());
                }
                Ok(list) => {
                    let cur = params.model.trim().to_string();
                    let chosen = if list.contains(&cur) { cur } else { list[0].clone() };
                    params.model = chosen.clone();
                    let shown: Vec<&str> = list.iter().take(6).map(String::as_str).collect();
                    extra = format!("\n已装 {} 个：{}{}", list.len(), shown.join("、"), if list.len() > 6 { "…" } else { "" });
                    *FOUND_MODELS.lock().unwrap_or_else(|e| e.into_inner()) = Some((tag, list, chosen));
                }
                Err(e) => return post_result(hwnd, ID_TEST_RESULT, tag, false, format!("✘ {e}")),
            }
        }
        let (ok, text) = match crate::services::ai::test_connection(&params) {
            Ok((ms, reply)) => (true, format!("✔ 连接成功！用时 {:.1} 秒。她回了：{reply}{extra}", ms as f64 / 1000.0)),
            Err(e) => (false, format!("✘ {e}{extra}")),
        };
        post_result(hwnd, ID_TEST_RESULT, tag, ok, text);
    });
    if spawned.is_err() {
        post_result(hwnd, ID_TEST_RESULT, tag, false, "✘ 开不了后台线程".into());
    }
}

fn start_play() {
    let Some((hwnd, t, tag)) = with(|d| {
        d.play_gen = GEN.fetch_add(1, Ordering::Relaxed);
        let mut t = d.orig.tts.clone();
        t.api_key = get_text(d.c(ID_TTS_KEY)).trim().to_string();
        t.mode = MODES[combo_sel(d.c(ID_MODE)).min(2)].0.into();
        let v = get_text(d.c(ID_VOICE)).trim().to_string();
        if !v.is_empty() {
            t.voice = v;
        }
        let s = get_text(d.c(ID_STYLE)).trim().to_string();
        if !s.is_empty() {
            t.style = s;
        }
        t.volume = send(d.c(ID_VOLUME), TBM_GETPOS, 0, 0).clamp(0, 100) as f32 / 100.0;
        set_text(d.c(ID_PLAY_RESULT), "正在合成…");
        RESULT_COLOR.with(|c| c.set([c.get()[0], 0]));
        enable(d.c(ID_PLAY), false);
        (d.hwnd.0 as isize, t, d.play_gen)
    }) else {
        return;
    };
    use crate::services::tts;
    // 没填 Key：哈基米声线有内置录音，直接放（试试音量）
    if t.api_key.is_empty() {
        let (ok, text) = match tts::bundled(&t, tts::SAMPLE) {
            Some(b) => {
                crate::services::audio::play(TEST_TOKEN, crate::types::Audio(b.to_vec()), t.volume);
                (true, "♪ 放的是内置录音（没填 Key 只能念固定台词）".to_string())
            }
            None => (false, "要先填 MiMo Key 才能试听这个声线".to_string()),
        };
        post_result(hwnd, ID_PLAY_RESULT, tag, ok, text);
        return;
    }
    let spawned = std::thread::Builder::new().name("hakimi-settings-tts".into()).spawn(move || {
        let t0 = std::time::Instant::now();
        let (ok, text) = match tts::synth_test(&t, tts::SAMPLE) {
            Ok(wav) => {
                // 窗口已经关了就不再出声
                if HWND_ATOMIC.load(Ordering::Acquire) == hwnd {
                    crate::services::audio::play(TEST_TOKEN, crate::types::Audio(wav), t.volume);
                }
                (true, format!("♪ 正在播放（合成用了 {:.1} 秒）", t0.elapsed().as_secs_f32()))
            }
            Err(e) => (false, format!("✘ {e}")),
        };
        post_result(hwnd, ID_PLAY_RESULT, tag, ok, text);
    });
    if spawned.is_err() {
        post_result(hwnd, ID_PLAY_RESULT, tag, false, "✘ 开不了后台线程".into());
    }
}

fn post_result(hwnd: isize, id: i32, tag: u64, ok: bool, text: String) {
    // 窗口已销毁则丢弃，防止句柄复用后把消息投递给无关窗口
    if HWND_ATOMIC.load(Ordering::Acquire) != hwnd {
        return;
    }
    RESULTS.lock().unwrap_or_else(|e| e.into_inner()).push((id, tag, ok, text));
    unsafe {
        let _ = PostMessageW(Some(HWND(hwnd as *mut _)), WM_RESULT, WPARAM(0), LPARAM(0));
    }
}

fn on_results() {
    let rs: Vec<_> = RESULTS.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect();
    with(|d| {
        for (id, tag, ok, text) in rs {
            if id == ID_UPDATE_RESULT {
                if tag == d.update_gen {
                    set_text(d.c(id), &text);
                    if UPDATE_FOUND.load(Ordering::Relaxed) { set_text(d.c(ID_UPDATE), "去下载新版本"); }
                    enable(d.c(ID_UPDATE), true);
                }
                continue;
            }
            let (want, btn, slot) = if id == ID_TEST_RESULT { (d.test_gen, ID_TEST, 0) } else { (d.play_gen, ID_PLAY, 1) };
            if tag != want {
                continue;
            }
            if id == ID_TEST_RESULT
                && let Some((t, list, chosen)) = FOUND_MODELS.lock().unwrap_or_else(|e| e.into_inner()).take()
                && t == tag
            {
                let h = d.c(ID_MODEL);
                let names: Vec<&str> = list.iter().map(String::as_str).collect();
                combo_fill(h, &names);
                match list.iter().position(|m| *m == chosen) {
                    Some(i) => combo_set(h, i),
                    None => set_text(h, &chosen),
                }
            }
            RESULT_COLOR.with(|c| {
                let mut v = c.get();
                v[slot] = if ok { 1 } else { -1 };
                c.set(v);
            });
            set_text(d.c(id), &text);
            enable(d.c(btn), true);
        }
    });
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_COMMAND => {
                let id = (wp.0 & 0xffff) as i32;
                let code = ((wp.0 >> 16) & 0xffff) as u32;
                // 输入框聚焦/失焦：只重画该输入框的外框区域（避免整窗闪烁）
                if (code == EN_SETFOCUS || code == EN_KILLFOCUS) && is_edit(HWND(lp.0 as *mut _)) {
                    let target = HWND(lp.0 as *mut _);
                    let rect = with(|d| {
                        d.frames.iter().find(|(_, h, _)| *h == target).map(|(r, _, _)| RECT {
                            left: d.px(r[0]) - 2,
                            top: d.px(r[1]) - 2,
                            right: d.px(r[0] + r[2]) + 2,
                            bottom: d.px(r[1] + r[3]) + 2,
                        })
                    })
                    .flatten();
                    match rect {
                        Some(rc) => { let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(hwnd), Some(&rc), true); }
                        None => repaint(hwnd),
                    }
                }
                on_command(hwnd, id, code);
                LRESULT(0)
            }
            WM_HSCROLL => {
                with(|d| d.update_volume_label());
                LRESULT(0)
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLORBTN | WM_CTLCOLOREDIT => ctl_color(HDC(wp.0 as *mut _), HWND(lp.0 as *mut _)),
            WM_CTLCOLORLISTBOX => {
                // 导航栏用栏底色；组合框弹出的下拉列表用白底
                let lb = HWND(lp.0 as *mut _);
                let nav = windows::Win32::UI::WindowsAndMessaging::GetDlgCtrlID(lb) == ID_LIST;
                let hdc = HDC(wp.0 as *mut _);
                SetTextColor(hdc, TEXT);
                SetBkColor(hdc, if nav { SIDE } else { FIELD });
                LRESULT(if nav { SIDE_BRUSH.with(|b| b.get()) } else { FIELD_BRUSH.with(|b| b.get()) })
            }
            WM_ERASEBKGND => {
                paint_bg(hwnd, HDC(wp.0 as *mut _));
                LRESULT(1)
            }
            WM_DRAWITEM => {
                if let Some(di) = (lp.0 as *const windows::Win32::UI::Controls::DRAWITEMSTRUCT).as_ref() {
                    draw_item(di);
                }
                LRESULT(1)
            }
            WM_SETCURSOR => {
                let over = HWND(wp.0 as *mut _);
                let id = windows::Win32::UI::WindowsAndMessaging::GetDlgCtrlID(over);
                if id == ID_KEY_LINK || id == ID_TTS_LINK {
                    if let Ok(c) = LoadCursorW(None, IDC_HAND) {
                        SetCursor(Some(c));
                    }
                    return LRESULT(1);
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_RESULT => {
                on_results();
                LRESULT(0)
            }
            WM_TIMER => {
                if wp.0 == 2 {
                    let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(Some(hwnd), 2);
                }
                with(|d| {
                    if d.page == 4 && (wp.0 == 2 || crate::core::memo::version() != d.memo_ver) {
                        d.memo_refresh();
                    }
                });
                LRESULT(0)
            }
            WM_DPICHANGED => {
                let dpi = (wp.0 & 0xffff) as f32;
                if let Some(r) = (lp.0 as *const RECT).as_ref() {
                    let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                }
                if dpi > 0.0 {
                    with(|d| d.relayout(dpi / 96.0));
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                close(hwnd, true);
                LRESULT(0)
            }
            WM_DESTROY => {
                HWND_ATOMIC.store(0, Ordering::Release);
                let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(Some(hwnd), 1);
                let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(Some(hwnd), 2);
                RESULTS.lock().unwrap_or_else(|e| e.into_inner()).clear();
                if let Some(h) = super::main_hwnd() {
                    let _ = SetFocus(Some(h));
                }
                LRESULT(0)
            }
            WM_NCDESTROY => {
                // 所有子控件都已销毁后再释放字体、图标和画刷，杜绝子控件使用已销毁的 GDI 句柄
                let d = DLG.with(|c| c.try_borrow_mut().ok().and_then(|mut g| g.take()));
                if let Some(d) = d {
                    Dlg::delete_fonts([d.font, d.bold, d.link, d.nav, d.nav_bold]);
                    if let Some(i) = d.icon {
                        let _ = DestroyIcon(i);
                    }
                }
                for b in [BRUSH.with(|b| b.replace(0)), SIDE_BRUSH.with(|b| b.replace(0)), FIELD_BRUSH.with(|b| b.replace(0))] {
                    if b != 0 {
                        let _ = DeleteObject(HGDIOBJ(b as *mut _));
                    }
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memo_when() {
        let now = Dt::new(2026, 11, 20, 21, 0).unwrap(); // 周五
        let n = Repeat::None;
        let at = |y, mo, d, h, mi| Ok(Some(Dt::new(y, mo, d, h, mi).unwrap()));
        assert_eq!(parse_when("", "", n, now), Ok(None));
        assert_eq!(parse_when("", "22:30", n, now), at(2026, 11, 20, 22, 30));
        assert_eq!(parse_when("今天", "8:00", n, now), at(2026, 11, 21, 8, 0));
        assert_eq!(parse_when("明天", "9点半", n, now), at(2026, 11, 21, 9, 30));
        assert_eq!(parse_when("11-25", "18：00", n, now), at(2026, 11, 25, 18, 0));
        assert_eq!(parse_when("1/3", "18点", n, now), at(2027, 1, 3, 18, 0));
        assert_eq!(parse_when("2026年12月1日", "7:05", n, now), at(2026, 12, 1, 7, 5));
        assert!(parse_when("2026-11-20", "20:00", n, now).is_err());
        assert_eq!(parse_when("2026-11-20", "20:00", Repeat::Daily, now), at(2026, 11, 21, 20, 0));
        assert_eq!(parse_when("", "9:00", Repeat::Workday, now), at(2026, 11, 23, 9, 0));
        assert!(parse_when("明天", "", n, now).is_err());
        assert!(parse_when("", "25:00", n, now).is_err());
        assert!(parse_when("2-30", "10:00", n, now).is_err());
        assert_eq!(parse_hm("18"), Some((18, 0)));
    }

    #[test]
    fn presets_sane() {
        assert_eq!(PRESETS[0].provider, "gemini");
        assert_eq!(PRESETS[CUSTOM].key_url, "");
        for p in PRESETS {
            assert!(p.provider == "gemini" || p.provider == "openai");
            if !p.key_url.is_empty() {
                assert!(p.key_url.starts_with("https://"), "{}", p.name);
            }
            if p.provider == "openai" && !p.base.is_empty() {
                assert!(p.base.starts_with("https://") || crate::core::config::is_local_url(p.base), "{}", p.name);
                assert!(!p.base.ends_with('/'));
            }
            if p.key_url.is_empty() {
                assert!(p.models.is_empty() && !p.note.is_empty());
            } else {
                assert!(!p.models.is_empty(), "{}", p.name);
            }
        }
        // 默认配置的模型在 Gemini 预设里
        assert!(PRESETS[0].models.contains(&Config::default().model.as_str()));
        assert!(MIMO_KEY_URL.starts_with("https://platform.xiaomimimo.com/"));
    }

    #[test]
    fn detect() {
        assert_eq!(detect_preset("gemini", ""), 0);
        assert_eq!(detect_preset("whatever", "https://x"), 0);
        assert_eq!(PRESETS[detect_preset("openai", "")].name, "OpenAI");
        assert_eq!(PRESETS[detect_preset("openai", "https://api.deepseek.com/v1/")].name, "DeepSeek（深度求索）");
        assert_eq!(PRESETS[detect_preset("openai", " HTTPS://open.bigmodel.cn/api/paas/v4 ")].name, "智谱 GLM");
        assert_eq!(detect_preset("openai", "https://my.proxy/v1"), CUSTOM);
        // 每个预设存了再读能认回自己
        for (i, p) in PRESETS.iter().enumerate().take(CUSTOM) {
            assert_eq!(detect_preset(p.provider, p.base), i, "{}", p.name);
        }
    }

    #[test]
    fn base_switching() {
        let ds = PRESETS.iter().position(|p| p.base.contains("deepseek")).unwrap();
        assert_eq!(switch_base(0, ds, ""), "https://api.deepseek.com/v1");
        assert_eq!(switch_base(ds, 0, "https://api.deepseek.com/v1"), "");
        assert_eq!(switch_base(ds, CUSTOM, "https://api.deepseek.com/v1"), "");
        assert_eq!(switch_base(CUSTOM, CUSTOM, " https://my/v1 "), "https://my/v1");
        assert_eq!(switch_base(0, CUSTOM, "https://relay.example/v1beta"), "https://relay.example/v1beta");
    }

    #[test]
    fn validation() {
        assert!(validate_ai(CUSTOM, "", "m").is_some());
        assert!(validate_ai(CUSTOM, "api.x.com/v1", "m").is_some());
        assert!(validate_ai(CUSTOM, "https://api.x.com/v1", "m").is_none());
        assert!(validate_ai(0, "", "").is_some());
        assert!(validate_ai(0, "", "gemini-2.5-flash").is_none());
        assert_eq!(parse_minutes("45", 1440), Some(45));
        assert_eq!(parse_minutes(" ", 1440), Some(0));
        assert_eq!(parse_minutes("-1", 1440), None);
        assert_eq!(parse_minutes("99999", 1440), None);
        assert_eq!(parse_clock("18:00").as_deref(), Some("18:00"));
        assert_eq!(parse_clock("8：5").as_deref(), Some("08:05"));
        assert_eq!(parse_clock("").as_deref(), Some(""));
        assert_eq!(parse_clock("0").as_deref(), Some(""));
        assert_eq!(parse_clock("25:00"), None);
        assert_eq!(parse_clock("six"), None);
        assert_eq!(nearest_scale(1.0), 2);
        assert_eq!(nearest_scale(1.45), 4);
        assert_eq!(nearest_scale(0.1), 0);
    }

    #[test]
    fn differs_only_counts_settings_fields() {
        let a = Config::default();
        let mut b = a.clone();
        b.costume = "maid".into(); // 菜单里改的，不算
        b.extra.insert("x".into(), serde_json::Value::Bool(true));
        assert!(!differs(&a, &b));
        b.tts.volume = 0.5;
        assert!(differs(&a, &b));
        let mut c = a.clone();
        assert!(c.fullscreen_hide, "默认全屏自动隐藏");
        c.fullscreen_hide = false;
        assert!(differs(&a, &c));
    }
}
