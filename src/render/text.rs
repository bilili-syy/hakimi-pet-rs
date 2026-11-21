//! 文字：断行（纯逻辑）+ GDI 光栅化。
//! 不用 fontdue：它加载 msyh.ttc 时会把 3 万个字形全部解析进内存（实测常驻 ~176MB），
//! 远超 40MB 目标。GDI 用系统共享的字体映射，几乎不占进程内存，还自带字体链接（emoji/符号回退）。
use std::collections::HashMap;

/// 是否是「可以在任意字符处断开」的字符（CJK、全角符号等）
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x2E80..=0x9FFF | 0xA960..=0xA97F | 0xAC00..=0xD7FF | 0xF900..=0xFAFF |
        0xFE30..=0xFE4F | 0xFF00..=0xFFEF | 0x20000..=0x3FFFF | 0x1F000..=0x1FAFF |
        0x2600..=0x27BF | 0x2190..=0x21FF | 0x2000..=0x206F)
}

/// 不能放在行首的标点（禁则）
fn no_line_start(c: char) -> bool {
    "，。！？、；：）》」』】〕…～,.!?;:)]}%·—~".contains(c)
}

/// 按 CSS 的习惯断行：CJK 字符之间可断；连续的西文/数字算一个词，词内不断（除非一个词就超宽）；
/// 闭标点不出现在行首（把前一个字一起带到下一行）。width(c) 返回字符宽度。
pub fn wrap(text: &str, max_w: f32, mut width: impl FnMut(char) -> f32) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        // 切成不可分的 token
        let mut tokens: Vec<String> = Vec::new();
        let mut word = String::new();
        for c in para.chars() {
            if c == '\r' { continue; }
            if is_cjk(c) || c == ' ' {
                if !word.is_empty() { tokens.push(std::mem::take(&mut word)); }
                tokens.push(c.to_string());
            } else {
                word.push(c);
            }
        }
        if !word.is_empty() { tokens.push(word); }

        let mut lines: Vec<(String, f32)> = vec![(String::new(), 0.0)];
        for tok in tokens {
            let tw: f32 = tok.chars().map(&mut width).sum();
            let (cur, cw) = lines.last_mut().unwrap();
            if *cw + tw <= max_w + 0.01 || cur.is_empty() && tw <= max_w {
                cur.push_str(&tok);
                *cw += tw;
                continue;
            }
            if tok == " " { continue; } // 行尾空格吞掉
            if tw > max_w {
                // 超长单词：逐字硬断
                for c in tok.chars() {
                    let w = width(c);
                    let (cur, cw) = lines.last_mut().unwrap();
                    if *cw + w > max_w + 0.01 && !cur.is_empty() {
                        lines.push((c.to_string(), w));
                    } else {
                        cur.push(c);
                        *cw += w;
                    }
                }
                continue;
            }
            let first = tok.chars().next().unwrap();
            if no_line_start(first) {
                // 把上一行最后一个 CJK 字带下来（不拆西文单词、不把空格带到行首）
                let (cur, cw) = lines.last_mut().unwrap();
                if cur.chars().count() > 1
                    && let Some(last) = cur.chars().next_back()
                    && is_cjk(last)
                {
                    cur.pop();
                    *cw -= width(last);
                    let nw = width(last) + tw;
                    lines.push((format!("{last}{tok}"), nw));
                    continue;
                }
                cur.push_str(&tok);
                *cw += tw;
                continue;
            }
            lines.push((tok, tw));
        }
        out.extend(lines.into_iter().map(|(s, _)| s.trim_end().to_string()));
    }
    out
}

