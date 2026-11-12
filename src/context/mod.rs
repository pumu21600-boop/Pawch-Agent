//! 上下文工程 v1
//!
//! 1. token 启发式估算：CJK ≈1 token/字，ASCII ≈1 token/4 字符，其余 ≈1/2——无 tokenizer 依赖。
//! 2. 工具结果头尾压缩：头部 60% + 尾部 40%，中间插省略标记（替代旧的单向硬截断）。
//! 3. 历史裁剪：每轮调模型前估算；超预算先归档旧工具结果（保留最近 N 个），
//!    仍超则按「轮」（user 开头的一组）从旧到新整组折叠成占位——保持 tool_calls/tool 配对完整。
//!    system、最后一条 user、最近 N 个工具结果永不裁。


use serde_json::{json, Value};

/// 头尾压缩时插入的省略标记（工具结果 truncated 判定依据）。
pub const OMITTED_MARK: &str = "……[中间省略";

/// 启发式 token 估算。
pub fn estimate_tokens(text: &str) -> u64 {
    let mut cjk: u64 = 0;
    let mut ascii: u64 = 0;
    let mut other: u64 = 0;
    for ch in text.chars() {
        if ('\u{4E00}'..='\u{9FFF}').contains(&ch)
            || ('\u{3000}'..='\u{303F}').contains(&ch)
            || ('\u{FF00}'..='\u{FFEF}').contains(&ch)
        {
            cjk += 1;
        } else if ch.is_ascii() {
            ascii += 1;
        } else {
            other += 1;
        }
    }
    cjk + other.div_ceil(2) + ascii.div_ceil(4)
}

/// 界面专用气泡（通知/错误）：随快照落盘给界面回放，发送前由 gateway replay 剥离
/// （`apply_outgoing_transforms`），**模型永远看不到**——对齐 DSH「只给模型可见内容计价」，
/// 计价 0，否则估算/预算/圆环都会被没发出去的内容顶高。
fn is_display_only(m: &Value) -> bool {
    m.get("display_only").and_then(Value::as_bool) == Some(true)
}

/// 思考计入上下文的口径（估算 / 压缩守卫 / 圆环共用）：
/// - `None`：不回放，思考不占上下文
/// - `All`：按渠道契约整段回放（Chat Completions 的 DeepSeek/Kimi 系）
/// - `Replayable`：只算真能回放的思考（Anthropic 带签名 thinking 块 /
///   Responses 带 encrypted_content 的 reasoning 条目）——没有回放元数据的
///   历史思考不会发出去，不能计入
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReasoningCount {
    #[default]
    None,
    All,
    Replayable,
}

impl From<bool> for ReasoningCount {
    fn from(v: bool) -> Self {
        if v { Self::All } else { Self::None }
    }
}

fn counts_reasoning(m: &Value, mode: ReasoningCount) -> bool {
    match mode {
        ReasoningCount::None => false,
        ReasoningCount::All => true,
        ReasoningCount::Replayable => {
            // 块序列是最新形状（多思考块 / redacted 密文块 / Responses 多条 reasoning）。
            // 只认真的能回放的块：thinking 带签名、redacted 带密文、reasoning 带密文
            let has_blocks = m.get("reasoning_blocks").and_then(Value::as_array).is_some_and(|arr| {
                arr.iter().any(|b| match b.get("type").and_then(Value::as_str).unwrap_or("") {
                    "thinking" => b.get("signature").and_then(Value::as_str).is_some_and(|s| !s.is_empty()),
                    "redacted_thinking" => b.get("data").and_then(Value::as_str).is_some_and(|s| !s.is_empty()),
                    "reasoning" => b
                        .get("encrypted_content")
                        .and_then(Value::as_str)
                        .is_some_and(|s| !s.is_empty()),
                    _ => false,
                })
            });
            has_blocks
                || m.get("reasoning_signature").and_then(Value::as_str).is_some_and(|s| !s.is_empty())
                || m.get("reasoning_encrypted").and_then(Value::as_str).is_some_and(|s| !s.is_empty())
        }
    }
}

