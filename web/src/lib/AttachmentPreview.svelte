<script>
  // 输入框附件预览（2026-11-10）：图片/视频/音频用浮层直接看；文档类应用内不解析，
  // 给「用系统程序打开」（App 侧调 /api/attachments/open 写临时文件并拉起默认程序）。
  import { createEventDispatcher } from 'svelte';
  import { fmtBytes } from './format.js';

  export let att = null; // { name, mime, data, size }
  export let err = '';
  const dispatch = createEventDispatcher();
  $: isImg = !!att && att.mime.startsWith('image/');
  $: isVid = !!att && att.mime.startsWith('video/');
  $: isAud = !!att && att.mime.startsWith('audio/');
</script>

{#if att}
  <!-- modal-backdrop = 壳 webview 让位清单成员（同 ImagePreview，2026-11-07 实锤） -->
  <div class="backdrop modal-backdrop" on:click={() => dispatch('close')}>
    <div class="att-modal" role="dialog" aria-label={att.name} on:click|stopPropagation>
      <div class="att-head">
        <span class="att-title" title={att.name}>{att.name}</span>
        <span class="att-meta">{fmtBytes(att.size)}</span>
        <span class="att-spacer"></span>
        {#if !isImg && !isVid && !isAud}
          <button class="btn ghost" on:click={() => dispatch('open-external', att)}>用系统程序打开</button>
        {/if}
        <button class="att-close" on:click={() => dispatch('close')} aria-label="关闭">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M6 6l12 12M18 6 6 18"/></svg>
        </button>
      </div>
      <div class="att-body">
        {#if isImg}
          <img src={att.data} alt={att.name} />
        {:else if isVid}
          <video src={att.data} controls autoplay playsinline></video>
        {:else if isAud}
          <audio src={att.data} controls autoplay></audio>
        {:else}
          <div class="att-doc">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><path d="M14 2v6h6"/></svg>
            <p>该类型在应用内不解析，用系统默认程序打开预览</p>
            <button class="btn primary" on:click={() => dispatch('open-external', att)}>打开</button>
          </div>
        {/if}
        {#if err}<div class="att-err">{err}</div>{/if}
      </div>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: var(--overlay);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 20;
    padding: 30px;
  }
  .att-modal {
    background: var(--g1);
    border: 1px solid var(--g4);
    border-radius: 10px;
    max-width: min(88vw, 1200px);
    max-height: 86vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    box-shadow: 0 10px 30px var(--overlay);
  }
  .att-head {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--g3);
    font-size: 12.5px;
    color: var(--g7);
  }
  .att-title {
    max-width: 46vw;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .att-meta {
    font-size: 11px;
    color: var(--g5);
  }
  .att-spacer { flex: 1; }
  .att-close {
    width: 24px;
    height: 24px;
    background: var(--g2);
    border: 1px solid var(--g4);
    color: var(--g7);
    border-radius: 50%;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }
  .att-close svg {
    width: 12px;
    height: 12px;
    display: block;
  }
  .att-body {
    padding: 12px;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 10px;
    min-height: 120px;
    max-height: 76vh;
    overflow: auto;
  }
  .att-body img {
    max-width: 82vw;
    max-height: 70vh;
    object-fit: contain;
    border-radius: 6px;
  }
  .att-body video {
    max-width: 82vw;
    max-height: 70vh;
    border-radius: 6px;
  }
  .att-body audio {
    width: min(560px, 70vw);
  }
  .att-doc {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    padding: 18px 26px;
    color: var(--g6);
    font-size: 12.5px;
    text-align: center;
  }
  .att-doc svg {
    width: 44px;
    height: 44px;
    color: var(--g5);
  }
  .att-err {
    color: var(--tint-red-fg, #e5484d);
    font-size: 12px;
  }
</style>
