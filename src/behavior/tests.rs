//! 状态机的纯逻辑测试（io=false：不碰磁盘/网络/音频，时间全部手动给）
use super::*;
use crate::core::stats::Stats;

fn mk() -> Behavior {
    fastrand::seed(7);
    let tx = EventSender::new(|_| {});
    let (s, _) = Stats::load(None);
    Behavior::with(tx, s, false, 0.0, String::new(), Vec::new(), false)
}

fn ev(b: &mut Behavior, e: AppEvent, now: u64) -> Vec<WinCmd> {
    let mut out = Vec::new();
    b.event_at(e, now, &mut out);
    out
}

fn tick(b: &mut Behavior, now: u64) -> (u32, Vec<WinCmd>) {
    let mut out = Vec::new();
    let ms = b.frame_at(now, &mut out);
    (ms, out)
}

fn click(b: &mut Behavior, now: u64) {
    ev(b, AppEvent::Mouse(MouseEv::Down { button: Button::Left, x: 120.0, y: 250.0, sx: 500, sy: 500 }), now);
    ev(b, AppEvent::Mouse(MouseEv::Up { button: Button::Left, x: 120.0, y: 250.0, sx: 500, sy: 500 }), now + 10);
}

#[test]
fn greets_on_startup_then_idles() {
    let mut b = mk();
    tick(&mut b, 100);
    assert!(b.scene.bubble.is_none());
    tick(&mut b, 600);
    assert_eq!(b.state, St::Happy);
    assert!(b.scene.bubble.is_some());
    assert!(b.calls.iter().any(|c| c.starts_with("speak:")));
    tick(&mut b, 2200);
    assert_eq!(b.state, St::Idle);
    tick(&mut b, 5100);
    assert!(b.calls.iter().any(|c| c.starts_with("prewarm:")));
}

#[test]
fn first_meeting_intro() {
    let mut b = mk();
    b.intro = true;
    tick(&mut b, 600);
    assert_eq!(b.state, St::Starry);
    assert!(b.calls.iter().any(|c| c == &format!("speak:{}", lines::INTRO[0])));
    for t in (1000..20_000).step_by(500) { tick(&mut b, t); }
    for l in lines::INTRO { assert!(b.calls.iter().any(|c| c == &format!("speak:{l}")), "{l}"); }
    assert!(!b.intro);
}

#[test]
fn five_quick_pokes_make_her_hiss() {
    let mut b = mk();
    tick(&mut b, 3000);
    for i in 0..4 { click(&mut b, 3000 + i * 300); }
    assert!(!matches!(b.state, St::Hiss | St::Puff));
    click(&mut b, 4300);
    // 一半炸毛哈气、一半鼓脸赌气
    assert!(matches!(b.state, St::Hiss | St::Puff));
    assert!(matches!(b.scene.face, Face::Hiss | Face::Puff));
    tick(&mut b, 4320 + 2200);
    assert_eq!(b.state, St::Idle);
}

#[test]
fn drag_and_land() {
    let mut b = mk();
    tick(&mut b, 3000);
    ev(&mut b, AppEvent::Mouse(MouseEv::Down { button: Button::Left, x: 120.0, y: 250.0, sx: 500, sy: 500 }), 3000);
    let out = ev(&mut b, AppEvent::Mouse(MouseEv::Move { x: 0.0, y: 0.0, sx: 520, sy: 510 }), 3016);
    assert_eq!(b.state, St::Drag);
    assert!(out.contains(&WinCmd::DragMove { sx: 520, sy: 510 }));
    let (ms, _) = tick(&mut b, 3030);
    assert_eq!(ms, DRAG_FRAME_MS);
    let out = ev(&mut b, AppEvent::Mouse(MouseEv::Up { button: Button::Left, x: 0.0, y: 0.0, sx: 520, sy: 510 }), 3100);
    assert!(out.contains(&WinCmd::DragEnd));
    assert_eq!(b.state, St::Fall);
    ev(&mut b, AppEvent::Landed { big: false }, 3600);
    assert_eq!(b.state, St::Idle);
}

