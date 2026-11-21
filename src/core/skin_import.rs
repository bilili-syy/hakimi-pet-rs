//! 角色包 `.zip` 安全导入器与检查报告（Phase 3 第 9 步）
//!
//! 安全边界（全部在写盘前完成校验，任一条违规整包拒绝、零文件落地）：
//! - 体积与防炸弹：压缩包 ≤ 64 MB，条目数 ≤ 200，单文件解压后 ≤ 16 MB，累计解压 ≤ 128 MB，高倍膨胀流实时截断拦截。
//! - 防路径穿越与特殊设备名：拒绝绝对路径、盘符/ADS 冒号 `:`、`..`、符号链接、控制字符及 `CON`/`NUL` 等 Windows 保留字。
//! - 严禁代码与可执行文件：白名单仅放行纯静态资源（`json`/`png`/`ogg`/`wav`/`mp3`/`flac`/`txt`/`md`），发现 `.exe`/`.dll`/脚本立即拦截。
//! - 原子落盘：先解压到同目录临时 `.import-*` 目录，全部通过 CRC32 与清单校验后再原子替换 `skins/<id>`，坏包绝不破坏旧角色。
use super::skin::{self, MANIFEST, OVERLAYS, STATES, VOICE_EXTS};
use flate2::read::DeflateDecoder;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::sync::Mutex;

pub const MAX_ZIP_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_UNCOMPRESSED_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_SINGLE_FILE_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 200;
const MAX_PATH_DEPTH: usize = 6;
const MAX_PNG_SIDE: u32 = 4096;

/// 角色包允许的静态资源扩展名（全小写）
pub const ALLOWED_EXTS: [&str; 8] = ["json", "png", "ogg", "wav", "mp3", "flac", "txt", "md"];

/// 导入成功后的检查报告
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportReport {
    pub id: String,
    pub display_name: String,
    pub format_version: u32,
    pub sprites_ok: usize,
    pub fallback_idle_count: usize,
    pub overlays_ok: usize,
    pub voice_files: usize,
    pub has_custom_tts: bool,
    pub warnings: Vec<String>,
}

impl ImportReport {
    /// 生成给气泡展示的简明体检摘要
    pub fn summary_bubble(&self) -> String {
        let mut parts = vec![format!("立绘 {}/{}", self.sprites_ok, STATES.len())];
        if self.fallback_idle_count > 0 {
            parts.push(format!("{} 个表情回退待机", self.fallback_idle_count));
        }
        if self.overlays_ok > 0 {
            parts.push(format!("叠加层 {}", self.overlays_ok));
        }
        if self.voice_files > 0 {
            parts.push(format!("语音 {} 条", self.voice_files));
        }
        if self.has_custom_tts {
            parts.push("自带声线配置".into());
        }
        let mut msg = format!("导入成功！现在是「{}」啦～（{}）", self.display_name, parts.join("，"));
        if let Some(first_warn) = self.warnings.first() {
            let short: String = first_warn.chars().take(28).collect();
            if self.warnings.len() == 1 {
                msg.push_str(&format!("；提示：{short}"));
            } else {
                msg.push_str(&format!("；另有 {} 条提示见日志", self.warnings.len()));
            }
        }
        msg
    }
}

/// 暂存最近一次导入成功的报告，供紧随其后的 `Behavior::on_skin_menu` 一次性取出展示
static LAST_REPORT: Mutex<Option<ImportReport>> = Mutex::new(None);

pub fn store_last_report(r: ImportReport) {
    *LAST_REPORT.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
}

pub fn take_last_report(id: &str) -> Option<ImportReport> {
    let mut g = LAST_REPORT.lock().unwrap_or_else(|e| e.into_inner());
    if g.as_ref().is_some_and(|r| r.id == id) { g.take() } else { None }
}

#[derive(Debug)]
struct RawEntry {
    name: String,
    is_dir: bool,
    method: u16,
    crc32: u32,
    comp_size: u32,
    uncomp_size: u32,
    local_offset: u32,
}

fn u16_le(b: &[u8], i: usize) -> u16 { u16::from_le_bytes([b[i], b[i + 1]]) }
fn u32_le(b: &[u8], i: usize) -> u32 { u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) }

