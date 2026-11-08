//! 设置 schema 元数据表（2026-10-03 自改机制重构轮阶段3）：把「能改什么设置」从散在
//! 4~5 处的编译期硬编码收成**一张表**——每行声明 path/label/type/default/risk/desc/setter。
//!
//! 由来（docs/自改方案-免逐项硬编码）：加一个设置项原先要动 SettingsPatch 字段 +
//! apply_patch 分支 + risk_settings 分级 + 红线表 + 工具描述键清单 5 处，漏一处就
//! 「能改但不生效 / 生效但不弹卡 / 弹卡但分类错」。现在只加一行数据。
//!
//! 落地形态（本轮刻意不引入宏/JSON 旁挂/属性注解）：
//! - 顶层键 18 个 + `features.*` / `roles.*` 点号子路径若干，全表 = 模型可改路径全集；
//! - `risk`：`red_line`（渠道指向，Dangerful，见 core::rules 的弹卡口径）/
//!   `normal`（常规改动，auto 档静默）——分级/红线集合/工具描述键清单全从本表派生；
//! - `setter`：把「值写进 Settings」的现有语义**原样搬进来**（不重造），patch 应用
//!   顺序 = 表序（active_config_id 排最前，保证「切完后改参数」落到切换后的配置）。
//!
//! **安全边界（2026-10-04 收紧到两条）**：api_key/clear_key（key 永不经模型之手）、
//! workspace（自己挪自己的笼子）刻意**不在表内**——表外路径一律报错并列合法路径，
//! 不存在「静默不生效」的灰区。审批档位也不在本表（config.toml 的 default_approval
//! 是它的持久形态，set_config 改它一律弹卡）。其余全部可改：渠道指向（原先的
//! red_line 四件套）降为 normal，MCP 配置（mcp_servers）入表；config.toml 其余键走
//! settings 工具 action=set_config；第三方插件安装走 plugin 工具（install）。

use serde_json::{Value, json};

use crate::config::settings::Settings;

/// 一条可改路径的元数据。serde 无关，纯 Rust 常量（编译期定死，无外部注入面）。
#[derive(Debug, Clone, Copy)]
pub struct SchemaEntry {
    /// 设置路径（点号 = 子字段，如 `features.self_mod` / `roles.summary`）
    pub path: &'static str,
    /// 人类可读名（喂模型与界面）
    pub label: &'static str,
    /// 值类型：bool / string / integer / float / string_array / config_name / object
    pub ty: &'static str,
    /// 可选枚举（值词表；空 = 不约束）
    pub enum_values: &'static [&'static str],
    /// 默认值（None = 落「清除/不设」语义，见各 setter）
    pub default: Option<&'static str>,
    /// 风险档：`red_line` / `normal`
    pub risk: &'static str,
    /// 一句话说明（怎么填、有什么联动）
    pub desc: &'static str,
}

