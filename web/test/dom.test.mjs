import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  parseCssColor,
  parsedToHex,
  rgbToHex,
  gradientFirstColor,
  resolveCssVars,
  rebuildGradient,
  specificityOf,
  classifyBgValue,
  microCss,
  parseInsetRing,
} from '../src/lib/dom.js';

// —— 取色 ——
test('parseCssColor 认 rgb/rgba/color(srgb) 计算值形态', () => {
  assert.deepEqual(parseCssColor('rgb(186, 138, 120)'), { r: 186, g: 138, b: 120, a: 1 });
  assert.equal(parseCssColor('rgba(0, 0, 0, 0)').a, 0);
  const c = parseCssColor('color(srgb 0.729412 0.541176 0.470588 / 0.14)');
  assert.equal(c.a, 0.14);
  assert.equal(c.r, Math.round(0.729412 * 255));
  assert.equal(parseCssColor('transparent'), null);
  assert.equal(parseCssColor(''), null);
});

test('parsedToHex / rgbToHex 输出六位小写十六进制', () => {
  assert.equal(rgbToHex('rgb(186, 138, 120)'), '#ba8a78');
  assert.equal(parsedToHex({ r: 10, g: 2, b: 3 }), '#0a0203');
  assert.equal(rgbToHex('linear-gradient(..)'), null);
});

test('parseCssColor 认十六进制（var 解析后源头声明的形态），含 3/4/8 位与非法', () => {
  assert.deepEqual(parseCssColor('#ba8a78'), { r: 186, g: 138, b: 120, a: 1 });
  assert.deepEqual(parseCssColor('#abc'), { r: 170, g: 187, b: 204, a: 1 });
  assert.equal(parseCssColor('#abcd').a > 0.7 && parseCssColor('#abcd').a < 0.9, true);
  assert.equal(parseCssColor('#ba8a787f').a < 0.51, true);
  assert.equal(parseCssColor('#xyz'), null);
  assert.equal(parseCssColor('#12345'), null);
});

// —— 渐变读数 ——
test('gradientFirstColor 取 computed 渐变的首个色 stop，var() 原文取不到', () => {
  const c = gradientFirstColor('linear-gradient(180deg, rgb(186, 138, 120), rgb(179, 131, 107))');
  assert.deepEqual(c, { r: 186, g: 138, b: 120, a: 1 });
  assert.equal(gradientFirstColor('linear-gradient(180deg, var(--brand), var(--brand-deep))'), null);
  assert.equal(gradientFirstColor('none'), null);
});

test('var 解析后的十六进制渐变能读出首 stop（2026-11-04 回归：不认 #hex 又掉 0%）', () => {
  const resolved = resolveCssVars('linear-gradient(180deg, var(--brand), var(--brand-deep))', (n) => ({ '--brand': '#ba8a78', '--brand-deep': '#b3836b' }[n] || ''));
  assert.equal(resolved, 'linear-gradient(180deg, #ba8a78, #b3836b)');
  assert.deepEqual(gradientFirstColor(resolved), { r: 186, g: 138, b: 120, a: 1 });
});

test('rebuildGradient 认十六进制 stop：只调透明度不拍平，换色全停替换', () => {
  const src = 'linear-gradient(180deg, #ba8a78, #b3836b)';
  assert.equal(rebuildGradient(src, '#ba8a78', 50), 'linear-gradient(180deg, rgba(186, 138, 120, 0.5), rgba(179, 131, 107, 0.5))');
  assert.equal(rebuildGradient(src, '#00ff00', 100), 'linear-gradient(180deg, rgba(0, 255, 0, 1), rgba(0, 255, 0, 1))');
});

// —— var 解析 ——
test('resolveCssVars 按查到的值替换，查不到用 fallback，再没有变 transparent', () => {
  const lookup = (n) => ({ '--brand': '#ba8a78' }[n] || '');
  assert.equal(resolveCssVars('linear-gradient(180deg, var(--brand), var(--brand-deep))', lookup), 'linear-gradient(180deg, #ba8a78, transparent)');
  assert.equal(resolveCssVars('var(--x, #fff)', () => ''), '#fff');
  assert.equal(resolveCssVars('rgb(1, 2, 3)', lookup), 'rgb(1, 2, 3)');
});

// —— 渐变重建 ——
test('rebuildGradient 只调透明度：保留各 stop 原色与位置，alpha 等比缩放', () => {
  const out = rebuildGradient('linear-gradient(180deg, rgb(186, 138, 120), rgb(179, 131, 107))', '#ba8a78', 50);
  assert.equal(out, 'linear-gradient(180deg, rgba(186, 138, 120, 0.5), rgba(179, 131, 107, 0.5))');
});

test('rebuildGradient 换色：所有 stop 变新色，alpha 仍按原比例', () => {
  const out = rebuildGradient('linear-gradient(180deg, rgb(186, 138, 120), rgb(179, 131, 107))', '#ff0000', 100);
  assert.equal(out, 'linear-gradient(180deg, rgba(255, 0, 0, 1), rgba(255, 0, 0, 1))');
});

test('rebuildGradient 半透明首 stop 的换算是相对首 stop 的', () => {
  const src = 'linear-gradient(90deg, rgba(10, 20, 30, 0.5), rgba(40, 50, 60, 0.25))';
  const out = rebuildGradient(src, null, 100);
  assert.equal(out, 'linear-gradient(90deg, rgba(10, 20, 30, 1), rgba(40, 50, 60, 0.5))');
});