/// 解码 ZIP 文件名：优先 UTF-8；Windows 资源管理器「发送到压缩文件夹」常写无标记的本地 ANSI/GBK，自动兜底解码
fn decode_zip_name(raw: &[u8], utf8_flag: bool) -> Result<String, String> {
    if let Ok(s) = std::str::from_utf8(raw) {
        return Ok(s.to_string());
    }
    if utf8_flag {
        return Err("ZIP 内文件名 UTF-8 编码损坏".into());
    }
    #[cfg(windows)]
    {
        use windows::Win32::Globalization::{CP_ACP, MB_ERR_INVALID_CHARS, MultiByteToWideChar};
        // SAFETY: 先传空输出缓冲探测 UTF-16 长度，再分配对应 Vec<u16> 写入
        unsafe {
            let n = MultiByteToWideChar(CP_ACP, MB_ERR_INVALID_CHARS, raw, None);
            if n > 0 {
                let mut wide = vec![0u16; n as usize];
                let n2 = MultiByteToWideChar(CP_ACP, MB_ERR_INVALID_CHARS, raw, Some(&mut wide));
                if n2 > 0 {
                    return String::from_utf16(&wide[..n2 as usize]).map_err(|_| "ZIP 内文件名编码无法识别".into());
                }
            }
        }
    }
    Err("ZIP 内文件名不是合法的 UTF-8 编码（建议用 UTF-8 重新打包）".into())
}

/// 定位 End of Central Directory 记录（反向扫描最多 64KB 注释区，校验长度精确吻合）
fn find_eocd(buf: &[u8]) -> Result<usize, String> {
    if buf.len() < 22 || !buf.starts_with(b"PK") {
        return Err("这不是合法的 .zip 压缩包".into());
    }
    let min_pos = buf.len().saturating_sub(22 + 65535);
    for pos in (min_pos..=buf.len() - 22).rev() {
        if &buf[pos..pos + 4] == b"PK\x05\x06" {
            let comment_len = u16_le(buf, pos + 20) as usize;
            if pos + 22 + comment_len == buf.len() {
                return Ok(pos);
            }
        }
    }
    Err("ZIP 目录结构不完整或已损坏".into())
}

fn parse_central_dir(buf: &[u8]) -> Result<Vec<RawEntry>, String> {
    let eocd = find_eocd(buf)?;
    let disk_num = u16_le(buf, eocd + 4);
    let cd_disk = u16_le(buf, eocd + 6);
    let disk_entries = u16_le(buf, eocd + 8) as usize;
    let total_entries = u16_le(buf, eocd + 10) as usize;
    let cd_size = u32_le(buf, eocd + 12) as usize;
    let cd_offset = u32_le(buf, eocd + 16) as usize;
    if disk_num != 0 || cd_disk != 0 || disk_entries != total_entries {
        return Err("不支持分卷压缩的 ZIP 包".into());
    }
    if cd_offset == 0xffff_ffff || cd_size == 0xffff_ffff || total_entries == 0xffff {
        return Err("不支持 ZIP64 超大归档格式".into());
    }
    if total_entries == 0 {
        return Err("压缩包是空的喵".into());
    }
    if total_entries > MAX_ENTRIES {
        return Err(format!("压缩包内文件过多（{} 个，上限 {MAX_ENTRIES} 个）", total_entries));
    }
    let cd_end = cd_offset.checked_add(cd_size).ok_or("ZIP 中央目录偏移溢出")?;
    if cd_end > eocd {
        return Err("ZIP 中央目录越界或已损坏".into());
    }
    let mut pos = cd_offset;
    let mut out = Vec::with_capacity(total_entries);
    for _ in 0..total_entries {
        if pos + 46 > cd_end || &buf[pos..pos + 4] != b"PK\x01\x02" {
            return Err("ZIP 中央目录条目损坏".into());
        }
        let gp_flag = u16_le(buf, pos + 8);
        if gp_flag & 0x01 != 0 {
            return Err("不支持带密码加密的 ZIP 包".into());
        }
        let method = u16_le(buf, pos + 10);
        if method != 0 && method != 8 {
            return Err(format!("不支持的 ZIP 压缩算法（method={method}，请用标准 Deflate 压缩）"));
        }
        let crc32 = u32_le(buf, pos + 16);
        let comp_size = u32_le(buf, pos + 20);
        let uncomp_size = u32_le(buf, pos + 24);
        if comp_size == 0xffff_ffff || uncomp_size == 0xffff_ffff {
            return Err("不支持 ZIP64 条目".into());
        }
        let name_len = u16_le(buf, pos + 28) as usize;
        let extra_len = u16_le(buf, pos + 30) as usize;
        let comment_len = u16_le(buf, pos + 32) as usize;
        let ext_attrs = u32_le(buf, pos + 38);
        let local_offset = u32_le(buf, pos + 42);
        // Unix symlink mode: S_IFLNK = 0o120000
        if ((ext_attrs >> 16) & 0o170000) == 0o120000 {
            return Err("安全拦截：角色包不允许包含符号链接（symlink）".into());
        }
        let name_end = pos.checked_add(46).and_then(|p| p.checked_add(name_len)).ok_or("ZIP 文件名长度异常")?;
        let next = name_end.checked_add(extra_len).and_then(|p| p.checked_add(comment_len)).ok_or("ZIP 扩展字段异常")?;
        if next > cd_end {
            return Err("ZIP 条目越界".into());
        }
        let utf8 = (gp_flag & 0x0800) != 0;
        let raw_name = decode_zip_name(&buf[pos + 46..name_end], utf8)?;
        let norm = raw_name.replace('\\', "/");
        let is_dir = norm.ends_with('/') || ((ext_attrs & 0x10) != 0 && uncomp_size == 0);
        out.push(RawEntry { name: norm, is_dir, method, crc32, comp_size, uncomp_size, local_offset });
        pos = next;
    }
    Ok(out)
}

