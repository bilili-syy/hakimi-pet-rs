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

#[test]
fn esc_while_waiting_drops_the_reply() {
    let mut b = mk();
    tick(&mut b, 3000);
    ev(&mut b, AppEvent::Menu(MenuAction::Chat), 3000);
    ev(&mut b, AppEvent::ChatSubmit("讲个长故事".into()), 3500);
    assert_eq!(b.state, St::Think);
    let out = ev(&mut b, AppEvent::ChatStop, 4000);
    assert!(out.contains(&WinCmd::ChatInput(ChatInputCmd::Enable(true))));
    assert!(b.chatting, "Esc 只是不等了，输入框还开着");
    assert_ne!(b.state, St::Think);
    assert!(b.scene.bubble.as_ref().is_some_and(|x| lines::CHAT_STOP.contains(&x.text.as_str())));
    // 晚到的回复丢掉
    ev(&mut b, AppEvent::ChatReply(ChatReply { ok: true, text: "从前有座山".into(), need_key: false }), 9000);
    assert!(b.calls.iter().any(|c| c == "chat:dropped"));
    assert!(!b.scene.bubble.as_ref().is_some_and(|x| x.text.contains("从前有座山")));
    // 下一句正常
    ev(&mut b, AppEvent::ChatSubmit("那算了".into()), 10_000);
    ev(&mut b, AppEvent::ChatReply(ChatReply { ok: true, text: "好嘛～".into(), need_key: false }), 11_000);
    assert_eq!(b.scene.bubble.as_ref().unwrap().text, "好嘛～");
    // 没在等时 ChatStop = 关闭
    ev(&mut b, AppEvent::ChatStop, 20_000);
    assert!(!b.chatting);
}

#[test]
fn screen_roast_waits_like_chat() {
    let mut b = mk();
    tick(&mut b, 3000);
    ev(&mut b, AppEvent::ScreenRoast(vec![1, 2, 3]), 3500);
    assert_eq!(b.state, St::Think);
    assert!(b.calls.iter().any(|c| c == "roast:3"));
    assert!(b.scene.bubble.as_ref().is_some_and(|x| lines::ROAST_LOOK.contains(&x.text.as_str())));
    // 等着时再来一张不理
    ev(&mut b, AppEvent::ScreenRoast(vec![1]), 4000);
    assert!(!b.calls.iter().any(|c| c == "roast:1"));
    ev(&mut b, AppEvent::ChatReply(ChatReply { ok: true, text: "又在摸鱼！".into(), need_key: false }), 8000);
    assert_eq!(b.scene.bubble.as_ref().unwrap().text, "又在摸鱼！");
    ev(&mut b, AppEvent::RoastNote("你在录屏呢".into()), 20_000);
    assert_eq!(b.scene.bubble.as_ref().unwrap().text, "你在录屏呢");
}

#[test]
fn req_id_cancel_never_swallows_newer_reply_and_dance_stops_on_song_or_fullscreen() {
    let mut b = mk();
    tick(&mut b, 3_000);
    ev(&mut b, AppEvent::Menu(MenuAction::Chat), 3_000);
    ev(&mut b, AppEvent::ChatSubmit("慢问题".into()), 3_200);
    let req1 = b.active_chat_req;
    assert!(req1 > 0);
    // 按 Esc 取消 req1，紧接着发 req2；哪怕 req2 比旧的 req1 先返回，也必须收下 req2 并丢弃晚到的 req1
    ev(&mut b, AppEvent::ChatStop, 3_500);
    ev(&mut b, AppEvent::ChatSubmit("快问题".into()), 3_600);
    let req2 = b.active_chat_req;
    assert!(req2 > req1);
    let out = ev(&mut b, AppEvent::ChatReplyId(req2, ChatReply { ok: true, text: "快回答喵".into(), need_key: false }), 4_000);
    assert!(!b.waiting);
    assert!(out.contains(&WinCmd::ChatInput(ChatInputCmd::Enable(true))));
    assert_eq!(b.scene.bubble.as_ref().unwrap().text, "快回答喵");
    ev(&mut b, AppEvent::ChatReplyId(req1, ChatReply { ok: true, text: "已取消的旧回答".into(), need_key: false }), 5_000);
    assert_eq!(b.scene.bubble.as_ref().unwrap().text, "快回答喵");

    // 跳 Routine 中途点「哈基米之歌」或进入全屏：必须停掉 Routine
    let mut b = mk();
    tick(&mut b, 3_000);
    ev(&mut b, AppEvent::Menu(MenuAction::Dance(String::new())), 3_200);
    assert!(b.routine.is_some());
    ev(&mut b, AppEvent::Menu(MenuAction::Song), 3_500);
    assert!(b.routine.is_none());

    ev(&mut b, AppEvent::Menu(MenuAction::Dance(String::new())), 12_000);
    assert!(b.routine.is_some());
    ev(&mut b, AppEvent::Sense(crate::types::SenseEv::Fullscreen(true)), 12_500);
    assert!(b.routine.is_none());

    // 外部事件：逗猫棒进行中或前一条气泡还在时排队，不打断游戏、不秒顶前一条气泡
    let mut b = mk();
    tick(&mut b, 3_000);
    b.scene.bubble = None;
    b.bubble = None;
    ev(&mut b, AppEvent::Menu(MenuAction::Game), 4_000);
    ev(&mut b, AppEvent::External(crate::types::ExtEvent { kind: crate::types::ExtKind::Done, source: "Claude".into(), msg: "构建完成".into() }), 4_200);
    assert!(b.play.game.is_some());
    assert!(!b.scene.bubble.as_ref().is_some_and(|x| x.text.contains("构建完成")));
}

