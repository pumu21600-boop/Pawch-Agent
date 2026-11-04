use super::*;

/// 执行一个工具调用——`subagent_spawn` 在核心循环拦截交给派发器
/// （子代理跑在独立会话，实现见 server/subagent.rs）；其余走工具注册表。
pub(crate) async fn exec_tool_call(
    cfg: &crate::tools::ExecConfig,
    name: &str,
    args: &str,
    subagents: Option<&Arc<dyn crate::core::subagent::SubagentRunner>>,
    parent_session: &str,
) -> Result<String, String> {
    // 参数体检先行（见 tool_args_guard）：截断/非法 JSON 在这里就给可操作反馈，
    // 不用等各个工具自己吐 serde 天书。
    if let Some(msg) = tool_args_guard(name, args) {
        return Err(msg);
    }
    if name == "subagent_spawn" {
        return match subagents {
            Some(runner) => {
                // 停止/「立即发送」会把本 future 直接丢弃（exec_with_cancel 的 select!）——
                // 子代理自己跑在独立任务里，不级联取消就会继续跑到 30 分钟上限、继续花钱
                // （2026-11-02 审计）。Drop 守卫负责这条路径；正常完成不触发。
                struct CascadeOnDrop {
                    runner: Arc<dyn crate::core::subagent::SubagentRunner>,
                    parent: String,
                    done: bool,
                }
                impl Drop for CascadeOnDrop {
                    fn drop(&mut self) {
                        if !self.done {
                            self.runner.cancel_children(&self.parent);
                        }
                    }
                }
                let mut guard = CascadeOnDrop {
                    runner: runner.clone(),
                    parent: parent_session.to_string(),
                    done: false,
                };
                let out = runner.run(parent_session, args).await;
                guard.done = true;
                Ok(out)
            }
            None => Err("子代理派发未启用（本任务没有派发器）".to_string()),
        };
    }
    tools::execute(cfg, name, args).await
}

/// 工具参数体检（2026-10-30 真机实锤后加）。
///
/// 实锤经过：模型想一次吐出 50KB 的单文件 HTML，输出打到 max_tokens 上限被切断，
/// `write_file` 的参数是个**没闭合的 JSON 字符串**。各工具只能报 serde 的原始错误
/// （`参数解析失败: EOF while parsing a string at line 1 column 50524`）——模型拿到
/// 这种天书纯靠运气猜「该分块了」，换个笨点的模型就会反复重试同一招。
///
/// 这里在分发前判形态：截断（EOF while parsing …）给人话 + 分块写法指引，其他非法
/// JSON 也给出比 serde 清楚的指示。返回 None = 放行。
pub(crate) fn tool_args_guard(name: &str, args: &str) -> Option<String> {
    // 空参数放行：让工具自己按原行为报（有工具认空参数）
    if args.trim().is_empty() {
        return None;
    }
    match serde_json::from_str::<Value>(args) {
        Ok(_) => None,
        Err(e) => {
            let msg = e.to_string();
            let chars = args.chars().count();
            // "EOF while parsing a string / value / object" = 输出被齐根切断
            let truncated = msg.starts_with("EOF while parsing");
            // try_event：日志没初始化也不许把工具执行搞崩（单测/极早调用）
            observability::try_event(
                EventType::State,
                json!({
                    "kind": if truncated { "tool.args_truncated" } else { "tool.args_invalid" },
                    "tool": name,
                    "chars": chars,
                    "column": e.column(),
                    "error": msg,
                }),
            );
            if truncated {
                Some(format!(
                    "工具参数被截断，本次没有执行：生成的 JSON 在第 {} 列处中断（字符串没闭合）。\
                     这几乎总是**输出长度达到上限**被打断，不是语法错误——原样重试还会再被截断。\n\
                     请改成分块写法（这次已经生成了 {chars} 字符，别再一把梭）：\n\
                     1. write_file 只写第一块，content 控制在 6KB 以内，文件末尾留一个占位标记（如 <!--PART2-->）；\n\
                     2. 然后用 edit_file 把占位标记替换成第二块 + 新的占位标记，逐块续写；\n\
                     3. 内容确实很多就拆成多个文件。单次输出越短越稳也越快。",
                    e.column()
                ))
            } else {
                Some(format!(
                    "工具参数不是合法 JSON（{msg}）。请重新生成完整参数：必须是一个 \
                     {{\"key\": \"value\"}} 形式的对象，不要夹带解释文字、代码块围栏或注释。"
                ))
            }
        }
    }
}

/// 坑 31 ① 收口 + 2026-10-22「立即发送」分家：工具执行统一过两道闸——
/// - 取消闸（停止）：工具卡住（浏览器 CDP 调用超时 ~30s、长命令未返回等）时
///   当场中断，不再干等工具返回（此前只靠下个决策点轮询 is_set，停止要等工具
///   自己超时，期间服务端仍显示在跑、新消息被排进队列——用户实测实锤）。
/// - 打断闸（立即发送）：用户插话要求让位——工具按「已中断」回喂，**本轮继续**
///   （下一个决策点取走新指令），与停止的区别就是循环不终止。
///
/// select! 丢掉工具 future 即返回；被丢的底层操作（沙箱子进程、CDP 连接等）
/// 按自身超时自终，不会复活本轮。
pub(crate) async fn exec_with_cancel<E>(cancel: &CancelFlag, interrupt: &CancelFlag, exec: E) -> Result<String, String>
where
    E: Future<Output = Result<String, String>>,
{
    tokio::select! {
        r = exec => r,
        () = cancel.cancelled() => Err("已取消：用户点了结束回答".to_string()),
        () = interrupt.cancelled() => Err(
            "已中断：用户发来新指令（「立即发送」），本操作未完成。请结合新指令决定下一步。".to_string(),
        ),
    }
}

/// 工具参数压成一行短 reason（写文件等大参数只留头部）。
pub(crate) fn short_args(args: &str, max_chars: usize) -> String {
    let flat: String = args.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    flat.chars().take(max_chars).collect()
}

/// A3（施工单 012）：能力标记「工具调用」闭环的核心判定——tools_enabled=false 时不带
/// tools 参数。独立成纯函数便于单测。
pub(crate) fn tools_def_for(params: &ChatParams) -> Option<Value> {
    if params.tools_enabled {
        Some(Value::Array(tools::definitions()))
    } else {
        None
    }
}
