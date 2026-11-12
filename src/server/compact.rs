//! 收尾静默压缩（server 侧）：触发判定 + 后台执行。
//!
//! 触发只有一条：上下文总量（每步 ContextUsage 的 projected，实报优先）≥ 窗口 ×
//! 压缩线——**到线就压，不做事前预估拦截**。停压条件只有一条：上一次压缩的实际结果
//! （记录里的 post_tokens）仍 ≥ 线的 80%——压完还在高位说明腾不出空间，继续压只会
//! 反复烧摘要调用，于是本会话不再压（回滚/编辑使记录作废、或调整窗口/线后自然解除）。
//! 判定在 done 发出之后，任务 `tokio::spawn` 独立跑——用户切会话/切工作区
//! 都不打断；SSE 发送失败（页面已关）只影响实时行，结果照常落盘 + 广播刷新。
//! 超时 120s、连续失败 3 次熔断；「点停止」不取消（回答已完成，压缩是收尾动作）。

use super::*;
use crate::compact::{self, CompactRecord};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub(super) type SseEmit = Arc<dyn Fn(Value) + Send + Sync>;

/// 正在压缩的会话（进程内）：同一会话同时只允许一个压缩 job 在飞。压缩不占
/// cancel 运行登记（不挡新一轮对话），没有这道闸时两轮收尾各起一个 job，慢的
/// 旧压缩后落地会把边界回退到更早一次压缩，标记行/圆环与真实请求视图错位
///（2026-11-08 审计 P2-2）。
static INFLIGHT: std::sync::OnceLock<std::sync::Mutex<HashSet<String>>> = std::sync::OnceLock::new();
fn inflight_slot() -> &'static std::sync::Mutex<HashSet<String>> {
    INFLIGHT.get_or_init(|| std::sync::Mutex::new(HashSet::new()))
}

