<script>
  import { tick } from 'svelte';
  import Select from '../Select.svelte';

  export let tab = '';
  // 模型配置分区的状态本体在 App 侧（模型快选菜单/上下文圆环/发送路径都读同一份），
  // 这里只做设置分区的界面半边：输入框全部 bind 回根，保存动作调根的 autoSave
  export let providers = [];
  export let configs = [];
  export let activeConfigId = '';
  export let configLabel = () => '';
  export let manageConfig = () => {};
  export let newConfig = () => {};
  export let deleteConfig = () => {};
  export let providerId = '';
  export let onProviderChange = () => {};
  /// 「API 格式」三选（openai/anthropic/responses）与「自动补后缀」勾选
  export let protocol = 'openai';
  export let autoSuffix = true;
  export let baseUrl = '';
  export let apiKey = '';
  export let typingHint = '';
  export let clearKey = () => {};
  export let configName = '';
  export let model = '';
  export let savedModels = [];
  /// 当前配置的「每模型参数覆盖」（键=模型 id）
  export let modelParams = {};
  export let fetchModels = () => {};
  export let capabilities = { tools: true, vision: false, video: false, audio: false };
  export let autoSave = () => {};
  export let confirm = async () => true;

  let openModel = ''; // 展开参数面板的模型
  let addOpen = false; // 「+ 添加模型」展开的输入行
  let addText = ''; // 手动添加模型 id
  let addErr = '';
  let addInput = null;
  let rowTest = {}; // 模型 id → { state: 'testing'|'ok'|'fail', text }
  let rowTestTimers = {}; // 结果 2 秒后自动收掉（避免测试记录常驻）

  const CAPS = [
    ['tools', '工具调用'],
    ['vision', '图片解析'],
    ['video', '视频解析'],
    ['audio', '音频解析'],
  ];
  // 每模型可覆盖的四项数值参数
  const NUM_FIELDS = [
    ['context_length', '最大上下文长度'],
    ['max_output_tokens', '最大输出长度'],
    ['temperature', '温度'],
    ['top_p', 'top_p'],
  ];

  /// 某模型的覆盖值（没填 = 空，运行时会回落配置级/渠道默认）
  function eff(m, key) {
    const p = (modelParams || {})[m] || {};
    return p[key] !== undefined && p[key] !== null ? p[key] : null;
  }
  function hasOverride(m, key) {
    return eff(m, key) !== null;
  }
  function effCaps(m) {
    const p = (modelParams || {})[m] || {};
    return p.capabilities || capabilities;
  }

  /// 改某个模型的一项覆盖：null/空 = 删除该项（回落配置级）；
  /// 整条记录空了就把该模型从 model_params 里删掉
  function patchModel(m, patch) {
    const next = { ...(modelParams || {}) };
    const cur = { ...(next[m] || {}) };
    for (const [k, v] of Object.entries(patch)) {
      if (v === null || v === undefined || v === '') delete cur[k];
      else cur[k] = v;
    }
    if (Object.keys(cur).length === 0) delete next[m];
    else next[m] = cur;
    modelParams = next;
    autoSave({ model_params: next }, 0);
  }
  function setCap(m, key, val) {
    patchModel(m, { capabilities: { ...effCaps(m), [key]: val } });
  }
  function setNum(m, key, raw) {
    const s = String(raw == null ? '' : raw).trim();
    const n = Number(s);
    patchModel(m, { [key]: s === '' || !Number.isFinite(n) ? null : n });
  }
  async function removeModel(m) {
    const list = (savedModels || []).filter((x) => x !== m);
    if (!list.length) {
      rowTest = { ...rowTest, [m]: { state: 'fail', text: '至少要保留一个模型' } };
      return;
    }
    if (!(await confirm(`删除模型「${m}」？`))) return;
    const nextModel = list.includes(model) ? model : list[0];
    const nextParams = { ...(modelParams || {}) };
    delete nextParams[m];
    if (openModel === m) openModel = '';
    modelParams = nextParams;
    autoSave({ models: list, model: nextModel, model_params: nextParams }, 0);
  }
  function openAdd() {
    addOpen = true;
    addErr = '';
    tick().then(() => addInput?.focus());
  }
  function closeAdd() {
    addOpen = false;
    addText = '';
    addErr = '';
  }
  function addModel() {
    const id = addText.trim();
    if (!id) return;
    const list = savedModels || [];
    if (list.includes(id)) {
      addErr = '这个模型已经在列表里';
      return;
    }
    closeAdd();
    autoSave({ models: [...list, id], model: model || id }, 0);
  }
  /// 单模型连通性：后端用这个模型真发一条极短补全（不是只看模型清单）。
  /// 结果带协议与目标 URL（2026-11-11）：失败能立刻看出「按错协议打错门」。
  const protoLabel = (p) =>
    p === 'anthropic-messages'
      ? 'Anthropic Messages'
      : p === 'openai-responses'
        ? 'Responses'
        : 'Chat Completions';
  async function testModelConn(m) {
    clearTimeout(rowTestTimers[m]);
    rowTest = { ...rowTest, [m]: { state: 'testing', text: '测试中…' } };
    try {
      const r = await fetch('/api/models/test', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ base_url: baseUrl, api_key: apiKey, model: m }),
      });
      const d = await r.json();
      if (!r.ok || !d.ok) {
        const where_ = d.url ? `（${protoLabel(d.protocol)} → ${d.url}）` : '';
        throw new Error((d.error || 'HTTP ' + r.status) + where_);
      }
      rowTest = {
        ...rowTest,
        [m]: {
          state: 'ok',
          text: `连通（${d.latency_ms}ms · ${protoLabel(d.protocol)}）${d.reply ? ' · ' + d.reply : ''}`,
        },
      };
    } catch (e) {
      rowTest = { ...rowTest, [m]: { state: 'fail', text: '连接失败：' + (e.message || e) } };
    }
    // 结果只留 2 秒（用户 2026-11-10：连通提示两秒后消失）
    rowTestTimers[m] = setTimeout(() => {
      const next = { ...rowTest };
      delete next[m];
      rowTest = next;
      delete rowTestTimers[m];
    }, 2000);
  }
  /// 直接改模型 id：列表、model_params 的键、当前模型三处一起迁移
  function renameModel(oldId, newId) {
    const id = String(newId || '').trim();
    const list = savedModels || [];
    if (!id || id === oldId || list.includes(id)) return;
    const nextList = list.map((x) => (x === oldId ? id : x));
    const nextParams = { ...(modelParams || {}) };
    if (nextParams[oldId] !== undefined) {
      nextParams[id] = nextParams[oldId];
      delete nextParams[oldId];
    }
    modelParams = nextParams;
    if (openModel === oldId) openModel = id;
    autoSave({ models: nextList, model: model === oldId ? id : model, model_params: nextParams }, 0);
  }
  function pickModel(m) {
    if (m === model) return;
    autoSave({ model: m }, 0);
  }