/// 识别 macOS / Windows 打包时自动塞进来的无害系统隐藏元数据（必须在路径穿越检查通过后才允许忽略）
fn is_os_junk(segs: &[&str]) -> bool {
    if segs.first() == Some(&"__MACOSX") {
        return true;
    }
    matches!(segs.last().copied(), Some(".DS_Store" | "Thumbs.db" | "desktop.ini"))
}

/// 严格校验相对路径片段（拦截 `..`、根路径、冒号、保留设备名、非法字符）；返回干净的路径分段列表
fn validate_entry_segments(norm: &str) -> Result<Vec<&str>, String> {
    if norm.is_empty() || norm.starts_with('/') || norm.contains(':') || norm.chars().any(char::is_control) {
        return Err(format!("安全拦截：角色包内含非法路径「{norm}」"));
    }
    let trimmed = norm.trim_end_matches('/');
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let mut segs = Vec::new();
    for seg in trimmed.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            return Err(format!("安全拦截：检测到路径穿越攻击「{norm}」"));
        }
        if seg.chars().count() > 120
            || seg.ends_with(['.', ' '])
            || seg.chars().any(|c| "<>:\"|?*".contains(c))
        {
            return Err(format!("角色包内文件名不合法：「{norm}」"));
        }
        let stem = seg.split('.').next().unwrap_or(seg);
        if skin::is_win_reserved_stem(stem) {
            return Err(format!("安全拦截：文件名包含系统保留设备名「{seg}」"));
        }
        segs.push(seg);
    }
    if segs.len() > MAX_PATH_DEPTH {
        return Err(format!("角色包目录层级过深（超过 {MAX_PATH_DEPTH} 层）：「{norm}」"));
    }
    Ok(segs)
}

fn check_allowed_file(segs: &[&str], full_name: &str) -> Result<(), String> {
    let Some(&file) = segs.last() else { return Ok(()) };
    if file.starts_with('.') {
        return Err(format!("安全拦截：不允许包含隐藏文件「{full_name}」"));
    }
    let ext = file.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    if ALLOWED_EXTS.contains(&ext.as_str()) {
        Ok(())
    } else {
        Err(format!(
            "安全拦截：角色包不允许包含代码、脚本或非白名单文件（发现「{full_name}」，仅允许 png/json/ogg/wav/mp3/flac/txt/md）"
        ))
    }
}

