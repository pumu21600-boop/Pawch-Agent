<script>
  // 右侧面板「产物」标签（2026-11-14 阶段 A 第一片，自 App.svelte 逐字平移）。
  // 数据与动作留根：文件清单/选中态由根加载与持有，这里只渲染 + 上行事件。
  import { createEventDispatcher } from 'svelte';
  const dispatch = createEventDispatcher();

  export let loading = false;
  export let files = []; // [{ path }]
  export let note = ''; // 空态文案（读取失败/无产物原因）
  export let backfilled = false; // 旧会话回溯提示
  export let selPath = ''; // 选中行（选中态归根：右键菜单动作要拿它）

  // 渲染上限：账本里躺几千条时逐行 <button> 会拖死右栏。噪声路径（venv 等）已在
  // 后端账本侧过滤，这里兜底正当的大批量产物：只渲染最新 400 条，头上给一句总数。
  // files 按首次记录顺序（旧在前新在后），保尾 = 保最新。
  const RP_MAX_RENDER = 400;
  $: shown = files.length > RP_MAX_RENDER ? files.slice(-RP_MAX_RENDER) : files;
  $: overflow = files.length - shown.length;

  function rowContext(e, path) {
    // 拦下冒泡：根上的微调右键（onMicroContext）不能再抢这个事件
    e.preventDefault();
    e.stopPropagation();
    dispatch('select', path);
    dispatch('artmenu', { e, path });
  }
</script>

{#if loading}
  <div class="trace-empty">加载中…</div>
{:else if !files.length}
  <div class="rp-empty">{note || '本会话还没有产物。agent 改到过的工作区文件会列在这里。'}</div>
{:else}
  {#if backfilled}
    <div class="rp-note">旧会话：按检查点回溯，可能不全</div>
  {/if}
  {#if overflow > 0}
    <div class="rp-note">共 {files.length} 个产物，只显示最新 {shown.length} 个</div>
  {/if}
  {#each shown as f (f.path)}
    <!-- 单击只选中、双击才预览、右键出菜单（2026-11-12 用户：和项目文件树同款交互） -->
    <button
      class="rp-file"
      class:sel={f.path === selPath}
      title={f.path}
      on:click={() => dispatch('select', f.path)}
      on:dblclick={() => dispatch('preview', f.path)}
      on:contextmenu={(e) => rowContext(e, f.path)}
    >
      {f.path}
    </button>
  {/each}
{/if}

<style>
  /* 与 App.svelte 同名类保持同步：.trace-empty/.rp-empty 是 git/ext 等面板共用类，
     那边原件还在；.rp-note/.rp-file 家族为本面板特有，已从 App 迁入此处。 */
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
  /* .rp-note 源里有两层同名定义（黄底提示条 + 灰字覆盖），级联结果才是原貌——两份都搬 */
  .rp-note {
    flex: none;
    margin: 0 0 6px;
    padding: 6px 10px;
    border-radius: 6px;
    background: var(--tint-yellow-bg, #3a3417);
    color: var(--tint-yellow-fg, #e8d47f);
    font-size: 12px;
  }
  .rp-note {
    color: var(--g5);
    font-size: 11.5px;
    padding: 4px 10px 6px;
    border-bottom: 1px solid var(--g2);
    margin-bottom: 4px;
  }
  .rp-file {
    text-align: left;
    padding: 7px 10px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--g6);
    font-family: Consolas, monospace;
    font-size: 12.5px;
    cursor: pointer;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    /* 禁止 flex 收缩：.rp-pane 是 flex 纵列，行是 overflow:hidden 的子项——
       内容一多整列的行被均匀压成横切片（每行只露半截字），就是「产物太多就这样」
       的渲染侧根因（2026-11-14 修）。flex:none 让行保持自然高、撑出滚动。 */
    flex: none;
  }
  .rp-file:hover {
    background: var(--g2);
    color: var(--g7);
  }
  .rp-file.sel {
    /* 产物行选中：橙色高亮无底色（2026-11-12 统一口径；底色模式铺 --sel-bg） */
    background: var(--sel-bg);
    color: var(--sel-fg);
    font-weight: 600;
  }
</style>
