//! 运行中收到的用户消息（排队 / 引导，2026-10-22 分家；「立即发送」2026-11-07 改语义）
//!
//! 同一会话已有一轮在跑时，新来的用户消息**不拒收**，按模式进本会话的队列：
//!
//! - **排队（Queued，默认）**：等这一轮结束再作为下一轮发出去（服务层收尾取走）。
//!   网页端默认走这条：消息先显示成「排队中」，本轮跑完自动发。
//! - **引导（Steer）**：核心循环取走并入本轮对话（模型下一个决策点看到），
//!   原轮**不打断**、不另起一轮——「不阻断对话，但引导模型的回答」。取走点有两个：
//!   每步决策前，以及最终回答落地后（补流式输出期间到达的——决策点只在模型调用
//!   之前，不补取就会漏掉最后窗口、被收尾 drain_all 捡去当 followup 另起一轮，
//!   2026-09-27 真机实测）。
//! - **打断（Interrupt，旧「立即发送」）**：2026-11-07 起 chat 端点**不再产生**这种
//!   条目——「立即发送」改为「停掉当前轮 + 这条消息作为新一轮」（排队保消息 +
//!   [SteerQueue::mark_restart] + 取消原轮，被停那轮的收尾取到标记就按 followup 开新轮）。
//!   枚举与并入路径保留给兼容（旧测试/潜在调用方），当前无生产者。
//!
//! 各种情况都**不**产生并发双轮：同一会话同时仍然只有一轮在跑（会话文件单写者、
//! 取消入口不被顶掉），这条不变量由 [super::cancel::CancelRegistry] 保证。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::cancel::CancelFlag;

/// 运行中消息的模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteerMode {
    /// 排队：本轮结束后作为下一轮发出（服务层收尾 drain_all 取走）
    Queued,
    /// 引导：并入本轮，下一个决策点生效，不打断
    Steer,
    /// 打断（旧「立即发送」并入语义）：置打断信号让等待点让位，消息并入本轮。
    /// 2026-11-07 起 chat 端点不再产生（「立即发送」= 停轮重启，走 Queued +
    /// mark_restart）；枚举保留给兼容路径。
    Interrupt,
}

/// 队列里的一条消息。
#[derive(Debug, Clone)]
pub struct SteerMsg {
    /// 纯文本（前端「已引导」标记、日志、会话展示兜底用）
    pub text: String,
    /// 模型侧的 user content：纯文本是 String；带图片/音频/视频/文档时是内容块数组
    /// （2026-11-03 引导支持附件：媒体流水线在服务层构好，核心循环原样注入）。
    pub content: Value,
    pub mode: SteerMode,
}

impl SteerMsg {
    /// 落进会话/推给前端的 user 消息——**三态标记的唯一出口**。
    /// 引导 = steered（前端渲染「已引导」静默行）；打断 = interrupted（正常气泡）；
    /// 排队 = 什么都不标（它本来就是「本轮结束后作为新消息发出去」的那条，标成
    /// 已引导会在界面上和真正引导过的那条重复——2026-11-07 真机：排队后点「引导」
    /// 出现两条「已引导」同文，一条还真的被当成新一轮发了出去）。
    pub fn to_message(&self) -> Value {
        let mut m = json!({ "role": "user", "content": self.content.clone(), "ts": now_ms() });
        match self.mode {
            SteerMode::Steer => m["steered"] = json!(true),
            SteerMode::Interrupt => m["interrupted"] = json!(true),
            SteerMode::Queued => {}
        }
        m
    }

