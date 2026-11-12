use super::*;

/// 组装 /chat/completions 请求体（纯函数，单测覆盖各端点样式）。
/// 硬约束：不含 api_key；思考参数按端点 compat 适配而非按 provider id 分支。
/// 生产路径走 build_body_with_compat（发送前先探测一次 compat 并复用）；本包装只给单测用。
#[cfg(test)]
pub fn build_body(
    model: &str,
    messages: &Value,
    stream: bool,
    params: &ChatParams,
    tools: Option<&Value>,
) -> Value {
    let compat = compat::resolve_with_protocol(
        params.provider_id.as_deref(),
        params.base_url.as_deref(),
        model,
        params.thinking_format,
        params.protocol,
    );
    build_body_with_compat(model, messages, stream, params, tools, &compat, "")
}

/// OpenAI prompt_cache_key 长度上限（官方规范：64 字符）。
/// 会话 id 超长时截断——同会话内键稳定即可，不需要全局唯一。
pub const PROMPT_CACHE_KEY_MAX_LENGTH: usize = 64;

/// tool 结果里的截图投递（2026-11-02 统一）：**判据是模型能力，载体才分协议**。
/// - `vision=true`：OpenAI 的 tool content 只收字符串，图塞不进去 → 把图挪到紧随
///   其后的一条 user 消息（和用户发附件完全同形），模型直接看得见；tool 文本留一句指路。
/// - `vision=false`：压平成一句人话（截图已存工作区 screenshots/）——真正的转文字
///   在核心循环里已经走过视觉角色，这里只兜底，不硬发 400、也不假装模型看得见。
///
/// 图片统一攒到一组连续 tool 消息之后再发一条 user 消息：OpenAI 要求
/// assistant(tool_calls) 之后的 tool 消息连续给完，中间不能插 user。
/// 没有 tool 数组消息时原样返回（零拷贝）。
fn deliver_tool_images(messages: &Value, vision: bool) -> Value {
    let Some(arr) = messages.as_array() else { return messages.clone() };
    if !arr
        .iter()
        .any(|m| m.get("role").and_then(Value::as_str) == Some("tool") && m.get("content").is_some_and(Value::is_array))
    {
        return messages.clone();
    }
    let mut out: Vec<Value> = Vec::with_capacity(arr.len() + 2);
    let mut pending: Vec<Value> = Vec::new();
    // 连续 tool 组结束（下一条不是 tool / 到结尾）时，把攒下的截图并成一条 user 消息
    fn flush(out: &mut Vec<Value>, pending: &mut Vec<Value>) {
        if pending.is_empty() {
            return;
        }
        let mut parts = vec![json!({
            "type": "text",
            "text": "（工具截图）以下是工具刚截取的屏幕画面，请据此判断界面状态。"
        })];
        parts.append(pending);
        out.push(json!({ "role": "user", "content": parts }));
    }
    for m in arr {
        if m.get("role").and_then(Value::as_str) != Some("tool") {
            flush(&mut out, &mut pending);
            out.push(m.clone());
            continue;
        }
        let Some(parts) = m.get("content").and_then(Value::as_array) else {
            // 纯字符串 tool 结果：原样（注意别在连续 tool 组中间 flush）
            out.push(m.clone());
            continue;
        };
        let mut text = String::new();
        let mut shots: Vec<Value> = Vec::new();
        for p in parts {
            match p.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(t) = p.get("text").and_then(Value::as_str) {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(t);
                    }
                }
                Some("image_url") => shots.push(p.clone()),
                _ => {}
            }
        }
        if !shots.is_empty() {
            if vision {
                text.push_str("\n（截图已附在下一条消息里）");
                pending.extend(shots);
            } else {
                text.push_str(&format!(
                    "\n（另有 {} 张截图已存工作区 screenshots/ 目录；当前模型没勾「图片解析」，看不到图——可让用户看面板，或在「设置 → 模型配置」给该渠道勾上图片解析）",
                    shots.len()
                ));
            }
        }
        out.push(json!({ "role": "tool", "tool_call_id": m.get("tool_call_id").cloned().unwrap_or(json!("")), "content": text }));
    }
    flush(&mut out, &mut pending);
    Value::Array(out)
}

