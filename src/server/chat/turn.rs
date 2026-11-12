use super::*;

/// //从配置项 + 供应商预设组装请求参数（采样/思考/端点探测线索/工具开关）。
/// 参数按 `model` 取：模型级覆盖 > 配置级（同一渠道不同模型各自调参）。
pub(crate) fn build_chat_params(
    c: &crate::config::settings::Config,
    preset: Option<&gateway::providers::ProviderPreset>,
    model: &str,
) -> gateway::ChatParams {
    let mut p = match preset {
        Some(p) => gateway::ChatParams::from_preset(p),
        None => gateway::ChatParams::default(),
    };
    let prm = c.params_for(model);
    let caps = prm.capabilities.clone().unwrap_or_else(|| c.capabilities.clone());
    p.max_tokens = prm.max_output_tokens;
    p.temperature = prm.temperature;
    p.top_p = prm.top_p;
    p.thinking = prm.thinking.clone();
    p.base_url = Some(c.effective_base_url());
    // A3：能力标记「工具调用」闭环
    p.tools_enabled = caps.tools;
    // 图片解析能力（2026-11-02 截图投递统一）：tool 结果里的截图按它决定「直接给
    // 模型看」还是「先走视觉角色转文字」——判模型能力，不判渠道/协议
    p.vision = caps.vision;
    // 提示缓存保留档（[cost] cache_retention，进程级单值）：决定 cache_control 断点 /
    // prompt_cache_key / 长保留 TTL 发不发。角色/降级/摘要渠道与主渠道同一档
    p.cache_retention = gateway::cache_retention();
    // 传输协议锁定（配置 protocol 字段）：锁定后端点探测让位——Anthropic 兼容端点
    // 不必依赖「/anthropic 路径段」这类命名习惯（2026-11-11）
    p.protocol = Some(c.effective_protocol());
    p.suffix_auto = c.auto_suffix;
    apply_thinking_style(&mut p, prm.thinking_style.as_deref());
    p
}

/// 思考参数形态覆盖——中转/自定义渠道端点探测认不出时显式指定（设置面板控件）。
/// B4 思考形态解析顺序（固化）：`Config.thinking_style` 显式覆盖 >
/// 端点探测（compat.rs：官方域名→native 供应商规范 / 本地→local / 其余→openai 兜底）>
/// providers.json 预设 `thinking_format` 兜底（预设值在 compat::resolve 内作为探测输入，
/// 当前仅豆包 zai/混元 qwen 带值且与探测结果一致）。
/// 词表（none/openai/deepseek/zai/qwen/qwen-chat-template）只增不改名；
/// 新形态入词表必须附真实渠道冒烟记录。none=不发任何思考参数。
pub(crate) fn apply_thinking_style(p: &mut gateway::ChatParams, style: Option<&str>) {
    use gateway::compat::ThinkingFormat as TF;
    match style {
        Some("none") => p.thinking_format = Some(TF::None),
        // 旧词表别名（设置残留）：thinking_object=GLM 式对象 → zai；
        // reasoning_effort=档位 → openai；enable_thinking（_budget）=布尔 → qwen
        Some("thinking_object") => p.thinking_format = Some(TF::Zai),
        Some("reasoning_effort") => p.thinking_format = Some(TF::Openai),
        Some("enable_thinking" | "enable_thinking_budget") => p.thinking_format = Some(TF::Qwen),
        Some(v) => p.thinking_format = TF::parse(v),
        None => {} // auto/未知：跟端点探测
    }
}

/// 角色值拆解：`配置名`/`配置id`，也认 `配置::模型`（2026-11-01 功能桥接按渠道
/// 的多模型清单展开）。返回 (配置引用, 模型覆盖)。配置段是否存在由调用方判——解析
/// 不到就退回把整串当配置名，老值行为不变。
pub(crate) fn split_role_value(name: &str) -> (&str, Option<&str>) {
    match name.rsplit_once("::") {
        Some((c, m)) if !c.trim().is_empty() && !m.trim().is_empty() => (c.trim(), Some(m.trim())),
        _ => (name, None),
    }
}

