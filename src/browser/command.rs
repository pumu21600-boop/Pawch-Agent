use super::*;

pub(crate) async fn current_session() -> Result<Arc<BrowserSession>, String> {
    // 2026-10-30：锁内只取快照，探活/摘死页都在锁外——全局 SESSION 锁不再跨网络 await
    // （否则一个卡死的页面让所有 browser 工具排队）。
    let Some(s) = SESSION.lock().await.clone() else {
        return Err("浏览器未打开：先打开一个网页".into());
    };
    // 先自愈：壳重启后旧会话里的 tab 可能全死了，不清就会把僵尸 tab 给出去
    if !browser_alive(s.port).await {
        log("browser.stale", json!({}));
        if let Some(dead) = take_session_if_current(&s).await {
            kill_child(&dead);
        }
        return Err("浏览器未打开：先打开一个网页".into());
    }
    let dead: Vec<String> = s
        .tabs
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|t| !t.cdp.is_alive())
        .map(|t| t.id.clone())
        .collect();
    for id in &dead {
        close_tab(&s, id).await;
    }
    if s.tabs.lock().unwrap_or_else(|e| e.into_inner()).is_empty() {
        // 全死：回收旧会话后按原语义重建一个（工具调用拿到可用会话，而不是报错）
        log("browser.stale", json!({ "reason": "tabs-dead" }));
        if let Some(dead_session) = take_session_if_current(&s).await {
            kill_child(&dead_session);
        }
        return ensure_session().await;
    }
    Ok(s)
}

/// 页面状态（隔离世界求值 + 导航历史）。
pub(crate) async fn page_state(tab: &WebTab) -> Result<PageState, BrowserError> {
    let (title, url) = tab_title_url(tab).await;
    let mut st = PageState { url, title, ..Default::default() };
    if let Ok(hist) = tab.cdp.call("Page.getNavigationHistory", json!({})).await {
        let index = hist.get("currentIndex").and_then(Value::as_i64).unwrap_or(0);
        let count = hist.get("entries").and_then(Value::as_array).map(|a| a.len() as i64).unwrap_or(0);
        st.can_go_back = index > 0;
        st.can_go_forward = index + 1 < count;
    }
    let ready = tab
        .cdp
        .evaluate_in_isolated_world(ISOLATED_WORLD, "document.readyState")
        .await
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    st.loading = !ready.is_empty() && ready != "complete";
    if let Some((w, h)) = *tab.viewport.lock().await {
        st.viewport_w = w;
        st.viewport_h = h;
    }
    Ok(st)
}

/// Agent 工具侧 URL 准入（2026-10-08 收紧）：只放行 http(s) 与 about:blank。
/// file:// 一度放行——navigate 判只读 Safe，read-only 档也零审批，模型
/// `navigate file:///C:/Users/.../.ssh/id_rsa` 再 snapshot 即拿到文件正文，
/// 等于绕过 read_file 的工作区禁锢读任意本地文件。用户想开本地文件（如 agent
/// 生成的 HTML 报告）走「网页」面板地址栏——check_panel_url 仍放行 file://。
pub(crate) fn check_url(url: &str) -> Result<(), BrowserError> {
    if url.starts_with("http://") || url.starts_with("https://") || url == "about:blank" {
        return Ok(());
    }
    if url.starts_with("file://") {
        // 只放行**工作区内**的本地文件（preview_file 的产物）：file://+snapshot 曾被
        // 点名是绕过 read_file 工作区禁锢读任意本地文件的路（2026-10-08 实锤），
        // 工作区外的照旧拒——读文件有 read_file 的审批口径，浏览器不例外
        let ws = crate::tools::workspace_root().to_path_buf();
        if file_url_in_workspace(url, &ws).is_ok() {
            return Ok(());
        }
        return Err(BrowserError::new(
            ErrorCode::NavigationBlocked,
            "file:// 只能打开当前工作区里的文件（预览工作区文件用 preview_file）；工作区外的本地文件不放行，要看内容用 read_file",
        ));
    }
    Err(BrowserError::new(
        ErrorCode::NavigationBlocked,
        "地址必须以 http:// / https:// / file:// 开头（或 about:blank）；工作区文件用 preview_file 方法",
    ))
}

/// file:/// URL 是否落在工作区根内（纯函数好单测；路径断言要 canonicalize 的老教训）。
pub(crate) fn file_url_in_workspace(url: &str, ws: &std::path::Path) -> Result<(), String> {
    let u = url::Url::parse(url).map_err(|_| "不是合法地址".to_string())?;
    if u.scheme() != "file" {
        return Err("非 file 地址".into());
    }
    let p = u.to_file_path().map_err(|_| "file 地址解析失败".to_string())?;
    let p = p.canonicalize().map_err(|_| "文件不存在".to_string())?;
    let ws_canon = ws.canonicalize().map_err(|_| "工作区不存在".to_string())?;
    if p.starts_with(&ws_canon) {
        Ok(())
    } else {
        Err("工作区外".into())
    }
}

/// 工作区相对路径 → file:/// URL（preview_file 的纯函数部分，好单测）。
/// canonicalize 防相对段/符号链接漂移（「路径断言要 canonicalize」的老教训）。
pub(crate) fn workspace_file_url(ws: &std::path::Path, rel: &str) -> Result<String, String> {
    let rel_trim = rel.trim();
    if rel_trim.is_empty() {
        return Err("path 不能为空：传工作区相对路径，如 index.html 或 dist/页面.html".into());
    }
    let canon = ws
        .join(rel_trim)
        .canonicalize()
        .map_err(|_| format!("工作区里没有这个文件：{rel_trim}（先确认文件名/目录，或先把文件写出来再预览）"))?;
    if !canon.is_file() {
        return Err(format!("「{rel_trim}」不是文件（可能是目录），preview_file 只收文件"));
    }
    // Windows canonicalize 给 verbatim 路径（\\?\C:\...），url crate 不认，剥掉前缀
    let plain = canon.to_string_lossy();
    let plain = plain.strip_prefix(r"\\?\").unwrap_or(&plain);
    url::Url::from_file_path(plain)
        .map(|u| u.to_string())
        .map_err(|_| format!("转本地地址失败：{plain} 不是标准绝对路径"))
}

/// 在标签页里导航并等加载完成；返回超时提示（None=正常）。会话死了返回 Err。
pub(crate) async fn navigate_tab(tab: &WebTab, url: &str) -> Result<Option<&'static str>, String> {
    let nav = tab
        .cdp
        .call_and_wait_event("Page.navigate", json!({ "url": url }), "Page.loadEventFired", LOAD_TIMEOUT)
        .await;
    match nav {
        Ok(_) => {
            tab.cdp.invalidate_worlds(); // 导航销毁了旧的隔离世界 context
            tab.reset_snapshot_state().await; // ref/diff 基线按页编号，换页即废（2026-10-08 修）
            Ok(None)
        }
        Err(e) if e.contains("等待事件超时") => Ok(Some("（页面加载未在预算内完成，以下为当前状态）\n")),
        Err(e) => Err(e),
    }
}

