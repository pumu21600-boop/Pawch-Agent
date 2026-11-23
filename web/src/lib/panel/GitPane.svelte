<script>
  // 右侧面板「git」标签（2026-11-15 阶段 A-3，自 App.svelte 逐字平移）。
  // 审查清单/提交盒/就地 diff；数据（rv* 簇、gitInfo、dockCp）与动作（loadReview/
  // doCommit/undoCheckpoint…）留根——SSE 钩子与提交流程都在根上——这里只渲染 + 上行。
  // 2026-11-23 内容改版：摘要/状态计数/两张汇总卡/逐行归属标签/时间线。
  import { createEventDispatcher } from 'svelte';
  import { gitStatusLabel, ownBadge } from '../format.js';
  const dispatch = createEventDispatcher();

  export let source = 'task'; // 'task' 本任务改动 | 'cum' 累计改动（bind 双向到根）
  export let files = []; // [{ path, status, adds, dels }]
  export let open = {}; // path → 是否展开 diff
  export let diff = {}; // path → { loading, text, error }
  export let baseline = ''; // source='task' 的检查点基线
  export let overview = null; // /api/git/overview：摘要/状态/两张卡/归属集合/时间线
  export let cumBase = ''; // 累计卡基线（展示用；后端算好的分叉点/HEAD）
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
  export let warn = ''; // 后端带回来的人话提醒（无上游退化对比/陈旧子树），有清单时以细行显示

  const GIT_INSTALL_HINT = '装一个 Git for Windows（https://git-scm.com/downloads）后重启本软件即可恢复';

  // 大目录折叠（2026-11-21）：一次安装（Android SDK/JDK…）会在工作区留下上万未跟踪
  // 文件，逐条铺出来源码根本找不到。超过阈值的「全是未跟踪文件」的顶层目录默认折成
  // 一行带计数，点开才逐条列；展开后也只渲染前 400 条，避免一次插几万个 DOM。
  const GROUP_MIN = 200;
  const GROUP_RENDER_MAX = 400;
  let expandedGroups = {};
  function rowKey(r) {
    return r.kind === 'file' ? 'f:' + r.f.path : r.kind + ':' + r.top;
  }
  $: groupInfo = (() => {
    const byTop = new Map();
    for (const f of files) {
      const i = f.path.indexOf('/');
      if (i <= 0) continue;
      const top = f.path.slice(0, i);
      const arr = byTop.get(top);
      if (arr) arr.push(f);
      else byTop.set(top, [f]);
    }
    const collapsed = new Set();
    for (const [top, arr] of byTop) {
      if (arr.length >= GROUP_MIN && arr.every((f) => f.status === '?')) collapsed.add(top);
    }
    return { byTop, collapsed };
  })();
  $: rows = (() => {
    const out = [];
    const seen = new Set();
    const shown = new Map();
    for (const f of files) {
      const i = f.path.indexOf('/');
      const top = i > 0 ? f.path.slice(0, i) : '';
      if (!top || !groupInfo.collapsed.has(top)) {
        out.push({ kind: 'file', f });
        continue;
      }
      if (!seen.has(top)) {
        seen.add(top);
        out.push({ kind: 'group', top, count: groupInfo.byTop.get(top).length, expanded: !!expandedGroups[top] });
      }
      if (!expandedGroups[top]) continue;
      const n = shown.get(top) || 0;
      if (n < GROUP_RENDER_MAX) {
        out.push({ kind: 'file', f });
        shown.set(top, n + 1);
      } else if (n === GROUP_RENDER_MAX) {
        out.push({ kind: 'more', top, count: groupInfo.byTop.get(top).length - GROUP_RENDER_MAX });
        shown.set(top, n + 1);
      }
    }
    return out;
  })();

  // 归属标签的路径集合与逐行判定（未提交 > stash > 回滚 > 已提交）。
  // 在 $: 里直接引用集合，overview 更新后行才会重算——把集合藏进函数体不行
  // （Svelte 4 的依赖分析看不到函数内部）。
  $: ownSets = {
    dirty: new Set((overview && overview.dirty) || []),
    stash: new Set((overview && overview.stash_paths) || []),
    rollback: new Set((overview && overview.rollback_paths) || []),
  };
  $: badgedRows = rows.map((r) => {
    if (r.kind !== 'file') return r;
    const badge = overview
      ? ownBadge(!!r.f.tracked, r.f.path, ownSets.dirty, ownSets.stash, ownSets.rollback)
      : '';
    return { ...r, badge };
  });
  $: ownCounts = (() => {
    const c = { committed: 0, uncommitted: 0, stash: 0, rollback: 0, untracked: 0 };
    for (const r of badgedRows) {
      if (r.kind !== 'file') continue;
      if (r.badge === '已提交') c.committed += 1;
      else if (r.badge === '未提交') c.uncommitted += 1;
      else if (r.badge === 'stash') c.stash += 1;
      else if (r.badge === '回滚') c.rollback += 1;
      else if (r.badge === '未跟踪') c.untracked += 1;
    }
    return c;
  })();
  $: uncommittedN = ((overview && overview.unstaged) || 0) + ((overview && overview.untracked) || 0);
  // 有未提交（含已暂存）改动才铺提交盒；overview 还没回来时按清单兜底
  $: canCommit = overview ? uncommittedN + (overview.staged || 0) > 0 : files.length > 0;
  function pickSource(v) {
    if (v === source) return;
    source = v;
    dispatch('rangechanged', v); // 带上目标档：根上不必等 bind 回写再取
  }

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
      <span class="flex-sp"></span>
      {#if dockCp && source === 'task' && baseline === dockCp.id}
        <button class="rv-btn" title="回滚到本任务起点（删除其后新建的已跟踪文件）" on:click={() => dispatch('undo')}>撤销</button>
      {/if}
      <button class="rv-btn" class:spin={loading} title="重新读取（总览 + 清单 + 分支状态）" on:click={() => dispatch('refresh')}>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M19 12a7 7 0 1 1-2.05-4.95" /><path d="M19 3.5V7h-3.5" /></svg>
      </button>
    </div>
    <!-- 任务摘要：任务起点检查点 / 分支 / 累计基线 -->
    <div class="git-summary">
      <div class="gs-row">
        <span class="gs-k">任务</span>
        <span class="gs-v gs-mono" title={overview?.task?.base || '还没有任务起点检查点'}>{overview?.task?.short || '—'}</span>
        <span class="gs-sep">·</span>
        <span class="gs-k">基线</span>
        {#if overview && overview.upstream}
          <span class="gs-v gs-mono" title={cumBase ? `${cumBase}…HEAD（上游分叉点）` : '上游分叉点…HEAD'}>{overview.cumulative?.short || '—'}…HEAD</span>
        {:else}
          <span class="gs-v" title="未设置上游分支：已跟踪文件按最后一次提交对比">无上游</span>
        {/if}
      </div>
      <div class="gs-row gs-counts">
        <span title="未暂存 + 未跟踪">未提交 <b>{uncommittedN}</b></span>
        <span title="已暂存（勾选提交即从这里走）">已暂存 <b>{overview?.staged || 0}</b></span>
        <span title="本地领先上游的提交数">已提交 <b>{overview?.ahead || 0}</b></span>
      </div>
    </div>
    <!-- 两张汇总卡：点哪张，下面的文件树就切到哪档 -->
    <div class="rv-cards">
      <button class="rv-card" class:active={source === 'task'} title="相对本次任务起点检查点" on:click={() => pickSource('task')}>
        <span class="rv-card-t">本任务改动</span>
        <span class="rv-card-n"><span class="review-add">+{overview?.task?.adds || 0}</span><span class="review-del">-{overview?.task?.dels || 0}</span></span>
        <span class="rv-card-f">{overview?.task?.files || 0} files</span>
      </button>
      <button class="rv-card" class:active={source === 'cum'} title="已跟踪文件对上游分叉点（无上游时为最后一次提交）；未提交文件对各自首次被记录的快照" on:click={() => pickSource('cum')}>
        <span class="rv-card-t">累计改动</span>
        <span class="rv-card-n"><span class="review-add">+{overview?.cumulative?.adds || 0}</span><span class="review-del">-{overview?.cumulative?.dels || 0}</span></span>
        <span class="rv-card-f">{overview?.cumulative?.files || 0} files</span>
      </button>
    </div>
    <div class="rv-meta">
      <div class="rv-facts">
        <span>冲突 <b>{overview?.conflicts || 0}</b></span>
        <span>rename <b>{overview?.renames || 0}</b></span>
        <span>binary <b>{overview?.binaries || 0}</b></span>
        <span>stash <b>{overview?.stash || 0}</b></span>
      </div>
      <div class="rv-facts rv-owns" title="逐行归属：未提交 > stash > 最近回滚 > 已提交 > 未跟踪">
        <span>归属</span>
        <span class="rv-own committed">已提交 {ownCounts.committed}</span>
        <span class="rv-own uncommitted">未提交 {ownCounts.uncommitted}</span>
        <span class="rv-own stashed">stash {ownCounts.stash}</span>
        <span class="rv-own rolled">回滚 {ownCounts.rollback}</span>
        <span class="rv-own untracked">未跟踪 {ownCounts.untracked}</span>
      </div>
    </div>
    {#if overview && overview.warning}
      <div class="git-missing-note">{overview.warning}</div>
    {/if}
  {/if}
  {#if warn && files.length}
    <!-- 后端人话提醒（如：陈旧子树）：细行显示，不弹不打扰 -->
    <div class="git-missing-note">{warn}</div>
  {/if}
  {#if canCommit && gitInfo && gitInfo.is_repo && !gitMissing}
    <!-- 提交盒：顶部紧凑一行（2026-11-22 用户选型）——输入框吃余量、按钮贴右，清单占满其余空间；
         勾选只出现在「未提交」行上；未跟踪文件默认不勾（按钮 tooltip 说明） -->
    <div class="commit-box">
      <textarea
        class="agents-editor commit-msg"
        rows="1"
        spellcheck="false"
        bind:value={commitMsg}
      ></textarea>
      <button
        class="commit-go"
        disabled={commitBusy}
        title="只做本地 commit，不推送；未跟踪文件默认不勾"
        on:click={() => dispatch('commit')}
      >
        {commitBusy ? '提交中…' : `提交 ${commitSelN} 个文件`}
      </button>
    </div>
    {#if commitErr}<div class="commit-err commit-err-row">{commitErr}</div>{/if}
  {/if}
  {#if loading && !files.length}
    <div class="trace-empty">加载中…</div>
  {:else if error && !files.length}
    <div class="rp-empty">{error}</div>
  {:else if !files.length}
    <div class="rv-empty">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9" /><path d="M8.5 12h7" /></svg>
      <div class="rv-empty-t">{source === 'cum' ? '相对基线没有改动' : '本任务还没有改动'}</div>
      <div class="rv-empty-d">{source === 'cum' ? '已跟踪文件对上游分叉点，未提交文件对各自首次被记录的快照。' : 'agent 修改/创建文件后，这里会列出改动清单（相对本次任务起点）。'}</div>
    </div>
  {:else}
    <div class="rv-list">
      {#each badgedRows as r (rowKey(r))}
        {#if r.kind === 'group'}
          <button
            class="rv-group"
            title={`${r.top}/：${r.count} 个未跟踪文件，点击${r.expanded ? '收起' : '展开'}`}
            on:click={() => (expandedGroups = { ...expandedGroups, [r.top]: !r.expanded })}
          >
            <svg class="rv-fico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M3.5 6.5h6l2 2h9v10h-17z" /></svg>
            <span class="rv-fname">{r.top}/</span>
            <span class="rv-gcount">{r.count} 个文件</span>
            <svg class="rv-chev" class:open={r.expanded} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6" /></svg>
          </button>
        {:else if r.kind === 'more'}
          <div class="rv-more">还有 {r.count} 个文件未显示（目录太大，已折叠）</div>
        {:else}
          {@const f = r.f}
          {@const name = f.path.split('/').pop()}
          {@const dir = f.path.split('/').slice(0, -1).join('/')}
          {@const dc = diff[f.path] || {}}
          <div class="rv-row" class:open={!!open[f.path]}>
            <button
              class="rv-file"
              class:added={f.status === 'A' || f.status === '?'}
              class:deleted={f.status === 'D'}
              class:nocheck={r.badge !== '未提交'}
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
              <!-- 未跟踪文件也有增删数（任务档后端按行数补了 adds）——用户：每个文件都要有 +x/-x 痕迹（2026-11-22）；没有数字（未提交档的未跟踪）才不显示 -->
              {#if f.status !== '?' || f.adds || f.dels}
                <span class="rv-stat"><span class="review-add">+{f.adds}</span><span class="review-del">-{f.dels}</span></span>
              {/if}
              {#if r.badge}
                <!-- 提交归属：固定宽度的一列，所有行对齐（2026-11-23 用户：标签要整齐） -->
                <span
                  class="rv-own"
                  class:committed={r.badge === '已提交'}
                  class:uncommitted={r.badge === '未提交'}
                  class:stashed={r.badge === 'stash'}
                  class:rolled={r.badge === '回滚'}
                  class:untracked={r.badge === '未跟踪'}
                  title="提交归属"
                >{r.badge}</span>
              {/if}
              <svg class="rv-chev" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6" /></svg>
            </button>
            {#if r.badge === '未提交'}
              <!-- 提交勾选框：正右缘、无底色描边（2026-11-22 用户：同模型选择里的方框） -->
              <input
                class="rv-check"
                type="checkbox"
                checked={!!commitSel[f.path]}
                title={`提交这个文件（${gitStatusLabel(f.status)}）`}
                on:change={() => dispatch('togglefile', f.path)}
              />
            {/if}
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
        {/if}
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
    padding: 9px 12px;
    font-size: 12.5px;
    line-height: 1.6;
    color: var(--tint-orange-fg);
    border-bottom: 1px solid var(--g2);
  }
  .commit-msg {
    width: 100%;
    box-sizing: border-box;
    resize: none;
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
    gap: 12px;
    padding: 11px 12px 9px;
  }
  /* 提交盒：未提交档顶部紧凑一行（2026-11-22 用户选型）——输入框吃余量、按钮贴右；
     勾选在下面清单行上；tooltip 说明「本地 commit/未跟踪默认不勾」。
     不铺底（2026-11-24 用户圈出「输入框旁边有背景环绕」）：只留一条分隔线。 */
  .commit-box {
    flex: none;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 9px 12px;
    border-bottom: 1px solid var(--g2);
  }
  .commit-msg {
    flex: 1;
    min-width: 0;
    min-height: 32px;
    max-height: 84px;
    font-size: 13px;
    line-height: 1.5;
    padding: 7px 11px;
  }
  .commit-go {
    flex: none;
    /* 选中口径双态（2026-11-22）：文字高亮=描边无底色橙字；背景高亮=实底无描边（与标签页/启动按钮同款） */
    background: var(--sel-bg);
    border: 1px solid var(--sel-line);
    color: var(--sel-fg);
    font-size: 12.5px;
    font-weight: 650;
    cursor: pointer;
    padding: 6px 14px;
    border-radius: 6px;
    font-family: inherit;
  }
  .commit-go:hover:not(:disabled) {
    background: transparent;
    border-color: var(--sel-fg);
  }
  :root[data-sel=bg] .commit-go,
  :root[data-sel=bg] .commit-go:hover:not(:disabled) {
    border-color: transparent;
  }
  :root[data-sel=bg] .commit-go:hover:not(:disabled) {
    background: var(--g4);
  }
  .commit-go:disabled {
    opacity: 0.55;
    cursor: default;
  }
  .commit-err-row {
    padding: 4px 12px 6px;
  }
  /* —— git 面板：摘要 / 两张卡 / 统计 + 紧凑行 + 就地展开 diff —— */
  .git-summary {
    flex: none;
    padding: 2px 12px 11px;
    display: flex;
    flex-direction: column;
    gap: 5px;
    border-bottom: 1px solid var(--g2);
  }
  .gs-row {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    font-size: 12px;
    line-height: 1.5;
    color: var(--g5);
    white-space: nowrap;
  }
  .gs-k {
    flex: none;
    color: var(--g4);
  }
  .gs-v {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--g6);
  }
  .gs-mono {
    font-family: monospace;
    font-size: 11px;
  }
  .gs-sep {
    flex: none;
    color: var(--g3);
  }
  .gs-counts {
    gap: 12px;
  }
  .gs-counts b {
    color: var(--g6);
    font-variant-numeric: tabular-nums;
  }
  /* 两张汇总卡：点哪张切哪档（选中口径双态：文字高亮=描边橙字 / 背景高亮=实底无描边） */
  .rv-cards {
    flex: none;
    display: flex;
    gap: 10px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--g2);
  }
  .rv-card {
    flex: 1 1 0;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
    align-items: flex-start;
    padding: 9px 11px;
    background: transparent;
    border: 1px solid var(--g3);
    border-radius: 8px;
    color: var(--g6);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .rv-card:hover {
    border-color: var(--g4);
  }
  .rv-card.active {
    border-color: var(--sel-line);
    color: var(--sel-fg);
  }
  :root[data-sel=bg] .rv-card.active {
    background: var(--sel-bg);
    border-color: transparent;
  }
  .rv-card-t {
    font-size: 12.5px;
    font-weight: 650;
    color: var(--g7);
  }
  .rv-card.active .rv-card-t {
    color: inherit;
  }
  .rv-card-n {
    display: flex;
    align-items: center;
    gap: 8px;
    font-variant-numeric: tabular-nums;
  }
  .rv-card-f {
    font-size: 11px;
    color: var(--g5);
  }
  /* 统计与归属：一个整块两行，行内留白放松 */
  .rv-meta {
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 7px;
    padding: 9px 12px;
    border-bottom: 1px solid var(--g2);
  }
  .rv-facts {
    display: flex;
    align-items: center;
    gap: 16px;
    font-size: 11.5px;
    color: var(--g5);
    white-space: nowrap;
    overflow-x: auto;
  }
  .rv-facts b {
    color: var(--g6);
    font-variant-numeric: tabular-nums;
  }
  .rv-owns {
    gap: 10px;
  }
  /* 归属标签：行内 badge 与顶部汇总共用一套颜色（未提交 > stash > 回滚 > 已提交） */
  .rv-own {
    flex: none;
    font-size: 10.5px;
    line-height: 1.6;
    padding: 0 6px;
    border-radius: 4px;
    border: 1px solid var(--g3);
    color: var(--g5);
    white-space: nowrap;
  }
  /* 行内标签固定宽度成一列：未提交 / 已提交 / stash / 回滚 / 未跟踪全部对齐 */
  .rv-file .rv-own {
    width: 50px;
    box-sizing: border-box;
    text-align: center;
    padding: 1px 0;
  }
  .rv-own.committed {
    color: var(--tint-blue-fg);
    border-color: color-mix(in srgb, var(--tint-blue-fg) 40%, transparent);
  }
  .rv-own.uncommitted {
    color: var(--tint-orange-fg);
    border-color: color-mix(in srgb, var(--tint-orange-fg) 40%, transparent);
  }
  .rv-own.stashed {
    color: var(--tint-yellow-fg);
    border-color: color-mix(in srgb, var(--tint-yellow-fg) 40%, transparent);
  }
  .rv-own.rolled {
    color: var(--g5);
  }
  .rv-own.untracked {
    color: var(--g5);
    border-style: dashed;
  }
  .rv-owns .rv-own {
    border: none;
    padding: 0;
  }
  /* 增删计数（git 面板行与卡片共用） */
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
    padding: 4px 0;
  }
  /* 超大未跟踪目录的折叠行（SDK/JDK 安装目录一次上万文件，逐条铺会把源码淹没） */
  .rv-group {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    min-width: 0;
    height: 36px;
    padding: 0 12px;
    background: transparent;
    border: none;
    border-bottom: 1px solid var(--g2);
    color: var(--g7);
    font: inherit;
    font-size: 12.5px;
    text-align: left;
    cursor: pointer;
  }
  .rv-group:hover {
    background: var(--g2);
  }
  .rv-group .rv-fico {
    flex: none;
    width: 14px;
    height: 14px;
    color: var(--g5);
  }
  .rv-gcount {
    flex: 1 1 auto;
    min-width: 0;
    font-size: 11px;
    color: var(--g5);
    font-family: monospace;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .rv-chev.open {
    transform: rotate(180deg);
  }
  .rv-more {
    padding: 7px 12px;
    font-size: 11px;
    color: var(--g5);
    border-bottom: 1px solid var(--g2);
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
    font-size: 12.5px;
    line-height: 1.8;
    margin-top: 4px;
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
  /* 提交勾选框：正右缘、无底色描边自绘勾（2026-11-22 用户：同模型选择里的方框，ModelPicker 同套） */
  .rv-check {
    appearance: none;
    width: 15px;
    height: 15px;
    flex: none;
    margin: 0 10px 0 0;
    display: inline-grid;
    place-items: center;
    border: 1.5px solid var(--g3);
    border-radius: 3px;
    background: transparent;
    cursor: pointer;
  }
  .rv-check:hover {
    border-color: var(--g5);
  }
  .rv-check:checked {
    border-color: var(--g6);
  }
  .rv-check:checked::before {
    content: '';
    width: 7px;
    height: 4px;
    border-left: 1.5px solid var(--g7);
    border-bottom: 1.5px solid var(--g7);
    transform: rotate(-45deg) translate(0, -1px);
  }
  .rv-file {
    flex: 1 1 0%; /* basis 0 + 可收缩：勾选框（正右缘）才不会被 width:100% 顶到换行 */
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    height: 36px;
    padding: 0 12px;
    background: transparent;
    border: none;
    color: var(--g7);
    font-size: 13px;
    font-family: inherit;
    text-align: left;
    cursor: pointer;
  }
  .rv-file.nocheck {
    /* 没勾选框的行右侧留出同宽空位：标签/箭头和带勾选框的行对齐（15px 勾选框 + 10px 间距） */
    padding-right: 37px;
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
    font-size: 11.5px;
    color: var(--g5);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .rv-stat {
    flex: none;
    margin-left: auto; /* 无目录后缀的行也要把数字统一推到最右（2026-11-22 用户：统一居右） */
    display: flex;
    align-items: center;
    gap: 8px;
    font-family: monospace;
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }
  .rv-chev {
    flex: none;
    width: 14px;
    height: 14px;
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
    padding: 12px;
    font-size: 12.5px;
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
