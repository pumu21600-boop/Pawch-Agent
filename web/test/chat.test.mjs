import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  appendThinking,
  settleThinking,
  appendContent,
  addToolPart,
  settleToolArgs,
  settleToolPart,
  trimParts,
  partsFromStoredMessage,
  mergeAssistantRun,
  buildBlocks,
  buildCompletedView,
  buildMessageBlocks,
  completedHasProcess,
  isNoiseThinking,
  processSummary,
  processIsLive,
  toolMeta,
  prettyArgs,
  fmtDurCn,
  attachApprovalToToolPart,
  settleApprovalOnParts,
  parseTodoArgs,
  latestTodoRun,
  latestTodoInMessages,
  filesTouchedInMessages,
  attachAskToToolPart,
  settleAskOnParts,
} from '../src/lib/chat.js';

test('审批记录挂在最后一个同名工具段上，决定回填按 id 命中', () => {
  let p = [];
  p = addToolPart(p, { name: 'shell', args: '{"command":"node x.js"}', toolIndex: 0 });
  p = addToolPart(p, { name: 'read_file', args: '{"path":"a.txt"}', toolIndex: 1 });
  p = addToolPart(p, { name: 'shell', args: '{"command":"del a.txt"}', toolIndex: 2 });
  const appr = { id: 'a1', tool: 'shell', risk: 2, decided: null };
  const p2 = attachApprovalToToolPart(p, appr);
  // 挂在最后一个 shell（同名优先），且不是引用复制
  assert.equal(p2[2].approval.id, 'a1');
  assert.ok(p2 !== p);
  assert.equal(p2[0].approval, undefined);
  // 决定回填：只有命中的那条变
  const p3 = settleApprovalOnParts(p2, 'a1', true);
  assert.equal(p3[2].approval.decided, true);
  assert.ok(p3 !== p2);
  // 未命中的 id：原引用返回
  assert.equal(settleApprovalOnParts(p2, 'nope', true), p2);
  // 没有任何工具段：原样返回（调用方据此走气泡兜底）
  const empty = attachApprovalToToolPart([{ type: 'content', text: 'hi' }], appr);
  assert.equal(empty.length, 1);
});

test('正文分片紧挨着才续写，遇到工具就开新段（保证时间线顺序）', () => {
  let p = [];
  p = appendContent(p, '先看一下');
  p = appendContent(p, '目录。');
  p = addToolPart(p, { name: 'list_dir', args: '{"path":"."}', toolIndex: 0 });
  p = appendContent(p, '目录里有三个文件。');
  assert.deepEqual(
    p.map((x) => x.type),
    ['content', 'tool', 'content']
  );
  assert.equal(p[0].text, '先看一下目录。');
  assert.equal(p[2].text, '目录里有三个文件。');
});

test('思考分片只和未结束的同一段合并；结算后新分片开新段', () => {
  let p = [];
  p = appendThinking(p, '先想');
  p = appendThinking(p, '一下。');
  assert.equal(p.length, 1);
  assert.equal(p[0].text, '先想一下。');
  p = settleThinking(p, 3200);
  assert.equal(p[0].done, true);
  assert.equal(p[0].ms, 3200);
  p = appendThinking(p, '再想');
  p = settleThinking(p, 800);
  assert.equal(p.length, 2);
  assert.equal(p[1].ms, 800);
  // 已结算的思考不会被二次结算
  p = settleThinking(p, 9999);
  assert.equal(p[1].ms, 800);
});

test('孤立标点思考（. / 。 / …）不生成新的空思考段；已有段内标点照常拼', () => {
  assert.equal(isNoiseThinking('.'), true);
  assert.equal(isNoiseThinking('。'), true);
  assert.equal(isNoiseThinking('…'), true);
  assert.equal(isNoiseThinking('  '), true);
  assert.equal(isNoiseThinking('嗯'), false);
  let p = [];
  p = appendThinking(p, '.');
  assert.equal(p.length, 0, '孤立点号不应开新段');
  p = appendThinking(p, '先想');
  p = appendThinking(p, '.');
  assert.equal(p.length, 1);
  assert.equal(p[0].text, '先想.', '已有思考段内的标点要保留');
  // 历史里存下的纯标点 reasoning 也不显示
  const stored = partsFromStoredMessage({ reasoning_content: '.', content: '答案' });
  assert.deepEqual(
    stored.map((x) => x.type),
    ['content']
  );
});

