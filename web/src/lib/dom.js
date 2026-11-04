// DOM/CSS 纯助手(范式 App.svelte 拆分):无组件状态依赖,App 与各视图组件共用。
// 给每条顶层规则的选择器加 #app 前缀：压过 Svelte scoped 样式（.composer.s-xxx 两级类名）；
// @media/@keyframes 等整块原样保留
// 2026-10-08 修三处 CSS 语法盲区：① 选择器按逗号切分时不识别引号/[]/()——
// [data-x="a,b"]、:is(.a, .b) 会被切碎成语法错误规则，整条注入静默失效；
// ② 找声明块收尾的 '}' 不跳过字符串与注释——content: "}" 会提前截断，
// 后续全部错位；③ 找选择器结束的 '{'、@ 块括号配对、逗号切分都不跳过注释——
// 注释里含 { } , 时（/* .b { color: blue } */）会把注释里的 { 当选择器结束、
// 注释里的 , 当选择器分隔，产出 `#app /* .b {` 这类废规则，紧跟的真实规则
// 被整条吞掉后浏览器静默丢弃。三个 helper 都按字符扫描处理。
export function escalateCss(css) {
  let out = '';
  let i = 0;
  const n = css.length;
  // 顶层逗号切分：引号 / [] / () / 注释内部的逗号不是选择器分隔符
  const splitSelector = (sel) => {
    const parts = [];
    let cur = '';
    let quote = '';
    let bracket = 0;
    let paren = 0;
    for (let k = 0; k < sel.length; k++) {
      const ch = sel[k];
      if (quote) {
        cur += ch;
        if (ch === quote) quote = '';
        continue;
      }
      if (ch === '"' || ch === "'") {
        quote = ch;
        cur += ch;
        continue;
      }
      if (ch === '/' && sel[k + 1] === '*') {
        // 注释整体搬运（含里面的 , { }），不当分隔符
        const e = sel.indexOf('*/', k + 2);
        const stop = e === -1 ? sel.length : e + 2;
        cur += sel.slice(k, stop);
        k = stop - 1;
        continue;
      }
      if (ch === '[') bracket++;
      else if (ch === ']') bracket = Math.max(0, bracket - 1);
      else if (ch === '(') paren++;
      else if (ch === ')') paren = Math.max(0, paren - 1);
      if (ch === ',' && bracket === 0 && paren === 0) {
        parts.push(cur);
        cur = '';
        continue;
      }
      cur += ch;
    }
    parts.push(cur);
    return parts;
  };
  // 从 from 起找第一个「真」括号：跳过字符串与 /* */ 注释。
  // wantOpen=true 找 '{'（选择器结束），false 找 '}'（声明块结束）。
  const findBrace = (from, wantOpen) => {
    let quote = '';
    for (let j = from; j < n; j++) {
      const ch = css[j];
      if (quote) {
        if (ch === quote) quote = '';
        continue;
      }
      if (ch === '"' || ch === "'") {
        quote = ch;
        continue;
      }
      if (ch === '/' && css[j + 1] === '*') {
        const e = css.indexOf('*/', j + 2);
        j = e === -1 ? n : e + 1;
        continue;
      }
      if (wantOpen ? ch === '{' : ch === '}') return j;
    }
    return -1;
  };
  while (i < n) {
    const brace = findBrace(i, true);
    if (brace === -1) {
      out += css.slice(i);
      break;
    }
    const selector = css.slice(i, brace);
    if (selector.trim().startsWith('@')) {
      // @ 块括号配对同样跳字符串/注释：块内注释里的 { } 会把深度算错，
      // 后续规则被整段吞进 @ 块（同样静默丢失）。
      let depth = 1;
      let j = brace + 1;
      let quote = '';
      while (j < n && depth > 0) {
        const ch = css[j];
        if (quote) {
          if (ch === quote) quote = '';
          j++;
          continue;
        }
        if (ch === '"' || ch === "'") {
          quote = ch;
          j++;
          continue;
        }
        if (ch === '/' && css[j + 1] === '*') {
          const e = css.indexOf('*/', j + 2);
          j = e === -1 ? n : e + 1;
          continue;
        }
        if (ch === '{') depth++;
        else if (ch === '}') depth--;
        j++;
      }
      out += css.slice(i, j);
      i = j;
    } else {
      const prefixed = splitSelector(selector)
        .map((s) => {
          const t = s.trim();
          return t ? `#app ${t}` : t;
        })
        .join(', ');
      out += `${prefixed}{`;
      // 跳过这条规则的声明块，直到它的关闭 '}'（声明不嵌套）
      const close = findBrace(brace + 1, false);
      const end = close === -1 ? n : close + 1;
      out += css.slice(brace + 1, end);
      i = end;
    }
  }
  return out;
}
// 元素别名表：语义名 → 稳定选择器（Svelte scoped 类名跨构建不稳定，禁止用原始类名当选择器）。
// 每个别名可带 pick：命中大容器时进一步收敛到具体控件，避免"一点按钮选中整条底板"。
//  - 'self'：只在点中容器自身（padding/空隙）时才算容器，点中子元素时继续往下落
//  - 选择器：命中容器后，在容器内按该选择器找离点击最近的祖先，找到就用它
export const ELEMENT_ALIASES = [
  { sel: '.composer', name: '输入框', pick: 'self' },
  { sel: '.composer-input', name: '输入文本区' },
  { sel: '.composer-bar', name: '输入框工具栏', pick: 'self' },
  { sel: '.icon-btn', name: '图标按钮' },
  { sel: '.pill', name: '工具栏胶囊按钮' },
  { sel: '.send-btn', name: '发送按钮' },
  { sel: '.ctx-ring', name: '上下文圆环' },
  { sel: '.user-bubble', name: '用户消息气泡' },
  { sel: '.asst-bubble', name: 'AI 消息气泡', pick: 'self' },
  { sel: '.bubble', name: '消息气泡', pick: 'self' },
  { sel: '.error-bubble', name: '错误消息气泡' },
  { sel: '.tool-card', name: '工具卡' },
  { sel: '.tool-preview', name: '工具输出（代码块）' },
  { sel: '.tool-detail', name: '工具卡详情' },
  { sel: '.tool-pre', name: '工具参数/结果块' },
  { sel: '.tool-head', name: '工具卡头部' },
  { sel: '.tool-badge', name: '工具状态徽章' },
  { sel: '.tool-name', name: '工具名' },
  { sel: '.tool-args', name: '工具参数' },
  { sel: '.thinking', name: '思考折叠块', pick: 'self' },
  { sel: '.thinking-body', name: '思考内容' },
  { sel: '.approval-box', name: '审批框' },
  { sel: '.term-panel', name: '终端面板', pick: 'self' },
  { sel: '.term-head', name: '终端标题栏' },
  { sel: '.term-title', name: '终端标题' },
  { sel: '.term-hint', name: '终端提示文字' },
  { sel: '.term-clear', name: '终端清空按钮' },
  { sel: '.term-close', name: '终端关闭按钮' },
  { sel: '.term-in', name: '终端输入栏' },
  { sel: '.term-out', name: '终端输出区' },
  { sel: '.term-line', name: '终端输出行' },
  { sel: '.term-prompt', name: '终端命令提示符' },
  { sel: '.term-text', name: '终端输出文本' },
  { sel: '.cp-bar', name: '改动条', pick: 'self' },
  { sel: '.cp-btn', name: '改动条按钮' },
  { sel: '.new-chat', name: '新对话按钮' },
  { sel: '.chat-head', name: '顶栏', pick: 'self' },
  { sel: '.right-panel', name: '产物面板', pick: 'self' },
  { sel: '.rp-tabs', name: '产物面板标签栏', pick: 'self' },
  { sel: '.rp-tabs button', name: '产物面板标签' },
  { sel: '.rp-body', name: '产物面板内容区' },
  { sel: '.rp-empty', name: '产物面板空提示' },
  /* 网页面板（内置浏览器）：没有这几条时，右击网页黑色内容区会被外层
     「产物面板内容区 .rp-body」抢走，改到底也改不到网页画布本身（2026-11-03 真机实测） */
  { sel: '.web-body', name: '网页内容区' },
  { sel: '.web-empty', name: '网页空状态' },
  { sel: '.web-toolbar', name: '网页地址栏', pick: 'self' },
  { sel: '.web-tabbar', name: '网页标签栏', pick: 'self' },
  { sel: '.web-tab', name: '网页标签页' },
  /* 「打开标签页」选择页的卡片：不登记的话右击卡片只会命中外层 .rp-body */
  { sel: '.rp-open-card', name: '打开标签页卡片' },
  /* 聊天区代码块（md.js 生成的结构，{@html} 渲染）：语言标签/复制按钮也一并登记 */
  { sel: '.md-codeblock', name: '代码块', pick: '.md-codehead' },
  { sel: '.md-codehead', name: '代码块头部' },
  { sel: '.md-codelang', name: '代码块语言标签' },
  { sel: '.md-copy', name: '代码块复制按钮' },
  /* markdown 表格（md.js 直出）：整表/表头单元格/数据单元格各自可选——
     长按单元格直接选中它本身，不再被外层内容区抢走（2026-11-03 用户截图实锤） */
  { sel: '.md-table', name: 'markdown 表格' },
  { sel: '.md-th', name: '表头单元格' },
  { sel: '.md-td', name: '表格单元格' },
  { sel: '.md-code', name: '行内代码' },
  { sel: '.md-a', name: '正文链接' },
  { sel: '.md-p', name: '正文段落' },
  { sel: '.md-quote', name: '引用块' },
  { sel: '.md-ul', name: '无序列表' },
  { sel: '.md-ol', name: '有序列表' },
  { sel: '.md-toolcall', name: '工具调用折叠块' },
  { sel: '.md-h', name: '正文标题' },
  { sel: '.rp-refresh', name: '产物刷新按钮' },
  { sel: '.rp-file', name: '产物文件条目' },
  { sel: '.rp-preview', name: '预览代码块' },
  { sel: '.rp-preview-path', name: '预览文件路径' },
  { sel: '.modal.settings-modal', name: '设置弹窗', pick: 'self' },
  { sel: '.settings-nav button', name: '设置导航按钮' },
  { sel: '.theme-opt', name: '主题选项按钮' },
  { sel: '.agents-editor', name: 'CSS 注入框' },
  { sel: '.field input', name: '设置输入框' },
  { sel: '.field select', name: '设置下拉框' },
  { sel: '.btn', name: '按钮' },
  { sel: '.csel', name: '下拉框' },
  { sel: '.csel-caret', name: '下拉箭头' },
  { sel: '.csel-menu', name: '下拉菜单', pick: 'self' },
  { sel: '.csel-item', name: '下拉选项' },
  { sel: '.chip', name: '配置标签' },
  { sel: '.sidebar', name: '侧栏', pick: 'self' },
  { sel: '.side-head', name: '侧栏头部', pick: 'self' },
  { sel: '.row-btn', name: '侧栏工具按钮' },
  { sel: '.conv', name: '会话条目' },
  { sel: '.conv-title', name: '会话标题' },
  { sel: '.conv-time', name: '会话时间' },
  { sel: '.conv-more', name: '会话更多按钮' },
  { sel: '.proj', name: '项目行' },
  { sel: '.proj-name', name: '项目名' },
  { sel: '.foot-btn', name: '侧栏底部按钮' },
  { sel: '.cp-dock', name: '改动条容器', pick: 'self' },
  { sel: '.cp-undo', name: '撤销按钮' },
  { sel: '.ctx-wrap', name: '上下文圆环容器', pick: 'self' },
  { sel: '.ctx-card', name: '上下文明细卡' },
  { sel: '.head-btn', name: '顶栏按钮' },
  { sel: '.user-text', name: '用户消息文字' },
  { sel: '.bar-dropdown', name: '下拉菜单', pick: 'self' },
  { sel: '.bar-item', name: '下拉菜单项' },
];
// 取色/透明度解析：支持 rgb()/rgba()、Chromium 解析 color-mix 后给出的
// color(srgb r g b / a)（分量是 0-1 小数），以及十六进制（源头声明经 var 解析后
// 常见 #ba8a78 这类写法，2026-11-04 补——只认函数形态会让渐变读数又掉回 0%）。
// 取不出返回 null，由调用方决定不存该属性，禁止再兜底成 #000000（那是黑底叠层的直接来源）
export function parseCssColor(c) {
  const s = String(c || '').trim();
  let m = s.match(/^#([0-9a-f]{3,8})$/i);
  if (m) {
    const h = m[1];
    const n = h.length;
    if (n !== 3 && n !== 4 && n !== 6 && n !== 8) return null;
    const pair = (t) => parseInt(t.length === 1 ? t + t : t, 16);
    return {
      r: pair(n <= 4 ? h[0] : h.slice(0, 2)),
      g: pair(n <= 4 ? h[1] : h.slice(2, 4)),
      b: pair(n <= 4 ? h[2] : h.slice(4, 6)),
      a: n === 4 ? pair(h[3]) / 255 : n === 8 ? pair(h.slice(6, 8)) / 255 : 1,
    };
  }
  m = s.match(/^rgba?\(\s*([\d.]+)[,\s]+([\d.]+)[,\s]+([\d.]+)(?:\s*[,/]\s*([\d.]+%?))?\s*\)$/);
  if (m) {
    const a = m[4] == null ? 1 : m[4].endsWith('%') ? parseFloat(m[4]) / 100 : parseFloat(m[4]);
    return { r: +m[1], g: +m[2], b: +m[3], a };
  }
  m = s.match(/^color\(srgb\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)(?:\s*\/\s*([\d.]+))?\s*\)$/);
  if (m) {
    return { r: Math.round(+m[1] * 255), g: Math.round(+m[2] * 255), b: Math.round(+m[3] * 255), a: m[4] == null ? 1 : +m[4] };
  }
  return null;
}
export function parsedToHex(c) {
  if (!c) return null;
  return '#' + [c.r, c.g, c.b].map((x) => Math.max(0, Math.min(255, Math.round(x))).toString(16).padStart(2, '0')).join('');
}
export function rgbToHex(rgb) {
  return parsedToHex(parseCssColor(rgb));
}
// 渐变底：主按钮/发送键的橙色画在 linear-gradient（background-image）上，
// backgroundColor 全透明——只读 backgroundColor 的面板会显示「透明度 0%」且拖不动。
// 色值 token：函数形态（computed 值）+ 十六进制（源头声明经 var 解析后的常见形态，
// 2026-11-04 补：只认函数形态会让 var 渐变解析不出、又掉回 0%）
const COLOR_TOK = /rgba?\([^)]*\)|color\([^)]*\)|#[0-9a-fA-F]{3,8}/g;
export function gradientFirstColor(image) {
  const toks = String(image || '').match(COLOR_TOK) || [];
  for (const t of toks) {
    const c = parseCssColor(t);
    if (c) return c;
  }
  return null;
}
// 重建渐变：保留渐变结构与 stop 顺序（含 0%/45% 这类位置后缀），透明度按
// 「首个可解析 stop 的原透明度」等比换算——面板显示的 % 就是首 stop 的透明度，
// 拖到多少首 stop 就变多少。hex 与首 stop 同色（或为 null）时只调透明度、
// 保留各 stop 原色，不把双色渐变拍平；非 rgb()/color() 的 token 原样保留
export function rebuildGradient(image, hex, opacityPct) {
  const s = String(image || '');
  if (!/gradient/i.test(s)) return s;
  const toks = s.match(COLOR_TOK) || [];
  if (!toks.length) return s;
  const m = /^#([0-9a-f]{6})$/i.exec(hex || '');
  const firstHex = parsedToHex(gradientFirstColor(s));
  const recolor = !!m && !!firstHex && hex.toLowerCase() !== firstHex.toLowerCase();
  const firstA = (gradientFirstColor(s) || { a: 1 }).a || 1;
  const scale = Math.min(1, Math.max(0, (opacityPct ?? 100) / 100)) / firstA;
  let i = 0;
  return s.replace(COLOR_TOK, (tok) => {
    const p = parseCssColor(toks[i++]);
    if (!p) return tok;
    const a = Math.min(1, Math.max(0, p.a * scale));
    const rgb = recolor
      ? `${parseInt(m[1].slice(0, 2), 16)}, ${parseInt(m[1].slice(2, 4), 16)}, ${parseInt(m[1].slice(4, 6), 16)}`
      : `${Math.round(p.r)}, ${Math.round(p.g)}, ${Math.round(p.b)}`;
    return `rgba(${rgb}, ${+a.toFixed(3)})`;
  });
}
// 解析不出的按不透明处理
export function rgbAlpha(rgb) {
  const c = parseCssColor(rgb);
  return c ? Math.round(c.a * 100) : 100;
}
// 把 var(--x) 换成该元素上解析出的自定义属性值（查不到用 fallback，再没有就 transparent）。
// lookup 由调用方给（getComputedStyle(el).getPropertyValue），保持纯函数可单测；
// 嵌套 var（fallback 里再套 var）靠多轮替换收敛，最多 4 轮
export function resolveCssVars(text, lookup) {
  let s = String(text || '');
  for (let pass = 0; pass < 4; pass++) {
    let changed = false;
    s = s.replace(/var\(\s*(--[\w-]+)\s*(?:,\s*([^()]*?)\s*)?\)/g, (m0, name, fb) => {
      changed = true;
      const v = lookup ? String(lookup(name) || '').trim() : '';
      return v || String(fb || '').trim() || 'transparent';
    });
    if (!changed || !s.includes('var(')) break;
  }
  return s;
}
// 选择器特异度（简化版）：id=10000、类/属性/伪类=100、类型=1。只用于"几条候选里
// 挑层叠胜者"这种展示级比较，不追求规范精确
export function specificityOf(sel) {
  const s = String(sel || '').replace(/::(?:before|after|first-line|first-letter|selection|placeholder|marker|backdrop)/gi, '');
  const ids = (s.match(/#[\w-]+/g) || []).length;
  const classes = (s.match(/\.[\w-]+|\[[^\]]*\]|:(?!:)[\w-]+/g) || []).length;
  const types = (s.match(/(?:^|[\s>+~])[a-zA-Z][\w-]*/g) || []).length;
  return ids * 10000 + classes * 100 + types;
}
// —— 背景来源定位（DevTools 思路，2026-11-04）：不读最终算出来的值反推，而是找到
// 层叠胜出的那条背景声明原文——渐变/纯色/color-mix/var() 从此都是"改原文"一个套路。
// 返回 { kind:'image'|'color', text, sel } 或 null，text 已按元素把 var() 解析成实值。
// ⚠️ WebView2/Chromium「待替换值」坑（2026-11-04 真机实锤）：声明含 var() 时长手属性
// 读出来是空串（camelCase 与 getPropertyValue 都一样），只有简写
// getPropertyValue('background') 给原文——所以先试长手、空了再退简写，
// 拿到原文解析 var 后再按内容归类（渐变/图片→image，实色→color）
export function classifyBgValue(raw, lookup) {
  const resolved = resolveCssVars(raw, lookup);
  if (/gradient|url\(/i.test(resolved)) return { kind: 'image', text: resolved };
  const c = parseCssColor(resolved);
  if (c && !(c.r === 0 && c.g === 0 && c.b === 0 && c.a === 0)) return { kind: 'color', text: resolved };
  return null; // transparent / 解析不出
}
export function findBgSource(el) {
  if (!el || el.nodeType !== 1) return null;
  const doc = el.ownerDocument;
  const cs = getComputedStyle(el);
  const lookup = (n) => cs.getPropertyValue(n);
  // 按 longhand 模拟层叠：每条命中规则的声明展开成 image/color 两条记录
  //（简写会互相重置——实色简写把图片层清成 none、渐变简写把底色清成透明），
  // 最后各取 (important, 特异度, 文档序) 的胜者。这层建模是必须的：
  // 禁用态「background:var(--g3)」这类高特异度实色简写会真的盖掉基础渐变
  const decls = [];
  let order = 0;
  const push = (prop, resolved, spec, imp, sel) => decls.push({ prop, spec, imp, sel, resolved, order: order++ });
  const addStyle = (spec, imp, sel, st) => {
    const imgRaw = (st.getPropertyValue('background-image') || '').trim();
    const colRaw = (st.getPropertyValue('background-color') || '').trim();
    if (imgRaw || colRaw) {
      if (imgRaw) push('image', /none|initial/i.test(imgRaw) ? 'none' : resolveCssVars(imgRaw, lookup), spec, imp, sel);
      if (colRaw) {
        const cleared = colRaw === 'transparent' || colRaw === 'rgba(0, 0, 0, 0)' || /initial/i.test(colRaw);
        push('color', cleared ? 'transparent' : resolveCssVars(colRaw, lookup), spec, imp, sel);
      }
      return;
    }
    // 长手全空 = 声明含 var()（待替换值），只有简写给原文
    const sh = (st.getPropertyValue('background') || '').trim();
    if (!sh) return;
    const cl = classifyBgValue(sh, lookup);
    if (cl && cl.kind === 'image') {
      push('image', cl.text, spec, imp, sel);
      push('color', 'transparent', spec, imp, sel); // 渐变简写把底色重置成透明
    } else if (cl && cl.kind === 'color') {
      push('image', 'none', spec, imp, sel); // 实色简写把图片层重置成无
      push('color', cl.text, spec, imp, sel);
    } else {
      // transparent/解析不出：同样算一次双重置
      push('image', 'none', spec, imp, sel);
      push('color', 'transparent', spec, imp, sel);
    }
  };
  const walk = (rules) => {
    for (const r of rules) {
      if (r.type !== 1) {
        if (r.type === 4 || r.type === 12) {
          try {
            walk(r.cssRules);
          } catch (e) {}
        }
        continue;
      }
      // 右击时指针必然悬停在元素上，:hover/:active/:focus 的临时底不算基础外观
      if (/:hover|:active|:focus/i.test(r.selectorText || '')) continue;
      let hit = false;
      try {
        hit = el.matches(r.selectorText);
      } catch (e) {
        continue; // 带伪元素/非法选择器：打不中这个元素
      }
      if (!hit) continue;
      const imp =
        r.style.getPropertyPriority('background') === 'important' ||
        r.style.getPropertyPriority('background-image') === 'important' ||
        r.style.getPropertyPriority('background-color') === 'important';
      addStyle(specificityOf(r.selectorText), imp, r.selectorText, r.style);
    }
  };
  for (const sheet of doc.styleSheets) {
    let rules = null;
    try {
      rules = sheet.cssRules;
    } catch (e) {
      continue; // 跨源样式表读不到：跳过
    }
    walk(rules);
  }
  // 内联 style 优先级最高（本应用样式表不用 !important 画背景，从简不比 important）；
  // 内联的 var() 声明同样有"待替换值"坑，走同一套展开逻辑
  addStyle(Infinity, false, '(inline)', el.style);
  // 各 longhand 的级联胜者
  const win = (prop) =>
    decls
      .filter((d) => d.prop === prop)
      .sort((a, b) => (a.imp !== b.imp ? a.imp === b.imp ? 0 : a.imp ? -1 : 1 : a.spec !== b.spec ? b.spec - a.spec : b.order - a.order))[0] || null;
  const imgTop = win('image');
  if (imgTop && imgTop.resolved !== 'none' && /gradient|url\(/i.test(imgTop.resolved)) {
    return { kind: 'image', text: imgTop.resolved, sel: imgTop.sel };
  }
  const colTop = win('color');
  if (colTop && colTop.resolved !== 'transparent') {
    const c = parseCssColor(colTop.resolved);
    if (c && !(c.r === 0 && c.g === 0 && c.b === 0 && c.a === 0)) return { kind: 'color', text: colTop.resolved, sel: colTop.sel };
  }
  return null;
}
// 未登记元素：生成结构选择器。优先用稳定类名锚点（.icon-btn/.pill/.btn 等），
// 比纯 tag:nth-of-type 链更抗排序变化；实在没有类名才回落序号链（止于 #app）
export function structuralSelector(el) {
  const clsOf = (node) => {
    const cls = typeof node.className === 'string' ? node.className : (node.getAttribute && node.getAttribute('class')) || '';
    return cls.split(/\s+/).filter((c) => c && !/^s-[A-Za-z0-9_-]+$/.test(c));
  };
  // 自身或近祖先有稳定类名：直接用 ".类名" 锚定（多类取首个非 svelte 作用域类）
  let cur = el;
  for (let up = 0; up < 3 && cur && cur.id !== 'app'; up++) {
    const stable = clsOf(cur).find((c) => /^(icon-btn|pill|btn|chip|bar-item|conv|proj|row-btn|foot-btn|theme-opt|agents-editor|ctx-ring|ctx-card|send-btn|new-chat|composer-input|tool-card|tool-preview|tool-head|thinking-body|approval-box|cp-bar|cp-undo|head-btn|user-text|proj-name|conv-title)$/.test(c));
    if (stable) return `.${stable}`;
    cur = cur.parentElement;
  }
  const parts = [];
  cur = el;
  while (cur && cur.id !== 'app') {
    if (cur.id && cur !== el) {
      parts.unshift(`#${CSS.escape(cur.id)}`);
      return parts.join(' > ');
    }
    const parent = cur.parentElement;
    if (!parent) break;
    const sibs = [...parent.children].filter((c) => c.tagName === cur.tagName);
    let seg = cur.tagName.toLowerCase();
    if (sibs.length > 1) seg += `:nth-of-type(${sibs.indexOf(cur) + 1})`;
    parts.unshift(seg);
    cur = parent;
  }
  return parts.join(' > ');
}
// —— 单元素定位（修"改一处、同类全改"）：给具体元素盖一个 data-mt 唯一标记，
// 规则写成 #app [data-mt="mtXXXX"]，只作用这一个元素；Svelte 重渲染会重建节点丢标记，
// 所以存下"从 #app 起的孩子序号路径"，每次更新后按路径找回并补盖 ——
export function pathOf(el) {
  const parts = [];
  let cur = el;
  while (cur && cur.id !== 'app') {
    const parent = cur.parentElement;
    if (!parent) break;
    parts.unshift(`${cur.tagName.toLowerCase()}:${[...parent.children].indexOf(cur)}`);
    cur = parent;
  }
  return parts.join('/');
}
export function findByPath(path, root) {
  let cur = root || document.getElementById('app');
  for (const seg of String(path || '').split('/').filter(Boolean)) {
    const i = seg.lastIndexOf(':');
    const tag = seg.slice(0, i);
    const idx = Number(seg.slice(i + 1));
    if (!cur || !cur.children[idx]) return null;
    cur = cur.children[idx];
    if (tag && cur.tagName.toLowerCase() !== tag) return null;
  }
  return cur;
}
export function makeUid() {
  return 'mt' + Math.random().toString(36).slice(2, 8);
}
export function hexWithAlpha(hex, opacityPct) {
  const m = /^#([0-9a-f]{6})$/i.exec(hex || '');
  if (!m) return hex;
  const a = Math.min(1, Math.max(0, (opacityPct ?? 100) / 100));
  const r = parseInt(m[1].slice(0, 2), 16);
  const g = parseInt(m[1].slice(2, 4), 16);
  const b = parseInt(m[1].slice(4, 6), 16);
  return `rgba(${r}, ${g}, ${b}, ${a})`;
}
// 微调规则 → CSS（拼在手动 CSS 之前，手写 CSS 永远有最终话语权）；
// 只输出当初亲手改过的属性（有键才输出），没动过的背景/颜色不会凭空出现。
// background 用 background-color 而非简写，避免盖掉原有的渐变/图片/transparent 语义
// inFrame=true：给 iframe 文档用的规则——选择器相对 iframe 文档，不挂 #app 前缀，
// 且每条声明带 !important。为什么：插件/网页自己的样式常写成两级链（`.row .k`），
// iframe 里没有 `#app` 前缀加权重（(0,1,0) 打不过 (0,2,0)），2026-11-03 真机实测
// 「微调了没反应」就是它——微调是用户显式覆盖，加重要度才是预期语义。
export function microCss(tweaks, inFrame = false) {
  let out = '';
  const imp = inFrame ? ' !important' : '';
  for (const t of Object.values(tweaks || {})) {
    if (!t || !t.sel) continue;
    const decls = [];
    if (t.color) decls.push(`color: ${t.color}${imp}`);
    if (t.bgImage) {
      // 渐变底（主按钮的橙）：整条重建过的渐变写进 background-image；
      // 不能再叠 background-color——半透明渐变下面垫一层实色会把透明度吃掉
      decls.push(`background-image: ${t.bgImage}${imp}`);
    } else if (t.bg) {
      decls.push(`background-color: ${hexWithAlpha(t.bg, t.bgOpacity ?? 100)}${imp}`);
    }
    if (t.border) decls.push(`border-style: solid${imp}`, `border-width: ${t.border}${imp}`);
    if (t.borderColor) decls.push(`border-color: ${t.borderColor}${imp}`);
    if (t.blur != null && String(t.blur) !== '' && (parseInt(t.blur, 10) || 0) !== 0) decls.push(`backdrop-filter: blur(${parseInt(t.blur, 10) || 0}px)${imp}`);
    if (t.radius) decls.push(`border-radius: ${t.radius}${imp}`);
    if (t.fontSize) decls.push(`font-size: ${t.fontSize}${imp}`);
    if (!decls.length) continue;
    const scope = inFrame ? '' : '#app ';
    // 单个元素：用 data-mt 唯一标记；同类全部：用元素选择器
    if (t.uid) out += `/* 微调:${t.name || t.sel} · 仅此元素 */\n${scope}[data-mt="${t.uid}"] { ${decls.join('; ')} }\n`;
    else out += `/* 微调:${t.name || t.sel} · 同类全部 */\n${scope}${t.sel} { ${decls.join('; ')} }\n`;
  }
  return out;
}