#[test]
fn shaking_while_dragging_makes_her_dizzy() {
    let mut b = mk();
    tick(&mut b, 3000);
    ev(&mut b, AppEvent::Mouse(MouseEv::Down { button: Button::Left, x: 120.0, y: 250.0, sx: 500, sy: 500 }), 3000);
    let mut x = 500;
    for i in 0..10 {
        x = if i % 2 == 0 { 560 } else { 440 };
        ev(&mut b, AppEvent::Mouse(MouseEv::Move { x: 0.0, y: 0.0, sx: x, sy: 500 }), 3000 + i * 50);
    }
    ev(&mut b, AppEvent::Mouse(MouseEv::Up { button: Button::Left, x: 0.0, y: 0.0, sx: x, sy: 500 }), 3600);
    ev(&mut b, AppEvent::Landed { big: true }, 4000);
    assert_eq!(b.state, St::Dizzy);
    assert!(!b.scene.fx.is_empty());
}

#[test]
fn walk_and_walk_done() {
    let mut b = mk();
    b.set_scale(1.5);
    tick(&mut b, 3000);
    let out = ev(&mut b, AppEvent::Menu(MenuAction::Walk), 3000);
    assert_eq!(b.state, St::Walk);
    let walk = out.iter().find_map(|c| if let WinCmd::Walk { dir, distance, speed } = c { Some((*dir, *distance, *speed)) } else { None });
    let (dir, dist, speed) = walk.expect("walk cmd");
    assert!(dir == 1 || dir == -1);
    assert!((120.0 * 1.5..=420.0 * 1.5).contains(&dist));
    assert!((speed - 1.6 * 1.5).abs() < 1e-3);
    // 走路帧轮换：walk1 → idle → walk2 → idle，跟整体动画的颠步同步（500ms 两步，脚落地时迈腿）
    tick(&mut b, 3170);
    assert_eq!(b.scene.face, Face::Idle);
    tick(&mut b, 3250);
    assert_eq!(b.scene.face, Face::Walk2);
    tick(&mut b, 3400);
    assert_eq!(b.scene.face, Face::Idle);
    tick(&mut b, 3500);
    assert_eq!(b.scene.face, Face::Walk1);
    let out = ev(&mut b, AppEvent::WalkDone { hit_edge: true }, 4000);
    assert_eq!(b.state, St::Idle);
    assert!(out.contains(&WinCmd::WalkStop));
}

#[test]
fn sleep_then_poke_wakes_to_sit() {
    let mut b = mk();
    tick(&mut b, 3000);
    ev(&mut b, AppEvent::Menu(MenuAction::Sleep), 3000);
    assert_eq!(b.state, St::Yawn);
    tick(&mut b, 4900);
    assert_eq!(b.state, St::Sleep);
    tick(&mut b, 6000);
    assert!(!b.scene.zzz.is_empty());
    let (ms, _) = tick(&mut b, 9000);
    assert!(ms <= SLEEP_FRAME_MS);
    click(&mut b, 9100);
    assert_eq!(b.state, St::Sit);
    assert!(b.scene.zzz.is_empty());
}

#[test]
fn chat_round_trip() {
    let mut b = mk();
    tick(&mut b, 3000);
    let out = ev(&mut b, AppEvent::Menu(MenuAction::Chat), 3000);
    assert!(out.contains(&WinCmd::ChatInput(ChatInputCmd::Open)));
    let out = ev(&mut b, AppEvent::ChatSubmit("  今天天气怎么样  ".into()), 3500);
    assert!(out.contains(&WinCmd::ChatInput(ChatInputCmd::Enable(false))));
    assert_eq!(b.state, St::Think);
    assert!(b.calls.iter().any(|c| c == "chat:今天天气怎么样"));
    // 等回复时不会自己乱跑
    tick(&mut b, 20_000);
    assert_eq!(b.state, St::Think);
    let out = ev(&mut b, AppEvent::ChatReply(ChatReply { ok: true, text: "晴天喵～".into(), need_key: false }), 21_000);
    assert!(out.contains(&WinCmd::ChatInput(ChatInputCmd::Enable(true))));
    assert_eq!(b.state, St::Happy);
    assert_eq!(b.scene.bubble.as_ref().unwrap().text, "晴天喵～");
    tick(&mut b, 23_600);
    assert_eq!(b.state, St::Idle);
    ev(&mut b, AppEvent::ChatCancel, 24_000);
    assert!(!b.chatting);
}

