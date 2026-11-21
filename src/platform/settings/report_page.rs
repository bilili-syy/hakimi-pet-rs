//! 设置窗口「周报」页：这周的数字预览、勾选要放进图里的记忆、右侧实时缩略图预览、生成报告图（存到图片\哈基米 + 复制到剪贴板）
use super::*;

/// 最多列几条记忆给主人勾
pub(super) const REPORT_MEMS: usize = 6;
/// 右侧周报实时缩略图预览区（逻辑像素 [x, y, w, h]）
pub(super) const REPORT_PREVIEW_RECT: [f32; 4] = [368.0, 50.0, 154.0, 318.0];

impl Dlg {
    pub(super) fn report_build(&mut self) {
        let p = 5;
        let lw = 220.0;
        self.title(p, "每周陪伴报告");
        self.add(p, w!("STATIC"), "", 0, 0, ID_REPORT_INFO, [X0, 50.0, lw, 74.0]);
        self.hint(p, "勾选要画进右侧卡片里的记忆：", [X0, 130.0, lw, 22.0]);
        for i in 0..REPORT_MEMS {
            self.button(p, ID_REPORT_MEM0 + i as i32, "", BS_AUTOCHECKBOX, [X0, 156.0 + i as f32 * 28.0, lw, 24.0]);
        }
        self.button(p, ID_REPORT_MAKE, "生成报告图", 0, [X0, 336.0, 112.0, 32.0]);
        self.button(p, ID_REPORT_OPEN, "打开文件夹", 0, [X0 + 118.0, 336.0, 102.0, 32.0]);
        self.add(p, w!("STATIC"), "", 0, 0, ID_REPORT_MSG, [X0, 380.0, W - X0 - 18.0, 68.0]);
    }

    /// 切到这页时：刷新数字、记忆列表和右侧缩略图
    pub(super) fn report_refresh(&mut self) {
        let r = super::super::report::current(Vec::new());
        let face = r.top_face.as_ref().map(|f| format!("{}（{} 次）", f.0, f.1)).unwrap_or_else(|| "还没有".into());
        let info = format!(
            "{}\n陪你 {} · 喂 {} 次 · 聊 {} 句\n最多表情：{}",
            r.range,
            crate::render::report::duration_text(r.total_min),
            r.feeds,
            r.chats,
            face
        );
        set_text(self.c(ID_REPORT_INFO), &info);
        let book = crate::core::memory::snapshot();
        self.report_mems = book.ranked().iter().take(REPORT_MEMS).map(|m| m.text.clone()).collect();
        for i in 0..REPORT_MEMS {
            let h = self.c(ID_REPORT_MEM0 + i as i32);
            match self.report_mems.get(i) {
                Some(t) => {
                    let short: String = if t.chars().count() > 16 { t.chars().take(15).collect::<String>() + "…" } else { t.clone() };
                    set_text(h, &short);
                    set_checked(h, false);
                    unsafe { let _ = ShowWindow(h, SW_SHOW); }
                }
                None => unsafe {
                    let _ = ShowWindow(h, SW_HIDE);
                },
            }
        }
        if self.report_mems.is_empty() {
            set_text(self.c(ID_REPORT_MSG), "她还没记住什么事，报告里就只放这周的数字。");
        } else {
            set_text(self.c(ID_REPORT_MSG), "");
        }
        self.report_update_preview();
    }

    /// 按当前勾选的记忆重算右侧预览缩略图（自上而下的 BGRA）
    pub(super) fn report_update_preview(&mut self) {
        let picked = self.report_picked();
        let max_w = ((REPORT_PREVIEW_RECT[2] - 12.0) * self.scale).round().max(32.0) as u32;
        let max_h = ((REPORT_PREVIEW_RECT[3] - 12.0) * self.scale).round().max(32.0) as u32;
        self.report_preview = super::super::report::render_pixmap(picked).and_then(|(_, pm)| {
            let k = (max_w as f32 / pm.width() as f32).min(max_h as f32 / pm.height() as f32).min(1.0);
            let nw = ((pm.width() as f32 * k).round() as u32).max(1);
            let nh = ((pm.height() as f32 * k).round() as u32).max(1);
            let dst = crate::render::sprites::scale_pixmap(pm, nw, nh)?;
            let bgra: Vec<u8> = dst.data().as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], 255]).collect();
            Some((bgra, nw as i32, nh as i32))
        });
        repaint(self.hwnd);
    }

    /// 勾了的记忆
    pub(super) fn report_picked(&self) -> Vec<String> {
        self.report_mems.iter().enumerate().filter(|(i, _)| checked(self.c(ID_REPORT_MEM0 + *i as i32))).map(|(_, t)| t.clone()).collect()
    }
}

pub(super) fn report_make() {
    let Some(picked) = with(|d| d.report_picked()) else { return };
    let msg = match super::super::report::make(picked) {
        Ok((p, true)) => format!("做好啦！已经复制到剪贴板，可以直接粘贴到聊天里。\n存在：{}", p.display()),
        Ok((p, false)) => format!("图已存好（剪贴板正忙没复制上，可直接发文件）：\n{}", p.display()),
        Err(e) => format!("没做成：{e}"),
    };
    with(|d| set_text(d.c(ID_REPORT_MSG), &msg));
}
