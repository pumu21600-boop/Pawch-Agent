<script>
  // 右侧面板「git」标签（2026-11-15 阶段 A-3，自 App.svelte 逐字平移）。
  // 审查清单/提交盒/就地 diff；数据（rv* 簇、gitInfo、dockCp）与动作（loadReview/
  // doCommit/undoCheckpoint…）留根——SSE 钩子与提交流程都在根上——这里只渲染 + 上行。
  import { createEventDispatcher } from 'svelte';
  import Select from '../../Select.svelte';
  import { gitStatusLabel } from '../format.js';
  const dispatch = createEventDispatcher();

  export let source = 'task'; // 'task' 本任务改动 | 'work' 未提交改动（bind 双向到根）
  export let files = []; // [{ path, status, adds, dels }]
  export let open = {}; // path → 是否展开 diff
  export let diff = {}; // path → { loading, text, error }
  export let baseline = ''; // source='task' 的检查点基线
  export let loading = false;
  export let error = '';
  export let dockCp = null; // { id, ... } 本任务起点检查点
  export let gitInfo = null; // { is_repo, git_missing, branch, head, upstream, ahead, behind }
  export let gitMissing = false;
  export let commitMsg = ''; // bind 双向到根
  export let commitBusy = false;
  export let commitSel = {}; // path → 是否勾选（暂存）
  export let commitSelN = 0;
  export let commitErr = '';

  const GIT_INSTALL_HINT = '装一个 Git for Windows（https://git-scm.com/downloads）后重启本软件即可恢复';

  // diff 文本 → 带行号的着色行（纯展示逻辑，随面板走；自 App.svelte 逐字平移）
  function rvParse(text) {
    const out = [];
    let oldNo = 0;
    let newNo = 0;
    for (const ln of String(text || '').split('\n')) {
      if (/^(diff --git|index |old mode|new mode|similarity index|dissimilarity index|rename from|rename to|copy from|copy to|deleted file|new file|--- |\+\+\+ )/.test(ln)) {
        out.push({ t: 'meta', s: ln });
        continue;
      }
      const m = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/.exec(ln);
      if (m) {
        oldNo = Number(m[1]);
        newNo = Number(m[3]);
        out.push({ t: 'hunk', s: ln });
        continue;
      }
      if (ln.startsWith('\\')) {
        out.push({ t: 'meta', s: ln });
      } else if (ln.startsWith('+')) {
        out.push({ t: 'add', o: '', n: newNo, s: ln.slice(1) });
        newNo += 1;
      } else if (ln.startsWith('-')) {
        out.push({ t: 'del', o: oldNo, n: '', s: ln.slice(1) });
        oldNo += 1;
      } else {
        out.push({ t: 'ctx', o: oldNo, n: newNo, s: ln.startsWith(' ') ? ln.slice(1) : ln });
        oldNo += 1;
        newNo += 1;
      }
    }
    return out;
  }
</script>

