import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ownBadge, gitStatusLabel } from '../src/lib/format.js';
import { parseToolStats, settleToolPart } from '../src/lib/chat.js';

// 工具结果前缀 [pawch-diff +A -D]：前端画行尾 +A -D，预览文本去掉前缀
test('parseToolStats：解析前缀并剥离', () => {
  assert.deepEqual(parseToolStats('[pawch-diff +12 -3] 已编辑 src/a.txt：2 处替换'), {
    adds: 12,
    dels: 3,
    text: '已编辑 src/a.txt：2 处替换',
  });
  assert.deepEqual(parseToolStats('普通结果'), { adds: null, dels: null, text: '普通结果' });
});

test('settleToolPart：增删数写进工具段，预览不留前缀', () => {
  const parts = [{ type: 'tool', name: 'edit_file', status: 'running', preview: '' }];
  const out = settleToolPart(parts, 'edit_file', true, '[pawch-diff +2 -1] 已编辑 a.txt');
  assert.equal(out[0].adds, 2);
  assert.equal(out[0].dels, 1);
  assert.equal(out[0].preview, '已编辑 a.txt');
  assert.equal(out[0].status, 'ok');
});

// git 面板逐行「提交归属」：未提交 > stash > 最近回滚 > 已提交 > 未跟踪
test('ownBadge：工作区未提交文件标未提交', () => {
  const dirty = new Set(['a.java', 'new.txt']);
  assert.equal(ownBadge(true, 'a.java', dirty, new Set(), new Set()), '未提交');
  assert.equal(ownBadge(false, 'new.txt', dirty, new Set(), new Set()), '未提交');
});

test('ownBadge：不在脏集合里按 HEAD 跟踪状态归类', () => {
  // 任务中新建后已在真仓库提交的文件：任务档清单里仍是 '?'，标已提交
  assert.equal(ownBadge(true, 'b.txt', new Set(), new Set(), new Set()), '已提交');
  // 被 ignore / 从未提交进 HEAD 的文件：不能冒充已提交
  assert.equal(ownBadge(false, 'c.txt', new Set(), new Set(), new Set()), '未跟踪');
  assert.equal(ownBadge(true, 'd.txt', new Set(), new Set(['d.txt']), new Set()), 'stash');
  assert.equal(ownBadge(true, 'e.txt', new Set(), new Set(), new Set(['e.txt'])), '回滚');
});

test('ownBadge：当前未提交压过 stash/回滚标签（可操作状态优先）', () => {
  const dirty = new Set(['x.txt']);
  assert.equal(ownBadge(true, 'x.txt', dirty, new Set(['x.txt']), new Set(['x.txt'])), '未提交');
});

test('gitStatusLabel 原有口径不变', () => {
  assert.equal(gitStatusLabel('??'), '未跟踪');
  assert.equal(gitStatusLabel('R100'), '重命名');
  assert.equal(gitStatusLabel(''), '改动');
});