/// 全表。顺序 = apply_patch 的应用序（与既有逐字段语义一致：
/// 激活配置切换必须排在所有渠道字段之前）。
pub const SCHEMA: &[SchemaEntry] = &[
    // —— 渠道指向（2026-10-04 放开：除审批档位/API key 外全部可改；
    // 档位即用户意愿，改坏了可改回）——
    SchemaEntry {
        path: "active_config_id",
        label: "切换激活配置",
        ty: "config_name",
        enum_values: &[],
        default: None,
        risk: "normal",
        desc: "写 settings.json 里某套配置的 id（先 action=get 看 configs 清单）。后面的渠道字段改的是切换后的激活配置",
    },
    SchemaEntry {
        path: "provider_id",
        label: "供应商 id",
        ty: "string",
        enum_values: &[],
        default: None,
        risk: "normal",
        desc: "渠道提供商标识（presets.json 里的 id）",
    },
    SchemaEntry {
        path: "base_url",
        label: "渠道地址",
        ty: "string",
        enum_values: &[],
        default: None,
        risk: "normal",
        desc: "API 端点 URL。变更会清空已存模型与模型列表（URL 变了旧的作废），需重新拉取",
    },
    SchemaEntry {
        path: "model",
        label: "模型名",
        ty: "string",
        enum_values: &[],
        default: None,
        risk: "normal",
        desc: "当前激活配置用的模型",
    },
    // —— 普通：渠道周边 ——
    SchemaEntry {
        path: "name",
        label: "配置显示名",
        ty: "string",
        enum_values: &[],
        default: None,
        risk: "normal",
        desc: "当前激活配置的显示名",
    },
    SchemaEntry {
        path: "models",
        label: "模型列表",
        ty: "string_array",
        enum_values: &[],
        default: None,
        risk: "normal",
        desc: "弹窗里勾选保存的模型下拉清单（整表替换）",
    },
    SchemaEntry {
        path: "context_length",
        label: "上下文长度",
        ty: "integer",
        enum_values: &[],
        default: Some("0=清除，走渠道默认"),
        risk: "normal",
        desc: "tokens；0=清除恢复默认",
    },
    SchemaEntry {
        path: "max_output_tokens",
        label: "最大输出长度",
        ty: "integer",
        enum_values: &[],
        default: Some("0=清除"),
        risk: "normal",
        desc: "tokens；0=清除走渠道默认",
    },
    SchemaEntry {
        path: "temperature",
        label: "采样温度",
        ty: "float",
        enum_values: &[],
        default: Some("负数=清除"),
        risk: "normal",
        desc: "0.0 是合法值（确定性输出）；负数=清除走渠道默认",
    },
    SchemaEntry {
        path: "top_p",
        label: "Top-p",
        ty: "float",
        enum_values: &[],
        default: Some("负数=清除"),
        risk: "normal",
        desc: "采样；0.0 合法，负数=清除走渠道默认",
    },
    SchemaEntry {
        path: "thinking",
        label: "思考档位",
        ty: "string",
        enum_values: &["on", "off", "minimal", "low", "medium", "high", "xhigh", "max"],
        default: Some("空串=清除"),
        risk: "normal",
        desc: "思考开关与档位；空串=清除走渠道默认",
    },
    SchemaEntry {
        path: "thinking_style",
        label: "思考参数形态",
        ty: "string",
        enum_values: &[
            "none",
            "anthropic",
            "auto",
            "thinking_object",
            "openai",
            "deepseek",
            "zai",
            "qwen",
            "qwen-chat-template",
        ],
        default: Some("auto/空串=端点探测"),
        risk: "normal",
        desc: "各协议 thinking 字段形态覆盖；旧词表别名自动映射",
    },
    SchemaEntry {
        path: "capabilities",
        label: "模型能力标记",
        ty: "object",
        enum_values: &[],
        default: None,
        risk: "normal",
        desc: "工具调用/图片/视频/音频解析的用户勾选态（能力标记归用户手动勾选，切换模型不改写）",
    },
    SchemaEntry {
        path: "roles",
        label: "角色路由（整表）",
        ty: "object",
        enum_values: &[],
        default: None,
        risk: "normal",
        desc: "摘要/标题/多模态/降级链的专用配置名映射。逐字段改优先用点号路径 roles.<字段>",
    },
    SchemaEntry {
        path: "features",
        label: "功能开关（整表）",
        ty: "object",
        enum_values: &[],
        default: None,
        risk: "normal",
        desc: "内置功能开关整表替换。单个开关优先用点号路径 features.<键>",
    },
    SchemaEntry {
        path: "mcp_servers",
        label: "MCP server 清单",
        ty: "object",
        enum_values: &[],
        default: None,
        risk: "normal",
        desc: "整表替换（先 get 拿现状；按 name 合并：已存在的服务不带的字段保留旧值）。第三方服务的密钥也在 env 里——2026-10-04 放开，改动后用 action=mcp op=refresh 重连",
    },
    SchemaEntry {
        path: "disabled_plugins",
        label: "停用插件名单",
        ty: "string_array",
        enum_values: &[],
        default: None,
        risk: "normal",
        desc: "插件 id 整表替换（id 为准不写中文标题，对不上会报错并列可用 id）；关到带 feature 联动的插件会同步写 features 对应开关",
    },
    SchemaEntry {
        path: "disabled_skills",
        label: "停用技能名单",
        ty: "string_array",
        enum_values: &[],
        default: None,
        risk: "normal",
        desc: "技能名整表替换（对不上报错并列可用名）",
    },
    SchemaEntry {
        path: "schedules",
        label: "定时任务清单",
        ty: "object",
        enum_values: &[],
        default: None,
        risk: "normal",
        desc: "整表替换（先 get 拿现状）。新增任务 id 留空自动生成；带 id 改已有任务时创建时间与运行态以旧值为准。每条任务字段：name=名称；message=到点发送的内容；kind=message（固定消息，缺省）|memory（先回顾 memory_days 天对话再唤醒）；enabled=开关。时间二选一：cron=5 段「分 时 日 月 周」本地时区周期表达式（如 25 20 4 10 * = 每年 10 月 4 日 20:25）；once_ts=unix 秒时间戳的单次任务（到点跑一次自动停用，此时 cron 留空）。可选：workspace=绝对路径限定工作区；config_id+model=用哪套渠道跑；timeout_secs=整轮超时秒；memory_days=回顾天数 1-90；budget_tokens=上下文上限",
    },
    // —— features.* 点号子路径（单开关；整表替换请用顶层 features）——
    SchemaEntry {
        path: "features.roles",
        label: "功能桥接",
        ty: "bool",
        enum_values: &[],
        default: Some("true"),
        risk: "normal",
        desc: "关=摘要/标题/嵌入/多模态/子代理/预算降级全部回主渠道或默认退化",
    },
    SchemaEntry {
        path: "features.css_tool",
        label: "css-ui 工具",
        ty: "bool",
        enum_values: &[],
        default: Some("true"),
        risk: "normal",
        desc: "关=收起工具入口，已调好的样式继续生效",
    },
    SchemaEntry {
        path: "features.galaxy",
        label: "星云入口",
        ty: "bool",
        enum_values: &[],
        default: Some("true"),
        risk: "normal",
        desc: "关=只藏入口，记忆功能本体不受影响",
    },
    SchemaEntry {
        path: "features.cancel_on_disconnect",
        label: "断线取消任务",
        ty: "bool",
        enum_values: &[],
        default: Some("false"),
        risk: "normal",
        desc: "开=检测到 SSE 断开就取消本轮任务（默认关=后台跑完，刷新页面能接着聊）",
    },
    SchemaEntry {
        path: "features.self_mod",
        label: "自改总闸",
        ty: "bool",
        enum_values: &[],
        default: Some("true"),
        risk: "normal",
        desc: "关=agent 所有「改自身」动作（设置/界面/会话）一律拒绝并引导；只读不闸",
    },
    // 注意：电脑操作闸已并入插件列表——「电脑操作」插件的
    // 停用开关就是总闸，与浏览器同款。旧 features.computer 刻意不入表：agent 自己
    // 不得读写这个总闸；插件停用走 plugin 工具的审批/信任闸，
    // 同样不在 agent 手里。
    // —— roles.* 点号子路径（旁路专用配置名；None=回落主渠道）——
    SchemaEntry {
        path: "roles.summary",
        label: "滚动摘要专用配置",
        ty: "config_name",
        enum_values: &[],
        default: None,
        risk: "normal",
        desc: "配置名或 id（未设回落主力渠道）",
    },
    SchemaEntry {
        path: "roles.vision",
        label: "视觉理解专用配置",
        ty: "config_name",
        enum_values: &[],
        default: None,
        risk: "normal",
        desc: "配置名或 id（多模态旁路）",
    },
    SchemaEntry {
        path: "roles.video",
        label: "视频理解专用配置",
        ty: "config_name",
        enum_values: &[],
        default: None,
        risk: "normal",
        desc: "配置名或 id（多模态旁路）",
    },
    SchemaEntry {
        path: "roles.audio",
        label: "音频理解专用配置",
        ty: "config_name",
        enum_values: &[],
        default: None,
        risk: "normal",
        desc: "配置名或 id（多模态旁路）",
    },
    SchemaEntry {
        path: "roles.embedding",
        label: "向量嵌入专用配置",
        ty: "config_name",
        enum_values: &[],
        default: None,
        risk: "normal",
        desc: "配置名或 id（未设用本地哈希向量，不用主渠道兜底）",
    },
    SchemaEntry {
        path: "roles.budget",
        label: "预算降级配置",
        ty: "config_name",
        enum_values: &[],
        default: None,
        risk: "normal",
        desc: "配置名或 id（单任务累计超预算后剩余步骤改走它）",
    },
    SchemaEntry {
        path: "roles.subagent",
        label: "子代理专用配置",
        ty: "config_name",
        enum_values: &[],
        default: None,
        risk: "normal",
        desc: "配置名或 id（未设回落主渠道）；仅当 roles.subagent_mode=custom 时生效",
    },
    SchemaEntry {
        path: "roles.subagent_mode",
        label: "子代理派发模型",
        ty: "string",
        enum_values: &["parent", "custom"],
        default: Some("parent"),
        risk: "normal",
        desc: "parent=跟随父对话（子代理用父任务当前正在用的渠道/模型）；custom=用 roles.subagent 专用配置",
    },
    SchemaEntry {
        path: "roles.fallbacks",
        label: "降级备用配置链",
        ty: "string_array",
        enum_values: &[],
        default: None,
        risk: "normal",
        desc: "按序降级的配置名/ id 列表",
    },
    SchemaEntry {
        path: "compact_percent",
        label: "上下文压缩线",
        ty: "integer",
        enum_values: &[],
        default: Some("缺省 85；0=恢复默认"),
        risk: "normal",
        desc: "窗口百分比 40–85；达到阈值在本轮收尾后静默压缩",
    },
];