test('工具结果只结算最后一个同名 running，失败的留 fail + 预览', () => {
  let p = [];
  p = addToolPart(p, { name: 'shell', args: '{"command":"npm test"}', toolIndex: 0 });
  p = addToolPart(p, { name: 'shell', args: '{"command":"npm run build"}', toolIndex: 1 });
  p = settleToolPart(p, 'shell', false, 'exit 1');
  assert.equal(p[0].status, 'running');
  assert.equal(p[1].status, 'fail');
  assert.equal(p[1].preview, 'exit 1');
});

test('降级重试按字符数从尾部裁掉对应类型的文本', () => {
  let p = [];
  p = appendThinking(p, 'abcdef');
  p = appendContent(p, '1234567890');
  p = trimParts(p, 'content', 4);
  assert.equal(p[1].text, '123456');
  p = trimParts(p, 'thinking', 6);
  assert.equal(p.length, 1);
  assert.equal(p[0].type, 'content');
});

test('buildBlocks：连续过程合成块，正文隔断，按真实顺序排列', () => {
  const parts = [
    { type: 'thinking', text: '想', ms: 1000 },
    { type: 'tool', name: 'read_file' },
    { type: 'content', text: '第一步说明' },
    { type: 'thinking', text: '再想', ms: 2000 },
    { type: 'tool', name: 'edit_file' },
    { type: 'content', text: '最终回答' },
  ];
  const blocks = buildBlocks(parts);
  assert.deepEqual(
    blocks.map((b) => b.kind),
    ['process', 'content', 'process', 'content']
  );
  assert.equal(blocks[0].items.length, 2);
  assert.equal(blocks[1].text, '第一步说明');
});

test('buildCompletedView：完成后只留最后一段结语，执行过程全部折进一个过程块', () => {
  const parts = [
    { type: 'thinking', text: '想A', ms: 61000, done: true },
    { type: 'tool', name: 'shell' },
    { type: 'content', text: '我来。' },
    { type: 'thinking', text: '想B', ms: 4000, done: true },
    { type: 'tool', name: 'write_file' },
    { type: 'content', text: '做好了：文件已生成。' },
  ];
  const blocks = buildCompletedView(parts);
  assert.deepEqual(
    blocks.map((b) => b.kind),
    ['process', 'content']
  );
  assert.equal(blocks[0].items.length, 5, '思考/工具/中途说明都在过程块里');
  assert.equal(blocks[0].items[2].text, '我来。');
  assert.equal(blocks[1].text, '做好了：文件已生成。');
  // 简单问答没有过程，不凭空造过程块
  assert.deepEqual(buildCompletedView([{ type: 'content', text: '答案' }]), [{ kind: 'content', text: '答案' }]);
  // 纯标点思考不能撑出空过程块
  assert.deepEqual(buildCompletedView([{ type: 'thinking', text: '.' }, { type: 'content', text: '答案' }]), [
    { kind: 'content', text: '答案' },
  ]);
  // 流式入口仍按真实顺序展开
  assert.equal(buildMessageBlocks(parts, true).filter((b) => b.kind === 'process').length, 2);
});

test('completedHasProcess：有无过程块决定「已工作」显示位置（纯问答轮不丢时长）', () => {
  // 纯问答：无过程块 → 「已工作」单独显示在顶部条
  assert.equal(completedHasProcess([{ type: 'content', text: '答案' }]), false);
  // 纯标点思考撑不出过程块 → 仍是顶部条
  assert.equal(
    completedHasProcess([{ type: 'thinking', text: '.' }, { type: 'content', text: '答案' }]),
    false
  );
  // 有思考/工具 → 时长并进过程行，顶部不再单独显示
  assert.equal(
    completedHasProcess([
      { type: 'thinking', text: '想', ms: 61000, done: true },
      { type: 'content', text: '答案' },
    ]),
    true
  );
});

test('processSummary：已工作/思考时长 + 工具数；没有过程时给兜底文案', () => {
  assert.equal(
    processSummary([
      { type: 'thinking', ms: 65000 },
      { type: 'tool' },
      { type: 'tool' },
    ]),
    '思考 1 分 5 秒 · 2 个工具'
  );
  assert.equal(processSummary([{ type: 'tool' }]), '1 个工具');
  assert.equal(
    processSummary([{ type: 'thinking', ms: 65000 }, { type: 'tool' }], 822000),
    '已工作 13 分 42 秒 · 思考 1 分 5 秒 · 1 个工具'
  );
  assert.equal(processSummary([{ type: 'content', text: '中途说明' }]), '执行过程');
});

