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
        compact_fail_streak: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
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

/// 发一个 GET（带合法 Host + 本地访问令牌；不带 Origin = 非浏览器客户端），返回 (状态码, JSON)。
async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .uri(uri)
        .header("host", "127.0.0.1:5173")
        .header("authorization", format!("Bearer {}", auth::current()))
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

/// D1：`/api` 必须带令牌（Bearer 或同源 cookie 二者其一）；静态首页不拦且下发 cookie。
#[tokio::test]
async fn api_requires_token_and_accepts_bearer_or_cookie() {
    let root = test_root("auth");
    let app = app(&root);
    let token = auth::current();

    // 无凭证 → 401（人话错误）
    let req = Request::builder()
        .uri("/api/version")
        .header("host", "127.0.0.1:5173")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "无令牌必须 401");
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("未授权"), "401 文案要是人话: {text}");

    // Bearer → 200
    let req = Request::builder()
        .uri("/api/version")
        .header("host", "127.0.0.1:5173")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.clone().oneshot(req).await.unwrap().status(), StatusCode::OK);

    // 同源 HttpOnly cookie → 200（页面走这条，不手工加头）
    let req = Request::builder()
        .uri("/api/version")
        .header("host", "127.0.0.1:5173")
        .header("cookie", format!("foo=1; pawch_token={token}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.clone().oneshot(req).await.unwrap().status(), StatusCode::OK);

    // 错误令牌 → 401（别把「带了头」当「带对了头」）
    let req = Request::builder()
        .uri("/api/version")
        .header("host", "127.0.0.1:5173")
        .header("authorization", "Bearer deadbeef")
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.clone().oneshot(req).await.unwrap().status(), StatusCode::UNAUTHORIZED);

    // 静态首页（HTML 文档）不拦，并下发 Set-Cookie；页面内的 fetch 靠它过闸
    let req = Request::builder()
        .uri("/")
        .header("host", "127.0.0.1:5173")
        .header("accept", "text/html")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let set_cookie = resp
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(set_cookie.contains("pawch_token="), "HTML 文档要下发令牌 cookie: {set_cookie:?}");
    assert!(set_cookie.contains("HttpOnly"), "cookie 必须 HttpOnly: {set_cookie}");

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn non_loopback_host_is_rejected_with_human_message() {    let root = test_root("guard");
    let app = app(&root);
    let req = Request::builder()
        .uri("/api/version")
        .header("host", "evil.example.com")
        .header("authorization", format!("Bearer {}", auth::current()))
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

/// 发一个 POST（空体 + 合法 Host + 本地访问令牌），返回 (状态码, JSON)。
async fn post(app: &Router, uri: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "127.0.0.1:5173")
        .header("authorization", format!("Bearer {}", auth::current()))
        .body(Body::empty())
        .expect("构造请求");
    let resp = app.clone().oneshot(req).await.expect("router 不应崩溃");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("读响应体").to_bytes();
    let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, v)
}

/// 发一个带 JSON 体的 POST（合法 Host + 本地访问令牌）。
async fn post_json(app: &Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "127.0.0.1:5173")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", auth::current()))
        .body(Body::from(body.to_string()))
        .expect("构造请求");
    let resp = app.clone().oneshot(req).await.expect("router 不应崩溃");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("读响应体").to_bytes();
    let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, v)
}