/// 会话 id → prompt_cache_key（对齐 pi-ai clampOpenAIPromptCacheKey）。
pub fn clamp_prompt_cache_key(session_id: &str) -> String {
    session_id.trim().chars().take(PROMPT_CACHE_KEY_MAX_LENGTH).collect()
}

/// 思考预算扩容：思考开启时上游把 reasoning+正文一起算进同一个 max_tokens
/// （StepFun 官方文档：max_tokens 默认 INF、推理深度只有 reasoning_effort 三档、
/// 没有单独思考预算参数；GLM/DeepSeek/Qwen 同构——共用一个输出上限）。
/// 用户配的「最大输出长度」本意是正文额度，被思考吃掉半截正文就莫名截断
/// （2026-09-29 真机实测：step-5-preview + high 档思考，SVG 写到一半被切）。
/// 发送值 = 配置值 + max(配置值, 8192)，给思考留出与正文同量级的余量；
/// 端点硬上限拒收（400）时由 stream_chat 回退配置原值重试，不影响可用性。
/// 思考关闭 / 未配 max_tokens → 原样返回（此时上限本就只约束正文，语义正确）。
pub fn effective_max_tokens(params: &ChatParams) -> Option<u64> {
    let mt = params.max_tokens?;
    let thinking_on = params
        .thinking
        .as_deref()
        .map(str::trim)
        .is_some_and(|t| !t.is_empty() && t != "off");
    if !thinking_on {
        return Some(mt);
    }
    Some(mt + mt.max(8192))
}

/// 带 compat 的组装（供网关在发送前先做历史净化时复用同一次探测）。
/// `session_id`：本次调用归属的会话 id——OpenAI 官方的 prompt_cache_key 用它做
/// 剥掉首个 user 消息之前的 assistant/tool 悬空消息（多选精确删除 user 后的配套，
/// Anthropic 协议首条必须 user；OpenAI 侧统一处理更稳）。system 消息在句首合法，
/// 与 user 一起当锚点。非数组/无锚点原样返回。
pub(crate) fn drop_leading_non_user(messages: &Value) -> Value {
    let Some(arr) = messages.as_array() else { return messages.clone() };
    let Some(fu) = arr
        .iter()
        .position(|m| matches!(m.get("role").and_then(Value::as_str), Some("user") | Some("system")))
    else {
        return messages.clone(); // 没有 user/system：不动（异常历史，别越改越坏）
    };
    if fu == 0 {
        return messages.clone();
    }
    Value::Array(arr[fu..].to_vec())
}

/// 消息级字段白名单：只留协议认的键，content/tool_calls 值整体不动。
/// `reasoning_content` 必须保留：replay 判定要给 DeepSeek/zai 等渠道回传思考
/// （2026-11-02 审计：白名单把它剥掉，回放逻辑成了死代码）。
pub(crate) fn sanitize_messages(messages: &Value) -> Value {
    const KEEP: [&str; 6] = ["role", "content", "name", "tool_calls", "tool_call_id", "reasoning_content"];
    let Some(arr) = messages.as_array() else { return messages.clone() };
    Value::Array(
        arr.iter()
            .map(|m| match m {
                Value::Object(map) => Value::Object(
                    KEEP
                        .iter()
                        .filter_map(|k| map.get(*k).map(|v| (k.to_string(), v.clone())))
                        .collect(),
                ),
                other => other.clone(),
            })
            .collect(),
    )
}

