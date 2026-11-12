<script>
  import Select from '../Select.svelte';
  import ScheduleCronPicker from '../ScheduleCronPicker.svelte';
  import { createEventDispatcher, onDestroy } from 'svelte';

  // 「去查看」：交给宿主 App 打开对应会话（组件内拿不到会话切换逻辑）
  const dispatch = createEventDispatcher();

  export let tab = '';
  export let workspace = '';
  export let recentWorkspaces = [];

  // —— 定时任务（cron 到点唤醒无头会话；完成状态直接落卡片）——
  let schedulesData = [];
  let schedMsg = '';
  // 操作类提示只在本页停留期间有效，下次进入本页（切页签/重开设置）就清
  $: if (tab === 'schedule') schedMsg = '';
  // 通用确认弹窗（桌面壳 WebView2 不实现 window.confirm，同 App 侧 ConfirmModal）
  let confirmModal = null;
  function confirmAsk(text) {
    return new Promise((resolve) => {
      confirmModal = { text, resolve };
    });
  }
  let schedLoading = false;
  // 渠道配置候选（执行模型选择；GET /api/settings 的 configs，含 id/label/model/models）
  let schedConfigs = [];
  // 编辑器：null = 关；isNew = 新建（id 空）。档位三档语义见 SCHED_MODES
  let schedEditor = null;
  // 高级选项（会话/审批/超时）默认折叠；编辑老任务时若与默认不同则自动展开，不藏已配置的东西
  let schedAdvOpen = false;
  let schedReloadTimer = null;
  // 2026-10-08 修：编辑中/点了「立即运行」后立刻关设置弹窗，组件已销毁但定时器
  // 到点仍会对已销毁实例发 fetch 并写状态（Svelte4 不报错，纯浪费）。
  onDestroy(() => {
    clearTimeout(schedReloadTimer);
  });
  // 三档（2026-11-08 与「高危」下拉合并成一个：无人值守没有人点允许，高危策略由
  // 档位推导——只读/自动命中审批一律拒绝并回喂，完全访问全免审，
  // 后端 schedule::unattended_on_approval；标签不带括号说明）
  const SCHED_MODES = [
    { value: 'read-only', label: '只读' },
    { value: 'auto', label: '自动' },
    { value: 'full-access', label: '完全访问' },
  ];
  // 老任务档位归一（旧档名/空串 → 三档之一，映射与后端 normalize_mode 一致）
  function normApprovalMode(v) {
    const s = String(v || '').trim();
    if (s === 'suggest' || s === 'sandbox') return 'read-only';
    if (s === '' || s === 'auto-edit' || s === 'full-auto') return 'auto';
    if (s === 'read-only' || s === 'auto' || s === 'full-access') return s;
    return 'auto';
  }
  function schedAdvNonDefault(t) {
    const n = (v, d) => (v == null || v === '' ? d : v);
    return (
      n(t.session_mode, 'new') !== 'new' ||
      !!(t.session_id && t.session_id.trim()) ||
      normApprovalMode(t.approval_mode) !== 'auto' ||
      !!(t.config_id && t.config_id.trim()) ||
      !!(t.model && t.model.trim()) ||
      (Number(t.timeout_secs) || 1800) !== 1800
    );
  }
  // 工作区：默认跟随触发时的当前工作区；下拉里给最近项目（去重）
  function wsLabel(w) {
    return String(w).split(/[\\/]/).filter(Boolean).pop() || w;
  }
  // 2026-10-08 修：options={fn()} 在模板里不建依赖（Svelte4 只跟踪表达式直接引用
  // 的变量，函数体里读的 schedConfigs/workspace/recentWorkspaces 换了不重算）——
  // 真机实测：列表还在加载时开编辑器，供应商下拉永远只有「跟随当前激活配置」一项。
  // 改走响应式声明：$: 语句跟踪函数体依赖，源数据一换选项就重算。
  $: wsOpts = wsOptions();
  // —— 模型二级菜单（与功能桥接同款）：首项跟随当前激活配置，下面按渠道分组、
  // 点渠道展开它名下的模型。值编码成 "配置id::模型"，选中时拆回 config_id/model
  // 两个字段存（后端字段不动，落库值与旧任务一致）。
  function schedModelsOf(c) {
    return (c.models && c.models.length ? c.models : [c.model]).filter(Boolean);
  }
  // 编辑回显：config_id+model → 复合值。老任务只存了配置 id 的规范成「id::该配置
  // 当前模型」（RolesTab normValue 同款），否则会跟分组里同一条模型各显示一遍
  function schedModelValue(e) {
    if (!e || !e.config_id) return '';
    const c = schedConfigs.find((x) => x.id === e.config_id);
    const m = String(e.model || '').trim() || (c ? String(c.model || schedModelsOf(c)[0] || '').trim() : '');
    return m ? `${e.config_id}::${m}` : e.config_id;
  }
  $: schedModelOpts = (() => {
    const opts = [{ value: '', label: '跟随当前激活配置' }];
    for (const c of schedConfigs) {
      const items = schedModelsOf(c).map((m) => ({ value: `${c.id}::${m}`, label: m }));
      if (items.length) opts.push({ group: c.name, items });
    }
    // 当前值不在任何分组里（配置被删）时补一条回显项，别让下拉空着
    const cur = schedEditor ? schedModelValue(schedEditor) : '';
    if (cur && !opts.some((o) => o.group && o.items.some((it) => it.value === cur))) {
      const model = String((schedEditor && schedEditor.model) || '').trim();
      opts.splice(1, 0, { value: cur, label: model ? `配置已不在 · ${model}` : cur });
    }
    return opts;
  })();
  // 选中值同样走 $: 声明——模板里调函数读不到函数体里的 schedConfigs（同上面那个坑）
  $: schedModelCur = schedEditor ? schedModelValue(schedEditor) : '';
  function wsOptions() {
    const seen = new Set();
    const opts = [{ value: '', label: '跟随当前工作区' }];
    for (const w of [workspace, ...recentWorkspaces]) {
      if (w && !seen.has(w)) {
        seen.add(w);
        opts.push({ value: w, label: wsLabel(w) });
      }
    }
    return opts;
  }
  async function loadSchedules() {
    schedLoading = true;
    // 只清上一次加载留下的「读取失败」：操作反馈（已保存/触发失败…）留到下一次操作，
    // 不被加载抹掉。2026-10-08 修：这行原在 try/catch 之后——catch 刚设的错误
    // 在同一同步块里又被无条件擦掉，「服务没起时读取失败」永远看不见（死代码）。
    if (schedMsg.startsWith('读取失败')) schedMsg = '';
    try {
      const [r, st] = await Promise.all([
        fetch('/api/schedules').then((r) => r.json()),
        fetch('/api/settings', { cache: 'no-store' }).then((r) => r.json()).catch(() => ({})),
      ]);
      schedConfigs = (st.configs || []).map((c) => ({
        id: c.id,
        name: c.name || c.provider_id || c.id.slice(0, 8),
        model: c.model || '',
        models: c.models || [],
        context_length: c.context_length || 0,
      }));
      schedulesData = r.schedules || [];
    } catch (e) {
      schedMsg = `读取失败: ${e.message || e}`;
    } finally {
      schedLoading = false;
    }
    // 只清自己留下的读取失败：操作反馈（已保存/触发失败…）留到下一次操作，不被加载抹掉
    if (schedMsg.startsWith('读取失败')) schedMsg = '';
  }
  function newSchedule() {
    schedMsg = '';
    schedAdvOpen = false;
    // 输入框一律留空不预填：cron/回顾天数/超时都有占位提示，留空走默认（天数缺省 3、超时缺省 1800）
    schedEditor = {
      id: '',
      name: '',
      kind: 'message',
      cron: '',
      once_ts: 0,
      message: '',
      memory_days: '',
      session_mode: 'new',
      session_id: '',
      workspace: '',
      approval_mode: 'auto',
      enabled: true,
      timeout_secs: '',
      budget_tokens: '',
      config_id: '',
      model: '',
      isNew: true,
    };
  }
  function editSchedule(t) {
    // 已完成的单次任务：打开编辑即重新启用，保存后回到「待运行」（时间要改到将来）
    const revive = t.once_ts > 0 && t.last_status === 'done' && t.enabled === false;
    schedMsg = revive ? '已完成的单次任务：保存会重新启用，请把时间改到将来' : '';
    schedAdvOpen = schedAdvNonDefault(t);
    // 超时回显为空 + 占位符，不显示突兀的 0；保存时留空按 1800 发（见 saveSchedule）
    schedEditor = { ...t, enabled: revive ? true : t.enabled !== false, isNew: false, timeout_secs: Number(t.timeout_secs) || '', memory_days: t.memory_days || '', budget_tokens: t.budget_tokens ? Number(t.budget_tokens) : '', approval_mode: normApprovalMode(t.approval_mode), config_id: t.config_id || '', model: t.model || '' };
  }
  async function saveSchedule() {
    const e = schedEditor;
    if (!e) return;
    // body 先组装：fetch 调用点往后 16 行内必须出现 .json()（scripts/check_fetch_json.mjs 硬体检）
    const body = {
      id: e.id || '',
      name: e.name,
      kind: e.kind || 'message',
      cron: e.cron,
      once_ts: Number(e.once_ts) || 0,
      message: e.message,
      memory_days: Number(e.memory_days) || 3,
      session_mode: e.session_mode,
      session_id: e.session_id,
      workspace: e.workspace || '',
      approval_mode: e.approval_mode,
      enabled: e.enabled !== false,
      // 留空 = 1800（与服务端 DEFAULT_TIMEOUT_SECS 对齐，显式发值不靠「0 走默认」隐式契约，F10）
      timeout_secs: Number(e.timeout_secs) || 1800,
      // 上下文上限（tokens）：留空=0=跟全局/渠道口径，填了本任务硬覆盖
      budget_tokens: Number(e.budget_tokens) || 0,
      config_id: e.config_id || '',
      model: e.model || '',
    };
    // 单次模式没选时间（或周期模式没产出 cron）：本地拦下，不发请求
    if (!String(body.cron || '').trim() && !body.once_ts) {
      schedMsg = '还没选执行时间';
      return;
    }
    // 单次时间已过：本地直接提示，不等服务端校验（已跑完停用的任务编辑不受限，与服务端口径一致）
    if (body.once_ts && body.enabled && body.once_ts <= Math.floor(Date.now() / 1000)) {
      schedMsg = '单次任务的时间要设在将来';
      return;
    }
    try {
      const r = await fetch('/api/schedules', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      }).then((r) => r.json());
      if (r.ok) {
        schedEditor = null;
        await loadSchedules();
        schedMsg = '已保存'; // 放在加载之后：loadSchedules 会清提示，顺序反了就被抹掉
      } else {
        schedMsg = `保存失败: ${r.error}`;
        // 编辑器里挂着的任务已从列表消失（被删除/外部清掉）：关掉编辑器，说人话
        if (String(r.error || '').includes('任务不存在')) {
          schedEditor = null;
          schedMsg = '这条任务已不在列表里（可能已被删除），请重新创建';
        }
      }
    } catch (err) {
      schedMsg = `保存失败: ${err.message || err}`;
    }
  }
  async function deleteSchedule(id) {
    if (!(await confirmAsk('删除这条定时任务？（已跑出的会话与记录保留）'))) return;
    try {
      const r = await fetch(`/api/schedules?id=${encodeURIComponent(id)}`, { method: 'DELETE' }).then((r) => r.json());
      if (r.ok) {
        await loadSchedules();
        schedMsg = '已删除';
      } else {
        schedMsg = `删除失败: ${r.error}`;
      }
    } catch (e) {
      schedMsg = `删除失败: ${e.message || e}`;
    }
  }
  // 「立即运行」防连点（F6）：跑着的任务按钮置灰，响应回来才恢复
  let runningIds = {};
  async function runScheduleNow(id) {
    if (runningIds[id]) return;
    runningIds = { ...runningIds, [id]: true };
    try {
      const r = await fetch(`/api/schedules/run?id=${encodeURIComponent(id)}`, { method: 'POST' }).then((r) => r.json());
      schedMsg = r.ok ? '已触发，完成后卡片状态会更新' : `触发失败: ${r.error}`;
      clearTimeout(schedReloadTimer);
      schedReloadTimer = setTimeout(loadSchedules, 1500);
    } catch (e) {
      schedMsg = `触发失败: ${e.message || e}`;
    } finally {
      runningIds = { ...runningIds, [id]: false };
    }
  }
  // unix 秒 → 本地时间字符串（定时任务的运行时间展示）
  function fmtTs(ts) {
    if (!ts) return '—';
    const d = new Date(ts * 1000);
    const p = (n) => String(n).padStart(2, '0');
    return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
  }
  /// 卡片状态：完成/失败/停用直接上卡片，不再单独堆一层「完成记录」。
  function fmtShortTs(ts) {
    const s = fmtTs(ts);
    return s === '—' ? '' : s.slice(5);
  }
  function schedState(t) {
    const st = t.last_status || '';
    const at = fmtShortTs(Math.max(t.last_run_ts || 0, t.last_manual_ts || 0));
    const tail = at ? ` · ${at}` : '';
    if (t.enabled === false) {
      // 单次任务跑完会自动停用：这里显示「已完成」而不是「已停用」
      if (t.once_ts > 0 && st === 'done') return { cls: 'done', text: `已完成${tail}` };
      return { cls: 'off', text: '已停用' };
    }
    if (st === 'done') return { cls: 'done', text: `上次完成${tail}` };
    if (st === 'timeout') return { cls: 'warn', text: `上次超时${tail}` };
    if (st === 'cancelled') return { cls: 'warn', text: `上次取消${tail}` };
    if (st === 'skipped') return { cls: 'off', text: `上次跳过${tail}` };
    if (st === 'error') return { cls: 'err', text: `上次出错${tail}` };
    return { cls: 'run', text: '待运行' };
  }
  // 组件化回归修复（审计 02）：响应式声明必须在 <script> 顶层——原先缩进在 fmtTs
  // 函数体内被 Svelte 当普通 JS 标签编译，切页签永远不会自动拉列表
  $: if (tab === 'schedule') loadSchedules();