#[test]
fn calling_her_name_or_mentioning_honey_does_not_hijack_chat() {
    let mut b = mk();
    tick(&mut b, 3_000);
    // 正常喊名字聊天：应该走 AI 聊天，绝不能被当成梗直接跳舞
    ev(&mut b, AppEvent::ChatSubmit("哈基米，今天上班好累啊".into()), 3_200);
    assert_eq!(b.state, St::Think);
    assert!(b.calls.iter().any(|c| c == "chat:哈基米，今天上班好累啊"));
    ev(&mut b, AppEvent::ChatStop, 3_400);

    // 提到蜂蜜的正常对话：走 AI 聊天
    ev(&mut b, AppEvent::ChatSubmit("早上喝了蜂蜜柚子茶".into()), 3_600);
    assert_eq!(b.state, St::Think);
    assert!(b.calls.iter().any(|c| c == "chat:早上喝了蜂蜜柚子茶"));
    ev(&mut b, AppEvent::ChatStop, 3_800);

    // 纯玩梗短句和直接喂蜂蜜短句依然生效
    ev(&mut b, AppEvent::ChatSubmit("曼波！".into()), 4_000);
    assert_eq!(b.state, St::Dance);
    ev(&mut b, AppEvent::ChatSubmit("喝蜂蜜水".into()), 12_000);
    assert_eq!(b.state, St::Honey);
}

#[test]
fn need_key_reply_opens_ai_settings_page() {
    let mut b = mk();
    tick(&mut b, 3_000);
    ev(&mut b, AppEvent::ChatSubmit("你好".into()), 3_200);
    let req = b.active_chat_req;
    let out = ev(&mut b, AppEvent::ChatReplyId(req, ChatReply { ok: false, need_key: true, text: "要先填 Key".into() }), 3_500);
    assert!(out.contains(&WinCmd::OpenSettingsPage(0)));
}

#[test]
fn bubble_hover_wheel_click_copy_and_headroom() {
    let mut b = mk();
    tick(&mut b, 3_000);
    b.set_ui(UiInfo {
        bubble: Some(Rect { x: 8.0, y: 0.0, w: 224.0, h: 140.0 }),
        status: None,
        bubble_overflow: true,
    });
    b.say_opts("第一行\n第二行\n第三行\n第四行\n第五行", 2_000, false, SpeakKind::Chat);
    b.build_scene();
    assert!(b.scene.bubble.is_some());

    // 鼠标悬停在气泡上：超过原本 2000ms 消失时间也不会消失
    ev(&mut b, AppEvent::Mouse(MouseEv::Move { x: 100.0, y: 40.0, sx: 200, sy: 200 }), 3_500);
    tick(&mut b, 6_000);
    assert!(b.scene.bubble.is_some());

    // 在气泡上滚轮向下滚：变成手动滚动进度
    ev(&mut b, AppEvent::Mouse(MouseEv::Wheel { delta: -240, x: 100.0, y: 40.0 }), 6_100);
    assert!(b.scene.bubble.as_ref().unwrap().scroll > 0.3);

    // 窗口贴屏幕顶边时：bubble_top 下移气泡
    b.set_bubble_top(42.0);
    tick(&mut b, 6_200);
    assert_eq!(b.scene.bubble_top, 42.0);

    // 左键点气泡（已满 250ms）：立刻收起
    ev(&mut b, AppEvent::Mouse(MouseEv::Down { button: Button::Left, x: 100.0, y: 40.0, sx: 200, sy: 200 }), 6_300);
    tick(&mut b, 6_700);
    assert!(b.scene.bubble.is_none());

    // 就算气泡刚收起，点菜单「复制刚才那句话」也能复制出刚才那句正文
    ev(&mut b, AppEvent::Menu(MenuAction::CopyBubble), 6_800);
    assert!(b.calls.iter().any(|c| c == "copy:第一行\n第二行\n第三行\n第四行\n第五行"));
}