/// 在飞登记守卫：Drop 摘除（含 panic 路径），随 RunJob 进任务、随任务结束释放。
struct InflightGuard(String);
impl Drop for InflightGuard {
    fn drop(&mut self) {
        inflight_slot()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

/// 连续失败计数（进程内；重启即清零，熔断只是防渠道挂死白烧钱）。
pub type FailStreaks = std::sync::Mutex<HashMap<String, u32>>;

fn fail_streak(state: &AppState, sid: &str) -> u32 {
    state
        .compact_fail_streak
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(sid)
        .copied()
        .unwrap_or(0)
}

/// 触发入参打包（maybe_start 只收 state + 这一个结构，避免 8 参长签名）。
pub(super) struct CompactTrigger {
    pub session_id: String,
    pub gateway: Arc<crate::gateway::Gateway>,
    pub primary: crate::gateway::Channel,
    pub summary_role: Option<crate::gateway::Channel>,
    pub emit: Option<SseEmit>,
    pub pre_tokens: u64,
    pub tools_schema_tokens: u64,
    /// 主循环实测的系统提示占用（会话文件不存 system，只能由最后一次圆环快照带入；
    /// 原先写死 2000 常量，长系统提示的会话会漏报）。
    pub system_tokens: u64,
    /// 发送侧超大工具结果的截断阈值——压缩后的估价要和主循环请求视图同样经过这道
    /// 卫生，否则标记行/圆环会比下一轮真实请求偏高。
    pub max_tool_result_chars: usize,
    /// 本轮请求生效的窗口（渠道 context_length 覆盖全局后的值）——必须与圆环
    /// 同一把尺子：真机 2026-11-07 实锤「圆环 77% 不压缩」，触发用全局 9.6 万
    /// 而圆环显示渠道 5 万，阈值差 400 tokens 永远够不着。
    pub window: u64,
}

/// done 之后调用：满足条件就发 `compact_started` 并 spawn 后台任务。返回是否触发。
pub(super) fn maybe_start(state: &AppState, t: CompactTrigger) -> bool {
    let CompactTrigger {
        session_id,
        gateway,
        primary,
        summary_role,
        emit,
        pre_tokens,
        tools_schema_tokens,
        system_tokens,
        max_tool_result_chars,
        window,
    } = t;
    let session_id = session_id.as_str();
    let percent = state
        .settings
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .compact_percent
        .unwrap_or(85);
    let threshold = compact::threshold_tokens(window, percent);
    // 诊断（2026-11-07）：静默返回全部落一条 not_triggered——「为什么没压」不能再靠猜
    macro_rules! not_triggered {
        ($reason:expr) => {{
            observability::event(
                EventType::State,
                json!({ "kind": "compact.not_triggered", "session": session_id, "reason": $reason, "pre_tokens": pre_tokens, "window": window, "percent": percent, "threshold": threshold }),
            );
            return false;
        }};
    }
    if threshold == 0 {
        not_triggered!("窗口未知（context_length 未配置）"); // 窗口未知 = 不压（用户没配 context_length 时宁可不做）
    }
    if pre_tokens < threshold {
        not_triggered!("占用未到线");
    }
    // 2026-11-10 用户拍板：压缩失败一次即暂停自动重试，等用户点标记行上的「重试」
    // （POST /api/compact/retry 清掉暂停标记）。不再自动冷却/累积重试。
    if fail_streak(state, session_id) >= compact::MAX_FAIL_STREAK {
        observability::event(
            EventType::State,
            json!({ "kind": "compact.paused", "session": session_id }),
        );
        return false;
    }
    if inflight_slot().lock().unwrap_or_else(|e| e.into_inner()).contains(session_id) {
        not_triggered!("上一轮压缩还在跑");
    }
    let Some(f) = state.session.get(session_id) else { not_triggered!("会话文件读取失败") };
    // 停压条件（唯一）：上一次压缩的结果仍在线的 80% 以上（记录里的 post_tokens
    // 就是当时压完的估价，也是标记行/圆环上的数）——压完还在高位说明腾不出空间，
    // 再压只是反复烧摘要调用。记录作废（锚点被回滚/删除）时不看它，恢复「到线就压」。
    let record_live = f
        .compact
        .as_ref()
        .is_some_and(|r| f.messages.iter().any(|m| m.get("id").and_then(Value::as_str) == Some(r.tail_start_id.as_str())));
    if record_live
        && let Some(r) = f.compact.as_ref()
        && r.post_tokens >= threshold.saturating_mul(80) / 100
    {
        observability::event(
            EventType::State,
            json!({
                "kind": "compact.not_triggered", "session": session_id,
                "reason": "上次压完仍 ≥ 线的 80%（不再压）",
                "pre_tokens": pre_tokens, "window": window, "percent": percent,
                "threshold": threshold, "post_tokens": r.post_tokens,
            }),
        );
        return false;
    }
    // 请求视图（与核心循环同一把尺子）：旧摘要消息 + tail 起的历史
    let view = build_view(&f.messages, f.compact.as_ref());
    let Some((split, summarized_last_id, tail_start_id, after_id)) = compact::split_for_summary(&view) else {
        not_triggered!("没有可压的问答轮（被压段为空或锚点缺 id）");
    };
    if !compact::summarized_has_assistant(&view, split) {
        not_triggered!("被压段没有 assistant 回复");
    }
    let tail = &view[split..];
    // 占用估算要不要数思考：与主循环同款回放判定（主渠道为准）。标记行必须用它
    // ——写死 false 的尺子会把重发思考的渠道（DeepSeek/zai/白名单）整段漏掉
    //（真机 2026-11-07：尾段 2.7 万 thinking 全没进「压缩后上下文 29k」）。
    let count_reasoning = {
        let compat = crate::gateway::compat::resolve_with_protocol(
            primary.params.provider_id.as_deref(),
            Some(primary.base_url.trim()),
            &primary.model,
            primary.params.thinking_format,
            primary.params.protocol,
        );
        crate::gateway::replay::reasoning_count(&compat, &primary.model)
    };
    let job_id = uuid::Uuid::new_v4().to_string();
    let pre_k = pre_tokens.saturating_add(999) / 1000;
    if let Some(emit) = &emit {
        emit(json!({ "compact_started": { "id": job_id, "pre_k": pre_k, "text": compact::RUNNING_MARKER_TEXT } }));
    }
    // 标记同步走全局 UI 总线（2026-11-08 真机：聊天流在压缩期间可能已死——用户中途
    // 刷新过页面，started 只发死流 = 「正在压缩」全程不可见，done 靠文件重建才冒出来）。
    // 圆环的 usage_changed 早已同款双发，标记补齐。
    crate::server::ui_events::emit(json!({
        "kind": "compact_started", "session": session_id, "id": job_id, "pre_k": pre_k, "text": compact::RUNNING_MARKER_TEXT
    }));
    observability::event(
        EventType::State,
        json!({ "kind": "compact.started", "session": session_id, "job": job_id, "pre_tokens": pre_tokens, "threshold": threshold, "summarized_msgs": split }),
    );

    // 占在飞登记：从这里到任务结束（含 panic 展开走 Drop）同会话不再起第二个 job
    let inflight = InflightGuard(session_id.to_string());
    let job = RunJob {
        session: state.session.clone(),
        fail_streaks: state.compact_fail_streak.clone(),
        _inflight: inflight,
        session_id: session_id.to_string(),
        gateway,
        primary,
        summary_role,
        emit,
        job_id,
        pre_tokens,
        tools_schema_tokens,
        system_tokens,
        max_tool_result_chars,
        count_reasoning,
        summarized: view[..split].to_vec(),
        tail: tail.to_vec(),
        summarized_last_id,
        tail_start_id,
        after_id,
        transcript_dir: state.session.transcripts_dir(),
    };
    tokio::spawn(run_job(job));
    true
}

/// POST /api/compact/retry —— 用户点标记行上的「重试」：清掉该会话的「压缩失败暂停」
/// 标记。下一次轮次收尾会重新走完整触发判定（阈值/切分/守卫照旧）；不在这里同步
/// 起 job——重试缺本轮 ContextUsage 口径，硬造数据会把「统一尺子」弄歪（2026-11-10）。
pub(super) async fn retry(
    State(state): State<AppState>,
    Json(req): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let sid = req
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if sid.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "缺少 session_id" })),
        );
    }
    state
        .compact_fail_streak
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&sid);
    (
        StatusCode::OK,
        Json(json!({ "ok": true, "note": "已恢复自动压缩：下轮结束后会再尝试" })),
    )
}

