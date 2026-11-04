import { test } from 'node:test';
import assert from 'node:assert/strict';
import { md, highlightText } from '../src/lib/md.js';

test('HTML 一律转义，模型输出里的 script 不会被执行', () => {
  const html = md('<script>alert(1)</script>');
  assert.ok(!html.includes('<script>'));
  assert.ok(html.includes('&lt;script&gt;'));
});

test('代码块带语言与复制按钮，内容保持转义、不被包进段落', () => {
  const html = md('```js\nconst a = "<x>";\n```');
  assert.ok(html.includes('class="md-codeblock"'), html);
  assert.ok(html.includes('class="md-codelang">js<'), html);
  assert.ok(html.includes('class="md-copy"'), html);
  assert.ok(html.includes('&lt;x&gt;'), html);
  assert.ok(!html.includes('<p class="md-p"><div'), '代码块不能被包进 <p>');
});

test('代码块里的 markdown 符号不被二次解析', () => {
  const html = md('```\n**not bold** `not code`\n```');
  // 2026-10-31 起代码块带语法高亮，tokenize 会给词包 span，字面不再连续——
  // 意图保持「不解析成 markdown 结构」：星号/反引号原样在，不产生 <strong>/行内码
  assert.ok(html.includes('**'), html);
  assert.ok(html.includes('`not code`'), html);
  assert.ok(!html.includes('<strong>'), html);
  assert.ok(!html.includes('md-code"'), html);
});

test('链接只放行 http/https 与站内预览，javascript: 剥成纯文本', () => {
  assert.ok(md('[a](https://example.com)').includes('href="https://example.com"'));
  assert.ok(md('[a](/api/workspace/file?path=x)').includes('doc-cite'));
  const bad = md('[a](javascript:alert(1))');
  assert.ok(!bad.includes('href='), bad);
  assert.ok(bad.includes('a'), bad);
});

test('表格与列表结构正常', () => {
  const table = md('| A | B |\n| --- | --- |\n| 1 | 2 |');
  assert.ok(table.includes('<table class="md-table">'), table);
  assert.ok(table.includes('<td class="md-td">1</td>'), table);
  assert.ok(table.includes('<th class="md-th">A</th>'), table);
  const ul = md('- 一\n- 二');
  assert.ok(ul.includes('<ul class="md-ul"><li>一</li><li>二</li></ul>'), ul);
});

test('模型把 <tool_call> 协议吐成正文时折叠，不铺满屏幕', () => {
  const html = md(
    '先说明一下。\n\n<tool_call>\n<function=write_file>\n<parameter=content>\n<!DOCTYPE html>\n</parameter>\n</function>\n</tool_call>\n\n后文还在。'
  );
  assert.ok(html.includes('class="md-toolcall"'), html);
  assert.ok(html.includes('原始工具调用文本'), html);
  assert.ok(html.includes('后文还在'), html);
  assert.ok(!/<tool_call>/.test(html), '原始尖括号不能出现（必须转义）');
  // 没闭合（流式中/历史脏数据）也要能收住
  const unclosed = md('<tool_call>\n<function=shell>\n');
  assert.ok(unclosed.includes('class="md-toolcall"'), unclosed);
});

test('语法高亮：关键字/函数/数字上色，字符串内关键字不上色', () => {
  const html = md('```js\nconst x = foo(42);\nconst s = "const";\n```');
  assert.ok(html.includes('<span class="tk-kw">const</span>'), html);
  assert.ok(html.includes('<span class="tk-fn">foo</span>'), html);
  assert.ok(html.includes('<span class="tk-num">42</span>'), html);
  // 字符串里的 const 是整体 tk-str，不能出现关键字 span 套在实体外
  assert.ok(html.includes('<span class="tk-str">"const"</span>'), html);
});