#[test]
fn meme_words_trigger_dance() {
    let mut b = mk();
    tick(&mut b, 3000);
    ev(&mut b, AppEvent::Menu(MenuAction::Chat), 3000);
    ev(&mut b, AppEvent::ChatSubmit("哈基米唱首歌".into()), 3100);
    assert_eq!(b.state, St::Dance);
    assert!(!b.calls.iter().any(|c| c.starts_with("chat:")));
    // 跳舞：每 420ms 翻一次身
    tick(&mut b, 3100 + 420);
    tick(&mut b, 3100 + 420 + 300);
    assert!(b.scene.flip < 0.0);
    tick(&mut b, 3100 + 7000);
    assert_eq!(b.state, St::Idle);
}

#[test]
fn speech_holds_bubble_until_audio_done() {
    let mut b = mk();
    tick(&mut b, 600); // 打招呼，发了语音请求
    let tok = b.voice_req;
    assert!(tok > 0);
    // 旧 token 的结果丢掉
    ev(&mut b, AppEvent::Speech { token: tok + 100, audio: Some(crate::types::Audio(vec![1, 2, 3])) }, 700);
    assert!(!b.calls.iter().any(|c| c.starts_with("play:")));
    ev(&mut b, AppEvent::Speech { token: tok, audio: Some(crate::types::Audio(vec![1, 2, 3])) }, 800);
    assert!(b.calls.iter().any(|c| *c == format!("play:{tok}")));
    b.state_timer = None; // 别让自主行为插话
    // 默认 2.6 秒后该消失，但还在念
    tick(&mut b, 5000);
    b.state_timer = None;
    assert!(b.scene.bubble.is_some());
    ev(&mut b, AppEvent::AudioDone { token: tok }, 6000);
    tick(&mut b, 7500);
    assert!(b.scene.bubble.is_none());
}

#[test]
fn frame_interval_idle_vs_animating() {
    let mut b = mk();
    tick(&mut b, 3000);
    let (ms, _) = tick(&mut b, 3000 + 500);
    assert!(ms <= IDLE_FRAME_MS);
    ev(&mut b, AppEvent::Menu(MenuAction::Feed), 4000);
    let (ms, _) = tick(&mut b, 4016);
    // 待机扭曲还在渐出（DEPTH_RAMP_MS）那几帧沿用灵动帧间隔，渐出完就是普通动画帧
    if b.scene.live.warping() { assert_eq!(ms, crate::render::live::LIVE_FRAME_MS.min(ANIM_FRAME_MS)); } else { assert_eq!(ms, ANIM_FRAME_MS); }
}

#[test]
fn idle_wakes_rarely() {
    // 静止状态下大部分帧都是 200ms（只被眨眼之类的计时器提前）
    let mut b = mk();
    b.live.force_low_power = true; // 灵动效果开着时待机是 ~18fps（见 behavior/live.rs 的测试）
    let mut t = 10_000;
    tick(&mut b, t);
    b.state_timer = None;
    b.fx.clear();
    b.bubble = None;
    b.shot = None;
    let mut long = 0;
    for _ in 0..50 {
        let (ms, _) = tick(&mut b, t);
        b.state_timer = None;
        if ms == IDLE_FRAME_MS { long += 1; }
        t += ms as u64;
    }
    assert!(long > 25, "long frames: {long}");
}

