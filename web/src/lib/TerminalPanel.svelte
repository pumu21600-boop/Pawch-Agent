<script>
  // 终端抽屉（从 App.svelte 拆出的第一个分区，D6）：纯展示 + 输入，状态与请求都留在宿主。
  // 行数组每次变化（新增一行 / 命令输出回填）都贴底——原来是在 runTerm 里手动滚两次。
  import { createEventDispatcher, tick } from 'svelte';

  export let open = false;
  export let lines = [];
  export let running = false;
  export let height = 280;
  export let resizing = false;
  export let value = '';

  const dispatch = createEventDispatcher();
  let outEl;
  let lastLines = null;

  $: if (open && lines !== lastLines) {
    lastLines = lines;
    tick().then(() => {
      if (outEl) outEl.scrollTop = outEl.scrollHeight;
    });
  }

  function submit() {
    const cmd = value.trim();
    if (cmd && !running) dispatch('run', cmd);
  }

  function onKey(e) {
    if (e.isComposing || e.keyCode === 229) return;
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      submit();
    }
  }
</script>

{#if open}
  <div class="term-panel" class:resizing style="height:{height}px">
    <div class="term-resize" title="拖拽调整高度" on:pointerdown={(e) => dispatch('resizestart', e)}></div>
    <div class="term-head">
      <span class="term-title">终端</span>
      <span class="term-hint">工作区内 · 沙箱执行</span>
      <span class="term-sp"></span>
      <button class="term-clear" on:click={() => dispatch('clear')}>清空</button>
      <button class="term-close" title="关闭终端" on:click={() => dispatch('close')}>×</button>
    </div>
    <div class="term-out" bind:this={outEl}>
      {#if !lines.length}
        <div class="term-line dim">输入命令回车执行（如 dir、git status）。命令在你的工作区内、受限沙箱中运行。</div>
      {/if}
      {#each lines as l}
        <div class="term-block" class:err={!l.ok}>
          <div class="term-line"><span class="term-prompt">❯</span>{l.cmd}</div>
          {#if l.out}<pre class="term-text">{l.out}</pre>{/if}
        </div>
      {/each}
      {#if running}
        <div class="term-line dim">执行中…</div>
      {/if}
    </div>
    <div class="term-in">
      <span class="term-prompt">❯</span>
      <input bind:value placeholder="输入命令，Enter 运行" disabled={running} spellcheck="false" on:keydown={onKey} />
    </div>
  </div>
{/if}

<style>
  .term-panel {
    flex: none;
    position: relative;
    display: flex;
    flex-direction: column;
    background: #141210;
    color: #d9d5cd;
    border-top: 1px solid var(--g3);
    font-family: Consolas, 'Cascadia Mono', monospace;
    font-size: 12.5px;
  }
  :global(body.has-wallpaper) .term-panel {
    background: color-mix(in srgb, #141210 82%, transparent);
  }
  .term-resize {
    position: absolute;
    top: -3px;
    left: 0;
    right: 0;
    height: 7px;
    cursor: row-resize;
    z-index: 6;
    touch-action: none;
  }
  .term-panel.resizing {
    border-top-color: var(--g5);
  }
  .term-head {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 12px;
    background: rgba(255, 255, 255, 0.04);
    border-bottom: 1px solid rgba(255, 255, 255, 0.08);
    color: #8a8478;
  }
  .term-title {
    font-weight: 650;
    color: #d9d5cd;
  }
  .term-hint {
    font-size: 11px;
    color: #6e695f;
  }
  .term-sp {
    flex: 1;
  }
  .term-clear,
  .term-close {
    background: transparent;
    border: none;
    color: #8a8478;
    cursor: pointer;
    font-size: 12px;
    padding: 2px 6px;
    border-radius: 5px;
  }
  .term-clear:hover,
  .term-close:hover {
    background: rgba(255, 255, 255, 0.08);
    color: #f3f1ec;
  }
  .term-close {
    font-size: 16px;
    line-height: 1;
  }
  .term-out {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 8px 12px;
  }
  .term-line {
    white-space: pre-wrap;
    word-break: break-all;
    color: #d9d5cd;
  }
  .term-line.dim {
    color: #6e695f;
  }
  .term-block.err .term-text {
    color: #ff9b9b;
  }
  .term-prompt {
    color: #8fe3ae;
    margin-right: 8px;
  }
  .term-text {
    margin: 2px 0 6px;
    padding-left: 18px;
    white-space: pre-wrap;
    word-break: break-all;
    color: #b8b3a9;
    font-family: inherit;
  }
  .term-in {
    display: flex;
    align-items: center;
    padding: 6px 12px;
    border-top: 1px solid rgba(255, 255, 255, 0.08);
  }
  .term-in input {
    flex: 1;
    background: transparent;
    border: none;
    outline: none;
    color: #d9d5cd;
    font-family: inherit;
    font-size: 12.5px;
  }
</style>
