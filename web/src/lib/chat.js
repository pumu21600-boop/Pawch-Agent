// 聊天展示层的纯逻辑：流式分段、历史合并、工具人话化。
//
// 为什么要单独成模块（2026-10-30 UI 轮）：
// - 一轮回复在事实源里是「一步一条 assistant」，前端展示是一条气泡；合并/分段规则
//   一错就会出现「重启后工具调用被拆成 N 个气泡」这类回归（10-30 已踩过一次）。
// - 本模块不碰 DOM，可直接用 Node 内置 test runner 单测（web/test/chat.test.mjs），
//   前端此前没有单测框架，这一层是最值得先钉死的纯逻辑。
//
// 数据形状（parts，按**真实发生顺序**）：
//   { type: 'thinking', text, ms, done }
//   { type: 'content', text }
//   { type: 'tool', toolIndex, id, name, args, status: 'running'|'ok'|'fail', preview }
// 旧消息没有 parts 时由 messageParts() 用 thinking/content/tools 兜底。

/// 「3 分 47 秒」/「27 秒」；不到 1 秒显示 1 秒（别给 0 秒这种怪数字）
export function fmtDurCn(ms) {
  const s = Math.max(1, Math.round((ms || 0) / 1000));
  if (s < 60) return `${s} 秒`;
  const m = Math.floor(s / 60);
  const r = s % 60;
  return r ? `${m} 分 ${r} 秒` : `${m} 分`;
}

/// 只有标点/空白/分隔符的「思考」分片。部分渠道会在正文后补一个 "." 的 reasoning
/// 分片，若当成新思考段会凭空多出一个空思考块（2026-10-30 真机实测截图）。
/// 注意只在**开新段**时丢弃；已有思考段里的小标点是句子的正常组成，照常拼接。
export function isNoiseThinking(text) {
  const t = String(text || '').trim();
  if (!t) return true;
  return /^[\p{P}\p{Z}\s]+$/u.test(t);
}

/// 思考分片：紧挨着同一段思考就续写，遇到正文/工具就开新段
export function appendThinking(parts, text) {
  if (!text) return parts || [];
  const arr = (parts || []).slice();
  const last = arr[arr.length - 1];
  if (last && last.type === 'thinking' && !last.done) {
    arr[arr.length - 1] = { ...last, text: last.text + text };
  } else if (!isNoiseThinking(text)) {
    arr.push({ type: 'thinking', text, ms: 0, done: false });
  }
  return arr;
}

/// 本段思考结束（正文到达 / 工具调用 / 轮收尾）：把阶段跨度记到这一段上
export function settleThinking(parts, ms) {
  if (!parts || !parts.length) return parts || [];
  let idx = -1;
  for (let i = parts.length - 1; i >= 0; i--) {
    if (parts[i].type === 'thinking' && !parts[i].done) {
      idx = i;
      break;
    }
  }
  if (idx < 0) return parts;
  const arr = parts.slice();
  arr[idx] = { ...arr[idx], ms: (arr[idx].ms || 0) + Math.max(0, ms || 0), done: true };
  return arr;
}

/// 正文分片：紧挨着同一段正文就续写（工具/思考之后是新的一段）
export function appendContent(parts, text) {
  if (!text) return parts || [];
  const arr = (parts || []).slice();
  const last = arr[arr.length - 1];
  if (last && last.type === 'content') {
    arr[arr.length - 1] = { ...last, text: last.text + text };
  } else {
    arr.push({ type: 'content', text });
  }
  return arr;
}

/// 新增一个工具段。toolIndex 与消息的 tools 数组下标一致（折叠状态沿用旧键）
export function addToolPart(parts, tool) {
  const arr = (parts || []).slice();
  arr.push({
    type: 'tool',
    toolIndex: tool.toolIndex ?? 0,
    id: tool.id || '',
    name: tool.name || '?',
    args: tool.args || '',
    status: tool.status || 'running',
    preview: tool.preview || '',
  });
  return arr;
}

/// 审批记录挂到工具段上（审批卡不钉在气泡顶部，已决定的
/// 记录回到时间线里它发生的位置——即触发它的那个工具卡下面）。
/// approval_request 紧跟 tool_call 到达：优先挂最后一个同名且没挂过审批的工具段，
/// 找不到再退最后一个没挂过的；都找不到就原样返回（调用方以返回值是否变化判断 inFlow）。
export function attachApprovalToToolPart(parts, appr) {
  if (!parts || !appr) return parts || [];
  const arr = parts.slice();
  for (let i = arr.length - 1; i >= 0; i--) {
    const p = arr[i];
    if (p.type === 'tool' && p.name === appr.tool && !p.approval) {
      arr[i] = { ...p, approval: appr };
      return arr;
    }
  }
  for (let i = arr.length - 1; i >= 0; i--) {
    if (arr[i].type === 'tool' && !arr[i].approval) {
      arr[i] = { ...arr[i], approval: appr };
      return arr;
    }
  }
  return parts;
}