</script>

      {#if tab === 'schedule'}
        <div class="sub-row">
          <button class="btn primary btn-plus" on:click={newSchedule}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14"/></svg>
            新建任务
          </button>
          <button class="icon-act" title="刷新" aria-label="刷新" on:click={loadSchedules}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/></svg>
          </button>
          <span class="hint">{schedMsg}</span>
        </div>
        {#if schedEditor}
          <div class="field">
            <label>{schedEditor.isNew ? '新建定时任务' : `编辑：${schedEditor.name}`}</label>
            <div class="sub-row">
              <input placeholder="名称" bind:value={schedEditor.name} style="max-width:220px" />
              <Select
                value={schedEditor.kind || 'message'}
                title="任务类型"
                options={[
                  { value: 'message', label: '固定消息' },
                  { value: 'memory', label: '记忆提醒（回顾最近对话）' },
                ]}
                on:change={(e) => (schedEditor.kind = e.detail)}
              />
              <label class="mcp-risk-label"><input type="checkbox" bind:checked={schedEditor.enabled} /> 启用</label>
            </div>
            <div class="sub-row sched-time-row">
              <!-- 行尾的 cron 人话预览已去掉（2026-11-12 用户：说明多余）——
                   频率行自己的控件就是完整表达；旧格式提示由选择器内的 cp-hint 负责 -->
              <ScheduleCronPicker bind:cron={schedEditor.cron} bind:onceTs={schedEditor.once_ts} />
              {#if (schedEditor.kind || 'message') === 'memory'}
                <input placeholder="回顾天数" type="number" bind:value={schedEditor.memory_days} style="max-width:110px" />
              {/if}
            </div>
            {#if (schedEditor.kind || 'message') === 'message'}
              <textarea
                class="agents-editor"
                rows="4"
                spellcheck="false"
                placeholder="到点时唤醒 agent 的消息"
                bind:value={schedEditor.message}
              ></textarea>
            {:else}
              <div class="mcp-cmd">到点先检索最近 {schedEditor.memory_days || 3} 天的对话，让 agent 找出要跟进的事项</div>
            {/if}
            <div class="field">
              <label>工作区选择</label>
              <Select
                value={schedEditor.workspace || ''}
                title="工作区"
                options={wsOpts}
                on:change={(e) => (schedEditor.workspace = e.detail)}
              />
            </div>
            <div class="field">
              <label>模型</label>
              <!-- 与功能桥接同款二级菜单：点渠道展开模型；值 = 配置id::模型，拆回两字段存 -->
              <Select
                value={schedModelCur}
                title="执行模型：留空跟随当前激活配置；点渠道名展开后选模型"
                options={schedModelOpts}
                on:change={(e) => {
                  const v = String(e.detail || '');
                  const i = v.indexOf('::');
                  schedEditor.config_id = i < 0 ? v : v.slice(0, i);
                  schedEditor.model = i < 0 ? '' : v.slice(i + 2);
                }}
              />
            </div>
            <div class="sub-row">
              <button class="btn ghost" on:click={() => (schedAdvOpen = !schedAdvOpen)}>{schedAdvOpen ? '收起高级选项' : '高级选项'}</button>
            </div>
            {#if schedAdvOpen}
              <div class="sub-row">
                <Select
                  value={schedEditor.session_mode}
                  title="会话模式"
                  options={[
                    { value: 'new', label: '每轮新会话' },
                    { value: 'fixed', label: '固定会话' },
                  ]}
                  on:change={(e) => (schedEditor.session_mode = e.detail)}
                />
                {#if schedEditor.session_mode === 'fixed'}
                  <input placeholder="会话 id" bind:value={schedEditor.session_id} style="max-width:260px" />
                {/if}
              </div>
              <div class="sub-row">
                <!-- 高危策略不再单独选（2026-11-08 合并）：由档位推导——只读/自动命中审批拒绝、完全访问全免审 -->
                <Select
                  value={schedEditor.approval_mode}
                  title="审批档位"
                  options={SCHED_MODES}
                  on:change={(e) => (schedEditor.approval_mode = e.detail)}
                />
                <input placeholder="超时（秒）" type="number" min="0" bind:value={schedEditor.timeout_secs} style="max-width:110px" />
                <input
                  placeholder="上下文上限"
                  type="number"
                  min="0"
                  bind:value={schedEditor.budget_tokens}
                  style="max-width:150px"
                  title="本任务的上下文 token 预算。留空=跟全局设置或渠道的 context_length；要读更多历史、不想被过早裁剪就填大值。超过模型真实窗口会报错，按你渠道的量力填。"
                />
              </div>
            {/if}
            <div class="sub-row">
              <button class="btn primary" on:click={saveSchedule}>保存</button>
              <button class="btn" on:click={() => (schedEditor = null)}>取消</button>
            </div>
          </div>
        <!-- 与技能页同款：编辑器开着只留编辑器，列表让位（2026-10-05 真机实测回归：
             编辑时该任务还挂在下面） -->
        {:else if schedLoading && !schedulesData.length}
          <div class="trace-empty">加载中…</div>
        {:else}
          {#each schedulesData as t (t.id)}
            {@const st = schedState(t)}
            {@const ws = t.workspace || workspace}
            {@const cfg = t.config_id ? schedConfigs.find((c) => c.id === t.config_id) : null}
            <div class="sched-card" class:done={st.cls === 'done'} class:err={st.cls === 'err'} class:off={st.cls === 'off'}>
              <div class="sched-head">
                <span class="sched-state {st.cls}"><span class="sched-dot"></span>{st.text}</span>
                <b class="sched-name" title={t.name}>{t.name}</b>
                {#if t.kind === 'memory'}<span class="sched-tag">记忆 · {t.memory_days || 3} 天</span>{/if}
                <div class="sched-actions">
                  <button class="sched-act" disabled={runningIds[t.id]} on:click={() => runScheduleNow(t.id)} title="立即运行" aria-label="立即运行">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="m8 5 11 7-11 7Z"/></svg>
                  </button>
                  <button class="sched-act" on:click={() => editSchedule(t)} title="编辑" aria-label="编辑">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"/><path d="m15 5 4 4"/></svg>
                  </button>
                  {#if t.last_session_id}
                    <button class="sched-act" on:click={() => dispatch('open-session', { id: t.last_session_id })} title="去查看（上次运行的会话）" aria-label="去查看">
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M14 4h6v6"/><path d="M20 4 10 14"/><path d="M20 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1h5"/></svg>
                    </button>
                  {/if}
                  <button class="sched-act del" on:click={() => deleteSchedule(t.id)} title="删除" aria-label="删除">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18"/><path d="M8 6V4h8v2"/><path d="M19 6l-1 14H6L5 6"/><path d="M10 11v6"/><path d="M14 11v6"/></svg>
                  </button>
                </div>
              </div>
              <div class="sched-line" title={t.kind === 'memory' ? `到点回顾最近 ${t.memory_days || 3} 天的对话` : t.message}>
                {#if t.kind === 'memory'}到点回顾最近 {t.memory_days || 3} 天的对话，找出要跟进的事项{:else}{t.message}{/if}
              </div>
              <div class="sched-meta">
                <span>{t.cron_text}</span>
                {#if ws}<span title={ws}>@ {wsLabel(ws)}</span>{/if}
                {#if cfg || t.config_id}<span title={`用配置 ${cfg?.name || t.config_id.slice(0, 8)} 跑${t.model ? `（${t.model}）` : ''}`}>◆ {cfg?.name || t.config_id.slice(0, 8)}{t.model ? ` · ${t.model}` : ''}</span>{/if}
                {#if t.budget_tokens}<span title={`本任务上下文上限 ${Number(t.budget_tokens).toLocaleString()} tokens`}>⛨ {Number(t.budget_tokens).toLocaleString()} tokens</span>{/if}
                {#if t.enabled !== false && t.next_run_ts}<span class="sched-next">下次 {fmtTs(t.next_run_ts)}</span>{/if}
              </div>
              {#if t.last_error}
                <div class="sched-err" title={t.last_error}>{t.last_error}</div>
              {/if}
            </div>
          {/each}
        {/if}
      {/if}

      {#if confirmModal}
        <div class="modal-backdrop" on:click={() => { const r = confirmModal.resolve; confirmModal = null; r(false); }}>
          <div class="modal confirm-modal" on:click|stopPropagation>
            <div class="modal-head"><b>确认</b></div>
            <div class="confirm-text">{confirmModal.text}</div>
            <div class="confirm-actions">
              <button class="btn" on:click={() => { const r = confirmModal.resolve; confirmModal = null; r(false); }}>取消</button>
              <button class="btn primary" on:click={() => { const r = confirmModal.resolve; confirmModal = null; r(true); }}>确定</button>
            </div>
          </div>
        </div>
      {/if}

<style>
  /* 时间行：时间选择器下方可能带一行提示（选将来日期/勾星期），把行撑高后
     center 对齐会让旁边的「回顾天数」视觉偏下（2026-11-10 真机反馈）。
     该行改顶部对齐，时长输入框与日期/时间框顶部对齐。 */
  .sched-time-row {
    align-items: flex-start;
  }
  .confirm-modal {
    min-width: 380px;
    max-width: 520px;
  }
  .confirm-text {
    white-space: pre-line;
    font-size: 14px;
    line-height: 1.6;
    color: var(--g6);
  }
  .confirm-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 16px;
  }
  .menu-empty {
    color: var(--g5);
    font-size: 12px;
    padding: 8px 10px;
  }
  .agents-editor {
    width: 100%;
    box-sizing: border-box;
    font-family: Consolas, 'Courier New', monospace;
    font-size: 12.5px;
    line-height: 1.7;
    background: transparent; /* 描边无底色（2026-11-11 设置统一样式） */
    color: var(--g7);
    border: 1px solid var(--g3);
    border-radius: 8px;
    padding: 12px 14px;
    resize: vertical;
  }
  .agents-editor:focus {
    outline: none;
    border-color: var(--g4);
  }
  /* 任务卡片：状态在卡片上，操作收成图标行，正文/元信息各一行 */
  .sched-card {
    display: flex;
    flex-direction: column;
    gap: 6px;
    border: 1px solid var(--g3);
    border-radius: 10px;
    padding: 10px 12px;
  }
  .sched-card.done {
    border-color: color-mix(in srgb, var(--tint-green-fg) 30%, var(--g3));
  }
  .sched-card.err {
    border-color: color-mix(in srgb, var(--tint-red-fg) 35%, var(--g3));
  }
  .sched-card.off {
    opacity: 0.82;
  }
  .sched-head {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .sched-state {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 2px 8px;
    border-radius: 999px;
    font-size: 11px;
    line-height: 1.4;
    white-space: nowrap;
  }
  .sched-state.done {
    color: var(--tint-green-fg);
    background: var(--tint-green-bg);
  }
  .sched-state.err {
    color: var(--tint-red-fg);
    background: var(--tint-red-bg);
  }
  .sched-state.warn {
    color: var(--tint-orange-fg);
    background: var(--tint-orange-bg);
  }
  .sched-state.off {
    color: var(--g5);
    background: var(--g2);
  }
  .sched-state.run {
    color: var(--brand-text);
    background: color-mix(in srgb, var(--brand) 12%, transparent);
  }
  .sched-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: currentColor;
    flex: none;
  }
  .sched-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--g7);
  }
  .sched-tag {
    flex: none;
    font-size: 11px;
    color: var(--g5);
  }
  .sched-actions {
    flex: none;
    display: flex;
    align-items: center;
    gap: 2px;
    margin-left: auto;
  }
  .sched-act {
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
  .sched-act svg {
    width: 15px;
    height: 15px;
  }
  .sched-act:hover {
    background: var(--g2);
    color: var(--g7);
  }
  .sched-act:disabled {
    opacity: 0.45;
    cursor: default;
    background: transparent;
  }
  .sched-act.del:hover {
    background: var(--tint-red-bg);
    color: var(--tint-red-fg);
  }
  .sched-line {
    font-size: 12px;
    color: var(--g6);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sched-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 3px 10px;
    font-size: 11px;
    color: var(--g5);
  }
  .sched-meta .sched-next {
    color: var(--g6);
  }
  .sched-err {
    font-size: 11.5px;
    color: var(--tint-red-fg);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .mcp-cmd {
    font-family: Consolas, monospace;
    font-size: 11px;
    color: var(--g5);
    margin: 6px 0;
    word-break: break-all;
  }
  .mcp-risk-label {
    font-size: 12px;
    color: var(--g6);
  }
</style>