test('语法高亮：URL 的 // 不被当注释，实体不被拆散', () => {
  const html = md('```js\nconst u = "https://example.com/a"; // 注释\n```');
  assert.ok(html.includes('<span class="tk-str">"https://example.com/a"</span>'), html);
  assert.ok(html.includes('<span class="tk-cm">// 注释</span>'), html);
  // 含 HTML 的代码：真实尖括号转义成 &lt; 后必须原样完整（实体占位保证不被拆散）
  const esc = md('```html\n<div a="<b">x</div>\n```');
  assert.ok(esc.includes('&lt;div'), esc);
  assert.ok(!esc.includes('&<span'), esc);
  assert.ok(!esc.includes('<script>'), esc);
});

test('语法高亮：脚本语言支持 # 注释', () => {
  const html = md('```python\n# 这是注释\ndef run():\n```');
  assert.ok(html.includes('<span class="tk-cm"># 这是注释</span>'), html);
  assert.ok(html.includes('<span class="tk-kw">def</span>'), html);
});

test('highlightText：先转义再高亮，输出可直接 {@html}；JSON 参数能上色', () => {
  const out = highlightText('{"command":"git init && git status --short"}', 'json');
  // 2026-10-31 二次复验：键走 tk-key（蓝）、值走 tk-str（绿），不能再整片同色
  assert.ok(out.includes('<span class="tk-key">"command"</span>'), out);
  assert.ok(out.includes('<span class="tk-str">"git init &amp;&amp; git status --short"</span>'), out);
  // 危险字符必须已转义（{@html} 直出）
  const evil = highlightText('<img src=x onerror=alert(1)>', 'html');
  assert.ok(!evil.includes('<img'), evil);
  assert.ok(evil.includes('&lt;img'), evil);
});

test('工具参数块：JSON 键与值必须分色，不能整片同色', () => {
  const out = highlightText('{"command":"git status","timeout":30}', 'json');
  assert.ok(out.includes('<span class="tk-key">"command"</span>'), out);
  assert.ok(out.includes('<span class="tk-str">"git status"</span>'), out);
  assert.ok(out.includes('<span class="tk-num">30</span>'), out);
  assert.ok(out.includes('<span class="tk-punc">'), out);
  // 键和值不能都走 tk-str
  assert.ok(!out.includes('<span class="tk-str">"command"</span>'), '键不能当值上色');
});

test('工具结果块：报错整行标红、分隔横幅压灰、git 状态标黄、key=value 分色', () => {
  const out = highlightText(
    'exit=128 timed_out=false\n--- stderr ---\nfatal: not a git repository\n?? index.html',
    'output',
  );
  assert.ok(out.includes('<span class="tk-err-hl">fatal: not a git repository</span>'), out);
  assert.ok(out.includes('<span class="tk-cm">--- stderr ---</span>'), out);
  assert.ok(out.includes('<span class="tk-warn">??</span>'), out);
  assert.ok(out.includes('<span class="tk-key">exit</span>'), out);
  assert.ok(out.includes('<span class="tk-val">128</span>'), out);
  // 结果也必须上色（用户复验：结果没颜色不行）
  assert.ok(out.includes('tk-'), out);
});

test('图片语法：渲染成 img，且必须先于链接规则（不留孤立感叹号）', () => {
  const html = md('看图：\n\n![截图](/api/workspace/raw?path=.agent/screenshots/shot-1.png)\n');
  assert.ok(html.includes('class="md-img"'), html);
  assert.ok(html.includes('src="/api/workspace/raw?path=.agent/screenshots/shot-1.png"'), html);
  assert.ok(!html.includes('!<a'), '不能残留孤立感叹号');
  // http 外链图片也放行
  assert.ok(md('![x](https://example.com/a.png)').includes('md-img'));
});

test('图片语法：危险协议一律不渲染成 img', () => {
  for (const bad of [
    '![x](javascript:alert(1))',
    '![x](data:image/svg+xml;base64,PHN2Zz4=)',
    '![x](file:///c:/secret.png)',
  ]) {
    const html = md(bad);
    assert.ok(!html.includes('md-img'), `${bad} → ${html}`);
    assert.ok(!html.includes('<img'), `${bad} 不应产生 img`);
    assert.ok(!html.includes('javascript:'), 'javascript: 必须被剥掉');
  }
  // 位图 dataURL 放行（截图场景）
  assert.ok(md('![x](data:image/png;base64,iVBORw0KGgo=)').includes('md-img'));
});