/// 审批决定回填：按审批 id 更新工具段上的快照（没命中就原样返回，保持引用稳定）
export function settleApprovalOnParts(parts, id, decided) {
  if (!parts) return parts || [];
  let hit = false;
  const arr = parts.map((p) => {
    if (p.type === 'tool' && p.approval && p.approval.id === id) {
      hit = true;
      return { ...p, approval: { ...p.approval, decided } };
    }
    return p;
  });
  return hit ? arr : parts;
}

/// 工具参数收口：把执行前的完整参数同步到展示层 parts。
/// 流式 tool_call_args 有节流（新增 ≥32 字节或 ≥250ms 才发），末尾一小段可能从没发过；
/// 执行循环再发 ToolCall 时才是完整参数——只更新 m.tools 会让按参数解析的展示
/// （todo 清单卡）一直拿着半截 JSON 解析失败，直到重开会话才恢复（2026-11-03 真机实测）。
/// 优先按 toolIndex 精确匹配（并行同名调用不串段），没有就退最后一个同名 running 段。
export function settleToolArgs(parts, toolIndex, name, args) {
  if (!parts || !parts.length) return parts || [];
  const arr = parts.slice();
  let hit = -1;
  if (typeof toolIndex === 'number') {
    for (let i = arr.length - 1; i >= 0; i--) {
      const p = arr[i];
      if (p.type === 'tool' && p.toolIndex === toolIndex) { hit = i; break; }
    }
  }
  if (hit < 0) {
    for (let i = arr.length - 1; i >= 0; i--) {
      const p = arr[i];
      if (p.type === 'tool' && p.name === name && p.status === 'running') { hit = i; break; }
    }
  }
  if (hit < 0) return parts;
  arr[hit] = { ...arr[hit], args: args || '' };
  return arr;
}

/// 工具结果到达：只结算最后一个同名、仍在执行的工具段
export function settleToolPart(parts, name, ok, preview) {
  if (!parts || !parts.length) return parts || [];
  const arr = parts.slice();
  for (let i = arr.length - 1; i >= 0; i--) {
    const p = arr[i];
    if (p.type === 'tool' && p.name === name && p.status === 'running') {
      arr[i] = { ...p, status: ok ? 'ok' : 'fail', preview: preview || '' };
      break;
    }
  }
  return arr;
}

/// 降级链换渠道重试：按服务端给的字符数，从尾部裁掉 content/thinking
export function trimParts(parts, kind, n) {
  if (!parts || !n) return parts || [];
  let remain = n;
  const arr = parts.slice();
  for (let i = arr.length - 1; i >= 0 && remain > 0; i--) {
    if (arr[i].type !== kind) continue;
    const text = arr[i].text || '';
    const cut = Math.min(remain, text.length);
    remain -= cut;
    const left = text.slice(0, text.length - cut);
    if (left) arr[i] = { ...arr[i], text: left };
    else arr.splice(i, 1);
  }
  return arr;
}

/// 从落盘的一条 assistant 消息生成 parts。
/// 一步内的顺序按「思考 → 正文 → 工具调用」——正文通常是调工具前的说明。
export function partsFromStoredMessage(m) {
  const parts = [];
  if (m.reasoning_content && !isNoiseThinking(m.reasoning_content)) {
    parts.push({ type: 'thinking', text: m.reasoning_content, ms: m.reasoning_ms || 0, done: true });
  }
  if (m.content) parts.push({ type: 'content', text: m.content });
  (m.tool_calls || []).forEach((tc, i) => {
    parts.push({
      type: 'tool',
      toolIndex: i,
      id: tc.id || '',
      name: tc.function?.name || '?',
      args: tc.function?.arguments || '',
      status: 'ok',
      preview: '',
    });
  });
  return parts;
}

