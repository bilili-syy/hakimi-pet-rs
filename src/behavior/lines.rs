//! 台词：原样照搬 renderer.js 的 LINES / COSTUME_LINE / MOYU_NAME（文案以原版为准）
use crate::types::VoiceMood;

pub const POKE: &[&str] = &["喵？找哈基米有事吗～", "再戳要挠你了喵！", "哈基米在思考宇宙的奥秘喵", "要哈基米帮你查点什么吗？", "哈基米什么都知道哦～"];
pub const POKE_GREAT: &[&str] = &["今天心情超好喵！", "嘿嘿，你也在摸鱼吗？", "哈基米最喜欢你了～"];
pub const POKE_SAD: &[&str] = &["……哈基米不想说话", "哼，没人理哈基米", "心情不好喵……"];
pub const POKE_TIRED: &[&str] = &["好困……别戳了喵……", "让哈基米睡会儿嘛"];
pub const POKE_HUNGRY: &[&str] = &["先、先给哈基米吃的嘛……", "肚子在叫喵……听到了吗", "没力气陪你玩了，饿……"];
pub const POKE_MAD: &[&str] = &["够了够了！毛都要被戳秃了喵！", "哈基米生气了！哼！", "再戳就不理你了喵！"];
pub const FEED: &[&str] = &["小鱼干！哈基米最爱了喵～", "嗷呜——好吃！再来一条！", "吃饱了才有力气干活喵"];
pub const FEED_STARVING: &[&str] = &["终于有吃的了！呜呜呜得救了喵", "哈基米差点饿扁了……谢谢你！"];
pub const FULL: &[&str] = &["吃、吃不下了喵……", "肚子圆滚滚了，等会儿再吃", "再喂要变成大肥猫了！"];
pub const PAT: &[&str] = &["呼噜呼噜……再摸一下嘛", "耳朵、耳朵也要摸喵～", "哈基米心情变好了！"];
pub const DRAG: &[&str] = &["放、放哈基米下来喵！", "喵啊啊啊——", "不许拎哈基米！"];
pub const LAND_BIG: &[&str] = &["好痛喵！你是故意的吧", "哈基米的尾巴……毛都炸了"];
pub const HELLO: &[&str] = &["哈基米上线啦喵～", "今天也要一起加油喵！"];
pub const WALK: &[&str] = &["去巡逻一下喵～", "哈基米出门散步！", "这边好像有什么好玩的"];
pub const EDGE: &[&str] = &["到头了喵，掉头！", "屏幕的尽头……什么都没有喵"];
pub const SIT: &[&str] = &["坐一会儿喵", "哈基米歇歇脚～"];
pub const SLEEPY: &[&str] = &["哈——欠——困了喵……", "眼皮好重……"];
pub const WAKE: &[&str] = &["唔……谁吵醒哈基米的", "喵？天亮了吗……", "再睡五分钟嘛……"];
pub const WAKE_UP: &[&str] = &["睡饱啦！哈基米满血复活喵！", "伸——懒——腰——"];
pub const NIGHT: &[&str] = &["这么晚了还不睡吗？哈基米先陪你一会儿", "熬夜会掉毛的喵！"];
pub const HUNGRY: &[&str] = &["肚子饿了喵……有小鱼干吗？", "咕噜噜……哈基米饿了", "盯——（看着你的手）"];
pub const STARVING: &[&str] = &["要饿晕了喵……救命……", "小鱼干……小鱼干……"];
pub const TIRED: &[&str] = &["好累……想睡觉喵", "电量不足，需要充电……"];
pub const SAD: &[&str] = &["陪哈基米玩一下嘛……", "好无聊喵……"];
pub const THINK: &[&str] = &["嗯……让哈基米想想", "哈基米正在努力思考喵……", "星星在转，别急～"];
pub const WATER: &[&str] = &["喝口水吧喵！哈基米也要喝～", "提醒时间到：该喝水啦", "嘴巴干干的吧？去倒杯水喵"];
pub const STAND_UP: &[&str] = &["坐太久啦，起来伸个懒腰喵！", "站起来走两步嘛，哈基米陪你", "腰会变成大肥鱼的形状哦，快起来"];
pub const OFF_WORK: &[&str] = &["下班时间到啦！快收拾东西回家喵～", "还不下班吗？哈基米要罢工了！"];
pub const POMO_DONE: &[&str] = &["番茄钟结束！你超棒的喵，休息 5 分钟吧", "叮！一个番茄完成～奖励自己一下"];
pub const POMO_CANCEL: &[&str] = &["番茄钟取消了……下次加油喵"];
pub const CHASE: &[&str] = &["星星别跑喵！", "抓到了！……诶？又飘上去了", "喵喵喵喵——（原地转圈）"];
pub const PEEK: &[&str] = &["哈基米回来啦～", "探头——想我了吗？"];
pub const RELOADED: &[&str] = &["设置换好了喵！"];
pub const OUTFIT_ON: &[&str] = &["好看吗好看吗？哈基米转个圈给你看喵～", "新衣服！哈基米超喜欢！", "嘿嘿，今天的哈基米特别可爱喵"];
pub const OUTFIT_OFF: &[&str] = &["换回原来的样子啦喵", "还是原装的哈基米最好看～"];
pub const MOYU_ON: &[&str] = &["摸鱼监督开启！哈基米会盯着你的喵～", "好，哈基米来当监工！"];
pub const MOYU_OFF: &[&str] = &["不监督啦，你自己要自觉喵", "监工下班～"];
pub const MOYU_BACK: &[&str] = &["回来干活了！哈基米摸摸头～", "这才对嘛，哈基米陪你一起努力喵！", "好乖好乖，奖励一个哈基米的赞！"];
pub const FORGOT: &[&str] = &["咦？我们刚才在聊什么来着……（记忆清空了）"];
pub const SHY: &[&str] = &["呼噜呼噜……好舒服喵", "嘿嘿……被摸头了", "再、再摸一下也可以喵……"];
pub const RUB: &[&str] = &["诶嘿嘿，偷偷摸哈基米的头～", "呼噜噜……手法不错喵", "头毛要被揉乱啦喵～"];
pub const DIZZY: &[&str] = &["天、天在转……", "晕晕乎乎喵……好多星星", "别再晃了喵……要晕了"];
pub const SURPRISED: &[&str] = &["喵！吓哈基米一跳！", "诶？！干嘛突然戳我！"];
pub const CRY: &[&str] = &["呜呜呜……没人疼哈基米", "哇——哈基米要哭了"];
pub const HISS: &[&str] = &["哈——！！", "哈！哈基米要哈气了！", "飞机耳警告！哈——"];
pub const YAWN: &[&str] = &["哈——啊——伸懒腰", "唔……伸个懒腰喵"];
pub const LOVE: &[&str] = &["最喜欢你了喵！", "比心心～哈基米爱你"];
pub const STARE: &[&str] = &["盯——", "……（死亡凝视）", "盯——你在干嘛喵"];
pub const DANCE: &[&str] = &["哈基米～哈基米～", "曼波～曼波～", "哈基米哦南北绿豆～"];
pub const SONG: &[&str] = &["哈基米～哈基米～哈基米～哈基米哦南北绿豆～", "阿西嘎～阿西嘎～哈呀库奶龙～曼波！", "哈基米哦南北绿豆～曼波曼波～"];
pub const HONEY: &[&str] = &["蜂蜜水！舔了蜂蜜，脚步就会变快喵！", "其实哈基米就是蜂蜜的意思哦～咕嘟咕嘟"];
pub const FAST: &[&str] = &["冲冲冲！哈基米现在超快的喵！", "脚步变快啦——"];
pub const SLOW: &[&str] = &["蜂蜜的劲儿过去了喵……"];