/// 某 path 在不在表内。
pub fn is_known_path(path: &str) -> bool {
    SCHEMA.iter().any(|e| e.path == path)
}

/// 红线路径集合（由表派生：risk == red_line）。core::rules 的红线判定改查它，
/// 不再手写第二份名单。
pub fn red_line_paths() -> Vec<&'static str> {
    SCHEMA
        .iter()
        .filter(|e| e.risk == "red_line")
        .map(|e| e.path)
        .collect()
}

/// 某 path 的风险档（未知 = None，调用方按「表外一律拒」处理）。
pub fn risk_of(path: &str) -> Option<&'static str> {
    SCHEMA.iter().find(|e| e.path == path).map(|e| e.risk)
}

/// 合法路径清单（报错指路用；点号子路径与顶层键平铺）。
pub fn legal_paths_hint() -> String {
    SCHEMA
        .iter()
        .map(|e| e.path)
        .collect::<Vec<_>>()
        .join(" / ")
}

/// patch 应用顺序 = 表序（调用方按 SCHEMA 序遍历补件，别处不再定义第二序）。
pub fn application_order() -> impl Iterator<Item = &'static SchemaEntry> {
    SCHEMA.iter()
}

/// 全表可序列化清单（action=schema / GET /api/settings/schema 的报体）。
pub fn manifest_json() -> Value {
    json!({
        "paths": SCHEMA.iter().map(|e| json!({
            "path": e.path,
            "label": e.label,
            "type": e.ty,
            "enum": e.enum_values,
            "default": e.default,
            "risk": e.risk,
            "desc": e.desc,
        })).collect::<Vec<Value>>(),
        "out_of_scope": "api_key/clear_key（key 永不经模型之手，换 key 请到设置页）/workspace（自己挪自己的笼子）不可经本工具修改。审批档位不在本表：config.toml 的 default_approval 是它的持久形态，set_config 改它一律弹卡。config.toml 其余键走 action=set_config（改完需重启生效）。插件的安装/删除走 plugin 工具（install/remove）",
    })
}

