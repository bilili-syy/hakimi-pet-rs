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
                // 把上一行最后一个字带下来
                let (cur, cw) = lines.last_mut().unwrap();
                if cur.chars().count() > 1 {
                    let last = cur.pop().unwrap();
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
    let (dw, dh) = (dst.width() as i32, dst.height() as i32);
    let (x0, y0, x1, y1) = clip.unwrap_or((0, 0, dw, dh));
    let (x0, y0, x1, y1) = (x0.max(0), y0.max(0), x1.min(dw), y1.min(dh));
    let (r, g, b) = ((rgb >> 16) & 255, (rgb >> 8) & 255, rgb & 255);
    let op = (opacity.clamp(0.0, 1.0) * 255.0).round() as u32;
    let px = dst.data_mut();
    for cy in 0..cov.h as i32 {
        let ty = y + cy;
        if ty < y0 || ty >= y1 { continue; }
        for cx in 0..cov.w as i32 {
            let tx = x + cx;
            if tx < x0 || tx >= x1 { continue; }
            let c = cov.data[(cy as u32 * cov.w + cx as u32) as usize] as u32;
            if c == 0 { continue; }
            let a = c * op / 255; // 0..255
            let i = ((ty * dw + tx) * 4) as usize;
            let inv = 255 - a;
            px[i] = ((r * a + px[i] as u32 * inv) / 255) as u8;
            px[i + 1] = ((g * a + px[i + 1] as u32 * inv) / 255) as u8;
            px[i + 2] = ((b * a + px[i + 2] as u32 * inv) / 255) as u8;
            px[i + 3] = (a + px[i + 3] as u32 * inv / 255) as u8;
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

    pub struct Gdi {
        dc: HDC,
        fonts: HashMap<(u32, bool), HFONT>,
        cells: HashMap<(u32, bool), f32>,
    }

    impl Gdi {
        pub fn new() -> Option<Self> {
            let dc = unsafe { CreateCompatibleDC(None) };
            if dc.is_invalid() { return None; }
            Some(Self { dc, fonts: HashMap::new(), cells: HashMap::new() })
        }

        fn font(&mut self, px: f32, bold: bool) -> HFONT {
            let k = ((px * 4.0).round() as u32, bold);
            if let Some(f) = self.fonts.get(&k) { return *f; }
            let face: Vec<u16> = "Microsoft YaHei".encode_utf16().chain(Some(0)).collect();
            let f = unsafe {
                CreateFontW(
                    -(px.round().max(1.0) as i32), 0, 0, 0,
                    if bold { FW_BOLD.0 as i32 } else { FW_NORMAL.0 as i32 },
                    0, 0, 0, DEFAULT_CHARSET, OUT_TT_PRECIS, CLIP_DEFAULT_PRECIS, ANTIALIASED_QUALITY,
                    0, PCWSTR(face.as_ptr()),
                )
            };
            if self.fonts.len() > 16 {
                for (_, old) in self.fonts.drain() { unsafe { let _ = DeleteObject(old.into()); } }
            }
            self.fonts.insert(k, f);
            f
        }

        pub fn measure(&mut self, s: &str, px: f32, bold: bool) -> f32 {
            let f = self.font(px, bold);
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
            let mut tm = TEXTMETRICW::default();
            unsafe {
                let old = SelectObject(self.dc, f.into());
                let _ = GetTextMetricsW(self.dc, &mut tm);
                SelectObject(self.dc, old);
            }
            let h = if tm.tmHeight > 0 { tm.tmHeight as f32 } else { px * 1.32 };
            self.cells.insert(k, h);
            h
        }

        pub fn raster(&mut self, s: &str, px: f32, bold: bool) -> Option<Coverage> {
            if s.is_empty() { return None; }
            let f = self.font(px, bold);
            let wide: Vec<u16> = s.encode_utf16().collect();
            let w = (self.measure(s, px, bold).ceil() as i32 + (px * 0.3) as i32 + 2).max(1);
            let h = (self.cell_height(px, bold).ceil() as i32 + 2).max(1);
            let mut bmi = BITMAPINFO::default();
            bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = w;
            bmi.bmiHeader.biHeight = -h;
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = BI_RGB.0;
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            unsafe {
                let bmp = CreateDIBSection(Some(self.dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
                if bits.is_null() { let _ = DeleteObject(bmp.into()); return None; }
                let old_bmp = SelectObject(self.dc, bmp.into());
                let old_font = SelectObject(self.dc, f.into());
                SetBkMode(self.dc, TRANSPARENT);
                SetTextColor(self.dc, windows::Win32::Foundation::COLORREF(0x00FF_FFFF));
                let _ = TextOutW(self.dc, 0, 0, &wide);
                let _ = GdiFlush();
                let n = (w * h) as usize;
                let src = std::slice::from_raw_parts(bits as *const u8, n * 4);
                let mut data = vec![0u8; n];
                for (i, d) in data.iter_mut().enumerate() {
                    let (b, g, r) = (src[i * 4] as u16, src[i * 4 + 1] as u16, src[i * 4 + 2] as u16);
                    *d = ((r + g + b) / 3) as u8;
                }
                SelectObject(self.dc, old_font);
                SelectObject(self.dc, old_bmp);
                let _ = DeleteObject(bmp.into());
                Some(Coverage { w: w as u32, h: h as u32, data })
            }
        }
    }

    impl Drop for Gdi {
        fn drop(&mut self) {
            unsafe {
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
