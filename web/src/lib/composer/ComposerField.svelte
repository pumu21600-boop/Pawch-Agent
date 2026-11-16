<script>
  // 输入区（引用高亮浮层 + textarea）（2026-11-15 阶段 B，自 App.svelte 逐字平移）。
  // 高亮同步簇（composerHl 派生/RO 观察/宽度滚动同步）与 handleKey/insertAtCursor 等根级
  // 逻辑全部留根——inputEl/hlEl 经 bind 上行供根上的 DOM 操作（插引用/整块删除/回收高度），
  // input 与 IME 拼写态走 bind 双向。键入/滚动/粘贴事件原样上行（根上 preventDefault）。
  import { createEventDispatcher } from 'svelte';
  const dispatch = createEventDispatcher();

  export let input = ''; // bind 双向到根
  export let hl = ''; // 根上 highlightComposer(input) 派生
  export let streaming = false;
  export let composing = false; // bind 双向到根（handleKey 删引用块要看）
  export let inputEl = null; // bind 上行（根上 insertAtCursor/removeComposerSpan 用）
  export let hlEl = null; // bind 上行（根上 syncComposerHl 用）
</script>

        <!-- 引用高亮：textarea 没法给部分文字上色，叠一层
             同字号同排版的浮层，只把 /能力、@文件 这些引用画成强调色；其余字符透明，
             纯为对齐换行。textarea 的文字照旧显示（浮层在上，视觉上正好盖住同款字形），
             这样浮层即使有亚像素偏差也只是轻微描边，不会出现"字没了"。 -->
        <div class="composer-field" class:composing={composing}>
          <div class="composer-hl" aria-hidden="true" bind:this={hlEl}>{@html hl}</div>
          <textarea
            class="composer-input"
            rows="1"
            bind:this={inputEl}
            bind:value={input}
            placeholder={streaming ? '可发送消息以进行对话引导或打断' : ''}
            on:keydown={(e) => dispatch('key', e)}
            on:input={(e) => dispatch('inputev', e)}
            on:scroll={() => dispatch('scrollev')}
            on:compositionstart={() => (composing = true)}
            on:compositionend={() => (composing = false)}
            on:paste={(e) => dispatch('paste', e)}
          ></textarea>
        </div>

<style>
  .composer-input {
    display: block;
    width: 100%;
    box-sizing: border-box;
    resize: none;
    border: none;
    outline: none;
    background: transparent;
    color: var(--g7);
    font-size: 14px;
    line-height: 1.5;
    padding: 8px 6px 6px;
    max-height: 200px;
    font-family: inherit;
  }
  .composer-input::placeholder {
    color: var(--g5);
  }
  /* 引用高亮浮层：与 .composer-input 同字号/行高/内边距/断词。默认由它画全部文字
     （输入框文字透明，只留光标），引用 token 用强调色；中文输入法拼写期间反过来——
     浮层隐藏、输入框自己显示文字，否则 IME 的候选/下划线会看不见。 */
  .composer-field {
    position: relative;
  }
  .composer-field .composer-input {
    color: transparent;
    caret-color: var(--g7);
  }
  .composer-field.composing .composer-input {
    color: var(--g7);
  }
  .composer-field.composing .composer-hl {
    visibility: hidden;
  }
  .composer-hl {
    position: absolute;
    inset: 0;
    z-index: 1;
    box-sizing: border-box;
    padding: 8px 6px 6px;
    font-family: inherit;
    font-size: 14px;
    line-height: 1.5;
    white-space: pre-wrap;
    overflow-wrap: break-word;
    word-break: normal;
    overflow: hidden;
    color: var(--g7);
    pointer-events: none;
    user-select: none;
  }
  /* {@html} 注入的 span 不带 Svelte 作用域类，必须 :global 才吃得到样式（同 markdown 那坑） */
  .composer-hl :global(.ref) {
    /* 引用高亮（/能力、@文件）回到橙色（2026-11-12 用户：怎么变成白色了）——
       11-11 全局透明+描边那轮改成白字+下划线，丢了强调色 */
    color: var(--brand-text);
    text-decoration: underline;
    text-decoration-color: color-mix(in srgb, var(--brand-text) 45%, transparent);
    text-underline-offset: 2px;
  }
  .composer-input:disabled {
    opacity: 0.6;
  }
</style>