/// 加密/密文思考条目的代币估算（2026-11-11）：服务端会把密文展开成思考 token
/// 计费/占上下文，本地只能按长度保守估——base64 解码后字节 ≈ 原文，token ≈ bytes/4
/// （即 base64 长度 × 3/16）。redacted_thinking 的 data 同理。
fn encrypted_reasoning_tokens(m: &Value) -> u64 {
    let mut total = 0u64;
    if let Some(arr) = m.get("reasoning_blocks").and_then(Value::as_array) {
        for b in arr {
            match b.get("type").and_then(Value::as_str).unwrap_or("") {
                "reasoning" => {
                    if let Some(enc) = b.get("encrypted_content").and_then(Value::as_str) {
                        total += (enc.len() as u64 * 3 / 16).max(1);
                    }
                }
                "redacted_thinking" => {
                    if let Some(d) = b.get("data").and_then(Value::as_str) {
                        total += (d.len() as u64 * 3 / 16).max(1);
                    }
                }
                _ => {}
            }
        }
    } else if let Some(enc) = m.get("reasoning_encrypted").and_then(Value::as_str) {
        total += (enc.len() as u64 * 3 / 16).max(1);
    }
    total
}

/// 单条消息估算（role 开销 + content + tool_calls；按 `mode` 决定思考是否计入）。
/// 思考是否计入必须跟随「是否真的会回放进请求」——剥离渠道/不可回放的历史思考
/// 不进上下文、不计入（切勿一律计入，否则回滚重算会把没发出去的思考算进圆环）。
fn estimate_message(m: &Value, mode: ReasoningCount) -> u64 {
    if is_display_only(m) {
        return 0;
    }
    let mut total: u64 = 4;
    if let Some(c) = m.get("content") {
        total += value_tokens(c);
    }
    if counts_reasoning(m, mode) {
        let text_tokens = m
            .get("reasoning_content")
            .and_then(Value::as_str)
            .map(estimate_tokens)
            .unwrap_or(0);
        // 可见摘要与密文估算是同一份思考的两种口径（取大），不重复相加
        total += text_tokens.max(encrypted_reasoning_tokens(m));
    }
    if let Some(calls) = m.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            total += 8 + value_tokens(call);
        }
    }
    total
}

fn value_tokens(v: &Value) -> u64 {
    match v {
        Value::String(s) => {
            // 图片 data URL 不能按 base64 文本长度估 token（一张 1.5MB 截图会估出
            // 三十多万 token，圆环瞬间爆表并误裁真实历史）。按一张图固定估值。
            if s.starts_with("data:image/") { 1500 } else { estimate_tokens(s) }
        }
        Value::Array(a) => a.iter().map(value_tokens).sum(),
        // 2026-10-08 修：键名也要计——tool_calls 的 {"id"/"type"/"function"/"name"}
        // 这些结构键原先全漏，一次 write_file 调用少算 15-20 tokens，随步数线性
        // 累积，把裁剪时机系统性推晚（模块注释自己记录过同类偏差：估算 1866 vs 实际 4750）。
        Value::Object(o) => o.iter().map(|(k, v)| estimate_tokens(k) + value_tokens(v)).sum(),
        Value::Null => 0,
        other => estimate_tokens(&other.to_string()),
    }
}

/// 整段消息估算（生产侧统一走 `estimate_breakdown`，这里只给单测当「总额」尺子）。
#[cfg(test)]
pub fn estimate_messages(messages: &[Value], count_reasoning: impl Into<ReasoningCount>) -> u64 {
    let mode = count_reasoning.into();
    messages.iter().map(|m| estimate_message(m, mode)).sum()
}