/// 会话级缓存路由（同会话的前缀命中同一份缓存条目，跨会话不互相挤掉）。
pub fn build_body_with_compat(    model: &str,
    messages: &Value,
    stream: bool,
    params: &ChatParams,
    tools: Option<&Value>,
    compat: &Compat,
    session_id: &str,
) -> Value {
    // Anthropic Messages / OpenAI Responses 协议：请求体形状完全不同，交给专用适配器；
    // 响应侧同样由适配器合成为 OpenAI 分片，下游零感知
    if compat.protocol == Protocol::AnthropicMessages {
        return anthropic::build_body(model, messages, stream, params, tools, compat);
    }
    if compat.protocol == Protocol::OpenAiResponses {
        return responses::build_body(model, messages, stream, params, tools, compat, session_id);
    }
    // tool 结果里的截图投递（2026-11-02 统一）：模型能看图（勾了「图片解析」）就把图
    // 挪进紧随其后的 user 消息——和用户发附件同形，OpenAI 协议只允许图活在 user 消息里；
    // 看不了图才压平成落盘指路。判据是模型能力，不是渠道/协议。
    let messages = deliver_tool_images(messages, params.vision);
    // 消息级字段白名单（2026-10-26）：会话文件里的消息带内部字段（ts 时间戳、
    // steered/interrupted 引导标记、stats 信息卡数据），续跑轮次（last_snapshot）
    // 会把它们原样带进这里——OpenAI 形状是整对象透传，不洗就发给端点（多数兼容
    // 端点忽略未知字段，官方 OpenAI 直接 400）。只留协议认的字段，content 整体不动。
    let messages = sanitize_messages(&messages);
    // 句首孤儿剥除（2026-10-27 多选精确删除配套）：user 消息被精确删掉后，消息列可能
    // 以 assistant/tool 开头（悬空回复）。OpenAI 侧多数端点容忍，但统一剥掉更稳。
    let messages = drop_leading_non_user(&messages);
    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": stream,
    });
    if stream && compat.supports_usage_in_streaming {
        // 流式末尾带回 usage（非标端点不收 stream_options，按 compat 跳过）
        body["stream_options"] = json!({ "include_usage": true });
    }
    if let Some(t) = tools {
        body["tools"] = t.clone();
    }
    if let Some(mt) = params.max_tokens {
        body[compat.max_tokens_field] = json!(mt);
    }
    if let Some(t) = params.temperature
        && compat.supports_temperature {
            body["temperature"] = json!(t);
        }
    if let Some(t) = params.top_p {
        body["top_p"] = json!(t);
    }
    // —— 提示缓存（对齐 pi-ai openai-completions）——
    // prompt_cache_key：官方 OpenAI 的会话级缓存路由键，同键请求优先命中同一缓存条目。
    // 只对 api.openai.com 发（中转/国产端点普遍 400，compat 已把开关关掉）；
    // retention=none 时连键也不发（用户显式关缓存）。
    if compat.supports_prompt_cache_key
        && params.cache_retention != CacheRetention::None
        && !session_id.trim().is_empty()
    {
        body["prompt_cache_key"] = json!(clamp_prompt_cache_key(session_id));
    }
    // prompt_cache_retention="24h"：长保留档，仅官方端点认；其余端点 long 静默降级短保留
    if params.cache_retention == CacheRetention::Long && compat.supports_long_cache_retention {
        body["prompt_cache_retention"] = json!("24h");
    }

    // —— 思考参数：按 thinkingFormat 分支 ——
    // 关键保底：默认档位（None 或空串）绝不向任何网关发送任何思考字段，坚决尊重渠道自身行为
    let requested = params
        .thinking
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match compat.thinking_format {
        // 极简模式：不向端点发送任何思考相关参数
        ThinkingFormat::None => {}
        // zai（智谱 GLM/豆包 Seed）：{type:enabled|disabled}；
        // GLM-5.3 起官方新增 reasoning_effort 三档 low/high/max（默认 max，thinking
        // 恒开、不再收 disabled）——显式档位折算后随 thinking:enabled 一起发；
        // 旧 GLM 不认该参数时按渠道行为忽略（on/未知档不发 effort=渠道默认）
        ThinkingFormat::Zai => {
            if let Some(t) = requested {
                if t == "off" {
                    body["thinking"] = json!({ "type": "disabled" });
                } else {
                    body["thinking"] = json!({ "type": "enabled" });
                    let eff = match t {
                        "minimal" | "low" => Some("low"),
                        "medium" | "high" => Some("high"),
                        "xhigh" | "max" => Some("max"),
                        _ => None,
                    };
                    if let Some(e) = eff {
                        body["reasoning_effort"] = json!(e);
                    }
                }
            }
        }
        // deepseek（V4 Pro/Flash/R1）：thinking 对象；reasoning_effort 规范档只有
        // none/low/high/max（官方文档，默认 high）——minimal→low、medium/xhigh→high
        // 是官方自己的兼容折算，这里先折算成规范档再发，绝不外发非规范值
        ThinkingFormat::Deepseek => {
            if let Some(t) = requested {
                if t == "off" {
                    body["thinking"] = json!({ "type": "disabled" });
                    body["reasoning_effort"] = json!("none");
                } else {
                    body["thinking"] = json!({ "type": "enabled" });
                    let eff = match t {
                        "minimal" | "low" => Some("low"),
                        "medium" | "high" | "xhigh" => Some("high"),
                        "max" => Some("max"),
                        _ => None, // on/未知档：不发 effort，官方默认 high
                    };
                    if let Some(e) = eff {
                        body["reasoning_effort"] = json!(e);
                    }
                }
            }
        }
        // qwen（百炼/混元）：顶层布尔 enable_thinking
        ThinkingFormat::Qwen => {
            if let Some(t) = requested {
                body["enable_thinking"] = json!(t != "off");
            }
        }
        // 本地推理服务聊天模板：chat_template_kwargs.enable_thinking（vLLM/llama.cpp Qwen3 系）
        ThinkingFormat::QwenChatTemplate => {
            if let Some(t) = requested {
                let kwargs = body
                    .as_object_mut()
                    .unwrap()
                    .entry("chat_template_kwargs")
                    .or_insert_with(|| json!({}));
                kwargs["enable_thinking"] = json!(t != "off");
            }
        }
        // openai：reasoning_effort 档位，经 effort::resolve 按模型降级；无法表达"关"时不发
        ThinkingFormat::Openai => {
            let level = match requested {
                Some("on") => Some("medium".to_string()),
                Some(v) => effort::resolve_effort(model, v, None),
                None => None,
            };
            if let Some(level) = level {
                body["reasoning_effort"] = json!(level);
            }
        }
    }
    body
}