test('processIsLive：只有执行中的工具/未结算的思考才算活', () => {
  assert.equal(processIsLive([{ type: 'tool', status: 'ok' }]), false);
  assert.equal(processIsLive([{ type: 'tool', status: 'running' }]), true);
  assert.equal(processIsLive([{ type: 'thinking', done: true }]), false);
  assert.equal(processIsLive([{ type: 'thinking', done: false }]), true);
});

test('mergeAssistantRun：多步合并成一条，parts 顺序保留、toolIndex 重排', () => {
  const step1 = {
    id: 'm1',
    role: 'assistant',
    ts: 111,
    content: '先读文件。',
    thinking: '想A',
    thinkMs: 1000,
    tools: [{ name: 'read_file', args: '{"path":"a.txt"}' }],
    parts: [
      { type: 'thinking', text: '想A', ms: 1000, done: true },
      { type: 'content', text: '先读文件。' },
      { type: 'tool', toolIndex: 0, name: 'read_file', args: '{"path":"a.txt"}', status: 'ok' },
    ],
    stats: null,
    workMs: 0,
  };
  const step2 = {
    id: 'm2',
    role: 'assistant',
    ts: 222,
    content: '改完了。',
    thinking: '想B',
    thinkMs: 2000,
    tools: [{ name: 'edit_file', args: '{"path":"a.txt"}' }],
    parts: [
      { type: 'thinking', text: '想B', ms: 2000, done: true },
      { type: 'tool', toolIndex: 0, name: 'edit_file', args: '{"path":"a.txt"}', status: 'ok' },
      { type: 'content', text: '改完了。' },
    ],
    stats: { total_tokens: 10 },
    workMs: 5000,
  };
  const [m] = mergeAssistantRun([step1, step2]);
  assert.equal(m.id, 'm1');
  assert.equal(m.ts, 111);
  assert.equal(m.workMs, 5000);
  assert.deepEqual(m.stats, { total_tokens: 10 });
  assert.equal(m.content, '先读文件。改完了。');
  assert.equal(m.thinking, '想A想B');
  assert.deepEqual(
    m.parts.map((p) => p.type),
    ['thinking', 'content', 'tool', 'thinking', 'tool', 'content']
  );
  assert.equal(m.parts[4].toolIndex, 1);
  // 中间有 user 隔断则不能合并
  const list = mergeAssistantRun([step1, { role: 'user', content: '再说一句' }, step2]);
  assert.equal(list.length, 3);
});

test('mergeAssistantRun：引导(steered)对合并透明，标记插在发生位置；打断照旧拆分', () => {
  const mk = (id, ts, text, toolIndex = 0) => ({
    id,
    role: 'assistant',
    ts,
    content: text,
    tools: [{ name: 'shell', args: '{}' }],
    parts: [{ type: 'tool', toolIndex, name: 'shell', args: '{}', status: 'ok' }, { type: 'content', text }],
    workMs: 0,
  });
  const steer = { id: 's1', role: 'user', content: '为什么不用内置浏览器', steered: true, ts: 150 };
  // 引导夹在两条 assistant 中间：不拆，标记插在两段 parts 的边界
  const [m] = mergeAssistantRun([mk('a1', 100, '前半。'), steer, mk('a2', 200, '后半。', 1)]);
  assert.equal(m.id, 'a1');
  assert.deepEqual(
    m.parts.map((p) => p.type),
    ['tool', 'content', 'steer', 'tool', 'content'],
    '同一轮不再被引导拆成两半，标记在发生位置'
  );
  assert.equal(m.parts[2].text, '为什么不用内置浏览器');
  // 多条引导按序都收进来
  const [m2] = mergeAssistantRun([
    mk('b1', 100, '一。'),
    steer,
    { id: 's2', role: 'user', content: '顺便截个图', steered: true, ts: 160 },
    mk('b2', 200, '二。', 1),
  ]);
  assert.equal(m2.parts.filter((p) => p.type === 'steer').length, 2);
  // 引导后没有 assistant（轮被取消）：原样保留，不丢
  const tail = mergeAssistantRun([mk('c1', 100, '完。'), steer]);
  assert.equal(tail.length, 2);
  assert.equal(tail[1].steered, true);
  assert.equal(tail[1].content, '为什么不用内置浏览器');
  // 引导后跟的是新 user 消息：原样保留在原位
  const mid = mergeAssistantRun([steer, { role: 'user', content: '正式提问' }, mk('d1', 100, '答。')]);
  assert.equal(mid.length, 3);
  assert.equal(mid[0].steered, true);
  // 打断(interrupted)是真用户气泡：照旧拆分
  const intr = { id: 'i1', role: 'user', content: '立刻停', interrupted: true, ts: 150 };
  const split = mergeAssistantRun([mk('e1', 100, '前。'), intr, mk('e2', 200, '后。', 1)]);
  assert.equal(split.length, 3, 'interrupted 不合并');
  assert.equal(split[1].interrupted, true);
});

