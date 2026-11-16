<script>
  // 右侧面板「网页」标签（2026-11-15 阶段 A-4，自 App.svelte 逐字平移）。
  // 壳桥状态簇与同步机制（webTabs/webMode/webNative/视口矩形上报/弹窗让位/心跳）
  // 全部留根——它们与根上的 SSE 钩子、agent 自动唤起、面板拖宽、全局浮层观察器
  // 交织，见 App.svelte 2370 起的簇。这里只渲染 + 上行动作事件；
  // address 与 bodyEl/bodyW/bodyH 走 bind 双向（根上的视口同步要量这块矩形）。
  // 页签溢出滑杆是纯 DOM 几何，随面板走（root 的 afterUpdate 那条同步一并迁入；
  // rpWidth 作 prop 是为了拖宽时也重算滑杆）。
  import { createEventDispatcher, afterUpdate } from 'svelte';
  const dispatch = createEventDispatcher();

  export let tabs = []; // [{ id, agent, title, url, loading, suspended, visible }]
  export let activeId = '';
  export let active = null; // 根上派生的当前页签对象
  export let address = ''; // bind 双向到根
  export let busy = false;
  export let error = '';
  export let dialog = null; // 页面 JS 弹窗 { kind, message }
  export let native = false; // webNative：壳模式 + 本客户端是壳
  export let modalLive = false; // shellModalLive：弹窗让位态（面板改看帧流）
  export let showShot = false; // webShowShot：非空白页（渲染截图/帧流）
  export let shotT = 0; // 帧缓存穿透参数（根上递增）
  export let menuOpen = false; // 「⋯」菜单（仅无头模式的 HTML 下拉）
  export let rpWidth = 420; // 面板宽度：拖宽时触发本组件 afterUpdate 重算滑杆
  export let bodyEl = null; // bind 双向到根（web-body 元素，根上量矩形用）
  export let bodyW = 0; // bind 双向到根
  export let bodyH = 0; // bind 双向到根

  // 网页页签滑杆（「和右侧边栏那一条一样」）：同 rp-strip 模式，
  // 压在 .web-tabbar 下边框线上，页签多到溢出时出现，可点可拖。
  let webStripEl = null;
  let webStripBar = null; // { track, w, x } px；null = 没溢出
  function webStripSync() {
    const el = webStripEl;
    if (!el || el.scrollWidth <= el.clientWidth + 1) {
      if (webStripBar) webStripBar = null;
      return;
    }
    const track = el.clientWidth;
    const w = Math.max(24, Math.round((track * track) / el.scrollWidth));
    const x = Math.round(((track - w) * el.scrollLeft) / (el.scrollWidth - track));
    webStripBar = { track, w, x };
  }
  let webStripDrag = null; // { x0, s0 }
  function webStripDown(e) {
    const el = webStripEl;
    if (!el || !webStripBar) return;
    const r = el.getBoundingClientRect();
    const ratio = Math.min(1, Math.max(0, (e.clientX - r.left - webStripBar.w / 2) / (webStripBar.track - webStripBar.w)));
    el.scrollLeft = ratio * (el.scrollWidth - webStripBar.track);
    webStripDrag = { x0: e.clientX, s0: el.scrollLeft };
    e.currentTarget.setPointerCapture(e.pointerId);
  }
  function webStripMove(e) {
    const el = webStripEl;
    if (!el || !webStripDrag || !webStripBar) return;
    const span = Math.max(1, webStripBar.track - webStripBar.w);
    el.scrollLeft = webStripDrag.s0 + ((e.clientX - webStripDrag.x0) * (el.scrollWidth - el.clientWidth)) / span;
  }
  function webStripUp() {
    webStripDrag = null;
  }
  afterUpdate(() => { void rpWidth; webStripSync(); }); // 增删/拖宽重算滑杆（自 App.svelte 的根级 afterUpdate 迁入；读 rpWidth 消 unused-export 警告）
</script>

