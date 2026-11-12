//! 历史回放净化 —— 移植自 OpenClaw `packages/ai/src/transports/openai-completions-replay.ts`（v2026.8.2）。
//!
//! OpenAI Chat Completions 的 assistant 输入消息没有 reasoning 字段：Stock OpenAI 会在
//! 下一轮请求里拒收；OpenRouter/DeepSeek 式厂商反而要求带回（跨轮思考连续性）。
//! 因此按端点归属决定保留还是剥离，并附带两条消息级 transform：
//! developer 角色改写（OpenAI 新系）与空消息过滤（ModelStudio/Qwen 拒收空 content）。

use serde_json::Value;

use super::compat::{Compat, Protocol, ThinkingFormat};

/// assistant 消息上可能出现的思考回放字段（上游 COMPLETIONS_REASONING_REPLAY_FIELDS）。
pub const REPLAY_FIELDS: [&str; 4] = ["reasoning_details", "reasoning_content", "reasoning", "reasoning_text"];

/// DeepSeek 式 reasoning_content 回放契约的白名单模型（上游 REASONING_CONTENT_REPLAY_MODEL_IDS）。
const REPLAY_MODEL_IDS: [&str; 15] = [
    "deepseek-v4-flash",
    "deepseek-v4-pro",
    "kimi-for-coding",
    "kimi-k2.5",
    "kimi-k2.6",
    "kimi-k2.7-code",
    "kimi-k2.7-code-highspeed",
    "kimi-k3",
    "kimi-k2-thinking",
    "kimi-k2-thinking-turbo",
    "mimo-v2-pro",
    "mimo-v2-omni",
    "mimo-v2.5",
    "mimo-v2.5-pro",
    "mimo-v2.6-pro",
];

/// 渠道在模型 id 后追加的档位后缀（free/paid/trial），剥掉再比对白名单。
const TIER_SUFFIXES: [&str; 3] = ["-free", "-paid", "-trial"];

fn replay_model_id_candidates(model_id: &str) -> Vec<String> {
    let normalized = model_id.trim().to_lowercase();
    if normalized.is_empty() {
        return vec![];
    }
    let mut candidates: Vec<String> = Vec::new();
    let final_part = normalized.rsplit('/').next().unwrap_or(&normalized).to_string();
    candidates.push(final_part.clone());
    let mut colon: Vec<String> = final_part
        .split(':')
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect();
    if colon.len() > 1 {
        candidates.push(colon.remove(0));
        candidates.push(colon.pop().unwrap_or_default());
    }
    let base = candidates.clone();
    for c in base {
        for suffix in TIER_SUFFIXES {
            if c.len() > suffix.len() && c.ends_with(suffix) {
                candidates.push(c[..c.len() - suffix.len()].to_string());
            }
        }
    }
    candidates
}

/// assistant 消息是否应保留 reasoning_content 回放（上游 shouldPreserveReasoningContentReplay）。
/// 核心循环的估算/裁剪也用它对齐口径：会回放思考的渠道思考每轮真发出去、本地预算必须
/// 看见；剥离渠道思考不进下一次请求、不计入。
pub(crate) fn should_preserve_replay(model_id: &str, compat: &Compat) -> bool {
    // Anthropic Messages / Responses 适配器都不回放历史思考：Anthropic 需要带签名的
    // thinking 块（我们只有文本），Responses 的 input 只发正文/工具。估算、压缩守卫、
    // 回滚重算都跟着这个判定走——不能按 provider 的 thinking_format 算，否则圆环会把
    // 根本没发出去的思考计入（2026-11-11 用户实测：真实 2.5w，回滚后显示 3.8w）。
    if matches!(compat.protocol, Protocol::AnthropicMessages | Protocol::OpenAiResponses) {
        return false;
    }
    if compat.requires_reasoning_content_replay || compat.thinking_format == ThinkingFormat::Deepseek {
        return true;
    }
    replay_model_id_candidates(model_id)
        .iter()
        .any(|c| REPLAY_MODEL_IDS.contains(&c.as_str()))
}

