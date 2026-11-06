// 极简 markdown → HTML（聊天助手消息渲染；起因「星号太多太乱」）。
// 安全第一：先整体转义 HTML，再在已转义文本上做结构替换——{@html} 吃到的串里
// 任何原始 < > 都已是 &lt; &gt;，模型输出/插件消息里的 HTML 永远不会被执行。
// 支持：``` 围栏代码块、`行内代码`、**粗体**、*斜体*、### 标题、- 列表、1. 有序列表、
// > 引用、--- 分隔线、GFM 表格（| 表头 | + | --- | 分隔行）、[文字](http(s)链接)。链接只放行 http/https，其余协议剥成纯文本。

// —— 轻量语法高亮（2026-10-31 UI 改版：按效果图加「代码块带高亮」；035 轮 Shiki 4.5MB 过重
//    被整轮回退，这里自绘：单趟组合正则，注释/字符串/数字/函数调用/关键字/类型六类 token）。
//    输入必须是 escapeHtml 之后的文本；HTML 实体先占位再还原，保证 &lt; 不被标识符规则拆散。 */
const HL_KW = new Set(
  ('const let var function return if else for while do break continue class extends new import export from default ' +
    'try catch finally throw async await typeof instanceof in of void delete yield static get set this super ' +
    'null undefined true false NaN Infinity def self None True False and or not is lambda pass raise with as elif ' +
    'fn let mut pub impl struct enum trait match use mod where dyn ref move loop crate type interface namespace ' +
    'package public private protected virtual override explicit operator sizeof typedef const_cast switch case').
    split(' '),
);
const HL_HASH_LANGS = new Set(['python', 'py', 'bash', 'sh', 'shell', 'zsh', 'yaml', 'yml', 'toml', 'ruby', 'rb', 'conf', 'ini', 'makefile', 'dockerfile']);
export function highlightCode(escaped, lang) {
  const l = (lang || '').toLowerCase();
  // 实体占位：&lt; 里的 "lt" 不能被标识符规则吃掉。占位用 Unicode 私有区字符
  //（\uE000+i 单字符）——数字/字母占位会被 tokenize 规则命中，还原就断了。
  // &quot; 例外：先还原成字面 " 再 tokenize（转义只防属性注入，这里是文本节点；
  // 不还原的话字符串规则全瞎），其余实体（&lt; &gt; &amp; 数字实体）保持占位。
  const ents = [];
  const src = escaped
    .replace(/&quot;/g, '"')
    .replace(/&[a-zA-Z#][a-zA-Z0-9]*;/g, (m) => String.fromCharCode(0xe000 + ents.push(m) - 1));
  const hash = HL_HASH_LANGS.has(l);
  // 顺序刻意为「字符串优先于注释」：字符串里的 https:// 和 // 不能被注释规则截断
  //（注释里的引号被截断属可接受的次要损伤）。行注释加 (?<!:) 排除 "://" 的协议斜杠。
  const re = new RegExp(
    [
      '(\'(?:[^\'\\\\\\n]|\\\\.)*\'|"(?:[^"\\\\\\n]|\\\\.)*"|`(?:[^`\\\\]|\\\\.)*`)', // 1 字符串/模板串
      '(\\/\\*[\\s\\S]*?\\*\\/|(?<!:)\\/\\/[^\\n]*)', // 2 块注释/行注释（C 系）
      ...(hash ? ['(#[^\\n]*)'] : ['(\\u0004\\u0004)']), // 3 # 注释仅脚本语言；占位组永不匹配
      '(\\b\\d+(?:\\.\\d+)?(?:e[+-]?\\d+)?\\b)', // 4 数字
      '([A-Za-z_$][\\w$]*)(?=\\s*\\()', // 5 函数调用
      '([A-Za-z_$][\\w$]*)', // 6 标识符/关键字
    ].join('|'),
    'g',
  );
  const out = src.replace(re, (m, str, cm, hash_, num, fn, id) => {
    if (str) return `<span class="tk-str">${m}</span>`;
    if (cm) return `<span class="tk-cm">${m}</span>`;
    if (hash_) return `<span class="tk-cm">${m}</span>`;
    if (num) return `<span class="tk-num">${m}</span>`;
    if (fn) return HL_KW.has(m) ? `<span class="tk-kw">${m}</span>` : `<span class="tk-fn">${m}</span>`;
    if (id) {
      if (HL_KW.has(m)) return `<span class="tk-kw">${m}</span>`;
      if (/^[A-Z]/.test(m)) return `<span class="tk-type">${m}</span>`;
    }
    return m;
  });
  return out.replace(/[\uE000-\uF8FF]/g, (ch) => ents[ch.charCodeAt(0) - 0xe000]);
}

// 实体占位/还原（JSON 与命令输出两条高亮共用）
const HL_SET_ENTS = (s) => {
  const ents = [];
  const out = s
    .replace(/&quot;/g, '"')
    .replace(/&[a-zA-Z#][a-zA-Z0-9]*;/g, (m) => String.fromCharCode(0xe000 + ents.push(m) - 1));
  return { out, ents };
};
const HL_RESTORE = (out, ents) => out.replace(/[\uE000-\uF8FF]/g, (ch) => ents[ch.charCodeAt(0) - 0xe000]);

// JSON 专用高亮（工具「参数」块）：键与值必须分色，否则整块一片绿。
// 键 = 后跟冒号的字符串（tk-key 蓝），值串 tk-str 绿，数字 tk-num，true/false/null tk-kw，
// 括号逗号冒号 tk-punc 压暗——一眼能看出结构和内容的分界。
export function highlightJson(escaped) {
  const { out: src, ents } = HL_SET_ENTS(escaped);
  const re = /("(?:[^"\\\n]|\\.)*")(\s*:)?|(-?\b\d+(?:\.\d+)?(?:[eE][+-]?\d+)?\b)|\b(true|false|null)\b|([{}[\],:])/g;
  const out = src.replace(re, (m, str, colon, num, lit, punc) => {
    if (str) {
      return colon
        ? `<span class="tk-key">${str}</span><span class="tk-punc">${colon}</span>`
        : `<span class="tk-str">${str}</span>`;
    }
    if (num) return `<span class="tk-num">${num}</span>`;
    if (lit) return `<span class="tk-kw">${lit}</span>`;
    if (punc) return `<span class="tk-punc">${m}</span>`;
    return m;
  });
  return HL_RESTORE(out, ents);
}

// 命令输出高亮（工具「结果」块）：输出不是代码，按语义上色才有用——
// 分隔横幅（--- stdout ---）压成注释灰；fatal/error/failed 报红；git 的 ??/?? 与
// +/- 增删行分色；路径（含分隔符或带扩展名）标蓝；退出码/数量类数字标橙。
// 先按整行规则判定（行首 fatal:、+/- 行首），再在行内跑通用 token 规则。
export function highlightOutput(escaped) {
  const { out: src, ents } = HL_SET_ENTS(escaped);
  const lines = src.split('\n');
  const out = lines.map((line) => {
    // 整行分隔横幅：--- stdout --- / --- stderr --- / === 分段 ===
    if (/^\s*-{2,}.*-{2,}\s*$/.test(line) || /^\s*={2,}.*=*\s*$/.test(line)) {
      return `<span class="tk-cm">${line}</span>`;
    }
    // 报错行：fatal:/error:/Error:/failed/FAILED/denied/traceback（行首或缩进后）
    const errHit = line.match(/^(\s*)(fatal:|error:|Error:|ERROR:|failed|FAILED|Traceback|panic:)/);
    if (errHit) {
      return `<span class="tk-err-hl">${line}</span>`;
    }
    // git 状态标记：?? 未跟踪（黄）、+ 新增行（绿）、- 删除行（红）
    const gitMark = line.match(/^(\s*)(\?\?|\+\+|\+|--|-)(\s|$)/);
    if (gitMark && /^(\?\?|\+\+)/.test(line.trim())) {
      return line.replace(/^(\s*)(\?\?|\+\+)/, '$1<span class="tk-warn">$2</span>');
    }
    // 行内 token：路径 / 数字 / 键值对 / 关键字
    return line.replace(
      /((?:[A-Za-z]:[\\/]|\/)[^\s:,;)]+|[\w.-]+\.(?:js|ts|mjs|json|md|html|css|txt|exe|rs|toml|yml|yaml|py|log|png|jpg))|(\b(?:exit|code|timed_out|status)\b)(=)(\S+)|(-?\b\d+(?:\.\d+)?\b)|(\b(?:true|false|null|ok|done|success|warning|warn|deprecated)\b)/g,
      (m, path, key, eq, val, num, lit) => {
        if (path) return `<span class="tk-path">${path}</span>`;
        if (key) return `<span class="tk-key">${key}</span>${eq}<span class="tk-val">${val}</span>`;
        if (num) return `<span class="tk-num">${num}</span>`;
        if (lit) {
          if (/^(warning|warn|deprecated)$/i.test(lit)) return `<span class="tk-warn">${lit}</span>`;
          return `<span class="tk-kw">${lit}</span>`;
        }
        return m;
      },
    );
  });
  return HL_RESTORE(out.join('\n'), ents);
}

// 工具详情块（参数/结果面板）用的高亮入口：先转义再高亮，输出可直接 {@html}。
// mode='json' 走 JSON 分色（键/值/数字/字面量/标点）；mode='output' 走命令输出语义色；
// 其余走通用代码高亮。2026-10-31：工具详情此前是纯文本 <pre>，和正文代码块两条
// 渲染路径，导致「正文有高亮、详情没高亮」。
export function highlightText(raw, mode) {
  const esc = String(raw ?? '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
  if (mode === 'json') return highlightJson(esc);
  if (mode === 'output') return highlightOutput(esc);
  return highlightCode(esc, mode || '');
}
export function md(src) {
  if (!src) return '';
  const escapeHtml = (s) =>
    String(s)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;');
  // 0) 原始工具调用文本（模型把 <tool_call> 协议当正文吐出来的历史消息）：
  //    不静默丢，但也不让它铺满一屏——收成一个可展开的「原始工具调用文本」块。
  const leaks = [];
  let raw = String(src).replace(/<tool_call>[\s\S]*?(?:<\/tool_call>|$)/g, (m) => {
    leaks.push(m);
    return `\n\n\u0002${leaks.length - 1}\u0002\n\n`;
  });
  // 1) 整体转义（引号也转，防属性注入）
  let t = escapeHtml(raw);
  // 2) 围栏代码块（先把代码内容抠出来占位，别被后面的行内规则误伤）
  //    2026-10-30：加代码块头（语言 + 复制按钮）；复制走 content 上的事件委托。
  const blocks = [];
  t = t.replace(/```(\w*)\n?([\s\S]*?)```/g, (_, lang, code) => {
    const label = (lang || 'text').toLowerCase();
    blocks.push(
      `<div class="md-codeblock"><div class="md-codehead"><span class="md-codelang">${label}</span>` +
        `<button class="md-copy" type="button" title="复制代码">复制</button></div>` +
        `<pre class="md-pre"><code>${highlightCode(code.replace(/\n$/, ''), label)}</code></pre></div>`
    );
    return `\u0000${blocks.length - 1}\u0000`;
  });
  // 3) 行内代码（同样占位）
  const codes = [];
  t = t.replace(/`([^`\n]+)`/g, (_, c) => {
    codes.push(`<code class="md-code">${c}</code>`);
    return `\u0001${codes.length - 1}\u0001`;
  });
  // 4) 标题（### 以内；聊天里没 h1/h2）
  t = t
    .replace(/^######\s+(.+)$/gm, '<h6 class="md-h">$1</h6>')
    .replace(/^#####\s+(.+)$/gm, '<h5 class="md-h">$1</h5>')
    .replace(/^####\s+(.+)$/gm, '<h4 class="md-h">$1</h4>')
    .replace(/^###\s+(.+)$/gm, '<h4 class="md-h">$1</h4>')
    .replace(/^##\s+(.+)$/gm, '<h4 class="md-h">$1</h4>')
    .replace(/^#\s+(.+)$/gm, '<h4 class="md-h">$1</h4>');
  // 5) 分隔线
  t = t.replace(/^---+\s*$/gm, '<hr class="md-hr" />');
  // 6) 粗体/斜体（粗体在先，防 ** 被 * 规则吃掉）
  t = t
    .replace(/\*\*([^*\n]+)\*\*/g, '<strong>$1</strong>')
    .replace(/(^|[^*\w])\*([^*\n]+)\*(?=[^*\w]|$)/g, '$1<em>$2</em>');
  // 7a) 图片（必须先于链接：`![alt](url)` 若先走链接规则，会剩下一个孤立的 `!`）。
  //     白名单同链接：http/https + 站内 /api/workspace/（工作区文件、文档预览图）。
  //     data: 只放行位图 dataURL（截图内置浏览器会产出），不放 svg——svg 内可带脚本。
  t = t.replace(/!\[([^\]\n]*)\]\(([^)\s]+)\)/g, (m, alt, url) => {
    const ok =
      /^https?:\/\//i.test(url) ||
      url.startsWith('/api/workspace/') ||
      /^data:image\/(png|jpe?g|gif|webp|bmp);base64,/i.test(url);
    if (!ok) return alt || '';
    return (
      `<img class="md-img" src="${url}" alt="${alt}" loading="lazy" ` +
      `title="点击查看大图" data-md-img="1" />`
    );
  });
  // 7b) 链接：放行 http/https（外链）与 /api/workspace/（站内预览，宿主拦截跳预览标签）
  t = t.replace(/\[([^\]\n]+)\]\(([^)\s]+)\)/g, (m, text, url) => {
    if (/^https?:\/\//i.test(url)) return `<a class="md-a" href="${url}" target="_blank" rel="noreferrer">${text}</a>`;
    if (url.startsWith('/api/workspace/')) return `<a class="md-a doc-cite" href="${url}">${text}</a>`;
    return text;
  });
  // 8) GFM 表格：首行含 |、次行是 | --- | 分隔行（:--- 左对齐只作识别、对齐样式不做）
  t = t.replace(/(?:^[ \t]*\|?.*\|.*\n?)+/gm, (block) => {
    const lines = block.trimEnd().split('\n').map((l) => l.trim());
    if (lines.length < 2 || !/^\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$/.test(lines[1])) return block;
    const cells = (l) => l.replace(/^\|/, '').replace(/\|$/, '').split('|').map((c) => c.trim());
    // td/th 发稳定类：微调拾取对无类名元素只出结构选择器（改布局就漂），有类才能
    // 「长按单元格→选中表格单元格」直接调（2026-11-03 截图断网表格实测）
    const head = cells(lines[0]).map((c) => `<th class="md-th">${c}</th>`).join('');
    const rows = lines.slice(2).filter((l) => /\|/.test(l))
      .map((l) => `<tr>${cells(l).map((c) => `<td class="md-td">${c}</td>`).join('')}</tr>`).join('');
    return `<table class="md-table"><thead><tr>${head}</tr></thead><tbody>${rows}</tbody></table>`;
  });
  // 9) 列表（- / * / + 与 1. 两种；连续行整组成 ul/ol）
  t = t.replace(/(?:^[ \t]*[-*+]\s+.+\n?)+/gm, (block) => {
    const items = block.trimEnd().split('\n').map((l) => `<li>${l.replace(/^[ \t]*[-*+]\s+/, '')}</li>`).join('');
    return `<ul class="md-ul">${items}</ul>`;
  });
  t = t.replace(/(?:^[ \t]*\d+[.、]\s+.+\n?)+/gm, (block) => {
    const items = block.trimEnd().split('\n').map((l) => `<li>${l.replace(/^[ \t]*\d+[.、]\s+/, '')}</li>`).join('');
    return `<ol class="md-ol">${items}</ol>`;
  });
  // 10) 引用
  t = t.replace(/(?:^&gt;\s?.+\n?)+/gm, (block) => {
    const lines = block.trimEnd().split('\n').map((l) => l.replace(/^&gt;\s?/, '')).join('<br />');
    return `<blockquote class="md-quote">${lines}</blockquote>`;
  });
  // 11) 段落与换行：双换行分段，单换行转 <br>
  //     占位符（代码块/折叠块）不能被包进 <p>——否则还原后是 <p><div ...> 的非法嵌套
  t = t
    .split(/\n{2,}/)
    .map((p) => (p.trim() && !/^<(h[1-6]|ul|ol|pre|hr|blockquote|div|table)\b/i.test(p.trim()) && !/^\u0000\d+\u0000/.test(p.trim()) && !/^\u0002\d+\u0002/.test(p.trim()) ? `<p class="md-p">${p.trim()}</p>` : p.trim()))
    .join('');
  t = t.replace(/\n/g, '<br />');
  // 12) 还原占位
  t = t
    .replace(/\u0000(\d+)\u0000/g, (_, i) => blocks[+i])
    .replace(/\u0001(\d+)\u0001/g, (_, i) => codes[+i])
    .replace(/\u0002(\d+)\u0002/g, (_, i) => {
      const code = escapeHtml(leaks[+i] || '');
      return `<details class="md-toolcall"><summary>原始工具调用文本 · 已折叠</summary><pre>${code}</pre></details>`;
    });
  return t;
}