fn extract_entry_bytes(buf: &[u8], e: &RawEntry, total_so_far: &mut u64) -> Result<Vec<u8>, String> {
    if e.uncomp_size as u64 > MAX_SINGLE_FILE_BYTES {
        return Err(format!("文件「{}」超过单文件 16 MB 上限", e.name));
    }
    let lo = e.local_offset as usize;
    if lo.checked_add(30).is_none_or(|end| end > buf.len()) || &buf[lo..lo + 4] != b"PK\x03\x04" {
        return Err(format!("文件「{}」本地头损坏", e.name));
    }
    let lname = u16_le(buf, lo + 26) as usize;
    let lextra = u16_le(buf, lo + 28) as usize;
    let data_start = lo.checked_add(30).and_then(|p| p.checked_add(lname)).and_then(|p| p.checked_add(lextra)).ok_or("ZIP 数据偏移异常")?;
    let data_end = data_start.checked_add(e.comp_size as usize).ok_or("ZIP 压缩数据长度异常")?;
    if data_end > buf.len() {
        return Err(format!("文件「{}」压缩数据截断", e.name));
    }
    let comp = &buf[data_start..data_end];
    let remaining_total = MAX_UNCOMPRESSED_BYTES.saturating_sub(*total_so_far);
    let cap = MAX_SINGLE_FILE_BYTES.min(remaining_total);

    let out = if e.method == 0 {
        if comp.len() as u64 > cap {
            return Err("解压总大小超过安全上限（128 MB）".into());
        }
        comp.to_vec()
    } else {
        let mut dec = DeflateDecoder::new(comp).take(cap + 1);
        let mut v = Vec::with_capacity((e.uncomp_size as usize).min(256 * 1024));
        dec.read_to_end(&mut v).map_err(|_| format!("解压「{}」失败（数据损坏）", e.name))?;
        if v.len() as u64 > cap {
            return Err("安全拦截：解压体积超过上限（单文件 16 MB / 整包 128 MB）".into());
        }
        v
    };

    if e.comp_size > 0
        && out.len() > 4 * 1024 * 1024
        && (out.len() as u64) > (e.comp_size as u64).saturating_mul(100)
    {
        return Err(format!("安全拦截：「{}」压缩率异常，疑似压缩炸弹", e.name));
    }
    if out.len() != e.uncomp_size as usize {
        return Err(format!("文件「{}」长度与 ZIP 清单不符", e.name));
    }
    if crc32fast::hash(&out) != e.crc32 {
        return Err(format!("文件「{}」CRC32 校验失败（压缩包可能已损坏）", e.name));
    }
    *total_so_far = total_so_far.saturating_add(out.len() as u64);
    Ok(out)
}

/// 只读 PNG 头拿尺寸（不依赖 render 层，保持 core 层零上层耦合）
fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let be = |i: usize| u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    let (w, h) = (be(16), be(20));
    (w > 0 && h > 0).then_some((w, h))
}