/// 分类占比（前端「上下文容量」卡片用）：各分类之和 == estimate_messages。
#[derive(Debug, Clone, Copy, Default)]
pub struct Breakdown {
    /// user + assistant 正文
    pub messages: u64,
    /// 工具结果（role=tool）
    pub tools: u64,
    /// 系统提示词（role=system）
    pub system: u64,
    /// tool_calls 结构与每条消息的固定开销
    pub other: u64,
    /// 工具定义（随每次请求发送的 tools JSON Schema，渠道计入 prompt tokens；
    /// 不算它圆环会比真实用量少一大截）。由调用方（核心循环）估算后填入。
    pub tools_schema: u64,
}

impl Breakdown {
    pub fn total(&self) -> u64 {
        self.messages + self.tools + self.system + self.other + self.tools_schema
    }
}

pub fn estimate_breakdown(messages: &[Value], count_reasoning: impl Into<ReasoningCount>) -> Breakdown {
    let mode = count_reasoning.into();
    let mut b = Breakdown::default();
    for m in messages {
        if is_display_only(m) {
            continue;
        }
        match m.get("role").and_then(Value::as_str).unwrap_or("") {
            "system" => b.system += estimate_message(m, mode),
            "tool" => b.tools += estimate_message(m, mode),
            "user" => b.messages += estimate_message(m, mode),
            "assistant" => {
                if let Some(c) = m.get("content") {
                    b.messages += value_tokens(c);
                }
                if counts_reasoning(m, mode) {
                    let text_tokens = m
                        .get("reasoning_content")
                        .and_then(Value::as_str)
                        .map(estimate_tokens)
                        .unwrap_or(0);
                    b.messages += text_tokens.max(encrypted_reasoning_tokens(m));
                }
                if let Some(calls) = m.get("tool_calls").and_then(Value::as_array) {
                    b.other += 8 * calls.len() as u64 + calls.iter().map(value_tokens).sum::<u64>();
                }
                b.other += 4;
            }
            _ => b.other += estimate_message(m, mode),
        }
    }
    b
}

/// 占用口径（对齐 DSH token-meter 的 `projectedTokens`）：**实报锚点 + 自锚点以来的
/// 带符号增量**。`anchor = (渠道实报 prompt_tokens, 那次请求发出时的本地估价)`，
/// `est_now` = 当前上下文的本地估价。
///
/// 三个要点（都是本地自己算增量算不出来的）：
/// 1. 工具结果/追发消息等**在请求之后**落进上下文的部分自动计入（本地估价涨了多少就补多少）；
/// 2. 裁剪/折叠让上下文变小时增量是**负的**，圆环当场回落，不需要清空实报值；
/// 3. 渠道实报与本地估价不是一个尺子（实报是真实分词），所以只拿它当锚，增量用本地尺子量。
///
/// 没用过实报（首请求前）或渠道不回 usage 时 anchor 为 None → 纯估算兜底。
pub fn projected_tokens(anchor: Option<(u64, u64)>, est_now: u64) -> u64 {
    match anchor {
        Some((real, est_at_request)) => {
            let delta = est_now as i128 - est_at_request as i128;
            (real as i128 + delta).clamp(0, u64::MAX as i128) as u64
        }
        None => est_now,
    }
}

/// 头尾压缩：短文本原样返回；超限保留头 60% + 尾 40%。
pub fn compress(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    let head = max_chars * 6 / 10;
    let tail = max_chars - head;
    let head_s: String = text.chars().take(head).collect();
    let tail_s: String = text.chars().skip(total - tail).collect();
    let omitted = total - head - tail;
    format!("{head_s}\n{OMITTED_MARK} {omitted} 字符，完整内容见日志]……\n{tail_s}")
}

