//! 前端错误收集（2026-11-13）：页面里的未捕获异常与 Promise 拒绝落 JSONL。
//!
//! 前端 `window.onerror` / `unhandledrejection` 把错误 POST 到这里，落一条
//! `event_type=error / kind=client`。只收诊断字段：消息、栈、文件、行列、页面 URL。
//! 消息/栈做长度截断——一条巨大的栈把日志轮转吃掉是踩过的坑（gateway.request
//! 曾占日志 99.6%）。logger 未初始化时走 try_event 静默跳过，不 panic。

use super::*;

/// 前端上报的单条错误。
#[derive(Debug, Deserialize)]
pub(crate) struct ClientLog {
    /// 前端事件类型："error" | "unhandledrejection"
    #[serde(default)]
    pub kind: String,
    /// 级别："error"（默认）| "warn"
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub stack: String,
    /// 出错脚本文件
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub line: u32,
    #[serde(default)]
    pub col: u32,
    /// 出错页面地址
    #[serde(default)]
    pub url: String,
}

/// 消息与栈的截断上限（字符）。
const MESSAGE_MAX: usize = 2000;
const STACK_MAX: usize = 4000;
/// 文件名/页面地址截断上限（字符）。
const LOCATION_MAX: usize = 512;

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// 组装落盘 payload（纯函数，便于单测）。
pub(crate) fn client_event_payload(p: &ClientLog) -> (EventType, Value) {
    let level = if p.level.is_empty() { "error" } else { p.level.as_str() };
    let event_type = if level == "error" || level == "fatal" {
        EventType::Error
    } else {
        EventType::State
    };
    let payload = json!({
        "kind": "client",
        "source": if p.kind.is_empty() { "unknown" } else { p.kind.as_str() },
        "level": level,
        "message": truncate(&p.message, MESSAGE_MAX),
        "stack": truncate(&p.stack, STACK_MAX),
        "file": truncate(&p.file, LOCATION_MAX),
        "line": p.line,
        "col": p.col,
        "url": truncate(&p.url, LOCATION_MAX),
    });
    (event_type, payload)
}

/// POST /api/client-log —— 前端错误上报（受本地令牌/同源 cookie 保护）。
pub(crate) async fn post_client_log(Json(p): Json<ClientLog>) -> StatusCode {
    let (event_type, payload) = client_event_payload(&p);
    observability::try_event(event_type, payload);
    StatusCode::NO_CONTENT
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(kind: &str, level: &str) -> ClientLog {
        ClientLog {
            kind: kind.into(),
            level: level.into(),
            message: "boom".into(),
            stack: "at fn (app.js:1:1)".into(),
            file: "app.js".into(),
            line: 1,
            col: 1,
            url: "http://127.0.0.1:5173/".into(),
        }
    }

    #[test]
    fn payload_truncates_long_fields() {
        let mut p = sample("error", "error");
        p.message = "m".repeat(5000);
        p.stack = "s".repeat(9000);
        p.file = "x".repeat(900);
        p.url = "u".repeat(900);
        let (ty, v) = client_event_payload(&p);
        assert_eq!(ty, EventType::Error);
        assert_eq!(v["kind"], "client");
        assert_eq!(v["message"].as_str().unwrap().chars().count(), MESSAGE_MAX);
        assert_eq!(v["stack"].as_str().unwrap().chars().count(), STACK_MAX);
        assert_eq!(v["file"].as_str().unwrap().chars().count(), LOCATION_MAX);
        assert_eq!(v["url"].as_str().unwrap().chars().count(), LOCATION_MAX);
    }

    #[test]
    fn payload_defaults_level_to_error() {
        let p = sample("unhandledrejection", "");
        let (ty, v) = client_event_payload(&p);
        assert_eq!(ty, EventType::Error);
        assert_eq!(v["source"], "unhandledrejection");
        assert_eq!(v["level"], "error");
    }

    #[test]
    fn warn_level_uses_state_event() {
        let p = sample("error", "warn");
        let (ty, _) = client_event_payload(&p);
        assert_eq!(ty, EventType::State);
    }
}