test('rebuildGradient 保留位置后缀，非渐变原样返回，alpha 钳制在 0..1', () => {
  const out = rebuildGradient('linear-gradient(90deg, rgb(143, 90, 68) 0%, rgb(186, 138, 120) 45%, rgb(207, 147, 103) 100%)', '#00ff00', 20);
  assert.equal(out, 'linear-gradient(90deg, rgba(0, 255, 0, 0.2) 0%, rgba(0, 255, 0, 0.2) 45%, rgba(0, 255, 0, 0.2) 100%)');
  assert.equal(rebuildGradient('none', '#ff0000', 50), 'none');
});

// —— 背景来源归类（findBgSource 的核心判别）——
test('classifyBgValue：var 渐变解析后归 image，var 实色归 color，transparent/全透明归 null', () => {
  const lookup = (n) => ({ '--brand': '#ba8a78', '--brand-deep': '#b3836b', '--g3': '#332f2a' }[n] || '');
  assert.deepEqual(classifyBgValue('linear-gradient(180deg,var(--brand),var(--brand-deep))', lookup), {
    kind: 'image',
    text: 'linear-gradient(180deg,#ba8a78,#b3836b)',
  });
  // WebView2 待替换值坑：源头是 color 声明时 var 原文也要归成 color
  assert.deepEqual(classifyBgValue('var(--g3)', lookup), { kind: 'color', text: '#332f2a' });
  assert.equal(classifyBgValue('transparent', lookup), null);
  assert.equal(classifyBgValue('rgba(0, 0, 0, 0)', lookup), null);
  assert.equal(classifyBgValue('linear-gradient(180deg, rgb(186, 138, 120), rgb(179, 131, 107))', lookup).kind, 'image');
});

// —— 特异度 ——
test('specificityOf：id 压过类链，伪元素不计', () => {
  assert.ok(specificityOf('#app [data-mt="x"]') > specificityOf('.btn.primary'));
  assert.equal(specificityOf('.a.b::before'), specificityOf('.a.b'));
  assert.ok(specificityOf('.settings-nav button.active') > specificityOf('button'));
});

// —— 规则生成 ——
test('microCss：bgImage 走 background-image 且不再叠 background-color', () => {
  const withImg = microCss({ k: { sel: '.btn', name: '按钮', bg: '#ff0000', bgOpacity: 50, bgImage: 'linear-gradient(180deg, rgba(255, 0, 0, 0.5))' } });
  assert.ok(withImg.includes('background-image: linear-gradient(180deg, rgba(255, 0, 0, 0.5))'));
  assert.ok(!withImg.includes('background-color'));
  const solid = microCss({ k: { sel: '.btn', name: '按钮', bg: '#ff0000', bgOpacity: 50 } });
  assert.ok(solid.includes('background-color: rgba(255, 0, 0, 0.5)'));
  const frame = microCss({ k: { sel: '.btn', name: '按钮', bgImage: 'linear-gradient(180deg, rgba(255, 0, 0, 0.5))' } }, true);
  assert.ok(frame.includes('background-image: linear-gradient(180deg, rgba(255, 0, 0, 0.5)) !important'));
});

// —— 描边环（2026-11-12：选中态描边是 box-shadow inset 环画的，微调面板的描边要认它）——
test('parseInsetRing：从 computed box-shadow 认出 inset 环（Chromium 色在前 inset 在后）', () => {
  const ring = parseInsetRing('rgb(184, 179, 169) 0px 0px 0px 1px inset');
  assert.ok(ring, '单环层应命中');
  assert.equal(ring.width, 1);
  assert.equal(ring.color, 'rgb(184, 179, 169)');
  assert.deepEqual(ring.others, []);
});
test('parseInsetRing：多层 shadow 里认环并保留其余层', () => {
  const ring = parseInsetRing('rgba(0, 0, 0, 0.55) 0px 10px 30px, rgb(86, 82, 75) 0px 0px 0px 2px inset');
  assert.ok(ring);
  assert.equal(ring.width, 2);
  assert.equal(ring.others.length, 1);
  assert.ok(ring.others[0].includes('0px 10px 30px'));
});
test('parseInsetRing：普通投影（三段长度）与 0 宽环不是环', () => {
  assert.equal(parseInsetRing('rgba(0, 0, 0, 0.3) 0px 10px 20px'), null);
  assert.equal(parseInsetRing('rgb(0, 0, 0) 0px 0px 0px 0px inset'), null);
  assert.equal(parseInsetRing('none'), null);
  assert.equal(parseInsetRing(''), null);
});
test('microCss：ring 走 box-shadow 环；0px 环=去线；ring 优先于 border', () => {
  const on = microCss({ k: { sel: '.settings-nav button.active', name: '设置导航按钮', ring: '1px', ringColor: '#ba8a78' } });
  assert.ok(on.includes('box-shadow: inset 0 0 0 1px #ba8a78'));
  assert.ok(!on.includes('border-width'));
  const off = microCss({ k: { sel: '.b', name: 'x', ring: '0px', ringColor: '#ba8a78' } });
  assert.ok(off.includes('box-shadow: none'));
  const mixed = microCss({ k: { sel: '.b', name: 'x', ring: '2px', ringColor: '#ba8a78', border: '3px', borderColor: '#ff0000' } });
  assert.ok(mixed.includes('box-shadow: inset 0 0 0 2px #ba8a78'));
  assert.ok(!mixed.includes('border-width'), '并存时 ring 赢，不输出 border 免布局位移');
});