#[test]
fn costume_and_outfits_change_scene() {
    let mut b = mk();
    tick(&mut b, 3000);
    ev(&mut b, AppEvent::Menu(MenuAction::Costume("maid".into())), 3000);
    assert_eq!(b.scene.costume, "maid");
    assert_eq!(b.state, St::Happy);
    ev(&mut b, AppEvent::Menu(MenuAction::Outfit("santa".into(), true)), 3100);
    ev(&mut b, AppEvent::Menu(MenuAction::Outfit("glasses".into(), true)), 3200);
    assert_eq!(b.scene.outfits, vec!["santa".to_string(), "glasses".to_string()]);
    ev(&mut b, AppEvent::Menu(MenuAction::Outfit("santa".into(), false)), 3300);
    assert_eq!(b.scene.outfits, vec!["glasses".to_string()]);
    ev(&mut b, AppEvent::Menu(MenuAction::ClearOutfit), 3400);
    assert!(b.scene.outfits.is_empty());
    ev(&mut b, AppEvent::Menu(MenuAction::Costume("bogus".into())), 3500);
    assert_eq!(b.scene.costume, "sailor");
    ev(&mut b, AppEvent::Menu(MenuAction::Costume("hoodie".into())), 6500);
    assert_eq!(b.scene.costume, "hoodie");
    ev(&mut b, AppEvent::Menu(MenuAction::Costume("swim".into())), 9500);
    assert_eq!(b.scene.costume, "swim");
}

#[test]
fn status_card_shows_and_hides_on_click() {
    let mut b = mk();
    tick(&mut b, 3000);
    ev(&mut b, AppEvent::Menu(MenuAction::Status), 3000);
    tick(&mut b, 3300);
    assert!(b.scene.status.as_ref().is_some_and(|s| s.opacity > 0.99));
    b.set_ui(UiInfo { status: Some(Rect { x: 14.0, y: 46.0, w: 212.0, h: 140.0 }), ..Default::default() });
    let before = b.state;
    ev(&mut b, AppEvent::Mouse(MouseEv::Down { button: Button::Left, x: 120.0, y: 100.0, sx: 500, sy: 500 }), 3400);
    ev(&mut b, AppEvent::Mouse(MouseEv::Up { button: Button::Left, x: 120.0, y: 100.0, sx: 500, sy: 500 }), 3410);
    assert_eq!(b.state, before, "点状态卡不算戳她");
    tick(&mut b, 3800);
    assert!(b.scene.status.is_none());
}

#[test]
fn reminders_respect_sleep_and_chat() {
    let mut b = mk();
    tick(&mut b, 3000);
    ev(&mut b, AppEvent::Remind(RemindKind::Water), 3000);
    assert_eq!(b.state, St::Happy);
    b.set_state(St::Sleep, None);
    ev(&mut b, AppEvent::Remind(RemindKind::StandUp), 9000);
    assert_eq!(b.state, St::Sleep);
    ev(&mut b, AppEvent::Remind(RemindKind::OffWork), 9100);
    assert_eq!(b.state, St::Happy);
}

#[test]
fn moyu_levels() {
    let mut b = mk();
    tick(&mut b, 3000);
    ev(&mut b, AppEvent::Moyu(MoyuEv::Nag { level: 1, minutes: 20, what: "bilibili".into(), pomodoro: false }), 3000);
    assert_eq!(b.state, St::Stare);
    assert!(b.scene.bubble.as_ref().unwrap().text.contains("B站") || b.scene.bubble.as_ref().unwrap().text.contains("20"));
    ev(&mut b, AppEvent::Moyu(MoyuEv::Nag { level: 3, minutes: 40, what: "steam".into(), pomodoro: false }), 4000);
    assert_eq!(b.state, St::Hiss);
    ev(&mut b, AppEvent::Moyu(MoyuEv::Nag { level: 1, minutes: 3, what: "x".into(), pomodoro: true }), 5000);
    assert_eq!(b.state, St::Drag);
    ev(&mut b, AppEvent::Moyu(MoyuEv::Back { minutes: 40 }), 6000);
    assert_eq!(b.state, St::Happy);
}

