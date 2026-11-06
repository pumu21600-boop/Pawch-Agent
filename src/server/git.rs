//! Git 状态/提交/检查点端点（自 server.rs 按域拆出）

use super::*;

/// GET /api/changes?baseline=xx[&origin=shadow|workspace][&since=unix秒] —— 本任务相对基线的改动文件清单
/// （「产物」/「git」标签数据源）files: [{ path, status, adds, dels }]；未跟踪文件也在内
/// （agent 新建的文件必现）。origin=workspace 且不传 baseline = 真仓库相对 HEAD（「未提交改动」档）。
/// since：只保留**当前文件修改时间 ≥ since** 的条目（产物面板在非 git 工作区用会话开始时间过滤——
/// 空树基线会把工作区里所有历史文件都算成"本会话产物"，含用户自己拷进来的；2026-11-03 真机实测）。
pub(super) async fn list_changes(
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let err = |e: String| (StatusCode::BAD_REQUEST, Json(json!({ "error": e })));
    let origin = crate::recovery::RepoOrigin::from_query(params.get("origin").map(|s| s.as_str()));
    let since: u64 = params.get("since").and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    let mut baseline = params.get("baseline").map(|s| s.trim()).unwrap_or("").to_string();
    if baseline.is_empty() {
        if origin == crate::recovery::RepoOrigin::Workspace {
            baseline = "HEAD".to_string();
        } else {
            return Err(err("缺少 baseline".into()));
        }
    }
    let ws = crate::tools::workspace_root().to_path_buf();
    match tokio::task::spawn_blocking(move || {
        let mut files = crate::recovery::changed_files(&ws, origin, &baseline)?;
        if since > 0 {
            let since_t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(since);
            files.retain(|f| {
                std::fs::metadata(ws.join(&f.path))
                    .and_then(|m| m.modified())
                    .map(|t| t >= since_t)
                    .unwrap_or(false)
            });
        }
        // 陈旧子树警告：基线里的顶层目录已不在工作区（用户把目录挪走的典型症状），
        // 不拦清单，只在响应里带一句人话，让产物/git 面板有机会说明白
        let warning = crate::recovery::stale_subtree_warning(&ws, origin, &baseline, &files);
        Ok::<_, anyhow::Error>((files, warning))
    })
    .await
    {
        Ok(Ok((files, warning))) => Ok(Json(json!({ "ok": true, "files": files, "warning": warning }))),
        Ok(Err(e)) => Err(err(e.to_string())),
        Err(e) => Err(err(e.to_string())),
    }
}

/// GET /api/artifacts?session=<id> —— 本会话产物账本（工作区相对路径清单）。
/// 与 /api/changes 的区别：后者是**工作区全局**改动（git 面板/未提交改动用），
/// 本接口只认「这个会话的某一轮任务真正改到过」的文件。产物面板必须走这条，
/// 绝不能再拿全局 diff 充数——会话活得久，别的会话写的东西会全算到它头上
/// （2026-11-06 校园网会话里冒出便利店 index.html/截图，实锤）。
/// tracked=false 表示旧会话还没有归属记录（前端回退到消息里的 write/edit 路径）。
pub(super) async fn list_artifacts(
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let err = |e: String| (StatusCode::BAD_REQUEST, Json(json!({ "error": e })));
    let session = params.get("session").map(|s| s.trim()).unwrap_or("");
    if session.is_empty() {
        return Err(err("缺少 session".into()));
    }
    let ws = crate::tools::workspace_root().to_path_buf();
    // 旧会话没有账本（或账本还不是回溯来的）：从影子仓库里本会话自己的检查点链
    // 回溯一次，写进账本再返回。回溯只认「相邻两个本会话提交」之间的 diff，
    // 绝不拿工作区全局状态充数；影子仓库里没有这个会话的提交时返回 None，不写空账本。
    let mut ledger = crate::artifacts::read(session);
    let need_backfill = match &ledger {
        None => true,
        Some(l) => !l.backfilled && crate::artifacts::same_workspace(&l.workspace, &ws),
    };
    if need_backfill {
        let ws_calc = ws.clone();
        let sid = session.to_string();
        let calc = tokio::task::spawn_blocking(move || crate::recovery::session_changed_paths(&ws_calc, &sid)).await;
        if let Ok(Ok(Some(paths))) = calc {
            let _ = crate::artifacts::record_backfill(session, &ws, paths);
            ledger = crate::artifacts::read(session);
        }
    }
    let Some(ledger) = ledger else {
        return Ok(Json(json!({
            "ok": true,
            "tracked": false,
            "files": [],
            "note": "这个会话没有产物归属记录，也没找到可回溯的历史检查点"
        })));
    };
    // 工作区换了：旧账本属于别的项目，不冒充当前工作区的产物
    if !crate::artifacts::same_workspace(&ledger.workspace, &ws) {
        return Ok(Json(json!({
            "ok": true,
            "tracked": true,
            "backfilled": ledger.backfilled,
            "files": [],
            "note": "该会话的产物属于另一个工作区"
        })));
    }
    let files: Vec<Value> = crate::artifacts::existing_paths(&ledger, &ws)
        .into_iter()
        .map(|p| json!({ "path": p, "status": "M", "adds": 0, "dels": 0 }))
        .collect();
    let note = if ledger.backfilled && files.is_empty() {
        "旧会话：按检查点回溯没有找到工作区产出（可能只改了工作区外的文件）"
    } else {
        ""
    };
    Ok(Json(json!({
        "ok": true,
        "tracked": true,
        "backfilled": ledger.backfilled,
        "files": files,
        "note": note
    })))
}

