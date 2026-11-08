//! 运行中一轮的事件直播（2026-11-07）
//!
//! 刷新会断开原 SSE：任务被断线取消的宽限期救下来后，页面却再也收不到新事件，
//! 界面冻在刷新前最后一个工具调用上（真机：「一直卡在执行中」，其实后台一直在跑）。
//! 这里给每一轮留一份**带序号的事件尾巴**：原 SSE 照旧发，同时把同一份事件写进
//! 环形缓冲 + broadcast。刷新回来的页面用 `/api/chat/attach?after=<最后收到的 seq>`
//! 先补发漏掉的事件、再续上直播。
//!
//! 只保留当前一轮（按 session 覆盖）；一轮结束就摘掉，不会跨轮串事件。

use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::broadcast;

/// 单个会话保留的事件条数：刷新那几秒的流式分片（每步最多几百条）足够覆盖；
/// 更早的丢了也不影响——attach 只补「最后收到的 seq」之后的。
const KEEP_EVENTS: usize = 4096;
/// 直播广播的环形容量（订阅者慢消费时丢旧事件，attach 侧有 backlog 兜底）。
const LIVE_CHANNEL: usize = 512;

pub(super) struct TurnLive {
    seq: AtomicU64,
    events: Mutex<VecDeque<(u64, Value)>>,
    tx: broadcast::Sender<(u64, Value)>,
}

impl TurnLive {
    fn new() -> Self {
        let (tx, _) = broadcast::channel(LIVE_CHANNEL);
        Self {
            seq: AtomicU64::new(0),
            events: Mutex::new(VecDeque::new()),
            tx,
        }
    }

    /// 发一份事件副本：分配本轮自增 seq、进环形缓冲、广播给 attach 订阅者。
    /// **返回带 seq 的副本**——原 SSE 也要用它（前端靠这个 seq 记录「看到哪儿了」）。
    fn publish(&self, mut v: Value) -> Value {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        if let Value::Object(m) = &mut v {
            m.insert("seq".to_string(), json!(seq));
        }
        {
            let mut q = self.events.lock().unwrap_or_else(|e| e.into_inner());
            q.push_back((seq, v.clone()));
            while q.len() > KEEP_EVENTS {
                q.pop_front();
            }
        }
        let _ = self.tx.send((seq, v.clone()));
        v
    }
}

/// 进程级直播表：session_id → 当前在跑的这一轮。
fn live_map() -> &'static Mutex<HashMap<String, Arc<TurnLive>>> {
    static MAP: OnceLock<Mutex<HashMap<String, Arc<TurnLive>>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 新一轮开始：建直播槽并登记（同会话上一轮的残留直接覆盖）。
pub(super) fn begin(session: &str) -> Arc<TurnLive> {
    let live = Arc::new(TurnLive::new());
    live_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(session.to_string(), live.clone());
    live
}

/// 发事件给直播缓冲，返回带 seq 的副本（调用方拿它去发原 SSE，两边 seq 一致）。
pub(super) fn publish(live: &TurnLive, v: Value) -> Value {
    live.publish(v)
}

/// attach 的返回：(after 之后的补发事件, 后续直播订阅)
pub(super) type LiveAttach = (Vec<Value>, broadcast::Receiver<(u64, Value)>);

/// 取某个会话的直播：返回补发事件 + 后续订阅。
/// 没有在跑的轮次返回 None（页面按「已结束」走文件重载）。
pub(super) fn attach(session: &str, after: u64) -> Option<LiveAttach> {
    let live = live_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(session)
        .cloned()?;
    // 先订阅再取 backlog：两者之间的新事件不会漏（最多在 backlog 里重复一次，
    // 由前端按 seq 幂等忽略）。
    let rx = live.tx.subscribe();
    let backlog = {
        let q = live.events.lock().unwrap_or_else(|e| e.into_inner());
        q.iter()
            .filter(|(s, _)| *s > after)
            .map(|(_, v)| v.clone())
            .collect::<Vec<_>>()
    };
    Some((backlog, rx))
}

/// 一轮结束：只摘自己的槽（迟到的旧轮别把同一会话的新轮摘掉）。
pub(super) fn end(session: &str, live: &Arc<TurnLive>) {
    let mut m = live_map().lock().unwrap_or_else(|e| e.into_inner());
    if m.get(session).map(|cur| Arc::ptr_eq(cur, live)).unwrap_or(false) {
        m.remove(session);
    }
}

/// RAII：任务任何出口（正常/提前 return/panic）都摘掉直播槽。
pub(super) struct LiveGuard {
    session: String,
    live: Arc<TurnLive>,
}

impl LiveGuard {
    pub(super) fn new(session: &str, live: Arc<TurnLive>) -> Self {
        Self { session: session.to_string(), live }
    }
}

impl Drop for LiveGuard {
    fn drop(&mut self) {
        end(&self.session, &self.live);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seq_is_monotonic_and_attach_replays_after() {
        let live = TurnLive::new();
        for i in 0..5 {
            let out = live.publish(json!({ "content": format!("c{i}") }));
            assert_eq!(out["seq"], json!(i + 1));
            assert_eq!(out["content"], json!(format!("c{i}")));
        }
        let (backlog, _rx) = {
            // 直接测 TurnLive；registry 的 attach 需要会话名，见下一个测试
            let rx = live.tx.subscribe();
            let q = live.events.lock().unwrap();
            (
                q.iter()
                    .filter(|(s, _)| *s > 2)
                    .map(|(_, v)| v.clone())
                    .collect::<Vec<_>>(),
                rx,
            )
        };
        assert_eq!(backlog.len(), 3, "seq>2 的三条");
        assert_eq!(backlog[0]["seq"], json!(3));
        assert_eq!(backlog[0]["content"], json!("c2"));
    }

    #[test]
    fn ring_keeps_only_the_tail() {
        let live = TurnLive::new();
        for i in 0..(KEEP_EVENTS + 10) {
            live.publish(json!({ "i": i }));
        }
        let q = live.events.lock().unwrap();
        assert_eq!(q.len(), KEEP_EVENTS);
        assert_eq!(q.front().map(|(s, _)| *s), Some(11), "最老的 10 条被挤掉");
    }

    #[test]
    fn registry_begin_attach_end() {
        let live = begin("sess-live-test");
        publish(&live, json!({ "content": "a" }));
        publish(&live, json!({ "content": "b" }));
        let (backlog, _rx) = attach("sess-live-test", 0).expect("在跑的一轮");
        assert_eq!(backlog.len(), 2);
        let (after, _rx) = attach("sess-live-test", 1).expect("在跑的一轮");
        assert_eq!(after.len(), 1);
        assert_eq!(after[0]["content"], json!("b"));
        assert!(attach("sess-live-test-absent", 0).is_none(), "没在跑=附件失败");
        end("sess-live-test", &live);
        assert!(attach("sess-live-test", 0).is_none(), "结束即摘槽");
    }
}
