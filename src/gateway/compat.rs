//! OpenAI 兼容端点的兼容性探测 —— 移植自 OpenClaw `packages/ai/src/transports/openai-completions-compat.ts`（v2026.8.2）。
//!
//! 核心思想：不按 provider id 硬编码分支，而是按「端点归属」推导请求行为（thinkingFormat、
//! max_tokens 字段、temperature 收不收等），并允许渠道配置覆盖。
//! 本产品无模型目录，探测输入 = 渠道 provider_id（预设）+ base_url + model id，
//! 三者都认不出时按 OpenAI 标准端点处理——与上游「端点归属 + 配置覆盖」一致。

use super::effort;

/// 思考参数的线上形态（上游 thinkingFormat 的本产品子集——openrouter/together 是聚合平台，
/// 本产品只对接官方网关与本地服务，不收录；zai/deepseek/qwen 全保留）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingFormat {
    /// 极简/禁用：绝不向端点发送任何思考相关参数
    None,
    /// OpenAI 原生：reasoning_effort 档位（经 effort::resolve 按模型降级；无法表达"关"时不发）
    #[default]
    Openai,
    /// DeepSeek/小米：thinking 对象 {type:enabled|disabled}；响应读 reasoning_content；
    /// 历史回放保留 reasoning_content 字段
    Deepseek,
    /// 智谱 GLM：thinking 对象 {type:enabled|disabled}（档位无意义）
    Zai,
    /// 通义千问/混元：顶层布尔 enable_thinking
    Qwen,
    /// 本地推理服务聊天模板形态（vLLM/llama.cpp/ollama 的 Qwen3 系）：chat_template_kwargs.enable_thinking
    QwenChatTemplate,
}

impl ThinkingFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            ThinkingFormat::None => "none",
            ThinkingFormat::Openai => "openai",
            ThinkingFormat::Deepseek => "deepseek",
            ThinkingFormat::Zai => "zai",
            ThinkingFormat::Qwen => "qwen",
            ThinkingFormat::QwenChatTemplate => "qwen-chat-template",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "none" => Some(Self::None),
            "openai" => Some(Self::Openai),
            "deepseek" => Some(Self::Deepseek),
            "zai" => Some(Self::Zai),
            "qwen" => Some(Self::Qwen),
            "qwen-chat-template" => Some(Self::QwenChatTemplate),
            _ => None,
        }
    }
}

/// 传输协议维度：同一渠道走 OpenAI Chat Completions 还是 Anthropic Messages。
/// 与 ThinkingFormat 正交——Anthropic 协议的思考参数在 anthropic.rs 里按 Messages 规范组装。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Protocol {
    /// OpenAI 兼容 /chat/completions（含全部国产官方网关、中转、本地推理服务）
    #[default]
    OpenAiCompletions,
    /// Anthropic Messages（/v1/messages：system 顶层、tool_use/tool_result 块、x-api-key 鉴权）
    AnthropicMessages,
    /// OpenAI Responses（/responses：instructions+input 条目、function_call/function_call_output、
    /// 事件流 response.output_text.delta 等；2026-11-11 新增适配）
    OpenAiResponses,
}

impl Protocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            Protocol::OpenAiCompletions => "openai-completions",
            Protocol::AnthropicMessages => "anthropic-messages",
            Protocol::OpenAiResponses => "openai-responses",
        }
    }
    /// 设置字段词表（「API 格式」三选）：openai | anthropic | responses
    pub fn as_settings_str(&self) -> &'static str {
        match self {
            Protocol::OpenAiCompletions => "openai",
            Protocol::AnthropicMessages => "anthropic",
            Protocol::OpenAiResponses => "responses",
        }
    }
}

