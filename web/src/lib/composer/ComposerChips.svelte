<script>
  // 输入栏上方「附件 chips + 引用对话 chips + 已引导行」（2026-11-15 阶段 B，自 App.svelte 逐字平移）。
  // 附件数据（pendingAttachments/pendingConvs）与预览/移除落点留根，这里只渲染 + 上行事件。
  // .att-ico 是聊天区消息附件共用类，双侧保留（根上原件还在）；fmtBytes 走 lib/format.js。
  import { createEventDispatcher } from 'svelte';
  import { fmtBytes } from '../format.js';
  const dispatch = createEventDispatcher();

  export let pendingAttachments = [];
  export let pendingConvs = [];
  export let guidedList = []; // 当前会话的已引导行（根上按 guided[curKey()] 传）
</script>

                  {#if pendingAttachments.length > 0}
<div class="composer-attachments">
            {#each pendingAttachments as att, idx}
              <div
                class="att-chip"
                title={`${att.name} (${fmtBytes(att.size)}) · 点击预览`}
                role="button"
                tabindex="0"
                on:click={() => dispatch('preview', att)}
                on:keydown={(e) => {
                  if (e.key === 'Enter' || e.key === ' ') {
                    e.preventDefault();
                    dispatch('preview', att);
                  }
                }}
              >
                {#if att.mime.startsWith('image/')}
                  <img class="att-thumb" src={att.data} alt={att.name} />
                {:else if att.mime.startsWith('video/')}
                  <svg class="att-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><rect x="3" y="5.5" width="18" height="13" rx="2.5"/><path d="M3 9.5h18M7.5 5.5v4M16.5 5.5v4"/></svg>
                {:else if att.mime.startsWith('audio/')}
                  <svg class="att-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><circle cx="7.5" cy="17.5" r="2.5"/><circle cx="17.5" cy="15.5" r="2.5"/><path d="M10 17.5V6l10-2.5V15.5"/></svg>
                {:else}
                  <svg class="att-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="m8 12.5 6.5-6.5a2.8 2.8 0 0 1 4 4L9 19.5a4.5 4.5 0 0 1-6.4-6.4l8.5-8.5"/></svg>
                {/if}
                <span class="att-name">{att.name}</span>
                <span class="att-size">{fmtBytes(att.size)}</span>
                <button class="att-del" on:click|stopPropagation={() => dispatch('rmatt', idx)} title="移除" aria-label="移除"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M6 6l12 12M18 6 6 18"/></svg></button>
              </div>
            {/each}
          </div>
        {/if}
        {#if pendingConvs.length > 0}
          <div class="composer-attachments">
            {#each pendingConvs as cv}
              <div class="att-chip conv-chip" title={`引用对话：${cv.title}（发送时拼进消息正文${cv.omitted ? '，只保留最近部分' : ''}）`}>
                <svg class="att-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M21 11.5a8.4 8.4 0 0 1-9 8.4 8.9 8.9 0 0 1-3.8-.8L3 21l1.9-4.6a8.4 8.4 0 1 1 16.1-4.9"/><path d="M8.5 10.5h7M8.5 14h4.5"/></svg>
                <span class="att-name">{cv.title}</span>
                <button class="att-del" on:click={() => dispatch('rmconv', cv.id)} title="移除" aria-label="移除"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M6 6l12 12M18 6 6 18"/></svg></button>
              </div>
            {/each}
          </div>
        {/if}
        {#if guidedList.length > 0}
          <!-- 已引导标记（2026-10-22）：静默提示,不占对话气泡位;本轮结束自动消失 -->
          <div class="guided-row">
            {#each guidedList as g, gi}
              <span class="guided-chip" title={`不打断当前操作,跑完这步就并入:${g.text}`}>
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M22 2 11 13"/><path d="M22 2 15 22l-4-9-9-4Z"/></svg>
                已引导 · {g.text.length > 40 ? g.text.slice(0, 40) + '…' : g.text}
                <button
                  class="guided-x"
                  title="移除这条提示"
                  aria-label="移除这条提示"
                  on:click={() => dispatch('rmguided', gi)}
                ><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"><path d="M6 6l12 12M18 6 6 18"/></svg></button>
              </span>
            {/each}
          </div>
        {/if}

<style>
  .att-del svg {
    width: 13px;
    height: 13px;
    display: block;
  }
  /* 「已引导」静默提示（输入区上方，本轮结束消失） */
  .guided-row {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 2px 2px 6px;
  }
  .guided-chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    max-width: 100%;
    padding: 3px 10px;
    border-radius: 10px;
    font-size: 12px;
    color: var(--accent, #4a9);
    background: color-mix(in srgb, var(--accent, #4a9) 10%, transparent);
  }
  .guided-chip > svg {
    width: 12px;
    height: 12px;
    flex: none;
  }
  .guided-x {
    display: grid;
    place-items: center;
    width: 16px;
    height: 16px;
    padding: 0;
    border: 0;
    border-radius: 4px;
    background: transparent;
    color: inherit;
    opacity: 0.55;
    cursor: pointer;
    flex: none;
  }
  .guided-x:hover {
    opacity: 1;
    background: color-mix(in srgb, var(--accent, #4a9) 16%, transparent);
  }
  .guided-x svg {
    width: 10px;
    height: 10px;
  }
  /* 待发送附件条 */
  .composer-attachments {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    padding: 8px 12px 4px;
    border-bottom: 1px solid var(--g2);
  }
  .att-chip {
    display: flex;
    align-items: center;
    gap: 6px;
    background: var(--g2);
    border: 1px solid var(--g3);
    border-radius: 6px;
    padding: 3px 8px;
    font-size: 12px;
    color: var(--tint-blue-fg);
    max-width: 240px;
    cursor: pointer; /* 点击打开预览（2026-11-10） */
  }
  .att-chip:hover {
    border-color: var(--g5);
  }
  .att-thumb {
    width: 22px;
    height: 22px;
    object-fit: cover;
    border-radius: 4px;
  }
  .att-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 120px;
  }
  .att-size {
    font-size: 10.5px;
    color: var(--g5);
  }
  .att-del {
    background: transparent;
    border: none;
    color: var(--g6);
    cursor: pointer;
    font-size: 11px;
    padding: 0 2px;
    line-height: 1;
  }
  .att-del:hover {
    color: var(--tint-red-fg);
  }
  .conv-chip .att-name {
    max-width: 200px;
  }
  .att-ico {
    width: 15px;
    height: 15px;
    flex: none;
    color: var(--g6);
  }
</style>




