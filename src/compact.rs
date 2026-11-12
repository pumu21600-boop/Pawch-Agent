//! 会话静默压缩（2026-11-07）：可调阈值的收尾整卷压缩。
//!
//! 触发只有一条：上下文总量（渠道实报优先，估算兜底）≥ 窗口 × 压缩线。
//! 本轮 done 之后在后台做，全程静默不打断；完成后在会话里留一行
//! 「————压缩后上下文 Nk————」。机制对齐 ZCode auto-compact 的保留策略：
//! 最近一轮原文照搬 + 9 段结构化摘要 + 完整记录文件可回查。
//!
//! 三道「不丢上下文」的保险：
//! 1. 摘要固定 9 段，全部用户消息逐条原文保留，安全类约束原样保留；
//! 2. 被压缩的那段全文落盘成 markdown（transcripts），摘要消息里带路径，
//!    模型要精确细节可以自己回去读；
//! 3. 会话文件存完整历史，压缩只影响请求侧（历史不再被压缩后的列表覆写）。

use serde_json::{json, Value};

/// 摘要正文的设计上限（tokens）：9 段结构给足余量，超长由模型截断。
/// 实际请求的 max_tokens 在此之上再加思考余量（见 server/compact.rs）——
/// 有些模型在「不发思考参数」时仍会默认思考，正文预算要扣掉思考量。
pub const SUMMARY_MAX_TOKENS: u64 = 4000;
/// 单次压缩调用的硬超时。
pub const COMPACT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
/// 连续失败熔断：渠道挂了别每轮收尾都白打一次大请求。
pub const MAX_FAIL_STREAK: u32 = 3;

/// 一条压缩记录：请求视图 = 摘要消息 + tail_start_id 起的历史。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CompactRecord {
    pub summary: String,
    /// 被摘要覆盖的最后一条消息 id（边界前）
    pub summarized_last_id: String,
    /// 保留尾段的第一条消息 id
    pub tail_start_id: String,
    /// 触发时该轮最后一条消息 id（标记行插在它后面）
    pub after_id: String,
    pub pre_tokens: u64,
    pub post_tokens: u64,
    pub transcript_path: String,
    pub created_at: String,
}

/// 核心循环用的视图参数（record 的请求侧投影）。
#[derive(Debug, Clone)]
pub struct CompactView {
    pub summary: String,
    pub transcript_path: String,
    pub tail_start_id: String,
}

impl CompactView {
    pub fn from_record(r: &CompactRecord) -> Self {
        Self { summary: r.summary.clone(), transcript_path: r.transcript_path.clone(), tail_start_id: r.tail_start_id.clone() }
    }
}

/// 请求侧的摘要消息（user 角色，ZCode continuation 同位）。内容只由
/// summary + transcript 路径决定 → 跨轮逐字节稳定，不破坏缓存前缀。
pub fn summary_message(view: &CompactView) -> Value {
    json!({
        // 稳定 id：二次压缩在「请求视图」上切分时，摘要消息要能当锚点。旧实现无 id
        // → 压缩后再聊一轮切分必失败（not_triggered 报「锚点缺 id」，看不出真实
        // 原因；2026-11-10）。id 由尾段起点派生，同一记录跨轮不变。
        "id": format!("compact-summary:{}", view.tail_start_id),
        "role": "user",
        "content": format!(
            "【上下文已压缩】此前的对话已压缩为下面的摘要。需要精确细节（完整代码、报错原文）时可读完整记录文件：{}\n\n{}\n\n（接下来是压缩前保留的最近对话原文）",
            view.transcript_path, view.summary
        ),
    })
}

/// 在消息列表里按 id 找索引。
fn idx_by_id(msgs: &[Value], id: &str) -> Option<usize> {
    if id.is_empty() {
        return None;
    }
    msgs.iter().position(|m| m.get("id").and_then(Value::as_str) == Some(id))
}