/// 流式分片聚合器（从 stream_chat 抽出）：一个 OpenAI 形状分片进，
/// usage/思考剥离/tool_calls 增量聚合、model_chunk 事件、干净分片出。
/// Anthropic 协议的合成 OpenAI 分片走同一入口，下游零分叉。
pub(crate) struct StreamAgg {
    pub(crate) turn: AssistantTurn,
    pub(crate) calls: Vec<ToolCall>,
    pub(crate) think_split: TagSplit,
    pub(crate) chunk_count: u64,
}

impl StreamAgg {
    pub(crate) fn new() -> Self {
        Self { turn: AssistantTurn::default(), calls: Vec::new(), think_split: TagSplit::default(), chunk_count: 0 }
    }

    /// 处理一个分片：raw/data 原文 + OpenAI 形状 chunk（可变，思考剥离会写回）。
    pub(crate) fn feed(&mut self, chunk: &mut Value, data: &str) {
        // 末尾分片的 usage：OpenAI 风格 prompt_tokens_details.cached_tokens
        // 或 DeepSeek 风格 prompt_cache_hit_tokens（缓存命中率展示 +  台账分计量）
        if let Some(u) = chunk.get("usage")
            && let Some(p) = u.get("prompt_tokens").and_then(Value::as_u64)
        {
            self.turn.prompt_tokens = Some(p);
            self.turn.completion_tokens = u
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .or_else(|| u.get("output_tokens").and_then(Value::as_u64));
            self.turn.cached_tokens = u
                .pointer("/prompt_tokens_details/cached_tokens")
                .and_then(Value::as_u64)
                .or_else(|| u.get("prompt_cache_hit_tokens").and_then(Value::as_u64));
        }
        let mut delta = extract_delta(chunk);
        // 反代思考标签剥离：标签内文字改道思考通道；
        // 结果写回 chunk，让 on_chunk 转发路径（前端 SSE）也拿到干净值
        if !delta.content.is_empty() {
            let (think, clean) = self.think_split.feed(&delta.content);
            if !think.is_empty() || clean.len() != delta.content.len() {
                if !think.is_empty() {
                    delta.reasoning = Some(match delta.reasoning.take() {
                        Some(r) => format!("{r}{think}"),
                        None => think,
                    });
                }
                delta.content = clean.clone();
                if let Some(d) = chunk.pointer_mut("/choices/0/delta") {
                    d["content"] = json!(clean);
                    if let Some(r) = &delta.reasoning {
                        d["reasoning_content"] = json!(r);
                    }
                }
            }
        }
        // 2026-11-01 日志瘦身：原文（raw）不再无条件写进 JSONL——逐 token 的分片占
        // 实测日志的 79.5%，7MB/10 分钟的增速把 20MB 轮转反复冲穿，task.start 被冲掉
        // 后时间线全是「起点缺失」碎片（真机实测）。带诊断价值的两种分片仍在下面补回
        // 原文：工具调用分片（拆包/残缺问题靠它）和带 finish_reason 的收尾分片。
        let mut payload = json!({ "content": delta.content });
        if let Some(r) = &delta.reasoning {
            payload["reasoning_content"] = json!(r);
        }
        if let Some(f) = &delta.finish {
            payload["finish_reason"] = json!(f);
            self.turn.finish_reason = Some(f.clone());
        }
        let tc_arr = chunk
            .pointer("/choices/0/delta/tool_calls")
            .and_then(Value::as_array)
            .cloned();
        if let Some(arr) = &tc_arr {
            payload["tool_calls"] = json!(arr);
            accumulate_tool_calls(arr, &mut self.calls);
        }
        self.turn.content.push_str(&delta.content);
        // 流式计时：首个任意分片（含思考）= 首包；正文首/末分片 = 输出耗时（信息卡用）
        let now = std::time::Instant::now();
        self.turn.first_chunk_at.get_or_insert(now);
        if !delta.content.is_empty() {
            self.turn.content_first_at.get_or_insert(now);
            self.turn.content_last_at = Some(now);
        }
        if let Some(r) = &delta.reasoning {
            self.turn.reasoning.push_str(r);
            // 思考时长口径：首尾 reasoning 分片的到达时刻差（「思考 · 持续了 N 秒」）
            self.turn.reasoning_first_at.get_or_insert(now);
            self.turn.reasoning_last_at = Some(now);
        }
        // 回放元数据：Anthropic 思考签名 / Responses 加密思考条目（随 turn 落到消息上）
        if let Some(sig) = &delta.reasoning_signature {
            self.turn.reasoning_signature = Some(sig.clone());
        }
        if let Some(enc) = &delta.reasoning_encrypted {
            self.turn.reasoning_encrypted = Some(enc.clone());
        }
        if !delta.reasoning_blocks.is_empty() {
            self.turn.reasoning_blocks.extend(delta.reasoning_blocks.iter().cloned());
        }
        if tc_arr.is_some() || payload.get("finish_reason").is_some() {
            payload["raw"] = json!(data);
        }
        observability::event(EventType::ModelChunk, payload);
        observability::debug(&format!("[gateway] sse: {data}"));
        self.chunk_count += 1;
    }