/// /把配置名（或配置 id）解析成一条可用渠道。解析失败（名字不存在/地址为空）返回 None。
/// B3 角色解析顺序（固化）：settings.roles.<role> > config.toml [roles] > 回落主渠道；
/// 角色值按「配置名或配置 id」解析（本函数），解析失败回落主渠道并 Notice。
///
/// 2026-11-01：还认「配置::模型」形式——功能桥接的下拉现在按每个渠道的多模型清单
/// （设置-模型配置里的逗号列表）展开，同一个渠道换不同模型很常见。配置段解析不到时
/// 退回把整串当配置名，老值（纯 id/名字）行为不变。
pub(crate) fn find_channel(s: &Settings, name: &str) -> Option<gateway::Channel> {
    let find_cfg = |n: &str| s.configs.iter().find(|c| c.name == n || c.id == n);
    let (cfg_ref, model_part) = split_role_value(name);
    let (c, model_override) = match find_cfg(cfg_ref) {
        Some(c) => (c, model_part.map(str::to_string)),
        // 「配置::模型」的配置段解析不到：整串可能就是配置名本身（含 ::），再试一次
        None => (find_cfg(name)?, None),
    };
    let base_url = c.effective_base_url();
    if base_url.is_empty() {
        return None;
    }
    let model = model_override.unwrap_or_else(|| c.effective_model());
    let preset = gateway::providers::find(&c.provider_id);
    let params = build_chat_params(c, preset, &model);
    // B2 key 回落链：DPAPI key > DEEPSEEK_API_KEY 环境变量。
    // 角色/降级渠道与主渠道同一把尺子——没单独存 key 的配置靠环境变量兜底
    // （补：此前只有主渠道走这条链，budget/summary/title 渠道会「未设置 key」）
    let key = resolve_key(s, c);
    Some(gateway::Channel::new(&base_url, &model, &key, params))
}

/// B2 key 回落链的角色/降级渠道版：本配置 DPAPI key 优先，缺失回落环境变量。
pub(crate) fn resolve_key(s: &Settings, c: &crate::config::settings::Config) -> String {
    s.config_key(c)
        .or_else(|| std::env::var("DEEPSEEK_API_KEY").ok())
        .unwrap_or_default()
}

/// 审批档位归一化（四档 + 旧档名映射）。返回（生效档, 降级提示）。
/// /api/chat 与无头会话共用——档位语义只有这一处解释。
pub(crate) fn normalize_mode(cfg: &crate::config::AgentConfig, requested: &str) -> (String, Option<String>) {
    let norm = |m: &str| -> String {
        match m.trim() {
            "suggest" => "read-only".into(),
            "auto-edit" | "full-auto" => "auto".into(),
            "local" | "full-access" => "full-access".into(),
            other @ ("read-only" | "sandbox" | "auto") => other.to_string(),
            _ => "auto".into(),
        }
    };
    let mut mode = if requested.trim().is_empty() {
        norm(&cfg.default_approval)
    } else {
        norm(requested)
    };
    // full-access 需 config 总开关，没开就降级为 auto 并提示（档位与实际权限保持一致）
    if mode == "full-access" && !cfg.allow_unsandboxed {
        mode = "auto".into();
        return (
            mode,
            Some(
                "完全访问（脱沙箱）未开启：在 config.toml [agent] allow_unsandboxed = true 后重启服务生效，本次已回退为自动档"
                    .to_string(),
            ),
        );
    }
    (mode, None)
}

