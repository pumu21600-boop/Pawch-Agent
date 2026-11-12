//! OpenAI Responses 协议适配器（/responses，2026-11-11）。
//!
//! 请求侧：把内部 OpenAI 形状消息翻译成 Responses 条目（instructions + input[]，
//! function_call / function_call_output 条目，工具定义拆平）；响应侧把 output[] 与
//! 事件流合成回 OpenAI chat.completion 形状——下游（聚合器/核心循环/界面）零感知，
//! 与 anthropic.rs 同一条边界纪律。思考档位按官方形状下发 `reasoning:{effort,summary}`；
//! 不认该字段的兼容端点由网关的 400 剥参重试兜底。

use serde_json::{json, Value};

use super::compat::ThinkingFormat;

/// Responses 端点 URL（与 anthropic::messages_url 同一条补全规则，后缀换 /responses）：
/// 结尾 /responses 原样（全路径）；结尾 /v1 补 /responses；其余补 /v1/responses。
/// 「自动补后缀」不勾时整段跳过（Gateway::endpoint_url_with 直发 Base URL）。
pub fn responses_url(base_url: &str) -> String {
    let b = base_url.trim().trim_end_matches('/');
    if b.ends_with("/responses") {
        b.to_string()
    } else if b.ends_with("/v1") {
        format!("{b}/responses")
    } else {
        format!("{b}/v1/responses")
    }
}

/// input 条目的文本 part（Responses 的 content 块全带 type）。
fn input_text(text: &str) -> Value {
    json!({ "type": "input_text", "text": text })
}

/// 从消息 content 抽纯文本（string 原样；数组拼 text/output_text 块）。
fn content_text(c: Option<&Value>) -> String {
    match c {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|p| {
                let t = p.get("type").and_then(Value::as_str).unwrap_or("");
                if t == "text" || t == "input_text" || t == "output_text" {
                    p.get("text").and_then(Value::as_str)
                } else {
                    None
                }
            })
            .collect::<String>(),
        _ => String::new(),
    }
}

/// Responses `reasoning.effort` 档位。官方集合是 minimal/low/medium/high（新模型
/// 还有 xhigh/max）；共享解析器在请求档位不受支持时会掉到最低档（max→low），
/// 这里按「只升不降」夹回最接近的高档——宁可多想一点，不能把「最大」静默降成「低」。
fn reasoning_effort_for(model: &str, requested: &str) -> Option<String> {
    let req = requested.trim();
    if matches!(req, "off" | "none") {
        return None;
    }
    let req = if req == "on" { "medium" } else { req };
    let rank = |v: &str| match v {
        "none" => 0,
        "minimal" => 1,
        "low" => 2,
        "medium" => 3,
        "high" => 4,
        "xhigh" => 5,
        "max" => 6,
        _ => 3,
    };
    match super::effort::resolve_effort(model, req, None) {
        Some(r) if rank(&r) > 0 && rank(&r) >= rank(req) => Some(r),
        _ => Some(
            match req {
                "minimal" | "low" => "low",
                "high" => "high",
                "xhigh" | "max" => "high",
                _ => "medium",
            }
            .to_string(),
        ),
    }
}

/// 思考摘要：官方 o 系 / gpt-5 系支持 `reasoning.summary`，不带它时模型只回加密
/// 思考、界面看不到思考文本。兼容端点（DeepSeek 等）只发 effort，避免多余字段 400。
fn wants_reasoning_summary(model: &str) -> bool {
    let id = model.trim().to_lowercase();
    let bare = id.rsplit('/').next().unwrap_or(&id);
    bare.starts_with("gpt-5")
        || bare.starts_with("o1")
        || bare.starts_with("o3")
        || bare.starts_with("o4")
        || bare.contains("codex")
}

/// 可回放的 Responses 思考条目（按保存顺序）：新消息用 `reasoning_blocks`
/// （支持一条回答里多个 reasoning 条目）；旧消息回落单个 legacy encrypted 条目。
fn replay_reasoning_items(m: &Value, model: &str, current_source: Option<&str>) -> Vec<Value> {
    if !super::replay::reasoning_source_matches(m, model, current_source) {
        return Vec::new();
    }
    if let Some(arr) = m.get("reasoning_blocks").and_then(Value::as_array) {
        let mut out = Vec::new();
        for b in arr {
            if b.get("type").and_then(Value::as_str) == Some("reasoning")
                && let Some(enc) = b.get("encrypted_content").and_then(Value::as_str).filter(|e| !e.is_empty())
            {
                out.push(json!({ "type": "reasoning", "encrypted_content": enc, "summary": [] }));
            }
        }
        return out;
    }
    if let Some(enc) = m.get("reasoning_encrypted").and_then(Value::as_str).filter(|e| !e.is_empty()) {
        return vec![json!({ "type": "reasoning", "encrypted_content": enc, "summary": [] })];
    }
    Vec::new()
}

