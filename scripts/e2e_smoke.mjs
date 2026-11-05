// 真起服务的端到端冒烟（2026-11-05 审计修复 D14）
//
// 起一个隔离实例（临时应用根 + 临时数据目录 + 随机端口），配 mock 渠道，跑 5 条断言：
//   1) 未带令牌打 /api 必须 401；Bearer 令牌 200（D1 契约）
//   2) HTML 文档响应下发同源 cookie（页面自动带令牌）
//   3) /api/chat 走 mock 渠道流式出答案，SSE 以 done 收尾
//   4) 会话落盘到 <数据目录>/logs/sessions/<id>.json
//   5) 杀掉服务再起（同一数据目录），/api/session 仍能读回这轮消息（重启恢复）
//
// 用法（仓库根目录）：
//   node scripts/e2e_smoke.mjs                 # 默认 target/debug/pawch-agent.exe
//   node scripts/e2e_smoke.mjs --exe target/release/pawch-agent.exe
//   node scripts/e2e_smoke.mjs --keep          # 保留临时目录，便于诊断
//
// 退出码：0 = 全过，1 = 有失败。mock 渠道用 scripts/mock_provider.mjs（默认 9399）。

import { spawn } from 'node:child_process';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';

const repo = path.resolve(import.meta.dirname, '..');
const argv = process.argv.slice(2);
let exe = path.join(repo, 'target', 'debug', 'pawch-agent.exe');
let keep = false;
for (let i = 0; i < argv.length; i += 1) {
  if (argv[i] === '--exe') exe = path.resolve(argv[++i]);
  else if (argv[i] === '--keep') keep = true;
}

const results = [];
function record(name, ok, detail = '') {
  results.push({ name, ok, detail });
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail ? ` — ${detail}` : ''}`);
}
function die(msg) {
  console.error(`[e2e] ${msg}`);
  process.exit(1);
}
function sleep(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

/** 找一个空闲端口（绑定 0 后立刻释放）。 */
function freePort() {
  return new Promise((resolve, reject) => {
    const srv = net.createServer();
    srv.unref();
    srv.on('error', reject);
    srv.listen(0, '127.0.0.1', () => {
      const p = srv.address().port;
      srv.close(() => resolve(p));
    });
  });
}
function portOpen(port) {
  return new Promise((resolve) => {
    const s = net.connect({ host: '127.0.0.1', port });
    s.once('connect', () => {
      s.destroy();
      resolve(true);
    });
    s.once('error', () => resolve(false));
  });
}

if (!fs.existsSync(exe)) die(`找不到服务二进制：${exe}（先 cargo build）`);

const work = fs.mkdtempSync(path.join(os.tmpdir(), 'pawch-e2e-'));
const appDir = path.join(work, 'app');
const dataDir = path.join(work, 'data');
fs.mkdirSync(appDir, { recursive: true });
fs.copyFileSync(exe, path.join(appDir, 'pawch-agent.exe'));
for (const f of ['providers.json']) {
  fs.copyFileSync(path.join(repo, f), path.join(appDir, f));
}
fs.cpSync(path.join(repo, 'prompts'), path.join(appDir, 'prompts'), { recursive: true });

const port = await freePort();
const config = fs
  .readFileSync(path.join(repo, 'config.toml'), 'utf8')
  .replace(/^data_dir = ""$/m, `data_dir = "${dataDir.replace(/\\/g, '/')}"`)
  .replace(/^port = 5173$/m, `port = ${port}`);
fs.writeFileSync(path.join(appDir, 'config.toml'), config);

const mockBusy = await portOpen(9399);
let mockProc = null;
if (!mockBusy) {
  mockProc = spawn(process.execPath, [path.join(repo, 'scripts', 'mock_provider.mjs')], {
    cwd: repo,
    stdio: 'ignore',
    windowsHide: true,
  });
  for (let i = 0; i < 50 && !(await portOpen(9399)); i += 1) await sleep(100);
}
if (!(await portOpen(9399))) die('mock 渠道没起来（9399）');

const env = {
  ...process.env,
  DEEPSEEK_BASE_URL: 'http://127.0.0.1:9399',
  DEEPSEEK_API_KEY: 'sk-e2e-smoke',
};

function startService() {
  return spawn(path.join(appDir, 'pawch-agent.exe'), ['--minimized'], {
    cwd: appDir,
    env,
    stdio: 'ignore',
    windowsHide: true,
  });
}
async function stopService(child) {
  if (!child || child.exitCode !== null) return;
  child.kill();
  for (let i = 0; i < 100 && child.exitCode === null; i += 1) await sleep(100);
  if (child.exitCode === null) {
    spawn('taskkill', ['/F', '/PID', String(child.pid)], { stdio: 'ignore', windowsHide: true });
    await sleep(500);
  }
  for (let i = 0; i < 100 && (await portOpen(port)); i += 1) await sleep(100);
}

const base = `http://127.0.0.1:${port}`;
async function token() {
  const p = path.join(dataDir, 'server.token');
  for (let i = 0; i < 300; i += 1) {
    try {
      const t = fs.readFileSync(p, 'utf8').trim();
      if (t) return t;
    } catch {}
    await sleep(100);
  }
  die(`等不到访问令牌文件：${p}`);
}
async function waitReady(tok) {
  for (let i = 0; i < 400; i += 1) {
    try {
      const r = await fetch(`${base}/api/version`, { headers: { Authorization: `Bearer ${tok}` } });
      if (r.ok) return;
    } catch {}
    await sleep(100);
  }
  die('服务 40 秒内没就绪');
}