/// 截断后圆环立即重算（2026-11-07 用户拍板）：回滚/编辑/删除把上下文变小了，
/// 必须立刻按当前请求视图重估并广播——否则圆环挂着旧值，直到下一轮实报回来
/// 「跳变」，用户会把跳变误读成压缩（真机：回滚到压缩边界之前，原文全量回归
/// 约 16.8 万，圆环却还停在压缩完成时的 4.4 万）。
#[tokio::test]
async fn truncate_refreshes_context_usage_ring() {
    let root = test_root("ring-refresh");
    let state = test_state(&root);
    let app = router(state.clone(), root.join("no-dist"));
    let sid = "ring-refresh-1";
    let msgs = vec![
        json!({"id":"u1","role":"user","content":"第一问 ".repeat(200)}),
        json!({"id":"a1","role":"assistant","content":"第一答 ".repeat(200)}),
        json!({"id":"u2","role":"user","content":"第二问 ".repeat(200)}),
        json!({"id":"a2","role":"assistant","content":"第二答 ".repeat(200)}),
    ];
    state.session.save(sid, &msgs, None).expect("写会话");
    // 上一轮圆环快照：实报锚点 + 固定开销（system / 工具定义）——截断后从它带入
    let breakdown = crate::context::Breakdown {
        messages: 40_000,
        tools: 2_000,
        system: 5_000,
        other: 3_000,
        tools_schema: 9_000,
    };
    let mut usage = crate::session::SessionUsage::default();
    usage.apply_context(Some(50_000), 50_000, 200_000, &breakdown);
    state.session.update_usage(sid, usage).expect("写用量快照");

    let mut rx = crate::server::ui_events::test_subscribe();
    let (status, v) = post_json(&app, "/api/sessions/truncate", json!({"id": sid, "mode": "rollback", "user_index": 1})).await;
    assert_eq!(status, StatusCode::OK, "截断应成功: {v}");
    assert_eq!(v["messages"], json!(2), "回滚到第二条用户消息，只剩一问一答: {v}");

    // 全局事件总线可能有别的用例的噪音：只认本会话的 context_usage
    let mut ev = None;
    for _ in 0..10 {
        let Ok(Ok(e)) = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv()).await else { break };
        if e["kind"] == json!("context_usage") && e["session"] == json!(sid) {
            ev = Some(e);
            break;
        }
    }
    let ev = ev.unwrap_or_else(|| panic!("截断后必须广播圆环刷新（context_usage）"));
    let cu = &ev["context_usage"];
    assert!(cu["pressure"].is_null(), "旧实报锚点随被删消息作废: {ev}");
    let projected = cu["projected"].as_u64().unwrap_or(0);
    assert!(projected > 0, "重估结果必须为正: {ev}");
    assert_eq!(cu["window"], json!(200_000), "窗口沿用上一轮快照: {ev}");
    assert_eq!(cu["breakdown"]["system"], json!(5_000), "system 从快照带入: {ev}");
    assert_eq!(cu["breakdown"]["tools_schema"], json!(9_000), "工具定义从快照带入: {ev}");
    // 重估口径：剩下的一问一答（含固定开销）——必须远小于截断前的 50_000 锚点
    assert!(projected < 50_000, "回滚后上下文变小，重估不得高于截断前: {ev}");
    // 校准偏差必须保留：旧快照实报 50_000、本地估价合计 59_000（偏差 -9_000），
    // 新 projected = 新构成合计 - 9_000。直接拿构成合计当 projected 会让删消息后
    // 圆环不降反升（2026-11-07 用户实报）。
    let raw: u64 = ["messages", "tools", "system", "other", "tools_schema"]
        .iter()
        .map(|k| cu["breakdown"][k].as_u64().unwrap_or(0))
        .sum();
    assert_eq!(projected, raw.saturating_sub(9_000), "回滚重估要保留旧锚点的校准偏差: {ev}");

    // 落盘同步：重启/切回会话后圆环也是新值
    let snap = state.session.get(sid).and_then(|f| f.usage).unwrap();
    assert_eq!(snap.projected, projected, "用量快照要与推送一致");
    assert!(snap.pressure.is_none(), "旧锚点作废");

    // 再回滚到空（一条消息不剩）：与新建对话同口径，圆环清零、固定底不再挂着
    let (status, v) = post_json(&app, "/api/sessions/truncate", json!({"id": sid, "mode": "rollback", "user_index": 0})).await;
    assert_eq!(status, StatusCode::OK, "回滚到空应成功: {v}");
    let mut empty_ev = None;
    for _ in 0..10 {
        let Ok(Ok(e)) = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv()).await else { break };
        if e["kind"] == json!("context_usage")
            && e["session"] == json!(sid)
            && e["context_usage"]["projected"] == json!(0)
        {
            empty_ev = Some(e);
            break;
        }
    }
    let ee = empty_ev.unwrap_or_else(|| panic!("回滚到空后必须广播 projected=0 的圆环刷新"));
    assert!(ee["context_usage"]["breakdown"].is_null(), "空会话不留构成: {ee}");
    let snap = state.session.get(sid).and_then(|f| f.usage).unwrap();
    assert_eq!(snap.projected, 0, "空会话圆环清零");
    assert_eq!(
        snap.messages + snap.tools + snap.system + snap.other + snap.tools_schema,
        0,
        "空会话构成清零"
    );
    assert_eq!(snap.window, 200_000, "窗口保留");

    let _ = std::fs::remove_dir_all(&root);
}