/// 估算 / 压缩守卫 / 圆环重算的思考口径：
/// - Anthropic / Responses 用 `Replayable`：只计带签名 / 加密条目的思考（真会发出去）
/// - Chat Completions 按渠道契约（DeepSeek/Kimi/白名单 → `All`，其余 `None`）
pub(crate) fn reasoning_count(compat: &Compat, model_id: &str) -> crate::context::ReasoningCount {
    match compat.protocol {
        Protocol::AnthropicMessages | Protocol::OpenAiResponses => {
            crate::context::ReasoningCount::Replayable
        }
        _ => crate::context::ReasoningCount::from(should_preserve_replay(model_id, compat)),
    }
}

/// 回放来源指纹：协议 + 端点 + 模型。签名/加密内容由端点签发，换端点（同模型名）
/// 也可能对不上；旧消息没有这个字段时按模型名守卫（向后兼容）。
pub fn source_tag(protocol: Protocol, base_url: &str, model: &str) -> String {
    let proto = match protocol {
        Protocol::AnthropicMessages => "anthropic",
        Protocol::OpenAiResponses => "responses",
        Protocol::OpenAiCompletions => "openai",
    };
    format!("{proto}|{}|{}", base_url.trim().trim_end_matches('/'), model.trim())
}

/// 这条消息的思考能否在当前端点回放：模型必须一致；消息带来源指纹时必须与当前一致；
/// 旧消息没有指纹 → 只按模型名判（保持升级前的行为，不让历史会话突然失去回放）。
pub fn reasoning_source_matches(m: &Value, model: &str, current: Option<&str>) -> bool {
    if m.get("reasoning_model").and_then(Value::as_str) != Some(model) {
        return false;
    }
    match (m.get("reasoning_source").and_then(Value::as_str), current) {
        (Some(saved), Some(cur)) => saved == cur,
        _ => true,
    }
}

