// 构建产物检查：markdown 渲染样式必须真的进了 CSS 包。
//
// 由来（2026-10-30 实锤）：markdown 是 `{@html md(...)}` 运行时生成的，元素上没有
// Svelte 作用域类。样式若写成 `.content .md-p`（普通后代选择器），编译器会认为
// 选择器未使用而整条裁掉——**构建成功、界面静默退化成浏览器默认样式**，很难靠
// 肉眼在开发机上发现。这个脚本在 build 之后跑，缺一条就红。
//
// 用法：先 `cd web && npm run build`，再 `node scripts/check_md_css.mjs`
import { readdirSync, readFileSync, existsSync } from 'node:fs';
import { join } from 'node:path';

const dist = join(process.cwd(), 'web', 'dist', 'assets');
if (!existsSync(dist)) {
  console.error('[check_md_css] 找不到 web/dist/assets，先构建前端');
  process.exit(1);
}
const css = readdirSync(dist)
  .filter((f) => f.endsWith('.css'))
  .map((f) => readFileSync(join(dist, f), 'utf8'))
  .join('\n');
if (!css) {
  console.error('[check_md_css] 没有 CSS 产物');
  process.exit(1);
}
const required = [
  'md-p',
  'md-h',
  'md-ul',
  'md-ol',
  'md-code',
  'md-codeblock',
  'md-codehead',
  'md-codelang',
  'md-copy',
  'md-pre',
  'md-table',
  'md-quote',
  'md-hr',
  'md-a',
  'md-toolcall',
];
const missing = required.filter((k) => !new RegExp(`\\.${k.replace(/-/g, '\\-')}(?![\\w-])`).test(css));
if (missing.length) {
  console.error(`[check_md_css] 构建产物缺少 markdown 样式: ${missing.join(', ')}`);
  console.error('原因通常是选择器没写 :global()，被 Svelte 当未使用选择器裁掉了。');
  process.exit(1);
}
console.log(`[check_md_css] OK：${required.length} 个 markdown 样式类都在构建产物里`);
