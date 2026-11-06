use super::*;

/// 任务结束时的改动统计条：先补一次「结束快照」提交（让任务内新建文件也进影子仓库），
/// 再算相对基线的 numstat；无改动不发。撤销 = 影子仓库 reset --hard 回基线。
///
/// 同一时机写**会话产物账本**（`artifacts` 模块）：从基线到结束快照的改动里，
/// 只保留 mtime 落在本轮任务时间窗内的路径。不能把「基线到当前状态」整个当产物——
/// 会话起点可能是一天前的旧快照，期间别的会话写进来的文件会被算成这个会话的
/// （2026-11-06 校园网会话里冒出便利店 index.html/截图，实测）。
pub(crate) async fn emit_checkpoint_bar(
    baseline: Option<String>,
    session_id: &str,
    // 任务钉住的工作区（2026-11-02 审计：原实现用全局单例，后台任务/中途切项目会串目录）
    ws: &std::path::Path,
    // 本轮任务开始时间（墙上时钟）：产物归属的时间窗下界
    task_started: std::time::SystemTime,
    on_event: &(dyn Fn(AgentEvent) + Send + Sync),
) {
    let Some(base) = baseline else { return };
    let ws = ws.to_path_buf();
    let sid = session_id.to_string();
    let res = tokio::task::spawn_blocking({
        let base = base.clone();
        move || -> anyhow::Result<(usize, u64, u64)> {
            recovery::checkpoint(&ws, "任务结束快照", &sid)?;
            // 产物归属：先失败也不影响改动统计条，账本写不进去只记日志
            match recovery::changed_files(&ws, recovery::RepoOrigin::Shadow, &base) {
                Ok(files) => {
                    let paths = crate::artifacts::paths_from_changed(&files, &ws, task_started);
                    if let Err(e) = crate::artifacts::record(&sid, &ws, paths) {
                        observability::event(
                            EventType::Error,
                            json!({ "message": format!("产物账本写入失败: {e}") }),
                        );
                    }
                }
                Err(e) => observability::event(
                    EventType::State,
                    json!({ "kind": "artifacts.skipped", "reason": e.to_string() }),
                ),
            }
            recovery::diff_stat(&ws, &base)
        }
    })
    .await;
    match res {
        Ok(Ok((files, adds, dels))) if files > 0 => {
            observability::event(
                EventType::State,
                json!({ "kind": "checkpoint.summary", "baseline": base, "files": files, "additions": adds, "deletions": dels }),
            );
            on_event(AgentEvent::Checkpoint { id: base, files_changed: files, additions: adds, deletions: dels });
        }
        Ok(Ok(_)) => {}
        Ok(Err(e)) => observability::event(
            EventType::State,
            json!({ "kind": "checkpoint.skipped", "reason": e.to_string() }),
        ),
        Err(e) => observability::event(
            EventType::State,
            json!({ "kind": "checkpoint.skipped", "reason": format!("任务失败: {e}") }),
        ),
    }
}
