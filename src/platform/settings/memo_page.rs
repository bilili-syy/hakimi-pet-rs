//! 设置窗口「备忘录」页：列表、表单、增删改（直接写 memo.json）
use super::*;

impl Dlg {
    pub(super) fn memo_refresh(&mut self) {
        use crate::core::memo as store;
        self.memo_ver = store::version();
        let today = store::Now::get().dt;
        let ringing = store::ringing();
        let items = store::pending();
        let list = self.c(ID_MEMO_LIST);
        send(list, LB_RESETCONTENT, 0, 0);
        self.memo_ids.clear();
        for it in &items {
            let bell = if ringing.contains(&it.id) { "⏰ " } else { "" };
            let w = wide(&format!("{bell}{}　　{}", it.when_label(today), it.text));
            send(list, LB_ADDSTRING, 0, w.as_ptr() as isize);
            self.memo_ids.push(it.id);
        }
        if items.is_empty() {
            let w = wide("（还没有待办：在下面写一条，点「添加这条」）");
            send(list, LB_ADDSTRING, 0, w.as_ptr() as isize);
        }
        match self.memo_sel.and_then(|id| self.memo_ids.iter().position(|x| *x == id)) {
            Some(i) => {
                send(list, LB_SETCURSEL, i, 0);
            }
            None => {
                self.memo_sel = None;
                send(list, LB_SETCURSEL, usize::MAX, 0);
            }
        }
        self.memo_buttons();
    }

    pub(super) fn memo_buttons(&self) {
        let sel = self.memo_sel.is_some();
        enable(self.c(ID_MEMO_DONE), sel);
        enable(self.c(ID_MEMO_DEL), sel);
        set_text(self.c(ID_MEMO_SAVE), if sel { "保存修改" } else { "添加这条" });
        repaint(self.c(ID_MEMO_SAVE));
    }

    pub(super) fn memo_msg(&self, s: &str) {
        set_text(self.c(ID_MEMO_MSG), s);
    }

    /// 点列表里一条：填进下面的表单
    pub(super) fn memo_pick(&mut self) {
        let i = send(self.c(ID_MEMO_LIST), LB_GETCURSEL, 0, 0);
        let Some(&id) = usize::try_from(i).ok().and_then(|i| self.memo_ids.get(i)) else { return self.memo_clear_form() };
        let Some(it) = crate::core::memo::get(id) else { return };
        self.memo_sel = Some(id);
        set_text(self.c(ID_MEMO_TEXT), &it.text);
        let (d, t) = match it.time_dt() {
            Some(t) => (format!("{:04}-{:02}-{:02}", t.y, t.mo, t.d), format!("{:02}:{:02}", t.h, t.mi)),
            None => (String::new(), String::new()),
        };
        set_text(self.c(ID_MEMO_DATE), &d);
        set_text(self.c(ID_MEMO_TIME), &t);
        combo_set(self.c(ID_MEMO_REPEAT), REPEATS.iter().position(|r| r.0 == it.repeat).unwrap_or(0));
        self.memo_msg("");
        self.memo_buttons();
    }

    pub(super) fn memo_clear_form(&mut self) {
        self.memo_sel = None;
        send(self.c(ID_MEMO_LIST), LB_SETCURSEL, usize::MAX, 0);
        for id in [ID_MEMO_TEXT, ID_MEMO_DATE, ID_MEMO_TIME] {
            set_text(self.c(id), "");
        }
        combo_set(self.c(ID_MEMO_REPEAT), 0);
        self.memo_buttons();
    }

    /// 「添加这条」/「保存修改」
    pub(super) fn memo_save(&mut self) -> Result<(), String> {
        use crate::core::memo as store;
        let text = get_text(self.c(ID_MEMO_TEXT)).trim().to_string();
        if text.is_empty() {
            return Err("先写上要提醒什么喵".into());
        }
        let repeat = REPEATS[combo_sel(self.c(ID_MEMO_REPEAT)).min(REPEATS.len() - 1)].0;
        let now = store::Now::get().dt;
        let when = parse_when(&get_text(self.c(ID_MEMO_DATE)), &get_text(self.c(ID_MEMO_TIME)), repeat, now)?;
        let (id, verb) = match self.memo_sel {
            Some(id) if store::update(id, &text, when, repeat) => (id, "改好了"),
            _ => (store::add(&text, when, repeat).id, "记下啦"),
        };
        let label = store::get(id).map(|i| i.when_label(now)).unwrap_or_default();
        if verb == "记下啦" {
            self.memo_clear_form();
        }
        self.memo_refresh();
        self.memo_msg(&if when.is_some() { format!("✔ {verb}，{label} 叫你") } else { format!("✔ {verb}（随手记，不提醒）") });
        Ok(())
    }

}

pub(super) fn memo_save_ui(hwnd: HWND) {
    if let Some(Err(e)) = with(|d| d.memo_save()) {
        msgbox(hwnd, &e, MB_OK | MB_ICONWARNING);
    }
}

