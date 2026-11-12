//! WebView 端 UI 偏好落盘（2026-11-12）。
//!
//! 界面微调规则（microTweaks）和自定义 CSS 原先只存在 WebView2 的 Local Storage，
//! 而 Local Storage 跟着 cache 走：迁移换数据目录、清缓存都会丢。改存数据目录后
//! 跟 settings 一起走，迁移/备份/导入都自动带上，cache 可以放心不迁。

use axum::{Json, http::StatusCode};
use serde_json::{Value, json};
use std::path::PathBuf;

/// UI 偏好文件（数据目录根下，导出/迁移随数据目录一起走）。
fn prefs_path() -> PathBuf {
    crate::config::data_root().join("ui-prefs.json")
}

const MAX_BYTES: usize = 2 * 1024 * 1024;

/// 只收这两类字段，顺手挡住把任意大 JSON 当存储用。
fn validate(v: &Value) -> Result<Vec<u8>, String> {
    let obj = v.as_object().ok_or("body 必须是 JSON 对象")?;
    for (k, val) in obj {
        match k.as_str() {
            "microTweaks" if val.is_object() => {}
            "customCss" if val.is_string() => {}
            "wallpaper" if val.is_string() => {}
            "wallpaperKind" if val.is_string() => {}
            "wallpaperOpacity" if val.is_number() => {}
            "microTweaks" | "customCss" | "wallpaper" | "wallpaperKind" | "wallpaperOpacity" => {
                return Err(format!("{k} 类型不对"));
            }
            other => return Err(format!("未知字段: {other}")),
        }
    }
    let bytes = serde_json::to_vec(v).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BYTES {
        return Err("UI 偏好超过 2MB".into());
    }
    Ok(bytes)
}

pub(super) async fn get_ui_prefs() -> Json<Value> {
    let text = std::fs::read_to_string(prefs_path()).unwrap_or_default();
    Json(serde_json::from_str(&text).unwrap_or_else(|_| json!({})))
}

pub(super) async fn put_ui_prefs(
    Json(v): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let bytes = validate(&v).map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))))?;
    let path = prefs_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("建数据目录失败: {e}") })),
            )
        })?;
    }
    // 先写临时文件再改名：写一半断电不会把好文件写坏
    let tmp = path.with_file_name("ui-prefs.json.tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("写 UI 偏好失败: {e}") })),
        )
    })?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("替换 UI 偏好失败: {e}") })),
        )
    })?;
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_known_fields() {
        let v = json!({
            "microTweaks": { "#app|.bubble": { "radius": "12px" } },
            "customCss": ".x{color:red}",
            "wallpaper": "/api/wallpaper/file",
            "wallpaperKind": "image",
            "wallpaperOpacity": 30
        });
        assert!(validate(&v).is_ok());
    }

    #[test]
    fn rejects_unknown_or_wrong_type() {
        assert!(validate(&json!({ "other": 1 })).is_err());
        assert!(validate(&json!({ "microTweaks": "x" })).is_err());
        assert!(validate(&json!({ "customCss": 1 })).is_err());
        assert!(validate(&json!([1, 2])).is_err());
    }

    #[test]
    fn rejects_oversize_css() {
        let big = "x".repeat(MAX_BYTES + 1);
        assert!(validate(&json!({ "customCss": big })).is_err());
    }
}