/// 旧形状（没有 parts）兜底：思考 → 工具 → 正文（与旧版展示一致）
export function legacyParts(m) {
  const parts = [];
  if (m.thinking) parts.push({ type: 'thinking', text: m.thinking, ms: m.thinkMs || 0, done: true });
  (m.tools || []).forEach((t, i) => {
    parts.push({
      type: 'tool',
      toolIndex: i,
      id: t.id || '',
      name: t.name || '?',
      args: t.args || '',
      status: t.status || 'ok',
      preview: t.preview || '',
    });
  });
  if (m.content) parts.push({ type: 'content', text: m.content });
  return parts;
}

/// 取消息的 parts（模板/合并统一入口）
export function messageParts(m) {
  return m.parts && m.parts.length ? m.parts : legacyParts(m);
}

/// 同一轮连续 assistant 并成一个气泡。落盘一步一条是事实源，展示层合并。
/// 合并规则：content/thinking 拼接、tools 依序合并、parts 依序拼接并重排 toolIndex、
/// thinkMs 累加；workMs/stats 取收尾那条；ts/id 留第一条。
/// 引导消息（steered 用户消息）对合并**透明**（2026-10-31 实测：引导在会话文件里
/// 是真 user——模型上下文需要它，落盘不动；但展示层不能让它把同一轮拆成两半）。
/// 折叠成 {type:'steer'} part 插进两段 parts 的**边界**——标记就显示在引导发生的
/// 位置（不该挪到轮顶）。后面没等到 assistant 的原样保留。
/// 打断（interrupted）是真用户气泡，照旧拆分。
export function mergeAssistantRun(list) {
  const out = [];
  let pending = []; // 待插入的引导标记（原样 user 消息）
  const takeParts = () => {
    const parts = pending.map((s) => ({ type: 'steer', text: s.content, ts: s.ts }));
    pending = [];
    return parts;
  };
  const flushPending = () => {
    for (const s of pending) out.push(s);
    pending = [];
  };
  for (const m of list) {
    if (m.role === 'user' && m.steered) {
      pending.push(m);
      continue;
    }
    const prev = out[out.length - 1];
    if (m.role === 'assistant' && prev && prev.role === 'assistant') {
      const prevTools = prev.tools || [];
      const prevParts = (prev.parts && prev.parts.length ? prev.parts : legacyParts(prev)).map((p) => ({ ...p }));
      const steerParts = takeParts();
      const mergedParts = [...prevParts, ...steerParts];
      for (const p of m.parts && m.parts.length ? m.parts : legacyParts(m)) {
        mergedParts.push(
          p.type === 'tool' ? { ...p, toolIndex: prevTools.length + (p.toolIndex || 0) } : { ...p }
        );
      }
      out[out.length - 1] = {
        ...prev,
        content: (prev.content || '') + (m.content || ''),
        thinking: (prev.thinking || '') + (m.thinking || ''),
        tools: [...prevTools, ...(m.tools || [])],
        parts: mergedParts,
        thinkMs: (prev.thinkMs || 0) + (m.thinkMs || 0),
        workMs: m.workMs || prev.workMs,
        stats: m.stats || prev.stats,
      };
    } else if (m.role === 'assistant') {
      const steerParts = takeParts();
      const own = m.parts && m.parts.length ? m.parts.map((p) => ({ ...p })) : legacyParts(m);
      out.push(steerParts.length ? { ...m, parts: [...steerParts, ...own] } : m);
    } else {
      flushPending();
      out.push(m);
    }
  }
  flushPending();
  return out;
}

/// 目标消息在会话文件里的「第几条 user 消息」序数（0 基）——回滚/单条删除/编辑重发
/// 按它向服务端定位截断点（删文件里第 N 条 user 及其后）。前提：视图与文件的 user
/// 行一一对应——
/// - 排队气泡没进文件 → 排除；
/// - 引导行在文件里是真 user：实时视图的 steered 用户行数 1；重建视图里它被
///   [mergeAssistantRun] 透明化成 assistant 的 {type:'steer'} part（2026-10-31 展示层
///   拍板，不占行位）→ **part 也要数回来**。漏数 = 序数整体少 1，回滚把前一步连带
///   滚掉（真机 2026-11-08 两次实锤：一次回滚冲掉两轮问答）。
/// 新增「文件有、视图无行」的消息形态时，这里必须同步计入。
export function fileUserIndexOf(list, id) {
  const at = list.findIndex((x) => x && x.id === id);
  if (at < 0) return 0;
  let n = 0;
  for (let k = 0; k < at; k++) {
    const x = list[k];
    if (x.role === 'user' && !x.queued) n++;
    else if (x.role === 'assistant') {
      for (const p of x.parts || []) if (p && p.type === 'steer') n++;
    }
  }
  return n;
}