/// 重复工具结果去重（2026-11-01，阈值无关的第一道卫生）：
/// 同一份内容（>300 字符，与归档同一道杠）在请求里出现多次时，只留第一份原文，
/// 之后的换成占位符。专治「模型把同一张 8k 整页回读 12 遍、同一条错误结果
/// 重发 40 遍」——相同内容重复附上不增加任何信息，只把上下文喂肥（实测那轮
/// 72k 上下文里约 3 万 token 是纯重复，模型随后退化、端点开始吐残缺调用）。
/// 返回 (新 messages, 去重条数)。第一份永远保留原文（含最近窗口内的）。
pub fn dedupe_tool_results(messages: &[Value]) -> (Vec<Value>, usize) {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = messages.to_vec();
    let mut deduped = 0usize;
    for m in out.iter_mut() {
        if m.get("role").and_then(Value::as_str) != Some("tool") {
            continue;
        }
        let Some(content) = m.get("content").and_then(Value::as_str) else { continue };
        if content.chars().count() <= 300 {
            continue; // 小结果不占地方，也可能是状态类短回包，别动
        }
        if !seen.insert(content.to_string()) {
            let name = m.get("name").and_then(Value::as_str).unwrap_or("tool");
            m["content"] = json!(format!("[重复工具结果已省略：{name} 的内容与此前结果完全相同，未重复附上]"));
            deduped += 1;
        }
    }
    (out, deduped)
}