/// GET /api/review/diff?baseline=xx[&path=xx][&origin=shadow|workspace] —— 相对基线的统一 diff 文本
/// （git 面板逐文件看具体改动用；path 缺省 = 全量）。
pub(super) async fn review_diff(
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let err = |e: String| (StatusCode::BAD_REQUEST, Json(json!({ "error": e })));
    let origin = crate::recovery::RepoOrigin::from_query(params.get("origin").map(|s| s.as_str()));
    let mut baseline = params.get("baseline").map(|s| s.trim()).unwrap_or("").to_string();
    if baseline.is_empty() {
        if origin == crate::recovery::RepoOrigin::Workspace {
            baseline = "HEAD".to_string();
        } else {
            return Err(err("缺少 baseline".into()));
        }
    }
    let ws = crate::tools::workspace_root().to_path_buf();
    let path = params.get("path").map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    match tokio::task::spawn_blocking(move || {
        crate::recovery::diff_text(&ws, origin, &baseline, path.as_deref())
    })
    .await
    {
        Ok(Ok(text)) => Ok(Json(json!({ "ok": true, "diff": text }))),
        Ok(Err(e)) => Err(err(e.to_string())),
        Err(e) => Err(err(e.to_string())),
    }
}

/// GET /api/git/status —— 工作区 git 概览（分支/HEAD/脏文件数/上游领先落后/未提交明细）。
/// is_repo=false 时分两种：git_missing=true = 机器没装 git（全套 git 功能不可用），
/// false = 装了 git 但当前工作区不是仓库。前端都要照实说话，不许静默隐藏。
pub(super) async fn git_status(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    let _ = &state;
    let ws = crate::tools::workspace_root().to_path_buf();
    match tokio::task::spawn_blocking(move || crate::recovery::status(&ws)).await {
        Ok(s) => (StatusCode::OK, Json(serde_json::to_value(s).unwrap_or_else(|_| json!({ "is_repo": false, "git_missing": true })))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

/// GET /api/git/info —— git 运行时状态（设置面板「插件」分区：可用性/版本/来源/内置包）。
pub(super) async fn git_runtime_info() -> Json<Value> {
    Json(crate::tools::git_info())
}

#[derive(Debug, Deserialize)]
pub struct GitCommitRequest {
    pub message: String,
    /// 要提交的路径；缺省/空数组 = 全部改动（add -A）
    #[serde(default)]
    pub files: Vec<String>,
}

/// POST /api/git/commit —— 用户手动提交：按选中路径 stage 后 commit，返回短哈希。
pub(super) async fn git_commit(
    State(state): State<AppState>,
    Json(req): Json<GitCommitRequest>,
) -> (StatusCode, Json<Value>) {
    let _ = &state;
    let ws = crate::tools::workspace_root().to_path_buf();
    let msg = req.message.clone();
    let files = req.files.clone();
    let res = tokio::task::spawn_blocking(move || crate::recovery::commit(&ws, &msg, &files)).await;
    match res {
        Ok(Ok(id)) => {
            observability::event(
                EventType::State,
                json!({ "kind": "git.committed", "id": id, "files": req.files.len(), "manual": true }),
            );
            (StatusCode::OK, Json(json!({ "ok": true, "id": id })))
        }
        // git 自己的报错（如「没有可提交的改动」）直接给用户看，比翻译成中文更有用
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

/// POST /api/checkpoints[?session=<id>] —— 手动重拍基线（「重新以当前状态为起点」）。
/// 场景：产物/git 面板的基线过期——基线里的目录已被挪走/删掉，任务档清单成鬼影
/// （stale_subtree_warning 触发）。用户点一下，以当前工作区状态为新起点，清单回到
/// 真实增量。会话归属：带 session 记到该会话（重拍是用户显式动作，归属当前会话）。
pub(super) async fn create_checkpoint(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    let _ = &state;
    let session = q.get("session").map(|s| s.trim().to_string()).unwrap_or_default();
    let ws = crate::tools::workspace_root().to_path_buf();
    let ws_for_list = ws.clone();
    match tokio::task::spawn_blocking(move || crate::recovery::checkpoint(&ws, "手动重拍基线", &session)).await {
        Ok(Ok(Some(cp))) => (StatusCode::OK, Json(json!({
            "ok": true,
            "checkpoint": { "id": cp.id, "ts": cp.ts, "session": cp.session }
        }))),
        // 无变更（checkpoint 的 None 语义：工作区相对影子仓库 HEAD 没有差异）不算失败——
        // 最近一个已有检查点本身就代表当前状态，直接拿它当新基线；一个检查点都没有
        // 或非 git 工作区，才真的是「拍不了基线」，给人话。
        Ok(Ok(None)) => match tokio::task::spawn_blocking(move || crate::recovery::list(&ws_for_list, 1)).await {
            Ok(Ok(list)) if !list.is_empty() => {
                let cp = &list[0];
                (StatusCode::OK, Json(json!({
                    "ok": true,
                    "note": "工作区自上个检查点后没有新改动，直接以它为起点。",
                    "checkpoint": { "id": cp.id, "ts": cp.ts, "session": cp.session }
                })))
            }
            _ => (StatusCode::OK, Json(json!({ "ok": false, "note": "工作区没有可跟踪的 git 文件，拍不了基线" }))),
        },
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

/// GET /api/checkpoints[?session=<id>] —— 影子仓库检查点列表（新→旧，展示最多 20 个）。
/// 带 session 时额外返回 `mine`（该会话自己的检查点，改动条基线取它）与 `first`
/// （该会话**最早**的检查点=会话起点）。产物面板必须用 `first`：危险工具的执行前
/// 快照会把检查点列表刷爆（2026-10-31 实锤：一个会话 16 个全是 shell/edit 快照，
/// 任务起点被挤出 20 窗口，产物面板拿「最新快照」当基线=会话早先的产物全部隐身）。
pub(super) async fn list_checkpoints(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    let _ = &state;
    let want = q.get("session").map(|s| s.trim().to_string()).unwrap_or_default();
    let ws = crate::tools::workspace_root().to_path_buf();
    match tokio::task::spawn_blocking(move || crate::recovery::list(&ws, 500)).await {
        Ok(Ok(list)) => {
            let mine_all: Vec<_> = list.iter().filter(|c| !want.is_empty() && c.session == want).cloned().collect();
            let first = mine_all.last().cloned();
            let mine_top: Vec<_> = mine_all.iter().take(20).cloned().collect();
            (StatusCode::OK, Json(json!({ "checkpoints": list.iter().take(20).cloned().collect::<Vec<_>>(), "mine": mine_top, "first": first })))
        }
        // 仓库还没有任何提交（git log fatal: does not have any commits yet）不是错误，
        // 是空态——空清单 + 人话备注，别 500 糊前端一脸
        Ok(Err(e)) => {
            let msg = e.to_string();
            if msg.contains("does not have any commits yet") || msg.contains("not a git repository") {
                return (StatusCode::OK, Json(json!({ "checkpoints": [], "mine": [], "note": "工作区 git 仓库还没有提交，暂无检查点" })));
            }
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": msg })))
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}

/// POST /api/checkpoints/restore —— 工作树回滚到指定检查点（shell 副作用不可回滚）。
pub(super) async fn restore_checkpoint(
    State(state): State<AppState>,
    Json(req): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let _ = &state;
    let Some(id) = req.get("id").and_then(Value::as_str) else {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "缺少 id" })));
    };
    let ws = crate::tools::workspace_root().to_path_buf();
    let id = id.to_string();
    match tokio::task::spawn_blocking(move || crate::recovery::restore(&ws, &id)).await {
        Ok(Ok(())) => {
            observability::event(
                EventType::State,
                json!({ "kind": "checkpoint.restored", "id": req.get("id").and_then(Value::as_str).unwrap_or_default() }),
            );
            (StatusCode::OK, Json(json!({ "ok": true })))
        }
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))),
    }
}
