<script>
  import { createEventDispatcher } from 'svelte';

  const dispatch = createEventDispatcher();
  export let tab = '';
  export let skills = [];
  export let loading = false;
  // 删除确认弹窗：用宿主 App 的通用确认框（confirmAsk）——技能/MCP/定时任务等删除类
  // 一律先弹确认。默认放行以兼容独立挂载（测试用）。
  export let confirm = async () => true;
  // 列表数据(skills)与加载(loadSkills)在 App 侧:能力菜单共用;保存/删除/启停后回酒刷新

  let skillMsg = '';
  // 操作/删除类提示只在本页停留期间有效，下次进入本页（切页签/重开设置）就清
  $: if (tab === 'skills') skillMsg = '';
  let skillEditor = null; // { isNew, name, description, content, source }
  // 新建输入框不再预填模板（正文自己写；name/description 由保存时回填校验）
  // 新建默认走全局（保存位置由后端定：无同名已存在技能 → ~/.pawch-agent/skills/）
  function newSkill() {
    skillEditor = { isNew: true, name: '', description: '', content: '', source: 'global' };
  }
  function editSkill(s) {
    skillEditor = {
      isNew: false,
      name: s.name,
      description: s.description || '',
      content: s.content || '',
      source: s.source || 'global',
    };
  }
  async function saveSkill() {
    if (!skillEditor) return;
    const name = skillEditor.name.trim();
    if (!name) {
      skillMsg = '技能名不能为空';
      return;
    }
    try {
      const r = await fetch('/api/skills', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          action: 'save',
          name,
          description: skillEditor.description,
          content: skillEditor.content,
        }),
      }).then((r) => r.json());
      if (r.error) {
        skillMsg = `保存失败: ${r.error}`;
        return;
      }
      skillMsg = `已保存到 ${r.dir}`;
      skillEditor = null;
      dispatch('reload');
    } catch (e) {
      skillMsg = `保存失败: ${e.message || e}`;
    }
  }
  async function deleteSkill(name) {
    if (!(await confirm(`删除技能「${name}」？不可恢复。`))) return;
    try {
      const r = await fetch('/api/skills', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ action: 'delete', name }),
      }).then((r) => r.json());
      skillMsg = r.error ? `删除失败: ${r.error}` : `已删除 ${name}`;
      dispatch('reload');
    } catch (e) {
      skillMsg = `删除失败: ${e.message || e}`;
    }
  }
  // 2026-10-02：行内启停（与插件页同款 switch）。停用的技能不进系统提示索引与 composer 菜单
  async function toggleSkill(s) {
    try {
      const r = await fetch('/api/skills', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ action: 'toggle', name: s.name, enabled: !(s.enabled !== false) }),
      }).then((r) => r.json());
      skillMsg = r.error ? `操作失败: ${r.error}` : '';
      dispatch('reload');
    } catch (e) {
      skillMsg = `操作失败: ${e.message || e}`;
    }
  }
</script>

      {#if tab === 'skills'}
        <div class="sub-row">
          <button class="btn primary btn-plus" on:click={newSkill}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14"/></svg>
            新建技能
          </button>
          <button class="icon-act" title="刷新" aria-label="刷新" on:click={() => dispatch('reload')}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/></svg>
          </button>
          {#if skillMsg}<span class="hint">{skillMsg}</span>{/if}
        </div>
        <!-- 编辑器开着时不出现下方技能列表（编辑态只留编辑器） -->
        {#if skillEditor}
          <div class="field">
            <!-- 技能统一归全局（保存时项目级旧副本自动搬走），标签直说全局 -->
            <label>{skillEditor.isNew ? '新建技能' : `编辑技能：${skillEditor.name}`}（全局技能）</label>
            <div class="sub-row">
              <!-- 2026-10-08 修：已存在技能的名字一律禁改。原条件只禁全局技能的改名、
                   却放行项目级——而后端 save 只认新名字，旧项目级副本搬不掉（项目级压过
                   全局，列表里旧名照常生效），用户以为改名完成实则旧的还在跑。
                   要改名走删除+新建（后端 /api/skills 没有 old_name 参数，不禁改就是
                   提供一个静默做错的入口） -->
              <input placeholder="技能名" bind:value={skillEditor.name} style="max-width:200px" disabled={!skillEditor.isNew} />
              <input placeholder="一句话描述" bind:value={skillEditor.description} />
            </div>
            <textarea class="agents-editor" rows="14" spellcheck="false" bind:value={skillEditor.content}></textarea>
            <div class="sub-row">
              <button class="btn primary" on:click={saveSkill}>保存</button>
              <button class="btn" on:click={() => (skillEditor = null)}>取消</button>
            </div>
          </div>
        {:else if loading}
          <div class="trace-empty">加载中…</div>
        {:else}
          <!-- 形式对齐插件页 plug-row：图标 + 名字 + 一句话描述 + 行内开关。
               2026-10-07：行标题去掉 $ 前缀（引用前缀早统一成 /，$ 是死记忆）；
               插件自带的技能不在这页列出（s.source !== 'plugin'），2026-10-06 重整后
               五个文档技能降级成全局技能，这页连它们一起管 -->
          {#each skills.filter((s) => s.source !== 'plugin') as s (s.name)}
            <div class="plug-row" class:off={s.enabled === false}>
              <div class="plug-ico"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M13 2 4.5 13.5H11L9.5 22 19 10h-6.5z"/></svg></div>
              <div class="plug-meta">
                <div class="plug-title">{s.name}</div>
                <div class="plug-desc">{s.description || '（无描述）'}</div>
              </div>
              <!-- 来源徽标只标例外：全局技能是这页的常态（每行都挂「全局」= 纯噪音，
                  2026-10-07 真机实测「这些全局按钮可以去掉」）；只有项目级/内置这种
                  少数情况值得亮出来。插件来源的技能本就被上面过滤掉，无需分支。
                  徽标统一放在 flex-sp 之后（靠按钮右对齐）——之前跟在描述后面，
                  位置随描述长度参差（2026-11-10 真机反馈）。 -->
              <span class="flex-sp"></span>
              {#if s.source === 'project'}<span class="plug-chip">项目级</span>{/if}
              {#if s.source === 'builtin'}<span class="plug-chip">内置</span>{/if}
              <button class="icon-act" title="编辑" aria-label="编辑" on:click={() => editSkill(s)}>
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"/><path d="m15 5 4 4"/></svg>
              </button>
              <!-- 内置技能是随包分发的文件，删除入口收起（2026-11-10）；编辑会在
                   数据目录生成覆盖副本，想覆盖就新建同名 -->
              {#if s.source !== 'builtin'}<button class="btn ghost danger" on:click={() => deleteSkill(s.name)}>删除</button>{/if}
              <button class="switch" class:on={s.enabled !== false} role="switch" aria-checked={s.enabled !== false} aria-label="{s.name} 启用/停用" on:click={() => toggleSkill(s)}><span class="knob"></span></button>
            </div>
          {/each}
        {/if}
      {/if}


<style>
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
  /* 停用的技能行：整体降透明度，一眼看出被关掉（对齐插件页停用态） */
  .plug-row.off {
    opacity: 0.55;
  }
  .menu-empty {
    color: var(--g5);
    font-size: 12px;
    padding: 8px 10px;
  }
</style>