/// 灰度覆盖率位图（0..255）
pub struct Coverage {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

/// 把覆盖率按颜色（预乘）用 src-over 叠到 RGBA 预乘像素上
/// clip = (x0, y0, x1, y1) 设备像素，不含右下边界
pub fn blit(dst: &mut tiny_skia::Pixmap, cov: &Coverage, x: i32, y: i32, rgb: u32, opacity: f32, clip: Option<(i32, i32, i32, i32)>) {
    let op = (opacity.clamp(0.0, 1.0) * 255.0).round() as u32;
    if op == 0 { return; }
    let (dw, dh) = (dst.width() as i32, dst.height() as i32);
    let (x0, y0, x1, y1) = clip.unwrap_or((0, 0, dw, dh));
    let (x0, y0, x1, y1) = (x0.max(0), y0.max(0), x1.min(dw), y1.min(dh));
    let cy0 = (y0 - y).max(0);
    let cy1 = (y1 - y).min(cov.h as i32);
    let cx0 = (x0 - x).max(0);
    let cx1 = (x1 - x).min(cov.w as i32);
    if cy0 >= cy1 || cx0 >= cx1 { return; }
    let (r, g, b) = ((rgb >> 16) & 255, (rgb >> 8) & 255, rgb & 255);
    let px = dst.data_mut();
    for cy in cy0..cy1 {
        let ty = y + cy;
        let row_src = cy as usize * cov.w as usize;
        let row_dst = ty as usize * dw as usize;
        for cx in cx0..cx1 {
            let c = cov.data[row_src + cx as usize] as u32;
            if c == 0 { continue; }
            let a = (c * op + 127) / 255; // 0..255
            if a == 0 { continue; }
            let i = (row_dst + (x + cx) as usize) * 4;
            let inv = 255 - a;
            px[i] = ((r * a + px[i] as u32 * inv + 127) / 255) as u8;
            px[i + 1] = ((g * a + px[i + 1] as u32 * inv + 127) / 255) as u8;
            px[i + 2] = ((b * a + px[i + 2] as u32 * inv + 127) / 255) as u8;
            px[i + 3] = (a + (px[i + 3] as u32 * inv + 127) / 255).min(255) as u8;
        }
    }
}

/// GDI 文字引擎。字号单位：设备像素
pub struct TextEngine {
    gdi: Option<gdi::Gdi>,
    widths: HashMap<(char, u32, bool), f32>,
}

impl Default for TextEngine {
    fn default() -> Self { Self::new() }
}

impl TextEngine {
    pub fn new() -> Self { Self { gdi: gdi::Gdi::new(), widths: HashMap::new() } }

    fn key(px: f32) -> u32 { (px * 4.0).round() as u32 }

    pub fn char_width(&mut self, c: char, px: f32, bold: bool) -> f32 {
        let k = (c, Self::key(px), bold);
        if let Some(w) = self.widths.get(&k) { return *w; }
        let w = match &mut self.gdi {
            Some(g) => g.measure(&c.to_string(), px, bold),
            None => if is_cjk(c) { px } else { px * 0.55 },
        };
        if self.widths.len() > 4000 { self.widths.clear(); }
        self.widths.insert(k, w);
        w
    }

    pub fn text_width(&mut self, s: &str, px: f32, bold: bool) -> f32 {
        if let Some(g) = &mut self.gdi {
            return g.measure(s, px, bold);
        }
        s.chars().map(|c| self.char_width(c, px, bold)).sum()
    }

    pub fn wrap(&mut self, text: &str, max_w: f32, px: f32, bold: bool) -> Vec<String> {
        wrap(text, max_w, |c| self.char_width(c, px, bold))
    }

    /// 字体单元格高度（ascent+descent，设备像素）
    pub fn cell_height(&mut self, px: f32, bold: bool) -> f32 {
        match &mut self.gdi {
            Some(g) => g.cell_height(px, bold),
            None => px * 1.32,
        }
    }

    /// 光栅化一行字，左上角是字体单元格左上角
    pub fn raster(&mut self, s: &str, px: f32, bold: bool) -> Option<Coverage> {
        self.gdi.as_mut()?.raster(s, px, bold)
    }
}

#[cfg(windows)]
mod gdi {
    use super::Coverage;
    use std::collections::HashMap;
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::*;
    use windows::core::PCWSTR;

