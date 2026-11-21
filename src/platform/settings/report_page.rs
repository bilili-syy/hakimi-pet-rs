//! 设置窗口「周报」页：这周的数字预览、勾选要放进图里的记忆、生成报告图（存到图片\哈基米 + 复制到剪贴板）
use super::*;

/// 最多列几条记忆给主人勾
pub(super) const REPORT_MEMS: usize = 6;

impl Dlg {
    pub(super) fn report_build(&mut self) {
        let p = 5;
        self.title(p, "每周陪伴报告");
        self.add(p, w!("STATIC"), "", 0, 0, ID_REPORT_INFO, [X0, 50.0, W - X0 - 18.0, 74.0]);
        self.hint(p, "勾上的「她记住的事」才会画进图里，默认都不放：", [X0, 130.0, W - X0 - 18.0, 22.0]);
        for i in 0..REPORT_MEMS {
            self.button(p, ID_REPORT_MEM0 + i as i32, "", BS_AUTOCHECKBOX, [X0, 158.0 + i as f32 * 28.0, W - X0 - 18.0, 24.0]);
        }
        self.button(p, ID_REPORT_MAKE, "生成报告图", 0, [X0, 340.0, 120.0, 32.0]);
        self.button(p, ID_REPORT_OPEN, "打开文件夹", 0, [X0 + 130.0, 340.0, 110.0, 32.0]);
        self.add(p, w!("STATIC"), "", 0, 0, ID_REPORT_MSG, [X0, 382.0, W - X0 - 18.0, 66.0]);
    }

    /// 切到这页时：刷新数字和记忆列表
    pub(super) fn report_refresh(&mut self) {
        let r = super::super::report::current(Vec::new());
        let face = r.top_face.as_ref().map(|f| format!("{}（{} 次）", f.0, f.1)).unwrap_or_else(|| "还没有".into());
        let info = format!(
            "{}\n陪你 {} · 吃了 {} 条小鱼干 · 聊了 {} 句\n最常出现的表情：{}",
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
                    let short: String = if t.chars().count() > 34 { t.chars().take(33).collect::<String>() + "…" } else { t.clone() };
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
        }
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