#[test]
fn honey_makes_her_fast() {
    let mut b = mk();
    tick(&mut b, 3000);
    // 喂到不饱才能喝（stub 版 stats 永远 Ok）
    ev(&mut b, AppEvent::Menu(MenuAction::Honey), 3000);
    if b.state != St::Honey { return; } // 真 stats 下可能吃撑了
    let (_, out) = tick(&mut b, 5800);
    assert!(b.scene.glow);
    assert!(out.iter().any(|c| matches!(c, WinCmd::Walk { speed, .. } if *speed >= 5.0)));
    tick(&mut b, 5800 + 60_100);
    assert!(!b.scene.glow);
}

#[test]
fn rubbing_head_counts_as_pat() {
    let mut b = mk();
    tick(&mut b, 3000);
    let mut x = 100.0;
    for i in 0..8 {
        x += if i % 2 == 0 { 20.0 } else { -20.0 };
        ev(&mut b, AppEvent::Mouse(MouseEv::Move { x, y: 90.0, sx: 0, sy: 0 }), 3000 + i * 100);
    }
    assert_eq!(b.state, St::Shy);
}

#[test]
fn empty_chat_box_yields_to_other_actions() {
    let mut b = mk();
    tick(&mut b, 3000);
    ev(&mut b, AppEvent::Menu(MenuAction::Chat), 3000);
    assert!(b.chatting);
    // 空输入框：喂食会顺手把框关掉
    let out = ev(&mut b, AppEvent::Menu(MenuAction::Feed), 3500);
    assert!(out.contains(&WinCmd::ChatInput(ChatInputCmd::Close)));
    assert!(!b.chatting);
    // 再开，空着时被拖起来也会关
    ev(&mut b, AppEvent::Menu(MenuAction::Chat), 9000);
    ev(&mut b, AppEvent::Mouse(MouseEv::Down { button: Button::Left, x: 120.0, y: 250.0, sx: 500, sy: 500 }), 9100);
    let out = ev(&mut b, AppEvent::Mouse(MouseEv::Move { x: 160.0, y: 250.0, sx: 540, sy: 500 }), 9150);
    assert!(out.contains(&WinCmd::ChatInput(ChatInputCmd::Close)));
    assert!(!b.chatting);
}

#[test]
fn typed_chat_box_stays_and_blocks_autonomy() {
    let mut b = mk();
    tick(&mut b, 3000);
    ev(&mut b, AppEvent::Menu(MenuAction::Chat), 3000);
    b.fake_chat_text = true;
    // 打了字：喂食不关框
    let out = ev(&mut b, AppEvent::Menu(MenuAction::Feed), 3500);
    assert!(!out.contains(&WinCmd::ChatInput(ChatInputCmd::Close)));
    assert!(b.chatting);
    // 明确点「溜达」：照走，框留着（平台层让框跟着她）
    let out = ev(&mut b, AppEvent::Menu(MenuAction::Walk), 8000);
    assert!(out.iter().any(|c| matches!(c, WinCmd::Walk { .. })));
    assert!(!out.contains(&WinCmd::ChatInput(ChatInputCmd::Close)));
    assert!(b.chatting);
    // 走完后不自己溜达/睡觉：很久之后还是不动
    ev(&mut b, AppEvent::WalkDone { hit_edge: false }, 12_000);
    for t in (12_000..200_000).step_by(1000) {
        let (_, out) = tick(&mut b, t);
        assert!(!out.iter().any(|c| matches!(c, WinCmd::Walk { .. })), "walked at {t}");
    }
    assert!(b.chatting);
    assert_ne!(b.state, St::Sleep);
}

// ---------------------------------------------------------------- 语音回答（字和声音同步）

fn audio() -> Option<crate::types::Audio> { Some(crate::types::Audio(vec![1, 2, 3])) }