/// 把一个点号/顶层路径的值写进 Settings。**现有逐字段语义原样搬迁**：
/// - 顶层渠道字段复用 `server::settings::apply_settings_update`（与 HTTP 端点同一套）；
/// - `active_config_id`  extras 现 `Settings::activate`；
/// - `features.<键>` 走 `FeatureSettings::set`（键名白名单与报错口径不变）；
/// - `roles.<字段>` 点号只写指定字段（不整表覆盖其余）；
/// - 整表类（roles/features/mcp_servers/disabled_*/schedules）沿用原 apply_patch 分支
///   （该校验/联动在本函数外，见 settings 工具侧的整表辅助）——表中只搬运纯落值语义。
///
/// 返回本路径的人话变更行。
pub(crate) fn apply_entry_value(s: &mut Settings, path: &str, v: &Value) -> Result<String, String> {
    match path {
        "active_config_id" => {
            let id = v.as_str().unwrap_or("").trim();
            if id.is_empty() {
                return Err("active_config_id 不能为空串".into());
            }
            if !s.activate(id) {
                return Err(format!("配置不存在: {id}（先 action=get 查看全部配置 id）"));
            }
            Ok(format!("active_config_id → {id}"))
        }
        "provider_id" | "base_url" | "model" | "name" | "models" | "context_length"
        | "max_output_tokens" | "temperature" | "top_p" | "thinking" | "thinking_style"
        | "capabilities" => {
            let mut upd = crate::server::settings::SettingsUpdate::default();
            match path {
                "provider_id" => upd.provider_id = v.as_str().map(str::to_string),
                "base_url" => upd.base_url = v.as_str().map(str::to_string),
                "model" => upd.model = v.as_str().map(str::to_string),
                "name" => upd.name = v.as_str().map(str::to_string),
                "models" => {
                    upd.models = v
                        .as_array()
                        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect());
                    if upd.models.is_none() {
                        return Err(format!("{path} 需要字符串数组"));
                    }
                }
                "context_length" => upd.context_length = v.as_u64(),
                "max_output_tokens" => upd.max_output_tokens = v.as_u64(),
                "temperature" => upd.temperature = v.as_f64(),
                "top_p" => upd.top_p = v.as_f64(),
                "thinking" => upd.thinking = v.as_str().map(str::to_string),
                "thinking_style" => upd.thinking_style = v.as_str().map(str::to_string),
                "capabilities" => {
                    upd.capabilities = serde_json::from_value(v.clone())
                        .map_err(|e| format!("capabilities 不合法: {e}"))?;
                }
                _ => unreachable!("match 臂与上方模式同步漏写"),
            }
            let extra_note = match path {
                "model" => "model（注意：同补辰里改 base_url 会清空模型，需要成对给）",
                "base_url" => "base_url（渠道地址变更：模型与模型列表会清空，需重新拉取）",
                _ => path,
            };
            let (url_changed, _, _) = crate::server::settings::apply_settings_update(s, &upd)?;
            if path == "model" && url_changed {
                // 与「同补辰 base_url+model」老语义对齐：model 分支本来就允许多 apply 一遍
                let again = crate::server::settings::SettingsUpdate {
                    model: upd.model.clone(),
                    ..Default::default()
                };
                crate::server::settings::apply_settings_update(s, &again)?;
            }
            let mut out = extra_note.to_string();
            if url_changed && path == "base_url" {
                out.push_str("；模型与模型列表已清空");
            }
            Ok(out)
        }
        "roles" => {
            let Some(r) = v.as_object() else {
                return Err("roles 需要 object".into());
            };
            let mut next: crate::config::settings::RolesSettings =
                s.roles.clone().unwrap_or_default();
            for (k, val) in r {
                let sub = format!("roles.{k}");
                if !SCHEMA.iter().any(|e| e.path == sub) {
                    return Err(format!(
                        "未知 roles 字段 {k}（合法：{}）",
                        SCHEMA
                            .iter()
                            .filter(|e| e.path.starts_with("roles."))
                            .map(|e| e.path.trim_start_matches("roles."))
                            .collect::<Vec<_>>()
                            .join(" / ")
                    ));
                }
                apply_role_field(&mut next, k, val)?;
            }
            s.roles = Some(next);
            Ok("roles".into())
        }
        "features" => {
            let Some(f) = v.as_object() else {
                return Err("features 需要 object".into());
            };
            for (k, val) in f {
                let on = val.as_bool().ok_or_else(|| format!("features.{k} 需要 true/false"))?;
                s.features.set(k, on)?;
            }
            Ok("features".into())
        }
        "mcp_servers" | "disabled_plugins" | "disabled_skills" | "schedules" => {
            // 整表替换带校验/联动，主体在 settings 工具侧（apply_patch 全表分支），
            // 此处只接「值已由调用方预处理」的情形——全表动作不走点号路径，
            // 表驱动主循环直接走原分支（见 tools/settings.rs）。
            Err(format!(
                "{path} 是整表替换键，patch 里按顶层键整体给（不要拆点号子路径）"
            ))
        }
        p if p.starts_with("features.") => {
            let key = &p["features.".len()..];
            let on = v.as_bool().ok_or_else(|| format!("{p} 需要 true/false"))?;
            s.features.set(key, on)
                .map_err(|e| format!("{p} 写不进去: {e}"))?;
            Ok(p.to_string())
        }
        "compact_percent" => {
            let Some(n) = v.as_u64() else {
                return Err("compact_percent 需要整数（0=恢复默认，40–85 生效）".into());
            };
            let n = n.min(u8::MAX as u64) as u8;
            if n == 0 {
                s.compact_percent = None;
            } else if (40..=85).contains(&n) {
                s.compact_percent = Some(n);
            } else {
                return Err(format!("compact_percent={n} 不在 40–85（0=恢复默认）"));
            }
            Ok("compact_percent".into())
        }
        p if p.starts_with("roles.") => {
            let key = &p["roles.".len()..];
            let cur: &mut Option<crate::config::settings::RolesSettings> = &mut s.roles;
            let r = cur.get_or_insert_with(Default::default);
            apply_role_field(r, key, v)?;
            Ok(p.to_string())
        }
        other => Err(format!("path「{other}」不在设置 schema 表内（{}）", legal_paths_hint())),
    }
}