    /// 覆盖率加深表：255·(c/255)^0.7
    static GAMMA: std::sync::LazyLock<[u8; 256]> = std::sync::LazyLock::new(|| {
        let mut t = [0u8; 256];
        for (i, v) in t.iter_mut().enumerate() { *v = (255.0 * (i as f32 / 255.0).powf(0.7)).round() as u8; }
        t
    });

    /// 字体选择：(正文字体, 正文字重, 粗体字体, 粗体字重, 质量)。
    /// 默认优先思源黑体 Noto Sans SC（Medium 正文 / Bold 标题），笔画比微软雅黑小字号下实很多；
    /// 没装就退回微软雅黑。用 ClearType 栅格化再把 RGB 取平均，相当于横向 3 倍超采样，边缘更顺。
    /// HAKIMI_FONT / HAKIMI_FONT_W / HAKIMI_FONT_Q=aa 可临时覆盖（调试选字体用）。
    struct FontPick { body: String, body_w: i32, bold: String, bold_w: i32, q: FONT_QUALITY }
    static FONT: std::sync::LazyLock<FontPick> = std::sync::LazyLock::new(|| {
        let q = if std::env::var("HAKIMI_FONT_Q").is_ok_and(|v| v == "aa") { ANTIALIASED_QUALITY } else { CLEARTYPE_QUALITY };
        if let Ok(name) = std::env::var("HAKIMI_FONT") {
            let w = std::env::var("HAKIMI_FONT_W").ok().and_then(|v| v.parse().ok()).unwrap_or(FW_NORMAL.0 as i32);
            return FontPick { bold: name.clone(), body: name, body_w: w, bold_w: (w + 300).min(900), q };
        }
        if installed("Noto Sans SC Medium") && installed("Noto Sans SC") {
            FontPick { body: "Noto Sans SC Medium".into(), body_w: 500, bold: "Noto Sans SC".into(), bold_w: FW_BOLD.0 as i32, q }
        } else {
            FontPick { body: "Microsoft YaHei".into(), body_w: FW_NORMAL.0 as i32, bold: "Microsoft YaHei".into(), bold_w: FW_BOLD.0 as i32, q }
        }
    });

    /// 系统里有没有这个字体（建一个看 GDI 实际给的是不是它，没装会被悄悄换成别的）
    fn installed(name: &str) -> bool {
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() { return false; }
            let face: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            let f = CreateFontW(-16, 0, 0, 0, 0, 0, 0, 0, DEFAULT_CHARSET, OUT_TT_PRECIS, CLIP_DEFAULT_PRECIS, DEFAULT_QUALITY, 0, PCWSTR(face.as_ptr()));
            let old = SelectObject(dc, f.into());
            let mut buf = [0u16; 64];
            let n = GetTextFaceW(dc, Some(&mut buf)).max(0) as usize;
            SelectObject(dc, old);
            let _ = DeleteObject(f.into());
            let _ = DeleteDC(dc);
            let got = String::from_utf16_lossy(&buf[..n.saturating_sub(1).min(64)]);
            got.trim_end_matches('\0').eq_ignore_ascii_case(name)
        }
    }

    pub struct Gdi {
        dc: HDC,
        fonts: HashMap<(u32, bool), HFONT>,
        cells: HashMap<(u32, bool), f32>,
        /// 复用的临时 DIB，避免每次 raster() 都向内核申请/销毁一份 DIB Section
        dib_bmp: HBITMAP,
        dib_bits: *mut u8,
        dib_w: i32,
        dib_h: i32,
    }

    impl Gdi {
        pub fn new() -> Option<Self> {
            let dc = unsafe { CreateCompatibleDC(None) };
            if dc.is_invalid() { return None; }
            Some(Self {
                dc,
                fonts: HashMap::new(),
                cells: HashMap::new(),
                dib_bmp: HBITMAP::default(),
                dib_bits: std::ptr::null_mut(),
                dib_w: 0,
                dib_h: 0,
            })
        }

