//! 会话管理 —— 总架构 §13 / 、008
//!
//! 多会话：每条会话一个文件 `<log_dir>/sessions/<id>.json`（OpenAI 消息数组，不含 system），
//! 新建对话不清空历史；标题取首条用户消息。核心循环每关键点发 Snapshot 事件，server
//! 按 session_id 落盘；前端启动拉 /api/sessions 列表、点选切换。落盘即续命。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::Context;
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 会话级用量快照（圆环与缓存行的落盘续命）：随 ContextUsage/ApiUsage 事件更新进会话
/// 文件，重启/重开程序后由 /api/sessions/open 带回前端恢复，不用等下一轮请求再攒。
///
/// 口径对齐 DSH token-meter：圆环不再存「一个 tokens」，而是拆成三个语义明确的字段——
/// `projected`（下一次请求的预估占用，圆环分子）、`pressure`（最近一次渠道实报的 prompt，
/// 锚点）、`window`（模型窗口，圆环分母）。旧文件里的 `tokens`/`budget` 用 serde alias 兼容。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionUsage {
    /// 圆环分子：下一次请求的预估占用 = 实报锚点 + 自锚点以来的带符号增量
    /// （`context::projected_tokens`）。旧字段名 tokens 语义最接近它，按 alias 收下。
    #[serde(default, alias = "tokens")]
    pub projected: u64,
    /// 实报锚点：最近一次渠道实报的 prompt_tokens（渠道不回 usage / 首请求前为 None）。
    /// **展示用**（「实报 x」），跨轮沿用锚点用的是下面 projected + anchor_est 这一对。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pressure: Option<u64>,
    /// 与本次 `projected` 配对的本地估价（对齐 DSH 的 sampledSurfaceTokens）。
    /// 跨轮沿用锚点靠 `(projected, anchor_est)` 这一对：下一轮的占用 =
    /// projected + (当轮估价 − anchor_est)，所以新起一轮不会退回估算口径。
    #[serde(default)]
    pub anchor_est: u64,
    /// 圆环分母：模型窗口（模型 context_length 或全局 [context].budget_tokens）
    #[serde(default, alias = "budget")]
    pub window: u64,
    /// 分类占比（估算口径）：user+assistant 正文 / 工具结果 / 系统提示词 / 结构开销 / 工具定义
    #[serde(default)]
    pub messages: u64,
    #[serde(default)]
    pub tools: u64,
    #[serde(default)]
    pub system: u64,
    #[serde(default)]
    pub other: u64,
    #[serde(default)]
    pub tools_schema: u64,
    /// 主循环渠道实报累计（平均缓存命中率行）
    #[serde(default)]
    pub api_prompt: u64,
    #[serde(default)]
    pub api_cached: u64,
}

impl SessionUsage {
    /// 上下文占用事件 → 快照（圆环三字段 + 构成 + 跨轮锚点对）。chat 与无头两条路径
    /// 共用这一处，避免「网页聊天写、无头/定时/子代理不写」导致切回去圆环是空的。
    pub fn apply_context(
        &mut self,
        pressure: Option<u64>,
        projected: u64,
        window: u64,
        b: &crate::context::Breakdown,
    ) {
        self.pressure = pressure;
        self.projected = projected;
        self.window = window;
        // 本次 projected 的配对估价 = 本次构成总量（构成与测量同一口径，见 context::Breakdown）
        self.anchor_est = b.total();
        self.messages = b.messages;
        self.tools = b.tools;
        self.system = b.system;
        self.other = b.other;
        self.tools_schema = b.tools_schema;
    }

    /// 渠道实报累计并入（平均缓存命中率行）。圆环锚点不在这里更新——
    /// 锚点随 ContextUsage 一起落（那时回复已进上下文，落盘即终态）。
    pub fn apply_api(&mut self, prompt_tokens: u64, cached_tokens: Option<u64>) {
        self.api_prompt = self.api_prompt.saturating_add(prompt_tokens);
        self.api_cached = self.api_cached.saturating_add(cached_tokens.unwrap_or(0));
    }

