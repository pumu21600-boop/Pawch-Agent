<script>
  // 输入栏上方「审批请求 + 阻塞式提问」dock（2026-11-15 阶段 B，自 App.svelte 逐字平移）。
  // 审批/提问的数据流与决定落点（decideApproval/answerAsk、审批档位、120s 超时）留根，
  // 这里只渲染 + 上行决定/选项/输入事件。approval-head/kicker/risk 是时间线审批卡共用类，
  // 双侧保留（根上原件还在）；riskLabelFor 走 lib/format.js。
  import { createEventDispatcher } from 'svelte';
  import { riskLabelFor } from '../format.js';
  const dispatch = createEventDispatcher();

  export let pendingApprovals = [];
  export let pendingQuestions = [];
  export let askUi = {};
</script>

        {#if pendingApprovals.length > 0}
          <!-- 执行询问贴在输入框上方，不钉在对话顶部；
               决定后记录回到时间线的工具卡 -->
          <div class="approval-dock">
            {#each pendingApprovals as pa}
              <div class="approval-box">
                <div class="approval-head">
                  <span class="approval-title"
                    ><span class="approval-kicker">审批请求</span><b>{pa.tool}</b>
                    <span class="approval-risk risk-{pa.risk}"
                      >{pa.riskLabel || riskLabelFor(pa.risk)}</span
                    ></span
                  >
                  {#if pa.reason}<span class="approval-reason">{pa.reason}</span>{/if}
                  <span class="tool-args" title={pa.args}>{pa.args}</span>
                </div>
                <div class="approval-actions">
                  <button class="btn" on:click={() => dispatch('decide', { id: pa.id, allow: true })}>允许</button>
                  <button class="btn ghost" on:click={() => dispatch('decide', { id: pa.id, allow: true, always: true })}
                    >同类不再问</button
                  >
                  <button class="btn ghost" on:click={() => dispatch('decide', { id: pa.id, allow: false })}>拒绝</button>
                  <span class="approval-hint">等你的决定，120 秒不选自动拒绝</span>
                </div>
              </div>
            {/each}
          </div>
        {/if}
        {#if pendingQuestions.length > 0}
          <!-- 阻塞式提问（ask 工具）：与审批同一个槽位、同一主题；提问是方案决策不是权限闸 -->
          <div class="approval-dock">
            {#each pendingQuestions as q (q.id)}
              {@const ui = askUi[q.id] || { picked: [], other: '' }}
              <div class="approval-box ask-box">
                <div class="approval-head">
                  <span class="approval-title"
                    ><span class="approval-kicker">{q.header || '提问'}</span
                    >{#if q.multiSelect}<span class="ask-multi-tag">可多选</span>{/if}</span
                  >
                  <b class="ask-question">{q.question}</b>
                </div>
                <div class="ask-options">
                  {#each q.options as o}
                    <button
                      class="ask-opt"
                      class:sel={ui.picked.includes(o.label)}
                      on:click={() => dispatch('pickopt', { q, label: o.label })}
                      title={o.description}
                    >
                      <span class="ask-opt-label">{o.label}</span>
                      {#if o.description}<span class="ask-opt-desc">{o.description}</span>{/if}
                    </button>
                  {/each}
                </div>
                <div class="ask-other">
                  <input
                    class="ask-other-in"
                    placeholder="其他（自由输入）"
                    value={ui.other}
                    on:input={(e) => dispatch('setui', { id: q.id, patch: { other: e.target.value } })}
                    on:keydown={(e) => { if (e.isComposing) return; if (e.key === 'Enter') dispatch('submitother', q); }}
                  />
                  {#if q.multiSelect}
                    <button class="btn" disabled={!ui.picked.length} on:click={() => dispatch('answer', { q, payload: { selected: [...ui.picked] } })}>确认所选</button>
                  {/if}
                  <button class="btn" disabled={!ui.other.trim()} on:click={() => dispatch('submitother', q)}>提交</button>
                  <button class="btn ghost" on:click={() => dispatch('answer', { q, payload: { dismissed: true } })}>跳过</button>
                </div>
                <span class="approval-hint">回答前这一轮会挂起等你；跳过 = 让它自己拿主意</span>
              </div>
            {/each}
          </div>
        {/if}

<style>
  /* —— 阻塞式提问卡（ask 工具，2026-11-03）：与审批同一个 dock 槽位、同一主题 —— */
  .ask-box .ask-question {
    width: 100%;
    margin-top: 2px;
    font-size: 13.5px;
    line-height: 1.45;
  }
  .ask-multi-tag {
    padding: 1px 6px;
    border-radius: 4px;
    background: var(--tint-purple-bg);
    color: var(--tint-purple-fg);
    font-size: 11px;
  }
  .ask-options {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: 6px;
    margin-top: 8px;
  }
  .ask-opt {
    display: flex;
    flex-direction: column;
    gap: 2px;
    text-align: left;
    padding: 7px 9px;
    border: 1px solid var(--g3);
    border-radius: 8px;
    background: transparent;
    color: var(--g6);
    cursor: pointer;
    font-size: 12.5px;
  }
  .ask-opt:hover {
    border-color: var(--g5);
  }
  .ask-opt.sel {
    /* 提问卡选中项：统一选中口径（橙字无底色描边；底色模式铺 --sel-bg） */
    border-color: transparent;
    background: var(--sel-bg);
    color: var(--sel-fg);
    font-weight: 650;
  }
  .ask-opt-label {
    font-weight: 600;
    color: var(--g7);
  }
  .ask-opt-desc {
    font-size: 11.5px;
    color: var(--g6);
    line-height: 1.4;
  }
  .ask-other {
    display: flex;
    gap: 6px;
    align-items: center;
    margin-top: 8px;
  }
  .ask-other-in {
    flex: 1;
    min-width: 0;
    padding: 6px 9px;
    border: 1px solid var(--g3);
    border-radius: 8px;
    background: var(--g2);
    color: var(--g7);
    font-size: 12.5px;
  }
  .approval-dock {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-bottom: 8px;
  }
  .approval-reason {
    display: inline-block;
    padding: 1px 6px;
    border-radius: 4px;
    background: var(--g2);
    color: var(--g6);
    font-size: 11px;
  }
  .approval-actions {
    display: flex;
    gap: 8px;
    margin-top: 8px;
    align-items: center;
  }
  .approval-hint {
    color: var(--g5);
    font-size: 11px;
    align-self: center;
  }
  /* 审批卡里仍要显示原始工具参数（工具调用行已改成人话说明） */
  .approval-box .tool-args {
    color: var(--g5);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 320px;
  }
  .approval-kicker {
    display: inline-block;
    font-size: 11px;
    font-weight: 600;
    color: var(--g6);
    border: 1px solid var(--g3);
    border-radius: 4px;
    padding: 1px 6px;
    margin-right: 6px;
    vertical-align: 1px;
  }
  /* 审批卡（2026-11-03 重塑：跟随柔和主题，不再红框警示——严重度由档位徽章自己说话） */
  .approval-box {
    background: color-mix(in srgb, var(--g1) 70%, transparent);
    border: 1px solid var(--g3);
    border-left: 2px solid var(--g5);
    border-radius: 10px;
    padding: 10px 12px;
    font-size: 13px;
  }
  .approval-head {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
    color: var(--g7);
  }
  /* A4：风险档位徽章（安全/常规改动/高危操作，与 RiskLevel::label 对齐） */
  .approval-risk {
    display: inline-block;
    padding: 1px 6px;
    border-radius: 4px;
    font-size: 11px;
    font-weight: 600;
  }
  .approval-risk.risk-0 {
    background: var(--tint-green-bg);
    color: var(--tint-green-fg);
  }
  .approval-risk.risk-1 {
    background: var(--tint-blue-bg);
    color: var(--tint-blue-fg);
  }
  .approval-risk.risk-2 {
    background: var(--tint-red-bg);
    color: var(--tint-red-fg);
  }
</style>