struct RunJob {
    session: Arc<crate::session::SessionManager>,
    fail_streaks: Arc<FailStreaks>,
    /// 在飞登记：随任务 Drop 摘除（同会话并发压缩闸）
    _inflight: InflightGuard,
    session_id: String,
    gateway: Arc<crate::gateway::Gateway>,
    primary: crate::gateway::Channel,
    summary_role: Option<crate::gateway::Channel>,
    emit: Option<SseEmit>,
    job_id: String,
    pre_tokens: u64,
    tools_schema_tokens: u64,
    system_tokens: u64,
    max_tool_result_chars: usize,
    /// 回放判定同款：压缩后视图的占用估算要不要数思考（与主循环同一把尺子）
    count_reasoning: crate::context::ReasoningCount,
    summarized: Vec<Value>,
    tail: Vec<Value>,
    summarized_last_id: String,
    tail_start_id: String,
    after_id: String,
    transcript_dir: std::path::PathBuf,
}

impl RunJob {
    fn send(&self, v: Value) {
        if let Some(emit) = &self.emit {
            emit(v);
        }
    }
    /// 摘要请求：清形后的被压段 + 提示词（纯文本、禁工具）。`max_tokens` 由调用方给
    /// ——Responses 协议下模型默认思考且关不掉，常态 4000 的预算会被思考吃满。
    async fn ask_summary(&self, max_tokens: u64) -> anyhow::Result<String> {
        let mut req = compact::clean_for_summary(&self.summarized);
        req.push(json!({ "role": "user", "content": compact::COMPACT_PROMPT }));
        let ctx = crate::gateway::UsageCtx::aux(&self.session_id, "compact");
        // 摘要不思考（2026-11-08 真机）：摘要参数原样继承主渠道，「思考：最大」会跟着
        // 进来——DeepSeek 系思考直接吃满 SUMMARY_MAX_TOKENS（台账实锤：compact 调用
        // completion_tokens 条条顶格 4000），正文被挤成空 → 判「摘要为空」，压缩白跑。
        // 摘要是纯文本整卷归纳，不需要思考；两个分支统一剥掉。
        let out = match &self.summary_role {
            Some(role) => {
                let rp = crate::gateway::ChatParams {
                    max_tokens: Some(max_tokens),
                    thinking: None,
                    ..role.params.clone()
                };
                let ch = crate::gateway::Channel { params: rp, ..role.clone() };
                tokio::time::timeout(
                    compact::COMPACT_TIMEOUT,
                    self.gateway
                        .complete(&ctx, &ch.base_url, &ch.model, &ch.api_key, &json!(req), &ch.params),
                )
                .await
            }
            None => {
                let params = crate::gateway::ChatParams {
                    max_tokens: Some(max_tokens),
                    thinking: None,
                    ..self.primary.params.clone()
                };
                let ch = crate::gateway::Channel { params, ..self.primary.clone() };
                tokio::time::timeout(
                    compact::COMPACT_TIMEOUT,
                    self.gateway
                        .complete(&ctx, &ch.base_url, &ch.model, &ch.api_key, &json!(req), &ch.params),
                )
                .await
            }
        };
        match out {
            Ok(Ok(text)) => Ok(text.trim().to_string()),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(anyhow::anyhow!("超时（120s）")),
        }
    }
    /// 良性作废：边界消息被回滚/删除，结果不落盘、**不计失败熔断**（用户压缩期间
    /// 回滚几次就把会话熔断到重启才能恢复，是 2026-11-08 审计抓到的错位）。
    fn void(&self, reason: &str) {
        self.send(json!({ "compact_failed": { "id": self.job_id, "text": "压缩结果作废：该段已被回滚或删除" } }));
        crate::server::ui_events::emit(json!({
            "kind": "compact_failed", "session": self.session_id, "id": self.job_id, "text": "压缩结果作废：该段已被回滚或删除"
        }));
        observability::event(
            EventType::State,
            json!({ "kind": "compact.voided", "session": self.session_id, "job": self.job_id, "reason": reason }),
        );
    }
    fn fail(&self, reason: &str) {
        // 用户拍板（2026-11-10）：失败一次即暂停自动重试（不累积、不冷却）；失败原因
        // 直接写在标记行里并落盘，旁边给「重试」按钮——选择权交给用户。
        let short = short_fail_reason(reason);
        self.fail_streaks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(self.session_id.clone(), crate::compact::MAX_FAIL_STREAK);
        let text = format!("————压缩失败：{short}；点右侧重试————");
        self.send(json!({ "compact_failed": { "id": self.job_id, "text": text, "reason": short } }));
        crate::server::ui_events::emit(json!({
            "kind": "compact_failed", "session": self.session_id, "id": self.job_id, "text": text, "reason": short
        }));
        // 失败标记落盘：刷新页面后仍能看到原因与重试入口
        if let Some(mut f) = self.session.get(&self.session_id) {
            f.messages.push(compact::marker_message(&text));
            if let Err(e) = self
                .session
                .save_with_workspace(&self.session_id, &f.messages, None, None)
            {
                observability::debug(&format!("压缩失败标记落盘失败: {e}"));
            }
        }
        observability::event(
            EventType::Error,
            json!({ "kind": "compact.failed", "session": self.session_id, "job": self.job_id, "reason": reason }),
        );
    }
}