let service = startService();
try {
  const tok = await token();
  await waitReady(tok);
  const auth = { Authorization: `Bearer ${tok}` };

  // 1) 令牌契约：无凭证 401，有凭证 200
  const noAuth = await fetch(`${base}/api/version`);
  const withAuth = await fetch(`${base}/api/version`, { headers: auth });
  record('01 /api 无令牌 401、有令牌 200', noAuth.status === 401 && withAuth.ok, `${noAuth.status}/${withAuth.status}`);

  // 2) 页面 cookie：HTML 文档响应带 Set-Cookie
  const html = await fetch(`${base}/`, { headers: { Accept: 'text/html' } });
  const setCookie = html.headers.get('set-cookie') || '';
  record(
    '02 HTML 文档下发 HttpOnly cookie',
    html.ok && setCookie.includes('pawch_token=') && setCookie.toLowerCase().includes('httponly'),
    setCookie.slice(0, 60)
  );

  // 3) 走 mock 渠道跑一轮，SSE 流式出答案并以 done 收尾
  const sid = 'e2e-smoke';
  const msg = '你好，给我一个冒烟回复';
  const chat = await fetch(`${base}/api/chat`, {
    method: 'POST',
    headers: { ...auth, 'Content-Type': 'application/json' },
    body: JSON.stringify({ messages: [{ role: 'user', content: msg }], message: msg, session_id: sid }),
  });
  if (!chat.ok) die(`/api/chat HTTP ${chat.status}`);
  let sse = '';
  let done = false;
  const reader = chat.body.getReader();
  const dec = new TextDecoder();
  for (;;) {
    const { done: end, value } = await reader.read();
    if (end) break;
    sse += dec.decode(value, { stream: true });
    if (/"done"\s*:\s*true/.test(sse)) done = true;
  }
  const answer = [...sse.matchAll(/data: (\{.*\})\n\n/g)]
    .map((m) => {
      try {
        return JSON.parse(m[1]).content || '';
      } catch {
        return '';
      }
    })
    .join('');
  record(
    '03 mock 渠道流式回答 + done 收尾',
    done && answer.includes('模拟渠道'),
    answer.slice(0, 40)
  );

  // 4) 会话落盘
  const sessFile = path.join(dataDir, 'logs', 'sessions', `${sid}.json`);
  let persisted = false;
  for (let i = 0; i < 100 && !persisted; i += 1) {
    persisted = fs.existsSync(sessFile);
    if (!persisted) await sleep(100);
  }
  const saved = persisted ? JSON.parse(fs.readFileSync(sessFile, 'utf8')) : null;
  const savedText = JSON.stringify(saved || {});
  record('04 会话落盘且含本轮用户消息', persisted && savedText.includes(msg), sessFile);

  // 5) 重启恢复：同一数据目录重起，仍能读回这轮消息
  await stopService(service);
  service = startService();
  await waitReady(tok);
  const back = await fetch(`${base}/api/session?id=${sid}`, { headers: auth });
  const body = back.ok ? await back.json() : {};
  const msgs = body.messages || [];
  record(
    '05 重启后会话恢复（消息还在）',
    back.ok && msgs.length >= 2 && JSON.stringify(msgs).includes(msg),
    `messages=${msgs.length}`
  );
} catch (e) {
  console.error('[e2e] 异常：', e);
  results.push({ name: '异常中断', ok: false, detail: String(e) });
} finally {
  await stopService(service);
  if (mockProc) mockProc.kill();
  if (!keep) {
    try {
      fs.rmSync(work, { recursive: true, force: true });
    } catch {}
  } else {
    console.log(`[e2e] 保留临时目录：${work}`);
  }
}

const failed = results.filter((r) => !r.ok);
console.log(`\n[e2e] ${results.length - failed.length}/${results.length} 通过`);
process.exit(failed.length ? 1 : 0);