/// roles.<字段> 的纯落值（配置名/ id 串或降级链数组）。
fn apply_role_field(
    r: &mut crate::config::settings::RolesSettings,
    key: &str,
    v: &Value,
) -> Result<(), String> {
    let put = |dst: &mut Option<String>| -> Result<(), String> {
        // 类型不对必须报错：原来 `as_str().unwrap_or("")` 会把非字符串当成「清除」，
        // 静默清空角色路由还返回成功（2026-11-02 审计）。
        let s = v
            .as_str()
            .ok_or_else(|| format!("roles.{key} 需要字符串（配置名或 id）"))?
            .trim()
            .to_string();
        *dst = if s.is_empty() { None } else { Some(s) }; // 空串 = 清除回落
        Ok(())
    };
    match key {
        "summary" => put(&mut r.summary),
        "vision" => put(&mut r.vision),
        "video" => put(&mut r.video),
        "audio" => put(&mut r.audio),
        "embedding" => put(&mut r.embedding),
        "budget" => put(&mut r.budget),
        "subagent" => put(&mut r.subagent),
        // 子代理模式（parent=跟随主渠道 / custom=用 roles.subagent 指定模型）：
        // schema 把 roles.subagent_mode 列为可改路径，这里必须认——不然模型按工具描述
        // 去改会整条 patch 报错（审计 2026-11-08：schema 有入口、应用层不认的老洞）
        "subagent_mode" => {
            let s = v
                .as_str()
                .ok_or_else(|| "roles.subagent_mode 需要字符串（parent / custom）".to_string())?
                .trim()
                .to_string();
            match s.as_str() {
                // 空串 = 清除回落 parent；不认识的值也按 parent 处理（与读盘迁移同口径）
                "" | "parent" => r.subagent_mode = None,
                "custom" => r.subagent_mode = Some("custom".into()),
                other => return Err(format!("roles.subagent_mode 只能是 parent / custom（现在是「{other}」）")),
            }
            Ok(())
        }
        "fallbacks" => {
            r.fallbacks = v
                .as_array()
                .ok_or("roles.fallbacks 需要字符串数组")?
                .iter()
                .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect();
            Ok(())
        }
        other => Err(format!(
            "未知 roles 字段 {other}（合法：summary/vision/video/audio/embedding/budget/subagent/subagent_mode/fallbacks）"
        )),
    }
}