    /// 跨轮沿用锚点：`(基线占用, 该基线对应的本地估价)`。没数据（新会话/旧文件）返回 None。
    pub fn anchor(&self) -> Option<(u64, u64)> {
        (self.projected > 0 && self.anchor_est > 0).then_some((self.projected, self.anchor_est))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionFile {
    pub id: String,
    /// 首条用户消息截断（列表展示用）
    #[serde(default)]
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub messages: Vec<Value>,
    /// 滚动摘要（跨任务延续；无摘要时缺省）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// 创建时所在的工作区（DSH 式侧边栏「按工作区」分组用；旧文件缺省=未分组）
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub workspace: String,
    /// DSH 式侧边栏：置顶（列表内沉底排序的最前）
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    /// 【已废弃】旧归档标记（2026-10-31 机制移除，只保留删除）；字段仅兼容旧文件读取，新代码不得读写
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
    /// 用量快照（重启后恢复圆环/缓存行；旧文件缺省 None）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<SessionUsage>,
    /// 压缩记录（2026-11-07 静默压缩）：请求视图 = 摘要消息 + tail_start 起的历史。
    /// 会话文件始终存**完整历史**，压缩只影响请求侧；标记行随消息落盘（display_only）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact: Option<crate::compact::CompactRecord>,
}

/// 列表条目（不携带消息体）。
#[derive(Debug, Clone, Serialize)]
pub struct SessionMeta {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub count: usize,
    pub workspace: String,
    pub pinned: bool,
}

pub struct SessionManager {
    dir: PathBuf,
    /// 按会话 id 的写锁（2026-10-08 修）：save/update_usage/rename/set_flags 全是
    /// 「读整文件→改一个字段→整文件回写」，无锁时两个写者交错会把先写者的更新整体
    /// 抹掉（典型：任务收尾的最后一次 Snapshot 与用户点「置顶」同时落，最终回答
    /// 从会话文件里消失）。条目数随会话数增长，每个就一把空锁，可忽略。
    locks: RwLock<HashMap<String, Arc<Mutex<()>>>>,
}

/// session_id 只允许 uuid 安全字符，直接拼文件名防路径穿越。
fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// 空会话的占位标题（2026-10-30 统一叫「新对话」）。旧占位「新会话」继续
/// 按占位处理——盘上存量会话带的是旧值，不认它就会把「新会话」当真实标题显示出来。
pub(crate) const PLACEHOLDER_TITLE: &str = "新对话";
const LEGACY_PLACEHOLDER_TITLE: &str = "新会话";

fn is_placeholder_title(t: &str) -> bool {
    let t = t.trim();
    t == PLACEHOLDER_TITLE || t == LEGACY_PLACEHOLDER_TITLE
}

/// 标题 = 首条用户消息的前 10 字（2026-11-06 起不再调模型生成标题）。
/// 换行/连续空白折成单空格，侧栏单行显示不会出现半个空行。
fn first_user_title(messages: &[Value]) -> String {
    messages
        .iter()
        .filter(|m| m.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(|m| m.get("content").and_then(Value::as_str))
        .find(|s| !s.trim().is_empty())
        .map(|s| {
            let mut s = s.trim();
            // 剥离形如 [Audio understanding...] 或 [audio omitted...] 的前置投影块
            while s.starts_with('[') {
                if let Some(end_idx) = s.find(']') {
                    s = s[end_idx + 1..].trim();
                } else {
                    break;
                }
            }
            let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
            if s.is_empty() {
                "媒体理解对话".to_string()
            } else {
                s.chars().take(10).collect::<String>()
            }
        })
        .unwrap_or_else(|| PLACEHOLDER_TITLE.to_string())
}

/// RFC3339 → 本地「月-日 时:分」，给项目现状摘要用（解析失败原样返回，不报错）。
fn fmt_ts(iso: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso)
        .map(|t| t.with_timezone(&chrono::Local).format("%m-%d %H:%M").to_string())
        .unwrap_or_else(|_| iso.to_string())
}

/// 会话正文抽取（面板转写端点与 memory 工具读原文同一条路，常量不各抄一份）：
/// 只取 user/assistant 的文本，单条封顶 `max_msg` 字符；总量超 `max_total` 预算时
/// 从**最早**的消息开始丢（保尾部——最近的往来信息量最大）。
/// 返回 (保留的消息, 是否真的因预算丢过内容)。
///
/// `omitted` 只认预算截断：会话里的 tool 消息与空消息本就不进转写，拿
/// 「保留数 < 总会话数」判定会让一条没丢内容的会话也报「只保留了最近部分」
/// （前端 @引用 与 memory 工具的提示都信它，等于对用户说假话）。
pub fn transcript(f: &SessionFile, max_total: usize, max_msg: usize) -> (Vec<(String, String)>, bool) {
    let mut msgs: Vec<(String, String)> = Vec::new();
    for m in &f.messages {
        let Some(role) = m.get("role").and_then(Value::as_str) else { continue };
        if role != "user" && role != "assistant" {
            continue;
        }
        let text = match m.get("content") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(parts)) => parts
                .iter()
                .filter_map(|p| {
                    if p.get("type").and_then(Value::as_str) == Some("text") {
                        p.get("text").and_then(Value::as_str)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
            _ => continue,
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let mut text = text.to_string();
        if text.chars().count() > max_msg {
            text = format!("{}…", text.chars().take(max_msg).collect::<String>());
        }
        msgs.push((role.to_string(), text));
    }
    let mut used = 0usize;
    let mut kept: Vec<(String, String)> = Vec::new();
    let mut truncated = false;
    for (role, text) in msgs.into_iter().rev() {
        let cost = text.chars().count() + 8;
        if used + cost > max_total {
            truncated = true;
            break;
        }
        used += cost;
        kept.push((role, text));
    }
    kept.reverse();
    (kept, truncated)
}

impl SessionManager {
    /// dir 为日志目录，会话文件落 `<dir>/sessions/`。
    pub fn new(log_dir: &Path) -> Self {
        Self { dir: log_dir.join("sessions"), locks: RwLock::new(HashMap::new()) }
    }

    /// 取（或建）某会话的写锁。持锁期间只做本会话文件的读-改-写，不调其他会拿
    /// 本锁的方法（get/write 都不拿锁），无死锁面。
    fn session_lock(&self, id: &str) -> Arc<Mutex<()>> {
        let mut m = self.locks.write().unwrap_or_else(|e| e.into_inner());
        m.entry(id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    /// 压缩完整记录目录：`<日志目录>/transcripts/<会话id>-<序号>.md`。
    /// 摘要把细节压薄了，原文在这里兜底（模型可按摘要消息里带的路径回读）。
    pub fn transcripts_dir(&self) -> PathBuf {
        self.dir.parent().unwrap_or(Path::new(".")).join("transcripts")
    }

    /// 压缩完成落盘：写 compact 记录 + 在触发轮最后一条消息后插入完成标记行 +
    /// 更新 summary（新会话 brief 继续有料）。锚点消息已不在（回滚/删除）→
    /// 返回 Ok(false)：结果作废不落盘，等下一轮重新压。
    pub fn apply_compaction(
        &self,
        id: &str,
        record: crate::compact::CompactRecord,
        marker: Value,
    ) -> anyhow::Result<bool> {
        if !safe_id(id) {
            anyhow::bail!("非法 session id");
        }
        let lock = self.session_lock(id);
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        let Some(mut f) = self.get(id) else { return Ok(false) };
        // 旧标记行先清掉：一条会话只有一条压缩记录，标记行永远代表**最近一次**
        // 压缩的边界。原先按内容去重（同文=已插过）——两次压缩后 token 取整相同
        // （如两次都报 17k）时新边界标记被吞，前端按文件重建后底部那行直接消失
        // （真机 2026-11-07：压缩提示弹出→消失，刷新才在历史中部看到上一次的旧行）。
        f.messages.retain(|m| m.get("kind").and_then(Value::as_str) != Some("compact_marker"));
        let Some(after_idx) =
            f.messages.iter().position(|m| m.get("id").and_then(Value::as_str) == Some(&record.after_id))
        else {
            return Ok(false);
        };
        if !f.messages.iter().any(|m| m.get("id").and_then(Value::as_str) == Some(&record.tail_start_id)) {
            return Ok(false);
        }
        f.compact = Some(record);
        f.summary = f.compact.as_ref().map(|c| c.summary.clone());
        let insert_at = (after_idx + 1).min(f.messages.len());
        f.messages.insert(insert_at, marker);
        f.updated_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        self.write(&f)?;
        Ok(true)
    }

    /// 新建对话 id（不落盘；首条消息时 save 才建文件）。
    pub fn new_id(&self) -> String {
        uuid::Uuid::new_v4().to_string()
    }

    /// 新建一条空会话并立即落盘：点「新会话」就在侧栏当前项目下留一条，
    /// 不用等首条消息。同一工作区已有**没动过的**空会话（占位标题、无用量快照）
    /// 时直接复用，避免连点堆出一排空会话。
    /// 回滚成空的旧对话不算：标题还是原来的、用量快照也在，复用它会让「新建对话」
    /// 落回原对话（标题还是一开始的）。
    pub fn create_empty(&self, workspace: &str, title: &str) -> anyhow::Result<SessionFile> {
        if let Some(f) = self
            .list()
            .into_iter()
            .filter(|m| m.count == 0 && m.workspace == workspace)
            .filter_map(|m| self.get(&m.id))
            .find(|f| is_placeholder_title(&f.title) && f.usage.is_none())
        {
            return Ok(f);
        }
        std::fs::create_dir_all(&self.dir).with_context(|| format!("建会话目录失败: {}", self.dir.display()))?;
        let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let f = SessionFile {
            id: self.new_id(),
            title: title.to_string(),
            created_at: now.clone(),
            updated_at: now,
            messages: Vec::new(),
            summary: None,
            workspace: workspace.to_string(),
            pinned: false,
            archived: false,
            usage: None,
                compact: None,
        };
        self.write(&f)?;
        Ok(f)
    }

    /// 原子写（临时文件 + rename），save / rename / set_meta / fork 共用。
    /// 存储一门：会话落盘只走 `storage::write_session`（仓内唯一入口 + 审计）。
    fn write(&self, f: &SessionFile) -> anyhow::Result<()> {
        let text = serde_json::to_string(f).context("会话序列化失败")?;
        crate::storage::write_session(&self.path(&f.id), &text)
    }

    /// 落盘（原子写）：已存在则保留 created_at/标题/置顶/归档/工作区，否则标题取首条
    /// 用户消息、工作区记为当前工作区。summary 传 Some 才更新（None 保留旧值）。
    pub fn save(&self, id: &str, messages: &[Value], summary: Option<&str>) -> anyhow::Result<()> {
        if !safe_id(id) {
            anyhow::bail!("非法 session id");
        }
        let lock = self.session_lock(id);
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        std::fs::create_dir_all(&self.dir).with_context(|| format!("建会话目录失败: {}", self.dir.display()))?;
        let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        // 文件存在但读不出（损坏/被占用）≠ 新会话：get() 的 None 分不清这两种，
        // 早先按新会话整份覆写 → 置顶/归档/标题/创建时间/用量快照被静默抹掉。
        // 2026-10-08 修：存在却读不出时拒绝覆写（保住盘上现场，下次 save 再试），
        // 只有「文件确实不在」才走新会话分支。
        let path = self.path(id);
        let existed = path.exists();
        // 用量快照随文件保留：save 由消息侧高频调用，不能把 update_usage 写的快照抹掉
        let (title, created_at, old_summary, workspace, pinned, old_usage, old_compact) = match self.get(id) {
            Some(old) => (
                // 空会话先落盘（标题=占位），首条消息到了就把标题补成前 10 字并持久化；
                // 不再靠 list() 的显示兜底，打开会话的顶栏也能拿到同一个标题。
                // 空标题（老会话/被清空）同样在下次保存时自愈。
                if is_placeholder_title(&old.title) || old.title.trim().is_empty() {
                    first_user_title(messages)
                } else {
                    old.title
                },
                old.created_at,
                old.summary,
                old.workspace,
                old.pinned,
                old.usage,
                old.compact,
            ),
            None if !existed => (
                first_user_title(messages),
                now.clone(),
                None,
                crate::tools::workspace_root().display().to_string(),
                false,
                None,
                None,
            ),
            None => {
                anyhow::bail!(
                    "会话文件存在但读不出（损坏或被占用），拒绝覆写以防丢元数据: {}",
                    path.display()
                );
            }
        };
        let mut file = SessionFile {
            id: id.to_string(),
            title,
            created_at,
            updated_at: now,
            // 缺 id 的消息补 uuid：压缩边界/标记行锚点都按 id 定位（老会话首次重存即升级）
            messages: Self::ensure_ids(messages.to_vec()),
            summary: summary.map(|s| s.to_string()).or(old_summary),
            workspace,
            pinned,
            archived: false,
            usage: old_usage,
            compact: old_compact,
        };
        // 快照覆写保护（2026-11-07）：运行中的轮次拿的是压缩前的全量历史，它的快照
        // 不含压缩标记行——记录还在而标记丢了时按记录补回，别让「压缩后上下文」行
        // 在下一轮落盘时无声消失。
        if file.compact.is_some()
            && !file.messages.iter().any(|m| m.get("kind").and_then(Value::as_str) == Some("compact_marker"))
            && let Some(rec) = file.compact.clone()
            && let Some(i) =
                file.messages.iter().position(|m| m.get("id").and_then(Value::as_str) == Some(&rec.after_id))
        {
            let at = (i + 1).min(file.messages.len());
            file.messages.insert(at, crate::compact::marker_message(&crate::compact::done_marker_text(rec.post_tokens)));
        }
        self.write(&file)
    }

    /// 给缺 id 的消息补 uuid。压缩边界（tail_start/after）按消息 id 定位，
    /// 老会话首次重存即升级；content 之外的界面字段不动。
    fn ensure_ids(mut msgs: Vec<Value>) -> Vec<Value> {
        for m in &mut msgs {
            let missing = m.get("id").and_then(Value::as_str).map(str::is_empty).unwrap_or(true);
            if missing && let Some(obj) = m.as_object_mut() {
                obj.insert("id".into(), serde_json::json!(uuid::Uuid::new_v4().to_string()));
            }
        }
        msgs
    }

    /// 只更新用量快照（读-改-写，其余字段以盘上为准）。ContextUsage/ApiUsage 事件时调；
    /// 会话文件还没落盘时失败即跳过（下一事件再补，圆环少一次快照不致命）。
    pub fn update_usage(&self, id: &str, usage: SessionUsage) -> anyhow::Result<()> {
        if !safe_id(id) {
            anyhow::bail!("非法 session id");
        }
        let lock = self.session_lock(id);
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut f = self.get(id).ok_or_else(|| anyhow::anyhow!("会话不存在: {id}"))?;
        f.usage = Some(usage);
        self.write(&f)
    }

    pub fn get(&self, id: &str) -> Option<SessionFile> {
        if !safe_id(id) {
            return None;
        }
        let text = match std::fs::read_to_string(self.path(id)) {
            Ok(t) => t,
            Err(_) => return None,
        };
        match serde_json::from_str::<SessionFile>(&text) {
            Ok(f) => Some(f),
            Err(e) => {
                crate::observability::debug(&format!("会话文件损坏，忽略: {e}"));
                None
            }
        }
    }

    /// 最近更新的会话（启动时默认恢复它）。
    pub fn latest(&self) -> Option<SessionFile> {
        self.list().first().and_then(|m| self.get(&m.id))
    }

    /// 全部会话元信息：置顶优先，其余按更新时间新→旧（归档项一并返回，由前端过滤）。
    /// 标题兜底（2026-10-02 真机实测「@对话列表全显示新会话」）：占位/空标题时回落
    /// 首条用户消息前 10 字（与落盘口径同一个函数）；一条消息都没有的显示「新对话」。
    /// 2026-11-06 起标题在首条消息落盘时就写进会话文件，这里只剩存量旧会话的兜底。
    /// 占位判定要同时认「新对话」（2026-10-30 起的统一叫法）与「新会话」（旧占位，
    /// 盘上存量会话还是这个值）——只认新值的话，老空会话会带着旧名一路显示下去。
    pub fn list(&self) -> Vec<SessionMeta> {
        let mut metas: Vec<SessionMeta> = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return metas;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            let Ok(f) = serde_json::from_str::<SessionFile>(&text) else { continue };
            let title = if !f.title.trim().is_empty() && !is_placeholder_title(&f.title) {
                f.title
            } else {
                first_user_title(&f.messages)
            };
            metas.push(SessionMeta {
                id: f.id,
                title,
                created_at: f.created_at,
                updated_at: f.updated_at,
                count: f.messages.len(),
                workspace: f.workspace,
                pinned: f.pinned,
            });
        }
        metas.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(b.updated_at.cmp(&a.updated_at)));
        metas
    }

    /// 删除一条会话（其余不受影响）；不存在也返回 Ok。
    pub fn delete(&self, id: &str) -> anyhow::Result<()> {
        if !safe_id(id) {
            anyhow::bail!("非法 session id");
        }
        // 产物账本跟着会话走：会话文件删了，归属记录也不留（best-effort）
        if let Some(logs) = self.dir.parent() {
            crate::artifacts::clear_in(logs, id);
        }
        // 压缩完整记录跟着会话走（best-effort）
        let tdir = self.transcripts_dir();
        if let Ok(rd) = std::fs::read_dir(&tdir) {
            let prefix = format!("{id}-");
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().starts_with(&prefix) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        match std::fs::remove_file(self.path(id)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("删除会话失败: {}", self.path(id).display())),
        }
    }

    /// 改会话标题（TITLE 角色 / 侧边栏重命名）。
    /// 会话不存在时**报错**（2026-10-20 修）：早先静默 Ok，`POST /api/sessions/meta`
    /// 传空 id/写错 id 也回 `{"ok":true}`，前端以为改名成功。口径对齐 set_flags。
    /// 不动 updated_at：list 与「上次做到哪」都按它排序，改名不该把老会话顶到最前（与 set_flags 口径一致，F7）。
    pub fn rename(&self, id: &str, title: &str) -> anyhow::Result<()> {
        if !safe_id(id) {
            anyhow::bail!("非法 session id: {id}");
        }
        let lock = self.session_lock(id);
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        let Some(mut f) = self.get(id) else {
            anyhow::bail!("会话不存在: {id}");
        };
        f.title = title.chars().take(60).collect();
        self.write(&f)
    }

    /// DSH 式侧边栏：置顶标记（其余字段原样保留；不动 updated_at）。
    /// 归档已移除（2026-10-31）：只留删除，字段仅旧文件兼容。
    pub fn set_flags(&self, id: &str, pinned: Option<bool>) -> anyhow::Result<()> {
        let lock = self.session_lock(id);
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        let Some(mut f) = self.get(id) else {
            anyhow::bail!("会话不存在: {id}");
        };
        if let Some(p) = pinned {
            f.pinned = p;
        }
        self.write(&f)
    }

    /// 会话截断（2026-10-26 消息操作：回滚/编辑重发/删除共用）。
    /// `user_index` = 文件里第 N 条 user 消息（0 基）；mode：
    /// - "rollback"：删第 N 条 user 及其后全部（回到它之前）
    /// - "delete_turn"：删第 N 条 user + 紧随的 assistant/tool 串（这一问答对）
    /// - "delete_user"：只删第 N 条 user（单条删除不连带
    ///   回答；留下来的 assistant 由适配器按句首孤儿规则剥掉）
    ///
    /// 返回截断后的消息总数；找不到第 N 条 user 或会话不存在时报错。
    /// 锁内读-改-写：与 Snapshot 落盘/Snapshot 事件互斥（server 层已先挡掉在跑的会话）。
    pub fn truncate(&self, id: &str, user_index: usize, mode: &str) -> anyhow::Result<usize> {
        let lock = self.session_lock(id);
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        let Some(mut f) = self.get(id) else {
            anyhow::bail!("会话不存在: {id}");
        };
        let mut start: Option<usize> = None;
        let mut count = 0usize;
        for (i, m) in f.messages.iter().enumerate() {
            if m.get("role").and_then(serde_json::Value::as_str) == Some("user") {
                if count == user_index {
                    start = Some(i);
                    break;
                }
                count += 1;
            }
        }
        let Some(start) = start else {
            anyhow::bail!("找不到第 {user_index} 条用户消息");
        };
        let end = if mode == "rollback" {
            f.messages.len()
        } else if mode == "delete_user" {
            // 单条删除：只拿掉这条 user，后面的 assistant/tool 原样保留
            start + 1
        } else {
            let mut e = start + 1;
            while e < f.messages.len()
                && matches!(f.messages[e].get("role").and_then(serde_json::Value::as_str), Some("assistant") | Some("tool"))
            {
                e += 1;
            }
            e
        };
        f.messages.drain(start..end);
        let n = f.messages.len();
        self.write(&f)?;
        Ok(n)
    }

    /// 按 (role, content) 精确删除消息（2026-10-27 多选删除：勾了谁就删谁，
    /// 不连带问答对里的另一方）。targets 按前端视图顺序给，服务端在文件里
    /// **按序**匹配同序的未消耗目标——重复文本也能各自对上。
    /// 连带规则仅一条：被删 assistant 的 tool 结果消息必须跟着删
    /// （tool_call_id 失去主消息=下轮请求带悬空 tool_result，协议直接 400），
    /// 这不是“连带彼此”，是消息结构完整性。返回删除条数；有目标没匹配上则
    /// 整单不删（原子，不做半删）。
    pub fn delete_messages(&self, id: &str, targets: &[(String, String)]) -> anyhow::Result<usize> {
        let lock = self.session_lock(id);
        let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
        let Some(mut f) = self.get(id) else {
            anyhow::bail!("会话不存在: {id}");
        };
        let mut ti = 0usize;
        let mut doomed: Vec<usize> = Vec::new();
        for (i, m) in f.messages.iter().enumerate() {
            if ti >= targets.len() {
                break;
            }
            let role = m.get("role").and_then(serde_json::Value::as_str).unwrap_or("");
            let content = match m.get("content") {
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(serde_json::Value::Null) | None => String::new(),
                Some(v) => serde_json::to_string(v).unwrap_or_default(),
            };
            if role == targets[ti].0 && content == targets[ti].1 {
                doomed.push(i);
                ti += 1;
            }
        }
        if ti < targets.len() {
            anyhow::bail!("有 {} 条消息没匹配上（含附件/多模态的消息暂不支持精确删除）", targets.len() - ti);
        }
        // 被删 assistant 的 tool_calls id → 其 tool 结果消息一并删（消息结构完整性）
        let mut call_ids: Vec<String> = Vec::new();
        for &i in &doomed {
            if f.messages[i].get("role").and_then(serde_json::Value::as_str) != Some("assistant") {
                continue;
            }
            if let Some(calls) = f.messages[i].get("tool_calls").and_then(serde_json::Value::as_array) {
                for c in calls {
                    if let Some(cid) = c.get("id").and_then(serde_json::Value::as_str) {
                        call_ids.push(cid.to_string());
                    }
                }
            }
        }
        if !call_ids.is_empty() {
            for (i, m) in f.messages.iter().enumerate() {
                if doomed.contains(&i) {
                    continue;
                }
                if m.get("role").and_then(serde_json::Value::as_str) == Some("tool")
                    && m.get("tool_call_id")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|cid| call_ids.iter().any(|x| x == cid))
                {
                    doomed.push(i);
                }
            }
        }
        doomed.sort_unstable();
        let n = doomed.len();
        for &i in doomed.iter().rev() {
            f.messages.remove(i);
        }
        self.write(&f)?;
        Ok(n)
    }

    /// 同项目（workspace 相同）里最近更新的一条已落盘会话，排除 current_id。
    /// 新会话开场「上次做到哪」的来源；没有则返回 None。
    pub fn previous_in_workspace(&self, workspace: &str, current_id: &str) -> Option<SessionFile> {
        if workspace.trim().is_empty() {
            return None;
        }
        let mut best: Option<SessionFile> = None;
        for m in self.list() {
            if m.id == current_id || m.workspace != workspace {
                continue;
            }
            let Some(f) = self.get(&m.id) else { continue };
            if best.as_ref().map(|b| f.updated_at > b.updated_at).unwrap_or(true) {
                best = Some(f);
            }
        }
        best
    }

    /// 新会话开场的「项目现状」摘要：上一条同项目会话的标题/滚动摘要 + 工作区未提交改动量。
    /// 没有任何可写事实时返回 None（不注入，避免占上下文）。
    /// `changes` = (文件数, 新增行, 删除行)，相对影子仓库最近检查点。
    pub fn project_brief(&self, current_id: &str, changes: Option<(usize, u64, u64)>) -> Option<String> {
        let ws = crate::tools::workspace_root();
        let ws_path = ws.display().to_string();
        let ws_name = ws.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let mut lines: Vec<String> = Vec::new();
        if let Some(p) = self.previous_in_workspace(&ws_path, current_id) {
            lines.push(format!("- 上一条会话：「{}」（{}）", p.title, fmt_ts(&p.updated_at)));
            if let Some(sm) = p.summary.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                lines.push(format!("- 上次进展：{}", sm.chars().take(400).collect::<String>()));
            }
        }
        if let Some((files, add, del)) = changes
            && files > 0 {
                lines.push(format!("- 工作区已有 {files} 个文件改动（+{add}/-{del}，相对最近检查点），内容都在磁盘上"));
            }
        if lines.is_empty() {
            return None;
        }
        Some(format!(
            "\n\n【项目现状 · 新对话接手】\n项目目录：{ws_path}（{ws_name}）\n{}\n这是同一项目里新开的对话，之前的改动已落盘。动手前先确认现状，不要重做上一条会话已完成的工作。\n",
            lines.join("\n")
        ))
    }

    /// DSH 式侧边栏「分叉会话」：复制消息/摘要/工作区为新会话，标题加「（分叉）」。
    /// 返回新会话 id；分叉不带走置顶/归档状态。
    pub fn fork(&self, id: &str) -> anyhow::Result<String> {
        let Some(f) = self.get(id) else {
            anyhow::bail!("会话不存在: {id}");
        };
        let new_id = self.new_id();
        let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let title = format!("{}（分叉）", f.title).chars().take(60).collect();
        let file = SessionFile {
            id: new_id.clone(),
            title,
            created_at: now.clone(),
            updated_at: now,
            messages: f.messages,
            summary: f.summary,
            workspace: f.workspace,
            pinned: false,
            archived: false,
            // 分叉不带用量快照：台账记录归属原会话 id，分叉从零攒自己的账
            usage: None,
            // 压缩记录跟着走：消息连同 id 一起复制，边界锚点在分叉里仍然有效
            compact: f.compact,
        };
        self.write(&file)?;
        Ok(new_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    mod tempdir {
        use std::path::{Path, PathBuf};
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        pub struct TempDir(PathBuf);
        impl TempDir {
            pub fn new(tag: &str) -> Self {
                let n = SEQ.fetch_add(1, Ordering::SeqCst);
                let p = std::env::temp_dir().join(format!("agent-p61-{tag}-{}-{}", std::process::id(), n));
                let _ = std::fs::remove_dir_all(&p);
                std::fs::create_dir_all(&p).unwrap();
                TempDir(p)
            }
            pub fn path(&self) -> &Path {
                &self.0
            }
        }
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    fn mgr(tag: &str) -> (tempdir::TempDir, SessionManager) {
        let d = tempdir::TempDir::new(tag);
        let m = SessionManager::new(d.path());
        (d, m)
    }

    #[test]
    fn delete_user_keeps_answer_for_precise_single_delete() {
        // 2026-10-27 真机实测：点一条消息删除，结果一问一答都没了。
        // delete_user 只拿掉点中的那条 user，回答留在原地（句首孤儿由适配器剥）。
        let _ = crate::observability::init(
            &std::env::temp_dir().join("agent-test-logs"),
            "sessions.jsonl",
            format!("session-test-{}", uuid::Uuid::new_v4()),
        );
        let (_d, m) = mgr("delete-user");
        let a = m.new_id();
        m.save(
            &a,
            &[
                json!({"role":"user","content":"第一问"}),
                json!({"role":"assistant","content":"第一答"}),
                json!({"role":"user","content":"第二问"}),
                json!({"role":"assistant","content":"第二答"}),
            ],
            None,
        )
        .unwrap();

        // 删第一问：只少这一条，第一答还在
        assert_eq!(m.truncate(&a, 0, "delete_user").unwrap(), 3);
        let f = m.get(&a).unwrap();
        let texts: Vec<&str> = f
            .messages
            .iter()
            .map(|x| x.get("content").and_then(Value::as_str).unwrap_or(""))
            .collect();
        assert_eq!(texts, vec!["第一答", "第二问", "第二答"]);

        // 删第二问：第二答同样保留（删完第一问后，第二问是仅剩的 user，序数变 0）
        assert_eq!(m.truncate(&a, 0, "delete_user").unwrap(), 2);
        let f = m.get(&a).unwrap();
        let texts: Vec<&str> = f
            .messages
            .iter()
            .map(|x| x.get("content").and_then(Value::as_str).unwrap_or(""))
            .collect();
        assert_eq!(texts, vec!["第一答", "第二答"]);
    }

    #[test]
    fn usage_snapshot_survives_save_and_restart() {
        let _ = crate::observability::init(
            &std::env::temp_dir().join("agent-test-logs"),
            "sessions.jsonl",
            format!("session-test-{}", uuid::Uuid::new_v4()),
        );
        let (_d, m) = mgr("usage-snap");
        let a = m.new_id();
        m.save(&a, &[json!({"role":"user","content":"你好"})], None).unwrap();

        // 模拟一轮对话的用量事件：ApiUsage 落快照（projected=圆环分子，累计并入）
        m.update_usage(
            &a,
            SessionUsage {
                projected: 4750,
                pressure: Some(4750),
                anchor_est: 5100,
                window: 1_000_000,
                messages: 10,
                tools: 0,
                system: 1467,
                other: 0,
                tools_schema: 3273,
                api_prompt: 4750,
                api_cached: 0,
            },
        )
        .unwrap();

        // 后续消息侧 save（快照保存）不得抹掉用量快照
        m.save(
            &a,
            &[json!({"role":"user","content":"你好"}), json!({"role":"assistant","content":"你好！有什么可以帮你的？"})],
            None,
        )
        .unwrap();
        let f = m.get(&a).expect("会话应在");
        let u = f.usage.expect("save 后用量快照应保留");
        assert_eq!(u.projected, 4750);
        assert_eq!(u.pressure, Some(4750));
        assert_eq!(u.api_prompt, 4750);
        assert_eq!(u.tools_schema, 3273);
        assert_eq!(u.anchor(), Some((4750, 5100)), "跨轮锚点 = (基线占用, 配对估价)");

        // 再次事件：累计叠加（跨请求/跨重启延续）；圆环跟随最新一次锚点
        m.update_usage(
            &a,
            SessionUsage { projected: 3972, api_prompt: 4750 + 3972, api_cached: 128, ..u.clone() },
        )
        .unwrap();
        let f2 = m.get(&a).unwrap();
        let u2 = f2.usage.unwrap();
        assert_eq!(u2.api_prompt, 8722);
        assert_eq!(u2.api_cached, 128);
        assert_eq!(u2.projected, 3972, "圆环跟随最新一次占用");

        // 不存在的会话：明确报错而不是悄悄写盘
        assert!(m.update_usage("no-such-session", SessionUsage::default()).is_err());
    }

    #[test]
    fn legacy_usage_snapshot_still_loads() {
        // 旧会话文件里是 tokens/budget 两个字段（改造前口径），别名收下不许丢数据
        let old: SessionUsage = serde_json::from_str(
            r#"{"tokens":4750,"budget":128000,"messages":10,"tools":0,"system":1467,
                "other":0,"tools_schema":3273,"api_prompt":4750,"api_cached":128}"#,
        )
        .expect("旧格式必须能读");
        assert_eq!(old.projected, 4750, "旧 tokens 语义最接近 projected");
        assert_eq!(old.window, 128000, "旧 budget → window");
        assert_eq!(old.pressure, None, "旧文件没有实报锚点");
        // 新写出去的是新字段名
        let text = serde_json::to_string(&old).unwrap();
        assert!(text.contains("\"projected\"") && text.contains("\"window\""));
        assert!(!text.contains("\"tokens\""), "不再吐出旧字段名");
    }

    #[test]
    fn multi_session_lifecycle() {
        let _ = crate::observability::init(
            &std::env::temp_dir().join("agent-test-logs"),
            "sessions.jsonl",
            format!("session-test-{}", uuid::Uuid::new_v4()),
        );
        let (_d, m) = mgr("multi");

        // 空目录：无会话
        assert!(m.list().is_empty());
        assert!(m.latest().is_none());

        // 两条会话互不影响；标题取首条用户消息
        let a = m.new_id();
        let b = m.new_id();
        assert_ne!(a, b);
        m.save(&a, &[json!({"role":"user","content":"帮我修编译错误，这是会话A的开头"})], None).unwrap();
        m.save(&b, &[json!({"role":"user","content":"第二条会话"})], None).unwrap();
        let list = m.list();
        assert_eq!(list.len(), 2);
        assert!(list.iter().any(|s| s.id == a && s.title.starts_with("帮我修编译错误")), "标题取首句前 10 字");
        assert!(list.iter().any(|s| s.id == b));

        // 更新 A：标题与 created_at 保留，消息数变化；摘要随存
        m.save(
            &a,
            &[
                json!({"role":"user","content":"帮我修编译错误，这是会话A的开头"}),
                json!({"role":"assistant","content":"好的"}),
            ],
            Some("任务目标：修编译错误；已定位到 main.rs"),
        )
        .unwrap();
        let fa = m.get(&a).unwrap();
        assert_eq!(fa.messages.len(), 2);
        assert_eq!(fa.title, "帮我修编译错误，这是", "标题 = 首句前 10 字且后续保存不改写");
        assert!(fa.summary.as_deref().unwrap_or("").contains("main.rs"), " 摘要应持久化");
        // summary 传 None 时保留旧值
        m.save(&a, &fa.messages, None).unwrap();
        assert!(m.get(&a).unwrap().summary.is_some());

        // 删除 A，B 仍在
        m.delete(&a).unwrap();
        assert!(m.get(&a).is_none());
        assert_eq!(m.list().len(), 1);
        // 删除幂等
        m.delete(&a).unwrap();
    }

    /// 点「新对话」立即落盘并进列表；同工作区复用空会话，有消息的不算空。
    #[test]
    fn create_empty_session_is_listed_and_reused() {
        let (_d, m) = mgr("empty-new");
        let ws = "C:\\ws\\demo";
        let f = m.create_empty(ws, PLACEHOLDER_TITLE).unwrap();
        assert!(f.messages.is_empty());
        assert_eq!(f.workspace, ws);
        let list = m.list();
        assert_eq!(list.len(), 1, "空会话也要进列表（侧栏立即可见）");
        assert_eq!(list[0].count, 0);
        assert_eq!(list[0].title, "新对话", "空会话回落首条消息兜底（无消息=新对话）");

        // 同工作区再次新建 → 复用同一条，连点不堆空会话
        let f2 = m.create_empty(ws, PLACEHOLDER_TITLE).unwrap();
        assert_eq!(f2.id, f.id);
        assert_eq!(m.list().len(), 1);

        // 换工作区 → 另建一条
        let f3 = m.create_empty("C:\\ws\\other", PLACEHOLDER_TITLE).unwrap();
        assert_ne!(f3.id, f.id);
        assert_eq!(m.list().len(), 2);

        // 有消息的会话不再被当作可复用空会话
        m.save(&f.id, &[json!({"role":"user","content":"开始干活"})], None).unwrap();
        assert_eq!(m.get(&f.id).unwrap().title, "开始干活", "首条消息到了标题要落盘（打开会话的顶栏也靠它）");
        let f4 = m.create_empty(ws, PLACEHOLDER_TITLE).unwrap();
        assert_ne!(f4.id, f.id, "有消息的会话不该被复用");
    }

    /// 回滚成空的旧对话不能被「新建对话」复用——
    /// 复用会让标题还是一开始的，圆环被清零但会话根本没换。
    #[test]
    fn create_empty_does_not_reuse_rolled_back_session() {
        let (_d, m) = mgr("empty-rollback");
        let ws = "C:\\ws\\rollback";
        let f = m.create_empty(ws, PLACEHOLDER_TITLE).unwrap();
        m.save(&f.id, &[json!({"role":"user","content":"你好"})], None).unwrap();
        let mut usage = SessionUsage::default();
        usage.apply_context(None, 13_000, 150_000, &crate::context::Breakdown::default());
        m.update_usage(&f.id, usage).unwrap();
        // 回滚到空：标题与用量快照都留在会话文件里
        m.save(&f.id, &[], None).unwrap();
        assert_eq!(m.get(&f.id).unwrap().title, "你好");
        let f2 = m.create_empty(ws, PLACEHOLDER_TITLE).unwrap();
        assert_ne!(f2.id, f.id, "回滚空会话有真实标题/用量，不是可复用的「新对话」");
        assert_eq!(f2.title, PLACEHOLDER_TITLE);
    }

    /// 静默压缩（2026-11-07）：apply_compaction 写记录+标记行+summary；锚点没了作废；
    /// 快照覆写不丢记录，标记行被冲掉时按记录补回。
    #[test]
    fn compaction_apply_and_persistence() {
        let _ = crate::observability::init(
            &std::env::temp_dir().join("agent-test-logs"),
            "sessions.jsonl",
            format!("session-test-{}", uuid::Uuid::new_v4()),
        );
        let (_d, m) = mgr("compact-apply");
        let f = m.create_empty("C:\\ws\\compact", PLACEHOLDER_TITLE).unwrap();
        m.save(
            &f.id,
            &[
                json!({"role":"user","content":"第一个问题"}),
                json!({"role":"assistant","content":"第一个回答"}),
                json!({"role":"user","content":"第二个问题"}),
                json!({"role":"assistant","content":"第二个回答"}),
            ],
            None,
        )
        .unwrap();
        let saved = m.get(&f.id).unwrap();
        let ids: Vec<String> = saved.messages.iter().map(|x| x.get("id").and_then(Value::as_str).unwrap_or("").to_string()).collect();
        assert!(ids.iter().all(|s| !s.is_empty()), "save 必须补齐消息 id");
        let (tail_start, after) = (&ids[2], &ids[3]);

        let record = crate::compact::CompactRecord {
            summary: "任务目标：压缩测试".into(),
            summarized_last_id: ids[1].clone(),
            tail_start_id: tail_start.clone(),
            after_id: after.clone(),
            pre_tokens: 900_000,
            post_tokens: 40_000,
            transcript_path: "C:\\logs\\transcripts\\x-001.md".into(),
            created_at: "2026-11-07T00:00:00Z".into(),
        };
        let marker = crate::compact::marker_message(&crate::compact::done_marker_text(40_000));
        assert!(m.apply_compaction(&f.id, record, marker).unwrap(), "首次应用应成功");

        let got = m.get(&f.id).unwrap();
        assert_eq!(got.compact.as_ref().unwrap().post_tokens, 40_000);
        assert_eq!(got.summary.as_deref(), Some("任务目标：压缩测试"), "summary 同步给新会话 brief");
        // 标记行插在触发轮最后一条之后
        let marker_idx = got.messages.iter().position(|x| x.get("kind").and_then(Value::as_str) == Some("compact_marker")).unwrap();
        assert_eq!(marker_idx, 4, "标记行在最后一条消息后面");
        assert!(got.messages[marker_idx]["content"].as_str().unwrap().starts_with("————压缩后上下文 40k————"));

        // 快照覆写保护：下一轮整卷重写（不含标记行）→ 记录保留、标记补回
        let mut next_round = got.messages.iter().filter(|x| x.get("kind").and_then(Value::as_str) != Some("compact_marker")).cloned().collect::<Vec<_>>();
        next_round.push(json!({"role":"user","content":"第三个问题","id": ids[0].clone() + "-n"}));
        m.save(&f.id, &next_round, None).unwrap();
        let after_save = m.get(&f.id).unwrap();
        assert!(after_save.compact.is_some(), "记录不能被快照冲掉");
        assert!(after_save.messages.iter().any(|x| x.get("kind").and_then(Value::as_str) == Some("compact_marker")), "标记行要被补回");

        // 回滚把锚点消息删掉 → 应用作废
        let trimmed: Vec<Value> = after_save.messages[..2].to_vec();
        m.save(&f.id, &trimmed, None).unwrap();
        let record2 = crate::compact::CompactRecord {
            summary: "s".into(), summarized_last_id: ids[1].clone(), tail_start_id: tail_start.clone(),
            after_id: after.clone(), pre_tokens: 1, post_tokens: 1, transcript_path: "t".into(), created_at: "x".into(),
        };
        assert!(!m.apply_compaction(&f.id, record2, crate::compact::marker_message("————压缩后上下文 1k————")).unwrap(), "锚点不在 = 作废");
    }

    /// 二次压缩且完成文案取整相同（两次都报 17k）：标记行必须挪到**新**边界，
    /// 不能按内容去重把新标记吞掉（真机 2026-11-07：底部「正在压缩」行被重建冲掉、
    /// 刷新后只在历史中部看到上一次的旧行）。
    #[test]
    fn second_compaction_moves_marker_even_with_same_text() {
        let (_d, m) = mgr("compact-remark-same-text");
        let f = m.create_empty("C:\\ws\\compact2", PLACEHOLDER_TITLE).unwrap();
        let ids: Vec<String> = (0..4).map(|i| format!("{}-{}", f.id, i)).collect();
        let msgs = vec![
            json!({"role":"user","content":"q1","id": ids[0]}),
            json!({"role":"assistant","content":"a1","id": ids[1]}),
            json!({"role":"user","content":"q2","id": ids[2]}),
            json!({"role":"assistant","content":"a2","id": ids[3]}),
        ];
        m.save(&f.id, &msgs, None).unwrap();
        let rec = |after: &str, tail: &str| crate::compact::CompactRecord {
            summary: "s".into(),
            summarized_last_id: ids[1].clone(),
            tail_start_id: tail.into(),
            after_id: after.into(),
            pre_tokens: 1,
            post_tokens: 17_000,
            transcript_path: "t".into(),
            created_at: "x".into(),
        };
        // 第一次：边界在 a2（idx 3）之后
        assert!(m.apply_compaction(&f.id, rec(&ids[3], &ids[2]), crate::compact::marker_message(&crate::compact::done_marker_text(17_000))).unwrap());
        let got = m.get(&f.id).unwrap();
        assert_eq!(got.messages.last().unwrap()["kind"], json!("compact_marker"), "首次标记在末尾");
        // 第二次：同文标记（17k→17k），新边界在尾部新增的 a4 之后
        let mut more = got.messages.iter().filter(|x| x.get("kind").and_then(Value::as_str) != Some("compact_marker")).cloned().collect::<Vec<_>>();
        more.push(json!({"role":"user","content":"q3","id":"q3"}));
        more.push(json!({"role":"assistant","content":"a3","id":"a3"}));
        m.save(&f.id, &more, None).unwrap();
        assert!(m.apply_compaction(&f.id, rec("a3", "q3"), crate::compact::marker_message(&crate::compact::done_marker_text(17_000))).unwrap());
        let got2 = m.get(&f.id).unwrap();
        let markers: Vec<usize> = got2
            .messages
            .iter()
            .enumerate()
            .filter(|(_, x)| x.get("kind").and_then(Value::as_str) == Some("compact_marker"))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(markers, vec![got2.messages.len() - 1], "旧标记清掉、新标记只在末尾（新边界）");
    }

    /// 标题 = 首条用户消息前 10 字：换行折成空格，长消息截断；媒体投影块剥掉。
    #[test]
    fn first_message_becomes_persisted_title() {
        let (_d, m) = mgr("title-prefix");
        let f = m.create_empty("C:\\ws\\title", PLACEHOLDER_TITLE).unwrap();
        let long = format!("第一行\n第二行 {}", "很长".repeat(40));
        m.save(&f.id, &[json!({"role":"user","content":long})], None).unwrap();
        let title = m.get(&f.id).unwrap().title;
        assert!(title.starts_with("第一行 第二行"), "换行折成单空格: {title}");
        assert_eq!(title.chars().count(), 10, "按字符截到 10: {title}");
        assert!(!title.contains('\n'));

        // 媒体投影块剥掉后再取正文
        let f2 = m.create_empty("C:\\ws\\title2", PLACEHOLDER_TITLE).unwrap();
        m.save(&f2.id, &[json!({"role":"user","content":"[Audio understanding] 帮我听这段录音"})], None).unwrap();
        assert_eq!(m.get(&f2.id).unwrap().title, "帮我听这段录音");
    }

    /// 旧占位「新会话」的存量会话仍按占位处理，显示回落首条消息 / 「新对话」。
    /// 只认新占位的话，改名之前建的老空会话会顶着旧名一路显示下去。
    #[test]
    fn legacy_placeholder_title_still_falls_back() {
        let (_d, m) = mgr("legacy-placeholder");
        let ws = "C:\\ws\\demo";
        let f = m.create_empty(ws, "新会话").unwrap();
        assert_eq!(m.list()[0].title, "新对话", "旧占位（无消息）= 新对话");
        m.save(&f.id, &[json!({"role":"user","content":"真实的第一句"})], None).unwrap();
        assert_eq!(m.list()[0].title, "真实的第一句", "旧占位（有消息）回落首条消息");
    }

    #[test]
    fn rejects_bad_ids_and_missing() {
        let _ = crate::observability::init(
            &std::env::temp_dir().join("agent-test-logs"),
            "sessions.jsonl",
            format!("session-test-{}", uuid::Uuid::new_v4()),
        );
        let (_d, m) = mgr("badid");
        assert!(m.save("../evil", &[], None).is_err());
        assert!(m.get("../evil").is_none());
        assert!(m.delete("../evil").is_err());
        assert!(m.get("不存在的会话").is_none());
        // 改名/标记不存在的会话必须报错，不许静默成功（2026-10-20 修：
        // POST /api/sessions/meta 传空 id 曾回 {"ok":true}）
        assert!(m.rename("", "标题").is_err(), "空 id 改名必须报错");
        assert!(m.rename("ghost", "标题").is_err(), "不存在的会话改名必须报错");
        assert!(m.rename("../evil", "标题").is_err(), "非法 id 改名必须报错");
    }

    #[test]
    fn pin_archive_fork_lifecycle() {
        // DSH 式侧边栏三操作：置顶排序、归档标记、分叉复制
        let _ = crate::observability::init(
            &std::env::temp_dir().join("agent-test-logs"),
            "sessions.jsonl",
            format!("session-test-{}", uuid::Uuid::new_v4()),
        );
        let (_d, m) = mgr("pinfork");
        let a = m.new_id();
        m.save(&a, &[json!({"role":"user","content":"原始会话"})], None).unwrap();
        // 新会话记录当前工作区
        assert!(!m.get(&a).unwrap().workspace.is_empty(), "新会话应记录创建时的工作区");

        // 置顶 → 列表置前；save 续写后置顶保留
        m.set_flags(&a, Some(true)).unwrap();
        assert!(m.list()[0].pinned);
        m.save(&a, &[json!({"role":"user","content":"原始会话"}), json!({"role":"assistant","content":"好"})], None).unwrap();
        assert!(m.get(&a).unwrap().pinned, "save 不得丢置顶标记");

        // 取消置顶
        m.set_flags(&a, Some(false)).unwrap();
        assert!(!m.get(&a).unwrap().pinned);

        // 分叉：消息/摘要/工作区复制，标题加（分叉），新 id，无置顶
        m.set_flags(&a, Some(true)).unwrap();
        m.rename(&a, "原始标题").unwrap();
        let forked = m.fork(&a).unwrap();
        let ff = m.get(&forked).unwrap();
        assert_eq!(ff.title, "原始标题（分叉）");
        assert_eq!(ff.messages.len(), m.get(&a).unwrap().messages.len());
        assert_eq!(ff.workspace, m.get(&a).unwrap().workspace);
        assert!(!ff.pinned && !ff.archived, "分叉不带走置顶；archived 恒为 false");
        assert_ne!(forked, a);

        // 不存在的会话
        assert!(m.set_flags("ghost", Some(true)).is_err());
        assert!(m.fork("ghost").is_err());
    }

    #[test]
    fn save_refuses_to_clobber_unreadable_file() {
        // 2026-10-08：文件存在但读不出（损坏/被占）时 save 必须拒绝覆写——
        // 早先按「新会话」整份重写，置顶/归档/标题/创建时间/用量快照被静默抹掉。
        let _ = crate::observability::init(
            &std::env::temp_dir().join("agent-test-logs"),
            "sessions.jsonl",
            format!("session-test-{}", uuid::Uuid::new_v4()),
        );
        let (_d, m) = mgr("clobber");
        let a = m.new_id();
        m.save(&a, &[json!({"role":"user","content":"原始会话"})], None).unwrap();
        m.set_flags(&a, Some(true)).unwrap();
        m.rename(&a, "重要标题").unwrap();

        // 损坏盘上文件（合法 id、合法路径，内容不是 JSON）
        let path = _d.path().join("sessions").join(format!("{a}.json"));
        std::fs::write(&path, "{ 这不是合法 JSON").unwrap();

        // save 必须报错且不碰盘上现场
        let err = m.save(&a, &[json!({"role":"user","content":"新一轮"})], None).unwrap_err();
        assert!(err.to_string().contains("拒绝覆写"), "错误要说人话: {err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ 这不是合法 JSON", "损坏现场必须原样保留");
        // 恢复成合法内容后 save 照常工作（不是永久卡死）
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "id": a,
                "title": "重要标题",
                "created_at": "2026-10-08T00:00:00.000Z",
                "updated_at": "2026-10-08T00:00:00.000Z",
                "messages": [{"role":"user","content":"原始会话"}],
                "workspace": "C:\\ws",
                "pinned": true,
                "archived": true,
            }))
            .unwrap(),
        )
        .unwrap();
        m.save(&a, &[json!({"role":"user","content":"原始会话"})], None).unwrap();
        let f = m.get(&a).unwrap();
        assert!(f.pinned, "置顶必须还在");
        assert!(!f.archived, "废弃的归档标记随写入归零（机制已移除，字段仅兼容旧文件）");
        assert_eq!(f.title, "重要标题");
    }