/// 一轮对话的系统提示：prompts/system.md（每轮实时读盘）+ 记忆注入块
/// （自动记忆 profile.md → 全局约定 → 项目约定，靠后优先级高）+ 技能索引。
/// /api/chat 与无头会话共用同一套注入，行为对齐。
///
/// **缓存前缀纪律**：这里出去的每个字节都必须跨轮稳定——system 是 Anthropic
/// 三个缓存断点的第一个（system→tools→messages），system 一变，后面的 tools、
/// messages 稳定区全部作废（断点前缀miss 是整段的，不是部分的）。「项目现状」
/// brief 曾经放在这里末尾，结果新会话第 2 轮 system 比第 1 轮短一截 →
/// system+tools 全段重写（e2e 实测第 2 轮命中 21%，第 3 轮起才回 99%）——
/// 2026-10-20 起 brief 改并进首条 user 消息（见 `inject_brief_into_first_user`），
/// 它只属于 messages 区（区小、且本就带尾部断点）。
pub(crate) async fn turn_system_prompt(
    state: &AppState,
    primary: &gateway::Channel,
    ws: &std::path::Path,
) -> String {
    let prompt = read_system_prompt(state);
    let memory_md = state
        .memory
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .memory_context();
    // 工作区目录常驻系统提示：用户说「工作区/项目」都指这里，模型据此拼绝对路径，
    // 不再满磁盘 dir 猜位置（真机踩过：shell 工具描述写「项目根」，模型把用户说的
    // 工作区理解成了应用仓库，白搜 6 轮目录才找对文件）
    //
    // 2026-10-05 再补一句「改名 = 改建显示名」：用户说「把工作区/项目的名字改成 X」时，模型
    // 因为没有现成入口，去 dir 桌面、翻 .agent、读应用源码找接口，白花十几步（真机
    // 实测）。入口本来就在 settings 工具里，这里点名，别让模型再当侦探。
    // 额外文件夹只属于「当前全局工作区」；无头任务指定了别的项目目录时不带过去
    // （2026-11-10：工作区由调用方显式传入，任务提示不再跟着全局根走）。
    let extras = if ws == crate::tools::workspace_root().as_path() {
        crate::tools::workspace_extra_roots()
    } else {
        Vec::new()
    };
    let extras_hint = if extras.is_empty() {
        String::new()
    } else {
        let list = extras
            .iter()
            .map(|p| format!("- {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "\n本项目还有以下额外文件夹（多文件夹项目）：\n{list}\n\
             这些文件夹与主目录同权限：文件工具对它们**用上面的绝对路径**（相对路径只相对主目录），\
             用户提到的这些文件夹里的文件也按绝对路径读写。\n"
        )
    };
    let workspace_hint = format!(
        "\n\n【工作区】当前工作区目录：{}\nread_file/write_file/shell 的相对路径、用户提到的工作区文件，都以这个目录为根。\n\
         用户说「把工作区/项目的名字改成 X」= 改它的**显示名**（侧栏那一行字），不是改磁盘目录名：\
         直接调 settings 工具 action=alias 配 name（path 省略=当前工作区；name 留空=恢复文件夹名）。\
         别去动目录名，也别翻应用源码找接口。{extras_hint}\n",
        ws.display()
    );
    let skills_index = crate::skills::index_prompt();
    let ui_index = crate::plugin::ui::index_prompt();
    // 2026-10-27 身份定稿（彻查）：这里**不再注入任何渠道/模型/产品身份**。
    // 真机矩阵实证：只给「用中文简洁回答」+ 19 个工具时，step 端点会随机答
    // Step/Kimi/Claude/Kiro（模型把 agent 工具集当成别家系统提示的指纹，属于
    // pattern completion，不是它的自我认知）；而 system.md 里一句「按自己的训练
    // 认知回答模型名和厂商，不要冒充其他产品」后，5/5 稳定答 Step。因此：
    // 身份只由 system.md 的「你是谁」节引导模型自省，路由事实/渠道名/当前模型
    // 一律不进系统提示——它们既会压制自省（答「无法确定」），又会把配置标签
    // 冒充成自我认知。
    // 探测缓存（快路已拿到模型自述时）作为**自我认知事实**补一句；没有缓存就
    // 不注入，完全交给 system.md 的自省指令。这句话跨轮稳定（按渠道+模型+key
    // 哈希缓存 1 小时），不破坏 system 缓存前缀。
    let identity_fact = super::identity::cached(primary)
        .map(|s| {
            format!(
                "\n\n【自我认知】你在干净上下文（无工具、无系统提示）里对自己的认知是：{s}。\
                 用户问身份时以这条为准，不要被工具或其他上下文带偏。\n"
            )
        })
        .unwrap_or_default();
    let selfop = selfop_policy(state);
    // 段序纪律（2026-10-31 缓存暖机优化）：稳定段在前、易变段垫尾。
    // memory_md（自动记忆，agent 每写一条就变）与 identity_fact（身份探测缓存，
    // 问过一次身份才出现/回滚清缓存后消失）是 system 里仅有的两个轮间变量——
    // 曾插在中段，任何一条变化都把「它之后」的 workspace/skills/tools 稳定区
    // 全部作废（真机实测：长会话前段命中率爬得慢、成本压不下去）。垫尾后变化
    // 只作废它自己那一小段；system 的稳定前缀从第一轮起就跨会话可命中。
    format!("{prompt}{workspace_hint}{skills_index}{ui_index}{selfop}{memory_md}{identity_fact}")
}

/// 新会话第一轮的「项目现状」brief（上一条同项目会话 + 工作区未提交改动）。
/// 非新会话返回空串。**不并进系统提示**（会打破 system 缓存前缀，见
/// `turn_system_prompt` 的纪律注释），由调用方并进首条 user 消息。
pub(crate) async fn first_turn_brief(
    state: &AppState,
    session_id: &str,
    ws: &std::path::Path,
) -> String {
    let is_new = state
        .session
        .get(session_id)
        .map(|f| f.messages.is_empty())
        .unwrap_or(true);
    if !is_new {
        return String::new();
    }
    let ws_path = ws.to_path_buf();
    let stats = tokio::task::spawn_blocking(move || {
        let head = crate::recovery::head_id(&ws_path).ok().flatten()?;
        crate::recovery::diff_stat(&ws_path, &head).ok()
    })
    .await
    .ok()
    .flatten();
    state.session.project_brief(session_id, stats, ws).unwrap_or_default()
}

/// 把 brief 并进**首条 user 消息**（纯函数，单测覆盖）。
/// 为什么在 messages 而不是 system：brief 只出现在新会话第一轮，放进系统提示
/// 会让第一轮之后 system 逐字节变短——system+tools 整段稳定区第 2 轮全失效
/// （e2e 实测：第 2 轮命中 21% vs 第 3 轮起 99%；真实 Anthropic 语义下更狠，
/// 三个断点全部 miss、整单重写）。并进首条 user 消息后 system 从第一轮起就
/// 逐字节稳定，brief 随 messages 区走（区小，且本就带尾部断点）。
pub(crate) fn inject_brief_into_first_user(messages: &mut [Value], brief: &str) {
    if brief.trim().is_empty() {
        return;
    }
    let Some(first) = messages
        .iter_mut()
        .find(|m| m.get("role").and_then(Value::as_str) == Some("user"))
    else {
        return;
    };
    match &mut first["content"] {
        Value::String(s) => *s = format!("{s}\n\n{brief}"),
        // 数组形态（多模态）：追加一个 text part（不动图片/音频等原生 part）；
        // 它成为末块也无妨——text 块可缓存，断点照打
        Value::Array(parts) => parts.push(json!({ "type": "text", "text": brief })),
        _ => {}
    }
}

/// 落盘前把 brief 从首条 user 消息里剥掉（`inject_brief_into_first_user` 的逆操作）。
///
/// brief 只该出现在**发给模型的请求**里：模型要靠它接手项目现状，但用户气泡里
/// 显示的必须是用户的原话——brief 拼进消息后随 Snapshot 落进会话文件，用户过会
/// 重开对话就看见自己的提问后面拖着一截系统注入（2026-10-26 真机实测「提示词
/// 消息自己变了」）。运行中的轮次上下文不动（保缓存前缀），只在持久化边界剥离。
pub(crate) fn strip_brief_from_first_user(messages: &mut [Value]) {
    const MARKER: &str = "【项目现状 · 新对话接手】";
    let Some(first) = messages
        .iter_mut()
        .find(|m| m.get("role").and_then(Value::as_str) == Some("user"))
    else {
        return;
    };
    match first.get_mut("content") {
        Some(Value::String(s)) => {
            if let Some(pos) = s.find(MARKER) {
                *s = s[..pos].trim_end().to_string();
            }
        }
        Some(Value::Array(parts)) => {
            for part in parts.iter_mut() {
                if part.get("type").and_then(Value::as_str) != Some("text") {
                    continue;
                }
                if let Some(txt) = part.get("text").and_then(Value::as_str)
                    && let Some(pos) = txt.find(MARKER) {
                        part["text"] = Value::String(txt[..pos].trim_end().to_string());
                    }
            }
            // 纯 brief part（数组形态注入的那块）截断后成空串：整个撤掉
            parts.retain(|p| {
                !(p.get("type").and_then(Value::as_str) == Some("text")
                    && p.get("text").and_then(Value::as_str).is_some_and(str::is_empty))
            });
        }
        _ => {}
    }
}

/// 自我管理与探索纪律（与 core/rules 的红线/闸门同一套语义）：
/// 真机实测过翻车——用户关着「中枢」插件让改审批档位，agent 不知道这属于自身修改、
/// 更不知道审批档位根本没有它的通道，于是 dir/findstr 翻十六步撞上限被砍。这块就堵
/// 这个：先认领请求类型，再查闸/认红线，没有合法通道就直接说，不许自己造路绕。
fn selfop_policy(state: &AppState) -> String {
    let self_on = state.settings.read().map(|s| s.features.self_mod).unwrap_or(false);
    format!(
        "\n\n## 自我管理与探索纪律\n\
        - 改自身（设置/渠道/插件技能启停/定时任务/会话/侧栏面板/边栏/主题/壁纸/终端/设置弹窗）\
         由「中枢」插件管着：它现在{state}。没开时只回一句引导（去 设置→插件 打开「中枢」），\
         不要翻文件系统找替代路径、不要试绕；开了才动手，红线照旧先问。\n\
        - 红线（agent 没有任何通道可改，说了也做不到）：审批档位（read-only只读/auto自动/\
         full-access完全访问三档，用户手动在输入框上方的档位菜单里切；config.toml 的 \
         default_approval 是它的持久形态，改它必问）、API key（key 永不经模型之手）。\
         其余配置（渠道指向/MCP/config.toml/价格表/插件信任）2026-10-04 起全部放开：\
         走 settings/plugin 工具正常改，按审批档位走，改坏了可改回。\n\
        - 探索纪律：翻目录/search 找东西，同一目标两三次找不到就停下来汇报已知情况，\
         禁止反复 dir/findstr 扫全盘；本环境没有的能力直接说没有，列用户可走的路。\n\
        - 步数：本轮没有步数上限，也没有「相同调用自动终止」这类变相限制（都是变相步数限制，一律不要）——失控只有取消键和花费硬闸两道缰绳管着；\
         长任务按「做实事」分批推进，别把一步拆成十步、别为验证已确认的事反复调工具。\n",
        state = if self_on {
            "【开着】——按当前审批档位正常放行"
        } else {
            "【关着】——任何改自身动作先一句引导用户去开，不许绕路"
        },
    )
}

/// 一轮对话的渠道组装结果（主渠道 + 角色渠道 + 降级链 + 预算降级）。
pub(crate) struct TurnChannels {
    pub(crate) primary: gateway::Channel,
    pub(crate) summary: Option<gateway::Channel>,
    pub(crate) fallbacks: Vec<gateway::Channel>,
    /// 预算降级渠道（[roles] budget；超 token 预算后剩余步骤改走它）
    pub(crate) downgrade: Option<gateway::Channel>,
    /// 视觉转写渠道（roles.vision；没配或专家没勾「图片解析」= None）。
    /// 主模型没勾图片解析时，computer 截图先交给它转文字（2026-11-02 截图投递统一）。
    pub(crate) vision: Option<gateway::Channel>,
    /// 角色渠道解析失败等提示（转发给调用方展示，不静默）
    pub(crate) notices: Vec<String>,
}

/// 组装一轮对话的全部渠道。请求级覆盖优先（页面当前选择即时生效，不等保存）。
/// /api/chat 与无头会话共用——角色解析顺序（B3）与 key 回落链（B2）只有这一处实现。
pub(crate) fn assemble_turn_channels(
    state: &AppState,
    s: &Settings,
    active_cfg: &crate::config::settings::Config,
    req_model: &str,
    req_base_url: &str,
    req_api_key: &str,
) -> TurnChannels {
    let base_url = if !req_base_url.is_empty() {
        req_base_url.to_string()
    } else {
        active_cfg.effective_base_url()
    };
    let model = if !req_model.is_empty() {
        req_model.to_string()
    } else {
        active_cfg.effective_model()
    };
    // B2 key 回落链（固化）：请求显式 key > 激活配置 DPAPI key > DEEPSEEK_API_KEY 环境变量
    let api_key = if !req_api_key.is_empty() {
        req_api_key.to_string()
    } else {
        s.config_key(active_cfg)
            .or_else(|| std::env::var("DEEPSEEK_API_KEY").ok())
            .unwrap_or_default()
    };
    let mut params = build_chat_params(active_cfg, gateway::providers::find(&active_cfg.provider_id), &model);
    params.base_url = Some(base_url.clone());
    let primary = gateway::Channel::new(&base_url, &model, &api_key, params);
    // / 角色映射：优先 settings.roles，未设回落 config.toml [roles]；
    // features.roles=false（插件页「功能桥接」关）= 整条路由链跳过，全部走主渠道/默认退化
    let roles = if s.features.roles { s.roles.as_ref() } else { None };
    let cfg_roles = if s.features.roles { Some(&state.roles_cfg) } else { None };
    let summary_name = roles
        .and_then(|r| r.summary.as_deref())
        .or_else(|| cfg_roles.and_then(|c| c.summary.as_deref()));
    let summary = summary_name.and_then(|n| find_channel(s, n));
    let mut notices = Vec::new();
    if let Some(n) = summary_name
        && summary.is_none()
    {
        notices.push(format!(
            "角色「会话摘要」配置的渠道「{n}」解析失败（配置名不存在或地址为空），已回落主渠道代跑"
        ));
    }
    // / 降级链：优先 settings.roles.fallbacks，未设回落 [gateway].fallback_configs
    let fallback_names: Vec<String> = roles
        .map(|r| r.fallbacks.clone())
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| state.gateway_cfg.fallback_configs.clone());
    let mut failed_fallbacks: Vec<String> = Vec::new();
    let fallbacks: Vec<gateway::Channel> = fallback_names
        .iter()
        .filter_map(|n| match find_channel(s, n) {
            Some(c) => Some(c),
            None => {
                failed_fallbacks.push(n.clone());
                None
            }
        })
        .collect();
    // 写错的降级候选不再静默消失：降级链实际比配置的短，出故障时才发现就晚了
    // （2026-11-10 审查）。
    if !failed_fallbacks.is_empty() {
        notices.push(format!(
            "降级链里的渠道「{}」解析失败（配置名不存在或地址为空），已跳过（剩余 {} 个）",
            failed_fallbacks.join("、"),
            fallbacks.len()
        ));
    }
    // 预算降级渠道
    let budget_name = roles
        .and_then(|r| r.budget.as_deref())
        .or_else(|| cfg_roles.and_then(|c| c.budget.as_deref()));
    let downgrade = budget_name.and_then(|n| find_channel(s, n));
    if let Some(n) = budget_name
        && downgrade.is_none()
    {
        notices.push(format!(
            "角色「预算降级」配置的渠道「{n}」解析失败（配置名不存在或地址为空），超预算后维持主渠道"
        ));
    }
    // 视觉转写渠道（截图投递用，2026-11-02）：和附件转接同一道能力闸——专家配置
    // 没勾「图片解析」就当没配（不按模型名猜能力），免得把图发给看不了图的端点。
    // 只认 settings.roles.vision（config.toml 的 [roles] 没有多模态角色，附件转接同）。
    // 主模型自己就能看图时不解析——省得白推一条「专家没勾能力」的提示。
    let vision_name = if active_cfg.caps_for(&model).vision {
        None
    } else {
        roles.and_then(|r| r.vision.as_deref())
    };
    let vision = vision_name.and_then(|n| {
        let cfg_ref = split_role_value(n).0;
        match s.configs.iter().find(|c| c.id == cfg_ref || c.name == cfg_ref) {
            // 配置被删/改坏：不再静默压平成文字（2026-11-10 审查）
            None => {
                notices.push(format!(
                    "角色「视觉理解」配置「{n}」解析失败（配置名不存在或地址为空），截图改按落盘指路"
                ));
                None
            }
            Some(expert) if !expert.capabilities.vision => {
                notices.push(format!(
                    "角色「视觉理解」配置「{n}」没勾「图片解析」，截图不会转成文字（改按落盘指路）"
                ));
                None
            }
            Some(_) => find_channel(s, n),
        }
    });
    // 思考参数自适应降级登记：该端点此前 400 拒收过思考字段（剥参重试成功）。
    // 同进程内只在首次提醒一次（2026-11-05：原来每轮都发，多个对话里反复出现同一条通知）
    if state.gateway.take_thinking_degraded_notice(&base_url, &model) {
        notices.push("该端点此前拒绝了思考参数，已自动关闭思考下发（对话正常进行）".to_string());
    }
    // 标题不再在这里生成：会话落盘时用首条用户消息前 10 字（2026-11-06）。
    TurnChannels { primary, summary, fallbacks, downgrade, vision, notices }
}