/// 失败原因压成一行短句（标记行用）：常见三类归一，其余截断到 80 字符。
fn short_fail_reason(reason: &str) -> String {
    let first = reason.lines().next().unwrap_or(reason).trim();
    if first.contains("超时") {
        "渠道超时".to_string()
    } else if first.contains("摘要为空") {
        "摘要为空（模型没有输出摘要）".to_string()
    } else if first.contains("写失败") || first.contains("写盘") {
        "记录写盘失败".to_string()
    } else {
        first.chars().take(80).collect()
    }
}

async fn run_job(j: RunJob) {
    // 摘要输出预算：9 段摘要按上限 SUMMARY_MAX_TOKENS 计，另外必须给「思考」留余量
    // ——三条格式（messages / chat completions / responses）在不发思考参数时都会
    // 默认思考（2026-11-11 真机实测，4000 被思考吃满、正文空），统一按主循环同款
    // 扩容公式（mt + max(8192)）给足。空回再给一次机会（思考长度有抖动）。
    let budget = compact::SUMMARY_MAX_TOKENS + compact::SUMMARY_MAX_TOKENS.max(8192);
    let summary = match j.ask_summary(budget).await {
        Ok(t) if !t.is_empty() => t,
        Ok(_) => {
            observability::event(
                EventType::State,
                json!({ "kind": "compact.empty_retry", "session": j.session_id, "job": j.job_id, "budget": budget }),
            );
            match j.ask_summary(budget).await {
                Ok(t) if !t.is_empty() => t,
                Ok(_) => {
                    j.fail("摘要为空");
                    return;
                }
                Err(e) => {
                    j.fail(&e.to_string());
                    return;
                }
            }
        }
        Err(e) => {
            j.fail(&e.to_string());
            return;
        }
    };

    // 完整记录落盘：被压段的原文级细节都在这里
    let transcript_path = {
        let dir = j.transcript_dir.clone();
        let sid = j.session_id.clone();
        let pre_tokens = j.pre_tokens;
        let summarized = j.summarized.clone();
        let write = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
            std::fs::create_dir_all(&dir)?;
            let n = std::fs::read_dir(&dir)
                .map(|rd| {
                    let prefix = format!("{sid}-");
                    rd.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(&prefix)).count() + 1
                })
                .unwrap_or(1);
            let path = dir.join(format!("{sid}-{n:03}.md"));
            std::fs::write(&path, render_transcript_document(&sid, &summarized, pre_tokens))?;
            Ok(path.display().to_string())
        })
        .await;
        match write {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => {
                j.fail(&format!("记录文件写失败: {e}"));
                return;
            }
            Err(e) => {
                j.fail(&format!("记录文件写任务失败: {e}"));
                return;
            }
        }
    };

    // 压缩后的请求视图（摘要消息 + 保留尾段）：先按发送侧同一套卫生净化（重复结果
    // 去重 + 超大结果截断），再估算——否则标记行/圆环会比下一轮真实请求偏高。
    let view = compact::CompactView {
        summary: summary.clone(),
        transcript_path: transcript_path.clone(),
        tail_start_id: j.tail_start_id.clone(),
    };
    let ring_view = {
        let mut v = vec![compact::summary_message(&view)];
        v.extend(j.tail.iter().cloned());
        let (deduped, _) = crate::context::dedupe_tool_results(&v);
        let (truncated, _) =
            crate::context::truncate_oversized_results(&deduped, j.max_tool_result_chars);
        truncated
    };
    // 标记行和圆环取同一个 Breakdown：尺子与主循环同一把（思考判定/系统提示/工具定义
    // 全在里），两个数从构造上不可能再打架。
    let ring = compact::post_breakdown(&ring_view, j.system_tokens, j.tools_schema_tokens, j.count_reasoning);
    let post_tokens = ring.total();
    let record = CompactRecord {
        summary,
        summarized_last_id: j.summarized_last_id.clone(),
        tail_start_id: j.tail_start_id.clone(),
        after_id: j.after_id.clone(),
        pre_tokens: j.pre_tokens,
        post_tokens,
        transcript_path,
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    };
    let marker = compact::marker_message(&compact::done_marker_text(post_tokens));
    let marker_text = marker.get("content").and_then(Value::as_str).unwrap_or("").to_string();
    let session = j.session.clone();
    let sid = j.session_id.clone();
    let apply = tokio::task::spawn_blocking(move || session.apply_compaction(&sid, record, marker)).await;
    match apply {
        Ok(Ok(true)) => {
            j.fail_streaks.lock().unwrap_or_else(|e| e.into_inner()).remove(&j.session_id);
            // 先发完成事件、再广播 sessions_changed：前端收到广播会按会话文件重建
            // 消息列表（文件里此刻已有完成标记行，替换实时行是无缝的）；顺序反过来
            // 重建会先冲掉「正在压缩」实时行、compact_done 按 id 找不到目标被丢弃
            // ——「正在压缩弹出来就消失」（真机 2026-11-07）。
            j.send(json!({ "compact_done": { "id": j.job_id, "text": marker_text, "post_k": post_tokens.saturating_add(999) / 1000 } }));
            crate::server::ui_events::emit(json!({
                "kind": "compact_done", "session": j.session_id, "id": j.job_id, "text": marker_text
            }));
            crate::server::ui_events::emit(json!({ "kind": "sessions_changed" }));
            // 圆环立即回落：同一形状走 chat 流的 context_usage 通道，前端圆环/构成
            // 一起落；快照随会话落盘，重启/切回也是新值（真机 2026-11-07：不推这
            // 一条，压缩完圆环停在旧占用上）。数目就是标记行那个 Breakdown。
            let window = j
                .session
                .get(&j.session_id)
                .and_then(|f| f.usage)
                .map(|u| u.window)
                .unwrap_or(0);
            let b = ring;
            let projected = post_tokens;
            let mut snap = j.session.get(&j.session_id).and_then(|f| f.usage).unwrap_or_default();
            snap.apply_context(None, projected, window, &b);
            // 快照同时走全局 UI 事件：聊天流在此刻可能已经死了（用户中途刷新过页面/
            // 断流），context_usage 发到死流上就丢——真机 2026-11-07 实锤「压缩完成后
            // 圆环要再刷一次界面才同步」。前端按事件把圆环直接打到新快照上。
            crate::server::ui_events::emit(json!({
                "kind": "usage_changed",
                "session": j.session_id,
                "usage": snap,
            }));
            if let Err(e) = j.session.update_usage(&j.session_id, snap) {
                observability::debug(&format!("压缩后用量快照落盘失败: {e}"));
            }
            j.send(json!({
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
                }
            }));
            observability::event(
                EventType::State,
                json!({ "kind": "compact.done", "session": j.session_id, "job": j.job_id, "pre": j.pre_tokens, "post": post_tokens }),
            );
        }
        Ok(Ok(false)) => {
            // 边界消息已被回滚/删除：结果作废（不落盘），实时行收口但**不计失败**
            j.void("边界消息已不在（回滚或删除）");
        }
        Ok(Err(e)) => {
            j.fail(&e.to_string());
        }
        Err(e) => {
            j.fail(&format!("落盘任务失败: {e}"));
        }
    }
}