    #[test]
    fn project_brief_carries_prior_session() {
        let _ = crate::observability::init(
            &std::env::temp_dir().join("agent-test-logs"),
            "sessions.jsonl",
            format!("session-test-{}", uuid::Uuid::new_v4()),
        );
        let (_d, m) = mgr("brief");
        let ws = crate::tools::workspace_root().display().to_string();

        // 项目里还没有别的会话：没有可写事实 → 不注入
        let fresh = m.new_id();
        assert!(m.project_brief(&fresh, None).is_none(), "空项目不该注入现状");

        // 上一条会话有摘要 + 工作区有未提交改动 → 摘要里两者都要出现
        let prev = m.new_id();
        m.save(&prev, &[json!({"role":"user","content":"把登录接口改成 JWT"})], None).unwrap();
        m.save(
            &prev,
            &[
                json!({"role":"user","content":"把登录接口改成 JWT"}),
                json!({"role":"assistant","content":"已改完"}),
            ],
            Some("任务目标：登录改 JWT；已完成签发与校验，待补刷新逻辑"),
        )
        .unwrap();

        let brief = m.project_brief(&fresh, Some((2, 15, 4))).expect("应生成项目现状");
        assert!(brief.contains("把登录接口改成"), "带上一条会话标题（首句前 10 字）: {brief}");
        assert!(brief.contains("待补刷新逻辑"), "带上一条会话摘要: {brief}");
        assert!(brief.contains("2 个文件改动"), "带未提交改动量: {brief}");
        assert!(brief.contains(&ws), "带项目目录: {brief}");

        // 排除自己：当前会话不该被当成「上一条」
        assert!(m.previous_in_workspace(&ws, &prev).is_none());

        // 别的项目不算同项目
        assert!(m.previous_in_workspace("C:\\somewhere\\else", &fresh).is_none());
        assert!(m.project_brief(&fresh, None).is_some(), "换基线仍有上一条会话可写");
    }

