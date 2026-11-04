//! HTTP 层契约测试（2026-10-30）。
//!
//! 本仓是 **bin-only crate**（没有 lib.rs），`tests/*.rs` 集成测试够不到内部模块，
//! 所以这些用例放在 crate 内：用 `tower::ServiceExt::oneshot` 直接打 `server::router`，
//! 不起真服务器、不碰真端口。
//!
//! 纪律（ROADMAP 坑 19/23）：全局单例多（LEDGER / 插件目录 / 工作区），断言只用
//! 本用例唯一的临时目录，不依赖别的用例留下的干净态。

use super::*;

use axum::body::Body;
use http_body_util::BodyExt;
use tower::ServiceExt;

/// 每个用例一个隔离的应用根（会话/记忆/日志都写这里）。
fn test_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("agent-http-{}-{}", tag, uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 构造一个最小可用的 AppState（全部默认配置 + 临时目录）。
fn test_state(root: &std::path::Path) -> AppState {
    // logger 是全局 OnceLock：router 的「未构建前端」分支会调 observability::debug，
    // 未初始化会 panic。先装一个（已被别的用例占用也无妨）。
    let _ = observability::init(&root.join("logs"), "agent.jsonl", format!("http-{}", uuid::Uuid::new_v4()));
    let mem_cfg = Arc::new(crate::config::MemoryConfig::default());
    let memory = Arc::new(std::sync::RwLock::new(Arc::new(crate::memory::Memory::open(root, &mem_cfg))));
    AppState {
        gateway: Arc::new(Gateway::new()),
        cancel: Arc::new(CancelRegistry::new()),
        steer: Arc::new(crate::core::steer::SteerQueue::new()),
        settings: Arc::new(std::sync::RwLock::new(Settings::default())),
        settings_write: Arc::new(std::sync::Mutex::new(())),
        agent_cfg: Arc::new(crate::config::AgentConfig::default()),
        sandbox_cfg: Arc::new(crate::config::SandboxConfig::default()),
        context_cfg: Arc::new(crate::config::ContextConfig::default()),
        recovery_cfg: Arc::new(crate::config::RecoveryConfig::default()),
        system_prompt: Arc::new("测试系统提示".into()),
        prompt_path: Arc::new(root.join("system.md")),
        session: Arc::new(crate::session::SessionManager::new(root)),
        log_dir: Arc::new(root.to_path_buf()),
        memory,
        mem_cfg,
        approvals: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        approval_meta: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        approved_scopes: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        approval_mode: Arc::new(std::sync::RwLock::new("auto".into())),
        session_approval: Arc::new(std::sync::RwLock::new(std::collections::HashMap::new())),
        roles_cfg: Arc::new(crate::config::RolesConfig::default()),
        gateway_cfg: Arc::new(crate::config::GatewayConfig::default()),
        subagent_cfg: Arc::new(crate::config::SubagentConfig::default()),
    }
}

/// 发一个 GET（带合法 Host；不带 Origin = 非浏览器客户端），返回 (状态码, JSON)。
async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .uri(uri)
        .header("host", "127.0.0.1:5173")
        .body(Body::empty())
        .expect("构造请求");
    let resp = app.clone().oneshot(req).await.expect("router 不应崩溃");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("读响应体").to_bytes();
    let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, v)
}

fn app(root: &std::path::Path) -> Router {
    // 指向一个不存在的 dist：router 走内置冒烟页分支，正好避开静态文件
    router(test_state(root), root.join("no-dist"))
}

#[tokio::test]
async fn version_route_has_expected_shape() {
    let root = test_root("version");
    let (status, v) = get(&app(&root), "/api/version").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["name"], json!(env!("CARGO_PKG_NAME")));
    assert_eq!(v["version"], json!(env!("CARGO_PKG_VERSION")));
    assert!(v["root"].is_string(), "root 必须是字符串: {v}");
    assert!(v["exe"].is_string(), "exe 必须是字符串: {v}");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn sessions_list_and_missing_session_are_stable() {
    let root = test_root("sessions");
    let app = app(&root);
    let (status, v) = get(&app, "/api/sessions").await;
    assert_eq!(status, StatusCode::OK, "会话列表: {v}");
    assert!(v["project"].is_string(), "project 应为字符串: {v}");
    assert!(v["sessions"].is_array(), "sessions 应为数组: {v}");
    // 不存在的会话：契约是 200 + 空消息，而不是 500/404（前端靠它决定「新对话」）
    let (status, v) = get(&app, "/api/session?id=definitely-not-exist").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["messages"], json!([]), "不存在的会话应回空消息: {v}");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn usage_report_has_totals_and_empty_ledger_is_ok() {
    let root = test_root("usage");
    let (status, v) = get(&app(&root), "/api/usage?group=model&range=all").await;
    assert_eq!(status, StatusCode::OK, "用量报表: {v}");
    assert_eq!(v["group"], json!("model"));
    assert!(v["rows"].is_array(), "rows 应为数组: {v}");
    assert!(v["totals"].is_object(), "totals 应为对象: {v}");
    assert!(v["totals"]["cost_usd"].is_number(), "total cost 应为数字: {v}");
    assert!(v["spend_total_usd"].is_number(), "累计花费应为数字: {v}");
    assert!(v["priced"].is_boolean(), "priced 应为布尔: {v}");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn plugins_and_browser_state_routes_are_alive() {
    let root = test_root("plugins");
    let app = app(&root);
    let (status, v) = get(&app, "/api/plugins").await;
    assert_eq!(status, StatusCode::OK, "插件清单: {v}");
    assert!(v["plugins"].is_array(), "plugins 应为数组: {v}");
    assert!(v["dir"].is_string(), "dir 应为字符串: {v}");
    // 没有浏览器会话时必须是「running:false + 空 tabs」，且不能被能力闸误拦
    let (status, v) = get(&app, "/api/browser/state").await;
    assert_eq!(status, StatusCode::OK, "浏览器状态: {v}");
    assert_eq!(v["running"], json!(false));
    assert!(v["tabs"].is_array());
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn non_loopback_host_is_rejected_with_human_message() {
    let root = test_root("guard");
    let app = app(&root);
    let req = Request::builder()
        .uri("/api/version")
        .header("host", "evil.example.com")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.expect("router 不应崩溃");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("本机来源"), "403 文案要是人话: {text}");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn unknown_api_route_is_404_not_html() {
    let root = test_root("404");
    let (status, _v) = get(&app(&root), "/api/definitely-not-a-route").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "未知 API 必须是 404，不能掉进 SPA 兜底");
    let _ = std::fs::remove_dir_all(&root);
}

/// 发一个 POST（空体 + 合法 Host），返回 (状态码, JSON)。
async fn post(app: &Router, uri: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "127.0.0.1:5173")
        .body(Body::empty())
        .expect("构造请求");
    let resp = app.clone().oneshot(req).await.expect("router 不应崩溃");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("读响应体").to_bytes();
    let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, v)
}