/// 固定台词按「最可能在什么心情下说」预合成（PREWARM_MOOD；没写的是 calm）
pub const PREWARM: &[(&[&str], VoiceMood)] = &[
    (POKE, VoiceMood::Calm),
    (POKE_GREAT, VoiceMood::Happy),
    (POKE_SAD, VoiceMood::Sad),
    (POKE_TIRED, VoiceMood::Sleepy),
    (POKE_HUNGRY, VoiceMood::Hungry),
    (POKE_MAD, VoiceMood::Mad),
    (FEED, VoiceMood::Happy),
    (FEED_STARVING, VoiceMood::Hungry),
    (FULL, VoiceMood::Happy),
    (PAT, VoiceMood::Happy),
    (DRAG, VoiceMood::Mad),
    (LAND_BIG, VoiceMood::Mad),
    (HELLO, VoiceMood::Happy),
    (WALK, VoiceMood::Calm),
    (EDGE, VoiceMood::Calm),
    (SIT, VoiceMood::Sad),
    (SLEEPY, VoiceMood::Sleepy),
    (WAKE, VoiceMood::Sleepy),
    (WAKE_UP, VoiceMood::Calm),
    (NIGHT, VoiceMood::Sleepy),
    (HUNGRY, VoiceMood::Hungry),
    (STARVING, VoiceMood::Hungry),
    (TIRED, VoiceMood::Sleepy),
    (SAD, VoiceMood::Sad),
    (THINK, VoiceMood::Think),
    (WATER, VoiceMood::Calm),
    (STAND_UP, VoiceMood::Calm),
    (OFF_WORK, VoiceMood::Calm),
    (POMO_DONE, VoiceMood::Happy),
    (POMO_CANCEL, VoiceMood::Calm),
    (CHASE, VoiceMood::Calm),
    (PEEK, VoiceMood::Calm),
    (RELOADED, VoiceMood::Calm),
    (OUTFIT_ON, VoiceMood::Happy),
    (OUTFIT_OFF, VoiceMood::Calm),
    (MOYU_ON, VoiceMood::Calm),
    (MOYU_OFF, VoiceMood::Calm),
    (MOYU_BACK, VoiceMood::Happy),
    (FORGOT, VoiceMood::Calm),
    (SHY, VoiceMood::Happy),
    (RUB, VoiceMood::Happy),
    (DIZZY, VoiceMood::Sad),
    (SURPRISED, VoiceMood::Calm),
    (CRY, VoiceMood::Sad),
    (HISS, VoiceMood::Mad),
    (YAWN, VoiceMood::Sleepy),
    (LOVE, VoiceMood::Happy),
    (STARE, VoiceMood::Calm),
    (DANCE, VoiceMood::Happy),
    (SONG, VoiceMood::Happy),
    (HONEY, VoiceMood::Happy),
    (FAST, VoiceMood::Happy),
    (SLOW, VoiceMood::Calm),
];