/// 发送前净化 messages（原位修改）：
/// 1) assistant 消息的思考回放字段——保留（DeepSeek/zai 契约或白名单模型）或剥离（其余）；
/// 2) OpenRouter 形态 reasoning_details → reasoning 字符串合并后再按契约去留；
/// 3) developer 角色改写（上游 supportsDeveloperRole：OpenAI 新系）；
/// 4) 空消息过滤（上游 requiresNonEmptyUserOrAssistantMessage：ModelStudio/Qwen）；
/// 5) display_only 消息剔除：通知/错误等前端气泡随会话落盘只为重启后界面还在，
///    这里统一剥离——两条协议的请求体组装都在本净化之后，一处过滤即全渠道生效。
pub fn apply_outgoing_transforms(messages: &mut Value, model_id: &str, compat: &Compat) {
    let arr = match messages.as_array_mut() {
        Some(a) => a,
        None => return,
    };
    arr.retain(|m| m.get("display_only").and_then(Value::as_bool) != Some(true));
    // 引导/打断消息的前端渲染标记（steered/interrupted）：随会话落盘给界面回放用，
    // 模型不需要也不该看到（2026-10-22）——与 display_only 同一处统一剥。
    for m in arr.iter_mut() {
        if let Some(obj) = m.as_object_mut() {
            obj.remove("steered");
            obj.remove("interrupted");
        }
    }
    // 模型把 <tool_call> 协议当正文吐出来的泄漏（2026-11-01 step-5-preview 死循环实测）：
    // 界面由 md.js 收成「原始工具调用文本」折叠块给人看，但回放给端点时必须剥掉——
    // 否则模型每轮都看见自己泄漏的协议文本，只会继续自我强化地吐。未闭合到文末的
    // 半截一并丢（泄漏的协议文本没有信息量）。落盘与界面回放保留原文不动。
    for m in arr.iter_mut() {
        if m.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        if let Some(obj) = m.as_object_mut()
            && let Some(Value::String(s)) = obj.get("content")
        {
            let cleaned = strip_toolcall_text(s);
            if cleaned != *s {
                obj.insert("content".into(), Value::String(cleaned));
            }
        }
    }
    // Anthropic / Responses 的适配器自己决定回放（签名 thinking 块 / 加密 reasoning
    // 条目）——思考文本必须留到适配器手里，不能按 OpenAI 形状的契约剥离；没有回放
    // 元数据的消息适配器会忽略。OpenAI Chat 形状才沿用渠道契约的去留。
    let preserve = should_preserve_replay(model_id, compat)
        || matches!(compat.protocol, Protocol::AnthropicMessages | Protocol::OpenAiResponses);

    for msg in arr.iter_mut() {
        let is_assistant = msg.get("role").and_then(Value::as_str) == Some("assistant");
        if is_assistant {
            if preserve {
                // reasoning_details(string) → reasoning 合并，其余非法形态删除（上游 sanitize 同款）
                if let Some(obj) = msg.as_object_mut()
                    && let Some(details) = obj.get("reasoning_details") {
                        match details {
                            Value::String(s) => {
                                if !s.is_empty()
                                    && (!obj.get("reasoning").map(Value::is_string).unwrap_or(false))
                                {
                                    obj.insert("reasoning".into(), Value::String(s.clone()));
                                }
                                obj.remove("reasoning_details");
                            }
                            Value::Array(_) => {} // 合法形态：保留
                            _ => {
                                obj.remove("reasoning_details");
                            }
                        }
                    }
            } else if let Some(obj) = msg.as_object_mut() {
                for field in REPLAY_FIELDS {
                    obj.remove(field);
                }
            }
        }
        // developer 改写：OpenAI 新系把 system 称为 developer
        if compat.thinking_format == ThinkingFormat::Openai && !compat_requires_system(compat) {
            // 仅在明确 supports 时改写；本产品保守起见只在 OpenAI 官方格式做
            if is_assistant {
                // no-op
            }
        }
    }

    // developer 改写（放循环外，避免借用冲突）
    if compat.thinking_format == ThinkingFormat::Openai && developer_role_for_model(model_id) {
        for msg in arr.iter_mut() {
            if msg.get("role").and_then(Value::as_str) == Some("system")
                && let Some(obj) = msg.as_object_mut() {
                    obj.insert("role".into(), Value::String("developer".into()));
                }
        }
    }

    // 空消息过滤：ModelStudio/Qwen 拒收空 content 的 user/assistant（无工具调用时）
    if compat.thinking_format == ThinkingFormat::Qwen {
        arr.retain(|msg| {
            let role = msg.get("role").and_then(Value::as_str);
            if !matches!(role, Some("user") | Some("assistant")) {
                return true;
            }
            let empty_content = msg
                .get("content")
                .map(|c| c.as_str().map(str::trim).unwrap_or("").is_empty() || c.is_null())
                .unwrap_or(true);
            let has_tool_calls = msg
                .get("tool_calls")
                .and_then(Value::as_array)
                .map(|a| !a.is_empty())
                .unwrap_or(false);
            let has_tool_call_id = msg.get("tool_call_id").is_some();
            !(empty_content && !has_tool_calls && !has_tool_call_id)
        });
    }
}

/// developer 角色：GPT-5/o 系（OpenAI 新对话模型）。上游按模型目录；本产品按 id 推导。
fn developer_role_for_model(model_id: &str) -> bool {
    let id = model_id.trim().to_lowercase();
    let norm = id.rsplit('/').next().unwrap_or(&id).to_string();
    norm.starts_with("gpt-5") || norm.starts_with("o1") || norm.starts_with("o3") || norm.starts_with("o4")
}

fn compat_requires_system(_compat: &Compat) -> bool {
    false
}