<div class="git-pane">
  {#if gitMissing}
    <!-- 缺 git 明说：撤销/检查点/改动/提交/审查 diff 都用不了 -->
    <div class="git-missing-note">没检测到 git：撤销、检查点、改动清单、提交、审查 diff 都用不了。{GIT_INSTALL_HINT}。</div>
  {:else if gitInfo && !gitInfo.is_repo}
    <!-- 只说事实不列功能清单（2026-11-12 用户：后面那串去掉） -->
    <div class="git-missing-note">当前工作区不是 git 仓库</div>
  {:else if gitInfo && gitInfo.is_repo}
    <div class="git-status">
      <span class="git-ref" title={gitInfo.upstream ? `上游：${gitInfo.upstream}` : '未设置上游分支'}>
        <svg class="git-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="6.5" cy="6" r="2.5"/><circle cx="6.5" cy="18" r="2.5"/><circle cx="17.5" cy="12" r="2.5"/><path d="M6.5 8.5v7M9 6h3.5a3 3 0 0 1 3 3v1"/></svg>
        <b>{gitInfo.branch}</b>{#if gitInfo.head}<span class="git-head">@{gitInfo.head}</span>{/if}
      </span>
      {#if gitInfo.ahead || gitInfo.behind}
        <span class="git-ab" title="相对上游的领先/落后提交数">{#if gitInfo.ahead}↑{gitInfo.ahead}{/if}{#if gitInfo.behind}↓{gitInfo.behind}{/if}</span>
      {/if}
    </div>
  {/if}
  <!-- svelte-ignore a11y-label-has-associated-control -->
  <div class="rv-head">
    <!-- 自绘下拉（2026-11-12）：原生 select 的弹出层是系统画的，
         选中项灰底高亮改不了——换 Select 组件走统一选中口径 -->
    <div class="rv-src">
      <Select
        bind:value={source}
        options={[
          { value: 'task', label: '本任务改动' },
          { value: 'work', label: '未提交改动' },
        ]}
        title="切换范围"
        on:change={() => dispatch('rangechanged')}
      />
    </div>
    {#if files.length}
      <span class="rv-totals">
        <b>{files.length}</b> 个文件<span class="rv-tsep">·</span><span class="review-add">+{files.reduce((s, f) => s + (f.adds || 0), 0)}</span><span class="rv-tsep">·</span><span class="review-del">-{files.reduce((s, f) => s + (f.dels || 0), 0)}</span>
      </span>
    {/if}
    <span class="flex-sp"></span>
    {#if dockCp && source === 'task' && baseline === dockCp.id}
      <button class="rv-btn" title="回滚到本任务起点（删除其后新建的已跟踪文件）" on:click={() => dispatch('undo')}>撤销</button>
    {/if}
    <button class="rv-btn" class:spin={loading} title="重新读取（清单 + 分支状态）" on:click={() => dispatch('refresh')}>
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M19 12a7 7 0 1 1-2.05-4.95" /><path d="M19 3.5V7h-3.5" /></svg>
    </button>
  </div>
  {#if source === 'work' && gitInfo && gitInfo.is_repo && !gitMissing && files.length}
    <!-- 提交盒：跟清单同档出现（未提交改动 = 暂存区），勾选即「这次提交带上它」 -->
    <div class="commit-box">
      <textarea
        class="agents-editor commit-msg"
        rows="2"
        spellcheck="false"
        placeholder="提交信息（一句话说清这次改了什么）"
        bind:value={commitMsg}
      ></textarea>
      <div class="commit-bar">
        <button class="commit-go" disabled={commitBusy} on:click={() => dispatch('commit')}>
          {commitBusy ? '提交中…' : `提交 ${commitSelN} 个文件`}
        </button>
        <span class="commit-hint">只做本地 commit，不推送；未跟踪文件默认不勾</span>
      </div>
      {#if commitErr}<div class="commit-err">{commitErr}</div>{/if}
    </div>
  {/if}
  {#if loading && !files.length}
    <div class="trace-empty">加载中…</div>
  {:else if error && !files.length}
    <div class="rp-empty">{error}</div>
  {:else if !files.length}
    <div class="rv-empty">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9" /><path d="M8.5 12h7" /></svg>
      <div class="rv-empty-t">{source === 'work' ? '没有未提交的改动' : '本任务还没有改动'}</div>
      <div class="rv-empty-d">{source === 'work' ? 'agent 或你自己改动文件后，这里会列出来（相对 HEAD）。' : 'agent 修改/创建文件后，这里会列出改动清单（相对本次任务起点）。'}</div>
    </div>
  {:else}
    <div class="rv-list">
      {#each files as f (f.path)}
        {@const name = f.path.split('/').pop()}
        {@const dir = f.path.split('/').slice(0, -1).join('/')}
        {@const dc = diff[f.path] || {}}
        <div class="rv-row" class:open={!!open[f.path]}>
          {#if source === 'work'}
            <input
              class="rv-check"
              type="checkbox"
              checked={!!commitSel[f.path]}
              title={`提交这个文件（${gitStatusLabel(f.status)}）`}
              on:change={() => dispatch('togglefile', f.path)}
            />
          {/if}
          <button
            class="rv-file"
            class:added={f.status === 'A' || f.status === '?'}
            class:deleted={f.status === 'D'}
            title={`${f.path}\n${gitStatusLabel(f.status)}\n右键：预览 / 复制路径`}
            on:click={() => dispatch('togglerow', f)}
            on:contextmenu={(e) => {
              e.preventDefault();
              dispatch('menu', { x: e.clientX, y: e.clientY, path: f.path });
            }}
          >
            <svg class="rv-fico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M7 3.5h6.5L18 8v12.5H7z" /><path d="M13.5 3.5V8H18" /></svg>
            <span class="rv-fname">{name}</span>
            {#if dir}<span class="rv-fdir">{dir}/</span>{/if}
            {#if f.status !== '?'}
              <span class="rv-stat"><span class="review-add">+{f.adds}</span><span class="review-del">-{f.dels}</span></span>
            {/if}
            <svg class="rv-chev" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6" /></svg>
          </button>
          {#if open[f.path]}
            <div class="rv-diffwrap">
              {#if dc.loading}
                <div class="rv-diffnote">加载中…</div>
              {:else if dc.error}
                <div class="rv-diffnote">读取失败: {dc.error}</div>
              {:else if !dc.text}
                <div class="rv-diffnote">没有可显示的 diff（该文件是未跟踪新增或无文本改动）。</div>
              {:else}
                <pre class="rv-diff">{#each rvParse(dc.text) as l, i (i)}<span class="rv-dl rv-dl-{l.t}">{#if l.t === 'add' || l.t === 'del' || l.t === 'ctx'}<span class="rv-dno">{l.o}</span><span class="rv-dno">{l.n}</span>{/if}<span class="rv-dtx">{l.s}</span></span>{/each}</pre>
              {/if}
            </div>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  /* git 面板样式：自 App.svelte 逐字平移（原件已删，.rv-warn 死样式与 .rv-menu 弹窗
     留在根）。.agents-editor/.trace-empty/.rp-empty 为共享类，App 原件仍在，此处副本。 */
  .agents-editor {
    width: 100%;
    box-sizing: border-box;
    background: var(--g0);
    color: var(--g7);
    border: 1px solid var(--g3);
    border-radius: 8px;
    padding: 8px 10px;
    font: 13px/1.6 inherit;
    outline: none;
  }
  .agents-editor:focus {
    border-color: var(--g5);
  }
  .trace-empty {
    margin: auto;
    text-align: center;
    color: var(--g5);
    font-size: 12px;
  }
  .rp-empty {
    margin: auto;
    text-align: center;
    color: var(--g5);
    font-size: 12.5px;
    line-height: 1.7;
    padding: 0 16px;
  }
  .flex-sp {
    flex: 1;
  }
  .git-missing-note {
    padding: 8px 12px;
    font-size: 12.5px;
    line-height: 1.6;
    color: var(--tint-orange-fg);
    border-bottom: 1px solid var(--g2);
  }
  .commit-msg {
    width: 100%;
    box-sizing: border-box;
    resize: vertical;
  }
  .commit-err {
    font-size: 12px;
    color: var(--tint-red-fg);
    white-space: pre-wrap;
    word-break: break-word;
  }
  /* —— git 面板状态行（分支 / HEAD / 领先落后）—— */
  .git-ref {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--g7);
    font-size: 12px;
    max-width: 260px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .git-ref b {
    font-weight: 600;
  }
  .git-ico {
    width: 13px;
    height: 13px;
    flex: 0 0 auto;
    color: var(--g5);
  }
  .git-head {
    color: var(--g5);
    font-family: monospace;
    font-size: 11px;
  }
  .git-ab {
    color: var(--g6);
    font-family: monospace;
    font-size: 11px;
  }
  /* —— git 面板（状态行 + 范围选择 + 提交盒 + 文件行）—— */
  .git-pane {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .git-status {
    flex: none;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px 6px;
  }
  /* 提交盒：未提交档顶部——一句话 + 「提交 N 个文件」；勾选在下面清单行上 */
  .commit-box {
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px 10px;
    border-bottom: 1px solid var(--g2);
    background: var(--g1);
  }
  .commit-msg {
    font-size: 12.5px;
    line-height: 1.5;
  }
  .commit-bar {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .commit-go {
    flex: none;
    background: var(--g3);
    border: 1px solid var(--g5);
    color: var(--g7);
    font-size: 12px;
    font-weight: 650;
    cursor: pointer;
    padding: 4px 12px;
    border-radius: 6px;
    font-family: inherit;
  }
  .commit-go:hover:not(:disabled) {
    background: var(--g4);
    border-color: var(--g6);
  }
  .commit-go:disabled {
    opacity: 0.55;
    cursor: default;
  }
  .commit-hint {
    font-size: 11px;
    color: var(--g5);
  }
  /* —— git 面板：范围选择/撤销/刷新 + 紧凑行 + 就地展开 diff —— */
  .rv-head {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    border-bottom: 1px solid var(--g2);
  }
  /* git 面板范围选择器：Select 组件按面板尺寸收窄（默认是设置弹窗里的整行宽）。
     button.csel 提一档特异性：稳压 Select 组件自身的 .csel.svelte-hash 规则 */
  .rv-src {
    flex: none;
    max-width: 100%;
  }
  .rv-src :global(.csel-wrap) {
    width: auto;
  }
  .rv-src :global(button.csel) {
    padding: 4px 8px;
    border: 1px solid var(--g3);
    border-radius: 6px;
    font-size: 12px;
  }
  .rv-src :global(button.csel:hover) {
    border-color: var(--g4);
  }
  .rv-src :global(.csel-menu) {
    min-width: 150px;
  }
  .rv-totals {
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11.5px;
    color: var(--g5);
    white-space: nowrap;
  }
  .rv-totals b {
    color: var(--g6);
  }
  .rv-tsep {
    flex: none;
    color: var(--g4);
    padding: 0 5px;
  }
  /* 增删计数（git 面板行与合计行共用） */
  .review-add {
    flex: none;
    color: var(--tint-green-fg);
    font-family: monospace;
    font-size: 11.5px;
  }
  .review-del {
    flex: none;
    color: var(--tint-red-fg);
    font-family: monospace;
    font-size: 11.5px;
  }
  /* 刷新：转圈图标，加载中不重复点 */
  .rv-btn {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 5px;
    background: transparent;
    border: none;
    color: var(--g6);
    font-size: 12px;
    font-family: inherit;
    padding: 4px 6px;
    border-radius: 6px;
    cursor: pointer;
  }
  .rv-btn:hover {
    background: var(--g2);
    color: var(--g7);
  }
  .rv-btn svg {
    width: 15px;
    height: 15px;
  }
  .rv-btn.spin svg {
    animation: rv-spin 0.9s linear infinite;
  }
  @keyframes rv-spin {
    to {
      transform: rotate(360deg);
    }
  }
  .rv-list {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 2px 0;
  }
  .rv-empty {
    margin: auto;
    text-align: center;
    color: var(--g5);
    font-size: 12.5px;
    line-height: 1.7;
    padding: 24px 16px;
  }
  .rv-empty svg {
    width: 30px;
    height: 30px;
    color: var(--g4);
    margin-bottom: 8px;
  }
  .rv-empty-t {
    font-size: 13.5px;
    font-weight: 600;
    color: var(--g6);
  }
  .rv-empty-d {
    font-size: 12px;
    margin-top: 3px;
  }
  /* 行：紧凑行——文件名粗体 + 目录暗色，右侧增删 + 展开箭头。
     未提交档行首多一个提交勾选框：行改 flex，勾选框不参与伸缩、文件按钮吃掉余量、
     diff 展开块整行换行（flex-basis:100%），收起/展开两种态都不破版 */
  .rv-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    border-bottom: 1px solid var(--g2);
  }
  .rv-row:last-child {
    border-bottom: none;
  }
  .rv-check {
    flex: none;
    margin: 0 2px 0 8px;
    cursor: pointer;
    accent-color: var(--g5);
  }
  .rv-file {
    flex: 1 1 auto;
    display: flex;
    align-items: center;
    gap: 7px;
    width: 100%;
    min-width: 0;
    height: 30px;
    padding: 0 10px;
    background: transparent;
    border: none;
    color: var(--g7);
    font-size: 12.5px;
    font-family: inherit;
    text-align: left;
    cursor: pointer;
  }
  .rv-file:hover {
    background: var(--g2);
  }
  .rv-row.open .rv-file {
    background: var(--g2);
  }
  .rv-file .rv-fico {
    flex: none;
    width: 14px;
    height: 14px;
    color: var(--g5);
  }
  .rv-file.added .rv-fico {
    color: var(--tint-green-fg);
  }
  .rv-file.deleted .rv-fico {
    color: var(--tint-red-fg);
  }
  .rv-fname {
    flex: none;
    font-weight: 650;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .rv-fdir {
    flex: 1 1 auto;
    min-width: 0;
    font-size: 11px;
    color: var(--g5);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .rv-stat {
    flex: none;
    display: flex;
    align-items: center;
    gap: 7px;
    font-family: monospace;
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
  }
  .rv-chev {
    flex: none;
    width: 13px;
    height: 13px;
    color: var(--g5);
    transition: transform 0.12s;
  }
  .rv-row.open .rv-chev {
    transform: rotate(180deg);
  }
  /* 就地展开的 diff：左右行号列 + 增删着色 */
  .rv-diffwrap {
    flex: 1 1 100%; /* 父行是 flex-wrap：展开的 diff 独占一行，不跟勾选框/文件名并排 */
    background: var(--g0);
    border-top: 1px solid var(--g2);
    max-height: 460px;
    overflow: auto;
  }
  .rv-diffnote {
    padding: 10px 12px;
    font-size: 12px;
    color: var(--g5);
  }
  .rv-diff {
    margin: 0;
    padding: 8px 0;
    font-family: Consolas, 'Courier New', monospace;
    font-size: 12px;
    line-height: 1.6;
    min-width: max-content;
  }
  .rv-dl {
    display: flex;
    align-items: stretch;
  }
  .rv-dno {
    flex: none;
    width: 38px;
    padding-right: 9px;
    text-align: right;
    color: var(--g4);
    user-select: none;
    font-variant-numeric: tabular-nums;
  }
  .rv-dtx {
    white-space: pre;
    padding-right: 14px;
    color: var(--g6);
  }
  .rv-dl-add {
    background: color-mix(in srgb, var(--tint-green-bg) 55%, transparent);
  }
  .rv-dl-add .rv-dtx {
    color: var(--tint-green-fg);
  }
  .rv-dl-add .rv-dno {
    color: color-mix(in srgb, var(--tint-green-fg) 55%, var(--g4));
  }
  .rv-dl-del {
    background: color-mix(in srgb, var(--tint-red-bg) 55%, transparent);
  }
  .rv-dl-del .rv-dtx {
    color: var(--tint-red-fg);
  }
  .rv-dl-del .rv-dno {
    color: color-mix(in srgb, var(--tint-red-fg) 55%, var(--g4));
  }
  .rv-dl-hunk {
    background: var(--g2);
  }
  .rv-dl-hunk .rv-dtx {
    color: var(--tint-blue-fg);
    padding-left: 4px;
  }
  .rv-dl-hunk .rv-dno {
    color: var(--g4);
  }
  .rv-dl-meta .rv-dtx {
    color: var(--g5);
    padding-left: 4px;
  }
</style>
