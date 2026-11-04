//! ask：阻塞式提问（ZCode AskUserQuestion 同款，2026-11-03 交接清单第 2 项）。
//!
//! 语义（ZCode 源码 ask-user-question.ts）：agent 需要用户决策时调用，未回答前
//! 本轮挂起；2~4 个选项 + Other 自由输入（界面自动提供，参数里不许自带）+ 可多选。
//! **无人值守（定时/子代理/组件总线）禁用**——ZCode 同款：没人在场不许挂起等答案，
//! 模型应按已有信息自己决定并说明。与审批门并存不混淆：审批是权限闸，提问是方案决策。
//!
//! 挂起机制与审批门同构：oneshot + 全局注册表，答案经 POST /api/ask 回写；
//! 取消/打断由 exec_with_cancel 丢弃本 future，Drop 守卫负责清注册表；
//! 界面断流（壳/页签关闭）用 500ms 探测发现后作废。无硬超时（ZCode 语义：挂到
//! 回答为止），用户想放弃就点提问卡上的「跳过」或「停止」。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};

use super::ExecConfig;

type Sender = tokio::sync::oneshot::Sender<Value>;

fn pending() -> &'static Mutex<HashMap<String, Sender>> {
    static P: OnceLock<Mutex<HashMap<String, Sender>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(HashMap::new()))
}

/// POST /api/ask：用户答案回写（server 层调用）。false = id 不存在（已过期/已作废）。
pub fn decide(id: &str, answer: Value) -> bool {
    let tx = pending()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(id);
    match tx {
        Some(tx) => tx.send(answer).is_ok(),
        None => false,
    }
}