/// 检查模型是否具备指定模态的输入能力：只认用户手动勾选的能力标记，
/// 不再按模型名智能推导（聚合渠道上名字与真实能力对不上）。
/// 此前 accept_modality 的按名推导兜底已删：专家配置解析失败时按「未勾选」处理，
/// A5：媒体附件落在 <workspace>/.attachments/，若工作区是 git 仓库且
/// .gitignore 未含该目录，自动追加一行 `.attachments/`（只追加，绝不重写用户文件），
/// 防止用户 `git add .` 把二进制附件提交进历史。
pub(crate) fn ensure_attachments_ignored(ws: &std::path::Path) {
    if !ws.join(".git").exists() {
        return;
    }
    let gi = ws.join(".gitignore");
    let existing = std::fs::read_to_string(&gi).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == ".attachments/") {
        return;
    }
    let mut add = String::new();
    // 用户文件可能不带结尾换行：直接追加会粘成一行，先补一个换行
    if !existing.is_empty() && !existing.ends_with('\n') {
        add.push('\n');
    }
    add.push_str(".attachments/\n");
    let ok = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&gi)
        .and_then(|mut f| std::io::Write::write_all(&mut f, add.as_bytes()))
        .is_ok();
    if ok {
        // try_event：这条路径会在单测里被直接调用，logger 未初始化时不该 panic
        observability::try_event(
            EventType::State,
            json!({ "kind": "attachments.gitignored", "path": gi.display().to_string() }),
        );
    }
}