/// 历史导航（后退/前进）：Page.getNavigationHistory + navigateToHistoryEntry。
pub(crate) async fn history_go(tab: &WebTab, offset: i64) -> Result<(), BrowserError> {
    let hist = tab
        .cdp
        .call("Page.getNavigationHistory", json!({}))
        .await
        .map_err(exec_err)?;
    let index = hist.get("currentIndex").and_then(Value::as_i64).unwrap_or(0);
    let entries = hist.get("entries").and_then(Value::as_array);
    let target = index + offset;
    if let Some(entries) = entries
        && target >= 0
        && let Some(entry) = entries.get(target as usize)
        && let Some(entry_id) = entry.get("id").and_then(Value::as_i64)
    {
        tab.cdp
            .call("Page.navigateToHistoryEntry", json!({ "entryId": entry_id }))
            .await
            .map_err(exec_err)?;
        // 给导航一点沉降时间（不硬等 load：历史条目可能早已缓存）
        tokio::time::sleep(Duration::from_millis(400)).await;
        return Ok(());
    }
    Err(BrowserError::new(
        ErrorCode::CapabilityUnsupported,
        "没有更早/更晚的历史页",
    ))
}

/// wait_for 轮询 JS（隔离世界）：selector 出现/消失、文本出现/消失。
pub(crate) const WAIT_FOR_JS: &str = r#"((mode, needle) => {
  if (mode === 'selector') return !!document.querySelector(needle);
  if (mode === 'selectorGone') return !document.querySelector(needle);
  const body = document.body ? document.body.innerText : '';
  if (mode === 'text') return body.includes(needle);
  return !body.includes(needle);
})"#;

pub(crate) async fn wait_for(tab: &WebTab, cmd: &BrowserCommand) -> Result<String, BrowserError> {
    let (selector, text, text_gone, timeout_ms) = match cmd {
        BrowserCommand::WaitFor { selector, text, text_gone, timeout_ms, .. } => {
            (selector.clone(), text.clone(), text_gone.clone(), *timeout_ms)
        }
        _ => unreachable!(),
    };
    let (mode, needle) = if let Some(s) = selector {
        ("selector", s)
    } else if let Some(t) = text {
        ("text", t)
    } else if let Some(t) = text_gone {
        ("textGone", t)
    } else {
        return Err(BrowserError::new(
            ErrorCode::CapabilityUnsupported,
            "wait_for 需要 selector / text / text_gone 之一",
        ));
    };
    let budget = Duration::from_millis(timeout_ms.unwrap_or(WAIT_FOR_DEFAULT_MS).min(WAIT_FOR_MAX_MS));
    let expr = format!(
        "{WAIT_FOR_JS}({}, {})",
        serde_json::to_string(mode).unwrap(),
        serde_json::to_string(&needle).unwrap()
    );
    let deadline = Instant::now() + budget;
    loop {
        let hit = tab
            .cdp
            .evaluate_in_isolated_world(ISOLATED_WORLD, &expr)
            .await
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if hit {
            return Ok(match mode {
                "selector" => format!("已出现：selector「{needle}」"),
                "selectorGone" => format!("已消失：selector「{needle}」"),
                "text" => format!("已出现：文本「{needle}」"),
                _ => format!("已消失：文本「{needle}」"),
            });
        }
        if Instant::now() > deadline {
            return Err(BrowserError::new(
                ErrorCode::Timeout,
                format!("等待超时（{}ms）：{mode}「{needle}」未满足。可先 snapshot 看当前页面", budget.as_millis()),
            ));
        }
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
}

/// element_info：给坐标反查命中元素（移植自 ZCode elementInfoRuntime）。
pub(crate) const ELEMENT_INFO_JS: &str = r#"((x, y) => {
  const roleOf = (el) => el.getAttribute('role')
    ?? (el.matches('button,input[type=button],input[type=submit]') ? 'button'
      : el.matches('a[href]') ? 'link'
      : el.matches('input:not([type]),input[type=text],textarea') ? 'textbox' : null);
  const interactable = (el) => Boolean(roleOf(el) || el.matches('input,select,textarea,[tabindex],[contenteditable]'));
  return document.elementsFromPoint(x, y).filter((el) => interactable(el)).slice(0, 5).map((el) => {
    const r = el.getBoundingClientRect();
    return {
      tag: el.tagName.toLowerCase(),
      role: roleOf(el),
      name: (el.getAttribute('aria-label') || el.getAttribute('placeholder') || (el.innerText || '').trim() || el.value || '').slice(0, 60),
      rect: [Math.round(r.x), Math.round(r.y), Math.round(r.width), Math.round(r.height)],
    };
  });
})"#;

/// select：对 <select> 按 value 或可见文本选择。
pub(crate) const SELECT_JS: &str = r#"((sel, values) => {
  const el = document.querySelector('[data-agent-ref="' + sel + '"]');
  if (!el || el.tagName !== 'SELECT') return { ok: false, why: 'ref 不是 select' };
  const want = new Set(values);
  let hit = 0;
  for (const o of el.options) {
    const match = want.has(o.value) || want.has((o.text || '').trim());
    if (el.multiple) { o.selected = match; if (match) hit++; }
    else if (match) { el.value = o.value; hit++; break; }
  }
  el.dispatchEvent(new Event('input', { bubbles: true }));
  el.dispatchEvent(new Event('change', { bubbles: true }));
  return { ok: hit > 0, value: el.value };
})"#;

/// check：设置 checkbox/radio 勾选态。
pub(crate) const CHECK_JS: &str = r#"((sel, checked) => {
  const el = document.querySelector('[data-agent-ref="' + sel + '"]');
  if (!el) return { ok: false, why: 'ref 不存在' };
  if (el.type !== 'checkbox' && el.type !== 'radio') return { ok: false, why: 'ref 不是 checkbox/radio' };
  el.checked = checked;
  el.dispatchEvent(new Event('input', { bubbles: true }));
  el.dispatchEvent(new Event('change', { bubbles: true }));
  return { ok: true, checked: el.checked };
})"#;

/// 统一命令分发：agent 工具唯一入口。缺省 tab = agent 专用页。
///
/// 能力总闸（「插件开关就是能力总闸，关掉就是没有了」）：
/// 「浏览器」插件被停用时 agent 一律拒动。此前只有技能被收回（agent 少了说明书），
/// 工具本身照旧能跑——于是「停了还能用」，与用户看到的开关语义对不上。
pub async fn command(args: &str, max_chars: usize) -> Result<String, String> {
    if !capability_on() {
        return Err(BROWSER_OFF_MSG.to_string());
    }
    let cmd = commands::parse(args).map_err(String::from)?;
    // preview_file 在入口折算成 navigate（file:/// 工作区文件地址）：后面的标签页解析、
    // 落点检查、快照全走同一条路，零分叉
    let cmd = match cmd {
        BrowserCommand::PreviewFile { path, tab_id } => {
            let url = workspace_file_url(&crate::tools::workspace_root().to_path_buf(), &path)
                .map_err(|e| BrowserError::new(ErrorCode::ExecutionError, e))?;
            BrowserCommand::Navigate { url, tab_id }
        }
        other => other,
    };
    let out = dispatch(cmd, max_chars).await.map_err(String::from);
    // agent 碰过浏览器（不论成败）就记时间戳——面板据此自动唤起右侧栏。
    // 与面板显隐分开：agent 的触碰只唤醒界面，不直接把 webview 弹到聊天区上空。
    if let Ok(s) = current_session().await {
        s.agent_touched_ms.store(now_ms(), Ordering::Relaxed);
    }
    out
}