    /// 流结束：hold 里若剩半截标签（如截断在 "<thi"），按当前通道归位；tool_calls 落位。
    pub(crate) fn finish(&mut self) {
        self.turn.finished_at = Some(std::time::Instant::now());
        let (tail_think, tail_content) = self.think_split.finish();
        if !tail_think.is_empty() {
            self.turn.reasoning.push_str(&tail_think);
            let now = std::time::Instant::now();
            self.turn.reasoning_first_at.get_or_insert(now);
            self.turn.reasoning_last_at = Some(now);
        }
        self.turn.content.push_str(&tail_content);
        self.turn.tool_calls = std::mem::take(&mut self.calls);
    }
}

/// tool_call index 的 sane 上限：并行调用再多也到不了这个量级。越界或负值都是协议
/// 违反——照旧合并会把 Vec 撑爆（`as usize` 把 -1 变成 ~1.8e19，`while calls.len()
/// <= index` 无限 push 到 OOM；index: 999999999 同理）。2026-10-08 修。
const MAX_TOOL_CALL_INDEX: i64 = 128;

/// 按 index 合并流式 tool_calls 分片（OpenAI 兼容增量格式）。
pub(crate) fn accumulate_tool_calls(arr: &[Value], calls: &mut Vec<ToolCall>) {
    for entry in arr {
        let index = match entry.get("index").and_then(Value::as_i64) {
            None => 0, // 缺 index 按 0 合并（原行为）
            Some(i) if (0..=MAX_TOOL_CALL_INDEX).contains(&i) => i as usize,
            Some(i) => {
                observability::event(
                    EventType::Error,
                    json!({ "message": format!("tool_calls 分片 index 越界（{i}），忽略该分片"), "raw": entry }),
                );
                continue;
            }
        };
        while calls.len() <= index {
            calls.push(ToolCall { id: String::new(), name: String::new(), arguments: String::new() });
        }
        let call = &mut calls[index];
        // id/name 是**整值**不是增量：有的中转每个分片重复携带同一个 id/name，
        // 用 push_str 会拼成 `call_1call_1…` / `read_fileread_file`，工具结果对不上号
        // （2026-11-02 审计）。重复携带时保持首个值；分片只对 arguments 增量拼接。
        if let Some(id) = entry.get("id").and_then(Value::as_str)
            && call.id.is_empty()
        {
            call.id = id.to_string();
        }
        if let Some(name) = entry.pointer("/function/name").and_then(Value::as_str)
            && call.name.is_empty()
        {
            call.name = name.to_string();
        }
        if let Some(args) = entry.pointer("/function/arguments").and_then(Value::as_str) {
            call.arguments.push_str(args);
        }
    }
}