/// 记录文档：会话头 + 被压段原文级细节。
fn render_transcript_document(sid: &str, summarized: &[Value], pre_tokens: u64) -> String {
    let ts = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
    let pre_k = pre_tokens.saturating_add(999) / 1000;
    format!(
        "# 压缩完整记录 — 会话 {sid}\n\n压缩时间：{ts}  \n压缩前上下文：约 {pre_k}k tokens  \n> 本文件是「————压缩后上下文————」标记之前的对话全文记录；摘要只保留要点，精确细节（完整代码、报错原文）以这里为准。\n\n---\n\n",
    ) + &compact::render_transcript(summarized, summarized.len())
}

/// 请求视图（无 system 头）：旧摘要消息 + tail_start 起的历史；记录作废 → 原样返回。
pub(super) fn build_view(messages: &[Value], record: Option<&CompactRecord>) -> Vec<Value> {
    let Some(r) = record else { return messages.to_vec() };
    let view = crate::compact::CompactView::from_record(r);
    let Some(i) = messages
        .iter()
        .position(|m| m.get("id").and_then(Value::as_str) == Some(&view.tail_start_id))
    else {
        return messages.to_vec();
    };
    let mut out = Vec::with_capacity(messages.len() + 1);
    out.push(crate::compact::summary_message(&view));
    out.extend_from_slice(&messages[i..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn build_view_inserts_summary_and_voids_on_missing_anchor() {
        let msgs = vec![
            json!({"id":"u1","role":"user","content":"q1"}),
            json!({"id":"a1","role":"assistant","content":"ans1"}),
            json!({"id":"u2","role":"user","content":"q2"}),
        ];
        let rec = CompactRecord {
            summary: "摘要".into(),
            summarized_last_id: "a1".into(),
            tail_start_id: "u2".into(),
            after_id: "u2".into(),
            pre_tokens: 1,
            post_tokens: 1,
            transcript_path: "t.md".into(),
            created_at: "x".into(),
        };
        let view = build_view(&msgs, Some(&rec));
        assert_eq!(view.len(), 2);
        assert!(view[0]["content"].as_str().unwrap().contains("【上下文已压缩】"));
        assert_eq!(view[1]["id"], json!("u2"));
        // 锚点被回滚删掉 → 记录作废，视图=全量
        let broken = CompactRecord { tail_start_id: "gone".into(), ..rec.clone() };
        assert_eq!(build_view(&msgs, Some(&broken)), msgs);
    }
}
