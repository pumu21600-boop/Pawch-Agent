<script>
  import Select from '../Select.svelte';
  import { createEventDispatcher, onDestroy } from 'svelte';

  const dispatch = createEventDispatcher();
  export let tab = '';
  export let servers = [];
  // 动作统一回 App 执行(那边握着 loadMcp 与 settingsMsg);parseHeaders 是表单本地助手
  function mcpAction(action, name, server) { dispatch('action', { action, name, server }); }

  // 021：transport 二选一。stdio=本地子进程；http=远端 MCP（Streamable HTTP）
  // http 再加认证方式（无/自定义头、OAuth 授权码、OAuth 客户端凭据）
  let mcpForm = {
    name: '',
    transport: 'stdio',
    command: '',
    args: '',
    url: '',
    headers: '',
    oauth: 'none',
    oauth_client_id: '',
    oauth_client_secret: '',
    oauth_scope: '',
  };
  // 新建表单默认收起，点「新建 MCP」才展开，和技能页同款（表单开着时只留表单，列表让位）
  let addOpen = false;
  // 每个 server 的工具清单默认收起：工具多的服务（如 playwright 20+ 个）全铺开
  // 会把设置页拉得很长，用户要查时再展开。
  let toolsOpen = {};
  // 2026-11-10：提交后不立刻清空表单——等 App 刷新列表、servers 里真出现这个名字再清；
  // 后端校验失败（名字重复/transport 非法）时服务器列表不会多出它，输入原样保留。
  let pendingAdd = '';
  $: if (pendingAdd && servers.some((s) => s.name === pendingAdd)) {
    pendingAdd = '';
    resetMcpForm();
    addOpen = false;
  }
  // 授权中每 3 秒刷新一次状态（原实现要手动点刷新，「授权中…」永远不结束）
  let pollTimer = null;
  $: pendingAuth = servers.some((s) => s.oauth_status?.status === 'pending');
  $: if (pendingAuth && !pollTimer) {
    pollTimer = setInterval(() => dispatch('reload'), 3000);
  } else if (!pendingAuth && pollTimer) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
  onDestroy(() => {
    if (pollTimer) clearInterval(pollTimer);
  });
  function resetMcpForm() {
    mcpForm = {
      name: '',
      transport: 'stdio',
      command: '',
      args: '',
      url: '',
      headers: '',
      oauth: 'none',
      oauth_client_id: '',
      oauth_client_secret: '',
      oauth_scope: '',
    };
  }
  function cancelAdd() {
    resetMcpForm();
    addOpen = false;
  }
  // 表单里的 headers 写法：每行 `Key: Value`（值里可以有冒号，按第一个冒号切）
  function parseHeaders(text) {
    const out = {};
    for (const line of (text || '').split('\n')) {
      const t = line.trim();
      if (!t) continue;
      const i = t.indexOf(':');
      if (i > 0) out[t.slice(0, i).trim()] = t.slice(i + 1).trim();
    }
    return out;
  }
  function addMcpServer() {
    const isHttp = mcpForm.transport === 'http' || mcpForm.transport === 'sse';
    const args = mcpForm.args.trim() ? mcpForm.args.trim().split(/\s+/) : [];
    // 选了 OAuth 就带 oauth 配置（授权码可留空 client_id 走动态注册）
    const oauth =
      isHttp && mcpForm.oauth !== 'none'
        ? {
            type: mcpForm.oauth,
            client_id: mcpForm.oauth_client_id.trim(),
            client_secret: mcpForm.oauth_client_secret.trim(),
            scope: mcpForm.oauth_scope.trim(),
          }
        : undefined;
    const srv = isHttp
      ? {
          name: mcpForm.name.trim(),
          transport: mcpForm.transport,
          url: mcpForm.url.trim(),
          headers: parseHeaders(mcpForm.headers),
          ...(oauth ? { oauth } : {}),
        }
      : {
          name: mcpForm.name.trim(),
          transport: 'stdio',
          command: mcpForm.command.trim(),
          args,
        };
    // 2026-10-08 修：mcpAction(action, name, server) 是三参——曾按 App.svelte 旧签名
    // 传 4 参（'save', null, null, srv），server 收到 null、srv 被 JS 静默丢弃，
    // 后端 400「缺少 server 配置」，UI 添加 MCP 这条路 100% 不可用。
    mcpAction('save', null, srv);
    // 不在这里清空表单：等列表刷新出现该名字再清（失败保留输入，2026-11-10）
    pendingAdd = srv.name;
  }