test('steer 分块：完成态跟随过程折叠（徽标+行内原位），流式保持位置隔断', () => {
  const parts = [
    { type: 'tool', toolIndex: 0, name: 'write_file' },
    { type: 'content', text: '改好了第一版。' },
    { type: 'steer', text: '顺便把标题改成 X' },
    { type: 'tool', toolIndex: 1, name: 'edit_file' },
    { type: 'content', text: '标题已改，全部完成。' },
  ];
  const done = buildCompletedView(parts);
  assert.deepEqual(
    done.map((b) => b.kind),
    ['process', 'content'],
    '折叠态引导不把过程行劈成两半——前后过程合成一块'
  );
  assert.deepEqual(
    done[0].items.map((p) => p.type),
    ['tool', 'content', 'steer', 'tool'],
    '引导标记留在 items 里保持真实位置（展开后行体内可见）'
  );
  assert.equal(done[0].items.filter((p) => p.type === 'steer').length, 1, '摘要徽标按这个计数');
  assert.equal(done[1].text, '标题已改，全部完成。');
  // 流式仍按真实顺序展开、引导隔断
  const live = buildBlocks(parts);
  assert.deepEqual(
    live.map((b) => b.kind),
    ['process', 'content', 'steer', 'process', 'content']
  );
  // 开头就是引导 / 只有引导没正文：进过程块渲染
  const lead = buildCompletedView([{ type: 'steer', text: '先想好' }, { type: 'content', text: '答' }]);
  assert.deepEqual(lead.map((b) => b.kind), ['process', 'content']);
  assert.equal(lead[0].items[0].type, 'steer');
  // completedHasProcess：引导进了过程块 → 已工作并入摘要行
  assert.equal(completedHasProcess([{ type: 'steer', text: '先想好' }, { type: 'content', text: '答' }]), true);
});

test('partsFromStoredMessage：思考 → 正文 → 工具，工具 index 从 0 起', () => {
  const parts = partsFromStoredMessage({
    reasoning_content: '想了想',
    reasoning_ms: 1200,
    content: '执行。',
    tool_calls: [
      { id: 't1', function: { name: 'shell', arguments: '{"command":"dir"}' } },
      { id: 't2', function: { name: 'read_file', arguments: '{"path":"a"}' } },
    ],
  });
  assert.deepEqual(
    parts.map((p) => p.type),
    ['thinking', 'content', 'tool', 'tool']
  );
  assert.equal(parts[0].ms, 1200);
  assert.equal(parts[3].toolIndex, 1);
  assert.equal(parts[2].id, 't1');
});

test('toolMeta：不把原始 JSON 糊到脸上，路径/命令进 subject', () => {
  const w = toolMeta('write_file', '{"path":"src/main.js","content":"<!DOCTYPE html>"}');
  assert.equal(w.label, 'write_file');
  assert.equal(w.subject, 'src/main.js');
  const s = toolMeta('shell', '{"command":"node --version\\n&& echo hi"}');
  assert.equal(s.label, 'shell');
  assert.equal(s.subject, 'node --version && echo hi');
  const b = toolMeta('browser_navigate', '{"url":"https://example.com"}');
  assert.equal(b.label, 'browser_navigate');
  assert.equal(b.subject, 'https://example.com');
  const unknown = toolMeta('custom_tool', '{"q":"x"}');
  assert.equal(unknown.label, 'custom_tool');
  assert.equal(unknown.subject, 'x');
  // 参数不是 JSON 时原样缩略，不崩
  const raw = toolMeta('shell', 'npm run build -- --watch');
  assert.equal(raw.label, 'shell');
  assert.equal(raw.subject, 'npm run build -- --watch');
});

test('prettyArgs：合法 JSON 展开，非法原样，超长截断', () => {
  assert.equal(prettyArgs('{"a":1}'), '{\n  "a": 1\n}');
  assert.equal(prettyArgs('{oops'), '{oops');
  assert.equal(prettyArgs(''), '');
  const long = prettyArgs(JSON.stringify({ content: 'x'.repeat(9000) }), 100);
  assert.ok(long.length < 200, String(long.length));
  assert.ok(long.includes('已截断'), long);
});

