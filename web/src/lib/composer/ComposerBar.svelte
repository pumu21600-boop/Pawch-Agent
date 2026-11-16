<script>
  // 输入栏工具条（2026-11-15 阶段 B，自 App.svelte 逐字平移）：左 = 加号/能力菜单、
  // 附件选择、审批档位胶囊；右 = 上下文圆环卡（含压缩阈值输入）、模型快选、思考面板
  // （档位滑杆 + 粒子）、发送/停止。菜单开合与互斥（closeBarMenus）留根——全局点外关闭、
  // 发送与切会话都要收菜单，这里只渲染 + 上行事件；compactPercent 走 bind 双向。
  // 随面板内化：审批档图标/插件引用键（纯函数）、粒子动画机制（只喂滑杆视觉）。
  import { createEventDispatcher } from 'svelte';
  import { configLabel, cfgMediaBadge, fmtTok } from '../format.js';
  const dispatch = createEventDispatcher();

  export let streaming = false;
  export let input = '';
  export let pendingAttachments = [];
  export let pendingConvs = [];
  // —— 左侧菜单群（开合态留根，事件上行） ——
  export let plusMenuOpen = false;
  export let abilityMenuOpen = false;
  export let ownSkills = [];
  export let skillsLoading = false;
  export let mcpServers = [];
  export let mcpLoading = false;
  export let wasmPlugins = [];
  export let enabledExts = [];
  export let pluginSkills = [];
  export let menuExtIcon = (name) => ''; // 根上的 EXT_ICONS 图标函数（rpTabIcon 也在用）
  export let approvalEffective = 'auto';
  export let approvalMenuOpen = false;
  // —— 右侧：上下文圆环卡 ——
  export let ctxUsage = { window: 0, projected: 0 };
  export let ctxColor = '';
  export let ctxCirc = 0;
  export let ctxArcPct = 0;
  export let ctxPctText = '';
  export let ctxPct = 0;
  export let ctxRows = [];
  export let apiTotals = { prompt: 0, cached: 0 };
  export let compactPercent = 85; // bind 双向到根（阈值输入框）
  // —— 右侧：模型 / 思考 / 发送 ——
  export let modelMenuOpen = false;
  export let menuExpandedCfg = '';
  export let configs = [];
  export let activeConfigId = '';
  export let model = '';
  export let thinkMenuOpen = false;
  export let thinkingOn = false;
  export let thinking = '';
  export let thinkHint = '';
  export let pillThinkLabel = '';
  export let thinkLevels = null;
  export let thinkLevelIdx = 0;
  export let thinkMax = false;
  export let thinkProgress = 0;

  let fileInput; // 隐藏文件选择器引用（附件钮与 input 都在本面板，不再上行）

    const APPROVAL_LABELS = { 'read-only': '只读', auto: '自动', 'full-access': '完全访问' };
  // 审批档图标（收起态只显示图标；展开选择时才显示图标 + 名称）：
  // 只读=闭锁，自动=盾牌勾，完全访问=开锁。
  const APPROVAL_ICONS = {
    'read-only':
      '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="4.5" y="10.5" width="15" height="9.5" rx="2"/><path d="M8 10.5V7.6a4 4 0 0 1 8 0v2.9"/></svg>',
    auto: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3.2 19 6v5.2c0 4.4-3 7.6-7 9.1-4-1.5-7-4.7-7-9.1V6Z"/><path d="m9 11.6 2 2 4.2-4.4"/></svg>',
    'full-access':
      '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="4.5" y="10.5" width="15" height="9.5" rx="2"/><path d="M8 10.5V7.6a4 4 0 0 1 7.7-1.6"/></svg>',
  };
  const approvalIcon = (m) => APPROVAL_ICONS[m] || APPROVAL_ICONS.auto;

    function pluginSkillsOf(name) {
    return pluginSkills.filter((sk) => sk.plugin === name);
  }
  // 插件引用键：标题去掉空白——「Word 文档」这类带空格的名字插进去会把 /引用 切断。
  // 后端 index_prompt 用同一口径列「可用插件」，插进输入框的 /引用 在模型那儿才解得开。
  function pluginRef(e) {
    return (e.title || e.name || '').replace(/\s+/g, '');
  }

    // 2026-11-08 二次提速（「整体提高流速」）：BUBBLE_SPEED 0.75→0.5，基准时长也一并压短
  const BUBBLE_SPEED = 0.5;
  $: bubbleSpeedMult = 1 + 1.5 * (1 - thinkProgress / 100);
  // 粒子参数表：固定种子错峰流动（自右向左，透明度 0→0.55→0）。
  // 全档显示（ZCode 只在最高档有），实际时长乘 bubbleSpeedMult 随档位加速。
  // 2026-11-08 三轮重排：12→40→140→238 颗（用户「还是太少了」「再加大密度」），
  // 约七成是 1px 细尘与 1px 高的横丝（高速下像被气流拉长的轨迹），2px 中粒维持
  // 主体密度，3~4.5px 大泡只做点缀。形状/相位由固定种子伪随机生成，刷新不变。
  const EFFORT_BUBBLES = (() => {
    let seed = 20261108;
    const rnd = () => {
      seed = (Math.imul(seed, 1103515245) + 12345) & 0x7fffffff;
      return seed / 0x7fffffff;
    };
    const grain = (n, mk) => Array.from({ length: n }, mk);
    // 粒子色板：色相变化交给带子底（夕烧渐变），粒子回归以白为主——
    // 亮白在深火/琥珀上都压得住，掺少量米金和一颗「白热」冷白点缀
    const TINTS = ['#ffffff', '#ffffff', '#ffffff', '#ffffff', '#fff8e6', '#fff3d0', '#ffdf8a', '#ffd76a', '#dff2ff'];
    const tint = () => TINTS[Math.min(TINTS.length - 1, Math.floor(rnd() * TINTS.length))];
    return [
      // 细尘：1px 圆点，透明度压到最低（最远的那层，铺满整条带子）
      ...grain(128, () => ({ top: +(1.5 + rnd() * 21).toFixed(1), w: 1, h: 1, c: tint(), op: 0.3 + rnd() * 0.35, duration: 1.3 + rnd() * 0.8, delay: -rnd() * 2.2 })),
      // 横丝：1px 高、4~10px 长，加速时读作细流线
      ...grain(46, () => ({ top: +(3 + rnd() * 18).toFixed(1), w: +(4 + rnd() * 6).toFixed(1), h: 1, c: tint(), op: 0.28 + rnd() * 0.3, duration: 1.15 + rnd() * 0.7, delay: -rnd() * 2 })),
      // 中粒：2px 圆点
      ...grain(46, () => ({ top: +(3 + rnd() * 18).toFixed(1), w: 2, h: 2, c: tint(), op: 0.38 + rnd() * 0.28, duration: 1.5 + rnd() * 0.8, delay: -rnd() * 2.4 })),
      // 大泡：3~4.5px，少量提亮层次
      ...grain(18, () => {
        const s = +(3 + rnd() * 1.5).toFixed(1);
        return { top: +(5 + rnd() * 13).toFixed(1), w: s, h: s, c: tint(), op: 0.5 + rnd() * 0.24, duration: 1.8 + rnd() * 0.7, delay: -rnd() * 2.3 };
      }),
    ];
  })();
  // 粒子数量随填充宽度缩放：最低档填充只有 14px 圆头，全量塞进去会糊成一团。
  // 按填充占比等距抽样（不是截前 N 颗——表按粒径分组排列，截取会只剩细尘）。
  // 注意：pct 必须走参数传，Svelte 不追踪函数体内读到的响应式变量。
  const sampleForFill = (arr, pct) => {
    const n = Math.max(4, Math.round(arr.length * (0.05 + 0.95 * (pct / 100))));
    if (n >= arr.length) return arr;
    const step = arr.length / n;
    return Array.from({ length: n }, (_, i) => arr[Math.floor(i * step)]);
  };
  $: effortBubbles = sampleForFill(EFFORT_BUBBLES, thinkProgress);