#[test]
fn new_expressions_and_question_fx_work_in_chat_and_sense() {
    let mut b = mk();
    tick(&mut b, 3_000);
    for (tag, expected_st, expected_face) in [
        ("[wink]略略略～", St::Wink, Face::Wink),
        ("[drool]吸溜，小鱼干好香喵", St::Drool, Face::Drool),
        ("[sweat]啊哈哈，什么都没看到喵", St::Sweat, Face::Sweat),
        ("[cheer]冲呀！给主人打 call 喵！", St::Cheer, Face::Cheer),
        ("[sleepy]眼皮好重，迷迷糊糊喵", St::Sleepy, Face::Sleepy),
        ("[confused]歪头？这是什么意思喵", St::Confused, Face::Confused),
    ] {
        let next_t = b.now + 5_000;
        ev(&mut b, AppEvent::ChatReply(ChatReply { ok: true, need_key: false, text: tag.into() }), next_t);
        assert_eq!(b.state, expected_st);
        assert_eq!(b.scene.face, expected_face);
    }
    // [confused] 会冒出蓝色问号粒子 Question
    assert!(b.scene.fx.iter().any(|f| f.shape == crate::render::FxShape::Question));

    // 快速打字触发举爪欢呼 St::Cheer
    b.set_state(St::Idle, None);
    b.chatting = false;
    b.waiting = false;
    let t = b.now + 5_000;
    ev(&mut b, AppEvent::Sense(crate::types::SenseEv::Typing), t);
    assert_eq!(b.state, St::Cheer);
    assert_eq!(b.scene.face, Face::Cheer);
}

#[test]
fn stop_voice_and_disable_tts_prevent_60s_stuck_bubble_and_emotion() {
    // 1. 普通语音播报中途关掉语音：60 秒兜底气泡与 60 秒情绪保持立刻缩短到 ~2 秒内收尾
    let mut b = mk();
    tick(&mut b, 3_000);
    ev(&mut b, AppEvent::ChatReply(ChatReply { ok: true, need_key: false, text: "[cheer]太棒啦喵！".into() }), 4_000);
    let tok = b.voice_req;
    assert_ne!(tok, 0);
    ev(&mut b, AppEvent::Speech { token: tok, audio: audio() }, 4_200);
    assert_eq!(b.bubble.as_ref().and_then(|x| x.hide_at), Some(64_200));
    assert_eq!(b.state, St::Cheer);

    ev(&mut b, AppEvent::Menu(MenuAction::Tts(false)), 5_000);
    // 气泡在 1.5s 后淡出（不再挂死 60s），表情在 2s 后回到 Idle
    tick(&mut b, 7_200);
    assert!(b.scene.bubble.is_none());
    assert_eq!(b.state, St::Idle);

    // 2. ChatVoice 分段播报中途关掉语音：剩余文字出全且表情不再卡死 60 秒
    let mut b2 = mk();
    b2.fake_chat_voice = true;
    tick(&mut b2, 3_000);
    ev(&mut b2, AppEvent::ChatSubmit("讲长一点".into()), 3_100);
    let long = "[wink]第一句先眨个眼喵！第二句有很多很多字，足够被切分成多段来顺序播报哦～第三句也紧随其后喵！";
    ev(&mut b2, AppEvent::ChatReply(ChatReply { ok: true, need_key: false, text: long.into() }), 4_000);
    let t1 = b2.token_seq;
    ev(&mut b2, AppEvent::Speech { token: t1, audio: audio() }, 4_500);
    assert_eq!(b2.state, St::Wink);
    ev(&mut b2, AppEvent::Menu(MenuAction::Tts(false)), 5_000);
    assert!(b2.cv.is_none());
    assert!(b2.scene.bubble.as_ref().unwrap().text.contains("第三句"));
    tick(&mut b2, 7_500);
    assert_eq!(b2.state, St::Idle, "CV_TOKEN emotion should clear within 2s after disabling TTS");
}

