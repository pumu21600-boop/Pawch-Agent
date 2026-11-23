<script>
  // 消息复制按钮（用户消息与 AI 回答共用）：点了就地转成勾/叉，1.2 秒回原样。
  // 图标按钮没有文字位，失败也必须看得见（错误不许静默）。
  import { onDestroy } from 'svelte';
  import { writeClipboard } from '../clipboard.js';
  export let text = '';
  export let label = '复制这条消息';
  let state = ''; // '' | 'ok' | 'fail'
  let timer = 0;
  async function run() {
    const ok = text ? await writeClipboard(text) : false;
    state = ok ? 'ok' : 'fail';
    clearTimeout(timer);
    timer = setTimeout(() => (state = ''), 1200);
  }
  onDestroy(() => clearTimeout(timer));
</script>

<button
  class="msg-act"
  title={state === 'ok' ? '已复制' : state === 'fail' ? '复制失败' : label}
  aria-label="复制"
  on:click|stopPropagation={run}
>
  {#if state === 'ok'}
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m5 12.5 4.5 4.5L19 7.5"/></svg>
  {:else if state === 'fail'}
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M6 6l12 12M18 6 6 18"/></svg>
  {:else}
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V6a2 2 0 0 1 2-2h9"/></svg>
  {/if}
</button>
