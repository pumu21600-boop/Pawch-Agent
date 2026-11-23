import './lib/ui.css';
import { mount } from 'svelte';
import App from './App.svelte';

// 前端错误收集（2026-11-13）：未捕获异常 / 未处理的 Promise 拒绝 → POST /api/client-log，
// 服务端落 JSONL（event_type=error / kind=client），排障不再只靠用户口头描述。
// 防刷屏：同一错误 30 秒内只报一次；整页生命周期最多 30 条。上报本身失败静默吞掉。
const seen = new Map();
let reported = 0;
const MAX_REPORTS = 30;
const DEDUP_MS = 30000;

function reportClientError(payload) {
  if (reported >= MAX_REPORTS) return;
  const fingerprint = `${payload.message}|${payload.file}|${payload.line}`;
  const now = Date.now();
  if (now - (seen.get(fingerprint) || 0) < DEDUP_MS) return;
  seen.set(fingerprint, now);
  reported += 1;
  try {
    fetch('/api/client-log', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
      keepalive: true, // 页面关闭/刷新时也尽量把最后一条送出去
    }).catch(() => {});
  } catch {}
}

window.addEventListener('error', (e) => {
  // 资源加载失败（img/script 404）没有 message、target 不是 window，不归这里管
  if (!e.message) return;
  reportClientError({
    kind: 'error',
    level: 'error',
    message: e.message || '',
    stack: (e.error && e.error.stack) || '',
    file: e.filename || '',
    line: e.lineno || 0,
    col: e.colno || 0,
    url: location.href,
  });
});

window.addEventListener('unhandledrejection', (e) => {
  const reason = e.reason;
  const message =
    reason && typeof reason === 'object' ? reason.message || String(reason) : String(reason);
  reportClientError({
    kind: 'unhandledrejection',
    level: 'error',
    message,
    stack: (reason && reason.stack) || '',
    file: '',
    line: 0,
    col: 0,
    url: location.href,
  });
});

// 主线程卡顿哨兵（2026-11-23）：200ms 一跳，两跳间隔超过 600ms = 主线程被某个长
// 任务占住——「任务跑着的时候点了没反应」类问题从此有硬数据可查（卡多久、什么档位）。
// 后台标签页的定时器被浏览器节流成 ≥1s，不算卡顿：hidden 时只重置锚点不报警。
let lastBeat = performance.now();
let wasHidden = false; // 恢复可见的第一拍不计（后台节流留下的间隔不是卡顿）
setInterval(() => {
  const now = performance.now();
  const gap = now - lastBeat;
  lastBeat = now;
  if (document.hidden) {
    wasHidden = true;
    return;
  }
  if (wasHidden) {
    wasHidden = false;
    return;
  }
  if (gap < 600) return;
  const bucket = gap < 1200 ? '0.6-1.2s' : gap < 3000 ? '1.2-3s' : gap < 8000 ? '3-8s' : '>8s';
  reportClientError({
    kind: 'main-stall',
    level: 'warn',
    message: `主线程卡顿 ~${Math.round(gap)}ms（${bucket}）`,
    stack: '',
    file: 'main.js 主线程哨兵',
    line: 0,
    col: 0,
    url: location.href,
  });
}, 200);

// Svelte 5 入口：mount() 取代 Svelte 4 的 new App()（034 迁移轮）。
// 组件仍按 legacy 语法编译（无 runes），行为不变。
const app = mount(App, { target: document.getElementById('app') });

export default app;