#[cfg(test)]
mod brief_injection_tests {
    use super::*;

    #[test]
    fn empty_brief_is_noop() {
        let mut msgs = json!([{"role":"user","content":"问"}]).as_array().cloned().unwrap();
        inject_brief_into_first_user(&mut msgs, "");
        inject_brief_into_first_user(&mut msgs, "   \n  ");
        assert_eq!(msgs[0]["content"], json!("问"), "空 brief 不许碰消息");
    }

    #[test]
    fn string_content_appends_brief() {
        let mut msgs = json!([{"role":"user","content":"问"}]).as_array().cloned().unwrap();
        inject_brief_into_first_user(&mut msgs, "【项目现状】git 干净");
        assert_eq!(msgs[0]["content"], json!("问\n\n【项目现状】git 干净"));
    }

    #[test]
    fn array_content_appends_text_part() {
        // 多模态数组形态：追加 text part，图片/音频等原生 part 一个不动
        let mut msgs = json!([{"role":"user","content":[
            {"type":"text","text":"看图"},
            {"type":"image_url","image_url":{"url":"https://example.com/cat.jpg"}}
        ]}]).as_array().cloned().unwrap();
        inject_brief_into_first_user(&mut msgs, "【项目现状】有改动");
        let parts = msgs[0]["content"].as_array().unwrap();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[1]["type"], json!("image_url"), "原生 part 不许被动");
        assert_eq!(parts[2], json!({"type":"text","text":"【项目现状】有改动"}));
    }

    #[test]
    fn only_first_user_message_touched() {
        // 只并进**首条** user：后面的历史消息一个字不许改
        let mut msgs = json!([
            {"role":"system","content":"系统"},
            {"role":"user","content":"第一问"},
            {"role":"assistant","content":"第一答"},
            {"role":"user","content":"第二问"}
        ]).as_array().cloned().unwrap();
        inject_brief_into_first_user(&mut msgs, "B");
        assert_eq!(msgs[1]["content"], json!("第一问\n\nB"));
        assert_eq!(msgs[2]["content"], json!("第一答"));
        assert_eq!(msgs[3]["content"], json!("第二问"), "后续 user 消息不沾");
    }

    #[test]
    fn no_user_message_is_noop() {
        // 全是 assistant/system（异常历史）：不崩、不动
        let mut msgs = json!([{"role":"assistant","content":"答"}]).as_array().cloned().unwrap();
        inject_brief_into_first_user(&mut msgs, "B");
        assert_eq!(msgs[0]["content"], json!("答"));
    }

    #[test]
    fn null_content_message_is_skipped_gracefully() {
        // content 是 null（异常形态）：不该 panic，落到 _ 分支原样不动
        let mut msgs = json!([{"role":"user","content":null}]).as_array().cloned().unwrap();
        inject_brief_into_first_user(&mut msgs, "B");
        assert_eq!(msgs[0]["content"], json!(null));
    }

    const BRIEF: &str = "\n\n【项目现状 · 新对话接手】\n项目目录：C:\\x（x）\n- 上一条会话：「旧」（10-03）\n";

    #[test]
    fn strip_restores_original_string() {
        // 注入再剥离 = 原文一个字不差（落盘内容对用户可见，必须可逆）
        let mut msgs = json!([{"role":"user","content":"你是什么模型"}]).as_array().cloned().unwrap();
        inject_brief_into_first_user(&mut msgs, BRIEF);
        strip_brief_from_first_user(&mut msgs);
        assert_eq!(msgs[0]["content"], json!("你是什么模型"));
    }

    #[test]
    fn strip_drops_pure_brief_part_in_array() {
        // 数组形态注入的纯 brief part：剥离后整块撤掉，原生 part 不动
        let mut msgs = json!([{"role":"user","content":[
            {"type":"text","text":"看图"},
            {"type":"image_url","image_url":{"url":"https://example.com/cat.jpg"}}
        ]}]).as_array().cloned().unwrap();
        inject_brief_into_first_user(&mut msgs, BRIEF);
        strip_brief_from_first_user(&mut msgs);
        let parts = msgs[0]["content"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["text"], json!("看图"));
        assert_eq!(parts[1]["type"], json!("image_url"));
    }

    #[test]
    fn strip_trims_brief_merged_into_text_part() {
        // 附件流水线把 brief 并进 text part 字符串的形态：截回用户原话
        let merged = format!("你是什么模型{}", BRIEF);
        let mut msgs = json!([{"role":"user","content":[
            {"type":"text","text":merged},
            {"type":"image_url","image_url":{"url":"https://example.com/cat.jpg"}}
        ]}]).as_array().cloned().unwrap();
        strip_brief_from_first_user(&mut msgs);
        let parts = msgs[0]["content"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["text"], json!("你是什么模型"));
    }

    #[test]
    fn strip_only_touches_first_user_message() {
        let mut msgs = json!([
            {"role":"user","content":format!("第一问{}", BRIEF)},
            {"role":"assistant","content":"答"},
            {"role":"user","content":format!("第二问{}", BRIEF)}
        ]).as_array().cloned().unwrap();
        strip_brief_from_first_user(&mut msgs);
        assert_eq!(msgs[0]["content"], json!("第一问"));
        assert_eq!(msgs[2]["content"], json!(format!("第二问{}", BRIEF)), "后续 user 消息不沾");
    }

    #[test]
    fn strip_without_marker_is_noop() {
        let mut msgs = json!([{"role":"user","content":"普通提问"}]).as_array().cloned().unwrap();
        strip_brief_from_first_user(&mut msgs);
        assert_eq!(msgs[0]["content"], json!("普通提问"));
    }
}