/// 端点兼容性档（请求组装与响应解析都按它走）。
#[derive(Debug, Clone)]
pub struct Compat {
    /// 传输协议：Anthropic 端点走 Messages 适配器，其余走 OpenAI Completions
    pub protocol: Protocol,
    pub thinking_format: ThinkingFormat,
    /// "max_tokens" 或 "max_completion_tokens"（OpenAI 新系模型要求后者）
    pub max_tokens_field: &'static str,
    /// 流式是否带 stream_options.include_usage
    pub supports_usage_in_streaming: bool,
    /// 是否支持 temperature（gpt-5.6 家族拒收 400）
    pub supports_temperature: bool,
    /// 历史回放：assistant 消息是否保留 reasoning_content（DeepSeek/zai 契约）
    pub requires_reasoning_content_replay: bool,
    /// 是否支持 OpenAI 的 prompt_cache_key（会话级缓存路由键）。
    /// 仅官方 api.openai.com 认这个字段——中转/国产端点普遍 400，绝不外发。
    pub supports_prompt_cache_key: bool,
    /// 是否支持长保留（Anthropic ttl:"1h" / OpenAI prompt_cache_retention:"24h"）。
    /// 同样只对官方端点开；long 档在其余端点静默降级为短保留。
    pub supports_long_cache_retention: bool,
}

/// 提示缓存保留档（对齐 pi-ai 的 cacheRetention 词表）。
/// 命中率的两个旋钮之一：断点标记由本档决定是否发出，见 anthropic.rs / openai.rs。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CacheRetention {
    /// 不发任何缓存标记：断点/TTL/cache_key 全部不出现（渠道自动缓存仍可能生效）
    None,
    /// 短保留（默认）：Anthropic ephemeral（5 分钟）/ OpenAI 默认保留
    #[default]
    Short,
    /// 长保留：Anthropic ttl:"1h" / OpenAI prompt_cache_retention:"24h"；
    /// 端点不支持时静默降级为 Short（supports_long_cache_retention=false）
    Long,
}

impl CacheRetention {
    pub fn as_str(&self) -> &'static str {
        match self {
            CacheRetention::None => "none",
            CacheRetention::Short => "short",
            CacheRetention::Long => "long",
        }
    }
    /// 词表解析（config.toml / 设置面板写入值）；无法识别按 Short（默认档）。
    pub fn parse(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "none" | "off" | "关" => CacheRetention::None,
            "long" | "1h" | "24h" => CacheRetention::Long,
            _ => CacheRetention::Short,
        }
    }
}

/// 提取 base_url 的 host 部分（小写）。
fn host_of(base_url: &str) -> String {
    base_url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or("")
        .to_lowercase()
}

/// 是否官方 OpenAI 端点（api.openai.com；地址空缺时按预设 id 认）。
/// prompt_cache_key / prompt_cache_retention 只发给它——中转与国产端点普遍 400。
fn is_openai_official(provider_id: Option<&str>, base_url: Option<&str>) -> bool {
    match base_url.map(str::trim).filter(|b| !b.is_empty()) {
        Some(b) => host_of(b) == "api.openai.com",
        None => provider_id == Some("openai"),
    }
}

/// 缓存能力两件套：prompt_cache_key（OpenAI 官方）与长保留（OpenAI 官方 / Anthropic）。
fn cache_capabilities(provider_id: Option<&str>, base_url: Option<&str>, protocol: Protocol) -> (bool, bool) {
    let openai_official = is_openai_official(provider_id, base_url);
    let anthropic = protocol == Protocol::AnthropicMessages;
    (openai_official, openai_official || anthropic)
}

/// 按域名归类端点：native=官方网关，local=本地推理服务，default=中转/未识别。
fn endpoint_class(provider_id: Option<&str>, base_url: Option<&str>) -> &'static str {
    let base = base_url.unwrap_or("");
    if base.trim().is_empty() {
        // 无地址（纯函数调用/未填渠道）：按预设 id 归类
        return match provider_id {
            Some(
                "deepseek" | "zhipu" | "qwen" | "doubao" | "moonshot" | "openai" | "anthropic"
                | "gemini" | "xai" | "minimax" | "hunyuan" | "ernie" | "spark",
            ) => "native",
            Some("ollama" | "lmstudio" | "mnn" | "llamacpp") => "local",
            _ => "default",
        };
    }
    let host = host_of(base);
    // 剥端口再判本地（localhost:11434 / 127.0.0.1:8080；IPv6 [::1]:port 保留括号形态）
    let bare_host = if host.starts_with("[::1]") {
        host.clone()
    } else {
        host.split(':').next().unwrap_or("").to_string()
    };
    let is_local = bare_host == "localhost" || bare_host.starts_with("127.") || bare_host.starts_with("[::1]");
    // 官方域名优先（custom 渠道指到官方域名也算 native）
    const OFFICIAL_DOMAINS: [(&str, &str); 13] = [
        ("deepseek.com", "deepseek"),
        ("bigmodel.cn", "zhipu"),
        ("aliyuncs.com", "qwen"),
        ("volces.com", "doubao"),
        ("moonshot", "moonshot"),
        ("openai.com", "openai"),
        ("anthropic.com", "anthropic"),
        ("googleapis.com", "gemini"),
        ("x.ai", "xai"),
        ("minimax", "minimax"),
        ("tencent.com", "hunyuan"),
        ("baidubce.com", "ernie"),
        ("xf-yun.com", "spark"),
    ];
    for (domain, _) in OFFICIAL_DOMAINS {
        if host.contains(domain) {
            return "native";
        }
    }
    if is_local {
        // 本地端口：预设认得出用预设 id，认不出也算 local（custom=旧「自定义端点」预设 id，保留兼容）
        if matches!(provider_id, Some("ollama" | "lmstudio" | "mnn" | "llamacpp" | "openai_compat" | "custom"))
            || provider_id.is_none()
            || provider_id == Some("custom")
        {
            return "local";
        }
    }
    "default"
}