</script>

        <div class="composer-bar">
          <div class="composer-left">
            <div class="bar-menu">
              <button
                class="icon-btn"
                title="添加上下文 / 选择能力"
                on:click={() => dispatch('toggleplus')}
              ><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M12 5v14M5 12h14"/></svg></button>
              {#if plusMenuOpen}
                <div class="bar-dropdown">
                  <button class="bar-item" on:click={() => dispatch('ctxpicker')}>
                    <span class="menu-sym">@</span> 添加上下文
                  </button>
                  <button class="bar-item" on:click={() => dispatch('openability')}>
                    <span class="menu-sym">/</span> 选择能力
                  </button>
                </div>
              {/if}
              {#if abilityMenuOpen}
                <!-- ①三类平铺，插件全列、不许展开；
                     ②引用前缀统一用「/」（技能/MCP/插件都 /，不再用 $） -->
                <div class="bar-dropdown">
                  <div class="bar-group">技能</div>
                  {#if ownSkills.length}
                    {#each ownSkills as sk (sk.name)}
                      <button class="bar-item" on:click={() => dispatch('insertref', `/${sk.name} `)}>
                        <span class="menu-sym">/</span> {sk.name}
                      </button>
                    {/each}
                  {:else if skillsLoading}
                    <div class="bar-empty">技能清单加载中…</div>
                  {/if}
                  <div class="bar-group">MCP</div>
                  <!-- MCP 只列服务本体，工具不逐条刷屏——
                       插服务级引用，模型按本体名找到并用它下面的工具 -->
                  {#each mcpServers as srv}
                    {#if srv.online !== false && srv.tools && srv.tools.length}
                      <button class="bar-item" on:click={() => dispatch('insertref', `/mcp__${srv.name} `)} title={srv.transport + ` · ${srv.tools.length} 个工具`}>
                        <span class="menu-sym">/</span> {srv.name}<span class="menu-sub">{srv.tools.length} 工具</span>
                      </button>
                    {/if}
                  {/each}
                  {#each wasmPlugins as w}
                    {#each w.tools || [] as t}
                      <button class="bar-item" on:click={() => dispatch('insertref', `/wasm__${w.stem}__${t.name} `)}>
                        <span class="menu-sym">/</span> {t.name}
                      </button>
                    {/each}
                  {/each}
                  {#if !mcpServers.some((s) => s.online !== false && s.tools && s.tools.length) && !wasmPlugins.some((w) => (w.tools || []).length) && mcpLoading}
                    <div class="bar-empty">MCP 工具加载中…</div>
                  {/if}
                  <div class="bar-group">插件</div>
                  <!-- 全部插件平铺（不展开）：点哪个都往输入框插一个 /引用——带技能的插件
                       插它第一个技能名（模型按技能索引认得），纯面板插件插插件标题（模型按
                       系统提示的「可用插件」索引认得）。面板不从这里开：右侧栏「+」菜单才是
                       面板的家（同一能力只在一个管理面出现）。列表口径 = enabledExts（启用且
                       清单合法），与应用插件目录一一对应，不筛子集 -->
                  {#each enabledExts as e (e.name)}
                    {@const sk = pluginSkillsOf(e.name)}
                    {@const ref = sk.length ? sk[0].name : pluginRef(e)}
                    <button
                      class="bar-item"
                      title={sk.length ? `${e.title || e.name}：插入技能引用 /${ref}` : `${e.title || e.name}：插入插件引用 /${ref}`}
                      on:click={() => dispatch('insertref', `/${ref} `)}
                    >
                      <span class="menu-sym menu-ico"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">{@html menuExtIcon(e.name)}</svg></span>
                      {e.title || e.name}
                    </button>
                  {/each}
                </div>
              {/if}
            </div>
            <input
              type="file"
              bind:this={fileInput}
              multiple
              accept="image/*,video/*,audio/*,.docx,.xlsx,.pptx,.pdf,.doc,.xls,.ppt,.csv,.rtf,.odt,.ods,.odp"
              style="display:none"
              on:change={(e) => dispatch('files', e)}
            />
            <button
              class="icon-btn"
              title="添加附件（图片/视频/音频/Office 文档）"
              on:click={() => fileInput && fileInput.click()}
            >
              <svg class="att-ico-lg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="m8 12.5 6.5-6.5a2.8 2.8 0 0 1 4 4L9 19.5a4.5 4.5 0 0 1-6.4-6.4l8.5-8.5"/></svg>
            </button>
            <div class="bar-menu">
              <button
                class="pill approval-pill"
                class:danger={approvalEffective === 'full-access'}
                on:click={() => dispatch('toggleapproval')}
                title="审批档位（按会话记忆，运行中切换下一步即生效）：只读=写/执行逐次问 · 自动=可动工作区与电脑高危先问 · 完全访问=全免审本机直跑"
                aria-label={`审批档位：${APPROVAL_LABELS[approvalEffective] || approvalEffective}`}
              >
                <span class="approval-ico" aria-hidden="true">{@html approvalIcon(approvalEffective)}</span>
              </button>
              {#if approvalMenuOpen}
                <div class="bar-dropdown">
                  {#each Object.entries(APPROVAL_LABELS) as [val, label]}
                    <button class="bar-item approval-item" class:active={val === approvalEffective} on:click={() => dispatch('pickapproval', val)}>
                      <span class="approval-opt">
                        <span class="approval-ico" aria-hidden="true">{@html approvalIcon(val)}</span>
                        <span>{label}</span>
                      </span>
                    </button>
                  {/each}
                </div>
              {/if}
            </div>
          </div>
          <div class="composer-right">
            {#if ctxUsage.window > 0}
              <div class="ctx-wrap">
                <svg class="ctx-ring" viewBox="0 0 30 30" width="20" height="20" title="上下文占用（下一次请求的预估）">
                  <circle class="ctx-track" cx="15" cy="15" r="12.5" />
                  <circle
                    class="ctx-arc"
                    cx="15"
                    cy="15"
                    r="12.5"
                    stroke={ctxColor}
                    stroke-dasharray={`${(ctxCirc * ctxArcPct) / 100} ${ctxCirc}`}
                    transform="rotate(-90 15 15)"
                  />
                </svg>
                <div class="ctx-card">
                  <div class="ctx-card-head">
                    <b>上下文容量</b>
                    <span>{fmtTok(ctxUsage.projected)}/{fmtTok(ctxUsage.window)}（{ctxPctText}）</span>
                  </div>
                  <div class="ctx-bar"><div class="ctx-bar-fill" style:width={`${ctxPct}%`} style:background={ctxColor}></div></div>
                  {#each ctxRows as row}
                    <div class="ctx-row">
                      <span class="ctx-dot" style:background={row.color}></span>
                      <span class="ctx-label">{row.label}</span>
                      <span class="ctx-val">{row.pct}</span>
                    </div>
                  {/each}
                  <div class="ctx-row cache-row">
                    <span class="ctx-label">平均缓存命中率</span>
                    <span class="ctx-val">{apiTotals.prompt ? ((apiTotals.cached / apiTotals.prompt) * 100).toFixed(1) + '%' : '—'}</span>
                  </div>
                  <div class="ctx-row cache-row compact-threshold-row">
                    <span class="ctx-label">上下文压缩阈值</span>
                    <span class="ctx-val threshold-val">
                      <input
                        class="compact-threshold-input"
                        type="number"
                        min="40"
                        max="85"
                        data-field="compactPercent"
                        bind:value={compactPercent}
                        title="上下文占到窗口这条线时，本轮结束后静默压缩；40–85，默认 85"
                        on:change={() => dispatch('compactsaved')}
                      /><span class="pct-suffix">%</span>
                    </span>
                  </div>
                  <!-- 口径说清：分子是「下一次请求」的预估（渠道实报锚点 + 之后的上下文增减，
                       裁剪会让它当场回落）；构成行是估算值按圆环百分比分摊（加总 = 圆环） -->
                </div>
              </div>
            {:else}
              <!-- 常驻空环：会话还没用过（无预算数据）时也显示空轨道环 -->
              <div class="ctx-wrap" title="上下文占用（发送消息后显示详情）">
                <svg class="ctx-ring" viewBox="0 0 30 30" width="20" height="20">
                  <circle class="ctx-track" cx="15" cy="15" r="12.5" />
                </svg>
              </div>
            {/if}
            <div class="bar-menu">
              <button
                class="pill model-pill"
                on:click={() => dispatch('togglemodel')}
                title="切换模型（按配置分组）"
              >
                {model || '未选模型'}
                <svg class="caret-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6"/></svg>
              </button>
              {#if modelMenuOpen}
                <div class="bar-dropdown right">
                  {#each configs as cfg}
                    <button
                      class="bar-item group-head"
                      class:active={cfg.id === activeConfigId}
                      on:click={() => dispatch('expandcfg', cfg.id)}
                    >
                      <span>{configLabel(cfg)}{cfg.id === activeConfigId ? ' · 当前' : ''}{cfgMediaBadge(cfg)}</span>
                      <svg class="caret-ico" class:open={menuExpandedCfg === cfg.id} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m9 6 6 6-6 6"/></svg>
                    </button>
                    {#if menuExpandedCfg === cfg.id}
                      {#if !(cfg.models || []).length}
                        <div class="menu-empty">（该配置还没有模型，去设置里获取）</div>
                      {/if}
                      {#each cfg.models || [] as m}
                        <button
                          class="bar-item sub"
                          class:active={cfg.id === activeConfigId && m === model}
                          on:click={() => dispatch('pickmodel', { cfg, m })}
                        >
                          {m}
                        </button>
                      {/each}
                    {/if}
                  {/each}
                  <div class="menu-sep"></div>
                  <button class="bar-item" on:click={() => dispatch('manage')}>
                    管理模型…
                  </button>
                </div>
              {/if}
            </div>
            <div class="bar-menu">
              <button
                class="pill think-pill"
                class:active={thinkingOn}
                on:click={() => dispatch('togglethink')}
                title={thinkHint}
              >
                <span class="pill-label">思考:</span> {pillThinkLabel}
                <svg class="caret-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6"/></svg>
              </button>
              {#if thinkMenuOpen}
                <div class="bar-dropdown right think-panel">
                  <!-- 三态分段（关/默认/开）：关/默认即收尾；「开」保持菜单展开——
                       档位滑杆只在开的时候出现（2026-11-01 改 ZCode 思考强度样式） -->
                  <div class="think-states">
                    <button class="think-state" class:active={thinking === 'off'} on:click={() => dispatch('pickthink', 'off')}>关</button>
                    <button class="think-state" class:active={thinking === ''} on:click={() => dispatch('pickthink', '')}>默认</button>
                    <button class="think-state" class:active={thinkingOn} on:click={() => dispatch('pickthink', 'on')}>开</button>
                  </div>
                  {#if thinkingOn && thinkLevels && thinkLevels.length > 1}
                    <!-- 档位滑杆：结构/动效自 ZCode ComposerModelMenu 复刻，配色走主色橙
                         （「颜色跟随主色调」）。粒子全档流动、
                         速度随档位升高加快（{#key} 换档重排，负延迟保持错峰不空场）；
                         填充延伸到圆钮圆心；最高档亮橙渐变层（非最高档纯橙填充） -->
                    <div class="think-strength">
                      <div class="ts-head">
                        <span class="ts-label">思考强度</span>
                        <span class="ts-value">{thinkLevels[thinkLevelIdx].label}</span>
                        <span class="ts-kbd">Ctrl+T</span>
                      </div>
                      <div class="ts-slider">
                        <div class="ts-vis" aria-hidden="true">
                          <div class="ts-track">
                            <!-- 描边放底层：填充选中区域会完全盖住它（ZCode 同款结构） -->
                            <div class="ts-ring"></div>
                            <div
                              class="ts-fill"
                              class:max={thinkMax}
                              style:width={`calc(14px + (100% - 28px) * ${thinkProgress / 100})`}
                            >
                              {#key thinkLevelIdx}
                                <div class="ts-bubbles">
                                  {#each effortBubbles as b}
                                    <span
                                      class="ts-bubble"
                                      style:top={`${b.top}px`}
                                      style:animation-duration={`${(b.duration * BUBBLE_SPEED * bubbleSpeedMult).toFixed(2)}s`}
                                      style:animation-delay={`${b.delay.toFixed(2)}s`}
                                    >
                                      <span
                                        class="ts-bubble-dot"
                                        style:width={`${b.w}px`}
                                        style:height={`${b.h}px`}
                                        style:opacity={b.op}
                                        style:color={b.c}
                                      ></span>
                                    </span>
                                  {/each}
                                </div>
                                <!-- 掉落火花 2026-11-08 试装后按用户要求撤掉（效果见提交 3f1f1f5） -->
                              {/key}
                            </div>
                          </div>
                          <div class="ts-thumb" style:left={`calc((100% - 28px) * ${thinkProgress / 100})`}></div>
                        </div>
                        <input
                          class="ts-range"
                          type="range"
                          min="0"
                          max={thinkLevels.length - 1}
                          step="1"
                          value={thinkLevelIdx}
                          aria-label="思考强度"
                          on:input={(e) => dispatch('thinkrange', +e.target.value)}
                        />
                      </div>
                    </div>
                  {/if}
                </div>
              {/if}
            </div>
            {#if streaming}
              <button class="send-btn stop" on:click={() => dispatch('stop')} title="停止" aria-label="停止"><svg viewBox="0 0 24 24" fill="currentColor"><rect x="7" y="7" width="10" height="10" rx="2"/></svg></button>
            {/if}
            <button class="send-btn" on:click={() => dispatch('send')} disabled={!input.trim() && !pendingAttachments.length && !pendingConvs.length} title={streaming ? '发送（进入队列，这一轮结束后自动发出）' : '发送（Enter）'} aria-label="发送"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 19V5M5 12l7-7 7 7"/></svg></button>
          </div>
        </div>

<style>
  .ctx-wrap {
    position: relative;
    display: flex;
    align-items: center;
    /* 圆环和百分数之间留 6px（2026-11-01 真机实测「圆环和数字撞位」——原先贴死） */
    gap: 6px;
    flex-shrink: 0;
  }
  .ctx-ring {
    display: block;
    cursor: default;
  }
  .ctx-card {
    display: none;
    position: absolute;
    bottom: calc(100% + 12px);
    right: -8px;
    width: 300px;
    background: var(--glass-float);
    border: 1px solid var(--g3);
    border-radius: 10px;
    padding: 12px 14px;
    box-shadow: 0 8px 24px var(--overlay);
    z-index: 20;
    font-size: 13px;
  }
  /* 悬停桥：卡片与环之间的间隙保持 hover 不断 */
  .ctx-card::after {
    content: '';
    position: absolute;
    top: 100%;
    right: 0;
    width: 60%;
    height: 12px;
  }
  .ctx-wrap:hover .ctx-card {
    display: block;
  }
  .ctx-card-head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    margin-bottom: 10px;
    /* 标题用正文色（2026-11-07 浅色实锤：原先整卡 tint-blue，浅色下全部文字染成亮蓝） */
    color: var(--g7);
  }
  .ctx-card-head span {
    color: var(--g6);
    font-size: 12px;
  }
  .ctx-bar {
    height: 8px;
    border-radius: 4px;
    background: var(--g2);
    overflow: hidden;
    margin-bottom: 12px;
  }
  /* 填充色随圆环 ctxColor（style:background 绑定）：常态与圆环同灰，压力高转警示色 */
  .ctx-bar-fill {
    height: 100%;
    border-radius: 4px;
    transition: width 0.3s;
  }
  .ctx-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 0;
    color: var(--g7);
  }
  .ctx-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
  }
  .ctx-label {
    flex: 1;
  }
  .ctx-val {
    color: var(--g6);
    font-variant-numeric: tabular-nums;
  }
  .cache-row {
    margin-top: 8px;
    padding-top: 8px;
    border-top: 1px solid var(--g2);
  }
  .att-ico-lg {
    width: 17px;
    height: 17px;
    display: block;
  }
  .caret-ico {
    width: 13px;
    height: 13px;
    flex: none;
    color: var(--g5);
  }
  .send-btn svg {
    width: 16px;
    height: 16px;
    display: block;
    margin: auto;
  }
  /* 上下文压缩阈值行：圆环卡片里缓存命中率下面的小输入框 */
  .compact-threshold-row .ctx-label {
    flex: 1;
  }
  .compact-threshold-input {
    width: 46px;
    padding: 1px 5px;
    font-size: 12px;
    /* 数字右对齐贴着 % 后缀，与上面几行的数字列对齐（同款等宽数字） */
    text-align: right;
    font-variant-numeric: tabular-nums;
    color: var(--g7);
    background: transparent;
    border: 1.5px solid var(--g3);
    border-radius: 6px;
    font-family: inherit;
  }
  .threshold-val {
    display: inline-flex;
    align-items: center;
    gap: 2px;
  }
  .pct-suffix {
    font-size: 12px;
    color: var(--g5);
  }
  .compact-threshold-input:focus {
    outline: none;
    border-color: var(--g6);
  }
  /* 数字输入框右侧的步进按钮会顶住文字，隐藏后数字才真正贴右 */
  .compact-threshold-input {
    appearance: textfield;
    -moz-appearance: textfield;
  }
  .compact-threshold-input::-webkit-outer-spin-button,
  .compact-threshold-input::-webkit-inner-spin-button {
    -webkit-appearance: none;
    margin: 0;
  }
  .bar-menu {
    position: relative;
    min-width: 0;
  }
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    max-width: 240px;
    padding: 4px 8px;
    border-radius: 8px;
    /* 输入栏里的 chip 不加描边（用户 2026-11-11 圈选：模型/思考/审批锁） */
    border: none;
    background: transparent;
    color: var(--g6);
    font-size: 12.5px;
    cursor: pointer;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .pill:hover {
    /* 输入栏 chip 一律无底色（用户 2026-11-11：思考那里不要背景色） */
    background: transparent;
    color: var(--g7);
  }
  .pill.active {
    /* 输入栏 chip（思考开）不走全局「背景高亮」口径：永远橙字、无底色
       （2026-11-12 用户：背景高亮下这里也不要有背景色） */
    background: transparent;
    color: var(--brand-text);
  }
  /* 完全访问档只留红字作静态警示：边框去掉（2026-10-07 真机实测「把红色框
     去掉」——整个输入栏就这一块挂红框太扎眼；档位含义有 title 与菜单说明讲）。 */
  .pill.danger {
    color: var(--tint-red-fg);
  }
  /* 审批档收起态：只留一枚图标（档位名称只在展开菜单里显示） */
  .approval-pill {
    padding: 4px 7px;
  }
  .approval-ico {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 16px;
    flex: none;
  }
  .approval-ico :global(svg) {
    width: 15px;
    height: 15px;
    display: block;
  }
  .approval-item {
    align-items: center;
  }
  .approval-opt {
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }
  .model-pill {
    font-family: Consolas, monospace;
  }
  .bar-dropdown {
    position: absolute;
    left: 0;
    bottom: calc(100% + 8px);
    min-width: 230px;
    max-height: 320px;
    overflow-y: auto;
    background: color-mix(in srgb, var(--g2) 92%, transparent);
    border: 1px solid var(--g3);
    border-radius: 10px;
    padding: 4px;
    z-index: 40;
    box-shadow: 0 8px 24px var(--overlay);
  }
  .bar-dropdown.right {
    left: auto;
    right: 0;
  }
  /* 思考三态分段 + 档位滑杆（2026-11-01，ZCode「思考强度」面板样式） */
  .think-states {
    display: flex;
    gap: 4px;
    background: var(--g1);
    border-radius: 9px;
    padding: 3px;
  }
  .think-state {
    flex: 1;
    border: none;
    background: transparent;
    color: var(--g6);
    font-size: 12.5px;
    padding: 5px 0;
    border-radius: 7px;
    cursor: pointer;
    transition: background 0.15s, color 0.15s;
  }
  .think-state:hover {
    color: var(--g7);
  }
  .think-state.active {
    /* 三态分段选中：橙色高亮无底色（2026-11-12 用户：去背景色用选中高亮） */
    background: var(--sel-bg);
    color: var(--sel-fg);
    font-weight: 600;
  }
  /* —— 档位滑杆：结构/动效自 ZCode ComposerModelMenu + composer-thought-slider.css 复刻 ——
     几何：滑块块 32px、轨道 24px（top 4）、圆钮 28px（行程 100%-28px）；
     动效：圆钮/填充 120ms ease-out（拖动也保留——离散档位切换不跳变，ZCode 源码注释同款）、
     渐变层 200ms 透明度。
     2026-11-01 两处定稿偏离 ZCode：①配色跟随主色橙（非 ZCode 绿蓝）；
     ②粒子全档流动，速度随档位升高加快（ZCode 只在最高档有气泡）。
     2026-11-08 三处：①去掉静止档点（首/中/尾各一颗，看着像卡住的粒子）；
     ②粒子 12→40→140→238 颗并掺 1px 细尘/横丝；③缓入曲线+整体提速（涡轮加速感）；
     ④填充改多色夕烧渐变；粒子回到以白为主（少量米金 + 一颗冷白点缀）。
     掉落火花试装过一次，用户看后要求撤掉，代码已移除（要恢复看提交 3f1f1f5）。 */
  .think-panel {
    min-width: 320px;
    padding: 6px;
  }
  .think-strength {
    padding: 10px 10px 8px;
  }
  .ts-head {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 4px;
  }
  .ts-label {
    font-size: 12px;
    color: color-mix(in srgb, var(--g7) 60%, transparent);
  }
  .ts-value {
    font-size: 13.5px;
    color: var(--g7);
  }
  .ts-kbd {
    margin-left: auto;
    font-size: 11px;
    line-height: 1;
    color: color-mix(in srgb, var(--g7) 55%, transparent);
    background: color-mix(in srgb, var(--g7) 8%, transparent);
    border-radius: 6px;
    padding: 4px 8px;
  }
  .ts-slider {
    position: relative;
    height: 32px;
    width: 100%;
  }
  .ts-vis {
    position: absolute;
    inset: 0;
    pointer-events: none;
  }
  .ts-track {
    position: absolute;
    left: 0;
    right: 0;
    top: 4px;
    height: 24px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--g7) 10%, transparent);
    overflow: hidden;
  }
  /* 描边放底层：填充选中区域会完全盖住它（ZCode 同款结构） */
  .ts-ring {
    position: absolute;
    inset: 0;
    border-radius: 999px;
    border: 1px solid color-mix(in srgb, var(--g7) 10%, transparent);
  }
  .ts-fill {
    position: absolute;
    top: 0;
    bottom: 0;
    left: 0;
    border-radius: 999px;
    /* 2026-11-08「整体颜色单一」：单色橙换成夕烧四段（余烬红→陶土→琥珀→浅金）。
       低档填充窄时只露出左段（深火），档位越高色带越完整、越亮。
       同日「左边颜色太深」：整条左端提亮一档（深酒红→灰玫瑰，深锈→中锈） */
    background: linear-gradient(90deg, #b05a3c 0%, #c8734a 35%, #dfa06e 68%, #f0c795 100%);
    overflow: hidden;
    transition: width 120ms ease-out;
  }
  /* 最高档渐变层：再加两档色相（左端压进酒红、右端推到浅金），比基础层更亮更有层次。
     纯色与渐变无法直接插值，用常驻层透明度过渡（200ms），反向调档也不瞬切（ZCode 结构同款） */
  .ts-fill::before {
    content: '';
    position: absolute;
    inset: 0;
    border-radius: inherit;
    opacity: 0;
    transition: opacity 200ms ease-out;
    background: linear-gradient(90deg, #8f4450 0%, #b2583f 18%, #d06f45 45%, #e89a63 72%, #f8dfa8 100%);
  }
  .ts-fill.max::before {
    opacity: 1;
  }
  .ts-bubbles {
    position: absolute;
    inset: 0;
  }
  .ts-bubble {
    position: absolute;
    left: 0;
    right: 0;
    opacity: 0;
    /* 缓入曲线=涡轮加速：进场慢、越走越快，出口甩出视线（不是匀速滑过） */
    animation: ts-bubble-flow cubic-bezier(0.45, 0.05, 0.8, 0.6) infinite;
  }
  .ts-bubble-dot {
    display: block;
    border-radius: 50%;
    background: currentColor; /* 色板经 color 传进来（主白 + 暖金点缀） */
  }
  /* 粒子在可见区间的峰值透明度：238 颗时要把细尘托起来才看得出「满」，0.55→0.62 */
  @keyframes ts-bubble-flow {
    0% {
      transform: translateX(104%);
      opacity: 0;
    }
    12%,
    82% {
      opacity: 0.62;
    }
    100% {
      transform: translateX(-4%);
      opacity: 0;
    }
  }
  /* 档点已移除（首/中/尾不要有点占着不动，滑杆只留流动粒子）。
     档位位置靠填充宽度+圆钮+上方档位名表达。 */
  .ts-thumb {
    position: absolute;
    top: 2px;
    width: 28px;
    height: 28px;
    border-radius: 50%;
    background: #fff;
    box-shadow: 0 1px 4px rgba(0, 0, 0, 0.45);
    transition: left 120ms ease-out;
  }
  /* 原生 range：只做交互层，视觉全透明；滑块尺寸与自绘圆钮一致保证首尾档与拖动坐标对齐 */
  .ts-range {
    position: absolute;
    inset: 0;
    margin: 0;
    width: 100%;
    height: 32px;
    appearance: none;
    -webkit-appearance: none;
    background: transparent;
    outline: none;
    cursor: pointer;
  }
  .ts-range::-webkit-slider-runnable-track {
    height: 32px;
    background: transparent;
  }
  .ts-range::-webkit-slider-thumb {
    -webkit-appearance: none;
    width: 28px;
    height: 28px;
    margin-top: 2px;
    border: 0;
    border-radius: 50%;
    background: transparent;
  }
  .ts-range::-moz-range-track {
    height: 32px;
    background: transparent;
  }
  .ts-range::-moz-range-thumb {
    width: 28px;
    height: 28px;
    border: 0;
    border-radius: 50%;
    background: transparent;
  }
  /* 注：不跟 prefers-reduced-motion 藏粒子——用户机器关了 Windows 客户区动画
     （SPI GETCLIENTAREAANIMATION=False 实测），WebView2 会把它映射成
     reduced-motion，照搬 ZCode 的隐藏守卫会让粒子在这台机器上永远不可见
     （2026-11-01 真机实测「我没看到粒子动效」）。粒子是本功能的主视觉，
     要直接可见，不交给系统开关裁断。 */
  /* 「/」统一能力菜单的分组标签（能力/技能/MCP·插件） */
  .bar-group {
    padding: 7px 10px 3px;
    font-size: 11px;
    color: var(--g5);
    user-select: none;
  }
  /* 能力菜单的加载占位：菜单先开、数据后台补（点了必须有反应，2026-11-07） */
  .bar-empty {
    padding: 4px 10px 6px;
    font-size: 12px;
    color: var(--g5);
  }
  /* 菜单行右侧的弱化小字（MCP 行显示工具数） */
  .menu-sub {
    margin-left: 8px;
    font-size: 11px;
    color: var(--g4, #8a8f98);
  }
  .bar-item {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 12px;
    width: 100%;
    text-align: left;
    padding: 7px 10px;
    background: transparent;
    border: none;
    border-radius: 7px;
    color: var(--g7);
    font-size: 13px;
    cursor: pointer;
    white-space: nowrap;
  }
  .bar-item:hover {
    background: var(--veil);
  }
  /* 模型快选：配置分组头 + 子项 */
  .bar-item.group-head {
    font-weight: 650;
    color: var(--g6);
    align-items: center;
  }
  .bar-item.group-head.active {
    color: var(--sel-fg);
  }
  .bar-item.group-head .caret-ico {
    width: 13px;
    height: 13px;
    flex: none;
    transition: transform 0.15s;
  }
  .bar-item.group-head .caret-ico.open {
    transform: rotate(90deg);
  }
  .bar-item.sub {
    padding-left: 24px;
  }
  .menu-sym {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    flex: none;
    border-radius: 5px;
    background: var(--g3);
    font-size: 12px;
    font-weight: 650;
    margin-right: 8px;
  }
  /* 「/」菜单插件行图标：同一格子不画字母，画各插件自己的 SVG（图标集见 EXT_ICONS） */
  .menu-sym.menu-ico {
    background: transparent;
    color: var(--g5);
  }
  .menu-sym.menu-ico svg {
    width: 16px;
    height: 16px;
  }
  .bar-item:hover .menu-sym.menu-ico {
    color: var(--g7);
  }
  .bar-item.active {
    /* 菜单内当前项（模型快选/@ 菜单等）：橙色高亮无底色（2026-11-12 统一口径） */
    background: var(--sel-bg);
    color: var(--sel-fg);
  }
  /* 上下文圆环：空色轨道 + 实色占用弧 */
  .ctx-ring .ctx-track {
    fill: none;
    stroke: var(--g3);
    stroke-width: 4.5;
  }
  .ctx-ring .ctx-arc {
    fill: none;
    stroke-width: 4.5;
    stroke-linecap: round;
    transition: stroke-dasharray 0.3s;
  }
  .send-btn {
    width: 34px;
    height: 34px;
    border-radius: 10px;
    border: none;
    background: linear-gradient(180deg,
      color-mix(in srgb, var(--brand) 72%, #fff),
      color-mix(in srgb, var(--brand-deep) 64%, #fff));
    color: var(--brand-ink);
    font-size: 16px;
    line-height: 1;
    cursor: pointer;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    box-shadow: 0 2px 10px color-mix(in srgb, var(--brand) 14%, transparent);
    transition: filter 0.15s ease, transform 0.15s ease;
  }
  .send-btn:hover:not(:disabled) {
    filter: brightness(1.04);
    transform: translateY(-1px);
  }
  .send-btn:disabled {
    background: var(--g3);
    color: var(--g5);
    cursor: default;
  }
  .send-btn.stop {
    background: var(--tint-red-bg);
    color: var(--tint-red-fg);
  }
  .send-btn.stop:hover {
    background: var(--tint-red-bg);
    filter: brightness(1.12);
  }
  .icon-btn {
    width: 34px;
    height: 34px;
    border-radius: 9px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    color: var(--g6);
    cursor: pointer;
    flex: none;
  }
  .icon-btn:hover {
    background: var(--g2);
    color: var(--g7);
  }
  .icon-btn:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .icon-btn svg {
    width: 20px;
    height: 20px;
  }
  .icon-btn {
    width: 28px;
    height: 28px;
    border-radius: 7px;
    /* 全局描边口径：常态无框，hover 才出 1px 描边 */
    border: 1px solid transparent;
    background: transparent;
    color: var(--g6);
    font-size: 16px;
    line-height: 1;
    cursor: pointer;
  }
  .icon-btn:hover {
    background: transparent;
    border-color: var(--g5);
    color: var(--g7);
  }
  /* 输入栏左侧两个图标钮（附件/加号）：悬浮回底色、不出描边（2026-11-14 用户圈选：
     这两个的悬浮态不要是边框）。描边口径只留给设置类图标钮 */
  .composer-left .icon-btn:hover {
    background: var(--g2);
    border-color: transparent;
    color: var(--g7);
  }
  .icon-btn:disabled {
    opacity: 0.4;
    cursor: default;
  }
  /* 输入栏一行的骨架：左（能力/附件/审批）右（圆环/模型/思考/发送）两端对齐，
     各自内部横排（B 轮平移时漏搬，2026-11-15 补回） */
  .composer-bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .composer-left,
  .composer-right {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }
  .menu-sep {
    height: 1px;
    margin: 5px 8px;
    background: var(--g2);
  }
  .menu-sep {
    height: 1px;
    background: var(--g2);
    margin: 4px 2px;
  }
  .menu-empty {
    color: var(--g5);
    font-size: 12px;
    padding: 8px 10px;
  }
</style>



