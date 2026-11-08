//! 会话列表/元信息/分叉/批量/打开/清空/HANDOFF 端点（自 server.rs 按域拆出）

use super::*;
use super::workspace::{apply_workspace, canon_workspace, same_workspace};

/// GET /api/sessions —— 会话列表（新→旧）+ 项目名（多会话）。
/// `?scope=workspace` 只返回当前工作区的会话（composer @ 上下文「选对话加入」用；
/// 工作区比对与时间线同一把尺子 same_workspace，别在前端拿裸字符串比）。
/// 额外带 `workspace_born`（各工作区目录的创建时间，毫秒）：会话归属按路径字符串，
/// 同名文件夹删掉再建时旧会话会被一起翻出来（清掉再建后旧对话会凭空出现）——
/// 前端用「会话创建时间 < 目录创建时间」把它们标成历史会话。
pub(super) async fn list_sessions(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Json<Value> {
    let project = crate::tools::workspace_root()
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let scope_workspace = q.get("scope").map(|s| s == "workspace").unwrap_or(false);
    let list = if scope_workspace {
        let cur = crate::tools::workspace_root().display().to_string();
        state
            .session
            .list()
            .into_iter()
            .filter(|m| same_workspace(&m.workspace, &cur))
            .collect::<Vec<_>>()
    } else {
        state.session.list()
    };
    let mut born = serde_json::Map::new();
    let mut seen = std::collections::HashSet::new();
    for m in &list {
        let ws = m.workspace.trim();
        if ws.is_empty() || !seen.insert(ws.to_string()) {
            continue;
        }
        if let Some(ms) = dir_created_ms(ws) {
            born.insert(ws.to_string(), json!(ms));
        }
    }
    Json(json!({ "project": project, "sessions": list, "workspace_born": born }))
}

/// GET /api/running —— 正在跑的会话 id 列表（侧栏会话行转圈圈）。
/// 前端 3 秒轮询一次；本体只是注册表 keys 快照，不碰会话文件。
pub(super) async fn running_sessions(State(state): State<AppState>) -> Json<Value> {
    Json(json!({ "running": state.cancel.running_ids() }))
}

/// 目录创建时间（毫秒）。取不到（不存在/权限/文件系统不支持）返回 None——调用方
/// 当作「不知道」，不做历史判定，宁可多显示也不误藏。
fn dir_created_ms(p: &str) -> Option<i64> {
    let meta = std::fs::metadata(p).ok()?;
    let t = meta.created().ok()?;
    let dt: chrono::DateTime<chrono::Utc> = t.into();
    Some(dt.timestamp_millis())
}

/// GET /api/sessions/transcript?id=xx —— 只读转写（composer @ 上下文「选对话加入」用）。
/// 不切当前会话、不带工具事件，只取 user/assistant 正文；总量超预算从**最早**的消息
/// 开始丢（保尾部——引用对话时最近的往来信息量最大），丢了就带 omitted=true。
pub(super) async fn session_transcript(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    let Some(id) = q.get("id").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "缺少 id" })));
    };
    let Some(f) = state.session.get(&id) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "会话不存在" })));
    };
    let (kept, omitted) = crate::session::transcript(&f, 16_000, 2_500);
    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "id": f.id,
            "title": f.title,
            "updated_at": f.updated_at,
            "omitted": omitted,
            "messages": kept.iter().map(|(r, t)| json!({ "role": r, "content": t })).collect::<Vec<_>>(),
        })),
    )
}

#[derive(Debug, Deserialize)]
pub struct SessionMetaUpdate {
    pub id: String,
    /// Some=重命名会话标题
    #[serde(default)]
    pub title: Option<String>,
    /// Some=置顶/取消置顶
    #[serde(default)]
    pub pinned: Option<bool>,
}