/// 推导端点兼容档（上游 resolveOpenAICompletionsCompatDefaults 的本产品版）。
/// `provider_id`/`base_url` 供域名判定；`model_id` 供模型族判定（gpt-5* 等）；
/// `format_override`：providers.json/渠道配置显式指定（优先级最高）。
#[allow(dead_code)] // 4 参便捷形态：anthropic/responses 模块的单测在用
pub fn resolve(
    provider_id: Option<&str>,
    base_url: Option<&str>,
    model_id: &str,
    format_override: Option<ThinkingFormat>,
) -> Compat {
    resolve_with_protocol(provider_id, base_url, model_id, format_override, None)
}

/// 同 resolve，但接受配置里的**显式协议锁定**（`protocol` 字段）：Some 时探测让位。
/// auto/None = 端点探测（provider_id、官方域、`/anthropic` 路径段启发）。
pub fn resolve_with_protocol(
    provider_id: Option<&str>,
    base_url: Option<&str>,
    model_id: &str,
    format_override: Option<ThinkingFormat>,
    protocol_override: Option<Protocol>,
) -> Compat {
    let class = endpoint_class(provider_id, base_url);
    // 传输协议：显式锁定 > 探测（provider_id=anthropic、官方域或 /anthropic 路径段 → Messages）
    let protocol = protocol_override.unwrap_or_else(|| {
        if super::anthropic::is_anthropic_endpoint(provider_id, base_url) {
            Protocol::AnthropicMessages
        } else {
            Protocol::OpenAiCompletions
        }
    });
    let openai_official = is_openai_official(provider_id, base_url);

    // 显式覆盖优先（providers.json 预设或渠道配置）
    if let Some(f) = format_override {
        let (cache_key, long_retention) = cache_capabilities(provider_id, base_url, protocol);
        return Compat {
            protocol,
            thinking_format: f,
            max_tokens_field: max_tokens_field_for(model_id),
            supports_usage_in_streaming: true,
            supports_temperature: !effort::is_gpt_5_6_model(model_id),
            requires_reasoning_content_replay: matches!(f, ThinkingFormat::Deepseek | ThinkingFormat::Zai),
            supports_prompt_cache_key: cache_key,
            supports_long_cache_retention: long_retention,
        };
    }

    // 本地推理服务：聊天模板形态思考开关；宽收 stream_options
    if class == "local" {
        return Compat {
            protocol,
            thinking_format: ThinkingFormat::QwenChatTemplate,
            max_tokens_field: "max_tokens",
            supports_usage_in_streaming: true,
            supports_temperature: true,
            requires_reasoning_content_replay: false,
            supports_prompt_cache_key: false,
            supports_long_cache_retention: false,
        };
    }

    // 端点归属判定：供应商预设 = 思考参数方言的选择器，
    // 选了谁就严格用谁的规范，URL 域名不再抢判断权——换预设的意义正是让非官方
    // URL 挂官方模型时用上官方标准思考参数（原 aliyuncs 域名会把 stepfun 预设
    // 判成 qwen 形态 → 档位消失，真机实测）。小米 URL 判定随本收口移除（无预设）。
    let is_deepseek = provider_id == Some("deepseek");
    let is_zai = matches!(provider_id, Some("zhipu") | Some("doubao"));
    let is_qwen = matches!(provider_id, Some("qwen") | Some("hunyuan"));

    let thinking_format = if is_deepseek {
        ThinkingFormat::Deepseek
    } else if is_zai {
        ThinkingFormat::Zai
    } else if is_qwen {
        ThinkingFormat::Qwen
    } else {
        ThinkingFormat::Openai
    };

    Compat {
        protocol,
        thinking_format,
        max_tokens_field: max_tokens_field_for(model_id),
        supports_usage_in_streaming: true,
        supports_temperature: !effort::is_gpt_5_6_model(model_id),
        requires_reasoning_content_replay: matches!(
            thinking_format,
            ThinkingFormat::Deepseek | ThinkingFormat::Zai
        ),
        supports_prompt_cache_key: openai_official,
        supports_long_cache_retention: openai_official || protocol == Protocol::AnthropicMessages,
    }
}