        fn ensure_dib(&mut self, w: i32, h: i32) -> bool {
            if !self.dib_bmp.is_invalid() && !self.dib_bits.is_null() && self.dib_w >= w && self.dib_h >= h {
                return true;
            }
            let nw = w.max(self.dib_w).min(4096);
            let nh = h.max(self.dib_h).min(1024);
            let mut bmi = BITMAPINFO::default();
            bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = nw;
            bmi.bmiHeader.biHeight = -nh;
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = BI_RGB.0;
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let Ok(bmp) = (unsafe { CreateDIBSection(Some(self.dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) }) else {
                return false;
            };
            if bits.is_null() {
                unsafe { let _ = DeleteObject(bmp.into()); }
                return false;
            }
            if !self.dib_bmp.is_invalid() {
                unsafe { let _ = DeleteObject(self.dib_bmp.into()); }
            }
            self.dib_bmp = bmp;
            self.dib_bits = bits as *mut u8;
            self.dib_w = nw;
            self.dib_h = nh;
            true
        }

        fn font(&mut self, px: f32, bold: bool) -> HFONT {
            let k = ((px * 4.0).round() as u32, bold);
            if let Some(f) = self.fonts.get(&k) { return *f; }
            let fp = &*FONT;
            let (fname, fw) = if bold { (&fp.bold, fp.bold_w) } else { (&fp.body, fp.body_w) };
            let face: Vec<u16> = fname.encode_utf16().chain(Some(0)).collect();
            let f = unsafe {
                CreateFontW(
                    -(px.round().max(1.0) as i32), 0, 0, 0,
                    fw,
                    0, 0, 0, DEFAULT_CHARSET, OUT_TT_PRECIS, CLIP_DEFAULT_PRECIS, fp.q,
                    0, PCWSTR(face.as_ptr()),
                )
            };
            if f.is_invalid() { return f; }
            if self.fonts.len() > 16 {
                for (_, old) in self.fonts.drain() { unsafe { let _ = DeleteObject(old.into()); } }
                self.cells.clear();
            }
            self.fonts.insert(k, f);
            f
        }

        pub fn measure(&mut self, s: &str, px: f32, bold: bool) -> f32 {
            let f = self.font(px, bold);
            if f.is_invalid() { return px * s.chars().count() as f32 * 0.6; }
            let w: Vec<u16> = s.encode_utf16().collect();
            let mut sz = SIZE::default();
            unsafe {
                let old = SelectObject(self.dc, f.into());
                let _ = GetTextExtentPoint32W(self.dc, &w, &mut sz);
                SelectObject(self.dc, old);
            }
            sz.cx as f32
        }

        pub fn cell_height(&mut self, px: f32, bold: bool) -> f32 {
            let k = ((px * 4.0).round() as u32, bold);
            if let Some(h) = self.cells.get(&k) { return *h; }
            let f = self.font(px, bold);
            if f.is_invalid() { return px * 1.32; }
            let mut tm = TEXTMETRICW::default();
            unsafe {
                let old = SelectObject(self.dc, f.into());
                let _ = GetTextMetricsW(self.dc, &mut tm);
                SelectObject(self.dc, old);
            }
            let h = if tm.tmHeight > 0 { tm.tmHeight as f32 } else { px * 1.32 };
            if self.cells.len() > 32 { self.cells.clear(); }
            self.cells.insert(k, h);
            h
        }

        pub fn raster(&mut self, s: &str, px: f32, bold: bool) -> Option<Coverage> {
            if s.is_empty() { return None; }
            let wide: Vec<u16> = s.encode_utf16().collect();
            let w = (self.measure(s, px, bold).ceil() as i32 + (px * 0.3) as i32 + 2).clamp(1, 4096);
            let h = (self.cell_height(px, bold).ceil() as i32 + 2).clamp(1, 1024);
            let n = (w as usize).checked_mul(h as usize)?;
            if !self.ensure_dib(w, h) { return None; }
            // 在 measure / cell_height 之后再取 HFONT，避免字体缓存清空导致悬空句柄
            let f = self.font(px, bold);
            if f.is_invalid() { return None; }
            let stride = (self.dib_w as usize).checked_mul(4)?;
            let used_bytes = (h as usize).checked_mul(stride)?;
            unsafe {
                std::ptr::write_bytes(self.dib_bits, 0, used_bytes);
                let old_bmp = SelectObject(self.dc, self.dib_bmp.into());
                let old_font = SelectObject(self.dc, f.into());
                SetBkMode(self.dc, TRANSPARENT);
                SetTextColor(self.dc, windows::Win32::Foundation::COLORREF(0x00FF_FFFF));
                let _ = TextOutW(self.dc, 0, 0, &wide);
                let _ = GdiFlush();
                let src = std::slice::from_raw_parts(self.dib_bits as *const u8, used_bytes);
                let mut data = vec![0u8; n];
                let wu = w as usize;
                for row in 0..h as usize {
                    let srow = &src[row * stride..row * stride + wu * 4];
                    let drow = &mut data[row * wu..(row + 1) * wu];
                    for (x, d) in drow.iter_mut().enumerate() {
                        let (b, g, r) = (srow[x * 4] as u16, srow[x * 4 + 1] as u16, srow[x * 4 + 2] as u16);
                        // 灰度抗锯齿的笔画边缘偏淡，细字在浅底上发虚：覆盖率做一次 0.7 次幂加粗边缘
                        *d = GAMMA[((r + g + b) / 3) as usize];
                    }
                }
                SelectObject(self.dc, old_font);
                SelectObject(self.dc, old_bmp);
                Some(Coverage { w: w as u32, h: h as u32, data })
            }
        }
    }