/// POST /api/sessions/meta —— 侧边栏：置顶/取消置顶、重命名。
/// 归档已移除（2026-10-31）：会话只有删除，旧 archived 字段仅兼容旧文件读取。
pub(super) async fn session_meta(
    State(state): State<AppState>,
    Json(req): Json<SessionMetaUpdate>,
) -> (StatusCode, Json<Value>) {
    let sess = state.session.clone();
    let res = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        if let Some(t) = &req.title {
            sess.rename(&req.id, t.trim())?;
        }
        if req.pinned.is_some() {
            sess.set_flags(&req.id, req.pinned)?;
        }
        Ok(())
    })
    .await;
    match res {
        Ok(Ok(())) => (StatusCode::OK, Json(json!({ "ok": true }))),
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

#[derive(Debug, Deserialize)]
pub struct ForkRequest {
    pub id: String,
}

/// POST /api/sessions/fork —— DSH 式「分叉会话」：复制消息/摘要/工作区为新会话。
pub(super) async fn fork_session(
    State(state): State<AppState>,
    Json(req): Json<ForkRequest>,
) -> (StatusCode, Json<Value>) {
    let sess = state.session.clone();
    let id = req.id.clone();
    match tokio::task::spawn_blocking(move || sess.fork(&id)).await {
        Ok(Ok(new_id)) => (StatusCode::OK, Json(json!({ "ok": true, "id": new_id }))),
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

/// GET /api/session?id=xxx —— 恢复指定会话；缺省返回最近更新的一条。
pub(super) async fn get_session(State(state): State<AppState>, axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>) -> (StatusCode, Json<Value>) {
    let f = match q.get("id") {
        Some(id) => state.session.get(id),
        None => state.session.latest(),
    };
    match f {
        Some(f) => (StatusCode::OK, Json(json!({ "id": f.id, "title": f.title, "updated_at": f.updated_at, "summary": f.summary, "workspace": f.workspace, "messages": f.messages }))),
        None => (StatusCode::OK, Json(json!({ "messages": [] }))),
    }
}

#[derive(Debug, Deserialize)]
pub struct SessionOpenRequest {
    pub id: String,
}

/// POST /api/sessions/new —— 新建一条空会话并立即落盘（当前项目下），
/// 侧栏马上能见到该会话；首条消息随后写入同一条。同工作区已有空会话则复用。
pub(super) async fn new_session(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    let ws = crate::tools::workspace_root().display().to_string();
    match state.session.create_empty(&ws, crate::session::PLACEHOLDER_TITLE) {
        Ok(f) => {
            observability::event(
                EventType::State,
                json!({ "kind": "session.created", "session": f.id, "workspace": ws }),
            );
            (
                StatusCode::OK,
                Json(json!({ "ok": true, "id": f.id, "title": f.title, "workspace": ws, "created_at": f.created_at })),
            )
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

/// POST /api/sessions/open —— 打开会话：其所属工作区与当前不同时先切过去，再返回消息。
/// 「切会话 = 回到那个项目」的闭环入口；目录已不存在时不硬失败，降级只读打开并回 warning。
pub(super) async fn open_session(
    State(state): State<AppState>,
    Json(req): Json<SessionOpenRequest>,
) -> (StatusCode, Json<Value>) {
    let Some(f) = state.session.get(&req.id) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "会话不存在" })));
    };
    let current = crate::tools::workspace_root().display().to_string();
    let mut switched = false;
    let mut warning: Option<String> = None;
    let target = f.workspace.trim();
    if !target.is_empty() && !same_workspace(&current, target) {
        let p = std::path::PathBuf::from(target);
        if p.is_dir() {
            let canon = canon_workspace(p);
            match apply_workspace(&state, &canon) {
                Ok(()) => switched = true,
                Err(e) => warning = Some(format!("切换项目失败：{e}")),
            }
        } else {
            warning = Some(format!("原项目目录已不存在，仍在当前项目里打开该会话：{target}"));
        }
    }
    (
        StatusCode::OK,
        Json(json!({
            "id": f.id,
            "title": f.title,
            "updated_at": f.updated_at,
            "summary": f.summary,
            "messages": f.messages,
            "workspace": f.workspace,
            // 用量快照：重启/重开后恢复圆环与缓存行（无快照的旧会话为 null，前端按零兜底）
            "usage": f.usage,
            "current_workspace": crate::tools::workspace_root().display().to_string(),
            "switched": switched,
            "warning": warning,
        })),
    )
}

/// DELETE /api/session?id=xxx —— 删除一条会话（其余不受影响）。
pub(super) async fn clear_session(State(state): State<AppState>, axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>) -> (StatusCode, Json<Value>) {
    let Some(id) = q.get("id") else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "缺少 id" })));
    };
    match state.session.delete(id) {
        Ok(()) => {
            // A3：删会话即撤销其「会话内同类允许」记忆（仅内存，从不落盘）
            if let Ok(mut m) = state.approved_scopes.lock() {
                m.remove(id);
            }
            // 2026-10-08 修：档位表也只插不删——每个历史会话永久留一条，
            // 长期运行无上限增长。删会话时一并收掉（session 字段不校验存在性，
            // 直接 curl 循环打 /api/approval-mode 也能灌，这里至少让正常路径闭合）。
            if let Ok(mut m) = state.session_approval.write() {
                m.remove(id);
            }
            observability::event(EventType::State, json!({ "kind": "session.cleared", "session": id }));
            (StatusCode::OK, Json(json!({ "ok": true })))
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

/// POST /api/sessions/truncate —— 会话截断（2026-10-26 消息操作三件套共用）。
/// body: { id, user_index, mode, restore_checkpoint? }；user_index = 文件里第 N 条
/// user 消息（0 基，前端按自己的用户气泡序数数——排队气泡不在文件里，两边天然对齐）：
/// - `rollback`：删第 N 条 user 及其后全部（回到它之前；编辑重发也走它）。
///   带 restore_checkpoint（任务起点基线 id，核心记在消息的 cp 字段上）时**先恢复
///   工作区文件再截对话**——回滚=文件+对话一起，只删消息等于删除。
/// - `delete_turn`：删第 N 条 user + 紧随的 assistant/tool 串（这一问答对），
///   更早与更晚的历史原样保留（纯对话整理，不动文件）
/// - `delete_user`：只删第 N 条 user（单条删除不连带回答；
///   留在句首的孤儿 assistant 由适配器剥除）
/// - `delete_selected`：按 body.targets=[{role,content}] 精确删除勾选的消息——
///   多选用（「勾了谁就删谁，不要连带彼此」）；user_index 忽略
///
/// 会话在跑时拒绝（409）：跑完的 Snapshot 会整份覆写，截了也白截——先停再截。
/// 会话消息的纯文本（与前端 contentText 同口径：字符串原样；内容块数组拼 text 块）。
/// 删前核对（expect）用。
fn session_msg_text(m: &Value) -> String {
    match m.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<String>(),
        _ => String::new(),
    }
}

pub(super) async fn truncate_session(
    State(state): State<AppState>,
    Json(req): Json<serde_json::Value>,
) -> (StatusCode, Json<Value>) {
    let Some(id) = req.get("id").and_then(|v| v.as_str()) else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "缺少 id" })));
    };
    let mode0 = req.get("mode").and_then(|v| v.as_str()).unwrap_or("rollback");
    let user_index: usize = match req.get("user_index").and_then(|v| v.as_u64()) {
        Some(v) => v as usize,
        // delete_selected 按 targets 删，不需要 user 序数
        None if mode0 == "delete_selected" => 0,
        None => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": "缺少 user_index" })));
        }
    };
    let mode = req.get("mode").and_then(|v| v.as_str()).unwrap_or("rollback");
    if !matches!(mode, "rollback" | "delete_turn" | "delete_user" | "delete_selected") {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("未知 mode: {mode}") })));
    }
    // delete_selected：按 (role, content) 精确删除勾选的消息（不连带问答对另一方，
    // 仅被删 assistant 的 tool 结果消息结构性跟随——否则下轮请求带悬空 tool_result 必 400）
    let targets: Vec<(String, String)> = if mode == "delete_selected" {
        match req.get("targets").and_then(|v| v.as_array()) {
            Some(arr) if !arr.is_empty() => arr
                .iter()
                .map(|t| {
                    Ok((
                        t.get("role").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                        t.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    ))
                })
                .collect::<Result<Vec<_>, ()>>()
                .unwrap_or_default(),
            _ => {
                return (StatusCode::BAD_REQUEST, Json(json!({ "error": "delete_selected 需要 targets" })));
            }
        }
    } else {
        Vec::new()
    };
    let restore_cp = req
        .get("restore_checkpoint")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if !restore_cp.is_empty()
        && (restore_cp.len() > 64 || !restore_cp.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
    {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "检查点 id 形状不对" })));
    }
    // 占位探测：拿得到标志=空闲（立刻还回去）；拿不到=有任务在跑
    match state.cancel.try_begin(id) {
        None => {
            return (StatusCode::CONFLICT, Json(json!({ "error": "这个会话正在跑，先停止再操作" })));
        }
        Some(flag) => state.cancel.end(id, &flag),
    }
    // 删前核对（2026-11-08 防线二）：前端按视图序数发 N，视图与文件的 user 行一旦
    // 错位就会静默删错段（引导行被展示层合并透明化 → 序数少 1，真机两次「一次回滚
    // 冲掉两轮问答」）。expect = 目标 user 消息的内容前缀——第 N 条对不上就拒绝，
    // 错要「响」，不能删完再说。放在文件恢复之前：校验不过连文件都不许动。
    let expect = req.get("expect").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if !expect.is_empty() && mode != "delete_selected" {
        let snap = state.session.get(id).map(|f| f.messages).unwrap_or_default();
        let actual = snap
            .iter()
            .filter(|m| m.get("role").and_then(Value::as_str) == Some("user"))
            .nth(user_index)
            .map(session_msg_text);
        let ok = matches!(&actual, Some(t) if t.trim().starts_with(&expect));
        if !ok {
            observability::event(
                EventType::State,
                json!({
                    "kind": "session.truncate_refused", "session": id, "mode": mode, "user_index": user_index,
                    "expect": expect.chars().take(40).collect::<String>(),
                    "actual": actual.unwrap_or_default().chars().take(40).collect::<String>(),
                }),
            );
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "页面的消息序号和会话文件对不上（页面数据可能过期），没有删除任何内容。请刷新页面后再试一次。" })),
            );
        }
    }
    // 先恢复文件再截对话：文件恢复失败就整个不动（半回滚比不回滚更坑）
    if !restore_cp.is_empty() {
        let ws = crate::tools::workspace_root().to_path_buf();
        let cp = restore_cp.clone();
        let restored = tokio::task::spawn_blocking(move || crate::recovery::restore(&ws, &cp)).await;
        match restored {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("文件恢复失败: {e}") })));
            }
            Err(e) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("文件恢复任务失败: {e}") })));
            }
        }
    }
    if mode == "delete_selected" {
        return match state.session.delete_messages(id, &targets) {
            Ok(n) => {
                observability::event(
                    EventType::State,
                    json!({ "kind": "session.messages_deleted", "session": id, "deleted": n }),
                );
                // 删完消息上下文就变小了：圆环必须立刻按新视图重算（否则挂着旧值
                // 直到下一轮实报跳变——会被误读成压缩，2026-11-07 真机）
                refresh_ring_after_truncate(&state, id);
                (StatusCode::OK, Json(json!({ "ok": true, "deleted": n })))
            }
            Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))),
        };
    }
    match state.session.truncate(id, user_index, mode) {
        Ok(n) => {
            // 回滚=回到该步初始状态：身份探测缓存活在会话之外，不清的话下一次
            // 「你是什么模型」会直接拿回滚前的缓存答案，表现得像秒出复用
            // （2026-10-27 真机实测：日志里只有 identity.probe_answered，没有
            // gateway.request）。只清 rollback；单条删除/多选删除不算回到初始状态。
            if mode == "rollback" {
                crate::server::chat::clear_identity_cache();
            }
            observability::event(
                EventType::State,
                json!({ "kind": "session.truncated", "session": id, "mode": mode, "user_index": user_index, "restore_checkpoint": !restore_cp.is_empty(), "messages": n }),
            );
            // 回滚/编辑/删除把上下文实际变小了（跨过压缩边界还会让压缩记录作废、
            // 原文全量回归）：圆环按当前请求视图重算并推送
            refresh_ring_after_truncate(&state, id);
            (StatusCode::OK, Json(json!({ "ok": true, "messages": n })))
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))),
    }
}