/// 给核心循环：把全量消息按压缩记录切成请求视图。
/// `msgs` 含下标 0 的 system；返回同形状（system 照搬，摘要消息插在它后面）。
/// 尾起点找不到（被回滚/删掉）→ 记录作废，原样返回。
pub fn build_request(msgs: &[Value], view: Option<&CompactView>) -> Vec<Value> {
    let Some(v) = view else { return msgs.to_vec() };
    let Some(i) = idx_by_id(msgs, &v.tail_start_id) else {
        return msgs.to_vec();
    };
    if i == 0 {
        return msgs.to_vec(); // 尾起点压到 system 上：记录不合法，作废
    }
    let mut out = Vec::with_capacity(msgs.len() + 1);
    out.push(msgs[0].clone());
    out.push(summary_message(v));
    out.extend_from_slice(&msgs[i..]);
    out
}

/// 触发时的切分：请求视图（不含 system）里最后一条**真实**用户消息为界，
/// 之前的进摘要、从它起（含）整段原文保留。display_only / 标记行不参与。
/// 返回 (split_idx, summarized_last_id, tail_start_id, after_id)。
pub fn split_for_summary(view: &[Value]) -> Option<(usize, String, String, String)> {
    let real = |m: &Value| {
        !m.get("display_only").and_then(Value::as_bool).unwrap_or(false)
            && m.get("kind").and_then(Value::as_str) != Some("compact_marker")
    };
    let split = view
        .iter()
        .rposition(|m| real(m) && m.get("role").and_then(Value::as_str) == Some("user"))?;
    if split == 0 {
        return None; // 第一条就是最后一条用户消息：没有可压的东西
    }
    let summarized_last = view[..split].iter().rev().find(|m| real(m))?;
    let tail_start = view[split..].iter().find(|m| real(m))?;
    let after = view.iter().rev().find(|m| real(m))?;
    let id = |m: &Value| m.get("id").and_then(Value::as_str).unwrap_or("").to_string();
    let (s_id, t_id, a_id) = (id(summarized_last), id(tail_start), id(after));
    if s_id.is_empty() || t_id.is_empty() || a_id.is_empty() {
        return None; // 锚点缺 id（老会话还没被新 save 升级）→ 不压，等下次
    }
    Some((split, s_id, t_id, a_id))
}

/// 摘要里必须出现 assistant（没有回答的纯问答轮没东西可压）。
pub fn summarized_has_assistant(view: &[Value], split: usize) -> bool {
    view[..split]
        .iter()
        .any(|m| m.get("role").and_then(Value::as_str) == Some("assistant"))
}

/// 压缩后请求视图的完整估价——**与圆环同一个函数、同一把尺子**：摘要消息 +
/// 保留尾段（调用方已按发送侧同一套卫生净化）+ 系统提示 + 工具定义。
/// 「压缩后上下文 Nk」标记行和紧随其后的圆环都取它的 `total()`，两个数不会再打架
/// （真机 2026-11-07：标记行漏数思考报 29k、圆环按真实请求口径是 53k）。
pub fn post_breakdown(
    view: &[Value],
    system_tokens: u64,
    tools_schema_tokens: u64,
    count_reasoning: crate::context::ReasoningCount,
) -> crate::context::Breakdown {
    let mut b = crate::context::estimate_breakdown(view, count_reasoning);
    b.system = system_tokens;
    b.tools_schema = tools_schema_tokens;
    b
}

/// 压缩线（tokens）。窗口 0（模型没配上下文长度且全局默认为 0）= 不压。
pub fn threshold_tokens(window: u64, percent: u8) -> u64 {
    let p = percent.clamp(40, 85) as u64;
    window.saturating_mul(p) / 100
}

/// 9 段结构化摘要提示词（对齐 ZCode compact 的保留纪律，中文承载）。
/// 红线：不注入身份/人设；这是纯任务交接约束。
pub const COMPACT_PROMPT: &str = r#"你是对话历史压缩器。把上面的对话压成一份结构化交接摘要，只输出摘要正文（纯文本，分节编号），不要开场白和解释。

