import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import fs from 'node:fs';
import path from 'node:path';

// 主服务地址（config.toml [server].port）。代理转发时把 Origin 改写成它——
// 主服务的来源校验要求 Origin 与 Host 同源（防 DNS rebinding），
// 而浏览器发来的是热更端口 5174，不改写会被 403。代理的意义正在于此。
const SERVICE = 'http://127.0.0.1:5173';

// D1：主服务 /api 要本地访问令牌。热更页（5174）没有 5173 下发的 cookie，
// 代理转发时补 Authorization。token 解析顺序与服务一致：
// PAWCH_AGENT_DATA_DIR > %LOCALAPPDATA%\Pawch Agent\.data-dir > %USERPROFILE%\.pawch-agent
function readLocalToken() {
  const dirs = [];
  if (process.env.PAWCH_AGENT_DATA_DIR) dirs.push(process.env.PAWCH_AGENT_DATA_DIR);
  try {
    const ptr = path.join(process.env.LOCALAPPDATA || '', 'Pawch Agent', '.data-dir');
    const t = fs.readFileSync(ptr, 'utf8').trim();
    if (t) dirs.push(t);
  } catch {}
  const home = process.env.USERPROFILE || process.env.HOME;
  if (home) dirs.push(path.join(home, '.pawch-agent'));
  for (const dir of dirs) {
    try {
      const t = fs.readFileSync(path.join(dir, 'server.token'), 'utf8').trim();
      if (t) return t;
    } catch {}
  }
  console.warn('[vite] 读不到主服务访问令牌（server.token）：热更页打 /api 会 401');
  return '';
}
const LOCAL_TOKEN = readLocalToken();

const proxyToService = {
  target: SERVICE,
  changeOrigin: true,
  configure: (proxy) => {
    proxy.on('proxyReq', (proxyReq) => {
      proxyReq.setHeader('Origin', SERVICE);
      if (LOCAL_TOKEN) proxyReq.setHeader('Authorization', `Bearer ${LOCAL_TOKEN}`);
    });
  },
};

export default defineConfig({
  plugins: [svelte()],
  server: {
    // 热更服务器：5174（5173 已让给主服务，见 config.toml [server].port）
    port: 5174,
    proxy: {
      '/api': proxyToService,
      '/cancel': proxyToService,
      '/ui-ext': proxyToService,
    },
  },
  build: {
    outDir: 'dist',
    // 2026-10-08：代码分包。原先单包 1.4MB（gzip 581KB）——文档类大依赖
    // （SheetJS xlsx / mammoth）和 svelte 运行时要跟应用代码一起重下。拆开后
    // 应用代码改动只失效自己的 chunk，vendor 大块靠长缓存。纯构建配置，
    // 不影响任何运行时行为。
    rollupOptions: {
      output: {
        // Svelte 5（034 迁移轮）：legacy runtime 不再从 'svelte/internal' 桶口整包
        // 导入（组件编译产物走 svelte/internal/client 等子路径），对象式 manualChunks
        // 落成了空块、运行时混进 index 大块。改函数式按模块 id 分组，保持 2026-10-08
        // 四块拆分的缓存结构：应用 / svelte 运行时 / xlsx / mammoth。
        manualChunks(id) {
          const p = id.replace(/\\/g, '/');
          if (!p.includes('/node_modules/')) return undefined;
          if (p.includes('/node_modules/svelte/')) return 'svelte';
          if (p.includes('/node_modules/xlsx/')) return 'xlsx';
          if (p.includes('/node_modules/mammoth/')) return 'mammoth';
          return undefined;
        },
      },
    },
  },
});