/// 剥掉 assistant 正文里泄漏的 tool_call 协议整块（含开闭标签）。未闭合到文末的
/// 半截一并丢（泄漏的协议文本没有信息量）。只净化发出去的内容；会话落盘/界面
/// 回放保留原文（md.js 负责把它收成折叠块展示）。
fn strip_toolcall_text(s: &str) -> String {
    const OPEN: &str = "<tool_call>";
    const CLOSE: &str = "</tool_call>";
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find(OPEN) {
        out.push_str(&rest[..start]);
        let after = &rest[start + OPEN.len()..];
        match after.find(CLOSE) {
            Some(end) => rest = &after[end + CLOSE.len()..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn openai_compat() -> Compat {
        Compat {
            protocol: crate::gateway::compat::Protocol::OpenAiCompletions,
            thinking_format: ThinkingFormat::Openai,
            max_tokens_field: "max_tokens",
            supports_usage_in_streaming: true,
            supports_temperature: true,
            requires_reasoning_content_replay: false,
            supports_prompt_cache_key: false,
            supports_long_cache_retention: false,
        }
    }

    #[test]
    fn strips_reasoning_for_stock_endpoints() {
        let mut msgs = json!([
            {"role":"user","content":"hi"},
            {"role":"assistant","content":"hello","reasoning_content":"hidden chain"},
            {"role":"user","content":"more"}
        ]);
        apply_outgoing_transforms(&mut msgs, "mystery-model", &openai_compat());
        let a = msgs.as_array().unwrap();
        assert!(a[1].get("reasoning_content").is_none(), "非契约端点应剥离 reasoning_content");
        assert_eq!(a[1]["content"], json!("hello"));
    }

    #[test]
    fn anthropic_keeps_reasoning_text_for_adapter_replay() {
        // Anthropic/Responses 适配器自己回放（签名 thinking 块 / 加密 reasoning item）：
        // 净化阶段不能把 reasoning_content 剥掉，否则适配器拿不到文本、回放失效。
        let mut c = openai_compat();
        c.protocol = Protocol::AnthropicMessages;
        let mut msgs = json!([
            {"role":"user","content":"q"},
            {"role":"assistant","content":"a","reasoning_content":"想","reasoning_signature":"s","reasoning_model":"m"}
        ]);
        apply_outgoing_transforms(&mut msgs, "mystery-model", &c);
        assert_eq!(msgs[1]["reasoning_content"], json!("想"));
        assert_eq!(msgs[1]["reasoning_signature"], json!("s"));
    }

    #[test]
    fn strips_toolcall_protocol_text_from_assistant_content() {
        // 2026-11-01 step-5-preview 死循环实测：模型把 tool_call 协议当正文吐，
        // 回放给端点会自我强化——发送前剥掉，落盘/界面保留原文（md.js 折叠展示）
        let mut msgs = json!([
            {"role":"assistant","content":"好的，我来查看\n<tool_call>{\"method\":\"evaluate\",\"expression\":\"x\"}</tool_call>"},
            {"role":"assistant","content":"开头正常<tool_call>{\"method\":\"evaluate>\"}"},
            {"role":"user","content":"帮我看看 <tool_call> 这个标签怎么用"},
            {"role":"tool","tool_call_id":"t","content":"<tool_call>在 tool 结果里保留</tool_call>"}
        ]);
        apply_outgoing_transforms(&mut msgs, "mystery-model", &openai_compat());
        let a = msgs.as_array().unwrap();
        assert_eq!(a[0]["content"], json!("好的，我来查看\n"), "闭合泄漏整块剥掉");
        assert_eq!(a[1]["content"], json!("开头正常"), "未闭合的半截丢到文末");
        assert_eq!(a[2]["content"], json!("帮我看看 <tool_call> 这个标签怎么用"), "只剥 assistant 正文，user 消息不动");
        assert_eq!(a[3]["content"], json!("<tool_call>在 tool 结果里保留</tool_call>"), "只剥 assistant 正文，tool 结果不动");
    }

    #[test]
    fn replay_decision_matches_whitelist() {
        // 估算/裁剪的思考口径跟着这个判定走：白名单/契约渠道计入，其余不计入
        assert!(should_preserve_replay("deepseek-v4-flash", &openai_compat()), "白名单模型回放");
        assert!(!should_preserve_replay("step-5-preview", &openai_compat()), "非白名单剥离");
        assert!(!should_preserve_replay("qwen3.8-flash", &openai_compat()));
    }

    #[test]
    fn anthropic_and_responses_use_replayable_reasoning_count() {
        // 两个适配器自己回放思考（Anthropic 签名块 / Responses 加密条目），估算口径是
        // Replayable（只计带元数据的）；`should_preserve_replay` 仍为 false——它管的是
        // OpenAI Chat 形状的净化，这两个协议不走那条路。
        use crate::context::ReasoningCount;
        let mut c = openai_compat();
        c.thinking_format = ThinkingFormat::Deepseek;
        c.requires_reasoning_content_replay = true;
        assert_eq!(reasoning_count(&c, "deepseek-flash"), ReasoningCount::All);
        c.protocol = Protocol::AnthropicMessages;
        assert_eq!(reasoning_count(&c, "deepseek-flash"), ReasoningCount::Replayable);
        assert!(!should_preserve_replay("deepseek-flash", &c), "OpenAI 形状净化不另走回放");
        c.protocol = Protocol::OpenAiResponses;
        assert_eq!(reasoning_count(&c, "deepseek-flash"), ReasoningCount::Replayable);
    }

    #[test]
    fn drops_display_only_notices() {
        // 通知/错误气泡（display_only）只随会话落盘给界面回放，绝不进模型上下文
        let mut msgs = json!([
            {"role":"system","content":"sys prompt"},
            {"role":"user","content":"hi"},
            {"role":"system","content":"[通知] 回答达到最大输出长度被截断","display_only":true},
            {"role":"system","content":"[错误] 渠道超时","display_only":true}
        ]);
        apply_outgoing_transforms(&mut msgs, "mystery-model", &openai_compat());
        let roles: Vec<&str> = msgs.as_array().unwrap().iter().map(|m| m["role"].as_str().unwrap()).collect();
        assert_eq!(roles, vec!["system", "user"], "display_only 消息应被剥离");
    }

    #[test]
    fn preserves_for_deepseek_contract_and_whitelist() {
        let mut compat = openai_compat();
        compat.thinking_format = ThinkingFormat::Deepseek;
        compat.requires_reasoning_content_replay = true;
        let mut msgs = json!([
            {"role":"assistant","content":"hi","reasoning_content":"chain"}
        ]);
        apply_outgoing_transforms(&mut msgs, "whatever", &compat);
        assert_eq!(msgs[0]["reasoning_content"], json!("chain"));
        // 白名单模型（kimi 家族）在 openai 格式下也保留
        let mut msgs = json!([{"role":"assistant","content":"hi","reasoning_content":"chain"}]);
        apply_outgoing_transforms(&mut msgs, "kimi-k2-thinking", &openai_compat());
        assert_eq!(msgs[0]["reasoning_content"], json!("chain"));
        // 档位后缀剥离后仍命中白名单
        let mut msgs = json!([{"role":"assistant","content":"hi","reasoning_content":"chain"}]);
        apply_outgoing_transforms(&mut msgs, "deepseek-v4-flash-free", &openai_compat());
        assert_eq!(msgs[0]["reasoning_content"], json!("chain"));
    }

    #[test]
    fn developer_role_and_empty_filter() {
        // GPT-5 新系：system → developer
        let mut msgs = json!([
            {"role":"system","content":"be nice"},
            {"role":"user","content":"hi"}
        ]);
        apply_outgoing_transforms(&mut msgs, "gpt-5", &openai_compat());
        assert_eq!(msgs[0]["role"], json!("developer"));
        // 旧模型不动
        let mut msgs = json!([{"role":"system","content":"x"}]);
        apply_outgoing_transforms(&mut msgs, "gpt-4o-mini", &openai_compat());
        assert_eq!(msgs[0]["role"], json!("system"));

        // qwen：空 user/assistant 消息剔除
        let mut compat = openai_compat();
        compat.thinking_format = ThinkingFormat::Qwen;
        let mut msgs = json!([
            {"role":"system","content":"sys"},
            {"role":"user","content":""},
            {"role":"assistant","content":null},
            {"role":"user","content":"real"},
            {"role":"assistant","content":"","tool_calls":[{"id":"c1"}]}
        ]);
        apply_outgoing_transforms(&mut msgs, "qwen-plus", &compat);
        let roles: Vec<&str> = msgs.as_array().unwrap().iter().map(|m| m["role"].as_str().unwrap()).collect();
        assert_eq!(roles, vec!["system", "user", "assistant"], "空消息剔除且带 tool_calls 的保留");
    }
}