// ---- 带参数的台词（原版里是函数）
pub fn back(h: &str) -> Vec<String> { vec![format!("你走了 {h}，哈基米好想你喵！"), format!("{h}没见了，有没有想哈基米？")] }
pub fn pomo_start(m: u32) -> Vec<String> { vec![format!("番茄钟开始，{m} 分钟专心干活喵！哈基米帮你看着"), format!("{m} 分钟，不许摸鱼哦～")] }
pub fn pomo_focus(m: u32) -> Vec<String> { vec![format!("还剩 {m} 分钟，专心专心！"), format!("别戳哈基米啦，还有 {m} 分钟喵")] }
pub fn moyu1(what: &str, m: u32) -> Vec<String> {
    vec![format!("{what}已经看了 {m} 分钟了喵……该回去干活了吧？"), format!("哈基米看到了哦，{what}刷了 {m} 分钟！"), format!("摸鱼 {m} 分钟了喵，休息够了没～")]
}
pub fn moyu2(what: &str, m: u32) -> Vec<String> {
    vec![format!("还在{what}！{m} 分钟了喵！"), format!("哈基米要生气了，已经 {m} 分钟了！"), "再不干活哈基米就把小鱼干没收了喵！".to_string()]
}
pub fn moyu3(_what: &str, m: u32) -> Vec<String> {
    vec![format!("{m} 分钟！！哈基米对你很失望喵……"), "你是不是忘了还有正事要做喵！".to_string(), "哼，哈基米不理你了，除非你现在就去干活！".to_string()]
}
pub fn moyu_pomo(what: &str) -> Vec<String> { vec![format!("番茄钟还没结束呢，怎么跑去{what}了喵！"), "说好专心的！快回来喵！".to_string()] }
pub fn level_up(n: &str) -> Vec<String> { vec![format!("和你的关系变成「{n}」了喵！"), format!("叮！亲密度升级：{n}～")] }

