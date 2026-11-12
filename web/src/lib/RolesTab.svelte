<script>
  import Select from '../Select.svelte';

  export let tab = '';
  // 功能桥接（角色路由）：features.roles 关掉时整个分区收起（App 侧也会把页签切走）
  export let rolesShown = true;
  export let configs = [];
  export let configLabel = () => '';
  // roles 本体在 App 侧（applyState 灌、后端 settings.roles），这里改完整体回传
  export let roles = {};
  export let autoSave = () => {};

  // 功能桥接按「渠道」分组，点渠道才展开它名下的模型；
  // 每个渠道的可选模型 = 设置-模型配置里那串逗号清单（models）。值编码成
  // "配置id::模型"，后端 find_channel 认这个后缀（老值纯配置 id 仍然能跑）。
  function modelsOf(c) {
    return (c.models && c.models.length ? c.models : [c.model]).filter(Boolean);
  }
  /// 某模型的实际能力：模型级覆盖 > 配置级（与后端 caps_for 同口径）。
  /// 功能桥接必须按模型过滤——配置级 video=false 不代表里面的 qwen3-vl-plus 不支持视频。
  function capsOf(c, m) {
    return c.model_params?.[m]?.capabilities || c.capabilities || {};
  }
  /// 按能力过滤后的分组：只把真正支持该能力的模型放进二级菜单。
  /// 整组没有可用模型就不显示这一组；菜单仍是「渠道分组 → 模型」两级结构。
  function groups(list, cap) {
    return list
      .map((c) => {
        const models = modelsOf(c).filter((m) => !cap || capsOf(c, m)[cap]);
        if (!models.length) return null;
        return {
          group: configLabel(c),
          items: models.map((m) => ({ value: `${c.id}::${m}`, label: m })),
        };
      })
      .filter(Boolean);
  }
  function cfgIdOf(v) {
    return String(v || '').split('::')[0];
  }
  // 老值兼容：纯配置 id/名字 → 规范成 "id::当前模型"。不规范化的话它会和分组里
  // 同一条模型各显示一遍（真机实测「为啥有两个 agnes-3.0-flash」）。
  function normValue(v) {
    if (!v) return null;
    const cid = cfgIdOf(v);
    const c = configs.find((x) => x.id === cid || x.name === cid);
    if (!c || String(v).includes('::')) return v;
    const m = (c.model || modelsOf(c)[0] || '').trim();
    return m ? `${c.id}::${m}` : v;
  }
  // 当前值不在任何分组里（配置被删/模型被删）时补一条回显项，别让下拉空着
  function withCurrent(value, opts) {
    if (!value) return opts;
    const hit = opts.some((o) => o.group && o.items.some((it) => it.value === value));
    if (hit) return opts;
    const cid = cfgIdOf(value);
    const c = configs.find((x) => x.id === cid || x.name === cid);
    const model = String(value).includes('::') ? String(value).split('::')[1] : c?.model || '未选模型';
    return [{ value, label: c ? `${configLabel(c)} · ${model}` : String(value) }, ...opts];
  }
  const roleValue = (k) => normValue(roles[k]);

  // 子代理二选一：跟随主渠道 / 指定模型（按渠道分组，和其他功能桥接同一个二级菜单）。
  // 指定模型 = 旧的 subagent_mode=custom + roles.subagent；不再单独占一行。
  // 旧设置里残留的 subagent_mode=ask 按「跟随主渠道」显示（该档已下线）。
  function subagentValue(r) {
    if (r?.subagent_mode === 'custom' && r?.subagent) return normValue(r.subagent);
    return 'parent';
  }
  // 当前选中的是不是「指定模型」值（parent 是模式项，不该拿去分组里找）
  const subagentModel = (r) => {
    const v = subagentValue(r);
    return v === 'parent' ? null : v;
  };
</script>

{#if tab === 'roles' && rolesShown}
  <div class="field roles-section">
    <label>功能桥接</label>
    <div class="roles-grid">
      <div class="role-item">
        <span class="role-name">会话摘要</span>
        <Select
          value={roleValue('summary')}
          title="会话摘要角色"
          options={[
            { value: null, label: '未配置：由主力渠道代跑摘要' },
            ...withCurrent(roleValue('summary'), groups(configs)),
          ]}
          on:change={(e) => {
            roles = { ...roles, summary: e.detail };
            autoSave({ roles }, 0);
          }}
        />
      </div>
      <div class="role-item">
        <span class="role-name">向量嵌入</span>
        <Select
          value={roleValue('embedding')}
          title="向量嵌入角色（记忆库语义检索用）"
          options={[
            { value: null, label: '未配置：记忆库用本地哈希向量（离线可用）' },
            ...withCurrent(roleValue('embedding'), groups(configs)),
          ]}
          on:change={(e) => {
            roles = { ...roles, embedding: e.detail };
            autoSave({ roles }, 0);
          }}
        />
      </div>
      <div class="role-item">
        <span class="role-name">图片理解</span>
        <Select
          value={roleValue('vision')}
          title="图片理解专家"
          options={[
            { value: null, label: '未配置' },
            ...withCurrent(
              roleValue('vision'),
              groups(configs, 'vision'),
            ),
          ]}
          on:change={(e) => {
            roles = { ...roles, vision: e.detail };
            autoSave({ roles }, 0);
          }}
        />
      </div>
      <div class="role-item">
        <span class="role-name">视频理解</span>
        <Select
          value={roleValue('video')}
          title="视频理解专家"
          options={[
            { value: null, label: '未配置' },
            ...withCurrent(
              roleValue('video'),
              groups(configs, 'video'),
            ),
          ]}
          on:change={(e) => {
            roles = { ...roles, video: e.detail };
            autoSave({ roles }, 0);
          }}
        />
      </div>
      <div class="role-item">
        <span class="role-name">音频理解</span>
        <Select
          value={roleValue('audio')}
          title="音频理解专家"
          options={[
            { value: null, label: '未配置' },
            ...withCurrent(
              roleValue('audio'),
              groups(configs, 'audio'),
            ),
          ]}
          on:change={(e) => {
            roles = { ...roles, audio: e.detail };
            autoSave({ roles }, 0);
          }}
        />
      </div>
      <div class="role-item">
        <span class="role-name">子代理</span>
        <Select
          value={subagentValue(roles)}
          title="子代理派发模型：跟随主渠道 / 指定模型"
          options={[
            { value: 'parent', label: '跟随主渠道' },
            // 指定模型：和其他功能桥接一样按渠道分组（点渠道展开模型），不再挤在一个平铺组里
            ...withCurrent(subagentModel(roles), groups(configs)),
          ]}
          on:change={(e) => {
            const v = e.detail;
            if (v === 'parent') roles = { ...roles, subagent_mode: null };
            else roles = { ...roles, subagent_mode: 'custom', subagent: v };
            autoSave({ roles }, 0);
          }}
        />
      </div>
    </div>
  </div>
{/if}

<style>
  .roles-section {
    border-top: 1px solid var(--g2);
    padding-top: 12px;
    margin-top: 4px;
  }
  .roles-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 16px 18px;
    margin-top: 8px;
  }
  .role-item {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .role-name {
    font-size: 11px;
    color: var(--g6);
  }
  /* 自绘下拉（Select 组件）在各容器里的尺寸：组件内部样式管外观，这里只调整体布局 */
  .role-item :global(.csel) {
    font-size: 12px;
    padding: 6px 8px;
  }
</style>
