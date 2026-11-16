<script>
  // 输入栏上方「任务清单 dock」（2026-11-15 阶段 B，自 App.svelte 逐字平移）。
  // 清单数据与手动态（todoDock* 簇/轮计数/指纹）留根——SSE 钩子与跨轮重置逻辑都在根上，
  // 这里只渲染 + 上行开合/收起事件。
  import { createEventDispatcher } from 'svelte';
  const dispatch = createEventDispatcher();

  export let todoDockVisible = false;
  export let todoDockOpen = false;
  export let currentTodo = null;
  export let todoCur = null;
  export let todoDoneN = 0;
</script>

{#if todoDockVisible}
          <!-- 任务清单 dock：贴输入框上方、展开向上长；
               与审批/提问上下堆叠——清单在最上，阻塞交互的提问/审批贴输入框 -->
          <div class="todo-dock">
            <div class="todo-dock-row">
              <button
                class="todo-head"
                on:click={() => dispatch('toggle')}
                title={todoDockOpen ? '收起任务清单' : '展开任务清单'}
              >
                <svg class="todo-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M9 6h11M9 12h11M9 18h11"/><path d="m3.5 5.5 1.5 1.5 3-3"/><path d="m3.5 11.5 1.5 1.5 3-3"/><circle cx="5" cy="18" r="1.1" fill="currentColor" stroke="none"/></svg>
                <span class="todo-title">任务清单</span>
                {#if !todoDockOpen}<span class="todo-cur" title={todoCur.content}>{todoCur.content}</span>{/if}
                <span class="todo-count">{todoDoneN}/{currentTodo.todos.length}</span>
                <svg class="todo-caret" class:open={todoDockOpen} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6"/></svg>
              </button>
              <button class="todo-x" on:click={() => dispatch('dismiss')} title="收起任务清单（出现新的任务清单前不再冒出）" aria-label="收起任务清单">×</button>
            </div>
            {#if todoDockOpen}
              <div class="todo-body">
                {#each currentTodo.todos as t, ti}
                  <div class="todo-item {t.status}">
                    <span class="todo-mark">
                      {#if t.status === 'completed'}<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="m8.5 12.5 2.5 2.5 4.5-5"/></svg>
                      {:else if t.status === 'in_progress'}<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12h13"/><path d="m13 7 5 5-5 5"/></svg>
                      {:else}<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><circle cx="12" cy="12" r="9"/></svg>{/if}
                    </span>
                    <span class="todo-text">{ti + 1}. {t.content}</span>
                  </div>
                {/each}
              </div>
            {/if}
          </div>
{/if}

<style>
  /* —— 任务清单 dock（ZCode TodoWrite 同款语义，2026-11-03 贴输入框上方）——
     清单在最上、展开向上长；审批/提问在它下面贴输入框（阻塞交互优先）。
     展开体限高内部滚动，避免把对话区挤没或盖住提问。 */
  .todo-dock {
    margin: 0 0 8px;
    padding-bottom: 2px;
    border-bottom: 1px solid var(--g3);
    font-size: 13px;
    min-width: 0;
  }
  .todo-dock-row {
    display: flex;
    align-items: center;
    gap: 2px;
    min-width: 0;
  }
  .todo-head {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    min-width: 0;
    flex: 1;
    padding: 4px 2px;
    background: none;
    border: none;
    color: var(--g6);
    cursor: pointer;
    font-size: 12.5px;
  }
  .todo-x {
    width: 22px;
    height: 22px;
    flex: none;
    padding: 0;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--g6);
    font-size: 16px;
    line-height: 1;
    cursor: pointer;
  }
  .todo-x:hover {
    color: var(--g7);
    background: var(--g3);
  }
  .todo-dock .todo-body {
    max-height: min(32vh, 260px);
    overflow-y: auto;
    padding: 0 2px 6px;
  }
  .todo-head:hover {
    color: var(--g7);
  }
  .todo-ico {
    width: 15px;
    height: 15px;
    color: var(--g6);
  }
  .todo-title {
    font-weight: 600;
    color: var(--g7);
  }
  .todo-cur {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    text-align: left;
  }
  .todo-count {
    font-family: ui-monospace, monospace;
    font-size: 11.5px;
    color: var(--g6);
  }
  .todo-caret {
    width: 14px;
    height: 14px;
    transition: transform 0.15s;
  }
  .todo-caret.open {
    transform: rotate(180deg);
  }
  .todo-body {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 2px 12px 10px 12px;
  }
  .todo-item {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    line-height: 1.5;
  }
  .todo-mark {
    width: 15px;
    height: 15px;
    flex: none;
    margin-top: 2px;
    color: var(--g6);
  }
  .todo-mark svg {
    width: 15px;
    height: 15px;
  }
  .todo-item.completed .todo-mark {
    color: var(--tint-green-fg);
  }
  .todo-item.in_progress .todo-mark {
    color: var(--g6);
    font-weight: 650;
  }
  .todo-text {
    min-width: 0;
  }
  .todo-item.pending .todo-text {
    color: var(--g6);
  }
  .todo-item.in_progress .todo-text {
    color: var(--g7);
    font-weight: 600;
  }
  .todo-item.completed .todo-text {
    color: var(--g6);
    text-decoration: line-through;
    opacity: 0.75;
  }
</style>