/// 截断（回滚/编辑/删除）后圆环立即重算。
///
/// 为什么需要：圆环分子 = 渠道实报锚点 + 自锚点以来的增量，锚点是截断**前**那次
/// 请求的实报——消息删掉后锚点已失真。不重算的话圆环会一直挂着旧值，直到下一轮
/// 实报回来才「跳变」，用户会把这次跳变误读成压缩（真机 2026-11-07：回滚到压缩
/// 边界之前，压缩记录作废、原文全量回归约 16.8 万，圆环却还停在压缩完成时的 4.4 万）。
///
/// 尺子与核心循环/压缩一致：请求视图走 `compact::build_view`（记录有效=摘要+尾段；
/// 锚点被删=全量原文），思考按渠道回放判定计入，system 与工具定义从上一轮快照带入
/// （会话文件不存 system）。旧实报锚点随被删消息作废，下一次请求的实报重新校准。
/// 重算**保留锚点的校准偏差**（只按构成增减平移），否则删消息后圆环可能不降反升。
/// 回滚到一条消息都不剩时直接清零（与「新建对话」同口径，见下方空视图分支）。
pub(super) fn refresh_ring_after_truncate(state: &AppState, id: &str) {
    let Some(f) = state.session.get(id) else { return };
    let prev = f.usage.unwrap_or_default();
    let window = prev.window;
    let view = crate::server::compact::build_view(&f.messages, f.compact.as_ref());
    // 回滚/删除到一条消息都不剩：与「新建对话」同一把尺子——没发过请求就没有圆环占用。
    // 固定底（系统提示词 + 工具定义）只在真正发请求时才算；回退后没有任何一轮读过它们，
    // 圆环不应留着占比。窗口与历史 api 统计保留。
    if view.is_empty() {
        let mut snap = prev;
        snap.apply_context(None, 0, window, &crate::context::Breakdown::default());
        if let Err(e) = state.session.update_usage(id, snap) {
            observability::debug(&format!("截断成空后用量快照落盘失败: {e}"));
        }
        crate::server::ui_events::emit(json!({
            "kind": "context_usage",
            "session": id,
            "context_usage": { "pressure": null, "projected": 0, "window": window, "breakdown": null },
        }));
        return;
    }
    let count_reasoning = {
        let s = state.settings.read().unwrap_or_else(|e| e.into_inner()).clone();
        // 一套配置都没有（极端/测试态）= 按不计入处理，绝不 panic（请求路径不许崩）
        if s.configs.is_empty() {
            false
        } else {
            let active = s.active_or_default().clone();
            let ch = crate::server::chat::assemble_turn_channels(state, &s, &active, "", "", "");
            let compat = crate::gateway::compat::resolve(
                ch.primary.params.provider_id.as_deref(),
                Some(ch.primary.base_url.trim()),
                &ch.primary.model,
                ch.primary.params.thinking_format,
            );
            crate::gateway::replay::should_preserve_replay(&ch.primary.model, &compat)
        }
    };
    let mut b = crate::context::estimate_breakdown(&view, count_reasoning);
    b.system = prev.system;
    b.tools_schema = prev.tools_schema;
    let new_parts = b.total();
    // 保留上一轮实报锚点的校准偏差：`projected` 是「实报 + 增量」的结果，本地估价
    // 往往偏高（system/工具定义尤其明显）。直接拿 b.total() 当新 projected 会把偏差
    // 丢掉——删掉消息后圆环反而可能变大（回滚后圆环没有变化）。
    // 只按构成增减平移：new = old + (new_parts - old_parts)，锚点仍作废（pressure=None），
    // 下一次请求的实报再重新校准。
    let old_parts = prev.messages + prev.tools + prev.system + prev.other + prev.tools_schema;
    let projected = if prev.projected > 0 && old_parts > 0 {
        let offset = prev.projected as i128 - old_parts as i128;
        (new_parts as i128 + offset).clamp(0, u64::MAX as i128) as u64
    } else {
        new_parts
    };
    let mut snap = prev;
    snap.apply_context(None, projected, window, &b);
    if let Err(e) = state.session.update_usage(id, snap) {
        observability::debug(&format!("截断后用量快照落盘失败: {e}"));
    }
    crate::server::ui_events::emit(json!({
        "kind": "context_usage",
        "session": id,
        "context_usage": {
            "pressure": null,
            "projected": projected,
            "window": window,
            "breakdown": {
                "messages": b.messages,
                "tools": b.tools,
                "system": b.system,
                "other": b.other,
                "tools_schema": b.tools_schema,
            },
        },
    }));
}
