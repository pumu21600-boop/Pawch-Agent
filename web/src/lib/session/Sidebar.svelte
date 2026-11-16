<script>
  import { createEventDispatcher } from 'svelte';
  import { relShort } from '../format.js';

  // —— 会话侧栏（阶段 D 从 App.svelte 平移）——
  // 状态口径：会话/项目数据流与派生链（displayRows/flatSessions 等）留根
  // （UI_OPS open_session 读 flatSessions；expandedFlags 根上四处写：启动归一/
  // startNewSession/applyWorkspace/切工作区反应），这里只收渲染行的交互态：
  // 宽度拖拽与搜索框开合组件私有；三个菜单开合与管理态经 bind 双向（根上的
  // closeMenusOnOutside/enterManage/batchDelete 等要读要写）；动作一律事件上行，
  // 载荷在 e.detail（Svelte 组件事件直挂收 CustomEvent，父层必须取 detail）。
  const dispatch = createEventDispatcher();

  export let collapsed = false; // 根上的 sidebarCollapsed（reportUiState/UI_OPS/主区让位都要读）
  export let resizing = false; // 拖拽中标记（根 afterUpdate 拖拽期跳过滚动锚定，避免每帧 reflow）
  export let searchQuery = '';
  export let sidebarView = 'workspace'; // 按工作区 | 单列表
  export let expandedFlags = {}; // path → 展开？（根拥有，这里只读）
  export let showAllFlags = {}; // path → 展开全部会话（默认每项目先显 5 条）
  export let sideMenuOpen = false; // 工具条合并菜单（两个视图 + 会话管理）
  export let sessMenuId = null; // 打开了操作菜单的会话 id
  export let projMenuPath = null; // 打开了操作菜单的项目 path
  export let manageMode = false;
  export let checkedIds = [];
  export let displayRows = []; // 渲染行序列（根上派生：项目行/会话行/展开其余/空态）
  export let searchQ = '';
  export let manageTotal = 0;
  export let runningIds = new Set();
  export let currentSessionId = null;
  export let galaxy = true; // features.galaxy：记忆库入口开合

  let sidebarW = Number(localStorage.getItem('sidebarW')) || 280; // 初始宽度刚好放下品牌名（右缘拖拽可调，240–720）
  let searchOpen = false;

  function toggleSearch() {
    searchOpen = !searchOpen;
    if (!searchOpen) searchQuery = '';
  }

  // 侧栏右缘拖拽调宽：按住手柄移动鼠标，松手持久化（240–720）。
  // 指针事件 + setPointerCapture：捕获后所有 pointermove/pointerup/pointercancel
  // 都路由到手柄元素——光标挪到 iframe 上、窗口外、别的元素上都不会丢事件。
  // 早两版用 window 级 mousemove/mouseup 监听，真机（Tauri WebView2）实测两类
  // 症状：拖到某处像被卡住要来回蹭、松手后侧栏还跟着没按住的鼠标跑（mouseup 丢了）。
  // 宽度在手柄里**直接**应用：Chromium 对 mousemove/pointermove 按帧合并（规范动作），
  // 125Hz+ 鼠标也是一帧一次写入；不做 rAF 中转——rAF 在壳里可能被节流，中转一下
  // 反而表现成「侧栏冻住不跟手」（同一批真机反馈）。
  function startResize(e) {
    if (e.button !== 0) return; // 只认左键（触摸/笔的 button 也是 0，一并支持）
    e.preventDefault();
    resizing = true;
    const handle = e.currentTarget;
    try {
      handle.setPointerCapture(e.pointerId);
    } catch (err) {
      /* 捕获失败（极端环境）下退回 DOM 冒泡也能拖，不阻断 */
    }
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
    const move = (ev) => {
      sidebarW = Math.min(720, Math.max(240, Math.round(ev.clientX)));
    };
    const up = () => {
      resizing = false;
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      localStorage.setItem('sidebarW', String(sidebarW));
      handle.removeEventListener('pointermove', move);
      handle.removeEventListener('pointerup', up);
      handle.removeEventListener('pointercancel', up);
    };
    handle.addEventListener('pointermove', move);
    handle.addEventListener('pointerup', up);
    handle.addEventListener('pointercancel', up);
  }
