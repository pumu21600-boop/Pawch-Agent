<script>
  // 聊天区消息渲染（2026-11-15 阶段 C，自 App.svelte 逐字平移）：启动失败行/空态引导/用户与
  // 助手气泡、过程块（思考回放+工具卡+审批记录）、引导行、排队气泡、压缩标记、消息附件
  // （视频无帧降级）、多选、编辑态、详情弹窗入口。滚动壳（chat-body）与「回到底部」留根——
  // 钉底/估位/滚动锚定是根上的滚动状态机；折叠开合/跟随态/滚动容器 els/视频降级是组件私有
  // 随之走。动作事件上行；editingMsg 是根上 beginEdit 塞进来的对象，编辑框直接改它的 text。
  // 样式：本轮聊天样式留在 App 以 :global 命中本组件（类名全 app 唯一），后续轮机械下沉。
  import { createEventDispatcher } from 'svelte';
  import { md, highlightText } from '../md.js';
  import { fmtDurCn, messageParts, buildMessageBlocks, completedHasProcess, processSummary, processIsLive, toolMeta, parseArgs, prettyArgs, userMsgHtml, finalAnswerText } from '../chat.js';
  import { riskLabelFor } from '../format.js';
  import MsgCopyButton from './MsgCopyButton.svelte';
  const dispatch = createEventDispatcher();

  export let messageChunks = [];
  export let messages = [];
  export let startupError = '';
  export let msgSelectMode = false;
  export let msgSelected = [];
  export let editingMsg = null;
  export let nowTick = 0; // 实时计时滴答（根上流式期间每秒更新）
  export let tailCompactMarker = null;
  export let scrollSpacerPx = 0;

  // 折叠/跟随/视频降级：本组件私有（根上没有别的写入方）
    // 思考与工具按真实发生顺序合成「过程块」；点过程行展开。流式期间默认展开，
  // 收尾后默认收起（跑完只留结尾语）。
  let processOpenMap = {}; // `${msgId}:${blockIndex}` → true=展开
  // 单段思考的展开态：流式中默认展开，思考一结束（done）默认自动折叠；
  // 用户点过就按用户的选择记（每次思考完毕要自动折叠）。
  let thinkOpenMap = {}; // `${msgId}:${blockIndex}:${itemIndex}` → true=展开
  function toggleThink(key, currentlyOpen) {
    thinkOpenMap = { ...thinkOpenMap, [key]: !currentlyOpen };
  }
  // 流式滚动区自动跟随（思考框 / 工具参数区同一套）：新分片贴底；
  // 滚轮往上滚 → 暂停跟随（出「回到底部」按钮）
  let thinkFollowMap = {}; // `${msgId}:${blockIndex}:${itemIndex}` → 是否跟随
  let thinkBoxEls = {}; // 同上 → 思考滚动容器（bind:this）
  let toolFollowMap = {}; // 同上 → 工具参数区是否跟随（写入类工具的参数是流式补长的）
  let toolPreEls = {}; // 同上 → 工具参数滚动容器（bind:this）
  // 工具详情折叠（`${msgId}:${toolIndex}` → true=展开）：默认收起，点行展开参数/结果
  let toolFold = {};

    // 提问只在该用户作答时出现（pendingQuestions 的交互卡）；答完立即消失——
  // 不进对话流、不留记录卡（「问题答完为什么还在」）。
  function toggleProcess(key, currentlyOpen) {
    processOpenMap = { ...processOpenMap, [key]: !currentlyOpen };
  }
  function toggleToolDetail(msgId, idx, currentlyOpen) {
    toolFold = { ...toolFold, [`${msgId}:${idx}`]: !currentlyOpen };
  }

    function followScroll(node, active = true) {
    let following = true;
    let live = !!active;
    let lastTop = node.scrollTop;
    const nearBottom = () => node.scrollHeight - node.scrollTop - node.clientHeight < 24;
    const setFollowing = (nf) => {
      if (nf === following) return;
      following = nf;
      node.dispatchEvent(new CustomEvent('followchange', { detail: { following: nf } }));
    };
    const stick = () => {
      node.scrollTop = node.scrollHeight;
      lastTop = node.scrollTop;
    };
    const onScroll = () => {
      const t = node.scrollTop;
      if (t < lastTop - 1) setFollowing(false); // 上移=用户意图（跟随只会向下拽）
      else if (nearBottom()) setFollowing(true); // 滚回底部恢复跟随
      lastTop = t;
    };
    const onWheel = (e) => {
      if (e.deltaY < 0) setFollowing(false);
    };
    const onTouchStart = () => setFollowing(false);
    const mo = new MutationObserver(() => {
      if (live && following) stick();
    });
    node.addEventListener('scroll', onScroll, { passive: true });
    node.addEventListener('wheel', onWheel, { passive: true });
    node.addEventListener('touchstart', onTouchStart, { passive: true });
    mo.observe(node, { childList: true, characterData: true, subtree: true });
    if (live) stick();
    return {
      update(next) {
        const nl = !!next;
        if (nl && !live) {
          // 重新进入流式（新一轮思考）：恢复跟随并贴底
          following = true;
          setFollowing(true);
          stick();
        }
        live = nl;
      },
      destroy() {
        node.removeEventListener('scroll', onScroll);
        node.removeEventListener('wheel', onWheel);
        node.removeEventListener('touchstart', onTouchStart);
        mo.disconnect();
      },
    };
  }
  /// 回到底部（思考框 / 工具参数区共用）：滚到底后 scroll 事件会自行恢复跟随
  function jumpScroll(el) {
    if (el) el.scrollTop = el.scrollHeight;
  }

    // 调用方在收尾态取最后一张摊开，中途一律不摊；要看中间的走工具行点主语开侧栏预览。
  // 只认工作区图片，别的链接不放。
  function shotsFromParts(parts) {
    const out = [];
    for (const p of parts || []) {
      if (p.type !== 'tool' || !p.preview || !p.preview.includes('/api/workspace/')) continue;
      const m = /!\[[^\]]*\]\((\/api\/workspace\/[^)\s]+)\)/.exec(p.preview);
      if (m && !out.includes(m[1])) out.push(m[1]);
    }
    return out;
  }
  // 工具行可点的主语 → 侧栏预览路径（ZCode 式：行上只有文件名，点一下侧边栏看
  // 内容/图，平时不摊截图）。文件工具从参数拿完整路径（半截 JSON 参数点了没用，
  // 只认解析成功的）；截图从结果动作文本里拿 screenshots/*.png（结果到了才可点）。
  function toolPreviewPath(it) {
    const o = parseArgs(it.args);
    if (o) {
      for (const k of ['path', 'file_path', 'file', 'filename']) {
        const v = o[k];
        if (typeof v === 'string' && v.trim()) return v.trim();
      }
    }
    if (it.preview) {
      const m = String(it.preview).match(/screenshots\/[^\s"')]+\.png/);
      if (m) return m[0];
    }
    return null;
  }

    function fmtTs(ts) {
    if (!ts) return '';
    const d = new Date(ts);
    const p = (n) => String(n).padStart(2, '0');
    return `${p(d.getHours())}:${p(d.getMinutes())}`;
  }

    //  对钉底的人只会造成「这一帧不跟、下一帧再补」的两段式上弹。）
  // 压缩失败态（刷新后从文件重建的标记行没有 state，靠文案识别；2026-11-10）
  function compactFailed(m) {
    return !!m && m.kind === 'compact_marker'
      && (m.state === 'failed' || (typeof m.content === 'string' && m.content.includes('压缩失败')));
  }

    // loadedmetadata 时 videoWidth===0 即「没有可解码画面」：降级成文件条，点击用系统播放器打开。
  let vidNoFrame = {};
  function vidNoFrameKey(m, a) {
    return (m.id || '') + '|' + (a.name || '') + '|' + (a.path || '');
  }
  function vidMetaFail(m, a, e) {
    const v = e && e.currentTarget;
    if (v && v.videoWidth) return;
    vidNoFrame = { ...vidNoFrame, [vidNoFrameKey(m, a)]: true };
  }
</script>






      {#if startupError}
        <!-- 启动失败必须可见：服务未就绪时不许只留一个空对话（2026-10-27 实测） -->
        <div class="msg error-row startup-error">
          <div class="error-bubble">{startupError}</div>
          <button class="btn ghost" on:click={() => location.replace('/?t=' + Date.now())}>重试</button>
        </div>
      {/if}
      {#if messages.length === 0}
        <div class="empty-hint"><div class="empty-title">开始新的对话</div><div class="empty-sub">输入消息后按 Enter 发送，支持粘贴截图与拖拽文件</div></div>
      {/if}
      {#each messageChunks as c (c.key)}
        <!-- 块级 content-visibility（单条 .msg 的规则已挪到 .cv-chunk） -->
        <div class="cv-chunk" style:contain-intrinsic-size={`auto ${c.n * 140}px`}>
          {#each c.msgs as m (m.id)}
        {#if m.role === 'user'}
          {#if m.steered}
            <!-- 引导消息：静默轻量行（引导不打断、不占对话气泡位） -->
            <div class="steer-line" title="引导消息：不打断当时的对话，静默并入了那一轮">
              <span class="steer-line-badge">已引导</span>
              <span class="steer-line-text">{m.content}</span>
            </div>
          {:else}
          <div class="msg user" data-mid={m.id} class:selecting={msgSelectMode} on:click={() => dispatch('selectrow', m)}>
            {#if msgSelectMode && !m.queued}
              <span class="msg-check" title="选中这条消息">
                <input type="checkbox" checked={msgSelected.includes(m.id)} tabindex="-1" />
              </span>
            {/if}
            <div class="msg-stack">
            {#if editingMsg && editingMsg.id === m.id}
              <!-- 编辑态：原位变输入框；保存=回滚到这条之前+以新文本重发（先停输出） -->
              <div class="bubble user-bubble edit-box">
                <textarea class="edit-area" bind:value={editingMsg.text} rows="3"></textarea>
                {#if editingMsg.attachments && editingMsg.attachments.length > 0}
                  <div class="edit-att-hint">附件 {editingMsg.attachments.length} 个会随这条一起重发</div>
                {/if}
                <div class="edit-actions">
                  <button class="btn primary" on:click={() => dispatch('saveedit')} on:mousedown|preventDefault>保存并重发</button>
                  <button class="btn ghost" on:click={() => dispatch('editcancel')}>取消</button>
                  <span class="edit-hint">保存会先停止正在跑的输出</span>
                </div>
              </div>
            {:else}
              <div class="bubble user-bubble">
              {#if m.attachments && m.attachments.length > 0}
                <div class="msg-attachments">
                  {#each m.attachments as a}
                    <!-- 本轮消息带 data（data URL，即时显示）；刷新/重开会话后只剩元数据，
                         走 /api/workspace/raw 从 .attachments/ 读回（2026-11-10 修「刷新丢附件」） -->
                    {@const src = a.data || (a.path ? `/api/workspace/raw?path=${encodeURIComponent(a.path)}` : '')}
                    {#if a.mime && a.mime.startsWith('image/')}
                      <img
                        class="msg-att-img"
                        {src}
                        alt={a.name}
                        title="点击查看原图"
                        on:click={() => dispatch('imgpreview', src)}
                      />
                    {:else if a.mime && a.mime.startsWith('video/')}
                      {#if vidNoFrame[vidNoFrameKey(m, a)]}
                        <!-- 解不出画面（多为 H.265，本机 WebView2 缺解码器）：降级成文件条，
                             点击用系统默认播放器打开——预览不了不等于用不了 -->
                        <button
                          class="msg-att-file msg-att-file-act"
                          title="该视频的编码本机无法预览画面（多为 H.265/HEVC），点击用系统播放器打开"
                          on:click|stopPropagation={() => dispatch('attexternal', a)}
                        ><svg class="att-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="m8 12.5 6.5-6.5a2.8 2.8 0 0 1 4 4L9 19.5a4.5 4.5 0 0 1-6.4-6.4l8.5-8.5"/></svg> {a.name}</button>
                      {:else}
                        <!-- preload=metadata + playsinline：帧信息尽早加载、内联播放（2026-11-10）。
                             src 带 #t=0.1：强制解码并显示首帧——有些环境（WebView2+手机录屏 mp4）
                             元数据读得到却不出首帧，渲染成空画面+底部播放条（2026-11-11）。
                             data URL 不能带 fragment，只对 http src 追加。 -->
                        <video
                          class="msg-att-video"
                          controls
                          preload="metadata"
                          playsinline
                          src={src.startsWith('data:') ? src : src + '#t=0.1'}
                          on:loadedmetadata={(e) => vidMetaFail(m, a, e)}
                          on:error={() => (vidNoFrame = { ...vidNoFrame, [vidNoFrameKey(m, a)]: true })}
                        >
                          <track kind="captions" />
                        </video>
                      {/if}
                    {:else if a.mime && a.mime.startsWith('audio/')}
                      <audio class="msg-att-audio" controls {src}></audio>
                    {:else}
                      <div class="msg-att-file"><svg class="att-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="m8 12.5 6.5-6.5a2.8 2.8 0 0 1 4 4L9 19.5a4.5 4.5 0 0 1-6.4-6.4l8.5-8.5"/></svg> {a.name}</div>
                    {/if}
                  {/each}
                </div>
              {/if}
              {#if m.content}
                <!-- /技能 与 @引用 发出去后仍带强调色（与输入框浮层同一套 token 与配色） -->
                <div class="user-text">{@html userMsgHtml(m.content)}</div>
              {/if}
              {#if m.convs && m.convs.length}
                <!-- 引用只在气泡上挂 chip（正文发送时已整块拼给模型，不上屏刷屏）：
                     kind='quote' 是引用某条回答，其余是引用整个对话 -->
                <div class="msg-convs">
                  {#each m.convs as cv}
                    <span class="conv-tag" title={cv.kind === 'quote' ? '已把这条回答作为引用附在消息里' : `已把对话「${cv.title}」作为上下文发给模型`}>{cv.kind === 'quote' ? '回答' : '对话'} · {cv.title}</span>
                  {/each}
                </div>
              {/if}
              {#if m.queued}
                <div class="queued-row">
                  <span class="queued-badge">排队中 · 这一轮结束后自动发送</span>
                  <button class="queued-send" on:click={() => dispatch('sendnow', { id: m.id, mode: 'interrupt' })} title="停止当前这一轮，这条消息作为新一轮重新开始">立即发送</button>
                  <button class="queued-send" on:click={() => dispatch('sendnow', { id: m.id, mode: 'steer' })} title="不打断：跑完当前这步就并入本轮。静默生效，不占对话气泡">引导</button>
                </div>
              {:else if m.steered}
                <div class="queued-row"><span class="queued-badge steered">已引导当前回答</span></div>
              {/if}
            </div>
            {#if !msgSelectMode && !m.queued}
              <!-- 操作条在消息下方：时间贴行边、功能贴另一边；图标+tooltip，不占正文 -->
              <div class="msg-meta user-meta">
                {#if m.ts}<span class="msg-ts">{fmtTs(m.ts)}</span>{/if}
                <span class="msg-acts" on:click|stopPropagation>
                  {#if m.content}
                    <MsgCopyButton text={m.content} label="复制这条消息" />
                  {/if}
                  <button class="msg-act" title="回滚：文件恢复到这条消息之前，并删掉这条和它之后的所有消息" aria-label="回滚" on:click|stopPropagation={() => dispatch('rollback', m)}>
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 12a9 9 0 1 0 3-6.7"/><path d="M3 4v5h5"/></svg>
                  </button>
                  <button class="msg-act" title="编辑并重发（会先停止正在跑的输出）" aria-label="编辑" on:click|stopPropagation={() => dispatch('beginedit', m)}>
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z"/></svg>
                  </button>
                  <button class="msg-act" title="从这条消息分叉：复制到这条消息为止的对话到新会话，原对话不动" aria-label="分叉" on:click|stopPropagation={() => dispatch('fork', m)}>
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M5 6.5h11"/><circle cx="18.3" cy="6.5" r="1.7"/><path d="M5 6.5c5.5 0 3.5 11 9.5 11"/><circle cx="18.3" cy="17.5" r="1.7"/></svg>
                  </button>
                  <button class="msg-act" title="删除这条消息（同一问答的另一半保留；不可恢复）" aria-label="删除" on:click|stopPropagation={() => dispatch('delete', m)}>
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18"/><path d="M8 6V4h8v2"/><path d="M19 6l-1 14H6L5 6"/><path d="M10 11v6"/><path d="M14 11v6"/></svg>
                  </button>
                  <button class="msg-act" title="多选消息：批量删除" aria-label="多选" on:click|stopPropagation={() => dispatch('enterselect')}>
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><path d="M14 6.5h7"/><path d="M14 17.5h7"/></svg>
                  </button>
                </span>
              </div>
            {/if}
            {/if}
            </div>
          </div>
          {/if}
        {:else if m.role === 'assistant'}
          {@const shots = shotsFromParts(messageParts(m))}
          {@const answer = finalAnswerText(m)}
          <div class="msg assistant" data-mid={m.id} class:selecting={msgSelectMode} on:click={() => dispatch('selectrow', m)}>
            {#if msgSelectMode && !m.streaming}
              <span class="msg-check" title="选中这条回复">
                <input type="checkbox" checked={msgSelected.includes(m.id)} tabindex="-1" />
              </span>
            {/if}
            <div class="msg-stack">
            <div class="bubble asst-bubble">
              {#if m.streaming && m.workStartedAt}
                <div class="turn-bar live">
                  <span class="live-dot"></span>
                  <span>正在工作</span>
                  <span class="turn-time">{fmtDurCn(nowTick - m.workStartedAt)}</span>
                </div>
              {:else if m.workMs && !completedHasProcess(messageParts(m))}
                <!-- 纯问答轮（无思考/无工具）：与过程块同一行样式，只是没有过程可展开 -->
                <div class="process-head static">
                  <svg class="process-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3.5 13.7 9l5.5 1.7-5.5 1.7L12 18l-1.7-5.6L4.8 10.7 10.3 9 12 3.5Z"/><path d="m19 15.5.7 2.2 2.3.8-2.3.8-.7 2.2"/></svg>
                  <span class="process-title">{processSummary([], m.workMs)}</span>
                </div>
              {/if}
              {#if m.approval && m.approval.decided !== null && !m.approval.inFlow}
                <!-- 没挂上工具卡的兜底记录（旧会话/工具段缺失）；正常路径记录在工具卡下 -->
                <div class="approval-box legacy">
                  <div class="approval-head">
                    <span class="approval-title"
                      ><span class="approval-kicker">审批</span><b>{m.approval.tool}</b>
                      <span class="approval-risk risk-{m.approval.risk}"
                        >{m.approval.riskLabel || riskLabelFor(m.approval.risk)}</span
                      ></span
                    >
                  </div>
                  <div class="approval-state">{m.approval.decided ? '已允许' : '已拒绝'}</div>
                </div>
              {/if}
              <!-- 过程（思考 + 工具）与正文按真实发生顺序渲染：连续的过程合成一个可展开块，
                   正文把前后过程隔开。多步任务就是「过程 → 说明 → 过程 → 最终回答」。
                   流式期间过程默认展开；收尾后默认收起，只留总结与结尾语。 -->
              {#each buildMessageBlocks(messageParts(m), m.streaming) as b, bi}
                {@const pk = `${m.id}:${bi}`}
                {@const open = processOpenMap[pk] !== undefined ? processOpenMap[pk] : !!m.streaming}
                {#if b.kind === 'steer'}
                  <!-- 流式中的引导：真实位置隔断（完成态跟随过程折叠成摘要徽标） -->
                  <div class="steer-line" title="引导消息：不打断当时的对话，静默并入了那一轮">
                    <span class="steer-line-badge">已引导</span>
                    <span class="steer-line-text">{b.text}</span>
                  </div>
                {:else if b.kind === 'process'}
                  <div class="process" class:live={m.streaming}>
                    <button
                      class="process-head"
                      on:click={() => toggleProcess(pk, open)}
                      title={open ? '收起过程' : '展开思考与工具调用'}
                    >
                      <svg class="process-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3.5 13.7 9l5.5 1.7-5.5 1.7L12 18l-1.7-5.6L4.8 10.7 10.3 9 12 3.5Z"/><path d="m19 15.5.7 2.2 2.3.8-2.3.8-.7 2.2"/></svg>
                      <span class="process-title">{processSummary(b.items, !m.streaming && bi === 0 ? m.workMs : 0)}</span>
                      {#if processIsLive(b.items)}<span class="process-live"><span class="live-dot"></span></span>{/if}
                      <svg class="process-caret" class:open={open} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6"/></svg>
                    </button>
                    {#if open}
                      <div class="process-body">
                        {#each b.items as it, ii}
                          {@const ik = `${pk}:${ii}`}
                          {#if it.type === 'steer'}
                            <!-- 展开后引导行出现在真实位置（折叠时只看摘要徽标） -->
                            <div class="steer-line" title="引导消息：不打断当时的对话，静默并入了那一轮">
                              <span class="steer-line-badge">已引导</span>
                              <span class="steer-line-text">{it.text}</span>
                            </div>
                          {:else if it.type === 'thinking'}
                            {@const tOpen = thinkOpenMap[ik] !== undefined ? thinkOpenMap[ik] : !it.done}
                            <div class="think-item">
                              <button
                                class="think-label"
                                on:click={() => toggleThink(ik, tOpen)}
                                title={tOpen ? '收起这段思考' : '展开这段思考'}
                              >
                                <span>{it.ms ? `思考 ${fmtDurCn(it.ms)}` : '思考'}</span>
                                {#if m.streaming && !it.done}<span class="think-live">进行中</span>{/if}
                                <svg class="think-caret" class:open={tOpen} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6"/></svg>
                              </button>
                              {#if tOpen}
                                <div
                                  class="think-text"
                                  class:streaming={m.streaming && !it.done}
                                  use:followScroll={m.streaming && !it.done}
                                  bind:this={thinkBoxEls[ik]}
                                  on:followchange={(e) => (thinkFollowMap = { ...thinkFollowMap, [ik]: e.detail.following })}
                                >{it.text}</div>
                                {#if thinkFollowMap[ik] === false}
                                  <button class="think-jump" title="回到底部（继续跟随）" on:click={() => jumpScroll(thinkBoxEls[ik])}>
                                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 5v13"/><path d="m6 12 6 6 6-6"/></svg>
                                  </button>
                                {/if}
                              {/if}
                            </div>
                          {:else if it.type === 'tool'}
                            {@const tm = toolMeta(it.name, it.args)}
                            <!-- 直接读 toolFold（Svelte 只对模板里直接引用的变量建依赖；
                                 包进函数读不到变更——2026-10-26 真机实测过同款坑） -->
                            {@const tOpen = toolFold[`${m.id}:${it.toolIndex}`] === true}
                            {@const previewPath = toolPreviewPath(it)}
                            <div class="tool-item">
                              <button
                                class="tool-row"
                                on:click={() => toggleToolDetail(m.id, it.toolIndex, tOpen)}
                                title={tOpen ? '收起详情' : '展开参数与结果'}
                              >
                                <span class="tool-ico" class:running={it.status === 'running'}>{@html tm.iconSvg}</span>
                                <span class="tool-name">{tm.label}</span>
                                {#if tm.subject}
                                  {#if previewPath}
                                    <!-- ZCode 式主语：文件名可点，侧栏开预览（截图/文件平时不摊内容） -->
                                    <span
                                      class="tool-subject linkable"
                                      role="button"
                                      tabindex="-1"
                                      title="在侧栏预览：{previewPath}"
                                      on:click|stopPropagation={() => dispatch('openpreview', previewPath)}
                                    >{tm.subject}</span>
                                  {:else}
                                    <span class="tool-subject" title={tm.subject}>{tm.subject}</span>
                                  {/if}
                                {/if}
                                {#if it.adds !== null && it.adds !== undefined}
                                  <!-- 本次改动增删行数（write_file/edit_file 结果前缀解析而来） -->
                                  <span class="tool-diff">
                                    <span class="tool-diff-add">+{it.adds}</span>
                                    <span class="tool-diff-del">-{it.dels}</span>
                                  </span>
                                {/if}
                                <span class="tool-status {it.status}">
                                  {#if it.status === 'running'}<span class="tool-spin"></span>
                                  {:else if it.status === 'ok'}<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="m5 12.5 4.5 4.5L19 7.5"/></svg>
                                  {:else}<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"><path d="M6 6l12 12M18 6 6 18"/></svg>
                                  {/if}
                                </span>
                                <svg class="tool-caret" class:open={tOpen} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m9 6 6 6-6 6"/></svg>
                              </button>
                              {#if it.approval && it.approval.decided !== null}
                                <!-- 审批记录：就在它发生的那个工具卡下面（2026-11-03） -->
                                <div class="tool-approval">
                                  <span class="approval-risk risk-{it.approval.risk}"
                                    >{it.approval.riskLabel || riskLabelFor(it.approval.risk)}</span
                                  >
                                  <span class="ta-state">{it.approval.decided ? '已允许' : '已拒绝'}</span>
                                  {#if it.approval.reason}
                                    <span class="ta-reason" title={it.approval.reason}>{it.approval.reason}</span>
                                  {/if}
                                </div>
                              {/if}
                              {#if tOpen}
                                <div class="tool-detail">
                                  {#if it.args}
                                    <div class="tool-sec">
                                      <div class="tool-sec-label">参数</div>
                                      <!-- 参数是流式补长的（模型边吐边涨，写入类工具最长），
                                           和思考框同一套跟随：默认贴底，上滑固定 -->
                                      <pre
                                        class="tool-pre"
                                        use:followScroll={m.streaming && it.status === 'running'}
                                        bind:this={toolPreEls[ik]}
                                        on:followchange={(e) =>
                                          (toolFollowMap = { ...toolFollowMap, [ik]: e.detail.following })}
                                      >{@html highlightText(prettyArgs(it.args), 'json')}</pre>
                                      {#if m.streaming && it.status === 'running' && toolFollowMap[ik] === false}
                                        <button class="tool-jump" title="回到底部（继续跟随）" on:click={() => jumpScroll(toolPreEls[ik])}>
                                          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 5v13"/><path d="m6 12 6 6 6-6"/></svg>
                                        </button>
                                      {/if}
                                    </div>
                                  {/if}
                                  {#if it.preview}
                                    <div class="tool-sec">
                                      <div class="tool-sec-label">结果</div>
                                      <pre class="tool-pre">{@html highlightText(it.preview, 'output')}</pre>
                                    </div>
                                  {/if}
                                  {#if !it.preview && it.status === 'running'}<div class="tool-wait">执行中…</div>{/if}
                                </div>
                              {/if}
                            </div>
                          {:else if it.type === 'content'}
                            <!-- 完成态折叠进来的中途说明：保留原文与 markdown 结构，颜色压暗 -->
                            <div class="content process-note" on:click={(e) => dispatch('contentclick', e)}>{@html md(it.text)}</div>
                          {/if}
                        {/each}
                      </div>
                    {/if}
                  </div>
                {:else if b.text}
                  <!-- doc-cite：agent 回复里 /api/workspace/... 的文件引用，点击落到右侧预览标签 -->
                  <div class="content" on:click={(e) => dispatch('contentclick', e)}>{@html md(b.text)}</div>
                {/if}
              {/each}
              {#if !m.streaming && shots.length}
                <!-- browser 截图只在收尾摊开（最后一张=收尾验证图）；中途截图静默给
                     模型自查，想看走工具行点主语开侧栏预览（2026-11-07 口径：
                     「静默让模型自己看，最后收尾才展示，别两张一样的图刷屏」）。
                     点在图上开大图，复用 md-img 的点击预览 -->
                <div class="msg-shots">
                  {#each [shots[shots.length - 1]] as src}
                    <div class="tool-shot" on:click={(e) => dispatch('contentclick', e)}>
                      <img class="md-img" {src} alt="页面截图" loading="lazy" data-md-img="1" />
                    </div>
                  {/each}
                </div>
              {/if}
              {#if m.streaming && !messageParts(m).length}
                <div class="typing"><span class="typing-dot"></span><span class="typing-dot"></span><span class="typing-dot"></span></div>
              {/if}
            </div>
            {#if m.ts || (!m.streaming && (m.stats || answer))}
            <div class="msg-meta">
              {#if m.ts}<span class="msg-ts">{fmtTs(m.ts)}</span>{/if}
              {#if (!m.streaming && !msgSelectMode && answer) || (m.stats && !m.streaming)}
                <!-- 回答操作：复制 / 引用这条回答 / 详细信息（复制与引用只对收尾态出现，
                     流式期间文本还在变；多选态让位给勾选） -->
                <span class="msg-acts" on:click|stopPropagation>
                  {#if !m.streaming && !msgSelectMode && answer}
                    <MsgCopyButton text={answer} label="复制这条回答" />
                    <button class="msg-act" title="引用这条回答：附到输入区，随下一条消息一起带给模型" aria-label="引用" on:click|stopPropagation={() => dispatch('quote', m)}>
                      <svg viewBox="0 0 24 24" fill="currentColor"><path d="M6.5 5.5C4.6 5.5 3 7.1 3 9c0 1.9 1.6 3.5 3.5 3.5.3 0 .6 0 .9-.1-.6 1.9-2 3.3-3.9 3.9l.6 1.8c3.4-1.1 5.6-4 5.6-7.6V9c0-1.9-1.6-3.5-3.5-3.5Z"/><path d="M17.5 5.5C15.6 5.5 14 7.1 14 9s1.6 3.5 3.5 3.5c.3 0 .6 0 .9-.1-.6 1.9-2 3.3-3.9 3.9l.6 1.8c3.4-1.1 5.6-4 5.6-7.6V9c0-1.9-1.6-3.5-3.5-3.5Z"/></svg>
                    </button>
                  {/if}
                  {#if m.stats && !m.streaming}
                    <!-- 信息卡入口：首 token 延迟/输出耗时/渠道/模型/tokens/缓存命中。
                         放回答下方右下角 -->
                    <button class="msg-act" title="这条回复的详细信息" aria-label="回答详情" on:click|stopPropagation={() => dispatch('stats', m)}>
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><circle cx="12" cy="12" r="9"/><path d="M12 11v5"/><path d="M12 8h.01"/></svg>
                    </button>
                  {/if}
                </span>
              {/if}
            </div>
            {/if}
            </div>
          </div>
        {:else if m.role === 'system' && m.kind === 'compact_marker'}
          <!-- 静默压缩标记行（2026-11-07）：「正在压缩→压缩后」同位更新，全程唯一可见痕迹；
               失败态带原因 +「重试」按钮（2026-11-10） -->
          <div class="compact-divider" data-mid={m.id} class:running={m.state === 'running'} class:failed={compactFailed(m)}>
            {m.content}{#if compactFailed(m)}<button class="compact-retry" on:click|stopPropagation={() => dispatch('retrycompact', m)}>重试</button>{/if}
          </div>
        {:else if m.role === 'error'}
          <!-- 报错行：与重启后落盘恢复的 [错误] 系统消息同款灰字（
               实时红气泡和重启后灰字两副面孔太突兀，统一成后者）。
               落盘侧已带 [错误] 前缀（server 落 [错误] {e}），实时侧这里补齐前缀 -->
          <div class="msg system" data-mid={m.id}><div class="sys-text">[错误] {m.content}</div></div>
        {:else if m.role === 'cp'}
          <!-- 检查点消息只给改动条提供基线（dockCp），不渲染 -->
        {:else}
          <div class="msg system" data-mid={m.id}><div class="sys-text">{m.content ?? ''}</div></div>
        {/if}
        {/each}
        </div>
      {/each}
      {#if tailCompactMarker}
        <!-- 压缩标记作为最后一条：移出分块单独渲染，零高度浮在底部留白区（见样式注释） -->
        <div class="compact-divider tail" data-mid={tailCompactMarker.id} class:running={tailCompactMarker.state === 'running'} class:failed={compactFailed(tailCompactMarker)}>
          {tailCompactMarker.content}{#if compactFailed(tailCompactMarker)}<button class="compact-retry" on:click|stopPropagation={() => retryCompact(tailCompactMarker)}>重试</button>{/if}
        </div>
      {/if}
      {#if scrollSpacerPx > 0}
        <!-- 编辑/回滚后的临时底部留白：保住 scrollTop 不被浏览器钳位；内容长出来自动收掉 -->
        <div class="scroll-spacer" style:height={`${scrollSpacerPx}px`}></div>
      {/if}