#[test]
fn chat_voice_waits_for_audio_then_shows_text_and_plays_together() {
    let mut b = mk();
    b.fake_chat_voice = true;
    tick(&mut b, 3000);
    ev(&mut b, AppEvent::Menu(MenuAction::Chat), 3000);
    ev(&mut b, AppEvent::ChatSubmit("讲讲天气".into()), 3100);
    let reply = "今天天气很好喵，阳光暖洋洋的，晒得哈基米都想打滚了！我们一起出去散散步吧，顺便买点小鱼干～记得带伞哦，下午可能会下雨，别淋湿了喵。";
    let out = ev(&mut b, AppEvent::ChatReply(ChatReply { ok: true, text: reply.into(), need_key: false }), 5000);
    // 回复到了但还在思考：输入框没解锁、气泡里还不是回复
    assert!(!out.contains(&WinCmd::ChatInput(ChatInputCmd::Enable(true))));
    assert_eq!(b.state, St::Think);
    let parts = super::chatvoice::split_sentences(reply);
    assert_eq!(parts.len(), 3, "{parts:?}");
    assert!(b.calls.iter().any(|c| *c == format!("cvspeak:{}", parts[0])));
    assert_ne!(b.scene.bubble.as_ref().map(|x| x.text.clone()), Some(parts[0].clone()));
    let t1 = b.token_seq;
    // 第一句合成好：字出现 + 同时开始念，并预合成第二句
    let out = ev(&mut b, AppEvent::Speech { token: t1, audio: audio() }, 6500);
    assert!(out.contains(&WinCmd::ChatInput(ChatInputCmd::Enable(true))));
    assert_eq!(b.state, St::Happy);
    assert_eq!(b.scene.bubble.as_ref().unwrap().text, parts[0]);
    let p1 = b.voice_playing.expect("playing first sentence");
    assert!(b.calls.iter().any(|c| *c == format!("cvplay:{p1}")));
    assert!(b.calls.iter().any(|c| *c == format!("cvspeak:{}", parts[1])));
    let t2 = b.token_seq;
    // 第二句先合成好：等第一句念完才出字
    ev(&mut b, AppEvent::Speech { token: t2, audio: audio() }, 7000);
    assert_eq!(b.scene.bubble.as_ref().unwrap().text, parts[0]);
    ev(&mut b, AppEvent::AudioDone { token: p1 }, 8000);
    assert_eq!(b.scene.bubble.as_ref().unwrap().text, format!("{}{}", parts[0], parts[1]));
    let p2 = b.voice_playing.expect("playing second sentence");
    assert_ne!(p1, p2);
    // 第三句合成失败：第二句念完把剩下的字全显示，不出声
    let t3 = b.token_seq;
    ev(&mut b, AppEvent::Speech { token: t3, audio: None }, 8500);
    ev(&mut b, AppEvent::AudioDone { token: p2 }, 9500);
    assert_eq!(b.scene.bubble.as_ref().unwrap().text, reply);
    assert!(b.cv.is_none());
}

#[test]
fn chat_voice_timeout_shows_text_without_voice() {
    let mut b = mk();
    b.fake_chat_voice = true;
    tick(&mut b, 3000);
    ev(&mut b, AppEvent::Menu(MenuAction::Chat), 3000);
    ev(&mut b, AppEvent::ChatSubmit("你好".into()), 3100);
    ev(&mut b, AppEvent::ChatReply(ChatReply { ok: true, text: "你好呀喵～".into(), need_key: false }), 4000);
    assert_eq!(b.state, St::Think);
    let t = b.token_seq;
    tick(&mut b, 4000 + 14_000);
    assert_eq!(b.state, St::Think);
    // 15 秒还没合成好：直接出字
    let out = { let mut o = Vec::new(); b.frame_at(4000 + 15_001, &mut o); o };
    assert!(out.contains(&WinCmd::ChatInput(ChatInputCmd::Enable(true))));
    assert_eq!(b.scene.bubble.as_ref().unwrap().text, "你好呀喵～");
    assert!(b.calls.iter().any(|c| c == "cvtimeout"));
    // 迟到的语音不再播
    ev(&mut b, AppEvent::Speech { token: t, audio: audio() }, 20_000);
    assert!(!b.calls.iter().any(|c| c.starts_with("cvplay:") || c.starts_with("play:")));
}