#[test]
fn hide_roast_memo_and_chat_edge_cases() {
    let mut b = mk();
    tick(&mut b, 3_000);

    // 1. 逗猫棒期间隐藏桌宠：清理置顶玩具；跳舞期间隐藏桌宠：静默停舞（不冒收尾台词）
    ev(&mut b, AppEvent::Menu(MenuAction::Game), 3_200);
    assert!(b.play.game.is_some());
    let out = ev(&mut b, AppEvent::Menu(MenuAction::ToggleVisible), 3_400);
    assert!(b.play.game.is_none());
    assert!(out.contains(&WinCmd::Toy(crate::types::ToyCmd::Remove)));

    ev(&mut b, AppEvent::Menu(MenuAction::Dance("wiggle".into())), 3_600);
    assert!(b.routine.is_some());
    let calls_before = b.calls.len();
    ev(&mut b, AppEvent::Menu(MenuAction::ToggleVisible), 4_000);
    assert!(b.routine.is_none());
    assert_eq!(b.state, St::Idle);
    assert!(!b.calls[calls_before..].iter().any(|c| c.starts_with("speak:")), "hiding should not speak dance outro");

    // 2. 聊天触发「曼波！」唱歌跳舞后，按 Esc 关掉空输入框：不会打断正在跳的舞
    ev(&mut b, AppEvent::Menu(MenuAction::Chat), 5_000);
    ev(&mut b, AppEvent::ChatSubmit("曼波！".into()), 5_100);
    assert_eq!(b.state, St::Dance);
    ev(&mut b, AppEvent::ChatCancel, 5_200);
    assert_eq!(b.state, St::Dance, "closing empty chat should not abort active dance");
    ev(&mut b, AppEvent::Menu(MenuAction::ToggleVisible), 5_300);

    // 3. 看屏吐槽未打开聊天框时（chatting == false, waiting == true）：think() 不会打断等待状态
    ev(&mut b, AppEvent::ScreenRoast(vec![1, 2, 3, 4]), 6_000);
    assert!(!b.chatting && b.waiting && b.chat_busy());
    assert_eq!(b.state, St::Think);
    b.think();
    assert_eq!(b.state, St::Think);
    ev(&mut b, AppEvent::ChatStop, 6_500);
    assert!(!b.waiting);
    ev(&mut b, AppEvent::ChatReply(ChatReply { ok: true, need_key: false, text: "已取消的吐槽".into() }), 6_600);
    assert_ne!(b.last_bubble_text, "已取消的吐槽");

    // 4. 备忘响铃期间收到 SecondInstance / Peek / ChatReply：90 秒提醒气泡不被覆盖，且高空标志复位
    b.play.perch = Some(crate::types::PerchKind::WindowTop);
    ev(
        &mut b,
        AppEvent::Memo(crate::types::MemoEv::Due(vec![crate::types::MemoHit { id: 42, text: "交周报".into(), late: false }])),
        7_000,
    );
    assert!(b.scene.bubble.as_ref().unwrap().text.contains("交周报"));
    ev(&mut b, AppEvent::SecondInstance, 7_500);
    assert!(b.play.perch.is_none(), "SecondInstance should reset perch state");
    assert!(b.scene.bubble.as_ref().unwrap().text.contains("交周报"), "SecondInstance must not overwrite memo alert");
    ev(&mut b, AppEvent::ChatReply(ChatReply { ok: true, need_key: false, text: "顺便回你一句喵".into() }), 8_000);
    assert!(b.scene.bubble.as_ref().unwrap().text.contains("交周报"), "ChatReply must not overwrite active memo alert");
    assert_eq!(b.last_bubble_text, "顺便回你一句喵");
}

