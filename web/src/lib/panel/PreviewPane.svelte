<script>
  // 右侧面板「文件预览」标签（2026-11-14 阶段 A-2，自 App.svelte 逐字平移）。
  // 预览内容/编辑态由根的 rpPreviewCache 持有，这里按 entry 渲染 + 上行动作事件；
  // 编辑/保存/刷新的写逻辑都在根（rpSet/savePreview/cancelEditPreview/startEditPreview）。
  import { createEventDispatcher } from 'svelte';
  import { previewKind, previewTruncated, extOf } from './preview.js';
  const dispatch = createEventDispatcher();

  export let path = ''; // 工作区相对路径（=预览标签的 t.path）
  export let entry = {}; // rpPreviewCache[path] || {}：{ loading, error, content, editing, draft, editNote, saving, savedAt }

  const MEDIA_VIDEO = ['mp4', 'm4v', 'webm', 'mov'];
  const rawSrc = (p) => `/api/workspace/raw?path=${encodeURIComponent(p)}`;
</script>

<div class="rp-preview-head">
  <span class="rp-preview-path" title={path}>{path}</span>
  <span class="flex-sp"></span>
  {#if entry.savedAt}
    <span class="rp-saved">已保存 {entry.savedAt}</span>
  {/if}
  {#if entry.editing}
    <button class="rp-btn" title="保存（写回工作区文件）" disabled={entry.saving} on:click={() => dispatch('save', path)}>
      {entry.saving ? '保存中…' : '保存'}
    </button>
    <button class="rp-btn" title="取消编辑（丢弃改动）" on:click={() => dispatch('canceledit', path)}>取消</button>
  {:else if previewKind(path) === 'text' && entry.content && !previewTruncated(entry.content)}
    <button class="rp-btn" title="编辑内容" on:click={() => dispatch('startedit', path)}>编辑</button>
  {/if}
  <button class="rp-refresh" title="重新读取" on:click={() => dispatch('refresh', path)}>↻</button>
</div>
{#if entry.editing}
  {#if entry.editNote}
    <div class="rp-edit-note">{entry.editNote}</div>
  {/if}
  <textarea
    class="rp-edit"
    spellcheck="false"
    value={entry.draft || ''}
    on:input={(e) => dispatch('draft', e.currentTarget.value)}
  ></textarea>
{:else if previewKind(path) === 'image'}
  <div class="rp-image-wrap">
    <img class="rp-image" src={rawSrc(path)} alt={path} />
  </div>
{:else if previewKind(path) === 'pdf'}
  <!-- PDF 内嵌（WebView 自带查看器；#toolbar=0 藏掉查看器工具条，纯预览） -->
  <iframe class="rp-pdf" title={path} src={`${rawSrc(path)}#toolbar=0`}></iframe>
{:else if previewKind(path) === 'docx'}
  <!-- docx：mammoth 排版直出，纯预览（工具条也去，无缩放/打印） -->
  {#if entry.loading}
    <pre class="rp-preview">渲染中…</pre>
  {:else if entry.error}
    <pre class="rp-preview">{`渲染失败: ${entry.error}`}</pre>
  {:else}
    <iframe
      class="rp-pdf rp-doc-paper"
      title={path}
      sandbox=""
      srcdoc={`<!doctype html><html><head><meta charset="utf-8"><style>body{font-family:'Segoe UI','Microsoft YaHei',sans-serif;padding:28px;margin:0;line-height:1.75;color:#24292f;background:#fff}img{max-width:100%}table{border-collapse:collapse;width:100%}td,th{border:1px solid #999;padding:6px 10px}h1,h2,h3{line-height:1.4}blockquote{border-left:3px solid #999;margin:0;padding-left:12px;color:#666}pre{background:#f3f4f6;padding:10px;border-radius:6px}</style></head><body>${(entry.content || '').replace(/<script/gi, '&lt;script')}</body></html>`}
    ></iframe>
  {/if}
{:else if previewKind(path) === 'xlsx'}
  <!-- xlsx：SheetJS 表格直出，纯预览（组件全去，无工具条/缩放/表名标注） -->
  {#if entry.loading}
    <pre class="rp-preview">渲染中…</pre>
  {:else if entry.error}
    <pre class="rp-preview">{`渲染失败: ${entry.error}`}</pre>
  {:else}
    <div class="rp-xlsx">
      {#each (() => { try { return JSON.parse(entry.content || '[]'); } catch { return []; } })() as sh}
        <div class="rp-xlsx-sheet">
          {#if sh.name}<div class="rp-xlsx-name">{sh.name}</div>{/if}
          <div class="rp-xlsx-body">
            <table>
              {#each sh.rows as row}<tr>{#each row as cell}<td>{cell}</td>{/each}</tr>{/each}
            </table>
          </div>
        </div>
      {/each}
    </div>
  {/if}
{:else if previewKind(path) === 'pptx'}
  <!-- pptx：后端分页文本直出，纯预览（组件全去，无工具条/缩放/页码） -->
  {#if entry.loading}
    <pre class="rp-preview">渲染中…</pre>
  {:else if entry.error}
    <pre class="rp-preview">{`渲染失败: ${entry.error}`}</pre>
  {:else}
    <div class="rp-pptx">
      {#each (() => { try { return JSON.parse(entry.content || '[]'); } catch { return []; } })() as sl}
        <div class="rp-slide"><pre class="rp-slide-text">{sl.text || '（空白页）'}</pre></div>
      {/each}
    </div>
  {/if}
{:else if previewKind(path) === 'media'}
  <div class="rp-media-wrap">
    {#if MEDIA_VIDEO.includes(extOf(path))}
      <video class="rp-media" controls autoplay={false} src={rawSrc(path)}>
        <track kind="captions" />
      </video>
    {:else}
      <audio class="rp-media rp-audio" controls src={rawSrc(path)}></audio>
    {/if}
  </div>
{:else if entry.loading}
  <pre class="rp-preview">读取中…</pre>
{:else if entry.error}
  <pre class="rp-preview">{`读取失败: ${entry.error}`}</pre>
{:else}
  {#if previewKind(path) === 'doc'}
    <div class="rp-note">文档类预览：这里是抽取出来的文本（排版与图表不在其中），不提供编辑</div>
  {:else if previewTruncated(entry.content)}
    <div class="rp-note">文件较大，预览是头尾压缩过的（中间省略）——为避免把省略部分写没，不提供编辑</div>
  {/if}
  <pre class="rp-preview">{entry.content || '（空文件）'}</pre>
{/if}

<style>
  /* 预览面板样式：自 App.svelte 逐字平移（原件已删，含 .rp-note/.rp-preview-path 两处
     同名级联副本）。.rp-refresh/.flex-sp 为多面板共享类，App.svelte 原件仍在，这里留副本。
     a.rp-btn（<a> 下载钮）在源里本就是死样式（构建既有 unused 告警），未随迁。 */
  .rp-preview-head {
    position: sticky;
    top: -8px;
    z-index: 2;
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
    margin: -8px -8px 6px;
    padding: 6px 6px 6px 10px;
    background: var(--g0);
    border-bottom: 1px solid var(--g3);
  }
  .rp-preview-head .rp-preview-path {
    position: static;
    top: auto;
    z-index: auto;
    margin: 0;
    padding: 0;
    flex: 1;
    min-width: 0;
    background: transparent;
    border: none;
    border-radius: 0;
  }
  .rp-preview-path {
    position: sticky;
    top: -8px; /* 抵消 .rp-body 的 8px padding，贴住面板上沿 */
    z-index: 2;
    flex: none;
    margin: -8px -8px 6px;
    padding: 8px 10px;
    background: var(--g1);
    border-bottom: 1px solid var(--g2);
    color: var(--g5);
    font-family: Consolas, monospace;
    font-size: 11.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .rp-btn {
    flex: none;
    padding: 2px 10px;
    border-radius: 6px;
    border: 1.5px solid var(--g3);
    background: transparent;
    color: var(--g6);
    font-size: 12px;
    cursor: pointer;
  }
  .rp-btn:hover {
    color: var(--g7);
    border-color: var(--g4);
  }
  .rp-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .rp-refresh {
    background: transparent;
    border: none;
    color: var(--g5);
    font-size: 15px;
    cursor: pointer;
    padding: 2px 8px;
    border-radius: 5px;
  }
  .rp-refresh:hover {
    color: var(--g7);
    background: var(--g2);
  }
  .flex-sp {
    flex: 1;
  }
  .rp-saved {
    flex: none;
    font-size: 11.5px;
    color: var(--tint-green-fg, #7fd6a4);
  }
  .rp-note {
    flex: none;
    margin: 0 0 6px;
    padding: 6px 10px;
    border-radius: 6px;
    background: var(--tint-yellow-bg, #3a3417);
    color: var(--tint-yellow-fg, #e8d47f);
    font-size: 12px;
  }
  .rp-note {
    color: var(--g5);
    font-size: 11.5px;
    padding: 4px 10px 6px;
    border-bottom: 1px solid var(--g2);
    margin-bottom: 4px;
  }
  .rp-edit-note {
    flex: none;
    margin: 0 0 6px;
    padding: 6px 9px;
    border-radius: 6px;
    border: 1px solid color-mix(in srgb, var(--tint-yellow-fg) 40%, transparent);
    background: var(--tint-yellow-bg);
    color: var(--tint-yellow-fg);
    font-size: 12px;
    line-height: 1.5;
  }
  .rp-edit {
    flex: 1;
    min-height: 0;
    margin: 0;
    padding: 8px;
    border: 1px solid var(--g3);
    border-radius: 8px;
    background: var(--g1);
    color: var(--g6);
    font: 12.5px/1.55 Consolas, "Cascadia Code", monospace;
    resize: none;
    outline: none;
  }
  .rp-edit:focus {
    border-color: var(--g6);
  }
  .rp-image-wrap {
    flex: 1;
    min-height: 0;
    display: grid;
    place-items: center;
    overflow: auto;
    padding: 6px;
  }
  .rp-image {
    max-width: 100%;
    max-height: 100%;
    border-radius: 6px;
    background: var(--g1);
  }
  /* PDF 内嵌查看器与音视频播放器 */
  .rp-pdf {
    flex: 1;
    min-height: 0;
    width: 100%;
    border: none;
    background: var(--g1);
  }
  /* docx 纸张预览：细边框把白纸和深色界面隔开 */
  .rp-doc-paper {
    border: 1px solid var(--g3);
    border-radius: 6px;
  }
  .rp-preview {
    margin: 0;
    padding: 8px;
    background: var(--g1);
    border: 1px solid var(--g3);
    border-radius: 8px;
    color: var(--g6);
    font-family: Consolas, monospace;
    font-size: 12px;
    /* 代码按词换行，不从单词中间劈开（原来 break-all 会把 SECURITY 拆成两行） */
    white-space: pre-wrap;
    word-break: normal;
    overflow-wrap: break-word;
    tab-size: 4;
  }
  /* xlsx 表格渲染区 */
  .rp-xlsx {
    flex: 1;
    min-height: 0;
    overflow: auto;
    background: #fff;
    color: #24292f;
  }
  .rp-xlsx-sheet { padding: 12px 16px 20px; }
  .rp-xlsx-body :global(table) { border-collapse: collapse; }
  .rp-xlsx-body :global(td), .rp-xlsx-body :global(th) { border: 1px solid #c8ccd4; padding: 3px 8px; font-size: 12px; white-space: nowrap; }
  .rp-xlsx-body :global(tr:first-child) { background: #f1f3f6; font-weight: 600; }
  /* pptx 分页渲染区 */
  .rp-pptx { flex: 1; min-height: 0; overflow: auto; display: flex; flex-direction: column; gap: 14px; padding: 14px; background: var(--g1); }
  .rp-slide { background: #fff; color: #24292f; border-radius: 8px; padding: 26px 28px; min-height: 160px; box-shadow: 0 1px 4px var(--overlay); }
  .rp-slide-text { margin: 0; white-space: pre-wrap; font-size: 13px; line-height: 1.7; font-family: inherit; }
  .rp-media-wrap {
    flex: 1;
    min-height: 0;
    display: grid;
    place-items: center;
    padding: 10px;
  }
  .rp-media {
    max-width: 100%;
    max-height: 100%;
    border-radius: 8px;
    background: var(--g1);
  }
  .rp-audio {
    width: 100%;
  }
</style>