必须包含以下 9 节（无内容的小节省略，编号保持）：
1. 用户原始意图：用户明确要求了什么，逐条列出
2. 关键技术概念：涉及的技术、框架、约束
3. 文件与代码：改过/读过哪些文件（带路径），关键代码片段保留原文
4. 错误与修复：遇到过什么错、怎么修的；用户明确纠正过的做法要重点记录
5. 问题解决：已解决什么、还在排查什么
6. 全部用户消息：把所有用户消息逐条原文列出（工具结果不算用户消息）
7. 待办：明确答应过还没做的事
8. 当前工作：压缩前正在做什么，做到哪一步
9. 下一步：接下来该做什么；必须与用户最近的明确要求一致，引用原话防止跑偏

硬性要求：安全相关的用户约束（不能碰的文件、不能执行的操作、密钥处理规则）必须原样保留；文件路径、关键参数、报错原文不得改写；宁可贵一点也不要丢事实。"#;

/// 把进摘要的消息清成模型可吃的最小形状：剥 display_only/标记行/图片/思考/界面字段，
/// 保住 role + content + tool_calls/tool_call_id 配对。
pub fn clean_for_summary(msgs: &[Value]) -> Vec<Value> {
    let cleaned: Vec<Value> = msgs
        .iter()
        .filter(|m| {
            !m.get("display_only").and_then(Value::as_bool).unwrap_or(false)
                && m.get("kind").and_then(Value::as_str) != Some("compact_marker")
        })
        .map(|m| {
            let role = m.get("role").and_then(Value::as_str).unwrap_or("user").to_string();
            let mut out = json!({ "role": role, "content": text_content(m) });
            if role == "assistant"
                && let Some(calls) = m.get("tool_calls").and_then(Value::as_array)
            {
                let calls: Vec<Value> = calls
                    .iter()
                    .filter_map(|c| {
                        let name = c.pointer("/function/name")?.as_str()?.to_string();
                        let args = c.pointer("/function/arguments").cloned().unwrap_or(json!("{}"));
                        Some(json!({
                            "id": c.get("id").cloned().unwrap_or(json!("")),
                            "type": "function",
                            "function": { "name": name, "arguments": args },
                        }))
                    })
                    .collect();
                if !calls.is_empty() {
                    out["tool_calls"] = Value::Array(calls);
                }
            }
            if role == "tool" && let Some(id) = m.get("tool_call_id") {
                out["tool_call_id"] = id.clone();
            }
            out
        })
        .collect::<Vec<Value>>();
    // 合并连续同角色消息（只合并 user/assistant 且不带 tool_calls/tool_call_id 的）：
    // 「引导插话跟在未回复的用户消息后」「错误轮无 assistant」都会产生连续 user，
    // Anthropic Messages 对连续 user 直接 400 → 压缩白跑还计熔断（2026-11-08 审计）。
    // tool 消息不并（tool_call_id 配对不能断），带 tool_calls 的 assistant 不并。
    let mut merged: Vec<Value> = Vec::with_capacity(cleaned.len());
    for m in cleaned {
        let can_merge = matches!(m.get("role").and_then(Value::as_str), Some("user") | Some("assistant"))
            && m.get("tool_calls").is_none()
            && m.get("tool_call_id").is_none();
        if can_merge
            && let Some(last) = merged.last_mut()
            && last.get("role").and_then(Value::as_str) == m.get("role").and_then(Value::as_str)
            && last.get("tool_calls").is_none()
            && last.get("tool_call_id").is_none()
        {
            let text = |v: &Value| v.get("content").and_then(Value::as_str).unwrap_or("").to_string();
            let joined = format!("{}\n\n{}", text(last), text(&m));
            last["content"] = json!(joined);
            continue;
        }
        merged.push(m);
    }
    merged
}

/// 文本化 content：数组只留 text 部分（图片/音频不进摘要请求）。
fn text_content(m: &Value) -> Value {
    match m.get("content") {
        Some(Value::Array(parts)) => {
            let text: String = parts
                .iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            if text.is_empty() {
                json!("[附件]")
            } else {
                json!(text)
            }
        }
        Some(Value::String(s)) => json!(s),
        Some(Value::Null) | None => json!(""),
        Some(other) => other.clone(),
    }
}