/// 大结果硬截断（2026-11-01，阈值无关的第二道卫生）：单条工具结果超过
/// `max_chars` 字符按头 60%/尾 40% 压缩并带省略标记（复用 compress）。
/// `max_chars=0` 关闭。返回 (新 messages, 截断条数)。
pub fn truncate_oversized_results(messages: &[Value], max_chars: usize) -> (Vec<Value>, usize) {
    if max_chars == 0 {
        return (messages.to_vec(), 0);
    }
    let mut out = messages.to_vec();
    let mut truncated = 0usize;
    for m in out.iter_mut() {
        if m.get("role").and_then(Value::as_str) != Some("tool") {
            continue;
        }
        let Some(content) = m.get("content").and_then(Value::as_str) else { continue };
        if content.chars().count() > max_chars {
            m["content"] = json!(compress(content, max_chars));
            truncated += 1;
        }
    }
    (out, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_estimation_cjk_vs_ascii() {
        // 纯 ASCII：约 1 token / 4 字符
        let ascii = "a".repeat(400);
        assert_eq!(estimate_tokens(&ascii), 100);
        // 纯 CJK：1 token / 字
        let cjk = "中文测试".repeat(25); // 100 字
        assert_eq!(estimate_tokens(&cjk), 100);
        // 混合：CJK 全算 + ASCII/4
        let mixed = format!("{cjk}{ascii}");
        assert_eq!(estimate_tokens(&mixed), 200);
    }

    #[test]
    fn compress_keeps_head_and_tail() {
        let short = "hello";
        assert_eq!(compress(short, 100), "hello");
        let long: String = std::iter::repeat_n('x', 10_000).collect();
        let out = compress(&long, 800);
        assert!(out.contains(OMITTED_MARK));
        assert!(out.starts_with("xxx"));
        assert!(out.ends_with("xxx"));
        assert!(out.chars().count() < 10_000);
    }



    #[test]
    fn reasoning_counts_only_when_replayed() {
        // 思考计入与否跟随回放判定：回放渠道（DeepSeek/zai 契约、白名单）思考每轮
        // 重发进 prompt，预算要看见；剥离渠道不计入，否则过度裁剪
        let msgs = vec![json!({
            "role": "assistant",
            "content": "好的",
            "reasoning_content": "思".repeat(100),
        })];
        let stripped = estimate_messages(&msgs, false);
        let replayed = estimate_messages(&msgs, true);
        assert_eq!(stripped, 4 + 2, "剥离渠道：只算 role 开销 + 正文");
        assert_eq!(replayed, stripped + 100, "回放渠道：思考按 CJK 1 token/字计入");
        assert_eq!(estimate_breakdown(&msgs, true).messages, 2 + 100, "breakdown 同口径");
        assert_eq!(estimate_breakdown(&msgs, false).messages, 2);
    }

    #[test]
    fn replayable_counts_only_signed_or_encrypted_reasoning() {
        // Anthropic/Responses 只回放带签名/加密条目的思考：旧消息（无元数据）不算
        let msgs = vec![
            json!({"role":"assistant","content":"答","reasoning_content":"思".repeat(100)}),
            json!({"role":"assistant","content":"答","reasoning_content":"思".repeat(100),"reasoning_signature":"sig"}),
            json!({"role":"assistant","content":"答","reasoning_content":"思".repeat(100),"reasoning_encrypted":"enc"}),
        ];
        let none = estimate_messages(&msgs, ReasoningCount::None);
        let replayable = estimate_messages(&msgs, ReasoningCount::Replayable);
        let all = estimate_messages(&msgs, ReasoningCount::All);
        assert_eq!(replayable, none + 200, "只计两条可回放的思考");
        assert_eq!(all, none + 300, "All 全计");
        assert!(none < replayable && replayable < all);
    }

    #[test]
    fn encrypted_and_redacted_blocks_are_priced_by_cipher_length() {
        // 块序列没有 legacy 字段时也要计入；密文块按 base64 长度估算（len*3/16）
        let plain = json!({"role":"assistant","content":"答"});
        let enc = json!({"role":"assistant","content":"答",
            "reasoning_blocks":[{"type":"reasoning","encrypted_content":"A".repeat(160)}]});
        let red = json!({"role":"assistant","content":"答",
            "reasoning_blocks":[{"type":"redacted_thinking","data":"B".repeat(160)}]});
        let base = estimate_messages(&[plain], ReasoningCount::Replayable);
        let with_enc = estimate_messages(std::slice::from_ref(&enc), ReasoningCount::Replayable);
        let with_red = estimate_messages(&[red], ReasoningCount::Replayable);
        assert_eq!(with_enc, base + 30, "160*3/16=30");
        assert_eq!(with_red, base + 30);
        assert_eq!(
            estimate_messages(std::slice::from_ref(&enc), ReasoningCount::None),
            base,
            "不回的渠道不计"
        );
        // 可见摘要与密文估算取大、不重复相加
        let both = json!({"role":"assistant","content":"答","reasoning_content":"短",
            "reasoning_blocks":[{"type":"reasoning","encrypted_content":"A".repeat(160)}]});
        assert_eq!(estimate_messages(&[both], ReasoningCount::Replayable), base + 30);
    }




    fn user_msg(s: &str) -> Value {
        json!({ "role": "user", "content": s })
    }
    #[test]
    fn display_only_bubbles_are_not_priced() {
        // 通知/错误气泡只给界面看（发送前被 gateway replay 剥离），不该进估算/预算
        let msgs = vec![
            json!({ "role": "system", "content": "sys" }),
            user_msg("问题"),
            json!({ "role": "system", "content": format!("[通知] {}", "啰嗦".repeat(500)), "display_only": true }),
        ];
        let b = estimate_breakdown(&msgs, false);
        assert_eq!(b.system, 4 + 1, "只算真正的 system 提示词");
        assert_eq!(estimate_messages(&msgs, false), 4 + 1 + 4 + 2, "display_only 计价 0");
    }

    #[test]
    fn projected_follows_anchor_with_signed_delta() {
        // 锚点 + 带符号增量：涨了补上（工具结果/追发消息），缩了回落（裁剪），没锚点用估算
        let anchor = Some((10_000u64, 9_000u64));
        assert_eq!(projected_tokens(anchor, 9_000), 10_000, "上下文没变 → 就是实报值");
        assert_eq!(projected_tokens(anchor, 12_000), 13_000, "新增 3000 估价的工具结果 → 补上");
        assert_eq!(projected_tokens(anchor, 4_000), 5_000, "裁剪掉 5000 → 负增量，当场回落");
        assert_eq!(projected_tokens(anchor, 0), 1_000, "极端缩水不出现负数");
        assert_eq!(projected_tokens(None, 7_777), 7_777, "首请求前/渠道不回 usage → 纯估算");
    }

}

#[cfg(test)]
mod protected_start_tests {
    use super::*;

    fn user_msg(s: &str) -> Value {
        json!({ "role": "user", "content": s })
    }
    fn tool_msg(s: &str) -> Value {
        json!({ "role": "tool", "tool_call_id": "t", "name": "shell", "content": s })
    }
    fn asst_msg() -> Value {
        json!({ "role": "assistant", "content": "好的" })
    }

    /// 2026-11-01 卫生三件套：重复去重 / 大结果截断（阈值无关，预算线以下也生效）。
    /// 病理样本：step-5-preview 死循环轮——同一 8k 整页回读 12 遍、同一条 363 字
    /// 错误重发 40 遍，总量不到预算线，旧机制一次都不动手，模型被肥上下文喂退化。
    #[test]
    fn dedupe_replaces_later_identical_tool_results() {
        let big = "x".repeat(500);
        let mut messages = vec![json!({ "role": "system", "content": "sys" })];
        messages.push(asst_msg());
        messages.push(tool_msg(&big)); // 第一份：保留原文
        messages.push(user_msg("继续"));
        messages.push(asst_msg());
        messages.push(tool_msg(&big)); // 第二份：换占位
        messages.push(asst_msg());
        messages.push(tool_msg(&"x".repeat(400))); // 内容不同：保留
        messages.push(asst_msg());
        messages.push(tool_msg("short")); // ≤300 字符：不参与去重
        let (out, n) = dedupe_tool_results(&messages);
        assert_eq!(n, 1, "只有第二份相同结果被去重");
        let tools: Vec<String> = out
            .iter()
            .filter(|m| m.get("role").and_then(Value::as_str) == Some("tool"))
            .map(|m| m.get("content").and_then(Value::as_str).unwrap_or("").to_string())
            .collect();
        assert_eq!(tools[0], big, "第一份保留原文");
        assert!(tools[1].starts_with("[重复工具结果已省略"), "第二份是占位符: {}", tools[1]);
        assert_eq!(tools[2].len(), 400, "内容不同的不受影响");
        assert_eq!(tools[3], "short");
    }


    #[test]
    fn truncate_oversized_tool_results() {
        let huge = "y".repeat(900);
        let messages = vec![
            json!({ "role": "system", "content": "sys" }),
            asst_msg(),
            tool_msg(&huge),
            asst_msg(),
            tool_msg("tiny"),
        ];
        let (out, n) = truncate_oversized_results(&messages, 300);
        assert_eq!(n, 1);
        let c = out[2].get("content").and_then(Value::as_str).unwrap_or("");
        // 省略标记自身占字符：总长 = max + 标记开销（头 60% + 尾 40% + 中间标注）
        assert!(c.chars().count() <= 300 + 40 && c.contains(OMITTED_MARK), "超限被头尾压缩: {c:?}");
        assert_eq!(out[4].get("content").and_then(Value::as_str), Some("tiny"));
        // max=0 关闭
        let (out2, n2) = truncate_oversized_results(&messages, 0);
        assert_eq!(n2, 0);
        assert_eq!(out2[2].get("content").and_then(Value::as_str), Some(huge.as_str()));
    }



    #[test]
    fn image_data_url_is_not_counted_as_raw_base64_tokens() {
        // 一张 1MB 的 base64 截图按文本估会到几十万 token，圆环爆表+误裁历史
        let big = format!("data:image/png;base64,{}", "A".repeat(1_000_000));
        let msgs = vec![json!({
            "role": "user",
            "content": [{ "type": "image_url", "image_url": { "url": big } }],
        })];
        let est = estimate_messages(&msgs, false);
        assert!(est < 10_000, "图片应按固定估值计，实际 {est}");
    }
}