/// 触发 settings 模块的类型/函数可见性检查（编译期断言表字段与 Settings 对齐）。
#[allow(dead_code)]
fn _compile_time_assertions() {
    let _ = crate::config::settings::shared_settings;
    let _: fn(&mut Settings, &str, &Value) -> Result<String, String> = apply_entry_value;
}

#[cfg(test)]
mod tests {
    //! 设置 schema 是「agent 自改设置」的唯一白名单：一张表同时喂 patch 应用顺序、
    //! 红线集合、工具描述键清单和 /api/settings/schema。表本身没有类型系统兜底，
    //! 这些不变式用单测钉死（2026-10-30 工程能力轮）。
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn paths_are_unique_and_non_empty() {
        let mut seen = HashSet::new();
        for e in SCHEMA {
            assert!(!e.path.trim().is_empty(), "path 不能为空");
            assert!(seen.insert(e.path), "path 重复: {}", e.path);
            assert!(!e.label.trim().is_empty(), "label 不能为空: {}", e.path);
            assert!(!e.desc.trim().is_empty(), "desc 不能为空: {}", e.path);
        }
    }

    #[test]
    fn types_and_risks_stay_in_vocabulary() {
        // 词表与前端/工具描述共用；新增类型必须同步改这里和应用逻辑
        let types = ["bool", "string", "integer", "float", "string_array", "config_name", "object"];
        for e in SCHEMA {
            assert!(types.contains(&e.ty), "未知类型 {}（路径 {}）", e.ty, e.path);
            assert!(
                matches!(e.risk, "normal" | "red_line"),
                "未知风险档 {}（路径 {}）",
                e.risk,
                e.path
            );
        }
    }