    #[test]
    fn rename_keeps_updated_at() {
        // F7：改名不许动 updated_at——list 与「上次做到哪」都按它排序，
        // 动了老会话会跳到侧栏最前，且「上次做到哪」可能选错会话
        let _ = crate::observability::init(
            &std::env::temp_dir().join("agent-test-logs"),
            "sessions.jsonl",
            format!("session-test-{}", uuid::Uuid::new_v4()),
        );
        let (_d, m) = mgr("rename");
        let a = m.new_id();
        m.save(&a, &[json!({"role":"user","content":"旧标题的开头"})], None).unwrap();
        // 人为把 updated_at 压老，模拟一个很久前的会话
        let mut f = m.get(&a).unwrap();
        f.updated_at = "2020-01-01T00:00:00.000Z".to_string();
        m.write(&f).unwrap();

        m.rename(&a, "新标题").unwrap();

        let after = m.get(&a).unwrap();
        assert_eq!(after.title, "新标题", "标题应改");
        assert_eq!(after.updated_at, "2020-01-01T00:00:00.000Z", "updated_at 不该被动");
        // 排序佐证：另建一个新会话，改名的老会话仍排在它后面
        let b = m.new_id();
        m.save(&b, &[json!({"role":"user","content":"更新的会话"})], None).unwrap();
        let order: Vec<String> = m.list().into_iter().map(|x| x.id).collect();
        let pos_b = order.iter().position(|id| id == &b).unwrap();
        let pos_a = order.iter().position(|id| id == &a).unwrap();
        assert!(pos_a > pos_b, "改名后的老会话不该被顶到前面: {order:?}");
    }
}