/// agent 工具的 tab 切换（new_tab / activate_tab）。**shell 模式下不写 visible_tab**：
/// visible_tab 是「面板正在看哪个页」，agent 一激活就把用户正在看的页换掉，紧接着那页
/// 还会因为「非可见 + 空闲」被驻留巡检挂起（frozen 的页面看着正常但 JS 停了，用户
/// 点击没反应——真机反馈"AI 点完之后我就点不了网站了"）。无头时代写它只为驱动
/// screencast，壳里这套语义已经不成立；agent 的页留在 tab 栏里，用户想看去点即可。
pub(crate) fn agent_activate(session: &Arc<BrowserSession>, id: &str) {
    session.set_agent_tab(id);
    if session.mode != HostMode::Shell {
        session.set_visible(id);
    }
}

pub(crate) async fn dispatch(cmd: BrowserCommand, max_chars: usize) -> Result<String, BrowserError> {
    // 会话级命令（不作用单 tab）
    match &cmd {
        BrowserCommand::NewTab => {
            let session =
                ensure_session().await.map_err(|e| BrowserError::new(ErrorCode::BackendUnavailable, e))?;
            session.touch();
            let tab = create_tab(&session, "about:blank", false)
                .await
                .map_err(|e| BrowserError::new(ErrorCode::BackendUnavailable, e))?;
            agent_activate(&session, &tab.id);
            sweep_session(&session).await;
            return Ok(format!("已新建标签页 {}（about:blank）。browser navigate 可加载地址。", tab.id));
        }
        BrowserCommand::List => {
            let session =
                current_session().await.map_err(|e| BrowserError::new(ErrorCode::BackendUnavailable, e))?;
            session.touch();
            let briefs = session.tab_briefs().await;
            if briefs.is_empty() {
                return Ok("当前没有标签页。".into());
            }
            let mut out = String::from("标签页：\n");
            for b in &briefs {
                out.push_str(&format!(
                    "  {} {}{}{} 「{}」 {}\n",
                    b.id,
                    if b.agent { "[agent] " } else { "" },
                    if b.visible { "[可见] " } else { "" },
                    if b.suspended { "[已挂起] " } else { "" },
                    if b.title.is_empty() { "（无标题）" } else { &b.title },
                    if b.url.is_empty() { "about:blank".to_string() } else { b.url.clone() },
                ));
            }
            return Ok(out);
        }
        BrowserCommand::ActivateTab { tab_id } => {
            let session =
                current_session().await.map_err(|e| BrowserError::new(ErrorCode::BackendUnavailable, e))?;
            session.touch();
            let tab = get_tab(&session, tab_id).map_err(|e| BrowserError::new(ErrorCode::CapabilityUnsupported, e))?;
            ensure_live(&tab)
                .await
                .map_err(|e| BrowserError::new(ErrorCode::BackendUnavailable, e))?;
            agent_activate(&session, tab_id);
            sweep_session(&session).await;
            return Ok(format!("已切换到标签页 {tab_id}（面板可见）。"));
        }
        BrowserCommand::Capabilities => {
            let host = match current_session().await {
                Ok(s) if s.mode == HostMode::Shell => "桌面壳 child webview（真页面）+ 页面级 CDP".to_string(),
                _ => "无头 Edge/Chrome + 页面级 CDP".to_string(),
            };
            return Ok(format!(
                "内置浏览器（{host}）。方法: {}。\n定位优先级: ref → selector → text/label 语义 → x/y 坐标；快照/定位/求值均在隔离世界执行，页面脚本无法篡改。截图类方法刷新网页面板画面；agent 侧读取页面请用 snapshot。",
                ALL_METHODS.join(" / ")
            ));
        }
        BrowserCommand::CloseBrowser => {
            let mode = current_session().await.ok().map(|s| s.mode);
            let alive = mode.is_some();
            close_internal("tool").await;
            if alive {
                let how = if mode == Some(HostMode::Shell) { "壳内页面已全部关闭" } else { "进程已退出" };
                return Ok(format!("浏览器已关闭（{how}；下次命令会自动重新拉起）。"));
            }
            return Ok("浏览器本来就没在运行。".into());
        }
        _ => {}
    }

    // tab 级命令：解析目标页（缺省 = agent 专用页）
    let session = ensure_session().await.map_err(|e| BrowserError::new(ErrorCode::BackendUnavailable, friendly_backend_error(&e)))?;
    session.touch();
    let tab = match cmd.tab_id() {
        Some(id) => get_tab(&session, id).map_err(|e| BrowserError::new(ErrorCode::CapabilityUnsupported, e))?,
        // navigate 且没指定页：跨站点开新标签（同站点/空白页复用），其余方法一律用 agent 当前页
        None => match &cmd {
            BrowserCommand::Navigate { url, .. } => {
                check_url(url)?; // 先验地址，别为非法 URL 白开一页
                agent_tab_for_navigate(&session, url)
                    .await
                    .map_err(|e| BrowserError::new(ErrorCode::BackendUnavailable, friendly_backend_error(&e)))?
                    .0
            }
            _ => agent_tab(&session)
                .await
                .map_err(|e| BrowserError::new(ErrorCode::BackendUnavailable, friendly_backend_error(&e)))?,
        },
    };
    tab.touch();
    ensure_live(&tab)
        .await
        .map_err(|e| BrowserError::new(ErrorCode::BackendUnavailable, format!("恢复挂起标签页失败: {e}")))?;

    match cmd {
        // preview_file 在入口（command）已折算成 Navigate；这里兜底防御
        BrowserCommand::PreviewFile { .. } => Err(BrowserError::new(
            ErrorCode::ExecutionError,
            "preview_file 内部折叠失败，请直接重试",
        )),
        BrowserCommand::Navigate { url, .. } => {
            check_url(&url)?;
            log("browser.navigate", json!({ "url": url, "tab": tab.id }));
            let note = match navigate_tab(&tab, &url).await {
                Ok(note) => note.unwrap_or(""),
                Err(e) => {
                    if e.contains("连接已断开") {
                        close_internal("dead").await;
                        // 标签已死：落点无从查起，直接给 friendly 文案
                        return Err(BrowserError::new(
                            ErrorCode::BackendUnavailable,
                            friendly_backend_error(&e),
                        ));
                    }
                    // 失败也先查落点：CDP 超时时 Chromium 往往还在连接，
                    // 错误页几秒后才落地——等它，拿到「别再重试 + 代理诊断」
                    let msg = chrome_error_note_wait(&tab, 12_000)
                        .await
                        .unwrap_or_else(|| friendly_backend_error(&e));
                    return Err(BrowserError::new(ErrorCode::BackendUnavailable, msg));
                }
            };
            // 落点检查（成功路径也要查——错误页同样会触发 load 事件）：
            // 页面停在 chrome-error = 站点连不上，给模型「别再重试」的人话 + 代理诊断，
            // 免得它在网络不通时反复烧 20-40 秒重试（2026-09-27 真机实测）。
            if let Some(msg) = chrome_error_note(&tab).await {
                return Err(BrowserError::new(ErrorCode::BackendUnavailable, msg));
            }
            let snap = take_snapshot(&tab, max_chars, None, false)
                .await
                .map_err(|e| BrowserError::new(ErrorCode::ExecutionError, e))?;
            Ok(format!("{note}{snap}"))
        }
        BrowserCommand::Back { .. } | BrowserCommand::Forward { .. } => {
            let back = matches!(cmd, BrowserCommand::Back { .. });
            history_go(&tab, if back { -1 } else { 1 }).await?;
            tab.cdp.invalidate_worlds();
            let st = page_state(&tab).await?;
            Ok(format!("已{}。\n{}", if back { "后退" } else { "前进" }, st.render()))
        }
        BrowserCommand::Reload { .. } => {
            tab.cdp.call("Page.reload", json!({})).await.map_err(exec_err)?;
            tokio::time::sleep(Duration::from_millis(400)).await;
            tab.cdp.invalidate_worlds();
            let st = page_state(&tab).await?;
            Ok(format!("已刷新。\n{}", st.render()))
        }
        BrowserCommand::Snapshot { max_elements, include_hidden, .. } => {
            take_snapshot(&tab, max_chars, max_elements, include_hidden == Some(true))
                .await
                .map_err(|e| BrowserError::new(ErrorCode::ExecutionError, e))
        }
        BrowserCommand::Click { reference, x, y, text, button, double_click, modifiers, .. } => {
            let (px, py, desc) = match (reference.as_deref(), text.as_deref(), x, y) {
                (None, None, Some(x), Some(y)) => (x as i64, y as i64, format!("坐标 ({x},{y})")),
                (r, t, _, _) => locate(&tab, r.unwrap_or(""), t.unwrap_or(""), "").await?,
            };
            let button = button.unwrap_or_default();
            let mask = modifier_mask(&modifiers);
            if double_click == Some(true) {
                dispatch_click(&tab, px, py, button, mask).await.map_err(exec_err)?;
                dispatch_click(&tab, px, py, button, mask).await.map_err(exec_err)?;
            } else {
                click_and_settle(&tab, px, py, button, mask).await.map_err(exec_err)?;
            }
            log("browser.click", json!({ "target": desc, "x": px, "y": py, "tab": tab.id }));
            let snap = take_snapshot(&tab, max_chars, None, false)
                .await
                .map_err(|e| BrowserError::new(ErrorCode::ExecutionError, e))?;
            Ok(format!("已点击 {desc}（{px},{py}）\n{snap}"))
        }
        BrowserCommand::Fill { reference, value, .. } => {
            let (px, py, desc) = locate(&tab, &reference, "", "").await?;
            input_into(&tab, px, py, &value, false).await.map_err(exec_err)?;
            log("browser.fill", json!({ "target": desc, "chars": value.chars().count(), "tab": tab.id }));
            let snap = take_snapshot(&tab, max_chars, None, false)
                .await
                .map_err(|e| BrowserError::new(ErrorCode::ExecutionError, e))?;
            Ok(format!("已填入 {desc}：{} 字符\n{snap}", value.chars().count()))
        }
        BrowserCommand::Type { reference, text, label, submit, .. } => {
            let (px, py, desc) = locate(&tab, reference.as_deref().unwrap_or(""), "", label.as_deref().unwrap_or("")).await?;
            input_into(&tab, px, py, &text, submit == Some(true)).await.map_err(exec_err)?;
            log("browser.type", json!({ "target": desc, "chars": text.chars().count(), "tab": tab.id }));
            let snap = take_snapshot(&tab, max_chars, None, false)
                .await
                .map_err(|e| BrowserError::new(ErrorCode::ExecutionError, e))?;
            Ok(format!(
                "已向 {desc} 输入 {} 字符{}\n{snap}",
                text.chars().count(),
                if submit == Some(true) { " 并回车" } else { "" }
            ))
        }
        BrowserCommand::Press { key, reference, modifiers, .. } => {
            let mask = modifier_mask(&modifiers);
            if let Some(r) = reference.as_deref()
                && !r.is_empty()
            {
                // 先点聚焦（真实事件，触发站点自己的聚焦逻辑）
                if let Ok((px, py, _)) = locate(&tab, r, "", "").await {
                    let _ = dispatch_click(&tab, px, py, MouseButton::Left, 0).await;
                }
            }
            press_key(&tab, &key, mask).await.map_err(exec_err)?;
            tokio::time::sleep(Duration::from_millis(250)).await;
            tab.cdp.invalidate_worlds();
            let st = page_state(&tab).await?;
            Ok(format!("已按键 {key}。\n{}", st.render()))
        }
        BrowserCommand::Scroll { reference, x, y, delta_x, delta_y, .. } => {
            if let Some(r) = reference.as_deref()
                && !r.is_empty()
            {
                // ref 滚动：把元素滚到视口中央
                let expr = format!(
                    "(() => {{ const el = document.querySelector('[data-agent-ref=\"{}\"]'); if (!el) return false; el.scrollIntoView({{block:'center'}}); return true; }})()",
                    r.replace('\\', "\\\\").replace('"', "\\\"")
                );
                let ok = tab
                    .cdp
                    .evaluate_in_isolated_world(ISOLATED_WORLD, &expr)
                    .await
                    .ok()
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if !ok {
                    return Err(BrowserError::new(
                        ErrorCode::RefNotFound,
                        format!("ref {r} 不存在，无法滚动到它"),
                    ));
                }
            } else {
                let dx = delta_x.unwrap_or(0.0);
                let dy = delta_y.unwrap_or(0.0);
                if dx == 0.0 && dy == 0.0 {
                    return Err(BrowserError::new(
                        ErrorCode::CapabilityUnsupported,
                        "scroll 需要 ref（滚到元素）或 delta_x/delta_y（滚轮增量）",
                    ));
                }
                let (ax, ay) = (x.unwrap_or(640.0), y.unwrap_or(450.0));
                tab.cdp
                    .call(
                        "Input.dispatchMouseEvent",
                        json!({ "type": "mouseWheel", "x": ax, "y": ay, "deltaX": dx, "deltaY": dy }),
                    )
                    .await
                    .map_err(exec_err)?;
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
            let pos = tab
                .cdp
                .evaluate_in_isolated_world(ISOLATED_WORLD, "[Math.round(scrollX), Math.round(scrollY)]")
                .await
                .ok()
                .and_then(|v| v.as_array().cloned())
                .map(|a| {
                    format!(
                        "（滚动位置 {},{})",
                        a.first().and_then(Value::as_i64).unwrap_or(0),
                        a.get(1).and_then(Value::as_i64).unwrap_or(0)
                    )
                })
                .unwrap_or_default();
            Ok(format!("已滚动{pos}"))
        }
        BrowserCommand::Hover { reference, x, y, .. } => {
            let (px, py, desc) = match (reference.as_deref(), x, y) {
                (Some(r), _, _) if !r.is_empty() => locate(&tab, r, "", "").await?,
                (None, Some(x), Some(y)) => (x as i64, y as i64, format!("坐标 ({x},{y})")),
                _ => {
                    return Err(BrowserError::new(ErrorCode::CapabilityUnsupported, "hover 需要 ref 或 x/y"));
                }
            };
            tab.cdp
                .call("Input.dispatchMouseEvent", json!({ "type": "mouseMoved", "x": px as f64, "y": py as f64 }))
                .await
                .map_err(exec_err)?;
            Ok(format!("已悬停 {desc}"))
        }
        BrowserCommand::Select { reference, values, .. } => {
            let expr = format!(
                "{SELECT_JS}({}, {})",
                serde_json::to_string(&reference).unwrap(),
                serde_json::to_string(&values).unwrap()
            );
            let res = tab
                .cdp
                .evaluate_in_isolated_world(ISOLATED_WORLD, &expr)
                .await
                .map_err(|e| BrowserError::new(ErrorCode::ExecutionError, e))?;
            if res.get("ok").and_then(Value::as_bool) != Some(true) {
                let why = res.get("why").and_then(Value::as_str).unwrap_or("没有匹配选项");
                return Err(BrowserError::new(ErrorCode::RefNotFound, format!("select 失败: {why}")));
            }
            let snap = take_snapshot(&tab, max_chars, None, false)
                .await
                .map_err(|e| BrowserError::new(ErrorCode::ExecutionError, e))?;
            Ok(format!("已选择 {}。\n{snap}", values.join(" / ")))
        }
        BrowserCommand::Check { reference, checked, .. } => {
            let expr = format!(
                "{CHECK_JS}({}, {})",
                serde_json::to_string(&reference).unwrap(),
                serde_json::to_string(&checked.unwrap_or(true)).unwrap()
            );
            let res = tab
                .cdp
                .evaluate_in_isolated_world(ISOLATED_WORLD, &expr)
                .await
                .map_err(|e| BrowserError::new(ErrorCode::ExecutionError, e))?;
            if res.get("ok").and_then(Value::as_bool) != Some(true) {
                let why = res.get("why").and_then(Value::as_str).unwrap_or("ref 不存在");
                return Err(BrowserError::new(ErrorCode::RefNotFound, format!("check 失败: {why}")));
            }
            let snap = take_snapshot(&tab, max_chars, None, false)
                .await
                .map_err(|e| BrowserError::new(ErrorCode::ExecutionError, e))?;
            Ok(format!("已{}。\n{snap}", if checked.unwrap_or(true) { "勾选" } else { "取消勾选" }))
        }
        BrowserCommand::Drag { from_reference, to_reference, from, to, .. } => {
            let (fx, fy, _) = resolve_point(&tab, from_reference.as_deref(), from, "起点").await?;
            let (tx, ty, _) = resolve_point(&tab, to_reference.as_deref(), to, "终点").await?;
            tab.cdp
                .call(
                    "Input.dispatchMouseEvent",
                    json!({ "type": "mousePressed", "x": fx as f64, "y": fy as f64, "button": "left", "clickCount": 1 }),
                )
                .await
                .map_err(exec_err)?;
            // 中间插几步 move：只发首尾的拖拽很多站点不认
            let (fx, fy, tx, ty) = (fx as f64, fy as f64, tx as f64, ty as f64);
            for i in 1..=4 {
                let mx = fx + (tx - fx) * i as f64 / 5.0;
                let my = fy + (ty - fy) * i as f64 / 5.0;
                tab.cdp
                    .call("Input.dispatchMouseEvent", json!({ "type": "mouseMoved", "x": mx, "y": my, "button": "left" }))
                    .await
                    .map_err(exec_err)?;
            }
            tab.cdp
                .call(
                    "Input.dispatchMouseEvent",
                    json!({ "type": "mouseReleased", "x": tx, "y": ty, "button": "left", "clickCount": 1 }),
                )
                .await
                .map_err(exec_err)?;
            tokio::time::sleep(Duration::from_millis(200)).await;
            Ok(format!("已从 ({fx},{fy}) 拖到 ({tx},{ty})"))
        }
        BrowserCommand::Screenshot { reference, full_page, clip, .. } => {
            // agent 侧拿不到图片内容（文本通道）：截图落进面板帧缓冲，用户可在面板直接看
            let mut params = json!({ "format": "jpeg", "quality": 80 });
            if full_page == Some(true) {
                params["captureBeyondViewport"] = json!(true);
            }
            // ref 元素截图：裁到元素rect（与 clip 互斥，clip 优先）
            let mut element_clip = None;
            if clip.is_none()
                && let Some(r) = reference.as_deref()
                && !r.is_empty()
            {
                let rect = tab
                    .cdp
                    .evaluate_in_isolated_world(
                        ISOLATED_WORLD,
                        &format!(
                            "(() => {{ const el = document.querySelector('[data-agent-ref=\"{}\"]'); if (!el) return null; const r = el.getBoundingClientRect(); return [Math.round(r.x), Math.round(r.y), Math.round(r.width), Math.round(r.height)]; }})()",
                            r.replace('\\', "\\\\").replace('"', "\\\"")
                        ),
                    )
                    .await
                    .ok();
                match rect.as_ref().and_then(Value::as_array) {
                    Some(a) if a.len() == 4 => {
                        element_clip = Some((
                            a[0].as_f64().unwrap_or(0.0),
                            a[1].as_f64().unwrap_or(0.0),
                            a[2].as_f64().unwrap_or(0.0),
                            a[3].as_f64().unwrap_or(0.0),
                        ));
                    }
                    _ => {
                        return Err(BrowserError::new(
                            ErrorCode::RefNotFound,
                            format!("ref {r} 不存在，无法裁元素截图"),
                        ));
                    }
                }
            }
            if let Some(c) = clip {
                params["clip"] = json!({ "x": c.x, "y": c.y, "width": c.width, "height": c.height, "scale": 1 });
            } else if let Some((x, y, w, h)) = element_clip {
                params["clip"] = json!({ "x": x, "y": y, "width": w.max(1.0), "height": h.max(1.0), "scale": 1 });
            }
            let res = tab.cdp.call("Page.captureScreenshot", params).await.map_err(exec_err)?;
            let data = res
                .get("data")
                .and_then(Value::as_str)
                .ok_or_else(|| BrowserError::new(ErrorCode::ExecutionError, "截图结果缺 data"))?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|e| BrowserError::new(ErrorCode::ExecutionError, format!("截图 base64 解码失败: {e}")))?;
            *tab.frame.lock().await = Some(Frame { data: bytes.clone() });
            let st = page_state(&tab).await?;
            // 「聊天界面显示图片」：截图同时落一份到工作区，
            // 返回里带 markdown 图片语法——前端 md() 把 ![](路径) 渲染成图，
            // 模型也能据此引用（此前只推面板，聊天里什么都看不到）。
            let shot_line = match save_shot_to_workspace(&bytes) {
                Some(rel) => format!("\n截图已保存：{rel}\n![页面截图]({rel})\n"),
                None => String::new(),
            };
            Ok(format!(
                "截图已刷新到网页面板{}。{}{}",
                if full_page == Some(true) {
                    "（整页）"
                } else if element_clip.is_some() {
                    "（元素裁剪）"
                } else {
                    ""
                },
                shot_line,
                st.render()
            ))
        }
        BrowserCommand::GetState { .. } => {
            let st = page_state(&tab).await?;
            let dialog = tab.dialog.lock().await.clone();
            let mut out = st.render();
            if let Some(d) = dialog {
                out.push_str(&format!("\n弹窗: [{}] {}", d.kind, d.message));
            }
            Ok(out)
        }
        BrowserCommand::ElementInfo { x, y, .. } => {
            let expr = format!("{ELEMENT_INFO_JS}({}, {})", x, y);
            let val = tab
                .cdp
                .evaluate_in_isolated_world(ISOLATED_WORLD, &expr)
                .await
                .map_err(|e| BrowserError::new(ErrorCode::ExecutionError, e))?;
            let items = val.as_array().cloned().unwrap_or_default();
            if items.is_empty() {
                return Ok(format!("({x},{y}) 处没有可交互元素"));
            }
            let mut out = format!("({x},{y}) 命中元素（从顶到底）：\n");
            for it in &items {
                let rect = it.get("rect").and_then(Value::as_array);
                let rect_s = match rect {
                    Some(r) if r.len() == 4 => format!(
                        "[{},{},{},{}]",
                        r[0].as_i64().unwrap_or(0),
                        r[1].as_i64().unwrap_or(0),
                        r[2].as_i64().unwrap_or(0),
                        r[3].as_i64().unwrap_or(0)
                    ),
                    _ => "?".into(),
                };
                out.push_str(&format!(
                    "  [{} {}] \"{}\" rect={}\n",
                    it.get("role").and_then(Value::as_str).unwrap_or("?"),
                    it.get("tag").and_then(Value::as_str).unwrap_or("?"),
                    it.get("name").and_then(Value::as_str).unwrap_or(""),
                    rect_s,
                ));
            }
            Ok(out)
        }
        BrowserCommand::Evaluate { expression, .. } => {
            let val = tab
                .cdp
                .evaluate_in_isolated_world(ISOLATED_WORLD, &expression)
                .await
                .map_err(|e| BrowserError::new(ErrorCode::ExecutionError, e))?;
            let s = serde_json::to_string(&val).unwrap_or_else(|_| val.to_string());
            Ok(format!("求值结果: {}", truncate_str(&s, 2000)))
        }
        BrowserCommand::PageText { .. } => {
            // 整页正文：innerText（可见文本，不含 script/style）。与 evaluate 的区别只在
            // 上限——走 max_chars 而不是钉死 2000（抓整篇文章/长页面用）。
            let val = tab
                .cdp
                .evaluate_in_isolated_world(ISOLATED_WORLD, "document.body ? document.body.innerText : ''")
                .await
                .map_err(|e| BrowserError::new(ErrorCode::ExecutionError, e))?;
            let text = val.as_str().unwrap_or("");
            if text.trim().is_empty() {
                return Err(BrowserError::new(ErrorCode::ExecutionError, "页面正文为空（可能还在加载或要求登录）"));
            }
            Ok(truncate_str(text, max_chars))
        }
        BrowserCommand::WaitFor { .. } => wait_for(&tab, &cmd).await,
        BrowserCommand::GetDialog { .. } => {
            let dialog = tab.dialog.lock().await.clone();
            match dialog {
                Some(d) => Ok(format!(
                    "当前弹窗: [{}] {}{}",
                    d.kind,
                    d.message,
                    if d.default_prompt.is_empty() {
                        String::new()
                    } else {
                        format!("（默认值: {}）", d.default_prompt)
                    }
                )),
                None => Ok("当前没有弹窗".into()),
            }
        }
        BrowserCommand::HandleDialog { accept, prompt_text, .. } => {
            if tab.dialog.lock().await.is_none() {
                return Err(BrowserError::new(
                    ErrorCode::CapabilityUnsupported,
                    "当前没有弹窗可处理",
                ));
            }
            let mut params = json!({ "accept": accept });
            if let Some(t) = prompt_text {
                params["promptText"] = json!(t);
            }
            tab.cdp.call("Page.handleJavaScriptDialog", params).await.map_err(exec_err)?;
            *tab.dialog.lock().await = None;
            Ok(format!("已{}弹窗", if accept { "接受" } else { "取消" }))
        }
        BrowserCommand::Close { tab_id } => {
            let target = match tab_id.as_deref() {
                Some(id) => id.to_string(),
                None => tab.id.clone(),
            };
            if close_tab(&session, &target).await {
                Ok(format!("已关闭标签页 {target}"))
            } else {
                Err(BrowserError::new(
                    ErrorCode::CapabilityUnsupported,
                    format!("标签页不存在: {target}"),
                ))
            }
        }
        // 会话级命令已在函数开头分发，不会到这里
        BrowserCommand::NewTab
        | BrowserCommand::CloseBrowser
        | BrowserCommand::List
        | BrowserCommand::ActivateTab { .. }
        | BrowserCommand::Capabilities => Err(BrowserError::new(
            ErrorCode::CapabilityUnsupported,
            "该命令应在会话级处理",
        )),
    }
}

