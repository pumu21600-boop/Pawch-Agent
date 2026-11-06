//! 身份问答的「干净上下文探测」——2026-10-27 彻查的落地。
//!
//! 真机矩阵结论：同一个 step-5-preview，裸请求 5/5 答「Step」；只给一句
//! 「用中文简洁回答」+ 19 个工具时，它会随机答 Step/Kimi/Claude/Kiro（把 agent
//! 工具集当成别家系统提示的指纹，是 pattern completion，不是自我认知）；把
//! 「按自己的训练认知回答」写进系统提示后仍有概率被长上下文带偏。因此身份问题
//! 不再让带工具的主循环答：**用同一渠道发一次无工具、无系统提示的干净请求**，
//! 让模型自己说；结果缓存 1 小时，纯身份问题直接走这条快路。
//!
//! 这不是「念配置」也不是「写死产品名」：答案来自模型本身在干净上下文里的
//! 自述。缓存只存答案与时间，不存 key（key 只参与 cache key 的哈希）。

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::gateway::{Channel, ChatOutcome, UsageCtx};
use crate::server::AppState;

/// 探测问题：明确要求一句、模型名+厂商、不解释、不调工具。
/// 真机 5/5 稳定答「Step，由阶跃星辰（StepFun）开发」。
pub(crate) const PROBE_QUESTION: &str = "你是谁？你是什么模型？直接回答模型名和厂商，一句话，不要解释，不要调用工具。";

const TTL: Duration = Duration::from_secs(3600);

struct Entry {
    answer: String,
    at: Instant,
}

static CACHE: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<String, Entry>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_key(ch: &Channel) -> String {
    // key 只做哈希，不落字符串、不落日志；换 key 视为换渠道。
    let mut h = std::collections::hash_map::DefaultHasher::new();
    ch.api_key.hash(&mut h);
    format!(
        "{}|{}|{}|{:x}",
        ch.base_url.trim_end_matches('/'),
        ch.model,
        ch.params.provider_id.as_deref().unwrap_or(""),
        h.finish()
    )
}

/// 取缓存（未过期）。过期条目顺手清掉，避免长跑进程无界增长。
pub(crate) fn cached(ch: &Channel) -> Option<String> {
    let key = cache_key(ch);
    let mut g = cache().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(e) = g.get(&key) {
        if e.at.elapsed() < TTL {
            return Some(e.answer.clone());
        }
        g.remove(&key);
    }
    None
}

/// 清空探测缓存：回滚=回到该步初始状态，下一次身份问题必须重新问模型，
/// 不能拿回滚前的缓存答案冒充新回答（2026-10-27 真机实测「秒出=复用」）。
/// 缓存是渠道级的，清掉只影响下一次探测多走一次真实请求，不伤会话数据。
pub(crate) fn clear_cache() {
    cache().lock().unwrap_or_else(|e| e.into_inner()).clear();
}

fn store(ch: &Channel, answer: &str) {
    let answer = sanitize(answer);
    if answer.is_empty() {
        return;
    }
    cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(cache_key(ch), Entry { answer, at: Instant::now() });
}

/// 只保留第一行、去掉 markdown 引号/反引号，封顶 200 字符。
fn sanitize(s: &str) -> String {
    let first = s
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let first = first.trim_matches(|c: char| {
        c.is_whitespace()
            || matches!(c, '`' | '"' | '\'' | '“' | '”' | '‘' | '’' | '「' | '」' | '『' | '』')
    });
    first.chars().take(200).collect()
}