/// 被压缩段的完整记录（markdown）。保留原文级别的细节：工具名 + 参数 + 结果全文。
pub fn render_transcript(view: &[Value], split: usize) -> String {
    let mut out = String::new();
    for m in &view[..split] {
        let role = m.get("role").and_then(Value::as_str).unwrap_or("?");
        match role {
            "user" => {
                out.push_str(&format!("## 用户\n\n{}\n\n", text_content(m).as_str().unwrap_or("")));
            }
            "assistant" => {
                out.push_str(&format!("## 助手\n\n{}\n\n", text_content(m).as_str().unwrap_or("")));
                if let Some(calls) = m.get("tool_calls").and_then(Value::as_array) {
                    for c in calls {
                        let name = c.pointer("/function/name").and_then(Value::as_str).unwrap_or("?");
                        let args = c.pointer("/function/arguments").and_then(Value::as_str).unwrap_or("{}");
                        out.push_str(&format!("- 调用工具 `{name}`，参数：`{args}`\n"));
                    }
                    out.push('\n');
                }
            }
            "tool" => {
                let id = m.get("tool_call_id").and_then(Value::as_str).unwrap_or("");
                out.push_str(&format!("### 工具结果（{id}）\n\n```\n{}\n```\n\n", text_content(m).as_str().unwrap_or("")));
            }
            _ => {}
        }
    }
    out
}

/// 标记行（落盘进会话消息，display_only：发送侧剥离，前端渲染成分隔线）。
pub fn marker_message(text: &str) -> Value {
    json!({
        "role": "system",
        "content": text,
        "kind": "compact_marker",
        "display_only": true,
        "ts": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
    })
}

/// 完成态标记行文案：用户定的格式「————压缩后上下文xxxxk————」。
pub fn done_marker_text(post_tokens: u64) -> String {
    format!("————压缩后上下文 {}k————", post_tokens.saturating_add(999) / 1000)
}