pub struct Delta {
    pub content: String,
    pub reasoning: Option<String>,
    pub finish: Option<String>,
    /// Anthropic 思考块签名（随 signature_delta 到达；回放时要原样带回）
    pub reasoning_signature: Option<String>,
    /// Responses 思考条目加密内容（回放时作为 reasoning item 的 encrypted_content）
    pub reasoning_encrypted: Option<String>,
    /// 思考块序列（Anthropic thinking/redacted_thinking、Responses reasoning 条目），
    /// 按到达顺序累积；回放时逐块原样带回
    pub reasoning_blocks: Vec<Value>,
}

/// 上游把错误塞进 200 的 SSE 分片里（典型：中转/队列服务返回
/// `finish_reason:"error"` 加顶层 `error:{message:...}`）。这类分片能正常解析，
/// 旧逻辑当普通分片吞掉，整条流结束时正文为空、界面只看到转圈结束——必须在这里
/// 认出来并抛给用户。返回 Some(人话错误) 表示这个分片是错误；普通分片返回 None。
pub(crate) fn sse_error_message(chunk: &Value) -> Option<String> {
    let finish_error = chunk.pointer("/choices/0/finish_reason").and_then(Value::as_str) == Some("error");
    let err = chunk.get("error").filter(|v| !v.is_null());
    if !finish_error && err.is_none() {
        return None;
    }
    let msg = match err {
        Some(Value::String(s)) if !s.trim().is_empty() => s.trim().to_string(),
        Some(Value::Object(o)) => o
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| {
                o.get("code")
                    .filter(|c| !c.is_null())
                    .map(|c| format!("错误码 {c}"))
            })
            .unwrap_or_else(|| Value::Object(o.clone()).to_string()),
        Some(v) => v.to_string(),
        None => "上游返回 finish_reason=error，但未附带错误说明".to_string(),
    };
    Some(msg)
}