/// 清理浏览器下载常见的 `xxx (1)` 后缀，作为平铺压缩包的备选角色 ID
fn clean_zip_stem(zip_path: &Path) -> String {
    let raw = zip_path.file_stem().and_then(|s| s.to_str()).unwrap_or("").trim();
    if let Some((base, num)) = raw.rsplit_once('(')
        && let Some(digits) = num.strip_suffix(')')
        && !digits.is_empty()
        && digits.chars().all(|c| c.is_ascii_digit())
    {
        let trimmed = base.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    raw.to_string()
}

/// 从磁盘上的 `.zip` 文件安全导入到 `skins_root/<id>`，返回检查报告
pub fn import_zip(zip_path: &Path, skins_root: &Path) -> Result<ImportReport, String> {
    let ext = zip_path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if !ext.eq_ignore_ascii_case("zip") {
        return Err("请拖入或选择 .zip 格式的角色压缩包哦喵～".into());
    }
    let meta = std::fs::metadata(zip_path).map_err(|_| "读不到这个 .zip 文件喵".to_string())?;
    if !meta.is_file() {
        return Err("请选择一个 .zip 文件而不是文件夹喵".into());
    }
    if meta.len() > MAX_ZIP_BYTES {
        return Err(format!("角色包超过 {} MB 大小上限喵", MAX_ZIP_BYTES / (1024 * 1024)));
    }
    let bytes = std::fs::read(zip_path).map_err(|e| format!("读取压缩包失败：{e}"))?;
    import_zip_bytes(&bytes, &clean_zip_stem(zip_path), skins_root)
}

/// 纯字节流导入入口（方便单元测试直接构造好包/缺状态包/恶意包验证）
pub fn import_zip_bytes(zip_bytes: &[u8], fallback_stem: &str, skins_root: &Path) -> Result<ImportReport, String> {
    if zip_bytes.len() as u64 > MAX_ZIP_BYTES {
        return Err("角色包超过 64 MB 大小上限喵".into());
    }
    let raw_entries = parse_central_dir(zip_bytes)?;

    // 第一遍：先对所有条目做路径安全与扩展名白名单审查（任何一条违规立即整包拒绝，绝不部分解压）
    let mut file_entries: Vec<(Vec<String>, &RawEntry)> = Vec::new();
    for e in &raw_entries {
        let segs = validate_entry_segments(&e.name)?;
        if segs.is_empty() || is_os_junk(&segs) {
            continue;
        }
        if e.is_dir {
            continue;
        }
        check_allowed_file(&segs, &e.name)?;
        file_entries.push((segs.into_iter().map(str::to_string).collect(), e));
    }
    if file_entries.is_empty() {
        return Err("压缩包里没有可用的角色资源文件喵".into());
    }

    // 识别包结构：根目录直接有 skin.json，或所有文件都在同一个顶层文件夹 `<top>/` 下且含 `<top>/skin.json`
    let has_root_manifest = file_entries.iter().any(|(s, _)| s.len() == 1 && s[0].eq_ignore_ascii_case(MANIFEST));
    let (rel_entries, folder_hint): (Vec<(String, &RawEntry)>, String) = if has_root_manifest {
        let mapped = file_entries.into_iter().map(|(s, e)| (s.join("/"), e)).collect();
        (mapped, fallback_stem.to_string())
    } else {
        let first_top = file_entries[0].0[0].clone();
        let same_top = file_entries.iter().all(|(s, _)| s.len() >= 2 && s[0] == first_top);
        let has_top_manifest = same_top
            && file_entries.iter().any(|(s, _)| s.len() == 2 && s[1].eq_ignore_ascii_case(MANIFEST));
        if !has_top_manifest {
            return Err("找不到 skin.json（请放在压缩包最外层或第一层角色文件夹内）".into());
        }
        let mapped = file_entries.into_iter().map(|(s, e)| (s[1..].join("/"), e)).collect();
        (mapped, first_top)
    };

    // 第二遍：解压全部文件到内存校验表（受 128MB 总上限与 CRC32 保护）
    let mut total_bytes = 0u64;
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for (rel, entry) in rel_entries {
        let key = if rel.eq_ignore_ascii_case(MANIFEST) { MANIFEST.to_string() } else { rel };
        if files.contains_key(&key) {
            return Err(format!("压缩包内存在重复路径：「{key}」"));
        }
        let data = extract_entry_bytes(zip_bytes, entry, &mut total_bytes)?;
        files.insert(key, data);
    }

    // 校验 skin.json
    let manifest_bytes = files.get(MANIFEST).ok_or("找不到 skin.json")?;
    if manifest_bytes.len() as u64 > skin::MAX_MANIFEST_BYTES {
        return Err("skin.json 体积过大（上限 256 KB）".into());
    }
    let manifest_text = std::str::from_utf8(manifest_bytes).map_err(|_| "skin.json 必须是 UTF-8 编码文本")?;
    let spec = skin::parse(manifest_text)?;

    // 解析并校验最终角色 ID（优先 skin.json 的 "id"，否则取顶层文件夹名 / zip 文件名）
    let id = skin::safe_single_id(&spec.manifest_id)
        .or_else(|| skin::safe_single_id(&folder_hint))
        .or_else(|| skin::safe_single_id(fallback_stem))
        .ok_or_else(|| {
            format!(
                "角色 ID「{}」不合法（不能叫 hakimi、不能以 @/. 开头，也不能含路径特殊符号）",
                if !spec.manifest_id.is_empty() { &spec.manifest_id } else { &folder_hint }
            )
        })?;

    // 查找表（Windows 文件系统大小写不敏感，这里做大小写不敏感匹配以准确生成报告）
    let lookup = |rel: &str| -> Option<&Vec<u8>> {
        let norm = rel.trim().replace('\\', "/");
        let clean = norm.trim_start_matches("./");
        files.get(clean).or_else(|| {
            files.iter().find(|(k, _)| k.eq_ignore_ascii_case(clean)).map(|(_, v)| v)
        })
    };

    let mut warnings = spec.warnings.clone();
    let mut sprites_ok = 0usize;
    let mut has_valid_idle = false;
    for &st in &STATES {
        if let Some(rel) = spec.sprites.get(st) {
            match lookup(rel) {
                Some(b) => match png_dimensions(b) {
                    Some((w, h)) if w <= MAX_PNG_SIDE && h <= MAX_PNG_SIDE => {
                        sprites_ok += 1;
                        if st == "idle" {
                            has_valid_idle = true;
                        }
                    }
                    Some((w, h)) => warnings.push(format!("sprites.{st} ({rel}) 尺寸 {w}×{h} 超过 4096 上限")),
                    None => warnings.push(format!("sprites.{st} ({rel}) 不是有效的 PNG 图片")),
                },
                None => warnings.push(format!("sprites.{st} 指向的「{rel}」在包里不存在")),
            }
        }
    }
    let fallback_idle_count = if has_valid_idle && !spec.fallback_builtin {
        STATES.len().saturating_sub(sprites_ok)
    } else {
        0
    };
    if !spec.sprites.is_empty() && !has_valid_idle {
        warnings.push("未提供有效的 idle 待机立绘，没画的表情将回退内置哈基米".into());
    }

    let mut overlays_ok = 0usize;
    for &ov in &OVERLAYS {
        if let Some(rel) = spec.overlays.get(ov) {
            match lookup(rel) {
                Some(b) if png_dimensions(b).is_some_and(|(w, h)| w <= MAX_PNG_SIDE && h <= MAX_PNG_SIDE) => {
                    overlays_ok += 1;
                }
                Some(_) => warnings.push(format!("overlays.{ov} ({rel}) 不是有效的 PNG 图片或尺寸超限")),
                None => warnings.push(format!("overlays.{ov} 指向的「{rel}」在包里不存在")),
            }
        }
    }

    let voice_files = files
        .keys()
        .filter(|k| {
            k.rsplit_once('.')
                .is_some_and(|(_, ext)| VOICE_EXTS.iter().any(|ve| ext.eq_ignore_ascii_case(ve)))
        })
        .count();

    if let Some(ref_rel) = spec.tts.as_ref().and_then(|t| t.ref_audio.as_deref())
        && lookup(ref_rel).is_none()
    {
        warnings.push(format!("tts.refAudio 指向的「{ref_rel}」在包里不存在"));
    }

    // 原子落盘：先写入临时目录 `.import-<pid>-<rand>`，写完再替换 `skins_root/<id>`
    std::fs::create_dir_all(skins_root).map_err(|e| format!("无法创建 skins 目录：{e}"))?;
    let nonce = format!("{}-{}", std::process::id(), fastrand::u64(..));
    let stage_dir = skins_root.join(format!(".import-{nonce}"));
    let write_res = (|| -> Result<(), String> {
        std::fs::create_dir_all(&stage_dir).map_err(|e| format!("创建临时导入目录失败：{e}"))?;
        for (rel, data) in &files {
            let dst = skin::safe_join(&stage_dir, rel).ok_or_else(|| format!("非法相对路径：{rel}"))?;
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("创建子目录失败：{e}"))?;
            }
            std::fs::write(&dst, data).map_err(|e| format!("写入文件「{rel}」失败：{e}"))?;
        }
        Ok(())
    })();
    if let Err(e) = write_res {
        let _ = std::fs::remove_dir_all(&stage_dir);
        return Err(e);
    }

    let target_dir = skins_root.join(&id);
    let backup_dir = skins_root.join(format!(".old-{id}-{nonce}"));
    if target_dir.exists() {
        let _ = std::fs::remove_dir_all(&backup_dir);
        if let Err(e) = std::fs::rename(&target_dir, &backup_dir) {
            let _ = std::fs::remove_dir_all(&stage_dir);
            return Err(format!("替换已有角色包「{id}」失败（文件可能被占用）：{e}"));
        }
    }
    if let Err(e) = std::fs::rename(&stage_dir, &target_dir) {
        if backup_dir.exists() {
            let _ = std::fs::rename(&backup_dir, &target_dir);
        }
        let _ = std::fs::remove_dir_all(&stage_dir);
        return Err(format!("完成角色包安装失败：{e}"));
    }
    if backup_dir.exists() {
        let _ = std::fs::remove_dir_all(&backup_dir);
    }

    let display_name = if spec.name.is_empty() { id.clone() } else { spec.name.clone() };
    let report = ImportReport {
        id,
        display_name,
        format_version: spec.format_version_or_default(),
        sprites_ok,
        fallback_idle_count,
        overlays_ok,
        voice_files,
        has_custom_tts: spec.tts.is_some(),
        warnings,
    };
    for w in &report.warnings {
        super::log::log(&format!("skin import warning [{}]: {w}", report.id));
    }
    super::log::log(&format!(
        "skin imported: id={} name={} sprites={}/{} voice={}",
        report.id,
        report.display_name,
        report.sprites_ok,
        STATES.len(),
        report.voice_files
    ));
    Ok(report)
}

