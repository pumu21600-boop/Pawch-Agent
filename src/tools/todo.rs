//! todo：任务清单工具（ZCode TodoWrite 同款，2026-11-03 交接清单第 1 项）。
//!
//! 语义照抄 ZCode（源码 apps/zcode-cli/packages/contracts/src/tools/todo.ts +
//! handlers/todo.ts）：整表替换——每次调用发完整清单，空清单=清零。
//! 「至多一个 in_progress」写在描述里但**不硬校验**（ZCode 已把 superRefine 硬拒
//! 注释掉：多子代理并行需要多个 in_progress），这里同款只提醒不拒绝。
//!
//! 状态不落服务端：清单本身作为 tool 调用参数进会话消息流（Snapshot 整卷落盘），
//! 重启后前端按消息里的 tool part 重渲染——与审批记录同一套持久化思路，零额外存储。

use serde_json::{Value};

pub async fn run(args: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(args).map_err(|e| format!("参数解析失败: {e}"))?;
    let items = v
        .get("todos")
        .and_then(Value::as_array)
        .ok_or("todo 需要 todos 数组：[{\"content\":\"…\",\"status\":\"pending\",\"priority\":\"medium\"}, …]")?;
    if items.len() > 100 {
        return Err(format!(
            "清单最多 100 项（给了 {}），拆成阶段性小清单更可用",
            items.len()
        ));
    }
    let (mut pending, mut doing, mut done) = (0usize, 0usize, 0usize);
    let mut lines = Vec::with_capacity(items.len());
    for (i, it) in items.iter().enumerate() {
        let content = it
            .get("content")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if content.is_empty() {
            return Err(format!("第 {} 项缺 content（一句话描述）", i + 1));
        }
        let status = it.get("status").and_then(Value::as_str).unwrap_or("pending");
        if !matches!(status, "pending" | "in_progress" | "completed") {
            return Err(format!(
                "第 {} 项 status 只能是 pending / in_progress / completed（给了 {status}）",
                i + 1
            ));
        }
        let priority = it.get("priority").and_then(Value::as_str).unwrap_or("medium");
        if !matches!(priority, "high" | "medium" | "low") {
            return Err(format!(
                "第 {} 项 priority 只能是 high / medium / low（给了 {priority}）",
                i + 1
            ));
        }
        match status {
            "in_progress" => doing += 1,
            "completed" => done += 1,
            _ => pending += 1,
        }
        lines.push(format!("{}. [{status}][{priority}] {content}", lines.len() + 1));
    }
    let warn = if doing > 1 {
        format!("（提示：同时进行中的有 {doing} 项，尽量保持一项进行中）")
    } else {
        String::new()
    };
    if items.is_empty() {
        return Ok("任务清单已清空。".to_string());
    }
    Ok(format!(
        "任务清单已更新：共 {} 项（进行中 {doing} / 待办 {pending} / 已完成 {done}）{warn}\n{}",
        items.len(),
        lines.join("\n")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn replaces_whole_list_and_counts() {
        let out = run(
            r#"{"todos":[
                {"content":"先读代码","status":"completed","priority":"high"},
                {"content":"写实现","status":"in_progress","priority":"high"},
                {"content":"跑测试","status":"pending","priority":"medium"}
            ]}"#,
        )
        .await
        .unwrap();
        assert!(out.contains("共 3 项"));
        assert!(out.contains("进行中 1 / 待办 1 / 已完成 1"));
        assert!(out.contains("2. [in_progress][high] 写实现"));
    }

    #[tokio::test]
    async fn empty_list_clears() {
        let out = run(r#"{"todos":[]}"#).await.unwrap();
        assert!(out.contains("已清空"));
    }

    #[tokio::test]
    async fn multiple_in_progress_warns_but_accepts() {
        // ZCode 同款：多 in_progress 只提醒不拒绝（并行子代理场景）
        let out = run(
            r#"{"todos":[
                {"content":"a","status":"in_progress","priority":"high"},
                {"content":"b","status":"in_progress","priority":"low"}
            ]}"#,
        )
        .await
        .unwrap();
        assert!(out.contains("同时进行中的有 2 项"));
    }

    #[tokio::test]
    async fn rejects_bad_status_and_missing_content() {
        assert!(run(r#"[{"todos":[{"content":"a","status":"doing","priority":"low"}]}]"#)
            .await
            .is_err());
        assert!(run(r#"[{"todos":[{"status":"pending","priority":"low"}]}]"#).await.is_err());
        assert!(run(r#"{"todos":"x"}"#).await.is_err());
    }
}