pub const RUNNING_MARKER_TEXT: &str = "————正在压缩上下文————";

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: &str, role: &str, content: &str) -> Value {
        json!({ "id": id, "role": role, "content": content })
    }

    #[test]
    fn build_request_inserts_summary_after_system() {
        let msgs = vec![
            json!({"role":"system","content":"sys"}),
            msg("u1", "user", "第一句"),
            msg("a1", "assistant", "第一答"),
            msg("u2", "user", "第二句"),
        ];
        let view = CompactView { summary: "摘要".into(), transcript_path: "t.md".into(), tail_start_id: "u2".into() };
        let req = build_request(&msgs, Some(&view));
        assert_eq!(req.len(), 3);
        assert_eq!(req[0]["role"], json!("system"));
        assert_eq!(req[1]["role"], json!("user"));
        assert!(req[1]["content"].as_str().unwrap().contains("【上下文已压缩】"));
        assert!(req[1]["content"].as_str().unwrap().contains("摘要"));
        assert!(req[1]["content"].as_str().unwrap().contains("t.md"));
        assert_eq!(req[2]["id"], json!("u2"), "尾段从 tail_start 起");
    }

    #[test]
    fn build_request_void_when_tail_missing() {
        let msgs = vec![json!({"role":"system","content":"sys"}), msg("u1", "user", "x")];
        let view = CompactView { summary: "s".into(), transcript_path: "t".into(), tail_start_id: "gone".into() };
        assert_eq!(build_request(&msgs, Some(&view)), msgs, "锚点不在 = 记录作废");
    }

    #[test]
    fn split_picks_last_real_user_and_skips_markers() {
        let view = vec![
            msg("u1", "user", "q1"),
            msg("a1", "assistant", "ans1"),
            marker_message(RUNNING_MARKER_TEXT),
            msg("u2", "user", "q2"),
            msg("a2", "assistant", "ans2"),
        ];
        let (split, s_last, t_start, after) = split_for_summary(&view).unwrap();
        assert_eq!(split, 3, "最后一条真实 user 是 u2（下标 3）");
        assert_eq!(s_last, "a1");
        assert_eq!(t_start, "u2");
        assert_eq!(after, "a2");
        assert!(summarized_has_assistant(&view, split));
    }

    #[test]
    fn split_needs_ids_and_prior_history() {
        // 全部消息都没有 id：宁可不压
        let view = vec![json!({"role":"user","content":"a"}), json!({"role":"assistant","content":"b"}), json!({"role":"user","content":"c"})];
        assert!(split_for_summary(&view).is_none());
        // 只有一条用户消息：没东西可压
        let one = vec![msg("u1", "user", "q"), msg("a1", "assistant", "ans")];
        assert!(split_for_summary(&one).is_none(), "split==0 必须拒");
    }

    #[test]
    fn post_breakdown_uses_ring_yardstick_for_every_channel() {
        let view = vec![
            summary_message(&CompactView { summary: "摘要".into(), transcript_path: "t.md".into(), tail_start_id: "u2".into() }),
            json!({"role":"assistant","content":"答","reasoning_content":"想".repeat(500)}),
            json!({"role":"user","content":"继续"}),
        ];
        let replay = post_breakdown(&view, 5000, 9000, crate::context::ReasoningCount::All).total();
        let strip = post_breakdown(&view, 5000, 9000, crate::context::ReasoningCount::None).total();
        assert!(
            replay >= strip + 500,
            "重发思考的渠道（DeepSeek/zai/白名单）必须把 thinking 算进标记行：replay={replay} strip={strip}"
        );
        assert!(strip >= 14_000, "系统提示 + 工具定义必须计入：strip={strip}");
    }

    #[test]
    fn clean_for_summary_strips_ui_fields_and_media() {
        let msgs = vec![
            json!({"role":"system","content":"[通知] x","display_only":true}),
            marker_message("marker"),
            json!({"role":"user","content":[{"type":"image_url","image_url":{"url":"data:.."}},{"type":"text","text":"看图"}],"ts":1}),
            json!({"role":"assistant","content":"好","reasoning_content":"想","tool_calls":[{"id":"c1","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"a\"}"}}],"stats":{}}),
            json!({"role":"tool","tool_call_id":"c1","content":[{"type":"image_url","image_url":{"url":"data:.."}},{"type":"text","text":"结果"}]}),
        ];
        let out = clean_for_summary(&msgs);
        assert_eq!(out.len(), 3, "display_only 与标记行剥掉");
        assert_eq!(out[0]["content"], json!("看图"));
        assert!(out[1].get("reasoning_content").is_none());
        assert!(out[1].get("stats").is_none());
        assert_eq!(out[1]["tool_calls"][0]["function"]["name"], json!("read_file"));
        assert_eq!(out[2]["content"], json!("结果"), "工具结果只留 text");
        assert!(out[2].get("ts").is_none());
    }

    #[test]
    fn transcript_keeps_tool_details() {
        let view = vec![
            msg("u1", "user", "读配置"),
            json!({"role":"assistant","content":"好","tool_calls":[{"id":"c1","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"a\"}"}}]}),
            msg("c1", "tool", "文件内容"),
        ];
        let md = render_transcript(&view, 3);
        assert!(md.contains("## 用户"));
        assert!(md.contains("read_file"));
        assert!(md.contains("{\"path\":\"a\"}"));
        assert!(md.contains("文件内容"));
    }

    #[test]
    fn threshold_clamps_percent() {
        assert_eq!(threshold_tokens(1_000_000, 85), 850_000);
        assert_eq!(threshold_tokens(1_000_000, 200), 850_000, "超范围钳到 85");
        assert_eq!(threshold_tokens(1_000_000, 10), 400_000, "低于 40 钳到 40");
        assert_eq!(threshold_tokens(0, 85), 0, "窗口未知 = 不压");
    }
}