/// OpenAI 新系模型（gpt-5*/o 系）要求 max_completion_tokens；其余一律 max_tokens
/// （国产/中转端点普遍只认 max_tokens；上游 usesMaxTokens 名单是它们目录的镜像）。
/// 2026-10-08 修：中转渠道常见的 `openai/gpt-5.1` 这类带厂商前缀的 id 也要认出来
/// （与 replay::developer_role_for_model 的 rsplit('/') 同规）——不然给 gpt-5 系发
/// max_tokens 直接 400，且剥参重试不覆盖这个字段。
fn max_tokens_field_for(model_id: &str) -> &'static str {
    let id = model_id.trim().to_lowercase();
    let norm = id.rsplit('/').next().unwrap_or(&id);
    if norm.starts_with("gpt-5") || norm.starts_with("o1") || norm.starts_with("o3") || norm.starts_with("o4") {
        "max_completion_tokens"
    } else {
        "max_tokens"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_endpoints_get_native_formats() {
        let c = resolve(Some("zhipu"), Some("https://open.bigmodel.cn/api/paas/v4"), "glm-4.6", None);
        assert_eq!(c.thinking_format, ThinkingFormat::Zai);
        // 上游 shouldPreserveReasoningContentReplay：deepseek 与 zai 形态都保留回放
        assert!(c.requires_reasoning_content_replay);

        let c = resolve(Some("deepseek"), Some("https://api.deepseek.com"), "deepseek-chat", None);
        assert_eq!(c.thinking_format, ThinkingFormat::Deepseek);
        assert!(c.requires_reasoning_content_replay);

        let c = resolve(Some("qwen"), Some("https://dashscope.aliyuncs.com/compatible-mode/v1"), "qwen-plus", None);
        assert_eq!(c.thinking_format, ThinkingFormat::Qwen);

        let c = resolve(Some("openai"), Some("https://api.openai.com/v1"), "gpt-5", None);
        assert_eq!(c.thinking_format, ThinkingFormat::Openai);
        assert_eq!(c.max_tokens_field, "max_completion_tokens");
        assert!(c.supports_temperature, "gpt-5.6 家族才拒收；gpt-5 应支持");
        let c = resolve(Some("openai"), Some("https://api.openai.com/v1"), "gpt-5.6", None);
        assert!(!c.supports_temperature);

        let c = resolve(Some("openai"), Some("https://api.openai.com/v1"), "gpt-4o-mini", None);
        assert_eq!(c.max_tokens_field, "max_tokens");

        // 2026-10-08 修：中转渠道的 `openai/gpt-5.1` 这类带厂商前缀的 id——
        // 不剥前缀会给 gpt-5 系发 max_tokens（该族只要 max_completion_tokens，400），
        // 且 gpt-5.6 的 temperature 拒收判定也会失效
        let c = resolve(Some("custom"), Some("https://relay.example.com/v1"), "openai/gpt-5.1", None);
        assert_eq!(c.max_tokens_field, "max_completion_tokens", "带前缀也要认出 gpt-5 系");
        let c = resolve(Some("custom"), Some("https://relay.example.com/v1"), "openai/gpt-5.6", None);
        assert!(!c.supports_temperature, "带前缀的 5.6 仍拒收 temperature");
        let c = resolve(Some("custom"), Some("https://relay.example.com/v1"), "zhipu/glm-4.6", None);
        assert_eq!(c.max_tokens_field, "max_tokens", "非 gpt-5 系不受影响");
    }

    #[test]
    fn local_endpoints_use_chat_template() {
        let c = resolve(Some("ollama"), Some("http://localhost:11434/v1"), "qwen3:8b", None);
        assert_eq!(c.thinking_format, ThinkingFormat::QwenChatTemplate);
        // custom 渠道指到本地端口也按 local
        let c = resolve(Some("custom"), Some("http://127.0.0.1:8080/v1"), "qwen3", None);
        assert_eq!(c.thinking_format, ThinkingFormat::QwenChatTemplate);
    }

    #[test]
    fn custom_endpoint_falls_back_to_openai_format() {
        let c = resolve(Some("custom"), Some("https://relay.example.com/v1"), "mystery-model", None);
        assert_eq!(c.thinking_format, ThinkingFormat::Openai);
        assert_eq!(c.max_tokens_field, "max_tokens");
        // 模型 id 自证身份只在 native 端点生效；中转域名（default）仍按 openai 格式，
        // 想要 deepseek 契约必须显式覆盖
        let c = resolve(Some("custom"), Some("https://relay.example.com/v1"), "deepseek-reasoner", None);
        assert_eq!(c.thinking_format, ThinkingFormat::Openai, "中转域名探测不出 → openai 格式");
        // 显式覆盖生效（预设 thinking_format / 渠道配置）
        let c = resolve(Some("custom"), Some("https://relay.example.com/v1"), "mystery-model", Some(ThinkingFormat::Deepseek));
        assert_eq!(c.thinking_format, ThinkingFormat::Deepseek);
        assert!(c.requires_reasoning_content_replay);
    }

    #[test]
    fn doubao_and_provider_dictate_protocol() {
        // 豆包供应商直接识别为 Zai 形态
        let c = resolve(Some("doubao"), Some("https://ark.cn-beijing.volces.com/api/v3"), "doubao-seed-1.6", None);
        assert_eq!(c.thinking_format, ThinkingFormat::Zai);

        // 选了什么供应商就用什么规范：即便地址是第三方中转，只要选了 deepseek，就走 deepseek 契约
        let c = resolve(Some("deepseek"), Some("https://token.sensenova.cn/v1"), "deepseek-v4-flash", None);
        assert_eq!(c.thinking_format, ThinkingFormat::Deepseek);
        assert!(c.requires_reasoning_content_replay);

        let c = resolve(Some("qwen"), Some("https://relay.custom.io/v1"), "qwen-max", None);
        assert_eq!(c.thinking_format, ThinkingFormat::Qwen);
    }

    #[test]
    fn provider_preset_beats_url_domain() {
        // 供应商预设 = 思考参数方言的选择器，URL 域名不抢权重。
        // 实测场景：stepfun 预设 + dashscope URL（换预设不动 base_url）原被判成
        // qwen 形态 → 档位消失；现在预设说了算 → openai 形态（低/中/高）
        let c = resolve(Some("stepfun"), Some("https://dashscope.aliyuncs.com/compatible-mode/v1"), "deepseek-v4.1-flash-1017", None);
        assert_eq!(c.thinking_format, ThinkingFormat::Openai, "stepfun 预设不因 aliyuncs 域名判成 qwen");
        // 同理：通用预设挂厂商域名，也按预设走 openai 形态
        let c = resolve(Some("openai_compat"), Some("https://open.bigmodel.cn/api/paas/v4"), "glm-4.6", None);
        assert_eq!(c.thinking_format, ThinkingFormat::Openai, "openai_compat 预设不因 bigmodel 域名判成 zai");
        let c = resolve(Some("openai_compat"), Some("https://dashscope.aliyuncs.com/compatible-mode/v1"), "qwen-plus", None);
        assert_eq!(c.thinking_format, ThinkingFormat::Openai, "openai_compat 预设不因 aliyuncs 域名判成 qwen");
        let c = resolve(Some("openai_compat"), Some("https://api.deepseek.com"), "deepseek-chat", None);
        assert_eq!(c.thinking_format, ThinkingFormat::Openai, "openai_compat 预设不因 deepseek 域名判成 deepseek");
        // 反向不受影响：厂商预设挂任意 URL 仍按预设
        let c = resolve(Some("zhipu"), Some("https://relay.example.com/v1"), "glm-4.6", None);
        assert_eq!(c.thinking_format, ThinkingFormat::Zai);
    }

    #[test]
    fn anthropic_endpoints_get_messages_protocol() {
        // 预设 id 与官方域都判 Anthropic Messages 协议；OpenAI 形状字段与之正交
        let c = resolve(Some("anthropic"), Some("https://api.anthropic.com"), "claude-sonnet-4-5", None);
        assert_eq!(c.protocol, Protocol::AnthropicMessages);
        assert_eq!(c.max_tokens_field, "max_tokens");
        // 域名判定：自定义渠道指到 anthropic.com 也是 Messages 协议
        let c = resolve(Some("openai_compat"), Some("https://api.anthropic.com/v1"), "claude-3-5-haiku", None);
        assert_eq!(c.protocol, Protocol::AnthropicMessages);
        // 中转域名不带 anthropic 标识 → OpenAI 协议（要 Messages 就选 anthropic 预设）
        let c = resolve(Some("openai_compat"), Some("https://relay.example.com/v1"), "claude-sonnet-4-5", None);
        assert_eq!(c.protocol, Protocol::OpenAiCompletions);
        // 国产官方域不受影响
        let c = resolve(Some("zhipu"), Some("https://open.bigmodel.cn/api/paas/v4"), "glm-4.6", None);
        assert_eq!(c.protocol, Protocol::OpenAiCompletions);
    }

    #[test]
    fn cache_capabilities_only_on_official_endpoints() {
        // prompt_cache_key 与长保留都只对官方端点开——中转/国产端点发这些字段=400
        let c = resolve(Some("openai"), Some("https://api.openai.com/v1"), "gpt-5", None);
        assert!(c.supports_prompt_cache_key, "官方 OpenAI 认 prompt_cache_key");
        assert!(c.supports_long_cache_retention, "官方 OpenAI 认 24h 保留");
        // Anthropic 协议：断点/TTL 是协议原生，长保留也开
        let c = resolve(Some("anthropic"), Some("https://api.anthropic.com"), "claude-sonnet-4-5", None);
        assert!(!c.supports_prompt_cache_key, "Anthropic 协议没有 prompt_cache_key 这个字段");
        assert!(c.supports_long_cache_retention, "Anthropic 官方认 ttl 1h");
        // 中转与国产官方：两个都不开（命中率靠前缀稳定 + 渠道自动缓存）
        for (pid, url) in [("custom", "https://relay.example.com/v1"), ("zhipu", "https://open.bigmodel.cn/api/paas/v4"), ("deepseek", "https://api.deepseek.com")] {
            let c = resolve(Some(pid), Some(url), "m", None);
            assert!(!c.supports_prompt_cache_key, "{url} 不发 prompt_cache_key");
            assert!(!c.supports_long_cache_retention, "{url} 不发长保留");
        }
        // 本地推理服务：缓存是它自己的事，我们一个字段都不加
        let c = resolve(Some("ollama"), Some("http://127.0.0.1:11434/v1"), "qwen3:8b", None);
        assert!(!c.supports_prompt_cache_key && !c.supports_long_cache_retention);
        // 显式 thinking_format 覆盖不影响缓存两件套
        let c = resolve(Some("openai"), Some("https://api.openai.com/v1"), "gpt-5", Some(ThinkingFormat::None));
        assert!(c.supports_prompt_cache_key && c.supports_long_cache_retention);
    }

    #[test]
    fn cache_retention_word_list() {
        assert_eq!(CacheRetention::parse("short"), CacheRetention::Short);
        assert_eq!(CacheRetention::parse("SHORT"), CacheRetention::Short, "大小写不敏感");
        assert_eq!(CacheRetention::parse("long"), CacheRetention::Long);
        assert_eq!(CacheRetention::parse("none"), CacheRetention::None);
        assert_eq!(CacheRetention::parse("off"), CacheRetention::None);
        assert_eq!(CacheRetention::parse("关"), CacheRetention::None);
        assert_eq!(CacheRetention::parse(""), CacheRetention::Short, "空值按默认档");
        assert_eq!(CacheRetention::parse("胡说"), CacheRetention::Short, "无法识别按默认档");
        assert_eq!(CacheRetention::default(), CacheRetention::Short);
    }
}