test('fmtDurCn：秒/分/分秒', () => {
  assert.equal(fmtDurCn(0), '1 秒');
  assert.equal(fmtDurCn(27000), '27 秒');
  assert.equal(fmtDurCn(65000), '1 分 5 秒');
  assert.equal(fmtDurCn(120000), '2 分');
});

// —— 任务清单卡（todo 工具，ZCode TodoWrite 同款）——
test('parseTodoArgs：合法清单解析+状态兜底，流式半截/坏参数返回 null', () => {
  const ok = parseTodoArgs('{"todos":[{"content":"读代码","status":"in_progress","priority":"high"},{"content":"跑测试","status":"pending","priority":"medium"}]}');
  assert.equal(ok.todos.length, 2);
  assert.equal(ok.todos[0].status, 'in_progress');
  // 非法状态/优先级按 pending/medium 兜底（与后端校验互补：界面不炸）
  const bad = parseTodoArgs('{"todos":[{"content":"x","status":"doing","priority":"urgent"}]}');
  assert.equal(bad.todos[0].status, 'pending');
  assert.equal(bad.todos[0].priority, 'medium');
  // 流式半截 JSON：卡不渲染
  assert.equal(parseTodoArgs('{"todos":[{"content":"读'), null);
  assert.equal(parseTodoArgs('{oops'), null);
  assert.equal(parseTodoArgs('{"todos":[]}'), null, '空清单不算有效卡');
});

test('latestTodoRun：取最后一次 todo 调用（整表替换=最新生效）', () => {
  let p = [];
  p = addToolPart(p, { name: 'todo', args: '{"todos":[{"content":"旧","status":"pending","priority":"low"}]}', toolIndex: 0 });
  p = addToolPart(p, { name: 'write_file', args: '{"path":"a.txt"}', toolIndex: 1 });
  p = addToolPart(p, { name: 'todo', args: '{"todos":[{"content":"新","status":"in_progress","priority":"high"}]}', toolIndex: 2 });
  const t = latestTodoRun(p);
  assert.equal(t.todos.length, 1);
  assert.equal(t.todos[0].content, '新');
  assert.equal(latestTodoRun([{ type: 'content', text: 'hi' }]), null);
});

test('latestTodoInMessages：跨消息取最后一次 todo，清空后不翻旧账', () => {
  const mk = (id, tool) => ({
    id,
    role: 'assistant',
    parts: tool ? [{ type: 'tool', name: 'todo', args: tool }] : [{ type: 'content', text: 'ok' }],
  });
  const a = mk('a', '{"todos":[{"content":"旧任务","status":"completed","priority":"low"}]}');
  const b = mk('b');
  assert.equal(latestTodoInMessages([a, b]).todos[0].content, '旧任务', '后面的消息没有 todo 不影响');
  const c = mk('c', '{"todos":[{"content":"新任务","status":"in_progress","priority":"high"}]}');
  assert.equal(latestTodoInMessages([a, b, c]).todos[0].content, '新任务');
  // 最后一次是空清单（清零）：返回 null，不能回退到更早的清单
  const d = mk('d', '{"todos":[]}');
  assert.equal(latestTodoInMessages([a, c, d]), null);
  // 最后一次是坏参数同理
  const e = mk('e', '{半截');
  assert.equal(latestTodoInMessages([a, e]), null);
  assert.equal(latestTodoInMessages([]), null);
});

// 流式节流回归（2026-11-03 真机实测「自始至终没看到清单」）：执行前的完整参数
// 只更新了 m.tools 没同步 parts，清单卡一直拿半截 JSON，重开会话才恢复。
test('settleToolArgs：执行前完整参数收口到 parts', () => {
  const partial = '{"todos":[{"content":"建文件夹","status":"pending","priority":"high"}';
  const full = '{"todos":[{"content":"建文件夹","status":"pending","priority":"high"},{"content":"写文件","status":"in_progress","priority":"medium"}]}';
  let p = addToolPart([], { name: 'todo', args: partial, toolIndex: 0, status: 'running' });
  assert.equal(latestTodoRun(p), null, '半截 JSON 不渲染');
  p = settleToolArgs(p, 0, 'todo', full);
  assert.equal(latestTodoRun(p).todos.length, 2, '收口后清单可解析');
  // 并行同名：只动 toolIndex 命中的那一段
  let q = addToolPart([], { name: 'todo', args: full, toolIndex: 0, status: 'running' });
  q = addToolPart(q, { name: 'todo', args: '{"todos":[{"content":"另一轮","status":"pending","priority":"low"}]}', toolIndex: 1, status: 'running' });
  const q2 = settleToolArgs(q, 0, 'todo', '{"todos":[{"content":"改过","status":"completed","priority":"high"}]}');
  assert.equal(JSON.parse(q2[0].args).todos[0].content, '改过');
  assert.equal(JSON.parse(q2[1].args).todos[0].content, '另一轮', '同名的另一段不被误改');
  // 没 toolIndex 时退回最后一个同名 running 段
  const r = settleToolArgs(q, null, 'todo', '{"todos":[{"content":"回退","status":"pending","priority":"low"}]}');
  assert.equal(JSON.parse(r[1].args).todos[0].content, '回退');
  // 没有匹配段：原样保持引用稳定
  const none = [{ type: 'content', text: 'hi' }];
  assert.equal(settleToolArgs(none, 3, 'todo', '{}'), none);
});