    /// 队列消息**作为新一轮的第一条用户消息**落盘/推前端时的形态
    /// （收尾 followup、接管、无头续跑）：引导没赶上原轮的决策点/收尾补取，
    /// 这轮以它开头——它没有「静默并入」任何正在跑的轮，前端必须按普通用户气泡
    /// 展示。若沿用 [Self::to_message] 的 steered 标记，界面会写「已引导」，
    /// 实际却已经当新一轮发出去并被回答（2026-11-12 真机：引导被 followup 起成
    /// 新轮，刷新后成静默行，用户以为消息被吞；实时视图 d.turn 又按普通气泡追加，
    /// 刷新前后形态还不一致）。
    /// 打断（立即发送）照旧标 interrupted——它在界面上本来就是正常气泡。
    pub fn to_turn_message(&self) -> Value {
        let mut m = json!({ "role": "user", "content": self.content.clone(), "ts": now_ms() });
        if self.mode == SteerMode::Interrupt {
            m["interrupted"] = json!(true);
        }
        m
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Default)]
pub struct SteerQueue {
    inner: Mutex<HashMap<String, Vec<SteerMsg>>>,
    /// 每会话的打断信号（旧「立即发送」并入语义用；2026-11-07 起 chat 端点不再
    /// 产生 Interrupt 条目，字段保留给兼容路径）。
    interrupts: Mutex<HashMap<String, Arc<CancelFlag>>>,
    /// 每会话的「停轮重启」标记（「立即发送」置位；被停那轮的收尾取走，取走即清）。
    restarts: Mutex<std::collections::HashSet<String>>,
}

