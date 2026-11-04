use super::*;

pub(crate) fn is_summary_msg(m: &Value) -> bool {
    // 必须同时是 system：只看内容前缀会误删「恰好以【历史摘要】开头」的
    // user/tool 消息（读到一个这样的文件就会破坏 tool_calls 配对；2026-11-02 审计）。
    m.get("role").and_then(Value::as_str) == Some("system")
        && m.get("content")
            .and_then(Value::as_str)
            .map(|c| c.starts_with(SUMMARY_PREFIX))
            .unwrap_or(false)
}

/// 快照用：去掉 system 头与摘要消息（摘要由会话文件 summary 字段单独持久化）。
/// tool 消息里的多模态图片一并剥掉——截图 base64 只活在请求上下文（computer 工具
/// 回填那一刻），落盘会话文件会被撑爆（一张 1080p PNG 的 base64 上百 KB）。
pub(crate) fn snapshot_msgs(msgs: &[Value]) -> Vec<Value> {
    msgs.iter()
        .skip(1)
        .filter(|m| !is_summary_msg(m))
        .map(|m| {
            if m.get("role").and_then(Value::as_str) != Some("tool") || !m.get("content").is_some_and(Value::is_array) {
                return m.clone();
            }
            let text: String = m["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            json!({ "role": "tool", "tool_call_id": m.get("tool_call_id").cloned().unwrap_or(json!("")), "content": text })
        })
        .collect()
}

/// 折叠内容消化成喂给摘要模型的文本（纯函数，可测）：
/// 每条消息头尾采样（compress 同款 60/40，不再是只砍头 500——工具结果的关键结论
/// 常在尾部）；总输入预算 12000 字符，超出截断并标注（对齐 ZCode auto-compact
/// 的 handoff 输入纪律：宁可有损也不能把摘要调用本身撑爆）。
pub(crate) fn build_fold_digest(prior: &Option<String>, folded: &[Value]) -> String {
    let mut text = String::new();
    if let Some(p) = prior {
        text.push_str(&format!("已有摘要：{p}\n\n"));
    }
    text.push_str("新增对话：\n");
    for m in folded {
        let role = m.get("role").and_then(Value::as_str).unwrap_or("?");
        let body: String = match m.get("content") {
            Some(Value::String(s)) => crate::context::compress(s, 600),
            Some(Value::Null) | None => String::new(),
            Some(other) => crate::context::compress(&other.to_string(), 600),
        };
        let calls = m
            .get("tool_calls")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|c| c.pointer("/function/name").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default();
        text.push_str(&format!("{role}: {body}{}\n", if calls.is_empty() { String::new() } else { format!(" [调用工具: {calls}]") }));
        if text.chars().count() > 12000 {
            text.push_str("……[折叠内容过长，仅保留前段]\n");
            break;
        }
    }
    text
}

/// 滚动摘要（auto-compact 等价物）：折叠内容 + 旧摘要 → 模型压成结构化交接要点
/// （≤600 字）。失败返回 None（调用方降级；连续失败由调用方熔断，ZCode 同款）。
/// 角色映射：优先走 [roles] summary 专用渠道，失败回落主渠道再降级占位符。
/// 辅助调用记用量台账（purpose=summary），归属会话由 sid 带入。
pub(crate) async fn summarize_folded(
    gateway: &Arc<Gateway>,
    primary: &Channel,
    summary_role: Option<&Channel>,
    prior: &Option<String>,
    folded: &[Value],
    sid: &str,
) -> Option<String> {
    let text = build_fold_digest(prior, folded);
    let req = json!([
        {"role": "system", "content": "你是对话历史压缩器。把给定对话压成不超过600字的中文交接摘要，\
         按以下结构输出（无内容的小节省略）：\
         「任务目标」「已完成与结果」「重要决定」「涉及文件与状态」「未完成与下一步」。\
         必须保留：改过哪些文件、关键参数与路径、用户明确的要求。只输出要点正文，不要开场白。"},
        {"role": "user", "content": text},
    ]);
    // 辅助调用固定小输出；600 字交接预算给足（上一版 400 配 300 字，长任务交接太薄）
    let params = ChatParams { max_tokens: Some(800), ..primary.params.clone() };
    let mut out = None;
    // 角色渠道优先（参数用角色的；无角色渠道则直接主渠道）
    if let Some(role) = summary_role {
        let rp = ChatParams { max_tokens: Some(800), ..role.params.clone() };
        let ch = Channel { params: rp, ..role.clone() };
        let ctx = UsageCtx::aux(sid, "summary");
        out = gateway.complete(&ctx, &ch.base_url, &ch.model, &ch.api_key, &req, &ch.params).await.ok();
    }
    // 回落主渠道
    if out.is_none() {
        let ch = Channel { params, ..primary.clone() };
        let ctx = UsageCtx::aux(sid, "summary");
        out = gateway.complete(&ctx, &ch.base_url, &ch.model, &ch.api_key, &req, &ch.params).await.ok();
    }
    let out = out?;
    let out = out.trim().to_string();
    (!out.is_empty()).then_some(out)
}

/// TITLE 角色：用最终回答生成 ≤20 字会话标题。失败返回 None（保留机械截断标题）。
/// 辅助调用记用量台账（purpose=title）。
pub(crate) async fn generate_title(gateway: &Arc<Gateway>, ch: &Channel, answer: &str, sid: &str) -> Option<String> {
    let text: String = answer.chars().take(1200).collect();
    let req = json!([
        {"role": "system", "content": "根据对话内容生成一个不超过20字的中文会话标题。只输出标题本身，不要引号、句号或任何解释。"},
        {"role": "user", "content": text},
    ]);
    let params = ChatParams { max_tokens: Some(50), ..ch.params.clone() };
    let ctx = UsageCtx::aux(sid, "title");
    let out = gateway.complete(&ctx, &ch.base_url, &ch.model, &ch.api_key, &req, &params).await.ok()?;
    let mut t = out.trim().trim_matches(|c| c == '"' || c == '“' || c == '”' || c == '「' || c == '」').to_string();
    // 防跑偏：截到 30 字符以内（标题超长说明模型没听话，硬截断）
    t = t.chars().take(30).collect();
    (!t.is_empty()).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_digest_samples_head_and_tail() {
        // 旧版只砍头 500 字符——工具结果的关键结论常在尾部，采样必须头尾都留
        let long_tool = format!("{}关键结论在尾部{}", "a".repeat(900), "DONE-42");
        let folded = vec![
            json!({ "role": "user", "content": "把登录接口改成 JWT 并补校验" }),
            json!({ "role": "assistant", "content": null, "tool_calls": [
                { "id": "c1", "type": "function", "function": { "name": "edit_file", "arguments": "{}" } }
            ] }),
            json!({ "role": "tool", "tool_call_id": "c1", "content": long_tool }),
        ];
        let digest = build_fold_digest(&None, &folded);
        assert!(!digest.contains("已有摘要"), "无旧摘要时不该有空前缀");
        assert!(digest.contains("把登录接口改成 JWT"));
        assert!(digest.contains("[调用工具: edit_file]"), "工具名要带上: {digest}");
        assert!(digest.contains("DONE-42"), "尾部关键结论必须保留（采样头尾）: {}", &digest[digest.len()-200..]);
    }

    #[test]
    fn fold_digest_carries_prior_and_caps_input() {
        let prior = Some("旧摘要：任务进行到一半".to_string());
        // 40 条 8k 消息 → 超 12000 字符预算，必须截断并标注
        let folded: Vec<Value> = (0..40)
            .map(|i| json!({ "role": "user", "content": format!("msg-{i}-{}", "m".repeat(8000)) }))
            .collect();
        let digest = build_fold_digest(&prior, &folded);
        assert!(digest.starts_with("已有摘要：旧摘要：任务进行到一半"), "旧摘要要在最前: {}", &digest[..60]);
        assert!(digest.contains("[折叠内容过长，仅保留前段]"), "超预算要标注截断");
        assert!(digest.chars().count() < 14000, "输入预算 12000 + 收尾标注");
        assert!(digest.contains("msg-0-"), "前段消息保留");
    }
}

#[cfg(test)]
mod summary_role_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_system_summary_is_filtered() {
        assert!(is_summary_msg(&json!({ "role": "system", "content": "【历史摘要】x" })));
        assert!(
            !is_summary_msg(&json!({ "role": "tool", "content": "【历史摘要】x" })),
            "工具结果以这个前缀开头时不能误删（会破坏 tool_calls 配对）"
        );
        assert!(!is_summary_msg(&json!({ "role": "user", "content": "【历史摘要】x" })));
    }
}
