//! 安卓面板后端——右侧「安卓」面板：启动/停止 + 帧流（SSE）+ 坐标回传。
//!
//! 与 agent 的 android 工具共享同一台模拟器与帧源；面板是用户亲自操作，
//! 不走审批门，但受全局来源校验与插件能力闸（guard_android_plugin）保护。
//! 帧流是变化驱动的 PNG：静止画面不推帧，前端拿 state 里 frame_seq 判断是否刷新。

use super::*;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use futures_util::stream::Stream;

fn bad(e: String) -> (StatusCode, Json<Value>) {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": e })))
}

/// 工具入口复用：面板动作与 agent 走同一条 execute（能力闸/状态机只有一份）。
async fn via_tool(args: Value) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let raw = serde_json::to_string(&args).unwrap_or_else(|_| "{}".to_string());
    match crate::android::execute(&raw, usize::MAX, &crate::tools::workspace_root()).await {
        Ok(text) => Ok(Json(json!({ "ok": true, "text": text }))),
        Err(e) => Err(bad(e)),
    }
}

/// GET /api/android/state —— 运行状态 + SDK/AVD（前端面板数据源）。
pub(super) async fn android_state() -> Json<Value> {
    Json(crate::android::status_json())
}

/// POST /api/android/start —— 启动模拟器（body 可带 avd 名；等 boot 完成）。
pub(super) async fn android_start(Json(req): Json<Value>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let avd = req.get("avd").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    match crate::android::start(avd, true).await {
        Ok(v) => Ok(Json(v)),
        Err(e) => Err(bad(e)),
    }
}

/// POST /api/android/stop —— 关模拟器。
pub(super) async fn android_stop() -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    match crate::android::stop().await {
        Ok(v) => Ok(Json(v)),
        Err(e) => Err(bad(e)),
    }
}

/// GET /api/android/frames —— SSE 帧流：先补一帧 LATEST，再广播变化帧。
/// 静止画面不推帧（模拟器变化驱动），前端不用做轮询。b64 按发送时才算
/// （没有 SSE 订阅者时泵侧零编码）。
pub(super) async fn android_frames() -> Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>> {
    use futures_util::StreamExt as _;
    let b64f = |f: &std::sync::Arc<crate::android::Frame>| {
        json!({ "seq": f.seq, "png": crate::android::b64_frame(&f.png), "w": f.w, "h": f.h }).to_string()
    };
    let first = crate::android::latest_frame().map(|f| Ok(Event::default().data(b64f(&f))));
    let rx = crate::android::subscribe_frames();
    let rest = futures_util::stream::unfold(rx, move |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(f) => {
                    let data = b64f(&f);
                    return Some((Ok(Event::default().data(data)), rx));
                }
                // 慢订阅掉帧：跳过旧的继续推新的
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    // 首帧与后续流拼接要用 Stream 的 chain（Iterator 的 chain 不带 async）
    Sse::new(futures_util::stream::iter(first).chain(rest)).keep_alive(KeepAlive::default())
}

/// GET /api/android/frame?seq=<客户端手里的 seq> —— 当前帧 JSON 快照。
/// seq 没变 = 画面没动：只回小 JSON（几行），**不带 png**——dock 每 600ms 轮询，
/// 旧实现不管动没动都回 ~1MB 的 base64，静止画面也在持续空转流量与序列化
/// （2026-11-23 CPU 实测后修正）。变了才带 base64 PNG；无帧返回全 null。
pub(super) async fn android_frame(
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Json<Value> {
    let client_seq = q.get("seq").and_then(|s| s.parse::<u32>().ok());
    match crate::android::latest_frame() {
        Some(f) if client_seq == Some(f.seq) => Json(json!({ "seq": f.seq, "png": null, "w": f.w, "h": f.h })),
        Some(f) => Json(json!({ "seq": f.seq, "png": crate::android::b64_frame(&f.png), "w": f.w, "h": f.h })),
        None => Json(json!({ "seq": null, "png": null, "w": null, "h": null })),
    }
}

/// GET /api/android/screenshot —— 当前画面 PNG（前端 <img> 兜底源，连接 SSE 前先拿一张）。
pub(super) async fn android_screenshot() -> Result<impl IntoResponse, (StatusCode, Json<Value>)> {
    match crate::android::current_png().await {
        Ok(png) => Ok((
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "image/png")],
            png,
        )),
        Err(e) => Err(bad(e)),
    }
}

/// POST /api/android/touch —— 点击/滑动（设备像素坐标；x2,y2 给了就是滑动）。
pub(super) async fn android_touch(Json(req): Json<Value>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut args = json!({ "method": "touch" });
    for k in ["x", "y", "x2", "y2", "duration_ms"] {
        if let Some(v) = req.get(k) {
            args[k] = v.clone();
        }
    }
    via_tool(args).await
}

/// POST /api/android/key —— Android keycode（4=返回 3=HOME 187=最近任务）。
pub(super) async fn android_key(Json(req): Json<Value>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let keycode = req.get("keycode").and_then(Value::as_i64).ok_or_else(|| bad("缺少 keycode".into()))?;
    via_tool(json!({ "method": "key", "keycode": keycode })).await
}

/// POST /api/android/text —— ASCII 文本直送。
pub(super) async fn android_text(Json(req): Json<Value>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let text = req.get("text").and_then(Value::as_str).ok_or_else(|| bad("缺少 text".into()))?;
    via_tool(json!({ "method": "text", "text": text })).await
}

/// POST /api/android/install —— 装 APK（path 相对工作区或绝对路径）。
pub(super) async fn android_install(Json(req): Json<Value>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let path = req.get("path").and_then(Value::as_str).ok_or_else(|| bad("缺少 path".into()))?;
    via_tool(json!({ "method": "install", "path": path })).await
}

/// POST /api/android/launch —— 启动应用（package 包名）。
pub(super) async fn android_launch(Json(req): Json<Value>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let package = req.get("package").and_then(Value::as_str).ok_or_else(|| bad("缺少 package".into()))?;
    let mut args = json!({ "method": "launch", "package": package });
    if let Some(a) = req.get("activity").and_then(Value::as_str) {
        args["activity"] = json!(a);
    }
    via_tool(args).await
}
