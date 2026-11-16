<script>
  // 右侧面板「插件」dock 标签（2026-11-15 阶段 A-4，自 App.svelte 逐字平移）。
  // 插件桥（extFrameEls/dockPaneFrames 登记、消息转发、CSS 注入）留根——
  // iframe 元素经 bind:frame 上行登记进根上的 dockPaneFrames，load 事件上行
  // 走根上的 onExtFrameLoad（桥在那边做就绪握手）。
  import { createEventDispatcher } from 'svelte';
  const dispatch = createEventDispatcher();

  export let title = '';
  export let src = '';
  export let ready = false; // enabledExts 里还有这个插件（停用/未加载走空态）
  export let placeholder = ''; // 空态文案（根上按 extList/extMsg 算）
  export let frame = null; // bind 双向到根：iframe 元素 → dockPaneFrames[name]
</script>

<div class="web-wrap">
  {#if ready}
    <iframe
      class="ext-frame"
      {title}
      sandbox="allow-scripts"
      bind:this={frame}
      {src}
      on:load={() => dispatch('load')}
    ></iframe>
  {:else}
    <div class="rp-empty">{placeholder}</div>
  {/if}
</div>

<style>
  /* 与 App.svelte 同名类保持同步：.web-wrap/.ext-frame 是网页面板与 ext 面板
     共用类，那边原件还在；.rp-empty 同 ArtifactsPane 的做法照抄一份。 */
  .web-wrap {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .ext-frame {
    flex: 1;
    min-height: 0;
    width: 100%;
    border: none;
    /* 透明：插件把 body 背景改成全透明时，让外层面板的底色/壁纸透上来，
       而不是被 iframe 元素自己那层实色挡住 */
    background: transparent;
  }
  .rp-empty {
    margin: auto;
    text-align: center;
    color: var(--g5);
    font-size: 12.5px;
    line-height: 1.7;
    padding: 0 16px;
  }
</style>
