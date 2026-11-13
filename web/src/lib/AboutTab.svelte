<script>
  import { onDestroy } from 'svelte';

  export let tab;
  export let version = '';

  let checking = false;
  let info = null;
  let err = '';
  let msg = '';
  let dl = null;
  let timer = 0;
  let installing = false;

  const GITHUB_URL = 'https://github.com/pumu21600-boop/Pawch-Agent';

  // 壳里走系统默认浏览器（webview 自己的新窗口会被 on_new_window 拒掉）；
  // 普通浏览器打开时保留 target=_blank 的原生行为。
  function openExternal(e, url) {
    if (window.agentWinApi && window.agentWinApi.openExternal) {
      e.preventDefault();
      window.agentWinApi.openExternal(url);
    }
  }

  const fmtBytes = (n) => {
    const v = Number(n) || 0;
    if (v >= 1024 * 1024) return `${(v / 1024 / 1024).toFixed(1)} MB`;
    if (v >= 1024) return `${(v / 1024).toFixed(0)} KB`;
    return `${v} B`;
  };
  // 必须用响应式声明：模板里的 {pct()} 是函数调用，编译器只看得到函数名、
  // 看不到函数体里的 dl，dl 轮询更新时百分比会一直停在上一次的值（实测卡 0%）
  let pct = 0;
  $: pct = dl && dl.total ? Math.min(100, Math.round((dl.bytes / dl.total) * 100)) : 0;

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

  async function check() {
    checking = true;
    err = '';
    msg = '';
    try {
      info = await api('/api/update/check');
    } catch (e) {
      err = e.message || String(e);
    } finally {
      checking = false;
    }
  }

  function stopPoll() {
    if (timer) {
      clearInterval(timer);
      timer = null;
    }
  }
  async function poll() {
    try {
      dl = await api('/api/update/progress');
      if (!dl.running) stopPoll();
    } catch {}
  }
  async function download() {
    err = '';
    msg = '';
    try {
      await api('/api/update/download', {});
      dl = { running: true, bytes: 0, total: info?.asset_size || 0 };
      stopPoll();
      timer = setInterval(poll, 500);
    } catch (e) {
      err = e.message || String(e);
    }
  }
  async function install() {
    err = '';
    try {
      await api('/api/update/install', {});
      installing = true;
      msg = '正在安装，应用会自动重启…';
    } catch (e) {
      err = e.message || String(e);
    }
  }

  onDestroy(stopPoll);
</script>

{#if tab === 'about'}
  <div class="about-card">
    <div class="brand">
      <img class="app-icon" src="/pawch-icon.svg" alt="Pawch Agent" />
      <div class="brand-text">
        <div class="app-name">Pawch Agent</div>
        <div class="app-ver">当前版本 {version || '—'}</div>
      </div>
    </div>

    <div class="field">
      <div class="field-label">项目链接</div>
      <a
        class="url-row"
        href={GITHUB_URL}
        target="_blank"
        rel="noreferrer"
        title="在默认浏览器中打开"
        on:click={(e) => openExternal(e, GITHUB_URL)}
      >
        <span class="url-text">{GITHUB_URL}</span>
        <svg class="url-ext" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M14 4h6v6"/><path d="M20 4l-8.5 8.5"/><path d="M18 13.5V19a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5.5"/></svg>
      </a>
    </div>

    <div class="field">
      <div class="field-label">版本更新</div>
      <div class="acts">
        <button class="btn primary" disabled={checking || installing} on:click={check}>
          {checking ? '检查中…' : '检查更新'}
        </button>
        {#if info && !info.has_update}<span class="hint">已是最新版本</span>{/if}
      </div>

      {#if info && info.has_update}
        <div class="upd">
          <div class="upd-title">
            发现新版本 <b>v{info.latest}</b>
            {#if info.via_mirror}<span class="hint">（走镜像检查）</span>{/if}
          </div>
          {#if info.notes}<pre class="notes">{info.notes}</pre>{/if}
          {#if !dl || (!dl.running && !dl.done)}
            <div class="acts">
              <button class="btn primary" on:click={download}>下载更新</button>
              <a class="link" href={info.page_url} target="_blank" rel="noreferrer" on:click={(e) => openExternal(e, info.page_url)}>查看发布页</a>
            </div>
          {/if}
          {#if dl && (dl.running || dl.done)}
            <div class="prog"><div class="prog-fill" style={`width:${pct}%`}></div></div>
            <div class="hint">
              已下载 {fmtBytes(dl.bytes)} / {fmtBytes(dl.total)}（{pct}%）
              {#if dl.used_mirror} · 镜像下载{/if}
              {#if dl.done} · SHA256 校验通过{/if}
            </div>
          {/if}
          {#if dl && dl.done && !installing}
            <div class="acts">
              <button class="btn primary" on:click={install}>安装并重启</button>
              <a class="link" href={info.page_url} target="_blank" rel="noreferrer" on:click={(e) => openExternal(e, info.page_url)}>查看发布页</a>
            </div>
          {/if}
          {#if dl && dl.error}<div class="err">{dl.error}</div>{/if}
        </div>
      {/if}
    </div>

    {#if msg}<div class="hint">{msg}</div>{/if}
    {#if err}<div class="err">{err}</div>{/if}
  </div>
{/if}

<style>
  .about-card {
    width: 100%;
    max-width: 620px;
    margin: 0 auto;
    display: flex;
    flex-direction: column;
    gap: 22px;
  }
  /* 字段标题口径同设置页其他分区（.field label：12px/600/--g6） */
  .field-label {
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.02em;
    color: var(--g6);
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 4px 0 2px;
  }
  .app-icon {
    width: 56px;
    height: 56px;
    border-radius: 14px;
    flex: none;
    display: block;
  }
  .app-name {
    font-size: 19px;
    font-weight: 650;
    line-height: 1.3;
    letter-spacing: 0.2px;
  }
  .app-ver {
    margin-top: 3px;
    font-size: 13px;
    color: var(--g5);
  }
  .url-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    border: 1.5px solid var(--g3);
    border-radius: 10px;
    color: var(--g7);
    text-decoration: none;
    transition: border-color 0.15s ease;
  }
  .url-row:hover {
    border-color: var(--g5);
  }
  .url-text {
    flex: 1;
    min-width: 0;
    font-size: 13px;
    line-height: 1.45;
    word-break: break-all;
  }
  .url-ext {
    width: 14px;
    height: 14px;
    flex: none;
    color: var(--g5);
  }
  .url-row:hover .url-ext {
    color: var(--g7);
  }
  .acts { display: flex; align-items: center; gap: 12px; flex-wrap: wrap; }
  .upd {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px;
    border: 1px solid var(--g3);
    border-radius: 10px;
  }
  .upd-title b { color: var(--brand-text); font-weight: 600; }
  .notes {
    margin: 0;
    max-height: 220px;
    overflow: auto;
    white-space: pre-wrap;
    word-break: break-word;
    font: inherit;
    line-height: 1.55;
    color: var(--g6);
  }
  .prog {
    height: 6px;
    border-radius: 999px;
    background: var(--g3);
    overflow: hidden;
  }
  .prog-fill {
    height: 100%;
    background: var(--brand);
    transition: width 0.2s ease;
  }
  .hint { color: var(--g6); }
  .link {
    color: var(--brand-text);
    text-decoration: none;
  }
  .link:hover { text-decoration: underline; text-underline-offset: 3px; }
  .err { color: var(--danger, #d33); }
</style>