</script>

      {#if tab === 'mcp'}
        <!-- 新建表单默认收起，点「新建 MCP」才展开；开着时只留表单（和技能页同款） -->
        <div class="sub-row">
          <button class="btn primary btn-plus" on:click={() => (addOpen = true)}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14"/></svg>
            新建 MCP
          </button>
          <button class="icon-act" title="刷新" aria-label="刷新" on:click={() => dispatch('reload')}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/></svg>
          </button>
        </div>
        {#if addOpen}
          <div class="field">
            <label>新建 MCP server</label>
            <div class="sub-row">
              <input placeholder="名称（字母数字-_）" bind:value={mcpForm.name} style="max-width:160px" />
              <Select
                value={mcpForm.transport}
                title="transport"
                options={[
                  { value: 'stdio', label: 'stdio（本地子进程）' },
                  { value: 'http', label: 'http（远端 MCP）' },
                  { value: 'sse', label: 'sse（远端 MCP·SSE 长连接）' },
                ]}
                on:change={(e) => (mcpForm.transport = e.detail)}
              />
              {#if mcpForm.transport === 'http' || mcpForm.transport === 'sse'}
                <input placeholder="https://host/mcp" bind:value={mcpForm.url} />
              {:else}
                <input placeholder="命令，如 node / npx" bind:value={mcpForm.command} />
                <input placeholder="参数（空格分隔）" bind:value={mcpForm.args} />
              {/if}
              <button
                class="btn primary"
                on:click={addMcpServer}
                disabled={!mcpForm.name.trim() ||
                  (mcpForm.transport === 'stdio' ? !mcpForm.command.trim() : !mcpForm.url.trim())}>添加</button
              >
              <button class="btn" on:click={cancelAdd}>取消</button>
            </div>
            {#if mcpForm.transport === 'http' || mcpForm.transport === 'sse'}
              <div class="sub-row">
                <textarea
                  placeholder={'自定义请求头，每行一条：\nAuthorization: Bearer xxx'}
                  bind:value={mcpForm.headers}
                  rows="2"
                ></textarea>
              </div>
              <div class="sub-row">
                <Select
                  value={mcpForm.oauth}
                  title="认证方式"
                  options={[
                    { value: 'none', label: '无 / 自定义请求头' },
                    { value: 'authorization_code', label: 'OAuth 授权码（浏览器登录）' },
                    { value: 'client_credentials', label: 'OAuth 客户端凭据' },
                  ]}
                  on:change={(e) => (mcpForm.oauth = e.detail)}
                />
              </div>
              {#if mcpForm.oauth !== 'none'}
                <div class="sub-row">
                  <input placeholder="client_id（留空=动态注册）" bind:value={mcpForm.oauth_client_id} style="max-width:220px" />
                  {#if mcpForm.oauth === 'client_credentials'}
                    <input placeholder="client_secret" type="password" bind:value={mcpForm.oauth_client_secret} style="max-width:220px" />
                  {/if}
                  <input placeholder="scope（可选）" bind:value={mcpForm.oauth_scope} style="max-width:160px" />
                </div>
              {/if}
            {/if}
          </div>
        {:else}
          {#each servers as sv}
            <div class="plug-row">
              <div class="plug-ico"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="4" y="4" width="16" height="7" rx="2"/><rect x="4" y="13" width="16" height="7" rx="2"/><circle cx="8" cy="7.5" r="0.9" fill="currentColor" stroke="none"/><circle cx="8" cy="16.5" r="0.9" fill="currentColor" stroke="none"/></svg></div>
              <div class="plug-meta">
                <div class="plug-title">{sv.name}</div>
                <div class="plug-desc">
                  {sv.transport === 'stdio' ? `${sv.command} ${(sv.args || []).join(' ')}` : `${sv.transport.toUpperCase()} · ${sv.url}`}
                </div>
              </div>
              <span class="plug-chip" class:warn={!sv.online}>
                <span class="status-dot" class:online={sv.online} class:offline={!sv.online}></span>
                {sv.online ? '在线' : '离线'}
              </span>
              <span class="flex-sp"></span>
              <button class="icon-act" title="重连" aria-label="重连" on:click={() => mcpAction('refresh', sv.name)}>
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><g transform="rotate(45 12 12)"><path d="M12 22v-5"/><path d="M9 8V2"/><path d="M15 8V2"/><path d="M18 8v5a4 4 0 0 1-4 4h-4a4 4 0 0 1-4-4V8Z"/></g></svg>
              </button>
              <button class="icon-act danger" title="删除" aria-label="删除" on:click={() => mcpAction('remove', sv.name)}>
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18"/><path d="M8 6V4h8v2"/><path d="M19 6l-1 14H6L5 6"/><path d="M10 11v6"/><path d="M14 11v6"/></svg>
              </button>
              <button class="switch" class:on={sv.enabled} role="switch" aria-checked={sv.enabled} aria-label="{sv.name} 启用/停用" on:click={() => mcpAction('toggle', sv.name)}><span class="knob"></span></button>
              {#if sv.tools?.length}
                <!-- 工具清单默认收起：只报数量，点开才列。放在卡片内部，不浮在卡片外 -->
                <div class="mcp-tools">
                  <button
                    class="mcp-tools-toggle"
                    aria-expanded={!!toolsOpen[sv.name]}
                    on:click={() => (toolsOpen = { ...toolsOpen, [sv.name]: !toolsOpen[sv.name] })}
                  >
                    <svg class="mcp-tools-caret" class:open={!!toolsOpen[sv.name]} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m9 6 6 6-6 6" /></svg>
                    可用工具 {sv.tools.length} 个
                  </button>
                  {#if toolsOpen[sv.name]}
                    <div class="mcp-tools-list">
                      {#each sv.tools as t}
                        <div class="mcp-tool-item"><code>{t.name}</code>{t.desc || ''}</div>
                      {/each}
                    </div>
                  {/if}
                </div>
              {/if}
            </div>
            {#if sv.oauth}
              <div class="sub-row mcp-oauth">
                <span class="mcp-status">
                  {#if sv.oauth_status?.status === 'pending'}授权中…
                  {:else if sv.oauth_status?.status === 'error'}授权失败
                  {:else if sv.oauth_status?.oauth?.state === 'authorized'}已授权
                  {:else if sv.oauth_status?.oauth?.state === 'expiring'}即将过期（自动续）
                  {:else}未授权{/if}
                </span>
                <button class="btn ghost" on:click={() => mcpAction('oauth_authorize', sv.name)}>
                  {sv.oauth_status?.oauth?.state === 'none' || !sv.oauth_status?.oauth ? '授权' : '重新授权'}
                </button>
                {#if sv.oauth_status?.oauth && sv.oauth_status.oauth.state !== 'none'}
                  <button class="btn ghost danger" on:click={() => mcpAction('oauth_revoke', sv.name)}>撤销</button>
                {/if}
                {#if sv.oauth_status?.status === 'pending' && sv.oauth_status?.authorization_url}
                  <a href={sv.oauth_status.authorization_url} target="_blank" rel="noreferrer">点这里完成授权</a>
                {/if}
                {#if sv.oauth_status?.status === 'error'}
                  <span class="mcp-status" style="color: var(--tint-red-fg, #e5484d)">{sv.oauth_status.message}</span>
                {/if}
              </div>
            {/if}
          {/each}
        {/if}
      {/if}

<style>
  .mcp-oauth {
    margin: -2px 4px 8px 46px;
  }
  .mcp-status {
    font-size: 11px;
    color: var(--g5);
  }
  /* 在线绿点呼吸；离线红点常亮 */
  .plug-chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }
  .status-dot {
    display: inline-block;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex: none;
  }
  .status-dot.online {
    background: var(--tint-green-fg);
    animation: mcp-breath 1.8s ease-in-out infinite;
  }
  .status-dot.offline {
    background: var(--tint-red-fg);
  }
  @keyframes mcp-breath {
    0%,
    100% {
      opacity: 0.55;
      box-shadow: 0 0 0 0 color-mix(in srgb, var(--tint-green-fg) 45%, transparent);
    }
    50% {
      opacity: 1;
      box-shadow: 0 0 0 4px transparent;
    }
  }
  .mcp-tools {
    flex: 0 0 100%;
    box-sizing: border-box;
    margin: 2px 0 0;
    padding-left: 44px;
  }
  /* MCP 行允许工具行换到卡片内部下一行 */
  .plug-row {
    flex-wrap: wrap;
  }
  .mcp-tools-toggle {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 2px 0;
    border: 0;
    background: transparent;
    color: var(--g5);
    font-size: 11.5px;
    cursor: pointer;
  }
  .mcp-tools-toggle:hover {
    color: var(--g7);
  }
  .mcp-tools-caret {
    width: 11px;
    height: 11px;
    transition: transform 0.15s;
  }
  .mcp-tools-caret.open {
    transform: rotate(90deg);
  }
  .mcp-tools-list {
    font-size: 12px;
    color: var(--g6);
    display: flex;
    flex-direction: column;
    gap: 3px;
    margin-top: 5px;
  }
  .mcp-tool-item code {
    background: var(--g2);
    border: 1px solid var(--g3);
    border-radius: 4px;
    padding: 0 4px;
    margin-right: 6px;
    font-size: 11px;
  }
  .mcp-confirm {
    margin-top: 6px;
  }
</style>
