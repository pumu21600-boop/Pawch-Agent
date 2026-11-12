<script>
  // 自绘下拉：弹出层是页面元素（不再是系统原生弹层），因此完全跟随主题、可被「界面微调」右键改样式。
  // 菜单用 position:fixed 按按钮位置定位，避免被设置弹窗的滚动容器裁掉。
  import { onMount, onDestroy, createEventDispatcher, tick } from 'svelte';

  export let value = null;
  export let options = []; // [{ value, label, hint? }]
  export let placeholder = '请选择';
  export let title = '';
  export let disabled = false;

  const dispatch = createEventDispatcher();
  let open = false;
  let btnEl;
  let menuEl;
  let menuStyle = '';
  let hlIndex = -1;

  // 值比较：兼容 null/undefined/'' 互等与字符串化比较（贴近原生 select 的老行为）
  function same(a, b) {
    if (a === b) return true;
    const an = a == null || a === '';
    const bn = b == null || b === '';
    if (an || bn) return an && bn;
    return String(a) === String(b);
  }

  // 2026-11-01 分组下拉：options 里可以混入 { group, items: [{value,label}] }——
  // 默认收起，点渠道名才展开它的模型（功能桥接要按渠道分组）。普通项照旧。
  let expanded = {};
  $: allOpts = options.flatMap((o) => (o && o.group ? o.items : [o]));
  $: current = allOpts.find((o) => same(o.value, value)) || null;
  // 可见行：分组头 + （展开时的）组内项。键盘导航与高亮都按这个列表走。
  $: rows = options.flatMap((o) => {
    if (o && o.group) {
      const out = [{ group: o.group, label: o.group, open: !!expanded[o.group], cur: o.items.some((it) => same(it.value, value)) }];
      if (expanded[o.group]) {
        for (const it of o.items) out.push({ item: it, nested: true });
      }
      return out;
    }
    return [{ item: o }];
  });
  // 打开菜单时把高亮落在当前值那一行（分组会先被展开）
  $: if (open) hlIndex = rows.findIndex((r) => r.item && same(r.item.value, value));
  // 按钮上的显示文案：分组项显示成「渠道 · 模型」，菜单里靠分组头给上下文
  $: currentText = (() => {
    if (!current) return placeholder;
    const g = options.find((o) => o && o.group && o.items.includes(current));
    return g ? `${g.group} · ${current.label}` : current.label;
  })();

  function place() {
    if (!btnEl) return;
    const r = btnEl.getBoundingClientRect();
    const below = window.innerHeight - r.bottom - 12;
    const above = r.top - 12;
    const useUp = below < 140 && above > below;
    const maxH = Math.max(120, Math.min(300, useUp ? above : below));
    menuStyle = useUp
      ? `left:${Math.round(r.left)}px; bottom:${Math.round(window.innerHeight - r.top + 4)}px; width:${Math.round(r.width)}px; max-height:${Math.round(maxH)}px;`
      : `left:${Math.round(r.left)}px; top:${Math.round(r.bottom + 4)}px; width:${Math.round(r.width)}px; max-height:${Math.round(maxH)}px;`;
  }

  async function toggle() {
    if (disabled) return;
    open = !open;
    if (open) {
      // 当前值落在某个分组里就先把那组展开，否则用户看不到自己选的是哪条
      const g = options.find((o) => o && o.group && o.items.some((it) => same(it.value, value)));
      if (g && !expanded[g.group]) expanded = { ...expanded, [g.group]: true };
      await tick();
      place();
    }
  }

  function pick(o) {
    value = o.value;
    open = false;
    dispatch('change', o.value);
  }

  // 微调面板（右键属性面板）里的操作不算"点到外面"：拖动滑杆/取色时保持菜单展开，
  // 方便一边调数值一边看菜单上的实时效果
  function inMicroPanel(t) {
    try {
      return !!(t && t.closest && t.closest('.micro-panel'));
    } catch (e) {
      return false;
    }
  }

  function onDocDown(e) {
    if (!open) return;
    if (inMicroPanel(e.target)) return;
    if (btnEl && btnEl.contains(e.target)) return;
    if (menuEl && menuEl.contains(e.target)) return;
    open = false;
  }

  function onKey(e) {
    if (e.key === 'Escape') {
      open = false;
      return;
    }
    if (!open) {
      if (e.key === 'Enter' || e.key === ' ' || e.key === 'ArrowDown') {
        e.preventDefault();
        toggle();
      }
      return;
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      hlIndex = Math.min(rows.length - 1, hlIndex + 1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      hlIndex = Math.max(0, hlIndex - 1);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      // 没高亮项时 Enter 关菜单——原来无反馈不关，键盘只剩 Esc 可用（F15）
      const r = rows[hlIndex];
      if (!r) open = false;
      else if (r.item) pick(r.item);
      else expanded = { ...expanded, [r.group]: !expanded[r.group] };
    } else if (e.key === 'Tab') {
      open = false;
    }
  }

  function closeOnViewportChange(e) {
    if (!open) return;
    // 微调面板开着时，实时预览会改动样式/布局并触发滚动，此时不能把菜单收掉；
    // 其余情况下滚动/缩放照旧关闭，避免菜单脱离按钮漂在半空
    if (typeof document !== 'undefined' && document.querySelector('.micro-panel')) return;
    if (e && e.type === 'scroll' && inMicroPanel(e.target)) return;
    open = false;
  }

  // 微调面板每次改值都会广播 micro-preview：菜单跟着按钮重新贴合定位。
  // 但如果用户正在微调的正是菜单自身，跳过重定位——否则会把它手动改的宽高/最大高度顶掉
  function onMicroPreview(e) {
    if (!open) return;
    const sel = e && e.detail && e.detail.sel;
    if (sel && menuEl) {
      try {
        if (menuEl.matches(sel)) return;
      } catch (err) {}
    }
    place();
  }

  onMount(() => {
    document.addEventListener('mousedown', onDocDown, true);
    window.addEventListener('resize', closeOnViewportChange);
    window.addEventListener('scroll', closeOnViewportChange, true);
    window.addEventListener('micro-preview', onMicroPreview);
  });
  onDestroy(() => {
    document.removeEventListener('mousedown', onDocDown, true);
    window.removeEventListener('resize', closeOnViewportChange);
    window.removeEventListener('scroll', closeOnViewportChange, true);
    // 2026-10-08 修：micro-preview 漏卸——每次挂载/销毁泄漏一个监听器 + 整个组件闭包
    // （设置弹窗里 ~10 个 Select，反复开合设置就累积一批，微调面板每次拖动都在跑）
    window.removeEventListener('micro-preview', onMicroPreview);
  });
</script>

<div class="csel-wrap">
  <button
    type="button"
    class="csel"
    class:open
    bind:this={btnEl}
    {title}
    {disabled}
    on:click={toggle}
    on:keydown={onKey}
  >
    <span class="csel-label" class:placeholder={!current}>{currentText}</span>
    <svg class="csel-caret" class:open viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6" /></svg>
  </button>
</div>

{#if open}
  <div class="csel-menu" bind:this={menuEl} style={menuStyle}>
    {#each rows as r, i}
      {#if r.item}
        <button
          type="button"
          class="csel-item"
          class:nested={r.nested}
          class:active={same(r.item.value, value)}
          class:hl={i === hlIndex}
          on:click={() => pick(r.item)}
        >
          <span class="csel-item-label">{r.item.label}</span>
          {#if r.item.hint}<span class="csel-item-hint">{r.item.hint}</span>{/if}
        </button>
      {:else}
        <button
          type="button"
          class="csel-group"
          class:open={r.open}
          class:cur={r.cur}
          class:hl={i === hlIndex}
          on:click={() => (expanded = { ...expanded, [r.group]: !expanded[r.group] })}
        >
          <span class="csel-group-label">{r.label}{r.cur ? ' · 当前' : ''}</span>
          <svg class="csel-group-caret" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m9 6 6 6-6 6" /></svg>
        </button>
      {/if}
    {/each}
    {#if !options.length}
      <div class="csel-empty">无可选项</div>
    {/if}
  </div>
{/if}

<style>
  .csel-wrap {
    display: block;
    width: 100%;
    min-width: 0;
  }
  .csel {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    width: 100%;
    box-sizing: border-box;
    padding: 8px 10px;
    border-radius: 8px;
    border: 1.5px solid var(--g3);
    /* 描边无底色（2026-11-11 设置弹窗统一样式）：Select 只在设置各页签用 */
    background: transparent;
    color: var(--g7);
    font-size: 13px;
    font-family: inherit;
    text-align: left;
    cursor: pointer;
  }
  .csel:hover {
    border-color: var(--g5);
  }
  .csel.open,
  .csel:focus-visible {
    outline: none;
    border-color: var(--g6);
  }
  .csel:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .csel-label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .csel-label.placeholder {
    color: var(--g5);
  }
  .csel-caret {
    width: 13px;
    height: 13px;
    flex: none;
    color: var(--g5);
    transition: transform 0.15s;
  }
  .csel-caret.open {
    transform: rotate(180deg);
  }
  /* 弹出菜单：fixed 定位（由脚本写入 left/top/width/max-height），不受弹窗滚动容器裁剪 */
  .csel-menu {
    position: fixed;
    z-index: 120;
    overflow-y: auto;
    padding: 4px;
    /* 浮层要盖住正文：比面板更实的玻璃 */
    background: var(--glass-float);
    border: 1px solid var(--g3);
    border-radius: 10px;
    box-shadow: 0 10px 30px var(--overlay);
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .csel-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    box-sizing: border-box;
    padding: 7px 10px;
    border: none;
    border-radius: 7px;
    background: transparent;
    color: var(--g6);
    font-size: 13px;
    font-family: inherit;
    text-align: left;
    cursor: pointer;
  }
  .csel-item.hl {
    /* 菜单底子已是 g2 系玻璃：高亮行要比底子深/浅一档（g3），否则 hover 糊进底色 */
    background: var(--g3);
    color: var(--g7);
  }
  .csel-item.active {
    /* 当前项：统一选中口径（文字高亮=橙字；底色模式铺 --sel-bg） */
    background: var(--sel-bg);
    color: var(--sel-fg);
    font-weight: 600;
  }
  .csel-item-label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .csel-item-hint {
    flex: none;
    font-size: 11px;
    color: var(--g5);
  }
  .csel-empty {
    padding: 8px 10px;
    color: var(--g5);
    font-size: 12px;
  }
  /* 分组下拉：渠道名一行，点开才列出它的模型（2026-11-01 功能桥接）。
     样式对齐输入框旁的模型快选菜单：文字在左、箭头在右、当前值所在的组标「· 当前」 */
  .csel-group {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
    width: 100%;
    box-sizing: border-box;
    padding: 7px 10px;
    border: none;
    border-radius: 7px;
    background: transparent;
    color: var(--g6);
    font-size: 13px;
    font-weight: 650;
    font-family: inherit;
    text-align: left;
    cursor: pointer;
  }
  .csel-group.hl,
  .csel-group:hover {
    background: var(--g3);
    color: var(--g7);
  }
  .csel-group.cur {
    color: var(--g7);
  }
  .csel-group-caret {
    width: 13px;
    height: 13px;
    flex: none;
    color: var(--g5);
    transition: transform 0.15s;
  }
  .csel-group.open .csel-group-caret {
    transform: rotate(90deg);
  }
  .csel-group-label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .csel-item.nested {
    padding-left: 30px;
  }
</style>