// —— 提问（ask 工具，ZCode AskUserQuestion 同款）——
test('提问记录挂到最后一个没挂过的 ask 工具段，回填按 id 命中', () => {
  let p = [];
  p = addToolPart(p, { name: 'ask', args: '{"question":"一","options":[]}', toolIndex: 0 });
  p = addToolPart(p, { name: 'ask', args: '{"question":"二","options":[]}', toolIndex: 1 });
  const rec = { id: 'q1', selected: ['乙'], other: '' };
  const p2 = attachAskToToolPart(p, rec);
  assert.notEqual(p2, p, '挂上要返回新数组');
  assert.equal(p2[1].ask.id, 'q1');
  assert.ok(!p2[0].ask, '只挂最后一个没挂过的');
  // 非引用复制
  assert.equal(p2[1].ask, rec);
  // 回填命中
  const p3 = settleAskOnParts(p2, 'q1', { dismissed: true });
  assert.equal(p3[1].ask.dismissed, true);
  assert.equal(p3[1].ask.id, 'q1');
  assert.equal(settleAskOnParts(p2, 'nope', { dismissed: true }), p2, '未命中保持引用稳定');
  // 挂不上（没有 ask 段）原样返回
  const q = [{ type: 'content', text: 'hi' }];
  assert.equal(attachAskToToolPart(q, rec), q);
});

test('filesTouchedInMessages：会话写入记录兜底产物清单', () => {
  const mk = (id, parts) => ({ id, role: 'assistant', parts });
  const t = (name, path) => ({ type: 'tool', name, args: JSON.stringify({ path }), status: 'ok' });
  const msgs = [
    mk('m1', [t('write_file', 'campusnet/a.py'), t('read_file', 'ignore.txt')]),
    mk('m2', [t('edit_file', 'campusnet/a.py'), t('write_file', 'campusnet\\b.json')]),
    // 坏参数、无路径、工作区外绝对路径都不列
    mk('m3', [{ type: 'tool', name: 'write_file', args: '{bad', status: 'ok' }, t('write_file', '')]),
    mk('m4', [t('write_file', 'D:/elsewhere/c.txt')]),
    // .agent/ 是内部临时脚本区，不算产物
    mk('m5', [t('write_file', '.agent/scan.ps1')]),
  ];
  const files = filesTouchedInMessages(msgs, 'C:\\Users\\x\\ws');
  assert.deepEqual(files.map((f) => f.path), ['campusnet/a.py', 'campusnet/b.json']);
  // 工作区内的绝对路径收敛成相对路径
  const abs = filesTouchedInMessages([mk('m', [t('write_file', 'C:\\Users\\x\\ws\\sub\\d.md')])], 'C:/Users/x/ws');
  assert.deepEqual(abs.map((f) => f.path), ['sub/d.md']);
  assert.equal(filesTouchedInMessages([], 'C:/ws').length, 0);
});

test('toolMeta：label 恒等于英文调用名，图标仍按族映射（2026-11-04 拍板）', () => {
  for (const name of ['search', 'read_page', 'read_skill', 'skill', 'plugin', 'session', 'settings', 'auto', 'batch', 'todo', 'ask', 'memory', 'computer']) {
    const m = toolMeta(name, '{}');
    assert.equal(m.label, name, name);
    assert.notEqual(m.iconSvg, '', name);
  }
  // ask 的问题文本进 subject
  const a = toolMeta('ask', '{"question":"选哪个？","options":[]}');
  assert.equal(a.subject, '选哪个？');
});