/// 组装 Responses 请求体（从内部 OpenAI 形状消息翻译）。
pub fn build_body(
    model: &str,
    messages: &Value,
    stream: bool,
    params: &super::ChatParams,
    tools: Option<&Value>,
    compat: &super::compat::Compat,
    session_id: &str,
) -> Value {
    let msgs = messages.as_array().cloned().unwrap_or_default();
    // 当前端点指纹（来源守卫用）：params.base_url 由网关发请求前写入实际地址
    let current_source = params
        .base_url
        .as_deref()
        .map(|b| super::replay::source_tag(super::Protocol::OpenAiResponses, b, model));
    let mut instructions = String::new();
    let mut input: Vec<Value> = Vec::new();
    for m in &msgs {
        let role = m.get("role").and_then(Value::as_str).unwrap_or("");
        match role {
            // system → instructions（Responses 的顶层系统字段；多条按序拼接）
            "system" => {
                let t = content_text(m.get("content"));
                if !t.is_empty() {
                    if !instructions.is_empty() {
                        instructions.push_str("\n\n");
                    }
                    instructions.push_str(&t);
                }
            }
            "user" => {
                let mut parts: Vec<Value> = Vec::new();
                match m.get("content") {
                    Some(Value::String(s)) => {
                        if !s.is_empty() {
                            parts.push(input_text(s));
                        }
                    }
                    Some(Value::Array(a)) => {
                        for p in a {
                            match p.get("type").and_then(Value::as_str).unwrap_or("") {
                                "text" | "input_text" => {
                                    if let Some(t) = p.get("text").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                                        parts.push(input_text(t));
                                    }
                                }
                                // 视觉投递（2026-11-02 统一）：图片进 user 消息，同一形状换名字
                                "image_url" => {
                                    if let Some(url) =
                                        p.pointer("/image_url/url").and_then(Value::as_str).filter(|u| !u.is_empty())
                                    {
                                        parts.push(json!({ "type": "input_image", "image_url": url }));
                                    }
                                }
                                // 音频投递（媒体旁路）：Responses 的输入内容块与 Chat 同形，
                                // 原样换成 input_audio（Responses 不认 audio_url，那种变体在
                                // 调用方已按协议筛掉）
                                "input_audio" => {
                                    if let Some(ia) = p.get("input_audio") {
                                        parts.push(json!({ "type": "input_audio", "input_audio": ia.clone() }));
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
                if !parts.is_empty() {
                    input.push(json!({ "role": "user", "content": parts }));
                }
            }
            "assistant" => {
                // 思考回放（2026-11-11，对齐 ZCode）：无状态模式下把加密思考条目带回
                // input（OpenAI 要 include 才返回 encrypted_content）。支持一条回答里
                // 多个 reasoning 条目（按序插回），来源/模型对不上时不回放。
                for item in replay_reasoning_items(m, model, current_source.as_deref()) {
                    input.push(item);
                }
                // 正文先行，随后逐个 function_call 条目。**必须是字符串**：
                // 历史 assistant 消息属于 EasyInputMessage，content 只收
                // string / input_* 内容块；`output_text` 是输出条目的类型，
                // StepFun 这类严格端点会 400（2026-11-11 真机：多轮对话
                // 「1255 validation error(s)」，单轮测试连通却正常）。
                let text = content_text(m.get("content"));
                if !text.is_empty() {
                    input.push(json!({ "role": "assistant", "content": text }));
                }
                if let Some(calls) = m.get("tool_calls").and_then(Value::as_array) {
                    for tc in calls {
                        let id = tc.get("id").and_then(Value::as_str).unwrap_or("");
                        let name = tc
                            .pointer("/function/name")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        let args = tc
                            .pointer("/function/arguments")
                            .and_then(Value::as_str)
                            .unwrap_or("{}");
                        if id.is_empty() || name.is_empty() {
                            continue;
                        }
                        input.push(json!({
                            "type": "function_call",
                            "call_id": id,
                            "name": name,
                            "arguments": args,
                        }));
                    }
                }
            }
            // 工具结果 → function_call_output（output 是字符串字段）
            "tool" => {
                let call_id = m.get("tool_call_id").and_then(Value::as_str).unwrap_or("");
                let out = content_text(m.get("content"));
                if !call_id.is_empty() {
                    input.push(json!({ "type": "function_call_output", "call_id": call_id, "output": out }));
                }
            }
            _ => {}
        }
    }
    if input.is_empty() {
        // 空输入会被端点拒（ModelStudio/Qwen 惯例同款防御）：补一条空 user
        input.push(json!({ "role": "user", "content": [input_text(" ")] }));
    }
    let mut body = json!({
        "model": model,
        "instructions": instructions,
        "input": input,
        "stream": stream,
        // 无状态：不依赖服务端存储的 response id；显式要回加密思考内容，
        // 下一轮才能把 reasoning 条目原样带回（对齐 ZCode 的 store=false 分支）
        "store": false,
        "include": ["reasoning.encrypted_content"],
    });
    if let Some(mt) = params.max_tokens {
        // Responses 的上限字段名不同；官方下限 16，低于会被 400
        body["max_output_tokens"] = json!(mt.max(16));
    }
    if let Some(t) = params.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(t) = params.top_p {
        body["top_p"] = json!(t);
    }
    // 工具定义拆平：chat 形状的 {"type":"function","function":{...}} → Responses 扁平形状
    if let Some(t) = tools {
        let arr = t.as_array().cloned().unwrap_or_default();
        let flat: Vec<Value> = arr
            .iter()
            .filter_map(|td| {
                let f = td.get("function")?;
                Some(json!({
                    "type": "function",
                    "name": f.get("name")?,
                    "description": f.get("description").cloned().unwrap_or(json!("")),
                    "parameters": f.get("parameters").cloned().unwrap_or(json!({ "type": "object" })),
                }))
            })
            .collect();
        if !flat.is_empty() {
            body["tools"] = json!(flat);
            body["tool_choice"] = json!("auto");
        }
    }
    // 提示缓存：官方 OpenAI 的会话级缓存路由键（与 Chat Completions 同一把尺子）。
    // 中转/兼容端点 compat 已把开关关掉，不会多发字段；retention=none 时连键也不发。
    if compat.supports_prompt_cache_key
        && params.cache_retention != super::compat::CacheRetention::None
        && !session_id.trim().is_empty()
    {
        body["prompt_cache_key"] = json!(super::openai::clamp_prompt_cache_key(session_id));
    }
    // 思考档位：官方形状 reasoning:{effort, summary}（对齐 ZCode）。off / 未设置 /
    // 渠道显式关思考（thinking_style=none）一律不发，尊重渠道默认；不认该字段的
    // 兼容端点由网关 400 剥参重试兜底（reasoning 已进 body_has_thinking_params）。
    if compat.thinking_format != ThinkingFormat::None
        && let Some(t) = params.thinking.as_deref().map(str::trim).filter(|s| !s.is_empty())
        && let Some(effort) = reasoning_effort_for(model, t)
    {
        let mut reasoning = json!({ "effort": effort });
        if wants_reasoning_summary(model) {
            reasoning["summary"] = json!("auto");
        }
        body["reasoning"] = reasoning;
    }
    body
}

/// 非流式响应 → OpenAI chat.completion 形状（下游 usage_triple/正文抽取零感知）。
pub fn to_openai_completion(data: &Value) -> Value {
    let mut content = String::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    if let Some(output) = data.get("output").and_then(Value::as_array) {
        for item in output {
            match item.get("type").and_then(Value::as_str).unwrap_or("") {
                "message" => {
                    if let Some(parts) = item.get("content").and_then(Value::as_array) {
                        for p in parts {
                            if p.get("type").and_then(Value::as_str) == Some("output_text")
                                && let Some(t) = p.get("text").and_then(Value::as_str)
                            {
                                content.push_str(t);
                            }
                        }
                    }
                }
                "function_call" => {
                    let idx = tool_calls.len();
                    tool_calls.push(json!({
                        "id": item.get("call_id").or_else(|| item.get("id")).cloned().unwrap_or(json!(format!("call_{idx}"))),
                        "type": "function",
                        "function": {
                            "name": item.get("name").cloned().unwrap_or(json!("")),
                            "arguments": item.get("arguments").cloned().unwrap_or(json!("{}")),
                        },
                    }));
                }
                _ => {}
            }
        }
    }
    let finish = if tool_calls.is_empty() { "stop" } else { "tool_calls" };
    let mut message = json!({ "role": "assistant", "content": content });
    if !tool_calls.is_empty() {
        message["tool_calls"] = json!(tool_calls);
    }
    let u = data.get("usage").cloned().unwrap_or(json!({}));
    let prompt = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
    let completion = u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
    // Responses 的 input_tokens 是含缓存的总输入（cached 是子集），
    // 缓存命中数在 input_tokens_details.cached_tokens——不映射的话
    // 用量面板命中率恒 0（2026-11-11 用户指出，与 Chat 口径对齐）。
    let cached = u
        .pointer("/input_tokens_details/cached_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    json!({
        "choices": [{
            "message": message,
            "finish_reason": finish,
        }],
        "usage": {
            "prompt_tokens": prompt,
            "completion_tokens": completion,
            "prompt_tokens_details": { "cached_tokens": cached },
        },
    })
}

/// Responses SSE 事件翻译结果（同 anthropic::Translated 的三态约定）。
#[derive(Debug)]
pub enum Translated {
    /// 合成出的 OpenAI 形状分片（走下游既有处理）
    Chunk(Value),
    /// 流正常结束
    #[allow(dead_code)] // Responses 流以 response.completed 收尾（无独立结束事件）；保留备中转补 [DONE] 形态
    Done,
    /// created/queued/content_part.added 等无需处理的事件
    Ignore,
}

/// Responses SSE → OpenAI delta 合成器。
/// usage 在 response.completed 事件里一次性给出，随 finish_reason 分片合成发出。
#[derive(Default)]
pub struct SseTranslator {
    /// 已开始的 function_call 条数（= 下一个 OpenAI tool_call index）
    call_count: usize,
    /// 思考条目的 encrypted_content（回放用；output_item.done 或收尾响应里带）
    reasoning_encrypted: Option<String>,
    /// 本条回答的全部 reasoning 条目（按到达顺序；一条回答可能有多个）
    reasoning_blocks: Vec<Value>,
}

impl SseTranslator {
    fn finish_chunk(&self, usage: &Value) -> Value {
        let finish = if self.call_count > 0 { "tool_calls" } else { "stop" };
        let prompt = usage.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
        let completion = usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
        // 缓存命中数（子集）：不映射会让 Responses 配置的命中率恒 0
        let cached = usage
            .pointer("/input_tokens_details/cached_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let mut delta = json!({});
        if let Some(enc) = &self.reasoning_encrypted {
            delta["reasoning_encrypted"] = json!(enc);
        }
        if !self.reasoning_blocks.is_empty() {
            delta["reasoning_blocks"] = json!(self.reasoning_blocks);
        }
        json!({
            "choices": [{ "delta": delta, "finish_reason": finish }],
            "usage": {
                "prompt_tokens": prompt,
                "completion_tokens": completion,
                "prompt_tokens_details": { "cached_tokens": cached },
            },
        })
    }

    /// 从 reasoning 条目里收 encrypted_content（回放时当 reasoning item 带回）。
    /// 同一条目可能先在 output_item.done、后在 completed.output[] 出现，按密文去重。
    fn capture_reasoning_item(&mut self, item: &Value) {
        if item.get("type").and_then(Value::as_str) != Some("reasoning") {
            return;
        }
        if let Some(enc) = item.get("encrypted_content").and_then(Value::as_str)
            && !enc.is_empty()
        {
            self.reasoning_encrypted = Some(enc.to_string());
            let dup = self
                .reasoning_blocks
                .iter()
                .any(|b| b.get("encrypted_content").and_then(Value::as_str) == Some(enc));
            if !dup {
                self.reasoning_blocks
                    .push(json!({ "type": "reasoning", "encrypted_content": enc }));
            }
        }
    }

    /// 翻译一个 data: JSON（Responses 的 data 自带 type 字段，event: 行可忽略）。
    /// Err = 渠道流内报错（response.failed / error 事件），上层以人话终止本轮。
    pub fn translate(&mut self, data: &Value) -> Result<Translated, String> {
        let typ = data.get("type").and_then(Value::as_str).unwrap_or("");
        match typ {
            "response.output_text.delta" => {
                let t = data.get("delta").and_then(Value::as_str).unwrap_or("");
                if t.is_empty() {
                    return Ok(Translated::Ignore);
                }
                Ok(Translated::Chunk(json!({
                    "choices": [{ "delta": { "content": t }, "finish_reason": null }]
                })))
            }
            // o 系/gpt-5 的推理摘要增量：当思考展示（同 anthropic thinking_delta 的去处）
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                let t = data.get("delta").and_then(Value::as_str).unwrap_or("");
                if t.is_empty() {
                    return Ok(Translated::Ignore);
                }
                Ok(Translated::Chunk(json!({
                    "choices": [{ "delta": { "reasoning_content": t }, "finish_reason": null }]
                })))
            }
            "response.output_item.added" => {
                let item = data.get("item").cloned().unwrap_or(json!({}));
                if item.get("type").and_then(Value::as_str) == Some("function_call") {
                    let idx = self.call_count;
                    self.call_count += 1;
                    let id = item
                        .get("call_id")
                        .or_else(|| item.get("id"))
                        .cloned()
                        .unwrap_or(json!(format!("call_{idx}")));
                    let name = item.get("name").cloned().unwrap_or(json!(""));
                    return Ok(Translated::Chunk(json!({
                        "choices": [{ "delta": { "tool_calls": [{
                            "index": idx,
                            "id": id,
                            "type": "function",
                            "function": { "name": name, "arguments": "" },
                        }] }, "finish_reason": null }]
                    })));
                }
                Ok(Translated::Ignore)
            }
            "response.function_call_arguments.delta" => {
                if self.call_count == 0 {
                    return Ok(Translated::Ignore);
                }
                let args = data.get("delta").and_then(Value::as_str).unwrap_or("");
                if args.is_empty() {
                    return Ok(Translated::Ignore);
                }
                Ok(Translated::Chunk(json!({
                    "choices": [{ "delta": { "tool_calls": [{
                        "index": self.call_count - 1,
                        "function": { "arguments": args },
                    }] }, "finish_reason": null }]
                })))
            }
            // output_item.done：函数调用参数已增量发出、忽略；reasoning 条目收加密内容
            "response.output_item.done" => {
                if let Some(item) = data.get("item") {
                    self.capture_reasoning_item(item);
                }
                Ok(Translated::Ignore)
            }
            "response.completed" | "response.incomplete" => {
                let resp = data.get("response").cloned().unwrap_or(json!({}));
                if let Some(items) = resp.get("output").and_then(Value::as_array) {
                    for item in items {
                        self.capture_reasoning_item(item);
                    }
                }
                let usage = resp.get("usage").cloned().unwrap_or(json!({}));
                Ok(Translated::Chunk(self.finish_chunk(&usage)))
            }
            "response.failed" => {
                let msg = data
                    .pointer("/response/error/message")
                    .or_else(|| data.pointer("/response/status"))
                    .map(|v| v.as_str().unwrap_or(&v.to_string()).to_string())
                    .unwrap_or_else(|| "响应失败".into());
                Err(msg)
            }
            "error" => {
                let msg = data.pointer("/error/message").and_then(Value::as_str).unwrap_or("未知错误");
                Err(msg.to_string())
            }
            _ => Ok(Translated::Ignore),
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use super::Translated::*;

    #[allow(dead_code)] // 部分用例走 chunk_of 解包
    fn feed(tr: &mut SseTranslator, v: Value) -> Result<Translated, String> {
        tr.translate(&v)
    }

    fn chunk_of(tr: &mut SseTranslator, v: Value) -> Value {
        match tr.translate(&v) {
            Ok(Chunk(c)) => c,
            other => panic!("预期内容分片，实际 {:?}", other),
        }
    }

    fn openai_compat() -> super::super::compat::Compat {
        super::super::compat::resolve(None, Some("https://x.example.com"), "m", None)
    }

    #[test]
    fn responses_url_suffix_rules() {
        assert_eq!(
            responses_url("https://api.openai.com"),
            "https://api.openai.com/v1/responses"
        );
        assert_eq!(
            responses_url("https://api.openai.com/v1"),
            "https://api.openai.com/v1/responses"
        );
        assert_eq!(
            responses_url("https://x.example.com/responses"),
            "https://x.example.com/responses"
        );
    }

    #[test]
    fn build_body_maps_system_user_tool_roundtrip() {
        let msgs = json!([
            { "role": "system", "content": "你是助手" },
            { "role": "user", "content": "查天气" },
            { "role": "assistant", "content": null, "tool_calls": [
                { "id": "call_1", "type": "function", "function": { "name": "get_weather", "arguments": "{\"city\":\"北京\"}" } }
            ]},
            { "role": "tool", "tool_call_id": "call_1", "content": "晴 25 度" },
            { "role": "assistant", "content": "北京今天晴。" },
        ]);
        let params = super::super::ChatParams::default();
        let body = build_body("m", &msgs, false, &params, None, &openai_compat(), "");
        assert_eq!(body["instructions"], json!("你是助手"));
        assert_eq!(body["input"].as_array().unwrap().len(), 4, "null 正文不产生条目");
        assert_eq!(body["input"][0]["role"], json!("user"));
        assert_eq!(body["input"][1]["type"], json!("function_call"));
        assert_eq!(body["input"][1]["call_id"], json!("call_1"));
        assert_eq!(body["input"][2]["type"], json!("function_call_output"));
        assert_eq!(body["input"][2]["output"], json!("晴 25 度"));
        assert_eq!(body["input"][3]["role"], json!("assistant"));
        // 严格端点（StepFun）只认字符串形态：数组 output_text 会 400
        assert_eq!(body["input"][3]["content"], json!("北京今天晴。"));
        assert!(body.get("max_output_tokens").is_none(), "无上限时不发字段");
    }

    #[test]
    fn build_body_maps_images_and_tools() {
        let msgs = json!([
            { "role": "user", "content": [
                { "type": "text", "text": "看图" },
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAA" } }
            ]}
        ]);
        let tools = json!([
            { "type": "function", "function": { "name": "f", "description": "d", "parameters": { "type": "object" } } }
        ]);
        let params = super::super::ChatParams {
            max_tokens: Some(100),
            ..super::super::ChatParams::default()
        };
        let body = build_body("m", &msgs, true, &params, Some(&tools), &openai_compat(), "");
        assert_eq!(body["max_output_tokens"], json!(100));
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["input"][0]["content"][1]["type"], json!("input_image"));
        assert_eq!(body["tools"][0]["name"], json!("f"), "工具定义拆平");
        assert!(body["tools"][0].get("function").is_none(), "不再嵌套 function");
    }

    #[test]
    fn build_body_maps_input_audio() {
        // 媒体旁路的音频变体按 Responses 形状翻译：input_audio 原样保留
        let msgs = json!([
            { "role": "user", "content": [
                { "type": "input_audio", "input_audio": { "data": "AAA", "format": "wav" } },
                { "type": "text", "text": "听一下" }
            ]}
        ]);
        let params = super::super::ChatParams::default();
        let body = build_body("m", &msgs, false, &params, None, &openai_compat(), "");
        let parts = body["input"][0]["content"].as_array().unwrap();
        assert_eq!(parts[0]["type"], json!("input_audio"));
        assert_eq!(parts[0]["input_audio"]["format"], json!("wav"));
        assert_eq!(parts[1]["type"], json!("input_text"));
    }

    #[test]
    fn body_sends_reasoning_effort_summary_and_cache_key() {
        use super::super::ChatParams;
        let msgs = json!([{ "role": "user", "content": "hi" }]);
        let official = super::super::compat::resolve(
            Some("openai"),
            Some("https://api.openai.com/v1"),
            "gpt-5.1",
            None,
        );
        assert!(official.supports_prompt_cache_key, "官方端点应允许缓存键");
        // 官方模型：档位 + summary:auto + 会话级 prompt_cache_key
        let p = ChatParams {
            thinking: Some("high".into()),
            ..ChatParams::default()
        };
        let b = build_body("gpt-5.1", &msgs, true, &p, None, &official, "sess-1");
        assert_eq!(b["reasoning"], json!({ "effort": "high", "summary": "auto" }));
        assert_eq!(b["prompt_cache_key"], json!("sess-1"));
        // on 折算 medium；off / 未设置不发
        let b = build_body("gpt-5.1", &msgs, true, &ChatParams { thinking: Some("on".into()), ..ChatParams::default() }, None, &official, "s");
        assert_eq!(b["reasoning"]["effort"], json!("medium"));
        let b = build_body("gpt-5.1", &msgs, true, &ChatParams { thinking: Some("off".into()), ..ChatParams::default() }, None, &official, "s");
        assert!(b.get("reasoning").is_none());
        let b = build_body("gpt-5.1", &msgs, true, &ChatParams::default(), None, &official, "s");
        assert!(b.get("reasoning").is_none());
        // 渠道显式关思考（thinking_style=none）不发
        let mut none = official.clone();
        none.thinking_format = ThinkingFormat::None;
        let b = build_body(
            "gpt-5.1",
            &msgs,
            true,
            &ChatParams { thinking: Some("high".into()), ..ChatParams::default() },
            None,
            &none,
            "s",
        );
        assert!(b.get("reasoning").is_none());
        // 兼容端点：只发 effort、不发 summary / cache_key；max 不能静默掉成 low
        let compat = openai_compat();
        assert!(!compat.supports_prompt_cache_key);
        let b = build_body(
            "deepseek-flash",
            &msgs,
            true,
            &ChatParams { thinking: Some("max".into()), ..ChatParams::default() },
            None,
            &compat,
            "s",
        );
        assert_eq!(b["reasoning"], json!({ "effort": "high" }), "max 夹到 high 而不是 low");
        assert!(b.get("prompt_cache_key").is_none(), "非官方端点不发缓存键");
    }

    #[test]
    fn to_openai_completion_extracts_text_and_tools() {
        let data = json!({
            "output": [
                { "type": "message", "content": [{ "type": "output_text", "text": "你好" }] },
                { "type": "function_call", "call_id": "call_9", "name": "f", "arguments": "{\"a\":1}" }
            ],
            "usage": {
                "input_tokens": 12, "output_tokens": 34,
                "input_tokens_details": { "cached_tokens": 8 }
            }
        });
        let out = to_openai_completion(&data);
        assert_eq!(out["choices"][0]["message"]["content"], json!("你好"));
        assert_eq!(out["choices"][0]["message"]["tool_calls"][0]["id"], json!("call_9"));
        assert_eq!(out["choices"][0]["finish_reason"], json!("tool_calls"));
        assert_eq!(out["usage"]["prompt_tokens"], json!(12));
        assert_eq!(out["usage"]["completion_tokens"], json!(34));
        // 缓存命中数（input_tokens 的子集）要带给用量面板；缺了命中率恒 0
        assert_eq!(out["usage"]["prompt_tokens_details"]["cached_tokens"], json!(8));
    }

    #[test]
    fn sse_tool_call_roundtrip_and_finish() {
        let mut tr = SseTranslator::default();
        let c = chunk_of(
            &mut tr,
            json!({"type":"response.output_text.delta","delta":"你好"}),
        );
        assert_eq!(c["choices"][0]["delta"]["content"], json!("你好"));
        let c = chunk_of(
            &mut tr,
            json!({"type":"response.output_item.added","item":{
                "type":"function_call","call_id":"call_2","name":"get_weather"}}),
        );
        assert_eq!(c["choices"][0]["delta"]["tool_calls"][0]["id"], json!("call_2"));
        assert_eq!(
            c["choices"][0]["delta"]["tool_calls"][0]["function"]["name"],
            json!("get_weather")
        );
        let c = chunk_of(
            &mut tr,
            json!({"type":"response.function_call_arguments.delta","delta":"{\"c\""}),
        );
        assert_eq!(c["choices"][0]["delta"]["tool_calls"][0]["index"], json!(0));
        assert_eq!(
            c["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"],
            json!("{\"c\"")
        );
        let c = chunk_of(
            &mut tr,
            json!({"type":"response.completed","response":{
                "usage": { "input_tokens": 7, "output_tokens": 9,
                    "input_tokens_details": { "cached_tokens": 5 } }}}),
        );
        assert_eq!(c["choices"][0]["finish_reason"], json!("tool_calls"));
        assert_eq!(c["usage"]["prompt_tokens"], json!(7));
        assert_eq!(c["usage"]["completion_tokens"], json!(9));
        assert_eq!(c["usage"]["prompt_tokens_details"]["cached_tokens"], json!(5));
    }

    #[test]
    fn sse_completed_carries_encrypted_reasoning() {
        // 思考条目加密内容随收尾分片带出（下一轮回放成 reasoning item）
        let mut tr = SseTranslator::default();
        let _ = tr.translate(&json!({
            "type":"response.output_item.done",
            "item":{"type":"reasoning","encrypted_content":"enc-1"}
        }));
        let c = chunk_of(&mut tr, json!({"type":"response.completed","response":{"usage":{}}}));
        assert_eq!(c["choices"][0]["delta"]["reasoning_encrypted"], json!("enc-1"));
    }

    /// 一条回答多个 reasoning 条目：按到达顺序收集、重复报告去重。
    #[test]
    fn sse_collects_multiple_reasoning_items() {
        let mut tr = SseTranslator::default();
        let _ = tr.translate(&json!({"type":"response.output_item.done","item":{"type":"reasoning","encrypted_content":"enc-1"}}));
        let _ = tr.translate(&json!({"type":"response.output_item.done","item":{"type":"reasoning","encrypted_content":"enc-2"}}));
        // completed 再报一次 enc-1（官方两种事件都可能带 output[]）
        let c = chunk_of(
            &mut tr,
            json!({"type":"response.completed","response":{"usage":{},"output":[{"type":"reasoning","encrypted_content":"enc-1"}]}}),
        );
        let blocks = c["choices"][0]["delta"]["reasoning_blocks"].as_array().unwrap();
        assert_eq!(blocks.len(), 2, "去重后保留两条");
        assert_eq!(blocks[0]["type"], json!("reasoning"));
        assert_eq!(blocks[0]["encrypted_content"], json!("enc-1"));
        assert_eq!(blocks[1]["encrypted_content"], json!("enc-2"));
    }

    /// 多个 reasoning 条目按序回放；来源指纹不符时一条都不回放。
    #[test]
    fn build_body_replays_multiple_reasoning_items_and_guards_source() {
        let msgs = json!([
            {"role":"user","content":"2+2?"},
            {"role":"assistant","content":"4","reasoning_model":"m",
             "reasoning_source":"responses|https://api.openai.com/v1|m",
             "reasoning_blocks":[
                {"type":"reasoning","encrypted_content":"enc-1"},
                {"type":"reasoning","encrypted_content":"enc-2"}
             ]}
        ]);
        let mut p = super::super::ChatParams {
            base_url: Some("https://api.openai.com/v1".into()),
            ..super::super::ChatParams::default()
        };
        let b = build_body("m", &msgs, false, &p, None, &openai_compat(), "s");
        let input = b["input"].as_array().unwrap();
        let items: Vec<&Value> = input.iter().filter(|it| it["type"] == json!("reasoning")).collect();
        assert_eq!(items.len(), 2, "{input:?}");
        assert_eq!(items[0]["encrypted_content"], json!("enc-1"));
        assert_eq!(items[1]["encrypted_content"], json!("enc-2"));
        // 换端点：来源不符，不回放
        p.base_url = Some("https://relay.example.com".into());
        let b = build_body("m", &msgs, false, &p, None, &openai_compat(), "s");
        assert!(
            !b["input"].as_array().unwrap().iter().any(|it| it["type"] == json!("reasoning")),
            "换端点不应回放"
        );
    }

    #[test]
    fn build_body_replays_encrypted_reasoning_for_same_model() {
        let msgs = json!([
            {"role":"user","content":"1+1?"},
            {"role":"assistant","content":"2","reasoning_content":"想","reasoning_encrypted":"enc-1","reasoning_model":"deepseek-flash"},
            {"role":"user","content":"2+2?"}
        ]);
        let p = super::super::ChatParams::default();
        let b = build_body("deepseek-flash", &msgs, false, &p, None, &openai_compat(), "");
        let input = b["input"].as_array().unwrap();
        assert!(
            input.iter().any(|it| it["type"] == json!("reasoning") && it["encrypted_content"] == json!("enc-1")),
            "{input:?}"
        );
        assert_eq!(b["store"], json!(false));
        assert_eq!(b["include"][0], json!("reasoning.encrypted_content"));

        let b = build_body("other-model", &msgs, false, &p, None, &openai_compat(), "");
        let input = b["input"].as_array().unwrap();
        assert!(!input.iter().any(|it| it["type"] == json!("reasoning")), "换模型不回放");
    }

    #[test]
    fn sse_reasoning_and_text_only_finish() {
        let mut tr = SseTranslator::default();
        let c = chunk_of(
            &mut tr,
            json!({"type":"response.reasoning_summary_text.delta","delta":"想"}),
        );
        assert_eq!(c["choices"][0]["delta"]["reasoning_content"], json!("想"));
        let c = chunk_of(
            &mut tr,
            json!({"type":"response.output_text.delta","delta":"答"}),
        );
        assert_eq!(c["choices"][0]["delta"]["content"], json!("答"));
        let c = chunk_of(
            &mut tr,
            json!({"type":"response.completed","response":{"usage":{}}}),
        );
        assert_eq!(c["choices"][0]["finish_reason"], json!("stop"));
    }

    #[test]
    fn sse_failed_is_error() {
        let mut tr = SseTranslator::default();
        let e = match tr.translate(&json!({
            "type":"response.failed",
            "response":{"error":{"message":"配额不足"}}
        })) {
            Err(e) => e,
            other => panic!("预期错误，实际 {:?}", other),
        };
        assert_eq!(e, "配额不足");
        let e = match tr.translate(&json!({"type":"error","error":{"message":"boom"}})) {
            Err(e) => e,
            other => panic!("预期错误，实际 {:?}", other),
        };
        assert_eq!(e, "boom");
    }
}