#[test]
fn click_zone_double_click_spam_poke_wheel_and_perch_feed() {
    let mut b = mk();
    tick(&mut b, 3_000);

    // 1. 气泡被点击收起进入 250ms 淡出期间，同位置再次点击不应再被旧 bubble_rect 吞掉，而应直接戳到哈基米
    b.set_ui(UiInfo {
        bubble: Some(Rect { x: 8.0, y: 0.0, w: 224.0, h: 130.0 }),
        status: Some(Rect { x: 12.0, y: 8.0, w: 216.0, h: 120.0 }),
        bubble_overflow: false,
    });
    ev(&mut b, AppEvent::Menu(MenuAction::Status), 3_100);
    // 第一下点状态卡：收起状态卡
    ev(&mut b, AppEvent::Mouse(MouseEv::Down { button: Button::Left, x: 120.0, y: 100.0, sx: 200, sy: 200 }), 3_200);
    ev(&mut b, AppEvent::Mouse(MouseEv::Up { button: Button::Left, x: 120.0, y: 100.0, sx: 200, sy: 200 }), 3_210);
    assert!(!b.status_fade.on);
    // 状态卡淡出中且无开启气泡：在 y=100（头部）紧接着点一下，直接触发戳她
    let pokes0 = b.pokes.len();
    ev(&mut b, AppEvent::Mouse(MouseEv::Down { button: Button::Left, x: 120.0, y: 100.0, sx: 200, sy: 200 }), 3_250);
    ev(&mut b, AppEvent::Mouse(MouseEv::Up { button: Button::Left, x: 120.0, y: 100.0, sx: 200, sy: 200 }), 3_260);
    assert_eq!(b.pokes.len(), pokes0 + 1);

    // 2. 非溢出的短气泡盖在头顶时，在头部（y >= PET_TOP）滚轮依然能顺畅撸猫
    b.say_opts("呼噜～", 2_000, false, SpeakKind::Line);
    let mood0 = b.stats.data.mood;
    for i in 0..3 {
        ev(&mut b, AppEvent::Mouse(MouseEv::Wheel { delta: 120, x: 120.0, y: 100.0 }), 4_000 + i * 50);
    }
    assert!(b.stats.data.mood > mood0, "wheel on non-overflowing bubble over head should pet her");

    // 3. 头顶上方空白处（y < PET_TOP - 10）双击（如双击刚消散的气泡）：绝不误开聊天框
    b.dismiss_bubble();
    ev(&mut b, AppEvent::Mouse(MouseEv::DoubleClick { x: 120.0, y: 20.0 }), 4_500);
    assert!(!b.chatting, "double-clicking empty sky above PET_TOP must not open chat");

    // 4. 在身体上正常双击：立即打开聊天框，且清除残留的 down 状态
    b.pokes.clear();
    ev(&mut b, AppEvent::Mouse(MouseEv::Down { button: Button::Left, x: 120.0, y: 220.0, sx: 200, sy: 320 }), 6_000);
    ev(&mut b, AppEvent::Mouse(MouseEv::Up { button: Button::Left, x: 120.0, y: 220.0, sx: 200, sy: 320 }), 6_020);
    ev(&mut b, AppEvent::Mouse(MouseEv::Down { button: Button::Left, x: 120.0, y: 220.0, sx: 200, sy: 320 }), 6_150);
    ev(&mut b, AppEvent::Mouse(MouseEv::DoubleClick { x: 120.0, y: 220.0 }), 6_170);
    assert!(b.chatting && b.down.is_none());

    // 5. 模拟 Windows 连续快速狂戳（奇偶交替 Down+Up 与 Down+DoubleClick）：不会反复闪开聊天框，第 5 次顺利炸毛/鼓脸
    let mut t = 6_300;
    for step in 3..=6 {
        ev(&mut b, AppEvent::Mouse(MouseEv::Down { button: Button::Left, x: 120.0, y: 220.0, sx: 200, sy: 320 }), t);
        if step % 2 == 0 {
            ev(&mut b, AppEvent::Mouse(MouseEv::DoubleClick { x: 120.0, y: 220.0 }), t + 20);
        } else {
            ev(&mut b, AppEvent::Mouse(MouseEv::Up { button: Button::Left, x: 120.0, y: 220.0, sx: 200, sy: 320 }), t + 20);
        }
        t += 150;
    }
    assert!(!b.chatting, "rapid spam-clicking should keep chat closed after yielding");
    assert!(matches!(b.state, St::Hiss | St::Puff), "expected angry Hiss/Puff after rapid spam clicks, got {:?}", b.state);

    // 6. 栖息在窗口顶部（WindowTop）时把小鱼干直接拖到她嘴边松手：直接吃掉，不会穿模掉地上
    b.stats.data.hunger = 30.0;
    ev(&mut b, AppEvent::Perched(crate::types::PerchKind::WindowTop), 10_000);
    ev(&mut b, AppEvent::Menu(MenuAction::Feed), 10_200);
    ev(&mut b, AppEvent::Toy(crate::types::ToyEv::Grabbed), 10_300);
    let out = ev(&mut b, AppEvent::Toy(crate::types::ToyEv::Dropped { dx: 5.0, dy: 5.0 }), 10_500);
    assert!(out.contains(&WinCmd::Toy(crate::types::ToyCmd::Remove)));
    assert_eq!(b.state, St::Eat);
}