</script>

{#if tab === 'model'}
  <div class="field">
    <label>配置</label>
    <div class="config-row">
      <Select
        value={activeConfigId}
        title="切换配置"
        options={configs.map((c) => ({
          value: c.id,
          label: configLabel(c),
          hint: c.has_key ? '' : '无key',
        }))}
        on:change={(e) => manageConfig('apply', e.detail)}
      />
      <button
        class="cfg-del"
        title={configs.length <= 1 ? '至少保留一套配置' : '删除当前配置'}
        disabled={configs.length <= 1}
        on:click={deleteConfig}
      >
        <svg
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.8"
          stroke-linecap="round"
          stroke-linejoin="round"
        ><path d="M3 6h18" /><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6" /><path
            d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"
          /></svg>
      </button>
      <button class="chip add" on:click={newConfig}>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14"/></svg>
        新建配置
      </button>
    </div>
  </div>

  <div class="field">
    <label>配置名称</label>
    <input
      data-field="configName"
      bind:value={configName}
      on:input={() => autoSave({ name: configName })}
    />
  </div>

  <div class="field">
    <label>供应商</label>
    <Select
      value={providerId}
      title="供应商预设"
      options={[
        ...providers.map((p) => ({ value: p.id, label: p.name })),
        // 老渠道可能还挂在已下架的预设 id 上：回显为「旧预设」，不主动改它
        ...(providerId && !providers.some((p) => p.id === providerId)
          ? [{ value: providerId, label: providerId + '（旧预设）' }]
          : []),
      ]}
      on:change={(e) => {
        providerId = e.detail;
        onProviderChange();
      }}
    />
  </div>

  <!-- API 格式三选（2026-11-11）：预设不背协议的账，任何厂商的双协议端点都从这里锁 -->
  <div class="field">
    <label>API 格式</label>
    <Select
      value={protocol}
      title="API 格式"
      options={[
        { value: 'openai', label: 'Chat Completions（/chat/completions）' },
        { value: 'anthropic', label: 'Anthropic Messages（/v1/messages）' },
        { value: 'responses', label: 'Responses（/responses）' },
      ]}
      on:change={(e) => {
        protocol = e.detail;
        autoSave({ protocol });
      }}
    />
    <label class="cap-check" title="不勾时 Base URL 就是最终地址，原样直发一个字符不加">
      <input
        type="checkbox"
        checked={autoSuffix}
        on:change={(e) => {
          autoSuffix = e.currentTarget.checked;
          autoSave({ auto_suffix: autoSuffix });
        }}
      />
      自动补后缀
    </label>
  </div>

  <div class="field">
    <label>Base URL</label>
    <input
      data-field="baseUrl"
      bind:value={baseUrl}
      on:input={() => autoSave({ base_url: baseUrl })}
    />
  </div>

  <div class="field">
    <label>API Key</label>
    <div class="sub-row">
      <input
        bind:value={apiKey}
        type="password"
        class="api-key"
        style="flex: 1;"
        autocomplete="new-password"
        spellcheck="false"
        placeholder={typingHint ? '••••••••••••••••••••' : ''}
        on:blur={() => apiKey.trim() && autoSave({})}
        on:keydown={(e) => e.key === 'Enter' && apiKey.trim() && autoSave({})}
      />
      {#if typingHint}
        <button class="btn del-key" on:click={clearKey} title="清除此配置保存的 API Key">清除 Key</button>
      {/if}
    </div>
  </div>

  <div class="field">
    <label>模型</label>
    <div class="list-head">
      <!-- 与「+ 新建配置」同款 chip 样式（2026-11-11：去掉主按钮配色，消掉割裂感） -->
      <button class="chip add" on:click={fetchModels} title="从端点拉取模型清单">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/></svg>
        获取模型
      </button>
      <button class="chip add" on:click={openAdd} title="添加模型">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14"/></svg>
        添加模型
      </button>
    </div>

    {#if addOpen}
      <div class="add-box">
        <input
          class="m-id-input"
          bind:this={addInput}
          bind:value={addText}
          spellcheck="false"
          on:keydown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault();
              addModel();
            }
            if (e.key === 'Escape') {
              e.preventDefault();
              closeAdd();
            }
          }}
        />
        <button class="btn primary" on:click={addModel}>添加</button>
        <button class="btn ghost" on:click={closeAdd}>取消</button>
      </div>
      {#if addErr}<div class="add-err">{addErr}</div>{/if}
    {/if}

    {#each savedModels as m}
      <div class="model-card" class:cur={m === model}>
        <div class="model-row">
          <span class="cur-dot" class:on={m === model} title={m === model ? '当前模型' : ''}></span>
          <span class="model-name" title={m}>{m}</span>
          <div class="row-actions">
            <button
              class="act"
              class:busy={rowTest[m] && rowTest[m].state === 'testing'}
              disabled={rowTest[m] && rowTest[m].state === 'testing'}
              title="测试连通性"
              on:click={() => testModelConn(m)}
            >
              <svg
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="1.8"
                stroke-linecap="round"
                stroke-linejoin="round"
              ><g transform="rotate(45 12 12)"><path d="M12 22v-5" /><path d="M9 8V2" /><path d="M15 8V2" /><path
                    d="M18 8v5a4 4 0 0 1-4 4h-4a4 4 0 0 1-4-4V8Z"
                  /></g></svg>
            </button>
            <button
              class="act"
              class:on={openModel === m}
              title="编辑参数"
              on:click={() => (openModel = openModel === m ? '' : m)}
            >
              <svg
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="1.8"
                stroke-linecap="round"
                stroke-linejoin="round"
              ><path d="M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z" /><path d="m15 5 4 4" /></svg>
            </button>
            <button class="act del" title="删除模型" on:click={() => removeModel(m)}>
              <svg
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="1.8"
                stroke-linecap="round"
                stroke-linejoin="round"
              ><path d="M3 6h18" /><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6" /><path
                  d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"
                /></svg>
            </button>
          </div>
        </div>
        {#if rowTest[m]}
          <div
            class="row-test"
            class:ok={rowTest[m].state === 'ok'}
            class:fail={rowTest[m].state === 'fail'}
          >{rowTest[m].text}</div>
        {/if}
        {#if openModel === m}
          <div class="model-params">
            {#if m !== model}
              <button class="use-current" on:click={() => pickModel(m)}>设为当前模型</button>
            {/if}
            <div class="mp-row">
              <div class="mp-label">模型 id</div>
              <input
                class="num-input m-id-input"
                value={m}
                spellcheck="false"
                on:change={(e) => renameModel(m, e.currentTarget.value)}
                on:keydown={(e) => {
                  if (e.key === 'Enter') {
                    e.preventDefault();
                    e.currentTarget.blur();
                  }
                }}
              />
            </div>
            {#each NUM_FIELDS as [key, label]}
              <div class="mp-row">
                <div class="mp-label">{label}</div>
                <input
                  class="num-input"
                  type="number"
                  min="0"
                  value={hasOverride(m, key) ? eff(m, key) : ''}
                  on:change={(e) => setNum(m, key, e.currentTarget.value)}
                />
              </div>
            {/each}
            <div class="mp-row caps">
              <div class="mp-label">模型能力</div>
              <div class="caps-row">
                {#each CAPS as [key, label]}
                  <label class="cap-check">
                    <input
                      type="checkbox"
                      checked={!!effCaps(m)[key]}
                      on:change={(e) => setCap(m, key, e.currentTarget.checked)}
                    />
                    {label}
                  </label>
                {/each}
              </div>
            </div>
          </div>
        {/if}
      </div>
    {/each}
  </div>
{/if}

<style>
  /* 配置一行放下拉 + 删除 + 新建（2026-11-11 改版：chips 行换成下拉菜单） */
  .config-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .config-row :global(.csel-wrap) {
    flex: 1;
    min-width: 0;
  }
  .cfg-del {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 34px;
    height: 34px;
    flex: none;
    padding: 0;
    border-radius: 8px;
    border: 1.5px solid var(--g3);
    background: transparent; /* 描边无底色（2026-11-11 设置统一样式） */
    color: var(--g6);
    cursor: pointer;
  }
  .cfg-del svg {
    width: 15px;
    height: 15px;
  }
  .cfg-del:hover {
    color: var(--g7);
    border-color: var(--g5);
  }
  .cfg-del:disabled {
    opacity: 0.4;
    cursor: default;
  }
  .chip {
    padding: 7px 13px;
    border-radius: 14px;
    border: 1px solid var(--g3);
    background: transparent; /* 描边无底色（2026-11-11 设置统一样式） */
    color: var(--g6);
    font-size: 12px;
    cursor: pointer;
  }
  /* 设置面板统一黑白：彩色只留给「已存 Key」的绿点（.dot.haskey） */
  .chip.add {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    color: var(--g6);
    border-color: var(--g3);
  }
  .chip.add svg {
    width: 12px;
    height: 12px;
    flex: none;
  }
  .chip.add:hover {
    color: var(--g7);
    border-color: var(--g5);
  }

  /* 模型区只有一行按钮：左获取模型，右 + 添加模型——两个都是 chip.add 样式，
     与上方「+ 新建配置」同款（标题在上面一行） */
  .list-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }
  .add-box {
    display: flex;
    gap: 8px;
    align-items: center;
    margin-top: 2px;
  }
  .add-box .m-id-input {
    flex: 1;
    min-width: 0;
  }
  .add-err {
    font-size: 11.5px;
    color: var(--tint-red-fg);
  }

  .model-card {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--g3);
    border-radius: 10px;
    background: var(--g1);
    transition: border-color 0.15s ease;
  }
  .model-card:hover {
    border-color: var(--g4);
  }
  .model-card.cur {
    border-color: color-mix(in srgb, var(--brand) 45%, var(--g3));
  }
  .model-row {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 46px;
    padding: 0 8px 0 12px;
  }
  .cur-dot {
    flex: none;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: transparent;
  }
  .cur-dot.on {
    background: var(--brand);
  }
  .model-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13px;
    color: var(--g7);
  }
  .model-card.cur .model-name {
    color: var(--brand-text);
  }
  .row-actions {
    flex: none;
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .act {
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    padding: 0;
    border: 0;
    border-radius: 7px;
    background: transparent;
    color: var(--g5);
    cursor: pointer;
  }
  .act svg {
    width: 15px;
    height: 15px;
  }
  .act:hover {
    background: var(--g2);
    color: var(--g7);
  }
  .act.on {
    /* 开关态钮：统一选中口径（橙字无底色；底色模式铺 --sel-bg） */
    background: var(--sel-bg);
    color: var(--sel-fg);
  }
  .act:disabled {
    opacity: 0.45;
    cursor: default;
    background: transparent;
  }
  .act.del:hover {
    background: var(--tint-red-bg);
    color: var(--tint-red-fg);
  }
  .row-test {
    padding: 0 12px 9px 26px;
    font-size: 11.5px;
    line-height: 1.5;
    color: var(--g5);
    word-break: break-word;
  }
  .row-test.ok {
    color: var(--tint-green-fg);
  }
  .row-test.fail {
    color: var(--tint-red-fg);
  }

  .m-id-input {
    min-width: 0;
    height: 32px;
    padding: 0 10px;
    border: 1px solid var(--g3);
    border-radius: 8px;
    background: var(--g1);
    color: var(--g6);
    font-size: 12.5px;
    font-family: Consolas, 'Cascadia Code', monospace;
  }
  .m-id-input:focus {
    outline: none;
    border-color: var(--g5);
    color: var(--g7);
  }

  .model-params {
    padding: 14px 16px 18px 26px;
    border-top: 1px solid var(--g3);
  }
  .use-current {
    margin-bottom: 8px;
    padding: 3px 10px;
    border: 1px solid color-mix(in srgb, var(--brand) 40%, var(--g3));
    border-radius: 6px;
    background: transparent;
    color: var(--brand-text);
    font-size: 11.5px;
    cursor: pointer;
  }
  .use-current:hover {
    background: color-mix(in srgb, var(--brand) 12%, transparent);
  }
  .mp-row {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-bottom: 12px;
  }
  .mp-row.caps {
    display: block;
    margin-bottom: 0;
  }
  .mp-label {
    flex: none;
    width: 150px;
    font-size: 12px;
    line-height: 1.6;
    color: var(--g6);
  }
  .mp-row .num-input {
    flex: 1;
    min-width: 0;
  }
  .caps-row {
    display: flex;
    gap: 12px 22px;
    flex-wrap: wrap;
    padding: 6px 0 0;
  }
  .cap-check {
    display: inline-flex;
    gap: 5px;
    align-items: center;
    font-size: 12px;
    color: var(--g6);
    cursor: pointer;
  }
</style>
