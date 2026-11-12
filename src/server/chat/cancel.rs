use super::*;

#[derive(Debug, Deserialize)]
pub struct CancelQuery {
    /// 目标会话（可空：不带 = 停全部在跑任务，命令行客户端与内置页的兼容语义）
    #[serde(default)]
    pub session: String,
}

/// POST /cancel[?session=<id>] —— 会话级停止：只停指定会话当前这一轮；
/// 不带 session 时停全部在跑任务。
pub(crate) async fn cancel(State(state): State<AppState>, Query(q): Query<CancelQuery>) -> Json<Value> {
    let session = q.session.trim();
    if session.is_empty() {
        let count = state.cancel.cancel_all();
        observability::event(
            EventType::State,
            json!({ "kind": "cancel", "scope": "all", "count": count }),
        );
        Json(json!({ "cancelled": true, "scope": "all", "count": count }))
    } else {
        let hit = state.cancel.cancel(session);
        // 父任务被停 → 级联停它的全部活跃子代理（子代理跑在独立会话，标志互不相通）
        let cascaded = subagent::cancel_children(session);
        observability::event(
            EventType::State,
            json!({ "kind": "cancel", "scope": "session", "session": session, "active": hit, "subagents_cancelled": cascaded }),
        );
        Json(json!({ "cancelled": true, "scope": "session", "session": session, "active": hit, "subagents_cancelled": cascaded }))
    }
}

#[derive(Debug, Deserialize)]
pub struct ReviveRequest {
    #[serde(default)]
    pub session: String,
}

/// POST /api/chat/revive —— 页面刷新回来认领正在跑的一轮（断线取消 30 秒宽限内）。
/// 只豁免断线看门狗；用户点过「停止」的轮次认领也救不回来。
pub(crate) async fn revive(State(state): State<AppState>, Json(req): Json<ReviveRequest>) -> Json<Value> {
    let session = req.session.trim();
    if session.is_empty() {
        return Json(json!({ "ok": false, "error": "缺少 session" }));
    }
    let hit = state.cancel.revive(session);
    observability::event(
        EventType::State,
        json!({ "kind": "chat.revive", "session": session, "hit": hit }),
    );
    Json(json!({ "ok": true, "revived": hit }))
}

#[derive(Debug, Deserialize)]
pub struct AttachQuery {
    #[serde(default)]
    pub session: String,
    /// 前端最后收到的 seq：只补发这之后的事件
    #[serde(default)]
    pub after: u64,
}

/// GET /api/chat/attach?session=<id>&after=<seq> —— 刷新回来后先补发漏掉的片段，
/// 再接上正在跑的轮次的直播（否则界面冻在刷新前最后一个工具调用）。
pub(crate) async fn attach(State(_state): State<AppState>, Query(q): Query<AttachQuery>) -> Response {
    let session = q.session.trim();
    if session.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "缺少 session" }))).into_response();
    }
    let Some((backlog, mut rx)) = super::live::attach(session, q.after) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "没有正在跑的一轮" }))).into_response();
    };
    let (tx, srx) = mpsc::unbounded_channel::<Result<Event, Infallible>>();
    tokio::spawn(async move {
        for v in backlog {
            if tx.send(Ok(json_event(v))).is_err() {
                return;
            }
        }
        loop {
            match rx.recv().await {
                Ok((_, v)) => {
                    if tx.send(Ok(json_event(v))).is_err() {
                        break;
                    }
                }
                // 订阅者慢消费丢了旧事件：backlog 已覆盖刷新窗口；剩下的继续看直播
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    Sse::new(UnboundedReceiverStream::new(srx)).into_response()
}

/// GET /api/cancel[?session=<id>] —— 带 session 查该会话；不带查「是否有在跑会话被取消」。
pub(crate) async fn cancel_status(State(state): State<AppState>, Query(q): Query<CancelQuery>) -> Json<Value> {
    let session = q.session.trim();
    if session.is_empty() {
        Json(json!({
            "cancelled": state.cancel.any_cancelled(),
            "active": state.cancel.active_count(),
        }))
    } else {
        Json(json!({
            "cancelled": state.cancel.is_set(session),
            "running": state.cancel.is_active(session),
            // 挂着的引导文本（2026-11-08）：「已引导」标记只活在前端内存，整页刷新
            // 就丢（真机：刷新后标记消失，几分钟后模型取走才由 steer 事件带回来）。
            // 这里给回权威清单，前端在 running 时铺回；模型取走后这里自然变空，
            // 之后由会话文件里的 steered 行接力展示。
            "pending_steers": state.steer.steer_texts(session),
            // 挂着的排队文本（2026-11-12）：前端「排队中」气泡可能来自刷新快照/服务端
            // 接管，比真实队列旧——消息已经送达跑成，气泡还挂着；点引导/立即发送会把
            // 同一句再投一遍。前端拿它对账：文本不在 pending_queued/pending_steers
            // 且对话里已有同文正式消息 → 只清气泡，不再投递。
            "pending_queued": state.steer.queued_texts(session),
        }))
    }
}

// —— trace 时间线