/// 刷新/重载恢复排队快照时，剔除**其实已经送达**的条目（2026-11-12 真机）。
/// 快照可能比出队旧：消息在出队时已经作为正式 user 消息发出（会话文件/消息数组
/// 里有它），outbox 里那份却还留着——刷新后铺回一个「排队中」气泡，用户点
/// 「引导/立即发送」就会把同一句话再投一遍（截图里同一问题出现两份的来路）。
/// 判据：同文本、非排队/非引导的正式用户消息，时间戳不早于条目的排队时间
/// （排队在前、送达在后）。排队时间比正式消息晚的保留——那是用户之后又排了
/// 一句同文消息，属于合法重复，不能吞。
export function outboxNotYetSent(items, msgs) {
  const text = (c) => (typeof c === 'string' ? c : Array.isArray(c) ? c.map((p) => (p && p.text) || '').join('') : '');
  const list = msgs || [];
  return (items || []).filter((it) => {
    if (!it || !it.id) return false;
    const t = typeof it.text === 'string' ? it.text : '';
    if (!t) return true;
    return !list.some(
      (m) =>
        m &&
        m.role === 'user' &&
        !m.queued &&
        text(m.content) === t &&
        Number(m.ts || 0) >= Number(it.ts || 0)
    );
  });
}

/// 工具结果回填到历史 parts（按 tool_call_id 精确匹配）
export function attachToolPreview(parts, toolCallId, preview) {
  if (!parts || !toolCallId) return parts || [];
  const idx = parts.findIndex((p) => p.type === 'tool' && p.id === toolCallId);
  if (idx < 0) return parts;
  const arr = parts.slice();
  arr[idx] = { ...arr[idx], preview: preview || '' };
  return arr;
}

// —— 任务清单卡 / 提问卡（2026-11-03，ZCode TodoWrite / AskUserQuestion 同款）——

const TODO_STATUS = ['pending', 'in_progress', 'completed'];
const TODO_PRIORITY = ['high', 'medium', 'low'];

/// 解析 todo 工具参数为条目数组：合法 JSON 且 todos 是数组 → 数组（**可能为空**，
/// 空数组=模型清零清单，调用方要区分）；半截 JSON / 没有 todos 键 → null。
function parseTodoList(args) {
  try {
    const v = JSON.parse(args);
    if (!v || !Array.isArray(v.todos)) return null;
    return v.todos
      .filter((t) => t && typeof t.content === 'string' && t.content.trim())
      .map((t) => ({
        content: String(t.content),
        status: TODO_STATUS.includes(t.status) ? t.status : 'pending',
        priority: TODO_PRIORITY.includes(t.priority) ? t.priority : 'medium',
      }));
  } catch {
    return null;
  }
}

/// 解析 todo 工具参数为清单。参数是流式 JSON（可能半截），解析失败或空清单返回
/// null（卡不渲染、dock 不显示）。
export function parseTodoArgs(args) {
  const list = parseTodoList(args);
  return list && list.length ? { todos: list } : null;
}

/// 消息里最后一次 todo 调用的清单（清单语义=最新一次整表替换生效；无则 null）
export function latestTodoRun(parts) {
  const arr = parts || [];
  for (let i = arr.length - 1; i >= 0; i--) {
    const p = arr[i];
    if (p && p.type === 'tool' && p.name === 'todo') {
      const t = parseTodoArgs(p.args);
      if (t) return t;
    }
  }
  return null;
}

/// 最近一次 todo 调用：清单（可能为空数组=清零）+ 判同指纹。指纹=条目内容拼接
/// （**不含状态**）——任务清单 dock 拿它判「是不是一次新的清单调用」：叉掉/手动
/// 收起的手动态只能被跨轮的**新条目**重置，不能被「发了条消息」或「条目只更新了
/// 状态」冲掉（2026-11-13 真机：叉掉后下一条消息又冒出来）。不用消息 id 当指纹：
/// 重建视图（刷新/回滚/对账）会换 id，会把叉掉的手动态误判成新调用。
/// 向前扫到第一份**可解析**的参数为止：半截 JSON 回退上一份完整清单（流式期
/// dock 不能闪没）；显式空清单原样返回（清零语义，调用方负责藏 dock）。
export function latestTodoCall(messages) {
  const arr = messages || [];
  for (let i = arr.length - 1; i >= 0; i--) {
    const m = arr[i];
    if (!m || m.role !== 'assistant') continue;
    const parts = messageParts(m);
    for (let j = parts.length - 1; j >= 0; j--) {
      const p = parts[j];
      if (p && p.type === 'tool' && p.name === 'todo') {
        const todos = parseTodoList(p.args);
        if (todos !== null) return { ref: todos.map((t) => t.content).join('\n'), todos };
      }
    }
  }
  return null;
}