#[test]
fn chat_voice_off_shows_text_immediately_without_tts() {
    let mut b = mk();
    b.fake_chat_voice_off = true;
    tick(&mut b, 3000);
    ev(&mut b, AppEvent::Menu(MenuAction::Chat), 3000);
    ev(&mut b, AppEvent::ChatSubmit("你好".into()), 3100);
    let before = b.calls.iter().filter(|c| c.starts_with("speak:")).count();
    ev(&mut b, AppEvent::ChatReply(ChatReply { ok: true, text: "你好呀喵～".into(), need_key: false }), 4000);
    assert_eq!(b.scene.bubble.as_ref().unwrap().text, "你好呀喵～");
    assert_eq!(b.state, St::Happy);
    assert_eq!(b.calls.iter().filter(|c| c.starts_with("speak:")).count(), before);
    assert!(!b.calls.iter().any(|c| c.starts_with("cvspeak:")));
}

#[test]
fn sleep_wake_jump_does_not_storm_timers() {
    let mut b = mk();
    tick(&mut b, 3_000);
    // 电脑休眠 8 小时后唤醒：on_blink / on_tick 按真实 now 重排，不应追赶成 1ms 死循环风暴
    let jump = 3_000 + 8 * 3_600_000;
    let mut out = Vec::new();
    b.frame_at(jump, &mut out);
    assert!(b.next_tick > jump);
    assert!(b.next_blink > jump);
    assert!(b.next_interval() >= 16);
}

#[test]
fn rub_ignored_during_game_and_perch_lost_cancels_dance() {
    let mut b = mk();
    tick(&mut b, 3_000);
    // 逗猫游戏中快速晃鼠标不应触发揉头，以免打断游戏状态
    ev(&mut b, AppEvent::Menu(MenuAction::Game), 3_100);
    assert!(b.play.game.is_some());
    let mood0 = b.stats.data.mood;
    for i in 0..8 {
        let x = if i % 2 == 0 { 90.0 } else { 150.0 };
        ev(&mut b, AppEvent::Mouse(MouseEv::Move { x, y: 120.0, sx: x as i32, sy: 120 }), 3_200 + i * 30);
    }
    assert!(b.play.game.is_some());
    assert_eq!(b.stats.data.mood, mood0);

    // 跳舞中途栖息窗口消失：必须取消 routine，不能在地上继续跳
    let mut b = mk();
    tick(&mut b, 3_000);
    ev(&mut b, AppEvent::Menu(MenuAction::Dance(String::new())), 3_200);
    assert!(b.routine.is_some());
    ev(&mut b, AppEvent::PerchLost, 3_500);
    assert!(b.routine.is_none());
}

