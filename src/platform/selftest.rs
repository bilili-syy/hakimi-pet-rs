//! 端到端自测（会真的开窗口和托盘，默认忽略）：cargo test selftest -- --ignored --nocapture
//! 用一个脚本化的假 Handler 驱动：溜达 → 拖拽 → 下落 → 聊天框 → 菜单（番茄钟/显隐/叫到中间）→ 退出
use super::*;
use crate::types::{AppEvent, Frame, Handler, WinCmd};
use std::sync::{Arc, Mutex};

struct Fake {
    log: Arc<Mutex<Vec<String>>>,
    frame: Frame,
    step: u32,
    t0: u64,
    wait_until: u64,
}

impl Fake {
    fn rec(&self, s: String) {
        eprintln!("[selftest] {s}");
        self.log.lock().unwrap().push(s);
    }
}

fn inject(id: &str) {
    MENU_QUEUE.lock().unwrap().push_back(id.into());
    if let Some(h) = main_hwnd() {
        unsafe {
            let _ = PostMessageW(Some(h), WM_APP_MENU, WPARAM(0), LPARAM(0));
        }
    }
}

impl Handler for Fake {
    fn event(&mut self, ev: AppEvent, out: &mut Vec<WinCmd>) {
        let s = format!("{ev:?}");
        if !s.starts_with("Mouse") {
            self.rec(s);
        }
        if let AppEvent::Menu(MenuAction::Quit) = ev {
            out.push(WinCmd::Quit);
        }
    }
    fn frame(&mut self, now: u64, out: &mut Vec<WinCmd>) -> u32 {
        if self.t0 == 0 {
            self.t0 = now;
        }
        if now < self.wait_until {
            return 16;
        }
        let (x, y) = with(|s| (s.x, s.y)).unwrap();
        self.step += 1;
        self.wait_until = now + 700;
        match self.step {
            1 => out.push(WinCmd::Walk { dir: -1, distance: 120.0, speed: 6.0 }),
            2 => {
                self.rec(format!("after walk x={x}"));
                with(|s| s.drag_off = (10, 10));
                out.push(WinCmd::DragMove { sx: x + 10, sy: y - 400 + 10 });
            }
            3 => out.push(WinCmd::DragEnd),
            4 => {
                self.rec(format!("after fall y={y}"));
                out.push(WinCmd::ChatInput(ChatInputCmd::Open));
            }
            5 => {
                let vis = with(|s| s.chat.as_ref().unwrap().visible).unwrap();
                self.rec(format!("chat visible={vis}"));
                out.push(WinCmd::ChatInput(ChatInputCmd::Enable(false)));
                out.push(WinCmd::ChatInput(ChatInputCmd::Close));
            }
            6 => inject("pomo:25"),
            7 => {
                self.rec(format!("pomodoro_left={:?}", pomodoro_left()));
                inject("pomo:stop");
            }
            8 => inject("visible"),
            9 => {
                self.rec(format!("visible={}", with(|s| s.is_visible()).unwrap()));
                inject("visible");
            }
            10 => inject("center"),
            11 => {
                self.rec(format!("moyu_today={:?}", moyu_today()));
                inject("quit");
            }
            _ => {}
        }
        16
    }
    fn render(&mut self, scale: f32) -> Option<&Frame> {
        let w = (WIN_W as f32 * scale).round() as u32;
        let h = (WIN_H as f32 * scale).round() as u32;
        if self.frame.w != w || self.frame.h != h {
            // 下半部分半透明粉色方块（预乘），上半透明（点穿）
            let mut bgra = vec![0u8; (w * h * 4) as usize];
            for yy in h / 2..h {
                for xx in w / 4..w * 3 / 4 {
                    let i = ((yy * w + xx) * 4) as usize;
                    bgra[i..i + 4].copy_from_slice(&[0x60, 0x50, 0xb0, 0xc0]);
                }
            }
            self.frame = Frame { w, h, bgra };
            return Some(&self.frame);
        }
        None
    }
    fn quit(&mut self) {
        self.rec("handler.quit".into());
    }
}

#[test]
#[ignore]
fn selftest() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let l2 = log.clone();
    run(move |_tx| {
        Box::new(Fake { log: l2, frame: Frame { w: 0, h: 0, bgra: vec![] }, step: 0, t0: 0, wait_until: 0 })
    });
    let log = log.lock().unwrap().join("\n");
    eprintln!("---- log ----\n{log}");
    assert!(log.contains("WalkDone"));
    assert!(log.contains("Landed { big: true }"));
    assert!(log.contains("chat visible=true"));
    assert!(log.contains("Pomodoro(Start { minutes: 25 })"));
    assert!(log.contains("pomodoro_left=Some(25)"));
    assert!(log.contains("Pomodoro(Cancel)"));
    assert!(log.contains("visible=false"));
    assert!(log.contains("Peek"));
    assert!(log.contains("handler.quit"));
}