#[cfg(test)]
mod concurrency_tests {
    use super::*;
    use serde_json::json;

    /// 2026-10-08 修：按 id 写锁的语义——同 id 必须拿到同一把锁（互斥），
    /// 不同 id 互不影响（否则并发扬不到，反而把所有会话写串行化）。
    #[test]
    fn session_lock_semantics() {
        let dir = std::env::temp_dir().join(format!("agent-sess-locksem-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let m = SessionManager::new(&dir);
        let a1 = m.session_lock("a");
        let a2 = m.session_lock("a");
        assert!(Arc::ptr_eq(&a1, &a2), "同 id 必须是同一把锁");
        let b = m.session_lock("b");
        assert!(!Arc::ptr_eq(&a1, &b), "不同 id 不同锁");
        let g = a1.lock().unwrap();
        assert!(a2.try_lock().is_err(), "同 id 第二把必须互斥");
        assert!(b.try_lock().is_ok(), "不同 id 不得被堵");
        drop(g);
        assert!(a2.try_lock().is_ok(), "释放后同 id 可再拿");
    }

    /// 并发写不互吃：消息侧 save 与用户置顶 set_flags 交替打同一个会话文件，
    /// 跑完两条线的更新都必须还在（无锁时后写者用陈旧基线整份回写，先写者的
    /// 更新被抹掉——典型症状是「最终回答从会话文件里消失」）。
    #[test]
    fn concurrent_save_and_flags_both_land() {
        let dir = std::env::temp_dir().join(format!("agent-sess-conc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let m = std::sync::Arc::new(SessionManager::new(&dir));
        let id = m.new_id();
        m.save(&id, &[json!({"role": "user", "content": "hi"})], None).unwrap();
        std::thread::scope(|s| {
            let mm = m.clone();
            let i = id.clone();
            s.spawn(move || {
                for n in 0..80 {
                    mm.save(
                        &i,
                        &[
                            json!({"role": "user", "content": format!("m{n}")}),
                            json!({"role": "assistant", "content": format!("a{n}")}),
                        ],
                        None,
                    )
                    .unwrap();
                }
            });
            let mm2 = m.clone();
            let i2 = id.clone();
            s.spawn(move || {
                for n in 0..80 {
                    mm2.set_flags(&i2, Some(n % 2 == 0)).unwrap();
                }
            });
        });
        let f = m.get(&id).expect("会话文件必须在");
        assert_eq!(f.messages.len(), 2, "消息不得被 set_flags 的回写抹掉");
        assert_eq!(f.messages[1]["content"], "a79", "应留下最后一轮 save 的内容");
        // 最后一轮 set_flags 是 n=79（奇数 → false）
        assert!(!f.pinned, "pinned 应是最后一轮 set_flags 写的值");
    }

    /// omitted 只认预算截断：tool 消息与空消息本就不进转写，不能因此报「已省略」。
    /// 旧实现拿「保留数 < 总会话数」判定，前端 @引用 与 memory 工具都会对用户说
    /// 「只保留了最近部分」——一条啥都没丢的会话被说成截断过。
    #[test]
    fn transcript_omitted_only_when_budget_truncates() {
        let f = SessionFile {
            id: "t1".into(),
            title: "带工具调用的会话".into(),
            created_at: "2026-10-01T00:00:00Z".into(),
            updated_at: "2026-10-01T00:00:00Z".into(),
            messages: vec![
                json!({"role":"user","content":"帮我查一下"}),
                json!({"role":"assistant","content":"","tool_calls":[{"id":"c1"}]}),
                json!({"role":"tool","content":"工具返回一大段……"}),
                json!({"role":"assistant","content":"查完了"}),
            ],
            summary: None,
            workspace: String::new(),
            pinned: false,
            archived: false,
            usage: None,
                compact: None,
        };
        // 4 条消息里只有 2 条进转写，但一条都没丢 → 不该报 omitted
        let (kept, omitted) = transcript(&f, 16_000, 2_500);
        assert_eq!(kept.len(), 2, "只取 user/assistant 的非空正文");
        assert!(!omitted, "没丢内容却报了省略: kept={:?}", kept);
        // 预算极小 → 真的丢 → 报 omitted
        let (kept2, omitted2) = transcript(&f, 12, 2_500);
        assert!(omitted2, "预算装不下必须报省略");
        assert!(kept2.len() < kept.len() || kept2.iter().map(|(_, t)| t.len()).sum::<usize>() <= 12);
    }
}