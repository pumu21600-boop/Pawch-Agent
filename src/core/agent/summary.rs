use super::*;

/// 快照用：去掉 system 头。tool 消息里的多模态图片一并剥掉——截图 base64 只活在
/// 请求上下文（computer 工具回填那一刻），落盘会话文件会被撑爆（一张 1080p PNG
/// 的 base64 上百 KB）。会话文件永远存完整历史（2026-11-07 静默压缩起）。
pub(crate) fn snapshot_msgs(msgs: &[Value]) -> Vec<Value> {
    msgs.iter()
        .skip(1)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_keeps_tool_image_out_but_text_in() {
        let msgs = vec![
            json!({ "role": "system", "content": "sys" }),
            json!({ "role": "user", "content": "截图看看" }),
            json!({ "role": "assistant", "content": "", "tool_calls": [
                { "id": "c1", "type": "function", "function": { "name": "computer", "arguments": "{}" } }
            ] }),
            json!({ "role": "tool", "tool_call_id": "c1", "content": [
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAAA" } },
                { "type": "text", "text": "屏幕上有一个按钮" }
            ] }),
        ];
        let snap = snapshot_msgs(&msgs);
        assert_eq!(snap.len(), 3, "system 头剥掉");
        let tool = snap.last().unwrap();
        assert_eq!(tool["content"], json!("屏幕上有一个按钮"), "工具结果只留 text");
    }
}