/// 删前核对（2026-11-08 防线二）：expect 与文件里第 N 条 user 对不上 → 400 且一条
/// 不删。引导行被展示层合并透明化曾让视图序数整体少 1，真机两次「一次回滚冲掉
/// 两轮问答」——序数类删除必须带内容核对，错了要响不要删完再说。
#[tokio::test]
async fn truncate_expect_guard_refuses_wrong_target() {
    let root = test_root("truncate-expect");
    let state = test_state(&root);
    let app = router(state.clone(), root.join("no-dist"));
    let sid = "truncate-expect-1";
    let msgs = vec![
        json!({"id":"u1","role":"user","content":"第一问"}),
        json!({"id":"s1","role":"user","content":"引导插话","steered":true}),
        json!({"id":"u2","role":"user","content":"第二问"}),
    ];
    state.session.save(sid, &msgs, None).expect("写会话");

    // 视图错位（把第 1 条当成第 2 条）→ 拒绝，文件一条不动
    let (status, v) = post_json(
        &app,
        "/api/sessions/truncate",
        json!({"id": sid, "mode": "rollback", "user_index": 1, "expect": "第二问"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "序数与内容对不上必须拒绝: {v}");
    let f = state.session.get(sid).expect("会话还在");
    assert_eq!(f.messages.len(), 3, "拒绝 = 没删任何消息: {}", f.messages.len());

    // 序数与内容都对 → 放行（delete_user 只删第 N 条 user 本身）
    let (status, v) = post_json(
        &app,
        "/api/sessions/truncate",
        json!({"id": sid, "mode": "delete_user", "user_index": 1, "expect": "引导插话"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "内容对得上要放行: {v}");
    let f = state.session.get(sid).expect("会话还在");
    assert_eq!(f.messages.len(), 2, "只删了引导那条: {}", f.messages.len());

    // 不带 expect（旧客户端/兼容路径）→ 照旧放行
    let (status, _) = post_json(&app, "/api/sessions/truncate", json!({"id": sid, "mode": "delete_user", "user_index": 1})).await;
    assert_eq!(status, StatusCode::OK, "不带 expect 走原有语义");

    let _ = std::fs::remove_dir_all(&root);
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

        // first = 该会话最早的检查点（产物面板基线语义，2026-10-31 实测：危险工具快照
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

/// HTML 文档必须 no-store + 下发同源 cookie。否则 WebView 恢复会话时直接用缓存页，
/// 永远拿不到 cookie，所有 /api 401，界面显示「暂无会话/未选模型」像数据全丢
/// （2026-11-06 桌面端实锤）。
#[tokio::test]
async fn html_document_gets_no_store_and_auth_cookie() {
    let root = test_root("cookie");
    let req = Request::builder()
        .uri("/")
        .header("host", "127.0.0.1:5173")
        .header("accept", "text/html")
        .body(Body::empty())
        .expect("构造请求");
    let resp = app(&root).oneshot(req).await.expect("router 不应崩溃");
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get("cache-control").and_then(|v| v.to_str().ok()),
        Some("no-store"),
        "文档必须禁止缓存"
    );
    let cookie = resp
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(cookie.contains("pawch_token="), "首屏要下发访问令牌 cookie: {cookie}");
    let _ = std::fs::remove_dir_all(&root);
}
