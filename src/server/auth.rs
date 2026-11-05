//! 本地 API 访问令牌（2026-11-05 审计修复 D1）。
//!
//! 服务只绑 `127.0.0.1`，但回环不等于可信：同机任何进程都能打 `/api`，恶意页面也能借
//! DNS rebinding 之外的口子（比如本机另一个端口的页面）碰上没有 Origin 的请求路径。
//! 这里加一道进程间凭证：
//!
//! - 首次启动生成 32 字节随机 token，写 `<数据目录>/server.token`（ACL 仅当前用户可读）；
//! - 页面的请求带同源 HttpOnly cookie（`GET /` 等 HTML 文档响应时下发），无需前端改动；
//! - CLI（`run`）、eval、服务自管理等非浏览器客户端读 token 文件加
//!   `Authorization: Bearer <token>`；
//! - 静态资源与插件 iframe 文档不校验（页面首次加载时还没 cookie，且它们不是数据面）。
//!
//! 这层**不是**为了防「已经能以你的账户运行的程序」——那种程序可以直接读 token 文件。
//! 它挡的是「本机另一个应用/页面顺手打进来」和未来可能出现的端口暴露事故。

use std::path::Path;

use axum::{
    extract::Request,
    http::{
        HeaderValue, Method, StatusCode,
        header::{ACCEPT, AUTHORIZATION, COOKIE, SET_COOKIE},
    },
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// 令牌文件名（落在数据目录下）。
pub const TOKEN_FILE: &str = "server.token";
/// 页面同源 cookie 名。
const COOKIE_NAME: &str = "pawch_token";

static TOKEN: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// 启动时调用一次：读 token 文件，没有就生成并写盘。
pub fn init(data_root: &Path) -> String {
    let path = data_root.join(TOKEN_FILE);
    let token = match std::fs::read_to_string(&path) {
        Ok(t) if valid_token(t.trim()) => t.trim().to_string(),
        _ => {
            let fresh = random_token();
            if let Err(e) = std::fs::write(&path, &fresh) {
                eprintln!("[pawch-agent] 写访问令牌失败（{}）：{e}", path.display());
            } else {
                restrict_to_current_user(&path);
            }
            fresh
        }
    };
    let _ = TOKEN.set(token.clone());
    token
}

/// 当前进程的令牌。未初始化（单测直接打 router）时给一个进程内临时 token——
/// 测试用它拼 Authorization 头，不落盘、不污染用户目录。
pub fn current() -> String {
    TOKEN.get_or_init(random_token).clone()
}

/// 非浏览器客户端（CLI / eval）：读数据目录里的 token 文件。
pub fn token_from_file() -> Option<String> {
    let path = crate::config::data_root().join(TOKEN_FILE);
    let text = std::fs::read_to_string(path).ok()?;
    let t = text.trim().to_string();
    valid_token(&t).then_some(t)
}

fn valid_token(t: &str) -> bool {
    t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit())
}

fn random_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Windows ACL：断开继承，只留当前用户。失败不致命（数据目录通常本来就在用户目录下），
/// 但要让日志里看得见，别静默变成「谁都能读」。
#[cfg(windows)]
fn restrict_to_current_user(path: &Path) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let Some(user) = std::env::var("USERNAME").ok().filter(|u| !u.is_empty()) else {
        eprintln!("[pawch-agent] 无法确定当前用户名，跳过 {} 的 ACL 收紧", path.display());
        return;
    };
    let grant = format!("{user}:F");
    match std::process::Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r", "/grant:r", &grant])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(o) if o.status.success() => {}
        Ok(o) => eprintln!(
            "[pawch-agent] 收紧 {} 的 ACL 失败：{}",
            path.display(),
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => eprintln!("[pawch-agent] 调用 icacls 失败：{e}"),
    }
}

#[cfg(not(windows))]
fn restrict_to_current_user(_path: &Path) {}

/// 需要令牌的路径：数据面 `/api/*` 与取消动作 `/cancel`。
/// 静态页、插件 iframe 文档、`/plugins/*` 资源不在此列。
fn needs_token(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/") || path == "/cancel"
}

fn authorized(req: &Request) -> bool {
    let expected = current();
    if let Some(v) = req.headers().get(AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        let v = v.trim();
        let bearer = v
            .strip_prefix("Bearer ")
            .or_else(|| v.strip_prefix("bearer "));
        if let Some(t) = bearer
            && t.trim() == expected
        {
            return true;
        }
    }
    if let Some(cookies) = req.headers().get(COOKIE).and_then(|v| v.to_str().ok()) {
        for part in cookies.split(';') {
            if let Some((name, value)) = part.trim().split_once('=')
                && name == COOKIE_NAME
                && value.trim() == expected
            {
                return true;
            }
        }
    }
    false
}

/// 令牌闸：`/api/*` 与 `/cancel` 要么带 Bearer，要么带同源 cookie，否则 401。
pub async fn guard(req: Request, next: Next) -> Response {
    if needs_token(req.uri().path()) && !authorized(&req) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "error": "未授权：缺少本地访问令牌。页面会自动携带 cookie；命令行请读数据目录下的 server.token 并加 Authorization: Bearer <token>"
            })),
        )
            .into_response();
    }
    next.run(req).await
}

/// HTML 文档响应下发/刷新同源 cookie（`HttpOnly; SameSite=Strict`）。
/// 只看 Accept 带 text/html 的 GET——浏览器导航会带，fetch/资源请求不会。
pub async fn issue_cookie(req: Request, next: Next) -> Response {
    let is_document = req.method() == Method::GET
        && req
            .headers()
            .get(ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|a| a.contains("text/html"));
    let mut resp = next.run(req).await;
    if is_document && resp.status().is_success() {
        let cookie = format!("{COOKIE_NAME}={}; HttpOnly; SameSite=Strict; Path=/", current());
        if let Ok(v) = HeaderValue::from_str(&cookie) {
            resp.headers_mut().append(SET_COOKIE, v);
        }
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_shape_and_auth_paths() {
        assert!(valid_token(&"a".repeat(64)));
        assert!(!valid_token("short"));
        assert!(!valid_token(&"g".repeat(64)), "非 hex 不算合法 token");
        assert!(needs_token("/api/chat"));
        assert!(needs_token("/api"));
        assert!(needs_token("/cancel"));
        assert!(!needs_token("/"));
        assert!(!needs_token("/plugins/board/index.html"));
        assert!(!needs_token("/assets/index-abc.js"));
    }

    #[test]
    fn generated_token_is_64_hex() {
        let t = random_token();
        assert!(valid_token(&t), "{t}");
        assert_ne!(t, random_token(), "两次生成不能相同");
    }
}