    #[test]
    fn enum_values_are_nonempty_and_unique() {
        for e in SCHEMA {
            let mut seen = HashSet::new();
            for v in e.enum_values {
                assert!(!v.trim().is_empty(), "枚举值不能为空: {}", e.path);
                assert!(seen.insert(*v), "枚举值重复 {}: {}", v, e.path);
            }
        }
    }

    #[test]
    fn active_config_id_must_apply_first() {
        // 注释里的排序约定：切配置必须排在所有渠道字段之前，否则「切完再改参数」
        // 会落到切换前的配置。这里把它变成可执行断言。
        assert_eq!(SCHEMA.first().map(|e| e.path), Some("active_config_id"));
        assert_eq!(application_order().next().map(|e| e.path), Some("active_config_id"));
    }

    #[test]
    fn manifest_matches_table_order_and_size() {
        let m = manifest_json();
        let paths = m["paths"].as_array().expect("manifest.paths 应为数组");
        assert_eq!(paths.len(), SCHEMA.len());
        for (i, e) in SCHEMA.iter().enumerate() {
            assert_eq!(paths[i]["path"], json!(e.path), "manifest 顺序必须与表一致");
            assert_eq!(paths[i]["type"], json!(e.ty));
            assert_eq!(paths[i]["risk"], json!(e.risk));
        }
    }

    #[test]
    fn helper_lookups_agree_with_table() {
        for e in SCHEMA {
            assert!(is_known_path(e.path));
            assert_eq!(risk_of(e.path), Some(e.risk));
        }
        assert!(!is_known_path("definitely.not.a.path"));
        assert_eq!(risk_of("definitely.not.a.path"), None);
        let red = red_line_paths();
        let expected: Vec<&str> = SCHEMA
            .iter()
            .filter(|e| e.risk == "red_line")
            .map(|e| e.path)
            .collect();
        assert_eq!(red, expected);
        let hint = legal_paths_hint();
        for e in SCHEMA {
            assert!(hint.contains(e.path), "合法路径提示漏了 {}", e.path);
        }
    }

    #[test]
    fn roles_and_features_subpaths_are_consistent() {
        // roles 点号子路径只能来自这两处的并集：单独列字段 + 整表 roles。
        let roles: HashSet<&str> = SCHEMA
            .iter()
            .filter_map(|e| e.path.strip_prefix("roles."))
            .collect();
        for k in ["summary", "vision", "video", "audio", "embedding", "budget", "subagent", "fallbacks"] {
            assert!(roles.contains(k), "roles.{k} 不在 schema 表内");
        }
        // 整表键要在表内（FeatureSettings::set 的键白名单由运行时兜底）。
        assert!(SCHEMA.iter().any(|e| e.path == "features"));
        assert!(SCHEMA.iter().any(|e| e.path == "mcp_servers"));
        assert!(SCHEMA.iter().any(|e| e.path == "schedules"));
    }

    #[test]
    fn compact_percent_roundtrip_and_bounds() {
        let mut s = crate::config::settings::Settings::default();
        assert_eq!(apply_entry_value(&mut s, "compact_percent", &serde_json::json!(60)).unwrap(), "compact_percent");
        assert_eq!(s.compact_percent, Some(60));
        assert!(apply_entry_value(&mut s, "compact_percent", &serde_json::json!(30)).is_err(), "低于 40 拒收");
        assert!(apply_entry_value(&mut s, "compact_percent", &serde_json::json!(90)).is_err(), "高于 85 拒收");
        assert!(apply_entry_value(&mut s, "compact_percent", &serde_json::json!("六十")).is_err(), "类型不对必须报错");
        apply_entry_value(&mut s, "compact_percent", &serde_json::json!(0)).unwrap();
        assert_eq!(s.compact_percent, None, "0 = 恢复默认 85");
    }
}
