//! 输入框附件的打开辅助：图片/视频/音频由前端浮层直接预览（data URL 现成可用），
//! 文档类（pdf/docx/xlsx/pptx 等）应用内不解析——写临时文件交给系统默认程序打开。

use super::*;
use base64::Engine;

/// POST /api/attachments/open —— body: { name, mime, data } 或 { name, mime, path }
/// `data` 接受 data URL 或裸 base64（composer 里刚选的附件）；`path` 是工作区相对路径
/// （历史消息的附件只落了元数据，2026-11-11 视频解码兜底要用系统播放器打开它）。
/// 只接受 composer 允许的扩展名；文件名取 basename 并过滤 Windows 非法字符（防路径
/// 穿越）；解码后上限 40MB。写进数据目录缓存后用系统默认程序打开（2026-11-10）。
pub(super) async fn open_attachment(Json(req): Json<Value>) -> (StatusCode, Json<Value>) {
    let err = |m: String| (StatusCode::BAD_REQUEST, Json(json!({ "error": m })));
    let raw_name = req
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("attachment");
    let data = req.get("data").and_then(Value::as_str).unwrap_or("");
    let ws_path = req
        .get("path")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    // basename（截掉 / 与 \）→ 过滤 Windows 非法字符 → 去掉首尾点
    let base = raw_name.rsplit(['/', '\\']).next().unwrap_or("attachment");
    let safe: String = base
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let safe = safe.trim().trim_matches('.').to_string();
    if safe.is_empty() {
        return err("文件名为空".into());
    }
    let ext = safe.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    const ALLOWED: [&str; 28] = [
        "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "mp4", "webm", "mov", "mp3", "wav",
        "m4a", "ogg", "pdf", "docx", "xlsx", "pptx", "doc", "xls", "ppt", "csv", "rtf", "odt",
        "ods", "odp", "txt", "md",
    ];
    if !ALLOWED.contains(&ext.as_str()) {
        return err(format!("暂不支持打开 .{ext} 类型"));
    }
    // 工作区文件直开：不收 base64（历史附件落过盘），同一套扩展名白名单 + 工作区边界
    if data.is_empty() {
        if ws_path.is_empty() {
            return err("缺少附件内容或路径".into());
        }
        let full = match crate::tools::resolve_workspace_path(&ws_path) {
            Ok(p) => p,
            Err(e) => return err(format!("路径不可用: {e}")),
        };
        if !full.is_file() {
            return err("工作区里找不到这个附件文件".into());
        }
        return match std::process::Command::new("explorer.exe").arg(&full).spawn() {
            Ok(_) => (
                StatusCode::OK,
                Json(json!({ "ok": true, "path": full.display().to_string() })),
            ),
            Err(e) => err(format!("打开失败: {e}")),
        };
    }
    let b64 = data.split(',').next_back().unwrap_or(data).trim();
    let bytes = match base64::engine::general_purpose::STANDARD.decode(b64).or_else(|_| {
        base64::engine::general_purpose::STANDARD_NO_PAD.decode(b64.trim_end_matches('='))
    }) {
        Ok(b) => b,
        Err(_) => return err("附件数据不是合法 base64".into()),
    };
    if bytes.len() > 40 * 1024 * 1024 {
        return err("附件超过 40MB，无法打开".into());
    }
    // 内容 hash 命名：同一附件重复点开复用同一文件，内容不同不互相覆盖
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.len().hash(&mut h);
    if let Some(first) = bytes.first() {
        first.hash(&mut h);
    }
    if let Some(last) = bytes.last() {
        last.hash(&mut h);
    }
    let tag = format!("{:016x}", h.finish());
    let dir = crate::config::data_root()
        .join("cache")
        .join("attachment-preview");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return err(format!("建缓存目录失败: {e}"));
    }
    let path = dir.join(format!("{tag}-{safe}"));
    if let Err(e) = std::fs::write(&path, &bytes) {
        return err(format!("写临时文件失败: {e}"));
    }
    prune_old(&dir);
    // Windows：explorer 对文件路径会转交 ShellExecute，用默认关联程序打开
    match std::process::Command::new("explorer.exe").arg(&path).spawn() {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({ "ok": true, "path": path.display().to_string() })),
        ),
        Err(e) => err(format!("打开失败: {e}")),
    }
}

/// 缓存清理：只留最近 7 天（用户偶尔预览，不清理会无限增长）。
fn prune_old(dir: &std::path::Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for e in rd.flatten() {
        let Ok(meta) = e.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if now
            .duration_since(modified)
            .map(|d| d.as_secs() > 7 * 24 * 3600)
            .unwrap_or(false)
        {
            let _ = std::fs::remove_file(e.path());
        }
    }
}