/// 纯身份问题识别（短句、命中身份关键词、不含任务动词）。
/// 「你是什么模型1」「你是谁？」「你是什么模型？直接说模型名」都算；
/// 「你是谁？帮我改文件」不算（交给主循环）。
pub(crate) fn is_identity_question(text: &str) -> bool {
    let raw = text.trim();
    if raw.is_empty() || raw.chars().count() > 40 {
        return false;
    }
    let t = raw.trim_end_matches(|c: char| {
        c.is_ascii_digit() || c.is_whitespace() || matches!(c, '?' | '？' | '!' | '！' | '。' | '.' | '~' | '～' | ',' | '，')
    });
    let lower = t.to_lowercase();
    let hit = [
        "你是什么模型",
        "你是什么大模型",
        "你用的什么模型",
        "你是谁",
        "你的身份",
        "什么模型",
        "哪个模型",
        "who are you",
        "what model are you",
        "what model",
    ]
    .iter()
    .any(|k| lower.contains(k));
    if !hit {
        return false;
    }
    let taskish = [
        "帮我", "写", "改", "读", "打开", "搜索", "文件", "代码", "翻译", "总结", "看看", "查一下",
        "安装", "运行", "执行", "任务", "项目", "插件", "技能", "介绍", "说说", "能力", "区别", "原理",
        "对比", "怎么", "如何",
    ];
    !taskish.iter().any(|k| t.contains(k))
}

/// 干净上下文探测：无工具、无系统提示、低温、短输出。走**流式**（与主对话同路）——
/// 免费/中转渠道常只支持流式，非流式 `complete` 会返回空内容（2026-10-27 真机：
/// tokeness 免费渠道就是这种）。失败返回 Err 由调用方决定是否退回普通对话。
pub(crate) async fn probe(state: &AppState, ch: &Channel, session_id: &str) -> anyhow::Result<String> {
    if let Some(a) = cached(ch) {
        return Ok(a);
    }
    let mut probe_ch = ch.clone();
    // 512 而不是 128：部分渠道不认 thinking=off，推理 token 会把 128 吃光，
    // 正文为空（2026-10-27 tokeness 真机踩到）；身份回答本身只有一句话。
    probe_ch.params.max_tokens = Some(512);
    probe_ch.params.temperature = Some(0.0);
    probe_ch.params.top_p = Some(0.1);
    probe_ch.params.thinking = Some("off".to_string());
    let msgs = json!([{ "role": "user", "content": PROBE_QUESTION }]);
    let ctx = UsageCtx::aux(session_id, "identity");
    let cancel = crate::core::cancel::CancelFlag::new();
    let answer = match state
        .gateway
        .stream_chat(&ctx, &probe_ch, &msgs, None, &cancel, None, |_| {})
        .await?
    {
        ChatOutcome::Done(t) => t.content,
        ChatOutcome::Interrupted(t) => t.content,
        ChatOutcome::Cancelled => anyhow::bail!("身份探测被取消"),
    };
    let answer = sanitize(&answer);
    if answer.is_empty() {
        anyhow::bail!("模型没有返回身份信息");
    }
    store(ch, &answer);
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_question_detection() {
        for q in [
            "你是什么模型",
            "你是什么模型1",
            "你是谁？",
            "你是什么模型？直接说模型名和厂商，不要解释。",
            "who are you",
        ] {
            assert!(is_identity_question(q), "应识别为身份问题: {q}");
        }
        for q in ["你是谁？帮我改文件", "你是什么模型，介绍一下能力", "帮我写个函数", "你好"] {
            assert!(!is_identity_question(q), "不该拦截: {q}");
        }
    }

    #[test]
    fn sanitize_keeps_one_line() {
        assert_eq!(sanitize("我是 Step，由阶跃星辰开发。\n多余"), "我是 Step，由阶跃星辰开发。");
        assert_eq!(sanitize("`Step`"), "Step");
        assert_eq!(sanitize("  "), "");
    }

    #[test]
    fn clear_cache_drops_entries() {
        let key = "__clear_cache_test__".to_string();
        cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.clone(), Entry { answer: "Step".into(), at: Instant::now() });
        clear_cache();
        assert!(!cache().lock().unwrap_or_else(|e| e.into_inner()).contains_key(&key));
    }
}