/// POST /api/checkpoints（手动重拍基线）契约：非 git 工作区 → 200 + ok:false + 人话 note
/// （拍不了基线不是错误）；git 仓库（有已跟踪文件）→ ok:true + 检查点归属带上的会话。
/// 产物/git 面板「基线过期 → 重新以当前状态为起点」走的就是这个端点（2026-10-31）。
#[tokio::test]
async fn manual_checkpoint_endpoint_contract() {
    let root = test_root("manual-cp");
    let ws = root.join("ws");
    std::fs::create_dir_all(&ws).expect("建工作区");
    let prev = crate::tools::workspace_root();
    crate::tools::set_workspace_root(&ws);

    // 非 git 且空的起步工作区：第一拍 = 空树基线（2026-10-31 新语义——空起步也要有
    // 任务起点，之后产出的一切相对它都是新增；「拍不了基线」只剩 git 本身坏掉时）
    let (status, v) = post(&app(&root), "/api/checkpoints?session=s-manual").await;
    assert_eq!(status, StatusCode::OK, "非 git 也应 200: {v}");
    assert_eq!(v["ok"], Value::Bool(true), "空起步要有空树基线: {v}");
    let id_empty = v["checkpoint"]["id"].clone();

    // git 仓库（有已跟踪文件）→ ok:true，检查点归属带上的会话
    if let Ok(bin) = crate::tools::git_exe() {
        let git = |args: &[&str]| {
            std::process::Command::new(&bin)
                .args(args)
                .current_dir(&ws)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
        };
        let _ = git(&["init", "--quiet"]);
        std::fs::write(ws.join("a.txt"), "v1\n").expect("写文件");
        let _ = git(&["add", "a.txt"]);
        let _ = git(&["commit", "--quiet", "-m", "init"]);
        let (status2, v2) = post(&app(&root), "/api/checkpoints?session=s-manual").await;
        assert_eq!(status2, StatusCode::OK, "{v2}");
        assert_eq!(v2["ok"], Value::Bool(true), "{v2}");
        assert!(!v2["checkpoint"]["id"].as_str().unwrap_or("").is_empty(), "{v2}");
        assert_eq!(v2["checkpoint"]["session"], Value::String("s-manual".into()), "{v2}");

        // 无变更再拍：checkpoint 的 None 语义——不算失败，直接以最近检查点为起点
        let (status3, v3) = post(&app(&root), "/api/checkpoints?session=s-manual").await;
        assert_eq!(status3, StatusCode::OK, "{v3}");
        assert_eq!(v3["ok"], Value::Bool(true), "{v3}");
        assert_eq!(v3["checkpoint"]["id"], v2["checkpoint"]["id"], "无变更应回指最近检查点: {v3}");
        assert!(v3["note"].as_str().map(|s| !s.is_empty()).unwrap_or(false), "无变更要给人话: {v3}");

        // first = 该会话最早的检查点（产物面板基线语义，2026-10-31 实锤：危险工具快照
        // 会把列表刷爆，产物面板拿「最新」当基线=会话早先的产物全部隐身）
        std::fs::write(ws.join("a.txt"), "v2\n").expect("改文件");
        let (_s4, v4) = post(&app(&root), "/api/checkpoints?session=s-manual").await;
        assert_eq!(v4["ok"], Value::Bool(true), "{v4}");
        let (gs, gv) = get(&app(&root), "/api/checkpoints?session=s-manual").await;
        assert_eq!(gs, StatusCode::OK);
        assert_eq!(gv["first"]["id"], id_empty, "first 应为该会话最早的检查点（空起步基线）: {gv}");
    }

    crate::tools::set_workspace_root(&prev);
    let _ = std::fs::remove_dir_all(&root);
}