/// 从分片里提取增量内容与思考内容。
/// 思考字段取 reasoning_content / reasoning / reasoning_text / reasoning_details 四路，
/// 按序取第一个非空（含 OpenRouter reasoning_details 的 type=reasoning.text）。
pub fn extract_delta(chunk: &Value) -> Delta {
    let choice = chunk.pointer("/choices/0").cloned().unwrap_or(Value::Null);
    let delta = choice.pointer("/delta").cloned().unwrap_or(Value::Null);
    Delta {
        content: delta.pointer("/content").and_then(Value::as_str).unwrap_or("").to_string(),
        reasoning: extract_reasoning(&delta),
        finish: choice.pointer("/finish_reason").and_then(Value::as_str).map(String::from),
        reasoning_signature: delta
            .get("reasoning_signature")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from),
        reasoning_encrypted: delta
            .get("reasoning_encrypted")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from),
        reasoning_blocks: delta
            .get("reasoning_blocks")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    }
}

/// OpenAI Completions 系思考字段（上游 OPENAI_COMPLETIONS_REASONING_FIELDS 顺序）。
pub(crate) fn extract_reasoning(delta: &Value) -> Option<String> {
    for field in ["reasoning_content", "reasoning", "reasoning_text"] {
        if let Some(s) = delta.get(field).and_then(Value::as_str)
            && !s.is_empty() {
                return Some(s.to_string());
            }
    }
    // OpenRouter：delta.reasoning_details 数组里的 reasoning.text 块
    if let Some(details) = delta.get("reasoning_details").and_then(Value::as_array) {
        let mut buf = String::new();
        for d in details {
            if d.get("type").and_then(Value::as_str) == Some("reasoning.text")
                && let Some(t) = d.get("text").and_then(Value::as_str) {
                    buf.push_str(t);
                }
        }
        if !buf.is_empty() {
            return Some(buf);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_logger() {
        static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        ONCE.get_or_init(|| {
            let dir = std::env::temp_dir().join("agent-test-logs");
            let _ = crate::observability::init(&dir, "openai.jsonl", format!("openai-{}", uuid::Uuid::new_v4()));
        });
    }

    /// 2026-10-08 修：负 index（`as usize` → ~1.8e19）与超大 index 会把合并 Vec
    /// 无限撑大直到 OOM；越界分片应被忽略，且不影响正常分片合并。
    #[test]
    fn tool_call_index_out_of_range_is_ignored() {
        init_logger();
        let mut calls: Vec<ToolCall> = Vec::new();
        accumulate_tool_calls(
            &[
                json!({"index": 0, "id": "a", "function": {"name": "read_file", "arguments": "{}"}}),
                json!({"index": -1, "id": "bad"}),
                json!({"index": 999999999, "id": "bad2"}),
                json!({"index": 1, "id": "b", "function": {"name": "shell", "arguments": "{\"c\":1}"}}),
            ],
            &mut calls,
        );
        assert_eq!(calls.len(), 2, "越界分片不得占位: {calls:?}");
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[1].name, "shell");
        assert_eq!(calls[1].arguments, "{\"c\":1}");
        // 缺 index 仍按 0 合并（原行为不变）
        let mut c2: Vec<ToolCall> = Vec::new();
        accumulate_tool_calls(&[json!({"id": "x", "function": {"name": "git"}})], &mut c2);
        assert_eq!(c2.len(), 1);
        assert_eq!(c2[0].id, "x");
    }

    #[test]
    fn drop_leading_non_user_trims_orphan_head() {
        // 多选精确删除 user 后：句首悬空 assistant/tool 剥掉，user 及其后原样
        let mut msgs = json!([
            {"role":"assistant","content":"悬空回复"},
            {"role":"tool","tool_call_id":"t1","content":"悬空结果"},
            {"role":"user","content":"第二问"},
            {"role":"assistant","content":"第二答"}
        ])
        .as_array()
        .cloned()
        .unwrap();
        msgs = drop_leading_non_user(&json!(msgs)).as_array().cloned().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["content"], json!("第二问"));
        assert_eq!(msgs[1]["content"], json!("第二答"));
    }

    #[test]
    fn drop_leading_non_user_keeps_system_head() {
        // system 在句首合法：与 user 一起当锚点，不剥
        let msgs = json!([
            {"role":"system","content":"系统"},
            {"role":"user","content":"问"}
        ]);
        let out = drop_leading_non_user(&msgs);
        assert_eq!(out, msgs, "system+user 开头一个不该动");
    }

    #[test]
    fn drop_leading_non_user_no_user_is_noop() {
        // 没有 user 的异常历史：原样返回，不越改越坏
        let msgs = json!([{"role":"assistant","content":"只有助手"}]);
        assert_eq!(drop_leading_non_user(&msgs), msgs);
    }

    #[test]
    fn sanitize_messages_strips_internal_fields() {
        // 内部字段（ts/steered/stats/cp）不出网关；协议字段与 content 值整体不动
        let msgs = json!([
            {"role":"user","content":"问","ts":123,"steered":true,"cp":"abc","stats":{"x":1}},
            {"role":"assistant","content":null,"tool_calls":[{"id":"c1"}],"work_ms":9,"stats":{"ttft_ms":1}},
            {"role":"tool","tool_call_id":"c1","content":"结果","display_only":true}
        ]);
        let out = sanitize_messages(&msgs);
        let arr = out.as_array().unwrap();
        assert_eq!(arr[0], json!({"role":"user","content":"问"}), "用户消息只剩协议字段");
        assert_eq!(arr[1], json!({"role":"assistant","content":null,"tool_calls":[{"id":"c1"}]}));
        assert_eq!(arr[2], json!({"role":"tool","tool_call_id":"c1","content":"结果"}));
    }

    #[test]
    fn sse_error_message_detects_in_stream_errors() {
        // 真机样本（api.sunguai.top 队列满）：200 流里 finish_reason=error + 顶层 error
        let real = json!({
            "choices":[{"delta":{},"finish_reason":"error","index":0}],
            "error":{"message":"HTTP 401 403 {\"code\":\"10605\",\"message\":\"queue full\"}"}
        });
        assert_eq!(
            sse_error_message(&real).as_deref(),
            Some("HTTP 401 403 {\"code\":\"10605\",\"message\":\"queue full\"}")
        );
        // 只有 finish_reason=error、没有 error 对象：也要报，不能当空回答
        let bare = json!({"choices":[{"delta":{},"finish_reason":"error","index":0}]});
        assert!(sse_error_message(&bare).unwrap().contains("finish_reason=error"));
        // 顶层 error 字符串
        let strerr = json!({"error":"queue overloaded"});
        assert_eq!(sse_error_message(&strerr).as_deref(), Some("queue overloaded"));
        // 正常分片 / error:null 不算错误
        assert!(sse_error_message(&json!({"choices":[{"delta":{"content":"好"},"finish_reason":null}]})).is_none());
        assert!(sse_error_message(&json!({"choices":[{"delta":{}}],"error":null})).is_none());
    }
}

#[cfg(test)]
mod accumulate_tests {
    use super::*;

    /// 2026-11-02 审计：id/name 是整值不是增量，重复携带时不能拼成 call_1call_1。
    #[test]
    fn tool_call_id_and_name_are_not_concatenated() {
        let arr = json!([
            {"index":0,"id":"call_1","function":{"name":"read_file","arguments":"{\"path\":"}},
            {"index":0,"id":"call_1","function":{"name":"read_file","arguments":"\"a\"}"}}
        ]);
        let mut calls = Vec::new();
        accumulate_tool_calls(arr.as_array().unwrap(), &mut calls);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].arguments, "{\"path\":\"a\"}", "arguments 仍按增量拼接");
    }
}