struct PendingGuard(String);
impl Drop for PendingGuard {
    fn drop(&mut self) {
        pending()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

pub async fn run(args: &str, cfg: &ExecConfig) -> Result<String, String> {
    let sink = match &cfg.ask {
        Some(s) => s.clone(),
        None => {
            return Err(
                "ask 提问工具在无人值守任务（定时任务/子代理/程序化调用）中不可用：没有用户在场回答。\
                 请按已有信息自己定一个合理的默认值继续，并在回答里说明你的选择。"
                    .to_string(),
            )
        }
    };
    let v: Value = serde_json::from_str(args).map_err(|e| format!("参数解析失败: {e}"))?;
    let question = v
        .get("question")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if question.is_empty() {
        return Err("缺少 question：把问题完整问出来（具体、以问号结尾）".to_string());
    }
    let opts = v
        .get("options")
        .and_then(Value::as_array)
        .ok_or("缺少 options：2~4 个选项数组")?;
    if !(2..=4).contains(&opts.len()) {
        return Err(format!("options 要 2~4 个（给了 {}）", opts.len()));
    }
    let mut options: Vec<(String, String)> = Vec::with_capacity(opts.len());
    for (i, o) in opts.iter().enumerate() {
        let label = o
            .get("label")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if label.is_empty() {
            return Err(format!("第 {} 个选项缺 label", i + 1));
        }
        if label.eq_ignore_ascii_case("other") {
            return Err("不要自带 Other 选项：自由输入由界面自动提供".to_string());
        }
        if options.iter().any(|(l, _)| l == label) {
            return Err(format!("选项 label 重复：{label}（同一问题内 label 必须唯一）"));
        }
        let desc = o.get("description").and_then(Value::as_str).unwrap_or("");
        options.push((label.to_string(), desc.to_string()));
    }
    let multi = v.get("multiSelect").and_then(Value::as_bool).unwrap_or(false);
    let header = v
        .get("header")
        .and_then(Value::as_str)
        .map(|s| s.trim().chars().take(12).collect::<String>())
        .unwrap_or_default();

    let id = uuid::Uuid::new_v4().simple().to_string();
    let (tx, mut rx) = tokio::sync::oneshot::channel::<Value>();
    pending()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id.clone(), tx);
    let _guard = PendingGuard(id.clone());

    // 推给前端（本轮 SSE 流）：与 approval_request 同通道同风格
    (sink.send)(json!({
        "ask_request": {
            "id": id, "session": sink.session, "header": header,
            "question": question, "multiSelect": multi,
            "options": options.iter().map(|(l, d)| json!({"label": l, "description": d})).collect::<Vec<_>>(),
        }
    }));
    crate::observability::event(
        crate::observability::EventType::State,
        json!({"kind": "ask.requested", "id": id, "session": sink.session, "question": question}),
    );

    // 等答案：无超时（挂到回答/跳过为止）；界面断流 = 没人能看到这个问题，作废
    let answer = loop {
        tokio::select! {
            r = &mut rx => {
                crate::observability::event(
                    crate::observability::EventType::State,
                    json!({"kind": "ask.answered", "id": id}),
                );
                break r.map_err(|_| "提问通道已关闭".to_string())?;
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                if (sink.closed)() {
                    return Err("提问已作废：界面连接断开，没人能看到这个问题".to_string());
                }
            }
        }
    };

    // 三分支文案照 ZCode：零回答不视为拒绝、不编造用户偏好
    if answer.get("dismissed").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(format!(
            "用户没有回答这个问题（直接关掉了提问）：「{question}」。请按你自己的最佳判断继续，\
             不要把这当成拒绝，也不要编造用户偏好。"
        ));
    }
    let selected: Vec<String> = answer
        .get("selected")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let other = answer
        .get("other")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if selected.is_empty() && other.is_empty() {
        return Ok(format!(
            "用户没有给出答案：「{question}」。请按你自己的最佳判断继续，\
             不要把这当成拒绝，也不要编造用户偏好。"
        ));
    }
    let mut parts: Vec<String> = Vec::new();
    if !selected.is_empty() {
        parts.push(selected.join("、"));
    }
    if !other.is_empty() {
        parts.push(format!("用户补充：{other}"));
    }
    Ok(format!(
        "用户已回答：「{question}」= {}。请基于这个回答继续。",
        parts.join("；")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SandboxConfig;
    use std::sync::Arc;

    fn cfg(sink: Option<Arc<crate::core::agent::AskSink>>) -> ExecConfig {
        ExecConfig {
            sandbox: SandboxConfig::default(),
            max_chars: 8000,
            unsandboxed: false,
            fs_unconfined: false,
            cpu_percent: 0,
            workspace: crate::tools::workspace_root(),
            cancel: None,
            ask: sink,
        }
    }

    #[tokio::test]
    async fn headless_disabled() {
        let err = run(r#"{"question":"？","options":[{"label":"a","description":""},{"label":"b","description":""}]}"#, &cfg(None))
            .await
            .unwrap_err();
        assert!(err.contains("无人值守"), "{err}");
    }

    #[tokio::test]
    async fn validates_shape() {
        let sink = cfg(Some(test_sink()));
        assert!(run(r#"{"options":[{"label":"a"},{"label":"b"}]}"#, &sink).await.is_err());
        assert!(run(r#"{"question":"？","options":[{"label":"a"},{"label":"b"},{"label":"a"}]}"#, &sink)
            .await
            .is_err(), "label 重复要报错");
        assert!(run(
            r#"{"question":"？","options":[{"label":"Other"},{"label":"b"}]}"#,
            &sink
        )
        .await
        .is_err(), "不许自带 Other");
        assert!(run(r#"{"question":"？","options":[{"label":"a"}]}"#, &sink).await.is_err());
    }

    #[tokio::test]
    async fn waits_for_answer_and_formats() {
        let state = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let state2 = state.clone();
        let sink = Arc::new(crate::core::agent::AskSink {
            session: "s1".into(),
            send: Box::new(move |_v| {
                state2.store(true, std::sync::atomic::Ordering::SeqCst);
            }),
            closed: Box::new(|| false),
        });
        let c = cfg(Some(sink));
        let args = r#"{"question":"用哪个？","header":"方案","options":[{"label":"甲","description":"快"},{"label":"乙","description":"稳"}],"multiSelect":false}"#;
        let h = tokio::spawn(async move { run(args, &c).await });
        // 请求发出后按 id 回答案
        let mut id = String::new();
        for _ in 0..100 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            if let Some(k) = pending().lock().unwrap_or_else(|e| e.into_inner()).keys().next().cloned() {
                id = k;
                break;
            }
        }
        assert!(!id.is_empty(), "提问应已注册");
        assert!(decide(&id, json!({"selected":["乙"]})), "回写应命中");
        let out = h.await.unwrap().unwrap();
        assert!(out.contains("用户已回答"), "{out}");
        assert!(out.contains("乙"), "{out}");
    }

    fn test_sink() -> Arc<crate::core::agent::AskSink> {
        Arc::new(crate::core::agent::AskSink {
            session: "s1".into(),
            send: Box::new(|_| {}),
            closed: Box::new(|| false),
        })
    }
}