</script>

<aside class="sidebar" class:collapsed={collapsed} class:resizing={resizing} style="width:{sidebarW}px">
  <div class="side-head">
    <div class="brand-row">
      <span class="brand-mark" aria-hidden="true"><img src="/pawch-icon.svg" alt="" /></span>
      <span class="brand-name">Pawch Agent</span>
      <button class="icon-btn" class:on={true} title="收起侧栏" on:click={() => dispatch('collapse')}>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><rect x="3" y="4.5" width="18" height="15" rx="3.2"/><line x1="9.8" y1="4.5" x2="9.8" y2="19.5"/></svg>
      </button>
      <!-- 重新加载界面（铁律 1 配套，2026-10-31）：前端换新构建后点一下丢旧代码。
           带时间戳强制拉最新入口（assets 带 hash，不受缓存影响）。 -->
      <button class="icon-btn" title="重新加载界面（前端更新后点这个）" on:click={() => (location.href = location.pathname + '?r=' + Date.now())}>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M20 11a8 8 0 1 0-2.3 6"/><path d="M20 5v6h-6"/></svg>
      </button>
    </div>

    <button class="new-chat" on:click={() => dispatch('newchat')}>
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><circle cx="12" cy="12" r="9"/><path d="M12 8v8M8 12h8" stroke-linecap="round"/></svg>
      新对话
    </button>

    <div class="section-row bar-menu">
      <span class="section-label">工作区</span>
      <span class="section-tools">
        <button class="icon-btn" class:on={searchOpen} title="搜索会话" on:click={toggleSearch}>
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><circle cx="11" cy="11" r="7"/><line x1="21" y1="21" x2="16.3" y2="16.3"/></svg>
        </button>
        <button class="icon-btn" class:on={sideMenuOpen || manageMode} title="视图与会话管理" on:click={() => (sideMenuOpen = !sideMenuOpen)}>
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><line x1="4" y1="7.5" x2="20" y2="7.5"/><circle cx="9.5" cy="7.5" r="2.2"/><line x1="4" y1="16.5" x2="20" y2="16.5"/><circle cx="14.5" cy="16.5" r="2.2"/></svg>
        </button>
        <button class="icon-btn" title="添加项目" on:click={() => dispatch('add-project')}>
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M20 11.2v5.8a2 2 0 0 1-2 2H5.5a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2h3.2l1.8 2H18a2 2 0 0 1 2 2v.2"/><line x1="17" y1="2.5" x2="17" y2="8"/><line x1="14.2" y1="5.2" x2="19.8" y2="5.2"/></svg>
        </button>
      </span>
      {#if sideMenuOpen}
        <div class="side-menu view-menu">
          <button on:click={() => dispatch('pick-view', 'workspace')}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"><path d="M20 19.5a1.8 1.8 0 0 0 1.8-1.8V8.3a1.8 1.8 0 0 0-1.8-1.8h-7.4a1.8 1.8 0 0 1-1.5-.8l-.8-1.2a1.8 1.8 0 0 0-1.5-.8H4a1.8 1.8 0 0 0-1.8 1.8v12.2A1.8 1.8 0 0 0 4 19.5Z"/></svg>
            <span>按工作区分组</span>
            {#if sidebarView === 'workspace'}<svg class="sel-check" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg>{/if}
          </button>
          <button on:click={() => dispatch('pick-view', 'list')}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><line x1="4" y1="6.5" x2="20" y2="6.5"/><line x1="4" y1="12" x2="20" y2="12"/><line x1="4" y1="17.5" x2="20" y2="17.5"/></svg>
            <span>单列表</span>
            {#if sidebarView === 'list'}<svg class="sel-check" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg>{/if}
          </button>
          <div class="menu-sep"></div>
          <button on:click={() => dispatch('enter-manage')}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="3.5" y="4" width="7" height="7" rx="1.5"/><path d="m5.2 7.6 1.4 1.4 2.4-2.6"/><rect x="13.5" y="4" width="7" height="7" rx="1.5"/><rect x="3.5" y="13.5" width="7" height="7" rx="1.5"/><rect x="13.5" y="13.5" width="7" height="7" rx="1.5"/></svg>
            <span>会话管理</span>
          </button>
        </div>
      {/if}
    </div>

    {#if searchOpen}
      <input class="side-search" bind:value={searchQuery} placeholder="搜索会话" />
    {/if}

    {#if manageMode}
      <div class="manage-bar">
        <span class="m-count">已选 {checkedIds.length} / {manageTotal}</span>
        <button class="m-btn" on:click={() => dispatch('select-all')}>全选</button>
        <button class="m-btn danger" disabled={!checkedIds.length} on:click={() => dispatch('batch-delete')}>删除所选</button>
        <button class="m-btn primary" on:click={() => dispatch('exit-manage')}>完成</button>
      </div>
    {/if}
  </div>

  <div class="side-scroll">
    {#each displayRows as row}
      {#if row.type === 'proj'}
        <div
          class="proj"
          class:expanded={!!searchQ || !!expandedFlags[row.path]}
          class:active={row.active}
          role="button"
          tabindex="0"
          title={row.path === '__ungrouped__' ? '无工作区记录的历史会话' : row.path === '__legacy__' ? '以前同名文件夹留下的旧会话（文件夹删掉重建后归到这里）' : `${row.path}（点行切换项目，点左侧图标展开/收起列表）`}
          on:click={() => ((row.path === '__ungrouped__' || row.path === '__legacy__' || row.active) ? dispatch('toggle-project', row.path) : dispatch('switch-project', row.path))}
          on:keydown={(e) => e.key === 'Enter' && ((row.path === '__ungrouped__' || row.path === '__legacy__' || row.active) ? dispatch('toggle-project', row.path) : dispatch('switch-project', row.path))}
        >
          <!-- 展开/收起：独立按钮，任何项目都能点开看会话（点行本身仍是切换项目） -->
          <button
            class="proj-toggle"
            title={!!searchQ || !!expandedFlags[row.path] ? '收起会话列表' : '展开会话列表'}
            on:click|stopPropagation={() => dispatch('toggle-project', row.path)}
          >
            {#if !!searchQ || !!expandedFlags[row.path]}
              <svg viewBox="0 0 24 24"><path d="M6.5 9.2 12 14.7l5.5-5.5z" fill="currentColor"/></svg>
            {:else}
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"><path d="M20 19.5a1.8 1.8 0 0 0 1.8-1.8V8.3a1.8 1.8 0 0 0-1.8-1.8h-7.4a1.8 1.8 0 0 1-1.5-.8l-.8-1.2a1.8 1.8 0 0 0-1.5-.8H4a1.8 1.8 0 0 0-1.8 1.8v12.2A1.8 1.8 0 0 0 4 19.5Z"/></svg>
            {/if}
          </button>
          <span class="proj-name">{row.name}{row.count ? '' : ''}</span>
          {#if row.running}<span class="conv-spin" title="这个项目里有会话正在跑" aria-label="有会话正在运行"></span>{/if}
          {#if row.active}<span class="proj-cur" title="当前项目"></span>{/if}
          {#if row.path === '__legacy__'}
            <span class="proj-side">
              <button class="row-btn" title="清空这些历史会话" on:click|stopPropagation={() => dispatch('clear-legacy')}>
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6.5h18"/><path d="M8 6.5V4.8A1.3 1.3 0 0 1 9.3 3.5h5.4A1.3 1.3 0 0 1 16 4.8v1.7"/><path d="M5.5 6.5 6.4 19a2 2 0 0 0 2 1.9h7.2a2 2 0 0 0 2-1.9l.9-12.5"/></svg>
              </button>
            </span>
          {:else if row.path !== '__ungrouped__'}
            <span class="proj-side">
              <button class="row-btn" class:on={projMenuPath === row.path} title="项目操作" on:click|stopPropagation={() => (projMenuPath = projMenuPath === row.path ? null : row.path)}>
                <svg viewBox="0 0 24 24" fill="currentColor"><circle cx="5.5" cy="12" r="1.5"/><circle cx="12" cy="12" r="1.5"/><circle cx="18.5" cy="12" r="1.5"/></svg>
              </button>
              <button class="row-btn" title="在项目内新建会话" on:click|stopPropagation={() => dispatch('new-session-in-project', { path: row.path, active: row.active })}>
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M12 5v14M5 12h14"/></svg>
              </button>
            </span>
          {/if}
          {#if projMenuPath === row.path}
            <div class="side-menu proj-menu">
              <button on:click|stopPropagation={() => { projMenuPath = null; dispatch('rename-project', { path: row.path, name: row.name }); }}>
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M16.8 3.7a2.1 2.1 0 0 1 3 3L7.3 19.2 3 20.3l1.1-4.3Z"/><path d="M13.5 20h7"/></svg>
                重命名项目
              </button>
              {#if row.active}
                <!-- 多文件夹项目（2026-11-04）：只有当前项目能改文件夹（端点按当前工作区生效） -->
                <button on:click|stopPropagation={() => { projMenuPath = null; dispatch('open-folders'); }}>
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"><path d="M3.5 7.2c0-1 .8-1.7 1.7-1.7h4l1.9 2.2h8.2c1 0 1.7.8 1.7 1.7v8.4c0 1-.8 1.7-1.7 1.7H5.2c-1 0-1.7-.8-1.7-1.7Z"/><path d="M3.5 10.5h17"/></svg>
                  项目文件夹
                </button>
              {/if}
              <button on:click|stopPropagation={() => dispatch('remove-project', { path: row.path, name: row.name })}>
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6.5h18"/><path d="M8 6.5V4.8A1.3 1.3 0 0 1 9.3 3.5h5.4A1.3 1.3 0 0 1 16 4.8v1.7"/><path d="M5.5 6.5 6.4 19a2 2 0 0 0 2 1.9h7.2a2 2 0 0 0 2-1.9l.9-12.5"/></svg>
                删除项目
              </button>
            </div>
          {/if}
        </div>
      {:else if row.type === 'conv'}
        {@const s = row.s}
        <div
          class="conv"
          class:active={s.id === currentSessionId}
          role="button"
          tabindex="0"
          on:click={() => (manageMode ? dispatch('toggle-check', s.id) : dispatch('open-session', s.id))}
          on:keydown={(e) => e.key === 'Enter' && (manageMode ? dispatch('toggle-check', s.id) : dispatch('open-session', s.id))}
        >
          {#if manageMode}
            <span class="conv-check" class:on={checkedIds.includes(s.id)}><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg></span>
          {/if}
          {#if runningIds.has(s.id)}
            <!-- 运行中：不管本页有没有这条会话的流（切走的/别的窗口起的）都转圈 -->
            <span class="conv-spin" title="正在运行" aria-label="正在运行"></span>
          {/if}
          <span class="conv-title" title={s.title}>{s.title}</span>
          {#if !manageMode && s.pinned}
            <!-- 置顶标记：标准图钉，放标题右侧（放最左会把标题整体挤开） -->
            <svg class="conv-pin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-label="已置顶"><path d="M12 17v5"/><path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z"/></svg>
          {/if}
          {#if !manageMode}
            <button class="conv-more" title="会话操作" on:click|stopPropagation={() => (sessMenuId = sessMenuId === s.id ? null : s.id)}>
              <svg viewBox="0 0 24 24" fill="currentColor"><circle cx="5.5" cy="12" r="1.6"/><circle cx="12" cy="12" r="1.6"/><circle cx="18.5" cy="12" r="1.6"/></svg>
            </button>
          {/if}
          <span class="conv-time">{relShort(s.updated_at)}</span>
          {#if sessMenuId === s.id}
            <div class="side-menu sess-menu">
              <button on:click|stopPropagation={() => dispatch('toggle-pin', s)}>
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 17v5"/><path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z"/></svg>
                {s.pinned ? '取消置顶' : '置顶'}
              </button>
              <button on:click|stopPropagation={() => { sessMenuId = null; dispatch('rename-session', s); }}>
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M16.8 3.7a2.1 2.1 0 0 1 3 3L7.3 19.2 3 20.3l1.1-4.3Z"/><path d="M13.5 20h7"/></svg>
                重命名
              </button>
              <button on:click|stopPropagation={() => dispatch('delete-session', s)}>
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18"/><path d="M8 6V4h8v2"/><path d="M19 6l-1 14H6L5 6"/><path d="M10 11v6"/><path d="M14 11v6"/></svg>
                删除会话
              </button>
            </div>
          {/if}
        </div>
      {:else if row.type === 'more'}
        <button class="expand-more" on:click={() => (showAllFlags = { ...showAllFlags, [row.path]: !row.all })}>
          {row.all ? '收起会话' : `展开其余 ${row.rest} 个会话`}
        </button>
      {:else}
        <div class="side-empty">{searchQ ? '没有匹配的会话' : '暂无会话，点上方「新对话」开始'}</div>
      {/if}
    {/each}
  </div>

  <div class="side-foot">
    {#if galaxy}
    <button class="foot-btn galaxy-btn" on:click={() => dispatch('open-galaxy')} title="记忆库：以星云看每一次会话，支持语义搜索">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="1.6" fill="currentColor" stroke="none"/><ellipse cx="12" cy="12" rx="9.2" ry="3.6" transform="rotate(-24 12 12)"/><circle cx="19.4" cy="8.6" r="1.1" fill="currentColor" stroke="none"/><circle cx="5.2" cy="15.2" r="0.9" fill="currentColor" stroke="none"/></svg>
      记忆库
    </button>
    {/if}
    <button class="foot-btn galaxy-btn" on:click={() => dispatch('open-settings')}>
      <svg class="settings-gear" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .34 1.88l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06A1.7 1.7 0 0 0 15 19.4a1.7 1.7 0 0 0-1 1.55V21a2 2 0 1 1-4 0v-.09A1.7 1.7 0 0 0 9 19.4a1.7 1.7 0 0 0-1.88.34l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.7 1.7 0 0 0 4.6 15a1.7 1.7 0 0 0-1.55-1H3a2 2 0 1 1 0-4h.09A1.7 1.7 0 0 0 4.6 9a1.7 1.7 0 0 0-.34-1.88l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.7 1.7 0 0 0 9 4.6h.09A1.7 1.7 0 0 0 10 3.05V3a2 2 0 1 1 4 0v.09a1.7 1.7 0 0 0 1 1.55 1.7 1.7 0 0 0 1.88-.34l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.7 1.7 0 0 0 19.4 9v.09a1.7 1.7 0 0 0 1.55 1H21a2 2 0 1 1 0 4h-.09a1.7 1.7 0 0 0-1.51 1.01z"/></svg>
      设置
    </button>
  </div>
  <div class="resize-handle" on:pointerdown={startResize}></div>
</aside>

<style>
  /* 壁纸模式下侧栏玻璃化：壁纸透出，文字仍有底（与 App 内其余面板同一批规则） */
  :global(body.has-wallpaper) .sidebar {
    background: color-mix(in srgb, var(--g0) 60%, transparent);
    border-right-color: color-mix(in srgb, var(--g4) 30%, transparent);
  }
  .sidebar {
    width: 280px;
    box-sizing: border-box; /*  修复：宽=sidebarW 语义对齐（渲染宽不再多出 padding+border 25px，拖拽手柄贴手指） */
    flex: none;
    /* 2026-11-11 全局透明+描边：面板玻璃化（壁纸透出，文字仍有底） */
    background: var(--glass-panel);
    border-right: 1px solid var(--g3);
    display: flex;
    flex-direction: column;
    padding: 14px 12px 10px;
    position: relative;
  }
  .sidebar.resizing {
    border-right-color: var(--g3);
  }
  .resize-handle {
    position: absolute;
    top: 0;
    bottom: 0;
    right: -3px;
    width: 6px;
    cursor: col-resize;
    z-index: 20;
    /* 指针拖拽：禁掉触摸的滚动/缩放手势，pointermove 才会连续到手柄 */
    touch-action: none;
  }
  .resize-handle:hover {
    background: var(--veil);
  }
  .sidebar.collapsed {
    display: none;
  }
  .side-head {
    flex: none;
  }
  .brand-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 2px 4px 14px;
  }
  .brand-mark {
    width: 30px;
    height: 30px;
    border-radius: 8px;
    /* 2026-11-01：图标换成无背景线稿，不再压阴影（透明底上阴影是一圈方框） */
    overflow: hidden;
    display: flex;
    align-items: center;
    justify-content: center;
    flex: none;
  }
  .brand-mark img {
    width: 100%;
    height: 100%;
    display: block;
  }
  .brand-name {
    flex: 1;
    font-size: 16px;
    font-weight: 650;
    color: var(--g7);
    letter-spacing: 0.2px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .icon-btn {
    /* 28px 是历史上侧栏图标钮的实际渲染尺寸：过去 App 里输入栏那份同名定义排在
       侧栏之后，宽/高/圆角一直由它说了算（钉死规则只盖了边框），用户看惯的就是它。
       拆出组件后按当时生效值定稿，不再复现跨段覆盖 */
    width: 28px;
    height: 28px;
    border-radius: 7px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    color: var(--g6);
    font-size: 16px;
    line-height: 1;
    cursor: pointer;
    flex: none;
  }
  .icon-btn:hover {
    /* 悬停只提亮文字不出底色（历史生效口径） */
    background: transparent;
    color: var(--g7);
  }
  .icon-btn.on {
    /* 开关型图标钮（侧栏收起钮/搜索/管理等）选中=橙色高亮无底色（2026-11-12 统一口径） */
    background: var(--sel-bg);
    color: var(--sel-fg);
  }
  .icon-btn:disabled {
    opacity: 0.4;
    cursor: default;
  }
  .icon-btn svg {
    width: 20px;
    height: 20px;
  }
  .new-chat {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    width: 100%;
    height: 40px;
    border-radius: 10px;
    border: 1.5px solid var(--g3);
    background: transparent;
    color: var(--g7);
    font-size: 13.5px;
    font-weight: 600;
    cursor: pointer;
    transition: background 0.15s ease, border-color 0.15s ease;
  }
  .new-chat:hover {
    background: transparent;
    border-color: var(--g5);
  }
  .new-chat:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .new-chat svg {
    width: 19px;
    height: 19px;
  }
  .section-row {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 14px 4px 6px;
  }
  .section-label {
    color: var(--g5);
    font-size: 14px;
    padding-left: 4px;
  }
  .section-tools {
    display: flex;
    gap: 3px;
  }
  .section-tools .icon-btn {
    width: 30px;
    height: 30px;
    border-radius: 8px;
    color: var(--g5);
  }
  .section-tools .icon-btn:hover {
    color: var(--g7);
  }
  .section-tools .icon-btn.on {
    /* 工作区行三钮（搜索/管理）：开=橙色高亮无底色（2026-11-12 用户拍板） */
    color: var(--sel-fg);
  }
  /* 侧栏这排钮（搜索/管理/添加项目）与顶部两钮（收起侧栏/重新加载）的排内口径：
     ① 常态/悬停一律不描边；② 选中态按全局口径铺 --sel-bg（高亮模式=透明，底色模式=浅底）；
     ③ 悬停不许刷掉选中底（历史上靠更高特异性压住同名 hover 的 transparent，留作保险） */
  .section-tools .icon-btn,
  .section-tools .icon-btn:hover,
  .side-head .icon-btn,
  .side-head .icon-btn:hover {
    border: none;
  }
  .section-tools .icon-btn.on,
  .section-tools .icon-btn.on:hover,
  .side-head .icon-btn.on,
  .side-head .icon-btn.on:hover {
    border: none;
    background: var(--sel-bg);
    color: var(--sel-fg);
  }
  .section-tools .icon-btn svg {
    width: 18px;
    height: 18px;
  }
  .side-search {
    margin: 2px 2px 8px;
    width: calc(100% - 4px);
    box-sizing: border-box;
    background: transparent;
    border: 1.5px solid var(--g3);
    border-radius: 10px;
    color: var(--g7);
    font-size: 14px;
    padding: 9px 12px;
    outline: none;
  }
  .side-search:focus {
    border-color: var(--g6);
  }
  .manage-bar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 2px 10px;
    flex-wrap: wrap;
  }
  .m-count {
    color: var(--g5);
    font-size: 13px;
    margin-right: auto;
    padding-left: 4px;
  }
  .m-btn {
    background: var(--g1);
    border: 1px solid var(--g2);
    color: var(--g6);
    font-size: 13px;
    border-radius: 8px;
    padding: 5px 10px;
    cursor: pointer;
  }
  .m-btn:hover:not(:disabled) {
    background: var(--g2);
  }
  .m-btn:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .m-btn.danger {
    color: var(--err, #e5697a);
    border-color: color-mix(in srgb, var(--err, #e5697a) 38%, transparent);
  }
  .m-btn.danger:hover:not(:disabled) {
    background: color-mix(in srgb, var(--err, #e5697a) 12%, transparent);
  }
  .m-btn:global(.primary) {
    background: transparent;
    border-color: var(--g3);
    color: var(--g7);
    font-weight: 620;
  }
  .side-scroll {
    flex: 1;
    overflow-y: auto;
    margin: 0 -4px;
    padding: 0 4px;
  }
  .proj {
    position: relative;
    display: flex;
    align-items: center;
    gap: 10px;
    height: 48px;
    padding: 0 10px;
    border-radius: 12px;
    cursor: pointer;
    color: var(--g7);
    font-size: 15.5px;
    /* 2026-11-12 用户拍板：项目/会话条目一律不加底色描边（hover/展开/选中都是），
       高亮只走文字（选中项橙色加粗，见 .proj.active/.conv.active） */
  }
  /* 展开/收起的独立热区（在项目行内部，点它不切项目） */
  .proj-toggle {
    width: 22px;
    height: 22px;
    margin: 0 -3px 0 -2px;
    padding: 0;
    border: none;
    background: transparent;
    color: var(--g6);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    flex: none;
    border-radius: 6px;
  }
  .proj-toggle:hover {
    /* 只提亮不铺底（2026-11-12 用户：收起展开瞬间条目/文件夹闪背景色）——
       钮在光标下随行重排忽有忽无，有底色就表现为闪烁 */
    color: var(--g7);
  }
  .proj-toggle svg {
    width: 19px;
    height: 19px;
  }
  .proj-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .proj.active {
    /* 选中口径统一（2026-11-12）：默认文字高亮；底色模式下铺 --sel-bg */
    background: var(--sel-bg);
  }
  .proj.active .proj-name {
    /* 项目名同会话条目一样走橙色高亮（2026-11-12 用户：所有选中态统一，不再「橙只留会话」） */
    color: var(--sel-fg);
    font-weight: 600;
  }
  /* 当前项目的小圆点标记 */
  .proj-cur {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--g5);
    flex: none;
    margin-left: 2px;
  }
  .proj-side {
    display: none;
    align-items: center;
    gap: 4px;
  }
  .proj:hover .proj-side,
  .proj.expanded .proj-side {
    display: flex;
  }
  .row-btn {
    width: 28px;
    height: 28px;
    border-radius: 8px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    color: var(--g6);
    cursor: pointer;
    flex: none;
  }
  .row-btn:hover {
    background: var(--g2);
    color: var(--g7);
  }
  .row-btn.on {
    /* 底色模式要跟着铺底（2026-11-12）；高亮模式 --sel-bg 是透明 */
    background: var(--sel-bg);
    color: var(--sel-fg);
  }
  .row-btn svg {
    width: 17px;
    height: 17px;
  }
  .conv {
    position: relative;
    display: flex;
    align-items: center;
    gap: 8px;
    height: 42px;
    padding: 0 10px 0 38px;
    border-radius: 10px;
    cursor: pointer;
    color: var(--g6);
    font-size: 14.5px;
    transition: background 0.15s ease, color 0.15s ease;
  }
  .conv.active {
    /* 选中口径统一：橙字加粗无底色；底色模式下铺 --sel-bg（同 .proj.active） */
    color: var(--sel-fg);
    background: var(--sel-bg);
    font-weight: 600;
  }
  /* 运行中：会话行/项目行前的小转圈（本页的流即时，服务端在跑的靠 3 秒轮询补） */
  .conv-spin {
    flex: none;
    width: 11px;
    height: 11px;
    border-radius: 50%;
    border: 1.5px solid var(--g4);
    border-top-color: var(--g7);
    animation: conv-spin 0.8s linear infinite;
  }
  @keyframes conv-spin {
    to {
      transform: rotate(360deg);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .conv-spin {
      animation-duration: 2s;
    }
  }
  .conv-pin {
    width: 15px;
    height: 15px;
    color: var(--g5);
    flex: none;
  }
  .conv-check {
    position: relative;
    width: 16px;
    height: 16px;
    border: 1.5px solid var(--g4);
    border-radius: 4.5px;
    flex: none;
  }
  .conv-check svg {
    position: absolute;
    inset: 1px;
    width: calc(100% - 2px);
    height: calc(100% - 2px);
    color: var(--g1);
    opacity: 0;
  }
  .conv-check.on {
    background: var(--g7);
    border-color: var(--g7);
  }
  .conv-check.on svg {
    opacity: 1;
  }
  .conv-title {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .conv-more {
    display: none;
    width: 26px;
    height: 26px;
    border-radius: 7px;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    color: var(--g6);
    cursor: pointer;
    flex: none;
  }
  .conv-more:hover {
    background: var(--g2);
    color: var(--g7);
  }
  .conv-more svg {
    width: 16px;
    height: 16px;
  }
  .conv:hover .conv-more {
    display: flex;
  }
  .conv:hover .conv-time {
    display: none;
  }
  .conv-time {
    font-size: 12.5px;
    color: var(--g5);
    flex: none;
  }
  .expand-more {
    display: block;
    width: 100%;
    text-align: left;
    background: none;
    border: none;
    color: var(--g5);
    font-size: 14px;
    padding: 10px 10px 10px 38px;
    cursor: pointer;
    border-radius: 10px;
  }
  .expand-more:hover {
    color: var(--g6);
    background: var(--veil);
  }
  .side-empty {
    color: var(--g4);
    font-size: 14px;
    padding: 18px 12px;
  }
  .side-menu {
    position: absolute;
    z-index: 40;
    min-width: 172px;
    background: var(--glass-float);
    border: 1px solid var(--g3);
    border-radius: 14px;
    padding: 6px;
    box-shadow: 0 12px 32px var(--overlay);
  }
  .sess-menu,
  .proj-menu {
    top: calc(100% + 4px);
    right: 6px;
  }
  .view-menu {
    top: calc(100% + 2px);
    right: 0;
  }
  .menu-sep {
    height: 1px;
    /* 4px 2px 是历史生效值（旧 App 里工作区切换那份同名定义排在后面盖掉了 5px 8px） */
    margin: 4px 2px;
    background: var(--g2);
  }
  .side-menu button {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    background: none;
    border: none;
    color: var(--g7);
    font-size: 14px;
    padding: 10px 12px;
    border-radius: 9px;
    cursor: pointer;
    text-align: left;
    white-space: nowrap;
  }
  .side-menu button:hover {
    /* 菜单底子已是 g2 系玻璃：hover 要深/浅一档（g3）才看得见 */
    background: var(--g3);
  }
  .side-menu button svg {
    width: 17px;
    height: 17px;
    color: var(--g6);
    flex: none;
  }
  .side-menu .sel-check {
    margin-left: auto;
    color: var(--g7);
  }
  .side-foot {
    flex: none;
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    padding-top: 8px;
  }
  .foot-btn {
    flex: 1;
    background: transparent;
    border: 1px solid var(--g2);
    color: var(--g5);
    font-size: 13px;
    border-radius: 10px;
    padding: 8px 0;
    cursor: pointer;
  }
  .foot-btn:hover {
    color: var(--g6);
    background: var(--g1);
  }
  .galaxy-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
  }
  .galaxy-btn svg {
    width: 15px;
    height: 15px;
  }
  /* 齿轮线框比记忆库的球体显大，缩半档对齐视觉大小 */
  .galaxy-btn .settings-gear {
    width: 13px;
    height: 13px;
  }
</style>