pub const WAKE_HUNGRY: &str = "肚子饿醒了喵……";
pub const PAT_SLEEPING: &str = "呼噜……（睡得更香了）";
pub const SONG_PLAYING: &str = "哈基米哈基米～🎵";

/// 换装台词（COSTUME_LINE）
pub fn costume_line(c: &str) -> &'static [&'static str] {
    match c {
        "xmas" => &["圣诞快乐喵！哈基米是今年最可爱的圣诞礼物～", "穿上小红裙啦，铃铛叮叮当～"],
        "maid" => &["欢迎回来，主人喵～", "女仆哈基米为您服务！要喝茶还是要小鱼干喵？"],
        "jk" => &["今天也要好好上学喵！", "水手服！哈基米是不是很像优等生～"],
        _ => &["换回卫衣啦，还是这件最舒服喵", "原装哈基米回来了！"],
    }
}

/// 摸鱼命中的关键词/进程 → 好听的名字（MOYU_NAME）
pub fn moyu_name(what: &str) -> String {
    let n = match what {
        "bilibili" | "哔哩哔哩" => "B站", "youtube" => "YouTube", "douyin" => "抖音", "tiktok" => "TikTok",
        "weibo" => "微博", "twitter" => "推特", "reddit" => "Reddit", "netflix" => "网飞", "twitch" => "Twitch",
        "instagram" => "Ins", "steam" | "steamwebhelper" => "Steam", "genshinimpact" | "yuanshen" => "原神",
        "leagueclient" | "league of legends" => "LOL", "qqmusic" => "QQ音乐", "cloudmusic" => "网易云",
        "" => "摸鱼",
        other => other,
    };
    n.to_string()
}

/// 离开时长（fmtAway）
pub fn fmt_away(mins: f64) -> String {
    if mins < 60.0 { format!("{} 分钟", mins.round() as i64) }
    else if mins < 1440.0 { format!("{} 个小时", (mins / 60.0).round() as i64) }
    else { format!("{} 天", (mins / 1440.0).round() as i64) }
}

/// 预合成清单：所有固定台词 + 换装台词（happy）
pub fn prewarm_items() -> Vec<(String, Option<VoiceMood>)> {
    let mut v: Vec<(String, Option<VoiceMood>)> = Vec::new();
    for (arr, m) in PREWARM {
        for t in arr.iter() { v.push((t.to_string(), Some(*m))); }
    }
    for c in ["xmas", "maid", "jk", ""] {
        for t in costume_line(c) { v.push((t.to_string(), Some(VoiceMood::Happy))); }
    }
    v.truncate(300);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn away_format() {
        assert_eq!(fmt_away(5.2), "5 分钟");
        assert_eq!(fmt_away(150.0), "3 个小时");
        assert_eq!(fmt_away(3000.0), "2 天");
    }

    #[test]
    fn prewarm_has_all_fixed_lines() {
        let v = prewarm_items();
        assert!(v.len() > 100 && v.len() <= 300);
        assert!(v.iter().any(|(t, m)| t == "哈——！！" && *m == Some(VoiceMood::Mad)));
        assert!(v.iter().any(|(t, m)| t == "欢迎回来，主人喵～" && *m == Some(VoiceMood::Happy)));
    }

    #[test]
    fn moyu_names() {
        assert_eq!(moyu_name("bilibili"), "B站");
        assert_eq!(moyu_name("某网站"), "某网站");
        assert_eq!(moyu_name(""), "摸鱼");
    }
}