/// 清理异常中断遗留的 `.import-*` / `.old-*` 临时目录
pub fn cleanup_staging(skins_root: &Path) {
    let Ok(rd) = std::fs::read_dir(skins_root) else { return };
    let self_mark = format!("-{}-", std::process::id());
    for e in rd.flatten() {
        let name = e.file_name();
        let Some(s) = name.to_str() else { continue };
        if (s.starts_with(".import-") || s.starts_with(".old-")) && !s.contains(&self_mark) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::DeflateEncoder;
    use std::io::Write;
    use std::path::PathBuf;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hakimi-zip-{}-{tag}-{}", std::process::id(), fastrand::u32(..)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 生成合法的最小 2×2 RGBA PNG 字节
    fn mini_png() -> Vec<u8> {
        let mut p = tiny_skia::Pixmap::new(2, 2).unwrap();
        p.fill(tiny_skia::Color::from_rgba8(120, 220, 180, 255));
        p.encode_png().unwrap()
    }

    /// 纯 Rust 构造标准 ZIP 字节流（支持 method=0 Store 与 method=8 Deflate）
    fn build_zip(entries: &[(&str, &[u8], u16)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut cd = Vec::new();
        for &(name, data, method) in entries {
            let crc = crc32fast::hash(data);
            let comp = if method == 8 {
                let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
                enc.write_all(data).unwrap();
                enc.finish().unwrap()
            } else {
                data.to_vec()
            };
            let local_off = out.len() as u32;
            let nb = name.as_bytes();
            // Local file header
            out.extend_from_slice(b"PK\x03\x04");
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 flag
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(comp.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(nb.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(nb);
            out.extend_from_slice(&comp);

            // Central directory header
            cd.extend_from_slice(b"PK\x01\x02");
            cd.extend_from_slice(&20u16.to_le_bytes());
            cd.extend_from_slice(&20u16.to_le_bytes());
            cd.extend_from_slice(&0x0800u16.to_le_bytes());
            cd.extend_from_slice(&method.to_le_bytes());
            cd.extend_from_slice(&0u32.to_le_bytes());
            cd.extend_from_slice(&crc.to_le_bytes());
            cd.extend_from_slice(&(comp.len() as u32).to_le_bytes());
            cd.extend_from_slice(&(data.len() as u32).to_le_bytes());
            cd.extend_from_slice(&(nb.len() as u16).to_le_bytes());
            cd.extend_from_slice(&0u16.to_le_bytes());
            cd.extend_from_slice(&0u16.to_le_bytes());
            cd.extend_from_slice(&0u16.to_le_bytes());
            cd.extend_from_slice(&0u16.to_le_bytes());
            cd.extend_from_slice(&0u32.to_le_bytes());
            cd.extend_from_slice(&local_off.to_le_bytes());
            cd.extend_from_slice(nb);
        }
        let cd_off = out.len() as u32;
        let cd_len = cd.len() as u32;
        out.extend_from_slice(&cd);
        // EOCD
        let n = entries.len() as u16;
        out.extend_from_slice(b"PK\x05\x06");
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&cd_len.to_le_bytes());
        out.extend_from_slice(&cd_off.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    #[test]
    fn imports_good_zip_and_missing_states_fallback_report() {
        let root = tmp_dir("good");
        let png = mini_png();
        // 1. 标准带顶层文件夹的角色包（只画了 idle + happy + wink 3 张，附带 __MACOSX 垃圾文件自动忽略）
        let manifest = r#"{
            "formatVersion": 1,
            "name": "薄荷糖",
            "author": "测试作者",
            "persona": "你是薄荷糖喵。",
            "sprites": { "idle": "sprites/idle.png", "happy": "sprites/happy.png", "wink": "sprites/wink.png" },
            "overlays": { "blink": "sprites/blink.png" },
            "voiceDir": "voice",
            "tts": { "mode": "preset", "voice": "蜜桃" }
        }"#;
        let zip = build_zip(&[
            ("小薄荷/skin.json", manifest.as_bytes(), 8),
            ("小薄荷/sprites/idle.png", &png, 8),
            ("小薄荷/sprites/happy.png", &png, 0),
            ("小薄荷/sprites/wink.png", &png, 8),
            ("小薄荷/sprites/blink.png", &png, 8),
            ("小薄荷/voice/hello.ogg", b"OggS", 0),
            ("__MACOSX/._skin.json", b"\x00\x05\x16\x07", 0),
        ]);
        let rep = import_zip_bytes(&zip, "mint_archive", &root).unwrap();
        assert_eq!(rep.id, "小薄荷");
        assert_eq!(rep.display_name, "薄荷糖");
        assert_eq!(rep.format_version, 1);
        assert_eq!(rep.sprites_ok, 3);
        assert_eq!(rep.fallback_idle_count, STATES.len() - 3);
        assert_eq!(rep.overlays_ok, 1);
        assert_eq!(rep.voice_files, 1);
        assert!(rep.has_custom_tts);
        assert!(rep.warnings.is_empty(), "unexpected warnings: {:?}", rep.warnings);
        assert!(root.join("小薄荷").join("skin.json").is_file());
        assert!(!root.join("__MACOSX").exists());
        assert!(rep.summary_bubble().contains("立绘 3/32"));

        // 2. 平铺无顶层目录 + 缺失的图片文件记录检查警告 + 浏览器 (1) 后缀清理
        let flat_manifest = r#"{"name":"平铺猫","sprites":{"idle":"idle.png","cry":"missing.png"}}"#;
        let flat_zip = build_zip(&[
            ("skin.json", flat_manifest.as_bytes(), 8),
            ("idle.png", &png, 8),
        ]);
        let zpath = root.join("flat_cat (1).zip");
        std::fs::write(&zpath, &flat_zip).unwrap();
        let rep2 = import_zip(&zpath, &root).unwrap();
        assert_eq!(rep2.id, "flat_cat");
        assert_eq!(rep2.sprites_ok, 1);
        assert_eq!(rep2.fallback_idle_count, STATES.len() - 1);
        assert!(rep2.warnings.iter().any(|w| w.contains("missing.png")));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_traversal_executables_reserved_and_corrupt_archives() {
        let root = tmp_dir("evil");
        let png = mini_png();

        // 1. 路径穿越 `..`
        let trav = build_zip(&[
            ("evil/skin.json", br#"{"name":"x"}"#, 8),
            ("evil/../../startup/backdoor.txt", b"pwn", 0),
        ]);
        let err = import_zip_bytes(&trav, "evil", &root).unwrap_err();
        assert!(err.contains("路径穿越"), "{err}");
        assert!(!root.join("evil").exists(), "rejected zip must extract nothing");

        // 2. 可执行 / 脚本代码文件
        for bad_file in ["evil/hook.dll", "evil/run.exe", "evil/setup.ps1", "evil/index.js", "evil/script.py"] {
            let code_zip = build_zip(&[
                ("evil/skin.json", br#"{"name":"x"}"#, 8),
                ("evil/idle.png", &png, 8),
                (bad_file, b"MZ...", 0),
            ]);
            let err = import_zip_bytes(&code_zip, "evil", &root).unwrap_err();
            assert!(err.contains("安全拦截"), "{bad_file} -> {err}");
            assert!(!root.join("evil").exists());
        }

        // 3. 盘符 / NTFS ADS 冒号与 Windows 保留名
        let colon_zip = build_zip(&[("C:/Windows/skin.json", b"{}", 0)]);
        assert!(import_zip_bytes(&colon_zip, "x", &root).is_err());
        let con_zip = build_zip(&[("bad/skin.json", b"{}", 0), ("bad/CON.png", &png, 0)]);
        assert!(import_zip_bytes(&con_zip, "bad", &root).unwrap_err().contains("保留"));

        // 4. 禁止覆盖内置 hakimi ID
        let hakimi_zip = build_zip(&[("hakimi/skin.json", br#"{"name":"fake"}"#, 8)]);
        assert!(import_zip_bytes(&hakimi_zip, "hakimi", &root).is_err());

        // 5. CRC32 篡改拦截
        let mut tampered = build_zip(&[("ok/skin.json", br#"{"name":"ok"}"#, 0)]);
        let last_byte = tampered.iter().position(|&b| b == b'}').unwrap();
        tampered[last_byte - 1] = b'X';
        assert!(import_zip_bytes(&tampered, "ok", &root).unwrap_err().contains("CRC32"));

        // 6. 高压缩比炸弹（6MB 全零 Deflate 压出来仅几 KB，超过 100x 阈值立即拦截）
        let zeros = vec![0u8; 5 * 1024 * 1024];
        let bomb = build_zip(&[
            ("bomb/skin.json", br#"{"name":"bomb"}"#, 8),
            ("bomb/huge.txt", &zeros, 8),
        ]);
        let berr = import_zip_bytes(&bomb, "bomb", &root).unwrap_err();
        assert!(berr.contains("压缩炸弹"), "{berr}");

        let _ = std::fs::remove_dir_all(&root);
    }
}
