//! batch：一次调用按顺序跑多步子调用（「一次调用连做多步」）。
//!
//! 为什么要有：真机日志里一轮 54 秒，其中约 50 秒是等模型自己的下一轮（每次往返
//! 2–5.8 秒）。动作本身是确定的连招（点输入框 → 输入 → 回车），一步一往返纯属浪费。
//!
//! 边界：
//! - **串行**、某步失败立刻停，已做的如实报，绝不假装全成；
//! - 白名单：只读工具 + 文件读写 + computer + browser。shell/git/settings/plugin/
//!   skill/session/auto/subagent/memory 不进批（花销、审批、任意命令、不可预期时长）；
//! - 不允许嵌套 batch（防递归与「一层层看不清」）；
//! - 审批按**整包最高风险**算（core/rules.rs 里逐步骤取 max），一次问清；
//! - 图片只保留最后一张（每张都是 token），其余只记一句「第 N 步的截图略去」。

use serde_json::{json, Value};

use super::{execute_one, ExecConfig};

/// 一次最多几步。再多就该拆成两轮——批太长一旦中途失败，前面的效果也难回溯。
pub const MAX_STEPS: usize = 8;

/// 允许进批的工具。理由写在模块注释里。
fn allowed(tool: &str) -> bool {
    matches!(
        tool,
        "read_file"
            | "read_page"
            | "read_skill"
            | "search"
            | "web_fetch"
            | "web_search"
            | "write_file"
            | "edit_file"
            | "computer"
            | "browser"
            // 2026-11-03：任务清单更新进批（连做多步时步进汇报）
            | "todo"
    )
}

/// 校验并展开步骤。**先把整包校验完再动手**——不能跑到第 5 步才说名字打错。
fn plan_steps(v: &Value) -> Result<Vec<(String, String)>, String> {
    let steps = v
        .get("steps")
        .and_then(Value::as_array)
        .ok_or("batch 需要 steps 数组：[{\"tool\":\"computer\",\"args\":{...}}, …]")?;
    if steps.is_empty() {
        return Err("steps 不能为空".into());
    }
    if steps.len() > MAX_STEPS {
        return Err(format!("一次最多 {MAX_STEPS} 步（给了 {}），拆成两次调用", steps.len()));
    }
    let mut plan = Vec::with_capacity(steps.len());
    for (i, s) in steps.iter().enumerate() {
        let tool = s.get("tool").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if tool.is_empty() {
            return Err(format!("第 {} 步缺 tool", i + 1));
        }
        if tool == "batch" {
            return Err("batch 不能嵌套 batch".into());
        }
        if !allowed(&tool) {
            return Err(format!(
                "{tool} 不能进 batch（可批：read_file/read_page/read_skill/search/web_fetch/web_search/write_file/edit_file/computer/browser/todo；ask 提问要单独调用）"
            ));
        }
        let args = s.get("args").cloned().unwrap_or_else(|| json!({}));
        if !args.is_object() {
            return Err(format!("第 {} 步的 args 要是对象", i + 1));
        }
        plan.push((tool, args.to_string()));
    }
    Ok(plan)
}

/// 一步的结果压成一行（多行输出保留关键信息、别把回包撑爆）。
fn one_line(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = flat.chars().take(max).collect();
    if flat.chars().count() > max {
        format!("{cut}…")
    } else {
        cut
    }
}

pub async fn run(cfg: &ExecConfig, args: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(args).map_err(|e| format!("参数解析失败: {e}"))?;
    let plan = plan_steps(&v)?;
    let total = plan.len();
    let mut lines: Vec<String> = Vec::new();
    let mut done = 0usize;
    let mut failed: Option<String> = None;
    let mut image: Option<(usize, String)> = None;
    let mut dropped = 0usize;
    for (i, (tool, a)) in plan.iter().enumerate() {
        match execute_one(cfg, tool, a).await {
            Ok(mut content) => {
                if let Some(b64) = crate::computer::take_image(&mut content) {
                    if image.is_some() {
                        dropped += 1;
                    }
                    image = Some((i + 1, b64));
                }
                done += 1;
                lines.push(format!("{}. ✅ {tool} — {}", i + 1, one_line(&content, 280)));
            }
            Err(e) => {
                lines.push(format!("{}. ❌ {tool} — {}", i + 1, one_line(&e, 320)));
                failed = Some(format!("第 {} 步失败，后面的 {} 步没有执行", i + 1, total - i - 1));
                break;
            }
        }
    }
    let head = match &failed {
        None => format!("一次跑完 {total} 步，全部成功。"),
        Some(why) => format!("{done}/{total} 步成功，{why}。"),
    };
    let mut out = format!("batch {head}\n{}\n", lines.join("\n"));
    if let Some((step, _)) = &image
        && *step != done
    {
        out.push_str(&format!("（第 {step} 步的截图见下）"));
    }
    if dropped > 0 {
        out.push_str(&format!("（另有 {dropped} 张中间截图略去，只留最后一张）"));
    }
    out.push('\n');
    match image {
        Some((_, b64)) => Ok(format!("{out}{IMG_OPEN}{b64}{IMG_CLOSE}", IMG_OPEN = crate::computer::IMG_OPEN, IMG_CLOSE = crate::computer::IMG_CLOSE)),
        None => Ok(out),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_rejects_bad_shapes() {
        assert!(plan_steps(&json!({})).unwrap_err().contains("steps"));
        assert!(plan_steps(&json!({"steps": []})).unwrap_err().contains("不能为空"));
        assert!(plan_steps(&json!({"steps": [{"args": {}}]})).unwrap_err().contains("缺 tool"));
        assert!(plan_steps(&json!({"steps": [{"tool": "batch", "args": {}}]}))
            .unwrap_err()
            .contains("嵌套"));
        // 花销/任意命令类一律不进批
        for t in ["shell", "git", "settings", "subagent_spawn", "memory", "plugin"] {
            let err = plan_steps(&json!({"steps": [{"tool": t, "args": {}}]})).unwrap_err();
            assert!(err.contains("不能进 batch"), "{t} 应被拒：{err}");
        }
        let many: Vec<Value> = (0..MAX_STEPS + 1).map(|_| json!({"tool": "computer", "args": {}})).collect();
        assert!(plan_steps(&json!({"steps": many})).unwrap_err().contains("最多"));
    }

    #[test]
    fn plan_keeps_order_and_defaults_args() {
        let plan = plan_steps(&json!({"steps": [
            {"tool": "computer", "args": {"method": "screen_size"}},
            {"tool": "read_file"}
        ]}))
        .unwrap();
        assert_eq!(plan[0].0, "computer");
        assert!(plan[0].1.contains("screen_size"));
        assert_eq!(plan[1].0, "read_file");
        assert_eq!(plan[1].1, "{}", "缺 args 按空对象走");
    }

    #[test]
    fn one_line_flattens_and_clips() {
        assert_eq!(one_line("a\nb   c", 100), "a b c");
        assert!(one_line(&"x".repeat(50), 10).ends_with('…'));
        assert_eq!(one_line("  ", 10), "");
    }
}
