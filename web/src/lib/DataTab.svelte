<script>
  // 数据管理（2026-11-02，2026-11-03 改版）：
  // 安装目录 / 数据目录都是可编辑输入框（📂 选目录或直接填），改了才能点「迁移」；
  // 迁移只复制不删旧，完成后自动重启后端，再弹窗问旧目录「删除 / 我自己删」。
  export let tab = '';
  export let confirm = async () => true;

  let info = null;
  let loading = false;
  let busy = '';
  let msg = '';
  let err = '';
  // 操作类提示只在本页停留期间有效，下次进入本页（切页签/重开设置）就清
  $: if (tab === 'data') {
    msg = '';
    err = '';
  }
  let withCache = false;
  let installInput = '';
  let dataInput = '';
  // 迁移/导入完成后的旧目录清理弹窗：{ install, data, kind: 'migrate'|'import' }
  let cleanup = null;
  // 导入进度（后端解压时目录在增长，前端 400ms 轮询）
  let progress = null;
  let pct = 0;
  let progTimer = null;

  async function api(path, body) {
    const res = await fetch(path, body === undefined ? {} : {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });
    const j = await res.json().catch(() => ({}));
    if (!res.ok || j.error) throw new Error(j.error || `HTTP ${res.status}`);
    return j;
  }

  const norm = (s) => (s || '').trim().replace(/[\\/]+$/, '').toLowerCase();
  const isAbs = (s) => /^[a-zA-Z]:[\\/]/.test((s || '').trim()) || /^\\\\/.test((s || '').trim());
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

  async function load() {
    loading = true;
    err = '';
    try {
      info = await api('/api/data/info');
      installInput = info.install_dir;
      dataInput = info.data_dir;
    } catch (e) {
      err = e.message || String(e);
    } finally {
      loading = false;
    }
  }
  $: if (tab === 'data' && !info && !loading) load();

  $: changedInstall = !!info && norm(installInput) !== norm(info.install_dir);
  $: changedData = !!info && norm(dataInput) !== norm(info.data_dir);
  $: canMigrate = !busy && !!info && (changedInstall || changedData) && isAbs(installInput) && isAbs(dataInput);

  function fmtBytes(n) {
    const v = Number(n) || 0;
    if (v >= 1024 * 1024 * 1024) return (v / 1024 / 1024 / 1024).toFixed(1) + ' GB';
    if (v >= 1024 * 1024) return (v / 1024 / 1024).toFixed(1) + ' MB';
    if (v >= 1024) return (v / 1024).toFixed(0) + ' KB';
    return v + ' B';
  }

  async function pick(kind) {
    err = '';
    try {
      const p = await api('/api/data/pick', { kind });
      if (!p.path) return;
      if (kind === 'install') installInput = p.path;
      else if (kind === 'data') dataInput = p.path;
    } catch (e) {
      err = e.message || String(e);
    }
  }

  async function openDir(which) {
    try {
      await api('/api/data/open', { which });
    } catch (e) {
      err = e.message || String(e);
    }
  }

  async function waitForRestart(oldStarted) {
    for (let i = 0; i < 120; i++) {
      await sleep(500);
      try {
        const d = await api('/api/data/info');
        if (d.started_at && d.started_at !== oldStarted) return true;
      } catch {
        // 重启窗口内连不上，继续等
      }
    }
    return false;
  }

  async function doMigrate() {
    if (!canMigrate) return;
    msg = '';
    err = '';
    const lines = [];
    lines.push(`安装：${info.install_dir}${changedInstall ? `\n      → ${installInput.trim()}` : '（不变）'}`);
    lines.push(`数据：${info.data_dir}${changedData ? `\n      → ${dataInput.trim()}` : '（不变）'}`);
    const ok = await confirm(`迁移目录：\n${lines.join('\n')}\n\n只复制、不删旧目录；完成后会自动重启后端。继续？`);
    if (!ok) return;
    const started0 = (info && info.started_at) || 0;
    busy = 'migrate';
    try {
      const r = await api('/api/data/migrate', {
        install_dir: installInput.trim(),
        data_dir: dataInput.trim(),
        include_cache: withCache,
      });
      if (!r.restarting) {
        msg = '已复制完成，但自动重启失败，请手动重启 Pawch 后生效。';
        return;
      }
      msg = '已复制，正在重启后端…';
      const ok2 = await waitForRestart(started0);
      if (!ok2) {
        err = '后端 60 秒内没有恢复，请手动重启 Pawch。';
        return;
      }
      msg = '';
      if (r.old_install || r.old_data) {
        cleanup = { install: r.old_install, data: r.old_data, kind: 'migrate' };
        return;
      }
      location.reload();
    } catch (e) {
      err = e.message || String(e);
    } finally {
      busy = '';
    }
  }

  async function finishCleanup(deleteNow) {
    const c = cleanup;
    cleanup = null;
    if (deleteNow) {
      try {
        const r = await api('/api/data/cleanup_old', {});
        if ((r.scheduled || []).length) {
          // 旧目录里还有正在运行的壳/缓存：重启整个应用后才能删
          msg = '旧目录里有正在运行的程序，正在重启应用，退出后会自动删除…';
          const started0 = (info && info.started_at) || 0;
          await api('/api/data/restart_app', {});
          if (await waitForRestart(started0)) {
            location.reload();
            return;
          }
          return;
        }
      } catch (e) {
        err = e.message || String(e);
      }
    } else {
      try {
        await api('/api/data/forget_old', {});
      } catch {
        // 没有记录也无所谓
      }
    }
    location.reload();
  }

  async function pollProgress() {
    try {
      const p = await api('/api/data/import/progress');
      progress = p;
      pct = p.total_bytes > 0 ? Math.min(99, Math.round((p.bytes / p.total_bytes) * 100)) : 0;
    } catch {
      // 后端正在重启时轮询会失败，忽略
    }
  }
  function startProgressPoll() {
    stopProgressPoll();
    progress = { running: true, bytes: 0, total_bytes: 0, files: 0, total_files: 0 };
    pct = 0;
    progTimer = setInterval(pollProgress, 400);
  }
  function stopProgressPoll() {
    if (progTimer) {
      clearInterval(progTimer);
      progTimer = null;
    }
  }

  async function doExport() {
    msg = '';
    err = '';
    try {
      busy = 'export';
      const r = await api('/api/data/export', { include_cache: withCache });
      if (r.cancelled) return;
      msg = `已导出 ${r.files} 个文件到 ${r.path}（${fmtBytes(r.zip_bytes)}）。`;
    } catch (e) {
      err = e.message || String(e);
    } finally {
      busy = '';
    }
  }

  async function doImport() {
    msg = '';
    err = '';
    const started0 = (info && info.started_at) || 0;
    try {
      busy = 'import';
      startProgressPoll();
      const r = await api('/api/data/import', {});
      if (r.cancelled) return;
      pct = 100;
      stopProgressPoll();
      if (!r.restarting) {
        msg = '已导入，但自动重启失败，请手动重启 Pawch 后生效。';
        return;
      }
      msg = '已替换数据，正在重启后端…';
      if (!(await waitForRestart(started0))) {
        err = '后端 60 秒内没有恢复，请手动重启 Pawch。';
        return;
      }
      msg = '';
      // 旧数据已备份到 backup 目录，问用户删不删
      if (r.backup) {
        cleanup = { install: null, data: r.backup, kind: 'import' };
        return;
      }
      location.reload();
    } catch (e) {
      err = e.message || String(e);
    } finally {
      stopProgressPoll();
      busy = '';
    }
  }