impl SteerQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加一条文本消息（服务层在「同会话已在跑」时调用）。返回积压条数。
    /// Interrupt 模式同时置位打断信号（三处等待点 select! 到它当场让位）。
    pub fn push(&self, session_id: &str, text: String, mode: SteerMode) -> usize {
        let content = json!(text.clone());
        self.push_content(session_id, text, content, mode)
    }

    /// 追加一条带 content 的消息（2026-11-03：引导/立即发送带附件——图片等原生
    /// 内容块由服务层的媒体流水线构好，这里只负责排队）。
    pub fn push_content(&self, session_id: &str, text: String, content: Value, mode: SteerMode) -> usize {
        if mode == SteerMode::Interrupt {
            self.interrupt_flag(session_id).set();
        }
        let mut m = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let v = m.entry(session_id.to_string()).or_default();
        v.push(SteerMsg { text, content, mode });
        v.len()
    }

    /// 把一条**已排队**的消息原地升级成引导/立即发送（同会话、同文本的第一条：
    /// 升引导找 Queued；升打断可以接手 Queued 或已引导的条目）。前端在「排队中」
    /// 气泡上点「引导/立即发送」时走这里：改档，而不是再排一条新的——否则原来那条
    /// 会在本轮收尾时被排水成下一轮，同一句话既显示「已引导」又被自动重发一遍
    /// （2026-11-07 真机；2026-11-12 真机又见「引导 + 立即发送」同文各排一条，
    /// 重启 followup 把同一句跑了两遍）。
    /// 找不到（没排过 / 已被取走 / 文本不同）返回 false，调用方按新消息正常入队。
    pub fn promote(&self, session_id: &str, text: &str, mode: SteerMode) -> bool {
        let mut m = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(v) = m.get_mut(session_id) else { return false };
        match mode {
            SteerMode::Steer => v
                .iter_mut()
                .find(|s| s.mode == SteerMode::Queued && s.text == text)
                .map(|s| s.mode = mode)
                .is_some(),
            SteerMode::Interrupt => v
                .iter_mut()
                // 排队中的直接升打断；已经引导过的也升（点「立即发送」= 这句要停轮重发，
                // 不能再留一条引导让 followup 把同文跑第二遍）
                .find(|s| s.text == text && matches!(s.mode, SteerMode::Queued | SteerMode::Steer))
                .map(|s| s.mode = mode)
                .is_some(),
            SteerMode::Queued => false,
        }
    }

    /// 队列里是否已有同文本的**任何**待办条目（排队/引导/打断）：「立即发送」的
    /// 原地判定——有就别再排一条，否则重启 followup 会把同一句话跑两遍。
    pub fn has_pending(&self, session_id: &str, text: &str) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .map(|v| v.iter().any(|s| s.text == text))
            .unwrap_or(false)
    }

    /// 「立即发送」的重启标记（2026-11-07 语义变更）：中断请求置位，被停的那一轮
    /// 收尾时取走——取到 = 这一轮停下后用队列里的消息开新一轮；没取到（用户点的是
    /// 「停止」）= 队列一并作废。**取走即清**：标记绝不能留到下一轮，否则之后一次
    /// 普通的「停止」会把用户想丢的排队消息当成新一轮跑掉。
    pub fn mark_restart(&self, session_id: &str) {
        self.restarts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(session_id.to_string());
    }

    /// 取走（并清除）重启标记。
    pub fn take_restart(&self, session_id: &str) -> bool {
        self.restarts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(session_id)
    }

    /// 本会话的打断信号（取或建；等待方 select! 它，push(Interrupt) 置位它）。
    pub fn interrupt_flag(&self, session_id: &str) -> Arc<CancelFlag> {
        let mut m = self.interrupts.lock().unwrap_or_else(|e| e.into_inner());
        m.entry(session_id.to_string()).or_insert_with(|| Arc::new(CancelFlag::new())).clone()
    }

    /// 本会话挂着的引导消息文本（**只读不动队列**）。GET /api/cancel 带出去给前端
    /// 恢复「已引导」标记用：标记只活在前端内存，刷新就丢——服务端队列在模型取走
    /// 之前是唯一权威，取走后由会话文件里的 steered 行接力。
    pub fn steer_texts(&self, session_id: &str) -> Vec<String> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .map(|v| {
                v.iter()
                    .filter(|s| s.mode == SteerMode::Steer)
                    .map(|s| s.text.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 本会话挂着的**排队**消息文本（只读不动队列）。GET /api/cancel 带出去给前端
    /// 对账：刷新快照/服务端接管恢复出来的「排队中」气泡可能比出队旧——消息已经
    /// 送达并跑成，前端却还挂着一个排队气泡，点「引导/立即发送」会把同一句再投
    /// 一遍（2026-11-12 真机：同一句被当成新一轮发出，界面上出现两份）。
    /// 前端据此判「已不在服务端队列 → 只清气泡，不再投递」。
    pub fn queued_texts(&self, session_id: &str) -> Vec<String> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .map(|v| {
                v.iter()
                    .filter(|s| s.mode == SteerMode::Queued)
                    .map(|s| s.text.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 本会话当前积压条数（防线用：超过上限说明客户端在循环重发，拒收新消息）。
    pub fn pending(&self, session_id: &str) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .map(|v| v.len())
            .unwrap_or(0)
    }

    /// 取走「引导/打断」的消息（核心循环每个决策点一次）；排队的留下等本轮结束。
    /// 取走打断消息时复位打断信号（指令已并入，让位使命完成）。
    /// 返回带 mode：消费方据此给消息打标记（引导=steered 静默，打断=interrupted 气泡）。
    pub fn drain_immediate(&self, session_id: &str) -> Vec<SteerMsg> {
        let mut m = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(v) = m.get_mut(session_id) else { return Vec::new() };
        let mut now = Vec::new();
        let mut keep = Vec::new();
        let mut took_interrupt = false;
        for s in v.drain(..) {
            match s.mode {
                SteerMode::Queued => keep.push(s),
                SteerMode::Steer => now.push(s),
                SteerMode::Interrupt => {
                    now.push(s);
                    took_interrupt = true;
                }
            }
        }
        if keep.is_empty() {
            m.remove(session_id);
        } else {
            *v = keep;
        }
        drop(m);
        if took_interrupt {
            self.clear_interrupt(session_id);
        }
        now
    }

    /// 是否有「引导/打断」的消息在等（**不取走**）。审批等待期据此让位：用户插话 =
    /// 要改方向，挂着的审批按拒绝处理，消息仍留在队列里给决策点取。
    pub fn has_immediate(&self, session_id: &str) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .map(|v| v.iter().any(|s| s.mode != SteerMode::Queued))
            .unwrap_or(false)
    }

    /// 复位打断信号（取走打断消息时调用；信号条目留给下次复用，drain_all 时清除）。
    fn clear_interrupt(&self, session_id: &str) {
        if let Some(f) = self
            .interrupts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
        {
            f.clear();
        }
    }

    /// 取走本会话全部消息（服务层收尾：一轮结束后按先到先得顺序接着跑），
    /// 并清掉打断信号条目（先复位再删——外部可能还持着旧 Arc，别让人看到置位残留）。
    /// 带 mode：消费方（followup / 接管）用 [SteerMsg::to_turn_message] 落盘——
    /// 这些消息成了新一轮的开头，不标 steered；放回队列时保真原 mode。
    pub fn drain_all(&self, session_id: &str) -> Vec<SteerMsg> {
        let mut m = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let out = m.remove(session_id).unwrap_or_default();
        drop(m);
        let mut ints = self.interrupts.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(f) = ints.remove(session_id) {
            f.clear();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_drain_per_session_and_order_kept() {
        let q = SteerQueue::new();
        assert!(q.drain_all("a").is_empty(), "没登记过的会话取到空表");
        assert_eq!(q.push("a", "第一条".into(), SteerMode::Queued), 1);
        assert_eq!(q.push("a", "第二条".into(), SteerMode::Steer), 2);
        assert_eq!(q.push("b", "别人的".into(), SteerMode::Steer), 1);

        // 先到先得（模型按顺序看到），取走后本会话清空、其它会话不受影响；
        // drain_all 带 mode（followup 轮按它打标记）
        let took = q.drain_all("a");
        assert_eq!(took.len(), 2);
        assert_eq!((took[0].text.as_str(), took[0].mode), ("第一条", SteerMode::Queued));
        assert_eq!((took[1].text.as_str(), took[1].mode), ("第二条", SteerMode::Steer));
        assert!(q.drain_all("a").is_empty(), "取空后再取是空表，不报错");
        assert_eq!(q.drain_all("b")[0].text, "别人的", "别的会话的消息不得被顺带清掉");
    }

    #[test]
    fn immediate_drains_now_queued_waits_for_turn_end() {
        let q = SteerQueue::new();
        q.push("s", "排队一".into(), SteerMode::Queued);
        q.push("s", "立即一".into(), SteerMode::Steer);
        q.push("s", "排队二".into(), SteerMode::Queued);

        // 探针只看不动：有立即项 = true，且取不走队列
        assert!(q.has_immediate("s"));
        assert_eq!(q.pending("s"), 3, "has_immediate 不得消费消息");
        assert!(!q.has_immediate("别的会话"));
        q.drain_immediate("s");
        assert!(!q.has_immediate("s"), "立即项取走后探针为 false");
        assert_eq!(q.pending("s"), 2, "排队项不受影响");

        // 决策点只取「立即」的，且保持原有先后顺序；排队的留在队列里
        assert!(q.drain_immediate("s").is_empty(), "取完立即项后为空");
        // 收尾把剩下的全取走（先到先得：排队一 → 排队二）
        let left = q.drain_all("s");
        assert_eq!(left.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), vec!["排队一", "排队二"]);
        assert!(q.drain_all("s").is_empty());
    }

    #[test]
    fn promote_upgrades_queued_in_place() {
        let q = SteerQueue::new();
        q.push("s", "把配色改成暖色".into(), SteerMode::Queued);
        // 「排队中」气泡上点引导 → 原地升级，不许再多出一条
        assert!(q.promote("s", "把配色改成暖色", SteerMode::Steer));
        assert_eq!(q.pending("s"), 1, "升级不能增加条数（否则收尾会再发一遍）");
        let now = q.drain_immediate("s");
        assert_eq!(now.len(), 1, "升级后的条目应能被决策点当引导取走");
        assert_eq!(now[0].mode, SteerMode::Steer);
        assert!(q.drain_all("s").is_empty(), "升级后队列里不许残留同文本的排队副本");
        // 文本对不上 / 队列里没有：不动，调用方按新消息入队
        assert!(!q.promote("s", "不存在的一句", SteerMode::Steer));
        assert!(!q.promote("s", "把配色改成暖色", SteerMode::Queued), "Queued 不能升级成 Queued");
    }

    /// 「引导过的消息再点立即发送」必须原地升打断，不许再排一条
    /// （2026-11-12 真机：同文一条 Steer + 一条 Interrupt 各占一份，
    /// 重启 followup 把同一句话跑了两遍）。
    #[test]
    fn promote_interrupt_takes_over_queued_or_steer_in_place() {
        let q = SteerQueue::new();
        q.push("s", "停下先改这个".into(), SteerMode::Steer);
        assert!(q.promote("s", "停下先改这个", SteerMode::Interrupt), "已引导的条目可原地升打断");
        assert_eq!(q.pending("s"), 1, "升级不能增加条数");
        let took = q.drain_all("s");
        assert_eq!(took.len(), 1);
        assert_eq!(took[0].mode, SteerMode::Interrupt, "同文只剩打断这一条");

        // 排队中的直接升打断
        q.push("s", "再排一条".into(), SteerMode::Queued);
        assert!(q.promote("s", "再排一条", SteerMode::Interrupt));
        assert_eq!(q.pending("s"), 1);
        assert_eq!(q.drain_all("s")[0].mode, SteerMode::Interrupt);
    }

    /// has_pending：排队/引导/打断任一同文条目都算（「立即发送」原地判定用）。
    #[test]
    fn has_pending_finds_any_mode() {
        let q = SteerQueue::new();
        q.push("s", "同一句".into(), SteerMode::Queued);
        assert!(q.has_pending("s", "同一句"));
        q.drain_all("s");
        q.push("s", "同一句".into(), SteerMode::Steer);
        assert!(q.has_pending("s", "同一句"), "引导条目也算待办");
        q.drain_all("s");
        assert!(!q.has_pending("s", "同一句"));
    }

    #[test]
    fn interrupt_queued_dedups_and_restart_flag_is_taken_once() {
        let q = SteerQueue::new();
        q.push("s", "停下先改这个".into(), SteerMode::Queued);
        // 「立即发送」的原地判定：队列里已有同文本待办条目 → 别再排一条
        assert!(q.has_pending("s", "停下先改这个"));
        assert!(!q.has_pending("s", "另一句"));
        assert_eq!(q.pending("s"), 1, "判重命中时不得新增条目");
        // 重启标记：置位 → 取走一次为 true，再取为 false（不能留给下一轮的「停止」）
        q.mark_restart("s");
        assert!(q.take_restart("s"));
        assert!(!q.take_restart("s"), "标记取走即清");
        assert!(!q.take_restart("别的会话"));
    }

    #[test]
    fn steer_texts_read_only_lists_pending_steers() {
        let q = SteerQueue::new();
        assert!(q.steer_texts("s").is_empty(), "没登记过的会话取到空表");
        q.push("s", "排队一".into(), SteerMode::Queued);
        q.push("s", "引导一".into(), SteerMode::Steer);
        q.push("s", "引导二".into(), SteerMode::Steer);
        // 只列引导，排队不打进标记；探针不消费——drain_immediate 还能全量取走
        assert_eq!(q.steer_texts("s"), vec!["引导一".to_string(), "引导二".to_string()]);
        assert_eq!(q.pending("s"), 3);
        let now = q.drain_immediate("s");
        assert_eq!(now.len(), 2, "引导被模型取走");
        assert!(q.steer_texts("s").is_empty(), "取走后探针为空，前端标记交给会话文件接力");
    }

    #[test]
    fn to_message_marks_only_immediate_modes() {
        let make = |mode| SteerMsg { text: "x".into(), content: json!("x"), mode };
        let s = make(SteerMode::Steer).to_message();
        assert_eq!(s["steered"], json!(true), "引导 → 已引导行");
        assert!(s.get("interrupted").is_none());
        let i = make(SteerMode::Interrupt).to_message();
        assert_eq!(i["interrupted"], json!(true), "打断 → 正常气泡");
        assert!(i.get("steered").is_none());
        let q = make(SteerMode::Queued).to_message();
        assert!(q.get("steered").is_none() && q.get("interrupted").is_none(), "排队 = 普通用户消息，不许标已引导");
        assert_eq!(q["role"], json!("user"));
        assert!(q["ts"].as_u64().unwrap_or(0) > 0);
    }

    /// followup / 接管把队列消息起成**新一轮**时：引导没并进原来的轮，不能标
    /// 「已引导」（2026-11-12 真机回归：标了 steered 后界面显示静默行，实际已被
    /// 当成新一轮回答；刷新前后形态也不一致）。
    #[test]
    fn to_turn_message_never_marks_steered() {
        let make = |mode| SteerMsg { text: "x".into(), content: json!("x"), mode };
        for mode in [SteerMode::Steer, SteerMode::Queued] {
            let m = make(mode).to_turn_message();
            assert!(m.get("steered").is_none(), "新轮的第一条用户消息不许标已引导");
            assert!(m.get("interrupted").is_none());
            assert_eq!(m["role"], json!("user"));
        }
        let i = make(SteerMode::Interrupt).to_turn_message();
        assert_eq!(i["interrupted"], json!(true), "打断仍标 interrupted（展示为普通气泡）");
        assert!(i.get("steered").is_none());
    }

    /// 排队文本探针：只列 Queued，给前端对账「气泡还在不在服务端队列里」。
    #[test]
    fn queued_texts_lists_only_queued_and_does_not_consume() {
        let q = SteerQueue::new();
        assert!(q.queued_texts("s").is_empty());
        q.push("s", "排队一".into(), SteerMode::Queued);
        q.push("s", "引导一".into(), SteerMode::Steer);
        q.push("s", "排队二".into(), SteerMode::Queued);
        assert_eq!(q.queued_texts("s"), vec!["排队一".to_string(), "排队二".to_string()]);
        assert_eq!(q.pending("s"), 3, "探针不得消费消息");
        q.drain_immediate("s");
        assert_eq!(q.queued_texts("s"), vec!["排队一".to_string(), "排队二".to_string()]);
        q.drain_all("s");
        assert!(q.queued_texts("s").is_empty());
    }

    /// 打断信号生命周期（2026-10-22「立即发送」）：push 置位 → 等待点 select! 让位 →
    /// 决策点取走打断消息时复位 → 复位后再 push 可再次置位 → 收尾清除。
    #[test]
    fn interrupt_signal_set_reset_and_reuse() {
        let q = SteerQueue::new();
        let flag = q.interrupt_flag("s");
        assert!(!flag.is_set(), "新建信号必须是复位态");

        q.push("s", "停下，听我说".into(), SteerMode::Interrupt);
        assert!(flag.is_set(), "打断式 push 必须置位信号");
        assert!(q.has_immediate("s"));

        // 决策点取走打断消息 → 信号复位（指令已并入本轮）
        let took = q.drain_immediate("s");
        assert_eq!(took.len(), 1);
        assert_eq!(took[0].text, "停下，听我说");
        assert_eq!(took[0].mode, SteerMode::Interrupt);
        assert!(!flag.is_set(), "取走打断消息必须复位信号");

        // 复用：同一条会话可以再次打断
        q.push("s", "又改主意了".into(), SteerMode::Interrupt);
        assert!(flag.is_set());
        q.drain_all("s");
        assert!(!flag.is_set(), "收尾 drain_all 清信号条目");

        // 打断消息只给本会话：别的会话置位不影响
        let f2 = q.interrupt_flag("other");
        q.push("s", "再打断".into(), SteerMode::Interrupt);
        assert!(!f2.is_set(), "别的会话的信号不得被连带置位");
    }
}