#[test]
fn double_honey_fast_keeps_glow_and_tts_off_aborts_cv_and_outfit_noop() {
    let mut b = mk();
    tick(&mut b, 3_000);
    // 连续喂两次蜂蜜：第一次到期时 fast_until 已被延后，glow 不能被提前关掉
    b.stats.data.hunger = 30.0;
    ev(&mut b, AppEvent::Menu(MenuAction::Honey), 3_100);
    tick(&mut b, 6_000); // 触发第一次 Later::HoneyFast（glow = true，排 60s 后 HoneyFastEnd）
    assert!(b.scene.glow);
    let first_end = b.laters.iter().find(|l| l.1 == Later::HoneyFastEnd).map(|l| l.0).unwrap();
    ev(&mut b, AppEvent::Menu(MenuAction::Honey), 20_000);
    tick(&mut b, 23_000); // 触发第二次 Later::HoneyFast，把 fast_until 推到 83_000
    tick(&mut b, first_end + 100);
    assert!(b.scene.glow);
    // 第二次真正到期后才关 glow
    tick(&mut b, 23_000 + 61_000);
    assert!(!b.scene.glow);

    // MenuAction::Tts(false) 应中止正在等语音的 ChatVoice 并立即显示文本
    let mut b = mk();
    b.fake_chat_voice = true;
    tick(&mut b, 3_000);
    ev(&mut b, AppEvent::Menu(MenuAction::Chat), 3_000);
    ev(&mut b, AppEvent::ChatSubmit("你好".into()), 3_100);
    ev(&mut b, AppEvent::ChatReply(ChatReply { ok: true, text: "收到喵～".into(), need_key: false }), 4_000);
    assert!(b.cv.is_some());
    let out = ev(&mut b, AppEvent::Menu(MenuAction::Tts(false)), 4_500);
    assert!(b.cv.is_none());
    assert!(out.contains(&WinCmd::ChatInput(ChatInputCmd::Enable(true))));
    assert_eq!(b.scene.bubble.as_ref().unwrap().text, "收到喵～");

    // 没穿配饰时点「清空配饰」不应说出脱衣台词
    let mut b = mk();
    tick(&mut b, 3_000);
    b.scene.bubble = None;
    ev(&mut b, AppEvent::Menu(MenuAction::ClearOutfit), 4_000);
    assert!(b.scene.bubble.is_none());
}

#[test]
fn off_work_remind_deferred_during_fullscreen_and_chat() {
    // 1. 全屏安静时触发下班提醒：先静默推迟，退出全屏立刻补说
    let mut b = mk();
    tick(&mut b, 3_000);
    ev(&mut b, AppEvent::Sense(crate::types::SenseEv::Fullscreen(true)), 4_000);
    ev(&mut b, AppEvent::Remind(RemindKind::OffWork), 5_000);
    assert!(b.pending_off_work.is_some());
    assert_ne!(b.state, St::Happy);
    ev(&mut b, AppEvent::Sense(crate::types::SenseEv::Fullscreen(false)), 10 * 60_000);
    assert!(b.pending_off_work.is_none());
    assert_eq!(b.state, St::Happy);
    assert!(b.scene.bubble.as_ref().is_some_and(|x| lines::OFF_WORK.contains(&x.text.as_str())));

    // 2. 聊天中触发下班提醒：先推迟，关闭聊天框后由重试定时器补说
    let mut b = mk();
    tick(&mut b, 3_000);
    ev(&mut b, AppEvent::Menu(MenuAction::Chat), 4_000);
    ev(&mut b, AppEvent::Remind(RemindKind::OffWork), 5_000);
    assert!(b.pending_off_work.is_some());
    ev(&mut b, AppEvent::ChatCancel, 7_000);
    tick(&mut b, 11_000);
    assert!(b.pending_off_work.is_none());
    assert_eq!(b.state, St::Happy);
    assert!(b.scene.bubble.as_ref().is_some_and(|x| lines::OFF_WORK.contains(&x.text.as_str())));
}

#[test]
fn usage_feeds_her_and_stuffed_talk() {
    let mut b = mk();
    b.stats.data.hunger = 40.0;
    // 第一次只记基准，不加
    b.feed_from_usage(5_000);
    assert_eq!(b.stats.data.hunger, 40.0);
    // 新吃了 3000 份 → +3
    b.feed_from_usage(8_000);
    assert!((b.stats.data.hunger - 43.0).abs() < 1e-9);
    // 一下吃很多：最多 +10
    b.feed_from_usage(108_000);
    assert!((b.stats.data.hunger - 53.0).abs() < 1e-9);
    // 跨天归零：不扣
    b.feed_from_usage(10);
    assert!((b.stats.data.hunger - 53.0).abs() < 1e-9);
    assert_eq!(super::chat::stuffed_text("好呀～", 0.1), "好呀～…嗝～");
    assert_eq!(super::chat::stuffed_text("好呀！", 0.9), "好呀～～～");
    assert_eq!(super::chat::stuffed_text("嗝，吃饱了", 0.9), "嗝，吃饱了");
}