export function latestTodoInMessages(messages) {
  const hit = latestTodoCall(messages);
  return hit && hit.todos.length ? { todos: hit.todos } : null;
}

/// 会话里 agent 写过/改过的文件（按首次出现顺序去重）。
/// 非 git 工作区 / 检查点历史缺失时，产物面板用它兜底列出「这次会话产出过什么」
/// （2026-11-03 真机实测：老会话的检查点没了，产物列表一片空）。
/// 只认 write_file/edit_file 的目标路径；坏参数、工作区外的绝对路径跳过。
export function filesTouchedInMessages(messages, workspaceRoot = '') {
  const ws = String(workspaceRoot || '').replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase();
  const seen = new Map();
  for (const m of messages || []) {
    if (!m || m.role !== 'assistant') continue;
    for (const p of messageParts(m)) {
      if (!p || p.type !== 'tool' || (p.name !== 'write_file' && p.name !== 'edit_file')) continue;
      let v;
      try {
        v = JSON.parse(p.args);
      } catch {
        continue;
      }
      const raw = v && typeof v.path === 'string' ? v.path.trim() : '';
      if (!raw) continue;
      let path = raw.replace(/\\/g, '/').replace(/^\.\//, '');
      const low = path.toLowerCase();
      if (ws && low.startsWith(ws + '/')) path = path.slice(ws.length + 1);
      else if (/^[a-z]:\//i.test(path) || path.startsWith('/')) continue; // 工作区外绝对路径
      if (!path) continue;
      // .agent/ 是 agent 的临时脚本/诊断区，不算交付产物
      if (path.toLowerCase().startsWith('.agent/')) continue;
      if (!seen.has(path)) seen.set(path, { path, status: 'M', additions: null, deletions: null });
    }
  }
  return [...seen.values()];
}

/// 提问记录挂到工具段上（答案从提问卡提交后，回到时间线里它发生的位置——
/// 与 attachApprovalToToolPart 同构）。挂到最后一个没挂过记录的 ask 工具段，
/// 挂不上原样返回。
export function attachAskToToolPart(parts, record) {
  if (!parts || !record) return parts || [];
  const arr = parts.slice();
  for (let i = arr.length - 1; i >= 0; i--) {
    const p = arr[i];
    if (p.type === 'tool' && p.name === 'ask' && !p.ask) {
      arr[i] = { ...p, ask: record };
      return arr;
    }
  }
  return parts;
}

/// 提问记录回填：按提问 id 更新工具段上的快照（没命中就原样返回，保持引用稳定）
export function settleAskOnParts(parts, id, payload) {
  if (!parts) return parts || [];
  let hit = false;
  const arr = parts.map((p) => {
    if (p.type === 'tool' && p.ask && p.ask.id === id) {
      hit = true;
      return { ...p, ask: { ...p.ask, ...payload } };
    }
    return p;
  });
  return hit ? arr : parts;
}

/// 流式渲染分块：连续的非正文（思考/工具）合成一个「过程」块，正文独立成块。
/// 这样多步任务按真实顺序呈现：过程 → 说明文字 → 过程 → 最终回答。
/// 引导标记（steer）也是边界：把过程块隔断，渲染成「已引导」行。
export function buildBlocks(parts) {
  const blocks = [];
  let proc = null;
  for (const p of parts || []) {
    // 纯标点思考（渠道补的 "."）不渲染，也不让它单独撑出一个过程块
    if (p.type === 'thinking' && isNoiseThinking(p.text)) continue;
    if (p.type === 'content') {
      if (p.text) blocks.push({ kind: 'content', text: p.text });
      proc = null;
    } else if (p.type === 'steer') {
      // 引导标记**留在当前过程块内**（与完成态 buildCompletedView 同口径：折叠进
      // items，展开可见、收起随块折叠）——不劈出独立块：流式期间它挂在过程块外
      // 看着像掉队的消息（真机 2026-11-08「引导条抛出去」）。
      if (!proc) {
        proc = { kind: 'process', items: [] };
        blocks.push(proc);
      }
      proc.items.push(p);
    } else {
      if (!proc) {
        proc = { kind: 'process', items: [] };
        blocks.push(proc);
      }
      proc.items.push(p);
    }
  }
  return blocks.filter((b) => b.kind !== 'process' || b.items.length > 0);
}

/// 完成态渲染：整轮的执行过程（思考/工具/中途说明）折成折叠过程块，
/// 只保留最后一段正文作为结语（「任务执行完毕只保留『做好了』下面的结语」）。
/// 引导标记（steer）**跟着过程一起折叠**：留在 items 里保持
/// 真实位置（展开后行体内可见），不把折叠行劈成两半——摘要行出「已引导」小徽标。
export function buildCompletedView(parts) {
  const list = (parts || []).filter(
    (p) => !(p.type === 'thinking' && isNoiseThinking(p.text)) && !(p.type === 'content' && !String(p.text || '').trim())
  );
  let finalIdx = -1;
  for (let i = list.length - 1; i >= 0; i--) {
    if (list[i].type === 'content') {
      finalIdx = i;
      break;
    }
  }
  const blocks = [];
  let proc = [];
  const flushProc = () => {
    if (proc.length) blocks.push({ kind: 'process', items: proc });
    proc = [];
  };
  for (let i = 0; i < list.length; i++) {
    const p = list[i];
    if (i === finalIdx) {
      flushProc();
      blocks.push({ kind: 'content', text: p.text });
    } else {
      proc.push(p);
    }
  }
  flushProc();
  return blocks;
}

/// 模板统一入口：流式按真实顺序展开，完成态折叠成「过程 + 结语」
export function buildMessageBlocks(parts, streaming) {
  return streaming ? buildBlocks(parts) : buildCompletedView(parts);
}

/// 完成态有没有「过程」折叠块——决定「已工作」时长显示在哪：
/// 有过程块时并进这一行（2026-10-30 定稿）；纯问答轮（无思考/无工具）没有
/// 过程块，就单独显示同样的行（不可展开），时长不能凭空消失（2026-10-31 真机实测）。
export function completedHasProcess(parts) {
  return buildCompletedView(parts).some((b) => b.kind === 'process');
}

/// 过程块是否还在跑：有执行中的工具，或还有没结算的思考段。
/// 只给「当前这一块」打活点——同一轮里已完成的旧过程块不该继续闪。
export function processIsLive(items) {
  return (items || []).some(
    (i) => (i.type === 'tool' && i.status === 'running') || (i.type === 'thinking' && !i.done)
  );
}

/// 过程块的一行说明：「已工作 13 分 42 秒 · 思考 6 分 27 秒 · 9 个工具」
/// workMs 只在完成态传入（把顶部的「已工作」并进这一行，少一行噪音）。
export function processSummary(items, workMs = 0) {
  const list = items || [];
  const thinkMs = list.filter((i) => i.type === 'thinking').reduce((s, i) => s + (i.ms || 0), 0);
  const tools = list.filter((i) => i.type === 'tool').length;
  const bits = [];
  if (workMs > 0) bits.push(`已工作 ${fmtDurCn(workMs)}`);
  if (thinkMs > 0) bits.push(`思考 ${fmtDurCn(thinkMs)}`);
  else if (list.some((i) => i.type === 'thinking')) bits.push('思考');
  if (tools) bits.push(`${tools} 个工具`);
  if (!bits.length) bits.push('执行过程');
  return bits.join(' · ');
}

// —— 工具行 ——
// 2026-11-04 定稿：label 直接用英文调用名（中文标签有歧义：操作界面/操作电脑、
// 读取文件/读取大文件），TOOL_RULES 只负责选图标；subject 仍是对象（路径/命令/URL），
// 原始参数 JSON 只进展开详情，不糊在行上。
const TOOL_RULES = [
  [/^(write_file|write)$/i, '写入文件', 'write'],
  [/^(edit_file|edit)$/i, '修改文件', 'write'],
  [/^(read_file|read)$/i, '读取文件', 'read'],
  [/^(list_dir|list_files|glob|search_files)$/i, '浏览文件', 'folder'],
  [/^(shell|bash|run_command|exec)$/i, '执行命令', 'terminal'],
  [/^web_search$/i, '搜索网页', 'globe'],
  [/^web_fetch$/i, '抓取网页', 'globe'],
  [/^browser/i, '浏览器', 'browser'],
  [/^memory/i, '记忆', 'brain'],
  [/^schedule/i, '定时任务', 'calendar'],
  [/^spawn_subagent|^subagent/i, '子代理', 'bot'],
  [/^computer/i, '操作电脑', 'screen'],
  [/^xlsx/i, 'Excel', 'sheet'],
  [/^pptx/i, '演示文稿', 'slides'],
  [/^docx/i, 'Word 文档', 'doc'],
  [/^pdf/i, 'PDF', 'doc'],
  [/^git/i, 'Git', 'git'],
  [/^mcp__/i, 'MCP 工具', 'plug'],
  [/^wasm__/i, '插件', 'plug'],
  // 内置管理类工具（2026-11-04 补齐：此前漏映射，聊天工具行漏英文原名）
  [/^search$/i, '搜索文件', 'folder'],
  [/^read_page$/i, '读取大文件', 'read'],
  [/^read_skill$/i, '读取技能', 'read'],
  [/^skill$/i, '技能管理', 'doc'],
  [/^plugin$/i, '插件管理', 'plug'],
  [/^session$/i, '会话管理', 'doc'],
  [/^settings$/i, '改设置', 'tool'],
  [/^auto$/i, '操作界面', 'screen'],
  [/^batch$/i, '批量调用', 'tool'],
  [/^todo$/i, '任务清单', 'list'],
  [/^ask$/i, '向用户提问', 'help'],
];

const SUBJECT_KEYS = [
  'path',
  'file_path',
  'file',
  'filename',
  'target',
  'command',
  'cmd',
  'query',
  'question',
  'url',
  'pattern',
  'action',
  'title',
  'task',
  'prompt',
  'name',
];

export function parseArgs(args) {
  if (args && typeof args === 'object') return args;
  if (typeof args !== 'string' || !args.trim()) return null;
  try {
    const v = JSON.parse(args);
    return v && typeof v === 'object' ? v : null;
  } catch {
    return null;
  }
}

function cleanSubject(v) {
  return String(v).replace(/\s+/g, ' ').trim();
}

// 半截 JSON 主语提取：流式参数从名字到达到流完还有好几秒，完整 JSON.parse 一直
// 失败，行上只剩一截原始 JSON 碎片冻在 110 字上（真机 2026-11-07「一直执行中，
// 请求和结果最后闪现」）。按主语键从参数串里抽值，让行尽早显示干净的主语（如
// 路径）并随流补全：先认已闭引号的完整值；都没有就认**正在写**的值（没闭引号），
// 主语跟着流一个字一个字长出来——「请求内容也是流式的」。
function partialSubject(raw) {
  if (!raw) return null;
  for (const k of SUBJECT_KEYS) {
    const m = raw.match(new RegExp('"' + k + '"\\s*:\\s*"((?:[^"\\\\]|\\\\.)*)"'));
    if (m && m[1].trim()) {
      let v;
      try { v = JSON.parse('"' + m[1] + '"'); } catch { v = m[1]; }
      return cleanSubject(v);
    }
  }
  // 值还在流（引号没闭）：抓到哪算哪，下一片 SSE 会更长
  for (const k of SUBJECT_KEYS) {
    const m = raw.match(new RegExp('"' + k + '"\\s*:\\s*"((?:[^"\\\\]|\\\\.)*)$'));
    if (m && m[1].trim()) {
      let v;
      try { v = JSON.parse('"' + m[1] + '"'); } catch { v = m[1]; }
      return cleanSubject(v);
    }
  }
  return null;
}

function subjectOf(args) {
  const o = parseArgs(args);
  if (!o) {
    const raw = typeof args === 'string' ? args : '';
    const partial = partialSubject(raw);
    if (partial) return partial.length > 110 ? partial.slice(0, 110) + '…' : partial;
    const fallback = cleanSubject(raw);
    return fallback.length > 110 ? fallback.slice(0, 110) + '…' : fallback;
  }
  for (const k of SUBJECT_KEYS) {
    const v = o[k];
    if (typeof v === 'string' && v.trim()) {
      const s = cleanSubject(v);
      return s.length > 110 ? s.slice(0, 110) + '…' : s;
    }
  }
  for (const v of Object.values(o)) {
    if (typeof v === 'string' && v.trim() && v.length <= 120 && !v.includes('\n')) {
      return cleanSubject(v);
    }
  }
  return '';
}

export function toolMeta(name, args) {
  const n = String(name || '');
  const label = n || '工具';
  let icon = 'tool';
  for (const [re, , ic] of TOOL_RULES) {
    if (re.test(n)) { icon = ic; break; }
  }
  let subject = subjectOf(args);
  // shell 的多行命令：压成一行，保留可读的第一段
  if (/^(shell|bash|run_command|exec)$/i.test(n) && subject) {
    subject = subject.replace(/\s+/g, ' ').trim();
    if (subject.length > 90) subject = subject.slice(0, 90) + '…';
  }
  return { label, subject, icon, iconSvg: toolIcon(icon) };
}

/// 参数按 JSON 展开（失败就原样）；超长只截预览，避免 50KB 的 write_file 参数
/// 把展开面板撑爆（完整内容本来也不适合在气泡里读）。
export function prettyArgs(args, limit = 6000) {
  if (args == null || args === '') return '';
  let out;
  if (typeof args === 'object') {
    try {
      out = JSON.stringify(args, null, 2);
    } catch {
      out = String(args);
    }
  } else {
    try {
      out = JSON.stringify(JSON.parse(args), null, 2);
    } catch {
      out = String(args);
    }
  }
  if (out.length > limit) {
    return out.slice(0, limit) + `\n…（已截断，共 ${out.length} 字符）`;
  }
  return out;
}

/// 静态 SVG（只来自本模块的固定字符串，不含任何用户输入）
export function toolIcon(kind) {
  const wrap = (body) =>
    `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">${body}</svg>`;
  switch (kind) {
    case 'write':
      return wrap('<path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z"/>');
    case 'read':
      return wrap('<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8Z"/><path d="M14 3v5h5"/>');
    case 'folder':
      return wrap('<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"/>');
    case 'terminal':
      return wrap('<path d="m5 7 5 5-5 5"/><path d="M13 17h6"/>');
    case 'globe':
      return wrap('<circle cx="12" cy="12" r="8.5"/><path d="M3.5 12h17M12 3.5c2.6 2.3 3.9 5.1 3.9 8.5s-1.3 6.2-3.9 8.5c-2.6-2.3-3.9-5.1-3.9-8.5s1.3-6.2 3.9-8.5Z"/>');
    case 'browser':
      return wrap('<rect x="3" y="4.5" width="18" height="15" rx="2"/><path d="M3 9h18"/><path d="M7 6.7h.01M10 6.7h.01"/>');
    case 'brain':
      return wrap('<path d="M12 5.5a3 3 0 0 0-3 3c0 .7.2 1.3.6 1.8a2.7 2.7 0 0 0 1 4.9h2.8a2.7 2.7 0 0 0 1-4.9c.4-.5.6-1.1.6-1.8a3 3 0 0 0-3-3Z"/><path d="M9.5 18.5h5"/>');
    case 'calendar':
      return wrap('<rect x="3.5" y="5" width="17" height="15" rx="2"/><path d="M8 3v4M16 3v4M3.5 10h17"/>');
    case 'bot':
      return wrap('<rect x="4" y="8" width="16" height="11" rx="2.5"/><path d="M12 5v3M8.5 12.5h.01M15.5 12.5h.01"/>');
    case 'screen':
      return wrap('<rect x="3" y="4.5" width="18" height="12" rx="2"/><path d="M9 20h6M12 16.5V20"/>');
    case 'sheet':
      return wrap('<rect x="3.5" y="4" width="17" height="16" rx="2"/><path d="M3.5 9h17M9 9v11M15 9v11"/>');
    case 'slides':
      return wrap('<rect x="3.5" y="4" width="17" height="12" rx="2"/><path d="M12 16v4M8.5 20h7"/>');
    case 'doc':
      return wrap('<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8Z"/><path d="M14 3v5h5M9 13h6M9 16.5h4"/>');
    case 'git':
      return wrap('<circle cx="6.5" cy="6" r="2.2"/><circle cx="6.5" cy="18" r="2.2"/><circle cx="17.5" cy="12" r="2.2"/><path d="M6.5 8.2v7.6M8.7 6h4.1a2.5 2.5 0 0 1 2.5 2.5v1.3"/>');
    case 'plug':
      return wrap('<path d="M12 3v4M8 3v4M16 3v4"/><path d="M6 7h12v4a6 6 0 0 1-6 6 6 6 0 0 1-6-6Z"/><path d="M12 17v4"/>');
    case 'list':
      return wrap('<path d="M9 6h11M9 12h11M9 18h11"/><path d="M4 6h.01M4 12h.01M4 18h.01"/>');
    case 'help':
      return wrap('<circle cx="12" cy="12" r="9"/><path d="M9.4 9.2a2.7 2.7 0 0 1 5.2.9c0 1.8-2.6 2.2-2.6 3.6"/><path d="M12 17h.01"/>');
    default:
      return wrap('<path d="M14.7 6.3a4 4 0 0 0-5.4 5.4L3 18l3 3 6.3-6.3a4 4 0 0 0 5.4-5.4l-2.4 2.4-2.4-2.4Z"/>');
  }
}
