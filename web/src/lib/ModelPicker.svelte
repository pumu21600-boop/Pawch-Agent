<script>
  // 获取到的模型清单弹窗（自 App.svelte 按视图拆出）：勾选状态由宿主的已存模型表决定。
  // 能力徽章（图片/视频/音频）随「按模型名智能推导」机制一并退役——
  // 聚合渠道上名字与真实能力对不上，预测比没有更糟；能力以设置里手动勾选为准。
  export let open = false;
  export let models = [];
  export let saved = []; // 已加入配置的模型名（checked 判定）
  import { createEventDispatcher } from 'svelte';
  const dispatch = createEventDispatcher();
  // checked 必须内联 saved.includes(m)：套函数读闭包变量不进 each 的脏值依赖，
  // saved 变了（全选/全不选）勾选框不重渲染，按钮就像失效
  // 搜索过滤（2026-11-10）：聚合渠道动辄上百个模型，按名字过滤
  let filter = '';
  $: needle = filter.trim().toLowerCase();
  $: shown = needle ? models.filter((m) => String(m).toLowerCase().includes(needle)) : models;
</script>

{#if open}
  <div class="modal-backdrop" on:click={() => dispatch('close')}>
    <div class="modal" on:click|stopPropagation>
      <div class="modal-head">
        <b>获取到的模型（{models.length} 个，勾选即加入）</b>
        <div class="modal-actions">
          <button class="btn" on:click={() => dispatch('selectall')}>全选</button>
          <button class="btn" on:click={() => dispatch('selectnone')}>全不选</button>
          <button class="btn ghost" on:click={() => dispatch('close')}>完成</button>
        </div>
      </div>
      <div class="model-filter">
        <span class="mf-label">搜索</span>
        <input class="mf-input" type="text" bind:value={filter} />
        {#if needle}<span class="mf-count">{shown.length}/{models.length}</span>{/if}
      </div>
      <div class="model-list">
        {#if shown.length === 0}
          <div class="empty">{models.length ? '没有匹配的模型' : '渠道未返回任何模型'}</div>
        {/if}
        {#each shown as m}
          <label class="model-item">
            <input type="checkbox" checked={saved.includes(m)} on:change={() => dispatch('toggle', m)} />
            <span class="m-name">{m}</span>
          </label>
        {/each}
      </div>
    </div>
  </div>
{/if}

<style>
  /* 与设置弹窗同一套「描边无底色」（2026-11-11）：全选/全不选/完成与搜索框
     透明底+灰描边，hover 只加深描边；搜索框原先的橙底橙描边一并去掉 */
  .modal .btn {
    background: transparent;
    border: 1.5px solid var(--g3);
    color: var(--g7);
    filter: none;
  }
  .modal .btn:hover {
    background: transparent;
    border-color: var(--g5);
  }
  .model-filter {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0 0 8px;
  }
  .mf-label {
    font-size: 12px;
    color: var(--g5);
    flex: none;
  }
  .mf-input {
    flex: 1;
    min-width: 0;
    height: 26px;
    padding: 0 9px;
    border: 1.5px solid var(--g3);
    border-radius: 6px;
    background: transparent;
    color: var(--g7);
    font-size: 12.5px;
    font-family: inherit;
  }
  .mf-input:focus {
    outline: none;
    border-color: var(--g6);
  }
  .mf-count {
    flex: none;
    font-size: 11.5px;
    color: var(--g5);
    font-variant-numeric: tabular-nums;
  }
  .model-list {
    overflow-y: auto;
    border: 1px solid var(--g2);
    border-radius: 8px;
    padding: 6px;
    max-height: 50vh;
  }
  .model-item {
    display: flex;
    gap: 8px;
    align-items: center;
    padding: 4px 6px;
    font-family: Consolas, monospace;
    font-size: 13px;
    cursor: pointer;
  }
  /* 勾选框同套描边无底色（2026-11-11）：自绘勾选标记，选中只加深描边不填色 */
  .model-item input[type='checkbox'] {
    appearance: none;
    width: 14px;
    height: 14px;
    flex: none;
    margin: 0;
    display: inline-grid;
    place-items: center;
    border: 1.5px solid var(--g3);
    border-radius: 3px;
    background: transparent;
    cursor: pointer;
  }
  .model-item input[type='checkbox']:hover {
    border-color: var(--g5);
  }
  .model-item input[type='checkbox']:checked {
    border-color: var(--g6);
  }
  .model-item input[type='checkbox']:checked::before {
    content: '';
    width: 7px;
    height: 4px;
    border-left: 1.5px solid var(--g7);
    border-bottom: 1.5px solid var(--g7);
    transform: rotate(-45deg) translate(0, -1px);
  }
  .m-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