<div class="web-wrap">
  {#if tabs.length}
    <div class="web-tabbar">
      <div class="web-tabs" bind:this={webStripEl} on:scroll={webStripSync}>
        {#each tabs as t (t.id)}
          <button
            class="web-tab"
            class:active={t.id === activeId}
            title={t.url || 'about:blank'}
            on:click={() => dispatch('select', t)}
          >
            <span class="web-tab-title">{t.title || (t.url === 'about:blank' ? '空白页' : t.url) || '空白页'}</span>
            {#if t.agent}<span class="web-tab-agent" title="agent 浏览器工具的工作页">agent</span>{/if}
            <span
              class="web-tab-x"
              title="关闭标签页"
              role="button"
              tabindex="-1"
              on:click|stopPropagation={() => dispatch('closetab', t.id)}
            >×</span>
          </button>
        {/each}
      </div>
      {#if webStripBar}
        <!-- 溢出滑杆：压在 .web-tabbar 下边框线上，同右侧标签页那条 -->
        <i
          class="web-strip-track"
          style="width:{webStripBar.track}px"
          on:pointerdown={webStripDown}
          on:pointermove={webStripMove}
          on:pointerup={webStripUp}
          on:pointercancel={webStripUp}
        >
          <i class="web-strip-thumb" style="width:{webStripBar.w}px; transform:translateX({webStripBar.x}px)"></i>
        </i>
      {/if}
    </div>
  {/if}
  <div class="web-toolbar">
    <button class="web-nav" title="后退" disabled={!active || busy} on:click={() => dispatch('action', 'back')}>
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="m14.5 6-6 6 6 6"/></svg>
    </button>
    <button class="web-nav" title="前进" disabled={!active || busy} on:click={() => dispatch('action', 'forward')}>
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="m9.5 6 6 6-6 6"/></svg>
    </button>
    <button class="web-nav" title="刷新" disabled={!active || busy} on:click={() => dispatch('action', 'reload')}>
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M19 12a7 7 0 1 1-2.05-4.95"/><path d="M19 3.5V7h-3.5"/></svg>
    </button>
    <input
      class="web-address"
      placeholder="输入网址后回车"
      bind:value={address}
      disabled={busy}
      spellcheck="false"
      on:keydown={(e) => {
        if (e.isComposing || e.keyCode === 229) return;
        if (e.key === 'Enter') { e.preventDefault(); dispatch('go'); }
      }}
    />
    {#if native && active && active.loading}
      <span class="web-native-loading" title="页面加载中">●</span>
    {/if}
    <button class="web-add" title="新建标签页" on:click={() => dispatch('newtab')}>+</button>
    <!-- ⋯ 按钮元素上行（根上的 webToggleMenu 要按它的矩形给壳定位浮层） -->
    <button class="web-nav" title="更多" on:click={(e) => dispatch('menutoggle', e.currentTarget)}>
      <svg viewBox="0 0 24 24" fill="currentColor"><circle cx="5" cy="12" r="1.8"/><circle cx="12" cy="12" r="1.8"/><circle cx="19" cy="12" r="1.8"/></svg>
    </button>
    {#if menuOpen && !native}
      <!-- svelte-ignore a11y-click-events-have-key-events a11y-no-static-element-interactions -->
      <div class="web-menu-backdrop" on:click={() => dispatch('menuclose')}></div>
      <div class="web-menu">
        <button on:click={() => dispatch('menuaction', 'open-external')}>
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M14 4h6v6"/><path d="M20 4 11 13"/><path d="M19 14v5a2 2 0 0 1-2 2H6a2 2 0 0 1 2-2h5"/></svg>
          在默认浏览器中打开
        </button>
        <!-- 无头模式没有「开发者工具」项：面板里是截图流，没有可挂真 DevTools 的
             实体（服务端 /api/browser/devtools 对无头模式返回 opened:false 并引导
             「在默认浏览器中打开后按 F12」）。壳模式的「⋯」菜单是壳里浮层
             （web/public/menu.html），「开发者工具」项在那边——child webview
             才有真 DevTools 可开。 -->
      </div>
    {/if}
  </div>
  {#if error}
    <div class="web-error">{error}</div>
  {/if}
  {#if dialog}
    <!-- 页面 JS 弹窗（alert/confirm/prompt）：无头页面的原生弹窗在这处理，不卡死页面 -->
    <!-- svelte-ignore a11y-click-events-have-key-events a11y-no-static-element-interactions -->
    <div class="web-dialog" on:click|stopPropagation>
      <span class="web-dialog-msg">页面弹窗〔{dialog.kind}〕{dialog.message || '（无内容）'}</span>
      <span class="flex-sp"></span>
      <button class="btn ghost" on:click={() => dispatch('dialog', true)}>确定</button>
      <button class="btn ghost" on:click={() => dispatch('dialog', false)}>取消</button>
    </div>
  {/if}
  <div
    class="web-body"
    class:has-shot={showShot && (!native || modalLive)}
    bind:this={bodyEl}
    bind:clientWidth={bodyW}
    bind:clientHeight={bodyH}
  >
    {#if !showShot}
      <div class="web-empty">
        <svg class="web-globe" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"><circle cx="12" cy="12" r="8.5"/><path d="M3.5 12h17M12 3.5c2.6 2.3 3.9 5.1 3.9 8.5s-1.3 6.2-3.9 8.5c-2.6-2.3-3.9-5.1-3.9-8.5s1.3-6.2 3.9-8.5Z"/></svg>
        <div class="web-empty-title">网页</div>
        <div class="web-empty-sub">粘贴或输入 URL 以打开网页。</div>
      </div>
    {:else if native && !modalLive}
      <!-- 壳窗口里的 child webview 就浮在这块区域上（原生 HWND 盖在 Svelte 之上），
           这里只留占位——点击/滚动/输入全部走真实网页，不再坐标回传。
           弹窗打开时 webview 移出屏幕保活、走下面的帧流分支（画面实时垫在弹窗下） -->
      <div class="web-native" aria-hidden="true"></div>
    {:else}
      {#if busy || (active && active.loading)}<div class="web-loading">加载中…</div>{/if}
      <!-- svelte-ignore a11y-click-events-have-key-events a11y-no-static-element-interactions -->
      <img
        class="web-shot"
        class:busy={busy}
        src={`/api/browser/frame?id=${encodeURIComponent(active.id)}&t=${shotT}&native=0`}
        alt="页面画面"
        draggable="false"
        on:click={(ev) => dispatch('shotclick', { el: ev.currentTarget, x: ev.clientX, y: ev.clientY })}
        on:wheel={(ev) => { ev.preventDefault(); dispatch('shotwheel', { deltaY: ev.deltaY }); }}
      />
    {/if}
  </div>
</div>

<style>
  /* 网页面板（内置浏览器：截图渲染 + 坐标回传，与 agent 工具共享实例） */
  .web-wrap {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .web-error {
    color: var(--tint-red-fg, #ff6b6b);
    font-size: 12px;
    padding: 4px 2px 0;
  }
  .web-dialog {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 4px 0;
    padding: 6px 8px;
    border: 1px solid var(--g3);
    border-radius: 6px;
    background: var(--g1);
    font-size: 12px;
  }
  .web-dialog-msg {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 60%;
  }
  .web-tabbar {
    position: relative; /* 溢出滑杆的锚点（压在下边框线上） */
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 6px 8px;
    border-bottom: 1px solid var(--g3);
  }
  .web-tabs {
    display: flex;
    align-items: center;
    gap: 4px;
    overflow-x: auto;
    flex: 1;
    min-width: 0;
    scrollbar-width: none; /* 原生滚动条藏掉，溢出用自绘滑杆（同 rp-tabstrip） */
  }
  .web-tabs::-webkit-scrollbar {
    display: none;
  }
  .web-strip-track {
    position: absolute;
    left: 8px;
    bottom: -3px;
    height: 7px;
    z-index: 2;
    cursor: pointer;
  }
  .web-strip-thumb {
    display: block;
    height: 2px;
    margin-top: 2.5px;
    border-radius: 1px;
    background: var(--g4);
  }
  .web-strip-track:hover .web-strip-thumb {
    background: var(--g6);
  }
  .web-tab {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    max-width: 150px;
    padding: 3px 6px;
    border: 1px solid var(--g3);
    border-radius: 7px;
    background: var(--g1);
    color: var(--g6);
    font-size: 12px;
    cursor: pointer;
    white-space: nowrap;
    flex: none;
  }
  .web-tab.active {
    /* 网页页签激活：统一选中口径（橙字无底色；底色模式铺 --sel-bg） */
    background: var(--sel-bg);
    color: var(--sel-fg);
  }
  .web-tab-title {
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 96px;
  }
  .web-tab-agent {
    font-size: 10px;
    color: var(--tint-blue-fg, #6aa9ff);
    border: 1px solid currentColor;
    border-radius: 4px;
    padding: 0 3px;
    flex: none;
  }
  .web-tab-x {
    opacity: 0.55;
    padding: 0 2px;
  }
  .web-tab-x:hover {
    opacity: 1;
  }
  .web-add {
    flex: none;
    width: 22px;
    height: 22px;
    border: 1px solid var(--g3);
    border-radius: 7px;
    background: var(--g1);
    color: var(--g6);
    font-size: 15px;
    line-height: 1;
    cursor: pointer;
  }
  .web-add:hover {
    color: var(--g7);
  }
  .web-menu-backdrop {
    position: fixed;
    inset: 0;
    z-index: 30;
  }
  .web-menu {
    position: absolute;
    top: calc(100% + 2px);
    right: 8px;
    z-index: 31;
    background: var(--glass-float);
    border: 1px solid var(--g3);
    border-radius: 9px;
    padding: 4px;
    min-width: 180px;
    box-shadow: 0 8px 28px rgba(0, 0, 0, 0.35);
  }
  /* 「⋯」菜单（仅无头模式：HTML 下拉盖在截图上，z-index 正常）。
     shell 模式不用这个浮层——菜单是壳里一个透明置顶的 webview（见 webToggleMenu：
     浮在网页之上、网页不动），因为 child webview 是 HWND，HTML 永远盖不过它。 */
  .web-menu button {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 7px 10px;
    border: none;
    background: transparent;
    color: var(--g7);
    font-size: 12.5px;
    border-radius: 6px;
    cursor: pointer;
    text-align: left;
  }
  .web-menu button:hover {
    background: var(--g3);
  }
  .web-menu svg {
    width: 14px;
    height: 14px;
    flex: none;
    color: var(--g6);
  }
  .web-toolbar {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 6px 8px;
    border-bottom: 1px solid var(--g3);
    position: relative;
  }
  .web-nav {
    flex: none;
    width: 26px;
    height: 26px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border: none;
    background: transparent;
    color: var(--g6);
    border-radius: 7px;
    cursor: pointer;
  }
  .web-nav:hover:not(:disabled) {
    background: var(--g2);
    color: var(--g7);
  }
  .web-nav:disabled {
    opacity: 0.4;
    cursor: default;
  }
  .web-nav svg {
    width: 15px;
    height: 15px;
  }
  .web-address {
    flex: 1;
    min-width: 0;
    padding: 5px 10px;
    border: 1px solid var(--g3);
    border-radius: 8px;
    background: var(--g1);
    color: var(--g7);
    font-size: 12.5px;
    outline: none;
  }
  .web-address:focus {
    border-color: var(--g5);
  }
  .web-body {
    flex: 1;
    min-height: 0;
    position: relative;
    overflow: auto;
    background: var(--g1);
    display: flex;
    align-items: flex-start;
    justify-content: center;
  }
  /* 帧流模式不能让滚动条参与布局：图高 = 宽 × 帧高宽比，1px 溢出就会长出滚动条，
     宽度随即少 15px、页面整体缩一圈（2026-11-06 实测「打开弹窗网页往里缩一点」）。 */
  .web-body.has-shot {
    overflow: hidden;
  }
  .web-empty {
    margin: auto;
    text-align: center;
    color: var(--g5);
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 40px 10px;
  }
  .web-globe {
    width: 52px;
    height: 52px;
    opacity: 0.5;
  }
  .web-empty-title {
    font-size: 14px;
    color: var(--g6);
  }
  .web-empty-sub {
    font-size: 12px;
  }
  .web-shot {
    width: 100%;
    height: auto;
    display: block;
    cursor: pointer;
    user-select: none;
  }
  /* 壳里 child webview 的占位区（原生 HWND 盖在这块上面，这里只是撑出尺寸） */
  .web-native {
    position: absolute;
    inset: 0;
    cursor: default;
  }
  .web-native-loading {
    color: var(--g-accent, #6cf);
    font-size: 9px;
    line-height: 1;
    animation: web-pulse 1s ease-in-out infinite;
  }
  @keyframes web-pulse {
    0%, 100% { opacity: 0.25; }
    50% { opacity: 1; }
  }
  .web-shot.busy {
    opacity: 0.6;
  }
  .web-loading {
    position: absolute;
    top: 8px;
    right: 10px;
    font-size: 11px;
    color: var(--g6);
    background: var(--g2);
    border: 1px solid var(--g3);
    border-radius: 6px;
    padding: 2px 8px;
    z-index: 2;
  }
</style>