/// 拖拽点解析：ref → 坐标。
pub(crate) async fn resolve_point(
    tab: &WebTab,
    reference: Option<&str>,
    point: Option<Point>,
    what: &str,
) -> Result<(i64, i64, String), BrowserError> {
    if let Some(p) = point {
        return Ok((p.x as i64, p.y as i64, format!("{what} 坐标")));
    }
    if let Some(r) = reference {
        return locate(tab, r, "", "").await.map(|(x, y, d)| (x, y, format!("{what} {d}")));
    }
    Err(BrowserError::new(
        ErrorCode::CapabilityUnsupported,
        format!("drag 的{what}需要 ref 或 x/y"),
    ))
}

pub(crate) fn exec_err(e: String) -> BrowserError {
    if e.contains("连接已断开") {
        BrowserError::new(
            ErrorCode::BackendUnavailable,
            format!("浏览器连接已断开（标签页崩溃或浏览器退出）: {e}"),
        )
    } else {
        BrowserError::new(ErrorCode::ExecutionError, e)
    }
}

/// 截图落盘到工作区（2026-10-31：用户要「聊天界面显示图片」）。
/// 存到 `<workspace>/.agent/screenshots/` 下，返回可直接塞进 markdown 的**相对路径**
/// （形如 `/api/workspace/raw?path=...`，前端 md() 的图片白名单认这个前缀）。
/// 只用时间戳命名：同一秒重复截图会覆盖上一张，不堆垃圾文件。
fn save_shot_to_workspace(png: &[u8]) -> Option<String> {
    let root = crate::tools::workspace_root();
    let dir = root.join(".agent").join("screenshots");
    if std::fs::create_dir_all(&dir).is_err() {
        return None;
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let rel = format!(".agent/screenshots/shot-{ts}.png");
    let full = root.join(&rel);
    if std::fs::write(&full, png).is_err() {
        return None;
    }
    Some(format!(
        "/api/workspace/raw?path={}",
        rel.replace('%', "%25").replace('/', "%2F")
    ))
}

/// 后端类错误的人话化（navigate 超时时满屏
/// 「[backend_unavailable] 浏览器会话异常已重置：CDP 调用超时: Page.navigate」，
/// 要的是 AI 能自然地说「访问超时」）。人话在前（模型转述给用户），
/// 原始细节留在尾部（诊断用），错误码仍由 BrowserError Display 前置。
pub(crate) fn friendly_backend_error(e: &str) -> String {
    // chrome_error_note 生成的已是完整人话（含代理诊断），原样透传
    if e.starts_with("访问失败：") {
        return e.to_string();
    }
    if e.contains("CDP 调用超时") {
        timeout_error_text(e, &probe_system_proxy())
    } else if e.contains("连接已断开") || e.contains("actor 已退出") {
        format!("浏览器连接已断开：浏览器进程可能已被系统回收。重试 navigate 会自动重新拉起。（{e}）")
    } else {
        format!("浏览器暂时不可用：重试 navigate 即可，仍不行就 close_browser 后重新打开。（{e}）")
    }
}

/// CDP 超时的人话（纯函数可单测）。确诊「代理客户端活着、系统代理开关关闭」=
/// 环境误配，重试/重开都救不了——文案必须终局（2026-09-27 真机实测：旧文案
/// 「可以重试一次；仍不行就 close_browser 后重新打开」在前、代理诊断缀在尾，
/// 模型照前半句连试 close_browser + 导航又烧一分多钟）。
pub(crate) fn timeout_error_text(e: &str, diag: &ProxyDiagnosis) -> String {
    let retry_ok = !matches!(diag, ProxyDiagnosis::ClientAliveSwitchOff(_));
    format!(
        "访问超时：浏览器没有在预期时间内响应（页面打开太慢或网络不通都会这样）。{}（{e}）{}",
        if retry_ok {
            "可以重试一次；仍不行就 close_browser 后重新打开。"
        } else {
            "原因已定位：本机代理客户端在运行但系统代理开关是关闭的（浏览器跟随系统代理=直连），重试和 close_browser 重开都救不了——不要再重试，直接结束本轮向用户说明：打开代理客户端的「系统代理」或 TUN 模式即可，改好后会让你重试。注意这只影响外网：本地地址（127.0.0.1/localhost/工作区文件）不走代理，照常可开，别因此放弃内置浏览器。"
        },
        diagnosis_hint(diag),
    )
}

/// 落点检查：导航结束后页面停在 chrome-error = 访问失败。
/// 先分「HTTP 错误页（站点可达，4xx/5xx）」与「网络不通（超时/被拒/被墙）」两种再给人话
/// （别再重试 + 对应诊断）；不在错误页返回 None。
pub(crate) async fn chrome_error_note(tab: &WebTab) -> Option<String> {
    let url = current_page_url(tab).await?;
    if !url.starts_with("chrome-error://") {
        return None;
    }
    Some(chrome_error_detail(tab).await)
}

/// chrome-error 落地后的完整人话：HTTP 错误页走「站点可达」口径，其余才是网络不通。
/// 求值读不到状态码（弹窗/页面楔死）时按网络不通处理，与旧行为一致。
async fn chrome_error_detail(tab: &WebTab) -> String {
    if let Some(code) = http_error_code(tab).await {
        return http_error_text(code);
    }
    let target = error_target_url(tab).await;
    chrome_error_text(target.as_deref().unwrap_or("chrome-error://chromewebdata/"), &probe_system_proxy())
}

/// 从导航历史里捞出错页**之前**的目标地址（失败诊断分流用：本地地址与外网的
/// 人话完全不同）。历史里只有 chrome-error 一条时返回 None，走通用文案。
async fn error_target_url(tab: &WebTab) -> Option<String> {
    let hist = tab.cdp.call("Page.getNavigationHistory", json!({})).await.ok()?;
    let entries = hist.get("entries")?.as_array()?;
    entries
        .iter()
        .rev()
        .filter_map(|e| e.get("url")?.as_str())
        .find(|u| !u.starts_with("chrome-error://"))
        .map(String::from)
}

/// 本地地址判定（错误文案分流用）：127.0.0.1/localhost/[::1]/file://。
/// 这些地址不走代理，失败也不该跑代理诊断——2026-10-31 真机实锤：agent 起的
/// 本地端口没服务在听，报「系统代理开关关闭」把它带去查代理，白烧十几轮。
pub(crate) fn is_local_url(url: &str) -> bool {
    let u = url.trim();
    if u.starts_with("file://") {
        return true;
    }
    let rest = match u.split_once("://") {
        Some((scheme, r)) if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") => r,
        _ => return false,
    };
    let host = rest.split('/').next().unwrap_or("");
    // [::1]:8080 → 剥方括号取 ::1；普通 host:port → 剥端口（两个分支顺序不能反）
    let bare = if let Some(h) = host.strip_prefix('[') {
        h.split(']').next().unwrap_or(h)
    } else {
        host.split(':').next().unwrap_or(host)
    };
    matches!(bare.to_ascii_lowercase().as_str(), "127.0.0.1" | "localhost" | "::1")
}

/// 读错误页正文里的 HTTP 状态码（Chromium 的 4xx/5xx 错误页正文含 "HTTP ERROR 404"，
/// 中文本地化也只翻前半句）。读不到/不是 HTTP 错误返回 None——按网络不通处理。
async fn http_error_code(tab: &WebTab) -> Option<u16> {
    let v = tab
        .cdp
        .evaluate_in_isolated_world(
            ISOLATED_WORLD,
            "document.body ? (document.body.innerText || '') : ''",
        )
        .await
        .ok()?;
    parse_http_error_code(v.as_str()?)
}

/// 纯函数：从错误页正文提取三位状态码（可单测）。
pub(crate) fn parse_http_error_code(text: &str) -> Option<u16> {
    const MARK: &str = "HTTP ERROR ";
    let idx = text.find(MARK)?;
    let digits: String = text[idx + MARK.len()..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits
        .parse()
        .ok()
        .filter(|code| (100..=599).contains(code))
}

/// HTTP 错误页的人话：站点可达，别再往「网络不通/代理」上诊断，也别重试——
/// 把真实状态码告诉用户，让用户决定换地址还是换方式。
pub(crate) fn http_error_text(code: u16) -> String {
    format!(
        "访问失败：站点可达，但该地址返回了 HTTP {code} 错误页——页面不存在或不允许直接打开。\
         常见情形：API 端点只认 POST，用浏览器直接 GET 就会 404，这不是故障。\
         不要再重试导航；把真实状态码 {code} 告诉用户，按用户意图换地址或换方式（如 web_fetch）。"
    )
}

/// 失败路径专用：CDP 超时那一刻 Chromium 往往还在连接（错误页要再等约 10s 才落地），
/// 轮询等错误页出现再给诊断——多花几秒换「别再瞎重试 + 怎么修」的 actionable 结论。
pub(crate) async fn chrome_error_note_wait(tab: &WebTab, max_ms: u64) -> Option<String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(max_ms);
    loop {
        if let Some(url) = current_page_url(tab).await
            && url.starts_with("chrome-error://")
        {
            return Some(chrome_error_detail(tab).await);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// chrome-error 页的人话（纯函数可单测）。确诊代理误配时点名「重开也没用」，
/// 别让模型照旧文案去 close_browser 折腾一轮。
pub(crate) fn chrome_error_text(url: &str, diag: &ProxyDiagnosis) -> String {
    // 本地地址连不上 = 端口没服务在听，跟代理无关——别把模型带去查代理
    //（2026-10-31 真机实锤：本地端口拒连被报成「系统代理开关关闭」，agent 白烧十几轮）
    if is_local_url(url) {
        return format!(
            "访问失败：本地地址（{url}）连不上——这个端口本机没有服务在监听（连接被拒），就到这一步为止。\
             不要再重试同一地址：先确认服务是否真的启动、端口是否写对；预览工作区文件优先用工作区接口的本地地址，别自己另起服务。"
        );
    }
    let diagnosed = matches!(diag, ProxyDiagnosis::ClientAliveSwitchOff(_));
    let fix = if diagnosed {
        "原因已定位：本机代理客户端在运行但系统代理开关是关闭的——重试和 close_browser 重开都救不了，直接结束本轮告诉用户：打开代理客户端的「系统代理」或 TUN 模式即可。注意这只影响外网：本地地址（127.0.0.1/localhost/工作区文件）不走代理，照常可开，别因此放弃内置浏览器。"
    } else {
        "直接把「访问失败」告诉用户；若用户说网络已恢复，先 close_browser 重开浏览器实例再试一次。"
    };
    format!(
        "访问失败：页面落在浏览器错误页（{url}）——站点连不上（连接超时/被拒）。不要再重试导航。{fix}{}",
        diagnosis_hint(diag)
    )
}

/// 当前页 URL（优先 location.href——chrome 错误页的导航历史仍记原目标 URL，
/// 只有页面自己报的 href 才是 chrome-error://，2026-09-27 实锤漏检；
/// 求值不可用时退回导航历史）。失败返回 None，不阻塞主流程。
pub(crate) async fn current_page_url(tab: &WebTab) -> Option<String> {
    if tab.dialog.lock().await.is_none()
        && let Ok(v) = tab
            .cdp
            .evaluate_in_isolated_world(ISOLATED_WORLD, "location.href")
            .await
        && let Some(s) = v.as_str()
        && !s.is_empty()
    {
        return Some(s.to_string());
    }
    let hist = tab
        .cdp
        .call("Page.getNavigationHistory", json!({}))
        .await
        .ok()?;
    let entries = hist.get("entries")?.as_array()?;
    let idx = hist.get("currentIndex")?.as_u64()? as usize;
    entries.get(idx)?.get("url")?.as_str().map(String::from)
}

/// 系统代理探测结果（chrome-error / CDP 超时的落点诊断用）。真机实锤（2026-09-27）：
/// 代理客户端在跑但「系统代理」开关关闭时，浏览器跟随系统=直连，被墙站点必超时
/// （curl 显式走代理却秒通）——这个组合值得点名，用户一拨开关就好。
pub(crate) enum ProxyDiagnosis {
    /// 探测不到可信结论（注册表读不到/开关关且无本地代理端口）：不带诊断
    Silent,
    /// 开关开着但仍不通：查分流规则
    EnabledButBlocked,
    /// 实锤误配：客户端活着、开关关闭——重试无解，必须用户拨开关
    ClientAliveSwitchOff(String),
}

/// 读 HKCU 注册表 + 探本地代理端口，归入 [ProxyDiagnosis]。
pub(crate) fn probe_system_proxy() -> ProxyDiagnosis {
    let Ok(out) = std::process::Command::new("reg")
        .args(["query", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings"])
        .creation_flags(0x0800_0000)
        .output()
    else {
        return ProxyDiagnosis::Silent;
    };
    let (enabled, server) = parse_proxy_state(&String::from_utf8_lossy(&out.stdout));
    match (enabled, server) {
        (true, _) => ProxyDiagnosis::EnabledButBlocked,
        (false, Some(server)) if local_proxy_alive(&server) => {
            ProxyDiagnosis::ClientAliveSwitchOff(server)
        }
        _ => ProxyDiagnosis::Silent,
    }
}

/// 诊断 → 附给模型的人话（不确诊时返回空串）。
pub(crate) fn diagnosis_hint(diag: &ProxyDiagnosis) -> String {
    match diag {
        ProxyDiagnosis::EnabledButBlocked => {
            "另外：系统代理当前已开启但仍不通——检查代理客户端的分流规则（该站点是否被分到了直连）。".to_string()
        }
        ProxyDiagnosis::ClientAliveSwitchOff(server) => format!(
            "另外：检测到本机有代理客户端在运行（注册表里留着 {server} 这个地址），但系统代理开关是关闭的——\
             现在没有任何流量在走 {server}，浏览器是直连，直连不通的站点才会超时；\
             打开代理客户端的「系统代理」或 TUN 模式即可，打开后外网流量才借道它出去。"
        ),
        ProxyDiagnosis::Silent => String::new(),
    }
}

/// 解析 `reg query` 输出里的 ProxyEnable/ProxyServer。纯函数（可单测）。
/// ProxyServer 可能是 `host:port` 或 `http=…;https=…` 分号形式。
pub(crate) fn parse_proxy_state(text: &str) -> (bool, Option<String>) {
    let mut enabled = true;
    let mut server: Option<String> = None;
    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        match parts[0] {
            "ProxyEnable" => enabled = parts[2] == "0x1",
            "ProxyServer" => server = Some(parts[2..].join(" ")),
            _ => {}
        }
    }
    (enabled, server)
}

/// 代理地址（可能分号分隔多个 host:port）是否有任一端口活着。
pub(crate) fn local_proxy_alive(server: &str) -> bool {
    server.split(';').any(|s| {
        let s = s.trim();
        // http=host:port 形态取值部分；纯 host:port 直接用
        let hp = s.split_once('=').map(|(_, v)| v).unwrap_or(s);
        let Some((host, port)) = hp.rsplit_once(':') else { return false };
        let Ok(addr) = format!("{host}:{port}").parse::<std::net::SocketAddr>() else { return false };
        std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(200)).is_ok()
    })
}

/// 输入：聚焦（真实点击）→ 全选清空 → 插文本 → 可选回车。
pub(crate) async fn input_into(tab: &WebTab, x: i64, y: i64, text: &str, submit: bool) -> Result<(), String> {
    dispatch_click(tab, x, y, MouseButton::Left, 0).await?;
    // Ctrl+A 全选后输入即替换（真实用户习惯，比直接置 value 更贴站点行为）
    for t in ["rawKeyDown", "keyUp"] {
        let _ = tab
            .cdp
            .call(
                "Input.dispatchKeyEvent",
                json!({ "type": t, "key": "a", "code": "KeyA", "windowsVirtualKeyCode": 65, "modifiers": 2 }),
            )
            .await;
    }
    if !text.is_empty() {
        tab.cdp.call("Input.insertText", json!({ "text": text })).await?;
    }
    if submit {
        press_key(tab, "Enter", 0).await?;
        tokio::time::sleep(Duration::from_millis(300)).await;
        tab.cdp.invalidate_worlds();
    }
    Ok(())
}

/// 按键（rawKeyDown + keyUp，带修饰键位掩码）。
pub(crate) async fn press_key(tab: &WebTab, key: &str, modifiers: i64) -> Result<(), String> {
    let vk = virtual_key_code(key);
    for t in ["rawKeyDown", "keyUp"] {
        tab.cdp
            .call(
                "Input.dispatchKeyEvent",
                json!({ "type": t, "key": key, "windowsVirtualKeyCode": vk, "modifiers": modifiers }),
            )
            .await?;
    }
    Ok(())
}

/// 常用键的 Windows 虚拟键码（缺省 0 让 Chromium 按 key 名自己推）。
pub(crate) fn virtual_key_code(key: &str) -> i64 {
    match key {
        "Enter" => 13,
        "Tab" => 9,
        "Escape" => 27,
        "Backspace" => 8,
        "Delete" => 46,
        "ArrowUp" => 38,
        "ArrowDown" => 40,
        "ArrowLeft" => 37,
        "ArrowRight" => 39,
        "Home" => 36,
        "End" => 35,
        "PageUp" => 33,
        "PageDown" => 34,
        " " => 32,
        _ => 0,
    }
}

// —— 网页面板（自桌面壳落地起，帧流/驻留/dialog）——


#[cfg(test)]
mod shot_tests {
    use super::*;

    /// 截图落盘（2026-10-31「聊天界面显示图片」）：写进工作区并把相对路径转成
    /// 前端 md() 认的 /api/workspace/raw 图片地址。切到临时工作区，不碰用户目录。
    #[test]
    fn shot_saved_to_workspace_with_md_ready_url() {
        let tmp = std::env::temp_dir().join(format!("pawch-shot-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let old = crate::tools::set_workspace_root(&tmp);
        let url = save_shot_to_workspace(b"\x89PNG\r\n\x1a\nfake").expect("应落盘成功");
        crate::tools::set_workspace_root(&old);

        assert!(url.starts_with("/api/workspace/raw?path="), "URL 形态: {url}");
        assert!(url.contains(".agent"), "应落在 .agent 下: {url}");
        let dir = tmp.join(".agent").join("screenshots");
        let n = std::fs::read_dir(&dir)
            .map(|d| d.filter_map(|e| e.ok()).count())
            .unwrap_or(0);
        assert_eq!(n, 1, "应写出 1 个文件");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