#[cfg(test)]
mod role_value_tests {
    //! 2026-11-01：功能桥接的角色值可以带模型（`配置::模型`），老值行为不变。
    use super::*;
    use crate::config::settings::{Config, ModelCapabilities, Settings};

    fn cfg(id: &str, name: &str, model: &str, models: &[&str]) -> Config {
        Config {
            id: id.to_string(),
            name: name.to_string(),
            provider_id: "deepseek".to_string(),
            base_url: "https://example.invalid/v1".to_string(),
            model: model.to_string(),
            models: models.iter().map(|s| s.to_string()).collect(),
            key_cipher: None,
            context_length: None,
            max_output_tokens: None,
            temperature: None,
            top_p: None,
            thinking: None,
            thinking_style: None,
            protocol: None,
            auto_suffix: true,
            capabilities: ModelCapabilities::default(),
            capabilities_user_set: false,
            model_params: Default::default(),
        }
    }

    #[test]
    fn role_value_can_pin_a_model() {
        let mut s = Settings::default();
        s.configs.push(cfg("c1", "主力", "m-a", &["m-a", "m-b"]));

        // 拆解：老值原样，带模型的剥出模型段；空模型段不算覆盖
        assert_eq!(split_role_value("c1"), ("c1", None));
        assert_eq!(split_role_value("主力"), ("主力", None));
        assert_eq!(split_role_value("c1::m-b"), ("c1", Some("m-b")));
        assert_eq!(split_role_value("主力::m-b"), ("主力", Some("m-b")));
        assert_eq!(split_role_value("c1::"), ("c1::", None), "空模型段不覆盖");

        // 解析成渠道：带模型时覆盖配置里当前选的那个
        assert_eq!(find_channel(&s, "c1").unwrap().model, "m-a");
        assert_eq!(find_channel(&s, "c1::m-b").unwrap().model, "m-b");
        assert_eq!(find_channel(&s, "主力::m-b").unwrap().model, "m-b");
        // 配置不存在 → None（不 panic、不误判）
        assert!(find_channel(&s, "不存在::m-b").is_none());
    }
}
