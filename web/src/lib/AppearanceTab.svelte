<script>
  import { microCss } from '../lib/dom.js';

  export let tab = '';
  // 主题/壁纸/CSS 注入器/微调引擎的状态本体在 App 侧（侧栏主题按钮、壁纸背景层、
  // 微调引擎的全局右键都挂在根上），这里只做设置分区的界面半边
  export let themeMode = 'dark';
  export let applyThemeMode = () => {};
  // 选中态口径（2026-11-12 用户拍板）：hl=文字高亮（默认）/ bg=背景高亮。
  // 全部选中态样式只引用 --sel-fg/--sel-bg，这里切换根元素 data-sel
  export let selStyle = 'hl';
  export let applySelStyle = () => {};
  export let wallpaper = '';
  export let wallpaperOpacity = 30;
  export let wallpaperMsg = '';
  export let onWallpaperPick = () => {};
  export let clearWallpaper = () => {};
  // features.css_tool：内置功能开关（css-ui 工具）关掉时收起注入器与微调模式两卡
  export let cssTool = true;
  export let customCss = '';
  export let applyCustomCss = () => {};
  export let microTweaks = {};
  export let clearAllMicro = () => {};
  export let editMode = false;
  export let exportCssRules = () => {};
  export let onImportCssRules = () => {};
  let cssImportInput;
</script>

{#if tab === 'appearance'}
  <div class="field">
    <label>主题</label>
    <div class="theme-opts">
      <button class="theme-opt" class:active={themeMode === 'dark'} on:click={() => applyThemeMode('dark')}>深色</button>
      <button class="theme-opt" class:active={themeMode === 'light'} on:click={() => applyThemeMode('light')}>浅色</button>
      <button class="theme-opt" class:active={themeMode === 'system'} on:click={() => applyThemeMode('system')}>跟随系统</button>
    </div>
  </div>
  <div class="field">
    <label>选中样式</label>
    <div class="theme-opts">
      <button class="theme-opt" class:active={selStyle === 'hl'} on:click={() => applySelStyle('hl')}>文字高亮</button>
      <button class="theme-opt" class:active={selStyle === 'bg'} on:click={() => applySelStyle('bg')}>背景高亮</button>
    </div>
  </div>
  <div class="field">
    <label>背景壁纸</label>
    <div class="wp-row">
      <label class="theme-opt wp-pick">选择媒体
        <input type="file" accept="image/*,video/mp4,video/webm,video/quicktime" on:change={onWallpaperPick} />
      </label>
      {#if wallpaper}
        <button class="theme-opt" on:click={clearWallpaper}>清除壁纸</button>
        <span class="wp-opacity">不透明度 <b>{wallpaperOpacity}</b>
          <input type="range" min="0" max="100" bind:value={wallpaperOpacity} />
        </span>
      {/if}
    </div>
    {#if wallpaperMsg}<div class="param-hint">{wallpaperMsg}</div>{/if}
  </div>
  {#if cssTool}
  <div class="field">
    <label>CSS 注入器</label>
    <textarea
      class="agents-editor css-injector"
      rows="8"
      spellcheck="false"
      bind:value={customCss}
      on:input={(e) => applyCustomCss(e.currentTarget.value)}
      placeholder={'直接写 CSS 规则，实时生效并记住。例如：\n.composer { border-radius: 24px; }\n.chat-head { display: none; }'}
    ></textarea>
    {#if microCss(microTweaks)}
      <label>微调生成的规则（只读预览）</label>
      <textarea
        class="agents-editor css-injector micro-readonly"
        rows="6"
        spellcheck="false"
        readonly
        value={microCss(microTweaks)}
      ></textarea>
      <div class="sub-row">
        <button class="btn" on:click={clearAllMicro}>清空全部微调规则</button>
      </div>
    {/if}
    <div class="sub-row">
      <button class="btn" on:click={exportCssRules}>导出规则</button>
      <button class="btn" on:click={() => cssImportInput && cssImportInput.click()}>导入规则</button>
      <input
        class="css-rules-file"
        type="file"
        accept=".json,.css,.txt,application/json,text/css,text/plain"
        bind:this={cssImportInput}
        on:change={onImportCssRules}
      />
      <span class="param-hint" style="align-self:center">导入会覆盖现有规则</span>
    </div>
  </div>
  <div class="field">
    <label>界面微调模式 <span class="exp-badge">实验性</span></label>
    <div class="sub-row">
      <button class="btn" class:primary={editMode} on:click={() => (editMode = !editMode)}>
        {editMode ? '● 微调中：右击界面元素即可修改（再点关闭）' : '开启微调：右击界面元素修改样式'}
      </button>
    </div>
  </div>
  {/if}
{/if}

<style>
  .theme-opts {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    align-items: center;
  }
  .theme-opt {
    padding: 7px 16px;
    border-radius: 8px;
    border: 1px solid var(--g3);
    background: var(--g2);
    color: var(--g7);
    font-size: 13px;
    cursor: pointer;
  }
  .theme-opt:hover {
    border-color: var(--g4);
  }
  .theme-opt.active {
    /* 本页选项钮选中：橙字 + 浅橙描边（2026-11-12 用户拍板：这里的选中态要有描边） */
    background: var(--sel-bg);
    border-color: var(--sel-line);
    color: var(--sel-fg);
    font-weight: 650;
  }
  .exp-badge {
    display: inline-block;
    margin-left: 6px;
    padding: 1px 8px;
    font-size: 10.5px;
    font-weight: 600;
    line-height: 1.6;
    border-radius: 999px;
    color: var(--brand-text);
    border: 1px solid color-mix(in srgb, var(--brand) 38%, transparent);
    background: color-mix(in srgb, var(--brand) 12%, transparent);
    vertical-align: 1px;
  }
  .wp-row {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }
  .wp-pick input[type='file'] {
    display: none;
  }
  .css-rules-file {
    display: none;
  }
  .wp-opacity {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-size: 12.5px;
    color: var(--g6);
  }
  .wp-opacity b {
    color: var(--g7);
    font-variant-numeric: tabular-nums;
    min-width: 26px;
    text-align: right;
  }
  .wp-opacity input[type='range'] {
    width: 180px;
    accent-color: var(--g5);
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
    border-color: color-mix(in srgb, var(--brand) 45%, var(--g3));
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--brand) 12%, transparent);
  }
  .agents-editor.micro-readonly {
    background: var(--g0);
    color: var(--g5);
    resize: vertical;
  }
</style>