    impl Drop for Gdi {
        fn drop(&mut self) {
            unsafe {
                if !self.dib_bmp.is_invalid() { let _ = DeleteObject(self.dib_bmp.into()); }
                for (_, f) in self.fonts.drain() { let _ = DeleteObject(f.into()); }
                let _ = DeleteDC(self.dc);
            }
        }
    }
}

#[cfg(not(windows))]
mod gdi {
    use super::Coverage;
    pub struct Gdi;
    impl Gdi {
        pub fn new() -> Option<Self> { None }
        pub fn measure(&mut self, _s: &str, px: f32, _b: bool) -> f32 { px }
        pub fn cell_height(&mut self, px: f32, _b: bool) -> f32 { px * 1.32 }
        pub fn raster(&mut self, _s: &str, _px: f32, _b: bool) -> Option<Coverage> { None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(c: char) -> f32 { if is_cjk(c) { 10.0 } else { 5.0 } }

    #[test]
    fn wraps_cjk_anywhere() {
        let l = wrap("哈基米哈基米哈基米", 30.0, w);
        assert_eq!(l, vec!["哈基米", "哈基米", "哈基米"]);
    }

    #[test]
    fn keeps_latin_words() {
        let l = wrap("abc defg hi", 25.0, w);
        assert_eq!(l, vec!["abc", "defg", "hi"]);
    }

    #[test]
    fn hard_breaks_long_words() {
        let l = wrap("abcdefghij", 20.0, w);
        assert_eq!(l, vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn punctuation_not_at_line_start() {
        // 第三个字后面跟逗号：逗号不能开头，带上「米」一起下去
        let l = wrap("哈基米，好", 30.0, w);
        assert_eq!(l, vec!["哈基", "米，好"]);
    }

    #[test]
    fn newlines_and_mixed() {
        let l = wrap("你走了 5 分钟\n想我", 200.0, w);
        assert_eq!(l, vec!["你走了 5 分钟", "想我"]);
        assert_eq!(wrap("", 100.0, w), vec![""]);
    }

    #[test]
    fn gdi_measures_text() {
        let mut t = TextEngine::new();
        let a = t.char_width('哈', 13.0, false);
        let b = t.char_width('i', 13.0, false);
        assert!(a > 8.0 && a < 20.0, "cjk width {a}");
        assert!(b < a);
        let cov = t.raster("哈基米", 13.0, false);
        if let Some(c) = cov { assert!(c.data.iter().any(|&v| v > 128)); }
    }
}