</script>

{#if tab === 'data'}
  <div class="data-card">
    <div class="path-field">
      <label for="install-dir">安装目录</label>
      <div class="path-input">
        <input id="install-dir" bind:value={installInput} spellcheck="false" placeholder="D:\Pawch Agent" />
        <button class="icon-btn" title="选择目录" aria-label="选择目录" on:click={() => pick('install')}>
          <svg class="ic" viewBox="0 0 24 24" width="16" height="16" aria-hidden="true"><path d="M3 7.5A1.5 1.5 0 0 1 4.5 6h4.6l2 2h8.4A1.5 1.5 0 0 1 21 9.5v8A1.5 1.5 0 0 1 19.5 19h-15A1.5 1.5 0 0 1 3 17.5v-10z" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round" /></svg>
        </button>
        <button class="open-btn" title="打开目录" on:click={() => openDir('install')}>打开</button>
      </div>
    </div>

    <div class="path-field">
      <label for="data-dir">数据目录</label>
      <div class="path-input">
        <input id="data-dir" bind:value={dataInput} spellcheck="false" placeholder="D:\pawch-data" />
        <button class="icon-btn" title="选择目录" aria-label="选择目录" on:click={() => pick('data')}>
          <svg class="ic" viewBox="0 0 24 24" width="16" height="16" aria-hidden="true"><path d="M3 7.5A1.5 1.5 0 0 1 4.5 6h4.6l2 2h8.4A1.5 1.5 0 0 1 21 9.5v8A1.5 1.5 0 0 1 19.5 19h-15A1.5 1.5 0 0 1 3 17.5v-10z" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round" /></svg>
        </button>
        <button class="open-btn" title="打开目录" on:click={() => openDir('data')}>打开</button>
      </div>
    </div>

    <div class="data-actions">
      <button class="btn primary" disabled={!canMigrate} on:click={doMigrate}>
        {busy === 'migrate' ? '迁移中…' : '迁移'}
      </button>
      <label class="cache-opt"><input type="checkbox" bind:checked={withCache} /> 连缓存一起</label>
      {#if info}
        <span class="size-note">数据 {fmtBytes(info.data_bytes)} · 缓存 {fmtBytes(info.cache_bytes)}</span>
      {/if}
    </div>
    {#if busy === 'migrate'}
      <div class="param-hint">正在复制目录，完成后会自动重启后端…</div>
    {/if}
  </div>

  <div class="data-card">
    <div class="field-title">导出 / 导入</div>
    <div class="data-actions">
      <button class="btn" disabled={!!busy} on:click={doExport}>
        {busy === 'export' ? '导出中…' : '导出数据 zip'}
      </button>
      <button class="btn" disabled={!!busy} on:click={doImport}>
        {busy === 'import' ? `导入中… ${pct}%` : '从 zip 导入'}
      </button>
    </div>
    {#if busy === 'import'}
      <div class="prog"><div class="prog-fill" style={`width:${pct}%`}></div></div>
      <div class="param-hint">
        已解压 {fmtBytes(progress?.bytes || 0)} / {fmtBytes(progress?.total_bytes || 0)}（{progress?.files || 0} / {progress?.total_files || 0} 个文件）
      </div>
    {/if}
  </div>

  {#if msg}<div class="data-msg">{msg}</div>{/if}
  {#if err}<div class="data-err">{err}</div>{/if}

  {#if cleanup}
    <div class="modal-backdrop" on:click|self={() => finishCleanup(false)}>
      <div class="modal cleanup-modal" on:click|stopPropagation>
        <div class="modal-head"><b>{cleanup.kind === 'import' ? '导入完成' : '迁移完成'}</b></div>
        <div class="cleanup-text">
          {cleanup.kind === 'import' ? '旧数据已备份到：' : '旧目录：'}
          {#if cleanup.install}<div class="old-path">旧安装目录：{cleanup.install}</div>{/if}
          {#if cleanup.data}
            <div class="old-path">{cleanup.kind === 'import' ? '旧数据备份：' : '旧数据目录：'}{cleanup.data}</div>
          {/if}
          <div class="cleanup-note">点「删除」会立刻删；正被程序占用的文件会在应用自动重启后删掉。</div>
        </div>
        <div class="cleanup-actions">
          <button class="btn" on:click={() => finishCleanup(false)}>我自己删</button>
          <button class="btn primary" on:click={() => finishCleanup(true)}>删除</button>
        </div>
      </div>
    </div>
  {/if}
{/if}

<style>
  .data-card {
    border: 1px solid var(--line);
    border-radius: 12px;
    padding: 14px 16px;
    margin-bottom: 14px;
    background: var(--bg1);
  }
  .path-field {
    margin-bottom: 12px;
  }
  .path-field label {
    display: block;
    font-size: 13px;
    color: var(--g5);
    margin-bottom: 6px;
  }
  .path-input {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .path-input input {
    flex: 1;
    min-width: 0;
    height: 34px;
    padding: 0 10px;
    font-size: 13px;
    color: var(--g6);
    background: var(--bg2);
    border: 1px solid var(--line);
    border-radius: 8px;
    outline: none;
  }
  .path-input input:focus {
    border-color: var(--brand);
  }
  .icon-btn {
    width: 34px;
    height: 34px;
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-size: 15px;
    color: var(--g5);
    background: var(--bg2);
    border: 1px solid var(--line);
    border-radius: 8px;
    cursor: pointer;
  }
  .icon-btn .ic {
    display: block;
  }
  .icon-btn:hover {
    color: var(--brand-text);
    border-color: var(--brand);
  }
  .open-btn {
    height: 34px;
    padding: 0 12px;
    flex: none;
    font-size: 12.5px;
    color: var(--g5);
    background: var(--bg2);
    border: 1px solid var(--line);
    border-radius: 8px;
    cursor: pointer;
  }
  .open-btn:hover {
    color: var(--brand-text);
    border-color: var(--brand);
  }
  .field-title {
    font-size: 13px;
    color: var(--g5);
    margin-bottom: 10px;
  }
  .data-actions {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }
  .size-note {
    font-size: 12px;
    color: var(--g4);
  }
  .prog {
    margin-top: 10px;
    height: 6px;
    border-radius: 4px;
    background: var(--g2);
    border: 1px solid var(--line);
    overflow: hidden;
  }
  .prog-fill {
    height: 100%;
    background: linear-gradient(90deg, var(--brand), var(--brand-deep));
    transition: width 0.3s ease;
  }
  .cache-opt {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    color: var(--g5);
  }
  .data-msg {
    margin-top: 6px;
    font-size: 13px;
    line-height: 1.6;
    color: var(--g5);
    overflow-wrap: anywhere;
  }
  .data-err {
    margin-top: 6px;
    font-size: 13px;
    line-height: 1.6;
    color: var(--danger, #d9534f);
    overflow-wrap: anywhere;
  }
  .cleanup-modal {
    min-width: 420px;
    max-width: 560px;
  }
  .cleanup-text {
    font-size: 13.5px;
    line-height: 1.7;
    color: var(--g6);
  }
  .old-path {
    margin-top: 6px;
    padding: 6px 8px;
    border-radius: 6px;
    background: var(--bg2);
    border: 1px solid var(--line);
    overflow-wrap: anywhere;
  }
  .cleanup-note {
    margin-top: 10px;
    font-size: 12px;
    color: var(--g4);
  }
  .cleanup-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 16px;
  }
</style>
