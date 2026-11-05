<script>
  import { onMount, afterUpdate, onDestroy, tick } from 'svelte';
  import Galaxy from './Galaxy.svelte';
  // docx 排版预览（2026-10-02 真机实测「要能看排版，完全体」）：mammoth 把 docx
  // 容器转成 HTML 再沙箱 iframe 渲染（表格/列表/图片/标题层级全保留）
  // docx/xlsx 预览的大依赖（mammoth 304KB / SheetJS 688KB）2026-10-08 起改**动态
  // 导入**：静态 import 时它们进首包，每次开应用都要多下 1MB，而大多数会话压根
  // 不打开文档预览。调用点都在 async 函数里，await import 零行为变化。
  import ImagePreview from './lib/ImagePreview.svelte';
  // 助手消息 markdown 渲染（「星号太多」：escape-first 安全渲染）
  import { md, highlightText } from './lib/md.js';
  // 聊天展示层纯逻辑（2026-10-30 UI 轮）：分段/合并/工具人话化。
  // 不碰 DOM，Node 单测见 web/test/chat.test.mjs（npm test / CI 门禁）。
  import {
    fmtDurCn,
    appendThinking,
    settleThinking,
    appendContent,
    addToolPart,
    settleToolArgs,
    settleToolPart,
    trimParts,
    partsFromStoredMessage,
    messageParts,
    mergeAssistantRun,
    attachToolPreview,
    attachApprovalToToolPart,
    settleApprovalOnParts,
    parseTodoArgs,
    latestTodoInMessages,
    filesTouchedInMessages,
    attachAskToToolPart,
    settleAskOnParts,
    buildMessageBlocks,
    completedHasProcess,
    processSummary,
    processIsLive,
    toolMeta,
    prettyArgs,
  } from './lib/chat.js';
  import RenameModal from './lib/RenameModal.svelte';
  import ConfirmModal from './lib/ConfirmModal.svelte';
  import WorkspaceModal from './lib/WorkspaceModal.svelte';
  import ModelPicker from './lib/ModelPicker.svelte';
  import MemoryTab from './lib/MemoryTab.svelte';
  import SkillsTab from './lib/SkillsTab.svelte';
  import McpTab from './lib/McpTab.svelte';
  import AppearanceTab from './lib/AppearanceTab.svelte';
  import ModelTab from './lib/ModelTab.svelte';
  import RolesTab from './lib/RolesTab.svelte';
  import ExtTab from './lib/ExtTab.svelte';
  import ScheduleTab from './lib/ScheduleTab.svelte';
  import DataTab from './lib/DataTab.svelte';
  import TerminalPanel from './lib/TerminalPanel.svelte';
  import { fmtBytes, relTime, fmtTok, norm, configLabel, gitStatusLabel, normPath, relShort, contentText, webNormalizeUrl, riskLabelFor } from './lib/format.js';
  import { parseCssColor, parsedToHex, rgbToHex, rgbAlpha, hexWithAlpha, gradientFirstColor, rebuildGradient, resolveCssVars, findBgSource, structuralSelector, pathOf, findByPath, makeUid, ELEMENT_ALIASES, escalateCss, microCss } from './lib/dom.js';

  // —— 配置（改动即自动保存即时生效）——
  let providers = [];
  let configs = [];
  let activeConfigId = '';
  let providerId = 'deepseek';
  let baseUrl = '';
  let model = '';
  let apiKey = '';
  let configName = '';
  let savedModels = [];
  let savedBaseUrl = ''; // 已保存的 base_url：只有「获取模型」打的就是这套端点时才据此收敛已选表
  let savedKeyUrl = '';   // 当前激活配置的已存 key 尾号
  let settingsMsg = '';
  let startupError = ''; // 启动流水线失败（服务未就绪/设置会话读不到）——必须可见
  let settingsOpen = false;
  let autoSaveTimer = null;
  let pendingSave = null; // autoSave 挂起的字段（切换配置前 flush 用）
  let approvalMode = (() => {
    // 三档（2026-10-03 并三档：sandbox 档取消，旧值迁移 read-only）；旧档名自动迁移
    const map = { suggest: 'read-only', sandbox: 'read-only', 'auto-edit': 'auto', 'full-auto': 'auto', local: 'full-access' };
    const saved = localStorage.getItem('approvalMode');
    return map[saved] || saved || 'auto';
  })(); // 审批三档：read-only / auto / full-access
  // M1：每会话各自的档位（切会话时圆点显示该会话真实在跑的档，而不是全局最后一次选的）
  let approvalBySession = {};
  $: approvalEffective = approvalBySession[currentSessionId || '@new'] || approvalMode;
  let chatPinned = true; // 是否钉在对话底部（决定是否自动滚屏）
  // 上下文占用（圆环+明细卡）。口径对齐 DSH token-meter：三个数语义分开——
  // projected=下一次请求的预估占用（圆环分子，实报锚点+带符号增量）、
  // pressure=最近一次渠道实报的 prompt（锚点，没拿到时为 null）、window=模型窗口（圆环分母）
  let ctxUsage = { projected: 0, pressure: null, window: 0, breakdown: null };
  let ctxLen = 0; // 当前模型上下文长度（0=默认）
  let maxOut = 0; // 当前模型最大输出长度（0=默认）
  let temperature = null; // 采样温度（null=渠道默认）
  let topP = null; // 核采样（null=渠道默认）
  let thinking = ''; // 思考开关：''=渠道默认 / on / off
  let thinkingStyle = 'auto'; // 生效的思考参数形态（thinkingFormat 词表）
  let capabilities = { tools: true, vision: false, video: false, audio: false }; //  模型能力标记（纯手动勾选）
  let roles = {}; //  角色路由映射
  // 内置功能开关（插件页「内置」组功能行；后端 settings.features，缺省全开）
  let features = { roles: true, css_tool: true, galaxy: true, computer: true };
  // —— 工作区 ——
  let workspace = '';
  let recentWorkspaces = [];
  let wsModalOpen = false;
  let wsMsg = '';
  // 多文件夹项目（2026-11-04，对齐 ZCode）：当前项目的额外根文件夹（绝对路径）。
  // 与主目录同权限参与文件面板/@ 引用/文件读写；git/产物/记忆仍只在主目录。
  let workspaceFolders = [];
  let foldersModal = false;
  let foldersMsg = '';
  let foldersBusy = false;
  let apiTotals = { prompt: 0, cached: 0 }; // 渠道真实用量累计（平均缓存命中率）
  // 027：用量按会话记账 —— key → { ctx, api }。后台会话的流写各自账本，
  // 切回会话时从账本恢复（跑完的会话不再发事件，不记账就会显示 0）。
  let usageBySession = {};
  let serverUsage = {}; // 服务端会话文件里的用量快照（重启/重开后恢复圆环；打开会话时刷新）

  // —— 获取模型弹窗 ——
  let modelModalOpen = false;
  let fetchedModels = [];

  // —— 模型快选（输入栏）——
  let modelMenuOpen = false;
  let menuExpandedCfg = ''; // 模型快选菜单里展开的配置组（默认展开当前配置）
  // —— 加号菜单（用户 2026-10-02 改版：只剩 @ 与 / 两个入口）——
  // @ = 上下文（对话 + 文件，见 openCtxPicker）；/ = 能力（模板 + 技能 + MCP/插件工具，
  // 一个菜单分组列出，$ 技能菜单并进来不再单列）
  let plusMenuOpen = false;
  let abilityMenuOpen = false;
  let ctxPickerOpen = false;
  let ctxFiles = [];
  let ctxLoading = false;
  let ctxQuery = '';
  let ctxMsg = '';
  let composerInputEl;

  // —— 对话（027：按会话隔离）——
  // 每个会话各有自己的消息缓冲与运行流：A 会话在跑不影响切到 B 会话发消息，
  // A 的流式内容也只写回 A 的缓冲（切走再切回不丢、不串台）。
  let messages = []; // 当前显示会话的消息（sessionMsgs 的镜像）
  // 消息分块（2026-10-07 真机实测「拖侧栏巨卡」，CDP trace 定位）：content-visibility
  // 的每个盒子都会被浏览器内部 IntersectionObserver 跟踪，7778 条消息 = 7778 个盒，
  // 侧栏一变宽全部盒位移，每帧重算交叉就要 18ms+。按固定条数收拢成块后，IO 要跟踪的
  // 盒降到几十分之一，每帧开销回到 1ms 级；块内消息仍是独立 keyed 节点，流式增量
  // 只更新变动的那一条，初次加载同样受益（只有可见块参与布局绘制）。
  // 块的估位高度 = 块内条数 × 140px（沿用单条消息的估位口径，滚动条不跳）。
  const CV_CHUNK = 20;
  $: messageChunks = (() => {
    const out = [];
    for (let i = 0; i < messages.length; i += CV_CHUNK) {
      const msgs = messages.slice(i, i + CV_CHUNK);
      out.push({ key: msgs[0].id, n: msgs.length, msgs });
    }
    return out;
  })();
  let sessionMsgs = {}; // key（会话 id 或 '@new'）→ 消息数组
  let streams = {}; // key → { asstId, controller } 正在跑的那条流
  // 服务端在跑、但本页没有流的会话（别的窗口/页面重载后）：侧栏也给转圈，
  // 不然「原对话看着已结束，其实后台还在跑」，人就不知道该不该引导（2026-11-05 实锤）。
  let serverRunning = {};
  // 侧栏会话行转圈的数据源：本页流 ∪ 服务端在跑列表（模板直接读这个 Set 才有依赖）
  $: runningIds = new Set([...Object.keys(streams), ...Object.keys(serverRunning)]);
  let outbox = {}; // key → [{ id, text, atts }] 排队待发（本轮结束后自动按序发出）
  // 服务端接管排队消息后的对账定时器（key → interval id）：本页流已死拿不到回答，
  // 轮询到服务端空闲后从会话文件重载（2026-09-27 教训，见 d.queued 分支注释）
  const reconcileTimers = {};
  let streaming = false; // 当前显示会话是否在跑（syncCur 维护）
  let controller = null; // 当前显示会话的 AbortController（停止按钮用）
  let input = '';
  let seq = 0;
  let chatBody;
  let chatInputEl = null; // 输入框浮层（footer）：dock 长高时聊天区底部留白跟着走
  // 本页发过的「立即发送/引导」内容：服务端 steer 事件按序配对，避免自己发的那条被重复处理
  let localSteers = [];
  // 「已引导」静默标记（引导不打断、不发用户气泡——显示
  // 「已引导」轻提示，本轮结束清除；发完输入框立即可继续干别的）
  let guided = {}; // key → [{ text }]
  /// 划掉一条「已引导」提示（纯展示层；引导早已并入，不影响正在跑的那轮）
  function dismissGuided(key, idx) {
    const list = [...(guided[key] || [])];
    if (idx < 0 || idx >= list.length) return;
    list.splice(idx, 1);
    guided = { ...guided, [key]: list };
  }

  // —— 消息操作（2026-10-26 用户需求批）——
  // 回滚/删除共用服务端截断端点（/api/sessions/truncate，按文件里第 N 条 user 消息定位）；
  // 编辑=停流→回滚到该消息之前→以新文本重发；多选用于批量删除与生成上下文摘录。
  let editingMsg = null; // { id, text } 正在编辑的用户消息
  let msgSelectMode = false;
  let msgSelected = []; // 选中的消息 id
  let statsModal = null; // 信息卡：正在展示的那条 assistant 消息
  let ctxCopied = false; // 「已复制」轻提示（2s 自灭，不弹不落盘）

  const curKey = () => currentSessionId || '@new';
  // 刷新「当前会话」的三个镜像变量（切会话、流开始/结束时调用）
  function syncCur() {
    const k = curKey();
    const wasStreaming = streaming;
    const nowStreaming = !!streams[k];
    streaming = nowStreaming;
    controller = streams[k] ? streams[k].controller : null;
    messages = sessionMsgs[k] || [];
    // 任务清单 dock：切会话 / 新任务开始 / 本轮结束都回到自动态
    //（新任务自动展开、结束自动收起；用户手动状态不跨任务残留）
    if (k !== todoDockKey || nowStreaming !== wasStreaming) {
      todoDockPinned = null;
      if (k !== todoDockKey || (!wasStreaming && nowStreaming)) todoDockDismissed = false;
      todoDockKey = k;
    }
    // 草稿附件的「未能保留」通知：等这条会话的消息数组存在了再补（否则会建出
    // 一个只有通知的数组，loadSession 见到「已有缓存」就不重建历史了）。
    const pend = composerNotices[k];
    if (pend && (sessionMsgs[k] || k === '@new')) {
      delete composerNotices[k];
      pushMsg(k, { id: nextId(), role: 'system', content: pend });
    }
  }
  function setMsgs(key, arr) {
    sessionMsgs = { ...sessionMsgs, [key]: arr };
    if (key === curKey()) messages = arr;
  }
  function updMsgs(key, fn) {
    setMsgs(key, fn(sessionMsgs[key] || []));
  }
  function pushMsg(key, msg) {
    updMsgs(key, (a) => [...a, msg]);
  }
  function patchAsst(key, asstId, fn) {
    updMsgs(key, (a) => a.map((m) => (m.id === asstId ? fn({ ...m }) : m)));
  }
  // 会话 id 从 '@new' 变成服务端分配的 id：缓冲 / 流 / 队列一起挪过去
  function rekey(from, to) {
    if (!from || from === to) return;
    // 输入区草稿跟着 key 走（'@new' → 真 id）。当前正显示的会话先把活输入落一次，
    // 只挪存储里的旧副本会丢掉「上次切换后又敲的那几行」。
    const isCur = curKey() === from;
    if (isCur) persistComposer(from);
    const cmap = composerMap();
    if (cmap[from]) {
      cmap[to] = cmap[from];
      delete cmap[from];
      composerWrite(cmap);
    }
    const arr = sessionMsgs[from] || [];
    const nextMsgs = { ...sessionMsgs };
    delete nextMsgs[from];
    nextMsgs[to] = arr;
    sessionMsgs = nextMsgs;
    if (streams[from]) {
      const st = streams[from];
      const nextStreams = { ...streams };
      delete nextStreams[from];
      nextStreams[to] = st;
      streams = nextStreams;
    }
    if (outbox[from]) {
      const q = outbox[from];
      const nextOut = { ...outbox };
      delete nextOut[from];
      nextOut[to] = q;
      outbox = nextOut;
    }
    // 027：用量账本跟着 key 走（'@new' → 真 id 时不丢记账）
    if (usageBySession[from]) {
      const u = usageBySession[from];
      const nextU = { ...usageBySession };
      delete nextU[from];
      nextU[to] = u;
      usageBySession = nextU;
    }
    // M1：会话档位跟着 key 走（新会话切过档后拿到 id，档位不能丢）
    if (approvalBySession[from]) {
      const m = approvalBySession[from];
      const nextM = { ...approvalBySession };
      delete nextM[from];
      nextM[to] = m;
      approvalBySession = nextM;
    }
    if (isCur) {
      setCurSession(to, true); // 草稿已挪到 to，别再拿 from 的当前输入覆盖
      messages = arr;
    }
    syncCur();
    // 直接在空输入框打字发送的会话：runner 懒创建会话时后端不广播 sessions_changed
    // （只有定时/CLI 唤醒才广播），侧栏要等这一轮跑完的 finally 才刷新——长回合期间
    // 新会话一直不在侧栏（2026-11-01 真机实测，死循环那轮拖了半个多小时）。拿到真
    // id 的瞬间刷一次列表（from==='@new' = 这是懒创建的那条，点新对话按钮的路径
    // 已自带 loadSessions，靠 from 区分不重复刷）。
    if (from === '@new') loadSessions();
  }

  // —— 多模态媒体附件与大图预览 ——
  let pendingAttachments = []; // 待发送附件列表 [{ name, mime, data, size }]
  let fileInput; // 隐藏文件选择器引用
  let previewImageModal = null; // 放大预览的图片 dataUrl

  function onFileSelect(e) {
    const files = e.target.files;
    if (!files || !files.length) return;
    for (let i = 0; i < files.length; i++) {
      addAttachmentFile(files[i]);
    }
    e.target.value = '';
  }

  async function addAttachmentFile(file) {
    if (file.size > 25 * 1024 * 1024) {
      await confirmAsk(`文件「${file.name}」超过 25MB 上限`, true);
      return;
    }
    // 对图片附件进行前端等比缩放与优质压缩（最长边 1536px，符合各大视觉模型规范且规避反代请求体体积限制）
    if (file.type && file.type.startsWith('image/')) {
      const reader = new FileReader();
      reader.onload = (ev) => {
        const rawData = ev.target.result;
        const img = new Image();
        img.onload = () => {
          const maxDim = 1536;
          let w = img.width;
          let h = img.height;
          if (w > maxDim || h > maxDim) {
            if (w > h) {
              h = Math.round((h * maxDim) / w);
              w = maxDim;
            } else {
              w = Math.round((w * maxDim) / h);
              h = maxDim;
            }
          }
          const canvas = document.createElement('canvas');
          canvas.width = w;
          canvas.height = h;
          const ctx = canvas.getContext('2d');
          ctx.drawImage(img, 0, 0, w, h);
          // 转为 0.85 压缩率的 jpeg，体积极小（百KB量级）且画质清晰
          const compressedData = canvas.toDataURL('image/jpeg', 0.85);
          pendingAttachments = [
            ...pendingAttachments,
            {
              name: file.name,
              mime: 'image/jpeg',
              data: compressedData,
              size: Math.round(compressedData.length * 0.75),
            },
          ];
        };
        img.src = rawData;
      };
      reader.readAsDataURL(file);
      return;
    }

    // 非图片文件（视频/音频/其他）走标准直读
    const reader = new FileReader();
    reader.onload = (ev) => {
      let mime = file.type || '';
      if (!mime || mime === 'application/octet-stream') {
        const n = file.name.toLowerCase();
        if (n.endsWith('.mp3')) mime = 'audio/mp3';
        else if (n.endsWith('.wav')) mime = 'audio/wav';
        else if (n.endsWith('.ogg')) mime = 'audio/ogg';
        else if (n.endsWith('.m4a')) mime = 'audio/m4a';
        else if (n.endsWith('.aac')) mime = 'audio/aac';
        else if (n.endsWith('.flac')) mime = 'audio/flac';
        else if (n.endsWith('.mp4')) mime = 'video/mp4';
        else if (n.endsWith('.webm')) mime = 'video/webm';
        else if (n.endsWith('.mov')) mime = 'video/quicktime';
        else mime = 'application/octet-stream';
      }
      pendingAttachments = [
        ...pendingAttachments,
        {
          name: file.name,
          mime,
          data: ev.target.result,
          size: file.size,
        },
      ];
    };
    reader.readAsDataURL(file);
  }

  function removeAttachment(index) {
    pendingAttachments = pendingAttachments.filter((_, i) => i !== index);
  }

  function handlePaste(e) {
    const files = e.clipboardData && e.clipboardData.files;
    if (files && files.length > 0) {
      let hasMedia = false;
      for (let i = 0; i < files.length; i++) {
        const file = files[i];
        if (file.type.startsWith('image/') || file.type.startsWith('video/') || file.type.startsWith('audio/') || file.size > 0) {
          hasMedia = true;
          addAttachmentFile(file);
        }
      }
      if (hasMedia) {
        e.preventDefault();
        return;
      }
    }
    const items = e.clipboardData && e.clipboardData.items;
    if (!items) return;
    for (let i = 0; i < items.length; i++) {
      const type = items[i].type || '';
      if (type.startsWith('image/') || type.startsWith('video/') || type.startsWith('audio/') || items[i].kind === 'file') {
        const file = items[i].getAsFile();
        if (file) {
          e.preventDefault();
          addAttachmentFile(file);
        }
      }
    }
  }

  // —— 拖拽文件进聊天区（2026-11-01 用户要「支持把文件拖拽进入 pawch」）——
  // 壳里 Tauri 默认 dragDropEnabled=true，会把 Windows 的拖放在原生层吃掉，前端
  // HTML5 的 drop 永远不触发（tauri-utils 文档原话：Windows 上要用前端拖放必须关掉）
  // → desktop/tauri.conf.json 已置 false。投放区从输入框扩到整个聊天列，拖动时给提示。
  let dragDepth = 0;
  let dropHover = false;
  function hasFiles(e) {
    const dt = e && e.dataTransfer;
    return !!dt && Array.prototype.indexOf.call(dt.types || [], 'Files') >= 0;
  }
  function onDragEnter(e) {
    if (!hasFiles(e)) return;
    e.preventDefault();
    dragDepth += 1;
    dropHover = true;
  }
  function onDragOver(e) {
    if (!hasFiles(e)) return;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy';
  }
  function onDragLeave(e) {
    if (!hasFiles(e)) return;
    dragDepth = Math.max(0, dragDepth - 1);
    if (dragDepth === 0) dropHover = false;
  }
  function onDropZone(e) {
    if (!hasFiles(e)) return;
    e.preventDefault();
    dragDepth = 0;
    dropHover = false;
    handleDrop(e);
  }

  function handleDrop(e) {
    e.preventDefault();
    const files = e.dataTransfer && e.dataTransfer.files;
    if (!files || !files.length) return;
    for (let i = 0; i < files.length; i++) {
      const f = files[i];
      if (!f) continue;
      // 文件夹拖进来在 WebView 里是一个 size=0、无 mime 的伪 File——读出来是空内容，
      // 用户会以为加上了。当场说清楚，别静默。
      if (f.size === 0 && !f.type) {
        confirmAsk(`「${f.name}」是空文件或文件夹，没有添加（文件夹请先压缩成 zip）`, true);
        continue;
      }
      addAttachmentFile(f);
    }
  }

  // —— 恢复与续命 /  多会话 ——
  let sessions = []; // 会话列表（新→旧）
  let workspaceBorn = {}; // 工作区路径 → 目录创建时间（ms）：同名文件夹重建时区分旧会话
  let currentSessionId = null; // 当前会话（null=下次发送自动新建）
  let projectName = ''; // 工作区目录名

  // B1 Plan/Act 手动档已移除：项目类任务由模型按系统提示自动规划步骤清单（todo list），普通聊天不规划

  // 无边框壳窗口控制 API（壳注入；浏览器客户端没有 → 隐藏窗口按钮/热区）
  let winApi = null;
  let winMax = false; // 窗口是否最大化（按钮 ▢ ↔ 还原图标）
  function adoptWinApi() {
    if (!winApi && window.agentWinApi) winApi = window.agentWinApi;
    refreshWinMax();
  }
  function refreshWinMax() {
    if (winApi && winApi.isMaximized) {
      winApi.isMaximized().then((v) => { winMax = !!v; }).catch(() => {});
    }
  }
  onMount(() => {
    adoptWinApi();
    // 输入区草稿：先按本标签页当前的 key 载入（随后恢复上次会话时由 setCurSession 换成那条的）
    restoreComposer(curKey());
    window.addEventListener('agentwinready', adoptWinApi);
    // 插件桥监听必须**同步**挂：右栏面板（trace/board/usage 等）iframe 一 load 完就发
    // pawch:hello/pawch:request（旧名 agent: 兼容一个版本），而下面那个 onMount 的启动流水线要 await
    // loadProviders→loadSettings→loadSessions（重会话恢复要几秒）。监听挂晚了，
    // 这期间到的消息全部丢失，面板卡「桥调用超时」且不重试（headless 全新 profile +
    // 7778 条消息会话必现；用户机器上偶发）。监听体不依赖这些数据，只管先接住。
    window.addEventListener('message', onExtMessage);
    // 兜底轮询：注入事件若在监听注册前已发过（首屏加载竞态），靠它接住
    const t = setInterval(() => {
      adoptWinApi();
      if (winApi) clearInterval(t);
    }, 500);
    // 最大化态跟随：系统级 Win+↑ / 双击标题栏 / 壳外还原都会 resize，节流 300ms 查一次
    let rzT = null;
    const onRz = () => { clearTimeout(rzT); rzT = setTimeout(refreshWinMax, 300); };
    window.addEventListener('resize', onRz);
    // 点工具栏菜单外部 → 全部收起（问题1）。监听与清理都必须在**同步** onMount 里：
    // async onMount 的 await 续体跑在 mount 完成之后，那里调 onDestroy 会抛
    // 「Function called outside component initialization」（2026-10-08 修过一版但放错了
    // 位置——放在 async 续体里照抛，且抛出会打断后面的启动续体）。清理函数随同步
    // onMount 的返回值注册，HMR 下不累积。
    document.addEventListener('click', closeMenusOnOutside);
    // Ctrl+T 思考三态轮换（ZCode 同款）；同样必须配对清理（HMR 不累积）
    document.addEventListener('keydown', thinkHotkey);
    // 刷新/关页前把正在跑的会话快照进 sessionStorage（重载后由 loadSession 兜回来）
    window.addEventListener('beforeunload', stashLiveState);
    // 输入框上方 dock（审批/提问/任务清单）长高时，聊天区底部留白跟着长：
    // footer 是绝对定位浮层，固定 170px 留白会让展开的清单盖住最后几条消息。
    let dockRo = null;
    if (typeof ResizeObserver !== 'undefined' && chatInputEl) {
      const fitChatBottom = () => {
        if (!chatBody) return;
        chatBody.style.paddingBottom = `${Math.max(170, (chatInputEl.offsetHeight || 0) + 24)}px`;
      };
      dockRo = new ResizeObserver(fitChatBottom);
      dockRo.observe(chatInputEl);
      fitChatBottom();
    }
    // 2026-10-08 修：三个根级监听器原无配对清理——生产态 App 只挂一次无感，
    // dev 的 HMR 每次热替换都销毁重建，message 监听累积会让插件桥的每次
    // pawch:request 被新旧实例各处理一次（reply 发两遍）。
    // 侧栏「运行中」转圈：本页的流即时，服务端在跑的（别的窗口/重载后）3 秒补一次
    refreshRunning();
    const runT = setInterval(() => {
      if (!document.hidden) refreshRunning();
    }, 3000);
    return () => {
      clearInterval(t);
      clearInterval(runT);
      clearTimeout(rzT);
      dockRo?.disconnect();
      window.removeEventListener('resize', onRz);
      window.removeEventListener('agentwinready', adoptWinApi);
      window.removeEventListener('message', onExtMessage);
      document.removeEventListener('click', closeMenusOnOutside);
      document.removeEventListener('keydown', thinkHotkey);
      window.removeEventListener('beforeunload', stashLiveState);
    };
  });

  onMount(async () => {
    await initStartupData();
    // 手风琴启动归一（同时只展开一个工作区）：老配置里可能存着
    // 多个展开态（改版前的行为），启动收敛成只展开当前项目
    expandedFlags = workspace ? { [workspace]: true } : { __ungrouped__: true };
    localStorage.setItem('projExpanded', JSON.stringify(expandedFlags));
    refreshGit(); // 顶条显示当前分支/未提交数（非仓库时自动隐藏）
    loadVersion(); // 021：侧栏「关于」行（版本 + 运行根）
    loadExt(); // 021：组件总线扩展清单（有扩展时侧栏才出入口）
    // 单元素微调标记首屏补盖（会话恢复后元素才就位，故在加载完再盖一次）
    stampMicroUids();
    // 面板上次就是开着且停在「网页」时，补一次浏览器状态加载：openRightPanel 只在用户
    // 点工具栏开关时跑，刷新页面这条路不走它——不补的话 webActiveId 一直是空的，面板
    // 显示空态引导、壳里 webview 也不显示（用户刷新后看到的「网页没了」）
    if (rightPanelOpen && rpTab === 'web') loadWebState();
  });

  function closeMenusOnOutside(e) {
    if (e.target && e.target.closest && (e.target.closest('.bar-menu') || e.target.closest('.side-menu') || e.target.closest('.row-btn') || e.target.closest('.conv-more'))) return;
    modelMenuOpen = false;
    thinkMenuOpen = false;
    approvalMenuOpen = false;
    sideMenuOpen = false;
    sessMenuId = null;
    projMenuPath = null;
  }

  // —— 工作区切换 ——
  async function applyWorkspace(path) {
    try {
      const r = await fetch('/api/workspace', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ path }),
      });
      const d = await r.json();
      if (!r.ok || d.error) throw new Error(d.error || `HTTP ${r.status}`);
      wsModalOpen = false;
      setCurSession(null);
      // 工作区变了：会话/消息/圆环重置，重新拉列表。
      // **例外：还在跑的会话原样留着**（流 + 缓冲 + 排队 + 账本）——切工作区只是换视图，
      // 不是取消（SSE 断 ≠ 取消，服务端那轮还在跑）。以前一律 abort+清空，切回来看到的是
      // 落盘快照：界面显示本轮已结束（带统计行），服务端却还在跑，引导按钮也跟着失效
      // （2026-11-05 真机：操作电脑自己切走工作区，人就没法引导了）。
      const liveStreams = {};
      const liveMsgs = {};
      const liveOut = {};
      const liveUsage = {};
      const liveGuided = {};
      for (const k of Object.keys(streams)) {
        const st = streams[k];
        if (!st) continue;
        // '@new' 不保留：会话 id 还没到手，rekey 会把它当成当前会话切过去（跨工作区串台）
        if (k === '@new') {
          try { st.controller.abort(); } catch {}
          continue;
        }
        liveStreams[k] = st;
        if (sessionMsgs[k]) liveMsgs[k] = sessionMsgs[k];
        if (outbox[k]) liveOut[k] = outbox[k];
        if (usageBySession[k]) liveUsage[k] = usageBySession[k];
        if (guided[k]) liveGuided[k] = guided[k];
      }
      streams = liveStreams;
      streaming = false;
      controller = null;
      sessionMsgs = liveMsgs;
      outbox = liveOut;
      usageBySession = liveUsage;
      guided = liveGuided;
      syncCur();
      restoreUsage('@new');
      await loadSettings();
      await loadSessions(true);
      // 手风琴（用户 2026-10-07）：切过来的项目独占展开态，其余工作区的对话
      // 列表一并收起——侧栏同时只显示当前项目的会话
      expandedFlags = workspace ? { [workspace]: true } : {};
      localStorage.setItem('projExpanded', JSON.stringify(expandedFlags));
      // git/产物面板跟着换工作区重置并重拉（openSession 同款）。漏了这段=切过去
      // 面板还挂着上一个工作区的清单，而 git 状态横幅已跟着新工作区更新——
      // 2026-10-13 真机实测「智云的改动串到另一个工作区」就是这个症状
      rvBaseline = '';
      rvFiles = [];
      rvOpen = {};
      rvDiff = {};
      rvWarn = '';
      rpFiles = [];
      rpArtifactsNote = '';
      rpWarn = '';
      manualBaseline = ''; // 重拍的基线只属于当时的会话/工作区，切走即作废
      rpStale = false;
      rvStale = false;
      rpStaleClosed = false;
      rvStaleClosed = false;
      rpStaleErr = '';
      rvStaleErr = '';
      if (rpTab === 'git') loadReview();
      else if (rpTab === 'artifacts') loadArtifacts();
    } catch (e) {
      // wsMsg 只渲染在 WorkspaceModal 里，而那个弹窗没有任何置 true 的入口（死代码）——
      // 切换失败一直是静默的。改应用内提示弹窗，壳内外都可见。
      await confirmAsk(`切换项目失败：${e.message}`, true);
    }
  }

  afterUpdate(() => {
    // 只有用户本来就钉在底部时才自动滚屏；上滑阅读时不打断。
    // 拖拽期（侧栏/右面板/终端三个手柄）整个跳过：这行读 scrollHeight 会把刚写进的
    // 宽度样式再强制同步布局一遍（写完读→写完还触发 scroll 事件再读），每帧多出一轮
    // 全量 reflow，拖拽就跟不上手——真机实测拖拽帧 p95 41ms、22% 掉到 30fps 以下。
    // 拖拽中用户也不看消息区，停手后下一次 flush 自然补上滚动位置与微调标记。
    const dragging = resizing || rpResizing || termResizing;
    if (!dragging && chatPinned && chatBody) chatBody.scrollTop = chatBody.scrollHeight;
    // 单元素微调标记：Svelte 重渲染会重建节点，这里按存下的路径补盖 data-mt
    if (!dragging) stampMicroUids();
  });

  function onChatScroll() {
    if (!chatBody) return;
    chatPinned = chatBody.scrollTop + chatBody.clientHeight >= chatBody.scrollHeight - 60;
  }

  // —— 上下文圆环——
  // 分子 = projected（下一次请求的预估占用），分母 = window（模型窗口）
  $: ctxPct = ctxUsage.window ? Math.min(100, Math.round((ctxUsage.projected / ctxUsage.window) * 100)) : 0;
  // 占用极少时百分比仍为 0，但弧保底 2%（实色弧可见，圆环"活"着）
  $: ctxArcPct = ctxUsage.projected > 0 && ctxPct === 0 ? 2 : ctxPct;
  $: ctxPctText = ctxUsage.projected > 0 && ctxPct === 0 ? '<1%' : ctxPct + '%';
  $: ctxCirc = 2 * Math.PI * 12.5;
  // 常态浅灰（同参考样式）；占用高时转警示色
  $: ctxColor = ctxPct >= 85 ? 'var(--tint-red-fg)' : ctxPct >= 60 ? 'var(--tint-yellow-fg)' : 'var(--g6)';
  // 构成行：估算构成**按圆环的百分比分摊**（DSH 同一根环的画法）——各行加起来正好是
  // 圆环那个数，不再各算各的分母；构成本身是近似值，只当构成看，不当总量
  $: ctxRows = (() => {
    const b = ctxUsage.breakdown || { messages: 0, tools: 0, system: 0, other: 0, tools_schema: 0 };
    const sum = Math.max(1, b.messages + b.tools + b.system + b.other + (b.tools_schema || 0));
    // 构成比（各行 / 上下文总量）：回答「上下文里装的是什么」。
    // 不要按圆环百分比分摊——窗口大时 ctxPct 舍入成 0，五行会一起变 0.0%（实锤）
    const row = (label, v, color) => ({ label, v, color, pct: ((v / sum) * 100).toFixed(1) + '%' });
    return [
      row('消息', b.messages, 'var(--tint-blue-fg)'),
      row('工具结果', b.tools, 'var(--tint-cyan-fg)'),
      row('系统提示词', b.system, 'var(--tint-blue-fg)'),
      // 随请求发送的 tools JSON Schema（后端计入总量；旧后端无此字段按 0 兜底）
      row('工具定义', b.tools_schema || 0, 'var(--tint-green-fg)'),
      row('其他', b.other, 'var(--g5)'),
    ];
  })();
  // —— 输入框：思考模式选择器（常驻）——
  // 复用设置面板的 thinking 字段（唯一数据源）：off/on=明确设置；''=渠道默认
  let thinkMenuOpen = false;
  const THINK_LABELS = { '': '默认', on: '开', off: '关', low: '低', medium: '中', high: '高', max: '最大', minimal: '最低', xhigh: '最高' };
  // 思考三态机制（2026-11-01 改 ZCode「思考强度」样式）：关('off')/默认('')/开。
  // 开 = 'on'（渠道默认强度）或显式档位；切到「开」菜单不收起，下面才出现档位滑杆；
  // 档位到线上参数的映射在网关：anthropic=预算比例（低20%/中50%/高80%，GLM 端点归 effort 档）
  const THINKING_ON_VALUES = ['on', 'low', 'medium', 'high', 'max', 'xhigh', 'minimal'];
  $: thinkingOn = THINKING_ON_VALUES.includes(thinking);
  // 档位滑杆的档位表（按生效形态给；没有表 = 该渠道无档位，滑杆不出现）。
  // deepseek 官方规范档只有 低/高/最大（「中」是假档，发出去会被折算成 high）；
  // zai（GLM-5.3+）官方 reasoning_effort 三档 low/high/max，官方用词「最高」；
  // StepFun（生效形态=openai）官方 reasoning_effort 三档 low/medium/high
  const THINK_LEVELS = {
    deepseek: [
      { v: 'low', label: '低' },
      { v: 'high', label: '高' },
      { v: 'max', label: '最大' },
    ],
    zai: [
      { v: 'low', label: '低' },
      { v: 'high', label: '高' },
      { v: 'max', label: '最高' },
    ],
    anthropic: [
      { v: 'low', label: '低' },
      { v: 'medium', label: '中' },
      { v: 'high', label: '高' },
    ],
    openai: [
      { v: 'low', label: '低' },
      { v: 'medium', label: '中' },
      { v: 'high', label: '高' },
    ],
  };
  $: thinkLevels = THINK_LEVELS[thinkingStyle] || null;
  // 滑杆当前位置：显式档位按值定位；'on'（渠道默认强度）停在中间档
  $: thinkLevelIdx = (() => {
    if (!thinkLevels || !thinkLevels.length) return 0;
    const i = thinkLevels.findIndex((l) => l.v === thinking);
    return i >= 0 ? i : Math.floor((thinkLevels.length - 1) / 2);
  })();
  // ZCode 同款几何（ComposerModelMenu 原样移植）：行程=100%-28px（圆钮宽），
  // 填充延伸到圆钮圆心（14px 起步），档点 inset 14px 与圆钮圆心对齐
  $: thinkProgress = thinkLevels && thinkLevels.length > 1 ? (thinkLevelIdx / (thinkLevels.length - 1)) * 100 : 0;
  // 最高档：橙渐变层亮起（非最高档纯品牌橙填充，ZCode 结构）
  $: thinkMax = !!(thinkLevels && thinkLevels.length > 1 && thinkLevelIdx === thinkLevels.length - 1);
  // 粒子速度随档位升高而加快：最低档 2.5x 基准时长，最高档 1x。
  // 基准再整体提速 1/4（用户同日「整体再稍微快点」）：BUBBLE_SPEED 缩放基准时长
  const BUBBLE_SPEED = 0.75;
  $: bubbleSpeedMult = 1 + 1.5 * (1 - thinkProgress / 100);
  // 粒子参数表：固定种子错峰流动（自右向左，透明度 0→0.4→0）。
  // 全档显示（ZCode 只在最高档有），实际时长乘 bubbleSpeedMult 随档位加速
  const EFFORT_BUBBLES = [
    { top: 4, size: 2, duration: 2.6, delay: -0.4 },
    { top: 14, size: 3, duration: 3.2, delay: -2.1 },
    { top: 8, size: 4, duration: 2.8, delay: -1.1 },
    { top: 18, size: 2, duration: 3.6, delay: -0.8 },
    { top: 3, size: 3, duration: 3, delay: -2.5 },
    { top: 12, size: 2, duration: 3.4, delay: -1.7 },
    { top: 6, size: 3, duration: 3.1, delay: -0.9 },
    { top: 17, size: 4, duration: 2.9, delay: -2.3 },
    { top: 10, size: 2, duration: 3.5, delay: -0.2 },
    { top: 2, size: 2, duration: 2.7, delay: -1.8 },
    { top: 15, size: 3, duration: 3.3, delay: -1.2 },
    { top: 7, size: 4, duration: 3.6, delay: -3.1 },
  ];
  // 档点悬停：原生 range 盖住圆点，按坐标反馈（±10px 内点亮，ZCode 同款）
  let tsHoverIdx = null;
  function tsHoverMove(e) {
    const n = thinkLevels ? thinkLevels.length : 0;
    const rect = e.currentTarget.getBoundingClientRect();
    const trackWidth = rect.width - 28;
    if (n < 2 || trackWidth <= 0) {
      tsHoverIdx = null;
      return;
    }
    const idx = Math.min(Math.max(Math.round(((e.clientX - rect.left - 14) / trackWidth) * (n - 1)), 0), n - 1);
    const center = rect.left + 14 + (trackWidth * idx) / (n - 1);
    tsHoverIdx = Math.abs(e.clientX - center) <= 10 ? idx : null;
  }
  // 原生 range 拖动/点选：离散步进，input 事件即落值（菜单保持展开，ZCode 面板同款）
  function thinkRangeInput(idx) {
    const l = thinkLevels && thinkLevels[idx];
    if (l) pickThinking(l.v, true);
  }
  // 「开」分段：已开就别动档位（把显式档位冲回渠道默认是倒退），只保持菜单展开
  function pickThinkOn() {
    if (!thinkingOn) pickThinking('on', true);
  }
  // 药丸显示：显式档位用该渠道自己的档位名（GLM 官方用词「最高」、DeepSeek「最大」）
  $: pillThinkLabel =
    (thinkLevels && thinkLevels.find((l) => l.v === thinking))?.label || THINK_LABELS[thinking] || '默认';
  // Ctrl+T：思考三态轮换 关→默认→开→关（ZCode 同款快捷键，滑杆面板角上有提示）
  function thinkHotkey(e) {
    if (!e.ctrlKey || e.altKey || e.metaKey || e.shiftKey) return;
    if ((e.key || '').toLowerCase() !== 't') return;
    e.preventDefault();
    if (thinking === 'off') pickThinking('');
    else if (thinking === '') pickThinking('on', true);
    else pickThinking('off');
  }
  // 提示按「生效形态」给（覆盖 > 端点探测），auto/无参数渠道给可操作指引
  $: thinkHint =
    thinkingStyle === 'zai'
      ? 'GLM/豆包 thinking 对象参数'
      : thinkingStyle === 'deepseek'
        ? 'DeepSeek thinking 对象参数'
        : thinkingStyle === 'qwen'
          ? 'Qwen 系 enable_thinking 布尔'
          : thinkingStyle === 'anthropic'
            ? 'Anthropic Messages thinking：低/中/高=预算 20%/50%/80%'
            : thinkingStyle === 'qwen-chat-template'
            ? '本地推理服务 chat_template_kwargs（Qwen3 系模板）'
            : thinkingStyle === 'openai'
              ? 'OpenAI 风格 reasoning_effort 档位'
              : thinkingStyle === 'none'
                ? '形态=不发参数，思考由渠道决定'
                : '该渠道无标准开关参数——关不掉到设置改「思考参数形态」';
  function pickThinking(v, keepOpen = false) {
    thinking = v;
    // 「开启」不收起菜单——档位要等开启后才出现（三态机制）
    if (!keepOpen) thinkMenuOpen = false;
    // 同步进设置（持久化到当前配置，刷新/切会话不丢）
    autoSave({ thinking: v }, 0);
  }

  // —— 输入栏下拉菜单 ——
  // 点外面就收（加号菜单此前只能再点一次加号才收）。
  // 菜单本体和触发钮都在 .bar-menu 里，点它们不收；右栏加号菜单维持原样（点哪都收）。
  function onGlobalClick(e) {
    rpAddOpen = false;
    const t = e.target;
    if (t instanceof Element && t.closest('.bar-menu')) return;
    plusMenuOpen = false;
    abilityMenuOpen = false;
    approvalMenuOpen = false;
  }

  // —— 审批档位菜单（输入栏内）——
  let approvalMenuOpen = false;
  // 四档语义：
  // 只读 = 只读操作自动放行，写/执行逐次询问（shell 恒沙箱）
  // 自动 = 工作区与整台电脑都可动（越界写按高危先问），shell 恒沙箱
  // 完全访问 = 整台电脑可动且免审批，shell 本机直跑（需 config 总开关）
  const APPROVAL_LABELS = { 'read-only': '只读', auto: '自动', 'full-access': '完全访问' };
  // 审批档切换的请求序号：并发乱序时以「最后一次点击」为准，过期响应丢弃（F14）
  let approvalPickSeq = 0;
  async function pickApproval(m) {
    const key = currentSessionId || '@new';
    // 完全访问 = 脱沙箱本机直跑，首次切档前确认一次（记在本机 localStorage，之后不再打扰）
    if (m === 'full-access' && localStorage.getItem('fullAccessAcked') !== '1') {
      const ok = await confirmAsk(
        '完全访问会让命令以你的完整用户权限直接运行（不再受沙箱限制，能读写整台电脑、联网）。\n\n确定切换到完全访问？'
      );
      if (!ok) return;
      localStorage.setItem('fullAccessAcked', '1');
    }
    approvalMode = m;
    approvalBySession = { ...approvalBySession, [key]: m };
    approvalMenuOpen = false;
    localStorage.setItem('approvalMode', m);
    // M1：带当前会话 id——多会话并发时只切本会话的档（不带 session 才改全局默认）
    const seq = ++approvalPickSeq;
    fetch('/api/approval-mode', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ mode: m, session: currentSessionId || '' }),
    })
      .then((r) => r.json())
      .then((r) => {
        if (seq !== approvalPickSeq) return; // 过期响应：之后又有新点击，不许覆盖本地档位
        // 服务端可能把「完全访问」降级为「自动」（config 未开脱沙箱）：以服务端裁决为准
        if (r && r.mode && r.mode !== m) {
          approvalMode = r.mode;
          approvalBySession = { ...approvalBySession, [key]: r.mode };
          localStorage.setItem('approvalMode', r.mode);
        }
      })
      .catch((e) => console.warn('审批档位切换失败', e)); // 原来空吞错，失败至少留痕（F13）
  }

  function handleKey(e) {
    // 中文输入法组词中的 Enter（isComposing/229）只负责确认候选词，不触发发送
    if (e.isComposing || e.keyCode === 229) return;
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      send();
    }
  }
  function autoRows(e) {
    const el = e.target;
    el.style.height = 'auto';
    el.style.height = Math.min(el.scrollHeight, 200) + 'px';
  }

  // 程序化改动 input 后手动回收高度（不触发 input 事件，autoRows 不会跑）
  function resetComposerHeight() {
    const el = composerInputEl;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = Math.min(el.scrollHeight, 200) + 'px';
  }

  function scrollToBottom() {
    if (!chatBody) return;
    chatPinned = true;
    chatBody.scrollTo({ top: chatBody.scrollHeight, behavior: 'smooth' });
  }

  async function loadProviders() {
    const r = await fetch('/api/providers').then((r) => r.json());
    providers = r.providers || [];
  }

  async function loadSettings() {
    const s = await fetch('/api/settings').then((r) => r.json());
    applyState(s);
  }

  // —— 启动流水线韧性（2026-10-27 重启丢轨迹根因）——
  // 桌面壳重启时 webview 可能先于后端就绪：首屏 fetch 直接 Connection refused。
  // 原实现 `await loadProviders(); await loadSettings(); await loadSessions(true);`
  // 第一发失败就整条链中断——侧栏空、未选模型、打开一条空新对话，看起来就像
  // 历史/思考/工具调用全没了（服务其实几秒后就绪，手动刷新即可恢复）。
  // 现在先等 /api/version 可用（最多 30s），每一步再带退避重试；仍失败就显式
  // 把错误挂到聊天气泡里，并给「重试」按钮，绝不静默留白。
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  async function waitForService(timeoutMs = 30000) {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      try {
        const r = await fetch('/api/version', { cache: 'no-store' });
        if (r.ok) return true;
      } catch (e) {
        /* 服务还没起：继续等 */
      }
      await sleep(400);
    }
    return false;
  }
  async function retryCall(fn, tries = 5, delay = 400) {
    let last;
    for (let i = 0; i < tries; i++) {
      try {
        return await fn();
      } catch (e) {
        last = e;
        if (i + 1 < tries) await sleep(delay * (i + 1));
      }
    }
    throw last;
  }
  async function initStartupData() {
    startupError = '';
    if (!(await waitForService(30000))) {
      startupError = '[启动失败] 后端服务未就绪（30 秒内 /api/version 无响应）。请点「重试」，或检查服务进程/5173 端口。';
      return false;
    }
    try {
      await retryCall(loadProviders);
    } catch (e) {
      startupError = `[启动警告] 读取供应商失败：${e.message || e}`;
    }
    try {
      await retryCall(loadSettings);
    } catch (e) {
      startupError = `[启动失败] 读取设置失败：${e.message || e}`;
      return false;
    }
    await loadSessions(true, true); // 内部失败会写 startupError（autoPick 分支）；刷新后原地恢复
    return !startupError;
  }

  // 正在编辑的字段不的服务端同步覆盖（2026-10-08 修）：settings_changed（外部改
  // settings.json/agent 自改）、SSE 重连、自身保存响应回来，三条路都会调 applyState，
  // 原来无条件整体重置——用户正在敲的 Base URL/配置名/参数被静默拽回旧值，未保存的
  // 字符全丢。范式同 loadWebState 守 addressBar：焦点在哪个输入框，那个字段就跳过。
  // （只守直接输入的文本框；Select 只有显式点选才变，变了就立即自动保存，无中间态。）
  function editingField(name) {
    try {
      return document.activeElement && document.activeElement.dataset
        ? document.activeElement.dataset.field === name
        : false;
    } catch (e) {
      return false;
    }
  }

  function applyState(s) {
    const prevWorkspace = workspace;
    savedBaseUrl = s.base_url || '';
    if (!editingField('baseUrl')) baseUrl = s.base_url || '';
    model = s.model || '';
    if (!editingField('configName')) configName = s.name || '';
    if (!editingField('ctxLen')) ctxLen = s.context_length || 0;
    if (!editingField('maxOut')) maxOut = s.max_output_tokens || 0;
    if (!editingField('temperature')) temperature = s.temperature ?? null;
    if (!editingField('topP')) topP = s.top_p ?? null;
    providerId = s.provider_id || 'deepseek';
    savedModels = s.models || [];
    thinking = s.thinking || '';
    thinkingStyle = s.thinking_style || 'auto';
    capabilities = s.capabilities || { tools: true, vision: false, video: false, audio: false };
    roles = s.roles || {};
    features = s.features || { roles: true, css_tool: true, galaxy: true, computer: true };
    workspaceAliases = s.workspace_aliases || {};
    workspace = s.workspace || '';
    workspaceFolders = (s.workspace_folders && s.workspace_folders[workspace]) || [];
    // 工作区换了：面板插件（插件制作器的工作区下拉、项目文件树）只在 init 时读一次
    // workspace.info，不通知就一直显示旧项目的路径和文件（2026-10-27 E2E 实锤：
    // 切回原工作区后制作器下拉仍标旧项目为「当前」）。切会话有 session 事件，工作区同理。
    if (
      prevWorkspace &&
      normPath(prevWorkspace).toLowerCase() !== normPath(workspace).toLowerCase()
    ) notifyExtsWorkspace(workspace);
    recentWorkspaces = s.recent_workspaces || [];
    // 圆环常驻：会话未开始时也用配置预算初始化（context_length 或全局默认 96000）
    if (!ctxUsage.window) {
      ctxUsage = { ...ctxUsage, window: s.context_length || 96000 };
    }
    configs = s.configs || [];
    activeConfigId = s.active_config_id || '';
    // key 按配置独占：hint 直接用当前激活配置的，与其他配置无关
    savedKeyUrl = s.key_hint || '';
  }

  // 当前激活配置是否已有 key（随配置切换刷新，不随地址输入实时变化）
  $: typingHint = savedKeyUrl;

  function flashSaved() {
    settingsMsg = '已自动保存 ' + new Date().toTimeString().slice(0, 8);
  }

  // 防抖自动保存设置字段；合并挂起字段，切换配置前必须落盘到当前配置
  function autoSave(fields, delay = 500) {
    clearTimeout(autoSaveTimer);
    pendingSave = { ...(pendingSave || {}), ...fields };
    autoSaveTimer = setTimeout(async () => {
      const body = { ...pendingSave };
      if (apiKey.trim()) {
        body.api_key = apiKey.trim();
      }
      pendingSave = null;
      await sendSettings(body);
    }, delay);
  }

  // 把挂起的防抖保存立即落盘（落给此刻的激活配置，防止串到切换后的配置）
  async function flushPendingSave() {
    clearTimeout(autoSaveTimer);
    if (!pendingSave) return;
    const body = { ...pendingSave };
    pendingSave = null;
    if (apiKey.trim()) body.api_key = apiKey.trim();
    await sendSettings(body);
  }

  async function sendSettings(body) {
    try {
      const r = await fetch('/api/settings', {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      });
      const d = await r.json();
      if (d.error) {
        settingsMsg = `保存失败: ${d.error}`;
        return false;
      }
      apiKey = '';
      applyState(d);
      flashSaved();
      return true;
    } catch (e) {
      settingsMsg = `保存失败: ${e}`;
      return false;
    }
  }

  async function clearKey() {
    await sendSettings({ clear_key: true });
    savedKeyUrl = '';
  }

  // —— 配置切换 / 新建 / 删除 ——
  async function manageConfig(action, id) {
    // 先把挂起的字段改动落给当前配置，再切——防抖的尾巴不能甩到新配置头上
    await flushPendingSave();
    const r = await fetch('/api/configs', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ action, id }),
    });
    const d = await r.json();
    if (d.error) {
      settingsMsg = `操作失败: ${d.error}`;
      return;
    }
    applyState(d);
    flashSaved();
  }

  function newConfig() {
    manageConfig('new');
  }

  async function deleteConfig() {
    if (configs.length <= 1) {
      settingsMsg = '至少保留一套配置';
      return;
    }
    const cur = configs.find((c) => c.id === activeConfigId);
    // 删除类先弹确认
    if (!(await confirmAsk(`删除配置「${cur ? configLabel(cur) : activeConfigId}」？\n该配置的密钥会一并删除，不可恢复。`))) return;
    manageConfig('delete', activeConfigId);
  }

  function onProviderChange() {
    const p = providers.find((x) => x.id === providerId);
    const newBase = p ? p.default_base_url : '';
    const matchesPreset = providers.some((x) => x.default_base_url && x.default_base_url === baseUrl);
    if (!baseUrl || matchesPreset) {
      baseUrl = newBase;
      autoSave({ provider_id: providerId, base_url: baseUrl });
    } else {
      autoSave({ provider_id: providerId });
    }
  }

  // —— 获取模型 ——
  async function fetchModels() {
    settingsMsg = '获取模型中…';
    try {
      const r = await fetch('/api/models', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ base_url: baseUrl, api_key: apiKey }),
      });
      const d = await r.json();
      if (!r.ok) throw new Error(d.error || `HTTP ${r.status}`);
      const models = d.models || [];
      fetchedModels = models;
      modelModalOpen = true;
      // 已选模型必须严格跟着端点这次返回的清单走（2026-10-27 真机实测：端点下架的
      // agnes-2.0-flash 在选完别的模型后还赖在已选列表里）。只在「获取的就是已保存
      // 的那套端点」时收敛——拿别的 base_url 试拉模型不能清掉当前配置的清单；
      // 端点返回空表也不动，避免一次异常响应把配置清空。
      const norm = (u) => (u || '').trim().replace(/\/+$/, '');
      if (models.length && norm(baseUrl) === norm(savedBaseUrl)) {
        await queueModelSave(async () => {
          const keep = savedModels.filter((m) => models.includes(m));
          if (keep.length === savedModels.length) return;
          const prevModels = savedModels;
          const prevModel = model;
          const newModel = keep.includes(model) ? model : keep[0] || '';
          savedModels = keep;
          model = newModel;
          if (!(await sendSettings({ models: keep, model: newModel }))) {
            savedModels = prevModels;
            model = prevModel;
          }
        });
      }
      settingsMsg = '';
    } catch (e) {
      settingsMsg = `获取模型失败: ${e.message}`;
    }
  }

  function isChecked(m) {
    return savedModels.includes(m);
  }

  // 模型勾选的保存串行队列（2026-10-26 实锤「批量选择后已选还是旧的」根因）：
  // 弹窗里每个勾选框 on:change 立即发一个 PUT（全量 models 列表），快速连点时
  // 多个 PUT 并发在途——先落地的旧响应经 applyState 把本地勾选态盖回旧值，
  // 之后的点击又按回退后的列表算全量清单，落盘的就残缺（日志实证：11 连发只剩 1 个）。
  // 串行化 = 每次都基于上一次已落定的状态计算，响应也按序落地，勾选态不再回跳。
  let modelSaveChain = Promise.resolve();
  function queueModelSave(fn) {
    modelSaveChain = modelSaveChain.then(fn, fn);
    return modelSaveChain;
  }

  function toggleModel(m) {
    return queueModelSave(async () => {
      const prevModels = savedModels;
      const prevModel = model;
      // 弹窗里能看见的只有本次拉回的清单：勾选以它为准，端点已下架的旧模型
      // 不参与（已选列表严格跟最后一次选择）
      const base = fetchedModels.length
        ? savedModels.filter((x) => fetchedModels.includes(x))
        : savedModels;
      const list = base.includes(m)
        ? base.filter((x) => x !== m)
        : [...base, m].sort();
      savedModels = list; // 乐观更新
      let newModel = model;
      if (list.length && !list.includes(model)) {
        newModel = list[0];
        model = newModel;
      }
      // A2：能力标记归用户所有，切换/勾选模型不再改写 capabilities；
      // 后端仅在该配置无用户标记历史时按新模型名推导初值
      if (!(await sendSettings({ models: list, model: newModel, api_key: apiKey.trim() || undefined }))) {
        savedModels = prevModels; // F3：保存失败回滚，勾选态不许与后端脱节
        model = prevModel;
      }
    });
  }

  function selectAllModels() {
    return queueModelSave(async () => {
      const prevModels = savedModels;
      const prevModel = model;
      const list = [...fetchedModels].sort();
      savedModels = list;
      let newModel = model;
      if (!list.includes(model) && list.length) {
        newModel = list[0];
        model = newModel;
      }
      // A2：同上，不随选择改写能力标记
      if (!(await sendSettings({ models: list, model: newModel }))) {
        savedModels = prevModels;
        model = prevModel;
      }
    });
  }

  function selectNoModels() {
    return queueModelSave(async () => {
      const prevModels = savedModels;
      const prevModel = model;
      savedModels = [];
      model = '';
      if (!(await sendSettings({ models: [], model: '' }))) {
        savedModels = prevModels;
        model = prevModel;
      }
    });
  }

  // —— 模型快选 ——
  async function pickModel(m) {
    const prevModel = model;
    model = m;
    modelMenuOpen = false;
    // 能力标记完全由用户手动勾选，切换模型不改写
    if (!(await sendSettings({ model: m }))) {
      model = prevModel;
    }
  }

  // 快选菜单里跨配置选模型：先切激活配置，再把该配置的模型设为目标
  async function pickCrossModel(cfg, m) {
    modelMenuOpen = false;
    if (cfg.id === activeConfigId) {
      await pickModel(m);
      return;
    }
    await manageConfig('apply', cfg.id);
    if (model !== m) await sendSettings({ model: m });
  }

  // —— 输入框引用高亮（2026-11-01 用户对照 ZCode：引用能力/技能后那段文字要用强调色）——
  // textarea 不支持局部着色，用同字号同排版的浮层叠在上面对齐渲染：引用 token 上色，
  // 其余字符用透明色占位只为保住换行位置。**字体/行高/内边距/断词规则必须和
  // .composer-input 一字不差**，否则浮层字和输入框光标会错位。
  let composerHlEl = null;
  let composerHl = '';
  // 中文输入法拼写期间让输入框自己显示文字（浮层隐藏）：否则 IME 候选/下划线看不见
  let composerComposing = false;
  const COMPOSER_REF_RE = /(^|\s)([/@][^\s]+)/g;
  function escHtml(s) {
    return s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
  }
  function highlightComposer(text) {
    const s = text || '';
    let out = '';
    let last = 0;
    COMPOSER_REF_RE.lastIndex = 0;
    let m;
    while ((m = COMPOSER_REF_RE.exec(s))) {
      const start = m.index + m[1].length;
      out += escHtml(s.slice(last, start)) + `<span class="ref">${escHtml(m[2])}</span>`;
      last = start + m[2].length;
    }
    out += escHtml(s.slice(last));
    return out;
  }
  $: composerHl = highlightComposer(input);
  // 发出去之后也要保留颜色：用户气泡里的 /技能、@引用 用同一套
  // token 规则与强调色重绘。正文是纯文本，highlightComposer 里已 escape-first，可用 {@html}。
  function userMsgHtml(content) {
    return highlightComposer(typeof content === 'string' ? content : String(content ?? ''));
  }
  function syncComposerHl() {
    if (composerHlEl && composerInputEl) composerHlEl.scrollTop = composerInputEl.scrollTop;
  }

  // —— 加号菜单：光标处插入文本（@ 引用 / 能力模板 / $ 技能）——
  async function insertAtCursor(text) {
    const el = composerInputEl;
    if (!el) {
      input += text;
      return;
    }
    const s = el.selectionStart ?? input.length;
    const epos = el.selectionEnd ?? input.length;
    input = input.slice(0, s) + text + input.slice(epos);
    await tick(); // 等 Svelte 把新 value 刷进 DOM 后再恢复光标
    el.focus();
    const caret = s + text.length;
    el.setSelectionRange(caret, caret);
    resetComposerHeight();
  }

  // @ 上下文选择（用户 2026-10-02 改版：@ = 上下文，可挑**对话**加入、也可挑文件）：
  // 文件走老路插 @相对路径（模型自己读）；对话走 /api/sessions/transcript 拉只读转写，
  // 发送时整块拼在消息正文后面（同 @文件「界面短引用、模型拿全文」的口径）。
  let ctxTab = 'files'; // 'conv' | 'files'
  let ctxSessions = [];
  let ctxSessionsLoading = false;
  let pendingConvs = []; // 待发送的引用对话 [{ id, title, text }]
  async function openCtxPicker() {
    plusMenuOpen = false;
    ctxPickerOpen = true;
    ctxQuery = '';
    ctxLoading = true;
    ctxTab = 'files';
    loadCtxSessions();
    try {
      const r = await fetch('/api/workspace/files').then((r) => r.json());
      ctxFiles = r.files || [];
    } catch (e) {
      ctxFiles = [];
    }
    ctxLoading = false;
  }
  async function loadCtxSessions() {
    ctxSessionsLoading = true;
    try {
      const r = await fetch('/api/sessions?scope=workspace').then((r) => r.json());
      ctxSessions = (r.sessions || []).filter((s) => s.id !== currentSessionId);
    } catch (e) {
      ctxSessions = [];
    }
    ctxSessionsLoading = false;
  }
  function pickCtxFile(rel) {
    ctxPickerOpen = false;
    insertAtCursor(`@${rel} `);
  }
  async function pickCtxSession(s) {
    ctxPickerOpen = false;
    try {
      const r = await fetch(`/api/sessions/transcript?id=${encodeURIComponent(s.id)}`).then((r) => r.json());
      const lines = (r.messages || []).map((m) => `${m.role === 'user' ? '用户' : '助手'}: ${m.content}`);
      const text = lines.join('\n\n');
      pendingConvs = [
        ...pendingConvs.filter((c) => c.id !== s.id),
        { id: s.id, title: r.title || s.title || s.id, text, omitted: !!r.omitted },
      ];
    } catch (e) {
      ctxMsg = `读取对话失败: ${e.message || e}`;
    }
  }
  function removePendingConv(id) {
    pendingConvs = pendingConvs.filter((c) => c.id !== id);
  }
  /// 引用对话块：拼在发送正文之后（放进 message 与本地 history，模型与界面同一份）
  function convBlocks(convs) {
    if (!convs || !convs.length) return '';
    return (
      '\n\n' +
      convs
        .map(
          (c) =>
            `【引用对话：${c.title}】\n${c.text}${c.omitted ? '\n（该对话较长，此处只保留了最近部分）' : ''}\n【引用对话结束】`
        )
        .join('\n\n')
    );
  }

  // 2026-10-04 修：原来 loadMcp/loadSkills 不 await，菜单先于数据打开——第一次开
  // 能力菜单 MCP/技能组永远空的（数据到了要开第二次才见）。改成等加载完再开。
  async function openAbilityMenu() {
    plusMenuOpen = false;
    if (!skillsData.length) await loadSkills();
    if (!mcpServers.length && !wasmPlugins.length) await loadMcp();
    abilityMenuOpen = true;
  }
  // 「/」菜单分组（用户 2026-10-02 第三/四轮）：技能=自己建的非插件技能；
  // 插件=**全部已启用插件**（第四轮实锤「只列了那几个，全都要呀」）。
  // 点任意一行都往输入框插一个 /引用（带技能的插技能名，纯面板插件插插件标题；
  // 2026-10-02 真机实测「很多插件点了不插入输入框」后统一）——面板归右侧栏「+」菜单。
  $: ownSkills = (skillsData || []).filter((sk) => sk.source !== 'plugin' && sk.enabled !== false);
  $: pluginSkills = (skillsData || []).filter((sk) => sk.source === 'plugin' && sk.enabled !== false);
  let pluginMenu = ''; // 展开中的插件名（/ 菜单）
  function pluginSkillsOf(name) {
    return pluginSkills.filter((sk) => sk.plugin === name);
  }
  // 插件引用键：标题去掉空白——「Word 文档」这类带空格的名字插进去会把 /引用 切断。
  // 后端 index_prompt 用同一口径列「可用插件」，插进输入框的 /引用 在模型那儿才解得开。
  function pluginRef(e) {
    return (e.title || e.name || '').replace(/\s+/g, '');
  }

  // —— 顶栏右侧：终端面板 + 产物/预览右侧栏 ——
  let terminalOpen = false;
  let termLines = []; // { cmd, out, ok }
  let termInput = '';
  let termRunning = false;
  // 右侧面板开合：**打开软件默认关着**（面板吃屏幕）——
  // 会话内刷新保持（sessionStorage 跟标签页同生命），重开软件从关着开始。
  // 旧 localStorage 键清掉：那套是"找不到内置浏览器"时期的默认开+持久化，
  // 用户已熟悉入口后反转，别留个 rpOpen=1 的误导痕迹。
  localStorage.removeItem('rpOpen');
  let rightPanelOpen = sessionStorage.getItem('rpOpen') === '1';
  // —— 右侧边栏 = 统一标签页 ——
  // 产物/网页/插件面板（时间线/项目速览/用量）/文件预览全在这里开标签页；标签可关、
  // 尾部 + 号加面板、关光了显示「打开标签页」选择页。
  // **默认零标签页**——不要一上来就开五个，侧栏打开时显示
  // 「打开标签页」选择页，点卡片才开。持久化：rpTabs（开着哪些）+ rpTab（激活 key）。
  // 「git」= 审查 + 提交合成一个标签（git 提交放进侧边栏跟审查一起，
  // 聊天区底部那根常驻条撤掉）。key 从老 'review' 迁过来，见下面 rpTabs 加载处的迁移。
  const RP_BUILTIN = [
    { key: 'artifacts', kind: 'artifacts', title: '产物' },
    { key: 'git', kind: 'git', title: 'git' },
    { key: 'web', kind: 'web', title: '网页' },
  ];
  // 侧栏只收编这三个宿主面板插件（时间线/项目文件/用量）——其余插件哪怕 manifest 声明
  // dock 也不进侧栏，+ 菜单与选择页都不出（装的插件别全列出来，六个够了）
  const RP_EXT_PANELS = new Set(['trace', 'board', 'usage']);
  function rpDefaultTabs() {
    return [];
  }
  // 页签列表与面板开合同一生命周期：**会话级**（sessionStorage）——刷新本页保留，重开
  // 软件从零开始（用户 2026-10-30 复述 10-10 的决定：打开侧栏不该自己冒出任何面板）。
  // 曾经存 localStorage：面板关着、页签列表却在重启后原样恢复，用户点开侧栏时
  // 「上次开着的那个面板」就成了默认值（实锤：重启后点开侧栏直接是「用量价格」）。
  // 旧 localStorage 键一次性清掉，避免旧记录继续误导。
  try {
    localStorage.removeItem('rpTabs');
    localStorage.removeItem('rpTab');
  } catch {}
  let rpTabs = (() => {
    try {
      const raw = JSON.parse(sessionStorage.getItem('rpTabs') || 'null');
      if (Array.isArray(raw) && raw.length) {
        return raw
          .map((t) => (t && t.key === 'review' ? { ...t, key: 'git', kind: 'git', title: 'git' } : t))
          .filter((t) => t && t.key && t.kind && (t.kind !== 'ext' || RP_EXT_PANELS.has(t.name)));
      }
    } catch {}
    return rpDefaultTabs();
  })();
  // 激活页签：只在恢复出来的列表里找，对不上就退回第一个（零页签时为空 = 选择页）。
  // 零标签页时不要把旧激活 key 的 ext 面板推回来——那会把「默认零标签」破功。
  let rpTab = (() => {
    const want = (sessionStorage.getItem('rpTab') || '').replace(/^review$/, 'git'); // 老「审查」激活标签跟着迁
    return rpTabs.some((t) => t.key === want) ? want : (rpTabs[0] || {}).key || '';
  })();
  let rpAddOpen = false; // 标签栏 + 号的添加菜单
  let rpFiles = [];
  // 「产物」空态/失败的原因（resolveBaseline 没基线、读取出错）——空也要说人话
  let rpArtifactsNote = '';
  // —— git 面板（审查+提交合成一个标签）——
  // 范围选择器：task=本任务改动（影子仓库检查点）、
  // work=未提交改动（工作区真仓库相对 HEAD）——同时是提交的暂存区（勾选即入选）
  let rvSource = 'task';
  let rvBaseline = ''; // source='task' 时的基线；点「审查」进面板会带上它自己的
  let rvFiles = [];
  let rvLoading = false;
  let rvError = '';
  let rvOpen = {}; // path → 是否展开 diff
  let rvDiff = {}; // path → { loading, text, error }
  let rvMenu = null; // 行右键菜单 { x, y, path }
  // 基线陈旧警告（后端 stale_subtree_warning）：基线里的顶层目录已不在当前工作区，
  // 清单里的「删除」很可能是目录被挪走造成的，不是本任务改的
  let rvWarn = '';
  // 产物面板同源警告（与 rvWarn 同一句话，两个面板各自展示）
  let rpWarn = '';
  // 基线过期（2026-10-31 真机实测「产物不是当前工作区的」）：stale 警告触发时任务档
  // 清单全是基线残留鬼影，不再展示，给「重新以当前状态为起点」按钮重拍
  let rpStale = false; // 已废弃：过期气泡机制整体撤除（2026-10-31），保留变量防旧引用报错
  let rvStale = false;
  let rpStaleCount = 0;
  let rvStaleCount = 0;
  let rpStaleClosed = false; // 用户关掉气泡后本次不再打扰（切会话/工作区即重置）
  let rvStaleClosed = false;
  let rpStaleErr = ''; // 重拍失败就地可见（此前藏在看不见的 note 里=用户「点了没用」）
  let rvStaleErr = '';
  // 用户手动重拍的基线：优先级压过消息流里的旧检查点（会话/工作区切换时清掉）
  let manualBaseline = '';
  // 打开 git 面板并把某个文件展开（产物行「审查」按钮用；变更条撤了，入口只剩产物行）。
  // 审查不是另开的 diff 标签，就是面板里那一行展开。
  function openReviewDiff(path, baseline) {
    if (!path) return;
    if (!rpTabs.some((t) => t.kind === 'git')) {
      rpTabs = [...rpTabs, { key: 'git', kind: 'git', title: 'git' }];
    }
    rightPanelOpen = true;
    terminalOpen = false;
    const needReload = rvSource === 'task' && baseline && baseline !== rvBaseline;
    if (needReload) {
      // 带来的基线与面板当前用的不是同一个 → 换基线重拉，别拿旧的冒充
      rvBaseline = baseline;
      rvFiles = [];
      rvOpen = { [path]: true };
      loadReview();
    } else {
      rvOpen = { ...rvOpen, [path]: true };
      const f = rvFiles.find((x) => x.path === path);
      if (f) loadRvDiff(f);
    }
    activateRpTab('git');
  }
  // git 面板文件清单（source 切换：本任务改动 / 未提交改动；未提交档即提交暂存区）
  async function loadReview() {
    rvLoading = true;
    rvError = '';
    rvWarn = '';
    if (rvSource === 'work') {
      try {
        const r = await fetch('/api/changes?origin=workspace').then((x) => x.json());
        if (r.error) {
          rvFiles = [];
          rvError = r.error;
        } else {
          rvFiles = r.files || [];
          rvWarn = r.warning || '';
          initCommitSel(rvFiles); // 清单换了，勾选态跟着重建（新文件默认按未跟踪不勾）
        }
      } catch (e) {
        rvFiles = [];
        rvError = e.message || String(e);
      }
      rvLoading = false;
      rvLoadOpenDiffs();
      return;
    }
    // 本任务改动：没有基线先按 resolveBaseline 兜底一套（与产物同源）
    if (!rvBaseline) {
      const { id } = await resolveBaseline();
      rvBaseline = id || '';
    }
    if (!rvBaseline) {
      rvFiles = [];
      rvLoading = false;
      return;
    }
    try {
      const r = await fetch(`/api/changes?baseline=${encodeURIComponent(rvBaseline)}`).then((x) => x.json());
      if (r.error) {
        rvFiles = [];
        rvError = r.error;
      } else {
        rvFiles = r.files || [];
        // 基线过期气泡已撤（「点了没用/一直留着」）：
        // 删除类条目对「这个任务改了什么」没意义，静默过滤，不再打扰用户确认。
        rvFiles = rvFiles.filter((f) => f.status !== 'D');
        rvWarn = '';
        rvStale = false;
      }
    } catch (e) {
      rvFiles = [];
      rvError = e.message || String(e);
    }
    rvLoading = false;
    rvLoadOpenDiffs();
  }
  // 已被标记展开、但 diff 还没拉过的行补拉（变更条「审查」先置展开态、清单后到的情形）
  function rvLoadOpenDiffs() {
    for (const f of rvFiles) {
      if (rvOpen[f.path] && !rvDiff[f.path]) loadRvDiff(f);
    }
  }
  function toggleRv(f) {
    const open = !rvOpen[f.path];
    rvOpen = { ...rvOpen, [f.path]: open };
    if (open) loadRvDiff(f);
  }
  async function loadRvDiff(f) {
    if (!f || !f.path) return;
    const hit = rvDiff[f.path] || {};
    if (hit.loading || (hit.text !== undefined && !hit.error)) return;
    rvDiff = { ...rvDiff, [f.path]: { loading: true } };
    const url =
      rvSource === 'work'
        ? `/api/review/diff?origin=workspace&path=${encodeURIComponent(f.path)}`
        : `/api/review/diff?baseline=${encodeURIComponent(rvBaseline)}&path=${encodeURIComponent(f.path)}`;
    try {
      const r = await fetch(url).then((x) => x.json());
      rvDiff = { ...rvDiff, [f.path]: r.ok ? { loading: false, text: r.diff || '', error: '' } : { loading: false, text: '', error: r.error || '读取失败' } };
    } catch (e) {
      rvDiff = { ...rvDiff, [f.path]: { loading: false, text: '', error: e.message || String(e) } };
    }
  }
  // 统一 diff 文本 → 带行号的渲染行（左右两个行号列 + 增删着色）
  function rvParse(text) {
    const out = [];
    let oldNo = 0;
    let newNo = 0;
    for (const ln of String(text || '').split('\n')) {
      if (/^(diff --git|index |old mode|new mode|similarity index|dissimilarity index|rename from|rename to|copy from|copy to|deleted file|new file|--- |\+\+\+ )/.test(ln)) {
        out.push({ t: 'meta', s: ln });
        continue;
      }
      const m = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/.exec(ln);
      if (m) {
        oldNo = Number(m[1]);
        newNo = Number(m[3]);
        out.push({ t: 'hunk', s: ln });
        continue;
      }
      if (ln.startsWith('\\')) {
        out.push({ t: 'meta', s: ln });
      } else if (ln.startsWith('+')) {
        out.push({ t: 'add', o: '', n: newNo, s: ln.slice(1) });
        newNo += 1;
      } else if (ln.startsWith('-')) {
        out.push({ t: 'del', o: oldNo, n: '', s: ln.slice(1) });
        oldNo += 1;
      } else {
        out.push({ t: 'ctx', o: oldNo, n: newNo, s: ln.startsWith(' ') ? ln.slice(1) : ln });
        oldNo += 1;
        newNo += 1;
      }
    }
    return out;
  }
  let rpLoading = false;
  let rpPreviewPath = ''; // 最近预览的文件（产物列表高亮用）
  let rpPreviewCache = {}; // path → { loading, content, error, editing, draft, base, editNote, saving, savedAt }（标签切换不重拉）

  // —— 文件预览的未保存修改：刷新/关标签/重开程序都不丢 ——
  // 按「工作区 + 相对路径」存（不同工作区的同名文件不串）；同时存编辑起点内容。
  // 恢复时若磁盘内容已经变了，仍然恢复草稿，但明确提示「保存会覆盖」。
  const FILE_DRAFT_KEY = 'pawchFileDrafts';
  function fileDraftKey(path) {
    return `${workspace || ''}::${path}`;
  }
  function fileDraftMap() {
    try {
      const m = JSON.parse(localStorage.getItem(FILE_DRAFT_KEY) || '{}');
      return m && typeof m === 'object' ? m : {};
    } catch {
      return {};
    }
  }
  function fileDraftWrite(map) {
    try {
      localStorage.setItem(FILE_DRAFT_KEY, JSON.stringify(map));
    } catch {
      /* 配额满：本次不落盘 */
    }
  }
  function forgetPreviewDraft(path) {
    const map = fileDraftMap();
    const k = fileDraftKey(path);
    if (map[k]) {
      delete map[k];
      fileDraftWrite(map);
    }
  }
  /// 关页/刷新前把「正在编辑且有改动」的标签落盘；已保存/已取消的清掉。
  function persistFileDrafts() {
    const map = fileDraftMap();
    const now = Date.now();
    for (const [path, c] of Object.entries(rpPreviewCache)) {
      if (!c) continue;
      const k = fileDraftKey(path);
      if (c.editing && typeof c.draft === 'string' && c.draft !== (c.content || '')) {
        map[k] = { draft: c.draft, base: typeof c.base === 'string' ? c.base : c.content || '', ts: now };
      } else if (!c.editing) {
        delete map[k];
      }
    }
    let raw = JSON.stringify(map);
    if (raw.length > 2_000_000) {
      // 太大：只留最近改的一条；仍超就整批放弃（不把 localStorage 撑爆）
      const latest = Object.entries(map).sort((a, b) => (b[1].ts || 0) - (a[1].ts || 0))[0];
      const trimmed = latest ? { [latest[0]]: latest[1] } : {};
      fileDraftWrite(JSON.stringify(trimmed).length <= 2_000_000 ? trimmed : {});
      return;
    }
    fileDraftWrite(map);
  }
  function restorePreviewDraft(path, content) {
    const map = fileDraftMap();
    const k = fileDraftKey(path);
    const e = map[k];
    if (!e || typeof e.draft !== 'string') return;
    if (e.draft === content) {
      delete map[k];
      fileDraftWrite(map);
      return;
    }
    const stale = typeof e.base === 'string' && e.base !== content;
    rpSet(path, {
      editing: true,
      draft: e.draft,
      base: content,
      editNote: stale
        ? '磁盘上的文件已变化；这是刷新前未保存的草稿，保存会覆盖磁盘上的新内容'
        : '已恢复刷新前未保存的修改',
    });
  }
  let dockPaneFrames = {}; // 插件标签的 iframe（bind:this），按插件名

  // —— 网页（右侧面板「网页」页签）——
  // 截图轮询改 screencast 帧端点（内存直出）；面板可见时 600ms 自动刷帧
  // 桌面壳在跑时，页面是壳窗口里的 child webview（真页面真交互）——面板只留
  //      占位区并把矩形报给后端（壳里的 webview 摆到这块）；浏览器直开 5173 时
  //      仍是 screencast 帧流，两条路径由后端 state.mode + 是否在 Tauri 里共同决定。
  let webTabs = []; // { id, agent, title, url, loading, suspended, visible }
  // —— 思考/工具：过程块的展开态（2026-10-30 排布重做）——
  // 思考与工具按真实发生顺序合成「过程块」；点过程行展开。流式期间默认展开，
  // 收尾后默认收起（跑完只留结尾语）。
  let processOpenMap = {}; // `${msgId}:${blockIndex}` → true=展开
  // 单段思考的展开态：流式中默认展开，思考一结束（done）默认自动折叠；
  // 用户点过就按用户的选择记（每次思考完毕要自动折叠）。
  let thinkOpenMap = {}; // `${msgId}:${blockIndex}:${itemIndex}` → true=展开
  function toggleThink(key, currentlyOpen) {
    thinkOpenMap = { ...thinkOpenMap, [key]: !currentlyOpen };
  }
  // 思考框自动跟随：新分片贴底；滚轮往上滚 → 暂停跟随（出「回到底部」按钮）
  let thinkFollowMap = {}; // `${msgId}:${blockIndex}:${itemIndex}` → 是否跟随
  let thinkBoxEls = {}; // 同上 → 思考滚动容器（bind:this）
  // 工具详情折叠（`${msgId}:${toolIndex}` → true=展开）：默认收起，点行展开参数/结果
  let toolFold = {};
  // —— 任务清单 dock（贴输入框上方、展开向上、与提问上下堆叠）——
  // 清单语义=当前会话最后一次 todo 调用（整表替换）。跑着且有未完项时默认展开；
  // 任务结束自动收成一行；用户可手动折叠，或点 × 收起整块（新任务/新会话再出现）。
  let todoDockPinned = null; // null=跟随任务状态；true/false=用户手动展开/收起
  let todoDockDismissed = false; // 用户叉掉：本任务内不再自动冒出
  let todoDockKey = null; // 上一次同步的会话 key（切会话重置手动态）
  let todoDockPrevStream = false; // 上一次同步的流式态（起/止时重置手动态）
  $: currentTodo = latestTodoInMessages(messages);
  $: todoDoneN = currentTodo ? currentTodo.todos.filter((t) => t.status === 'completed').length : 0;
  $: todoHasUnfinished = !!currentTodo && currentTodo.todos.some((t) => t.status !== 'completed');
  $: todoCur = currentTodo
    ? currentTodo.todos.find((t) => t.status === 'in_progress') ||
      currentTodo.todos.find((t) => t.status !== 'completed') ||
      currentTodo.todos[currentTodo.todos.length - 1]
    : null;
  $: todoDockVisible = !!currentTodo && !todoDockDismissed;
  $: todoDockOpen = todoDockVisible && (todoDockPinned !== null ? todoDockPinned : streaming && todoHasUnfinished);
  function toggleTodoDock() {
    todoDockPinned = !todoDockOpen;
  }
  function dismissTodoDock() {
    todoDockDismissed = true;
  }
  // 提问只在该用户作答时出现（pendingQuestions 的交互卡）；答完立即消失——
  // 不进对话流、不留记录卡（「问题答完为什么还在」）。
  function toggleProcess(key, currentlyOpen) {
    processOpenMap = { ...processOpenMap, [key]: !currentlyOpen };
  }
  function toggleToolDetail(msgId, idx, currentlyOpen) {
    toolFold = { ...toolFold, [`${msgId}:${idx}`]: !currentlyOpen };
  }
  let nowTick = Date.now(); // 实时计时滴答（流式期间每秒更新）
  let tickTimer = null;
  $: anyStreaming = Object.values(sessionMsgs).some((arr) => arr.some((m) => m.role === 'assistant' && m.streaming));
  $: if (anyStreaming && !tickTimer) tickTimer = setInterval(() => (nowTick = Date.now()), 1000);
  $: if (!anyStreaming && tickTimer) {
    clearInterval(tickTimer);
    tickTimer = null;
  }
  /// 轮收尾：停表（工作用时冻结；还开着的思考阶段也关表累加），过程块默认收起、留结尾语
  function endTurn(m) {
    const closed = m.thinkPhaseStart ? Date.now() - m.thinkPhaseStart : 0;
    return {
      ...m,
      streaming: false,
      parts: closed ? settleThinking(m.parts, closed) : m.parts,
      workMs: m.workMs || (m.workStartedAt ? Date.now() - m.workStartedAt : 0),
      thinkMs: (m.thinkMs || 0) + closed,
      thinkPhaseStart: 0,
      // 轮次终结 = 挂着的提问也终结了（回答/跳过/停止）：提问卡不能在 dock 里挂着等
      question: m.question && m.question.answered === null ? { ...m.question, answered: { dismissed: true } } : m.question,
    };
  }
  /// 思考框自动跟随：内容增长时贴底；用户上滚 → 停（followchange 通知界面）；
  /// 滚回最底 → 恢复。只在流式思考（active=true）时启动——历史里展开一段长思考
  /// 应该从顶部读起，不能一打开就跳到底。
  /// 2026-10-31 修「上滑被强行拽回」：靠「是否贴底」判意图在流式下永远输——
  /// MutationObserver 每次新内容都把位置拽回底部，scroll 事件到达时几何上总在
  /// 底部。改为滚轮/触摸一出手立即停跟（同步，先于任何拽回），scroll 事件只按
  /// 方向判（向上=用户、向下到底=恢复），跟随拽回不误伤。
  function thinkFollow(node, active = true) {
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
  function jumpThink(key) {
    const el = thinkBoxEls[key];
    if (el) el.scrollTop = el.scrollHeight;
  }
  /// 复制到剪贴板：优先异步 Clipboard API，拒绝/不可用时退回 execCommand
  /// （无头/权限被拒时前者会失败，不能让「复制」变摆设）
  async function copyText(text) {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      const ta = document.createElement('textarea');
      ta.value = text;
      ta.style.position = 'fixed';
      ta.style.opacity = '0';
      document.body.appendChild(ta);
      ta.select();
      let ok = false;
      try {
        ok = document.execCommand('copy');
      } catch {}
      ta.remove();
      return ok;
    }
  }
  /// 回复正文的点击委托：代码块「复制」与站内文件引用（doc-cite）落预览标签。
  async function onContentClick(e) {
    const t = e.target instanceof HTMLElement ? e.target : null;
    if (!t) return;
    const copy = t.closest('button.md-copy');
    if (copy) {
      e.preventDefault();
      const pre = copy.closest('.md-codeblock')?.querySelector('pre');
      if (!pre) return;
      const old = copy.textContent;
      copy.textContent = (await copyText(pre.innerText)) ? '已复制' : '复制失败';
      setTimeout(() => (copy.textContent = old || '复制'), 1200);
      return;
    }
    const img = t.closest('img.md-img');
    if (img) {
      e.preventDefault();
      previewImageModal = img.src;
      return;
    }
    const a = t.closest('a.doc-cite');
    if (!a) return;
    e.preventDefault();
    const p = new URL(a.href, location.origin).searchParams.get('path');
    if (p) openPreviewTab(p);
  }
  let webActiveId = '';
  let webAddress = '';
  let webBusy = false;
  let webError = ''; // 建页/导航失败的面板内提示（不再静默）
  let webShotT = 0; // 帧缓存穿透参数（自动刷新与动作后递增触发 <img> 重载）
  let webBodyW = 0;
  let webBodyH = 0; // 面板内容区实际尺寸（驱动浏览器视口跟随，截图铺满无黑边）
  let webBodyEl = null; // 面板内容区元素（：量矩形给壳里的 child webview 定位）
  let webStateReady = false; // 浏览器状态已加载过一次（就绪前不量矩形：web-tabbar 行随标签增删整行出现/消失，未定型前量的都是瞬态矩形）
  let webMode = 'headless'; // 后端宿主模式：shell（壳内真 webview）/ headless（帧流）
  let webMenuOpen = false; // 「⋯」菜单（仅无头模式的 HTML 下拉；壳模式是壳里浮层，前端不记状态）
  // 壳模式：「⋯」菜单是壳里一个**透明置顶的 webview**，浮在网页之上——网页纹丝不动、
  // 不被遮也不被藏。前端把面板矩形和按钮矩形交给服务端，壳负责建浮层；
  // 浮层自己关闭（点项/backdrop/Esc）时前端不知道，所以开合状态以服务端为准——这里
  // 的点击一律发 toggle。无头模式没有壳：菜单就是下面的 HTML 下拉（盖在截图上没问题）。
  function webMenuOverlay(body) {
    if (!webNative) return;
    webApi('/api/browser/overlay', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    }).catch(() => {});
  }
  function webToggleMenu(e) {
    if (!webNative) {
      webMenuOpen = !webMenuOpen; // 无头：HTML 下拉本地切换
      return;
    }
    // 壳模式：面板矩形 + ⋯ 按钮矩形（都是窗口坐标）交给后端
    const p = document.querySelector('.right-panel');
    const btn = e && e.currentTarget;
    if (!p || !btn) return;
    const pr = p.getBoundingClientRect();
    const br = btn.getBoundingClientRect();
    webMenuOverlay({
      toggle: true,
      panel: { x: pr.x, y: pr.y, w: pr.width, h: pr.height },
      anchor: { x: br.x, y: br.y, w: br.width, h: br.height },
    });
  }
  function webCloseMenu() {
    if (webNative) webMenuOverlay({ open: false });
    webMenuOpen = false;
  }
  // 离开网页页签/关面板：壳模式让壳关浮层（幂等），无头模式收起 HTML 下拉
  $: if (rpTab !== 'web' || !rightPanelOpen) {
    if (webNative) webMenuOverlay({ open: false });
    else if (webMenuOpen) webMenuOpen = false;
  }
  let webDialog = null; // 页面 JS 弹窗（alert/confirm/prompt）：面板内确定/取消，不卡死页面
  // 是否跑在 Tauri 壳里（决定壳内 webview 能不能落到本客户端窗口）
  function inTauriShell() {
    return typeof window !== 'undefined' && !!(window.__TAURI_INTERNALS__ || window.__TAURI__);
  }
  // 原生托管：壳模式 + 本客户端就是壳 → 面板内容区让给 child webview
  $: webNative = webMode === 'shell' && inTauriShell();
  // 壳 webview 弹窗让位（2026-10-31 用户给 ZCode 截图对照：「有弹窗的时候浏览器画面也在」）：
  // 壳里 child webview 是原生窗口，弹窗（DOM）永远盖不住它。做法：弹窗打开时服务端把
  // webview **移出屏幕外保活渲染**（visible 不关——IsVisible=false 会暂停渲染），
  // 前端切到帧流模式（同无头模式 600ms 一帧），画面实时垫在弹窗下；弹窗关闭立即
  // 恢复原位与原生交互。若 offscreen 不产帧（实测决定），降级为整块藏。
  let shellModalLive = false; // true = 弹窗态：webview 已移出屏幕，面板走帧流
  let shellSuppressSent = false;
  function shellModalOn() {
    if (!webNative || !webActive || rpTab !== 'web') return;
    shellModalLive = true;
    shellSuppressSent = true;
    webViewportKey = ''; // 冻结常规矩形上报（期间 webSyncViewport 直接 return）
    webApi('/api/browser/shell-suppress', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ on: true }),
    }).catch(() => {});
    webShotT += 1; // 立即拉一帧，不等 600ms 轮询
  }
  function shellModalOff() {
    if (!shellSuppressSent && !shellModalLive) return;
    shellModalLive = false;
    if (shellSuppressSent) {
      shellSuppressSent = false;
      webApi('/api/browser/shell-suppress', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ on: false }),
      }).catch(() => {});
    }
    webViewportKey = ''; // 强制下个心跳重发矩形（webview 回原位）
    webSyncViewport();
  }
  function shellSuppressSync() {
    // 需要「让位」的 DOM 浮层有两类：弹窗（.modal-backdrop）和标签栏「＋」的添加菜单
    // （.rp-add-menu）。后者 2026-11-01 用户截图实锤：菜单往下弹出正好落在网页画面上，
    // 壳里 child webview 是原生窗口、永远浮在 DOM 之上，菜单下半截被盖住。同样按
    // 弹窗处理——webview 移出屏幕保活 + 面板走帧流，菜单就能浮在画面上。
    const has = !!document.querySelector('.modal-backdrop, .rp-add-menu');
    if (has && !shellModalLive) shellModalOn();
    else if (!has && shellModalLive) shellModalOff();
  }
  $: if (typeof MutationObserver !== 'undefined') {
    // 观察器只挂一次（弹窗开关由回调处理）
    if (!globalThis.__pawchModalObs) {
      globalThis.__pawchModalObs = new MutationObserver(shellSuppressSync);
      globalThis.__pawchModalObs.observe(document.body, { childList: true, subtree: true });
    }
  }
  // 右侧面板宽度（左缘拖拽可调，320–820）
  let rpWidth = Number(localStorage.getItem('rpWidth')) || 420;
  let rpResizing = false;
  function startRpResize(e) {
    if (e.button !== 0) return;
    e.preventDefault();
    rpResizing = true;
    const handle = e.currentTarget;
    try {
      handle.setPointerCapture(e.pointerId);
    } catch (err) {
      /* 捕获失败退回冒泡，不阻断 */
    }
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
    // 合帧：一帧最多写一次宽度（pointermove 本身就可能一帧多次）。写宽度会触发整个
    // 聊天列重排，直接按事件写会让拖拽掉帧、面板跟手也发涩。
    let latestX = 0;
    let rafId = 0;
    const apply = () => {
      rafId = 0;
      rpWidth = Math.min(820, Math.max(320, Math.round(window.innerWidth - latestX)));
    };
    const move = (ev) => {
      latestX = ev.clientX;
      if (!rafId) rafId = requestAnimationFrame(apply);
    };
    const up = () => {
      if (rafId) {
        cancelAnimationFrame(rafId);
        rafId = 0;
        apply();
      }
      rpResizing = false;
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      localStorage.setItem('rpWidth', String(rpWidth));
      // 无头模式的视口同步在拖动期被压掉（见 webSyncViewport），停手补发最后一次；
      // 壳模式拖动中就一直跟着发，这里只是把落定的矩形再确认一遍（去重键挡重复）
      webQueueViewport();
      handle.removeEventListener('pointermove', move);
      handle.removeEventListener('pointerup', up);
      handle.removeEventListener('pointercancel', up);
    };
    handle.addEventListener('pointermove', move);
    handle.addEventListener('pointerup', up);
    handle.addEventListener('pointercancel', up);
  }

  $: webActive = webTabs.find((t) => t.id === webActiveId) || null;
  // 右侧面板开合/标签持久化（换任何路径改这几个状态都会落盘）。
  // rpOpen/rpTabs/rpTab 都走 sessionStorage：刷新本页不关、重开软件从零开始
  //（2026-10-10 默认关着；2026-10-30 补齐页签列表——之前只有开合
  //  是会话级，页签列表还是 localStorage，重启后点开侧栏会自己恢复上次的面板）。
  $: {
    try {
      sessionStorage.setItem('rpOpen', rightPanelOpen ? '1' : '0');
      sessionStorage.setItem('rpTab', rpTab);
      sessionStorage.setItem('rpTabs', JSON.stringify(rpTabs));
    } catch {}
  }
  // 空白页（about:blank）显示空态引导；有地址就渲染截图
  $: webShowShot = !!(webActive && webActive.url && webActive.url !== 'about:blank');
  // 面板「正在展示网页」的布尔态（只用原始值，避免对象标识每 2s 轮询都变导致重复触发）。
  // 后端在面板隐藏时会作废上次矩形（防收起再弹时旧矩形闪现），所以这个态每次
  // false→true（侧边栏重开/空白页加载出来/切回网页页签）都必须重发一次矩形，
  // 去重键先清——不然壳永远等不到矩形，网页不显示（用户实测实锤）。
  $: webShowing = rpTab === 'web' && rightPanelOpen && !!webActiveId && webShowShot;
  $: if (webShowing && webBodyW) {
    webViewportKey = '';
    webQueueViewport();
  }
  // 常规同步路径（尺寸变化/切页；去重键挡重复）：无头模式没有 ResizeObserver，
  // 靠 webActive 随轮询变标识兜底触发——保留它，别让上面那个跃迁响应式取代
  $: if (rpTab === 'web' && rightPanelOpen && webBodyW && webActive) webQueueViewport();

  async function loadWebState(keepActive = true) {
    try {
      const s = await webApi('/api/browser/state', { cache: 'no-store' });
      webTabs = s.tabs || [];
      webMode = s.mode || 'headless';
      webDialog = s.dialog || null;
      // 就绪门：状态到手后 web-tabbar 行才定型（有标签 = 多一行，无标签 = 整行消失），
      // 之前量的矩形都是瞬态（webview 会跳一下再跳回）。就绪后补一发同步——若标签条
      // 增删没引起 web-body 尺寸变化，ResizeObserver 不会开火，这一发是唯一机会。
      if (!webStateReady) {
        webStateReady = true;
        webQueueViewport();
      }
      if (!keepActive || !webTabs.some((t) => t.id === webActiveId)) {
        webActiveId = webTabs.length ? webTabs[webTabs.length - 1].id : '';
      }
      // 地址栏跟随当前标签页。注意不能读 $: webActive（函数内拿到的是本帧旧值），
      // 就地重查；用户正在地址栏打字时（聚焦中）不覆盖。
      // 无活动标签页（最后一个被关）时清空地址栏——否则还挂着已关标签的地址（bug 修复）。
      const act = webTabs.find((t) => t.id === webActiveId) || null;
      const addrEl = document.querySelector('.web-address');
      if (document.activeElement !== addrEl) {
        webAddress = act ? (act.url === 'about:blank' ? '' : act.url || '') : '';
      }
      // 上报可见页：后端据此只给这一页跑 screencast 帧流、挂起其余后台页
      if (webActiveId) webActivate(webActiveId);
    } catch (e) {
      webTabs = [];
      webError = String((e && e.message) || e);
    }
  }

  // 面板可见页上报（驱动 screencast 与后台挂起）
  function webActivate(id) {
    webApi('/api/browser/activate', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ id, native: !!webNative }),
    }).catch(() => {});
  }

  // 面板不看了（切走页签/关面板）：通知后端停帧流、放后台页去挂起；
  // 原生托管下空白页（about:blank）也通知隐藏——壳里那个 webview 要让位给空态引导
  function webNotifyHidden() {
    // 清去重键：面板重新打开时即使矩形没变也要重发一遍——后端在面板收起时会作废
    // 上次矩形（防旧矩形闪现），等新矩形才显示 webview；键不清就永远等不到
    webViewportKey = '';
    webApi('/api/browser/activate', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      // native：只有壳里那个客户端的「不看了」能藏壳里的 webview（壳外浏览器客户端
      // 切走页签不该把壳里的网页一起藏了）
      body: JSON.stringify({ id: '', native: !!webNative }),
    }).catch(() => {});
  }
  $: if (rpTab !== 'web' || !rightPanelOpen || (webNative && !webShowShot)) webNotifyHidden();

  // agent 操作内置浏览器时，面板自动唤起并**跟随 agent 正在操作的页**
  // （「agent 操作时自动唤起右侧边栏」+ 预期「面板跟着 agent 走」）。
  // agent 自己的切页不抢用户视图（服务端 agent_activate 不写 visible_tab），但界面要
  // 能看见 agent 在哪儿干活——常驻每 2s 探一次，agent 最近 10s 有动作就跟随过去。
  let webAgentWatch = null;
  let webAgentSeen = 0; // 已响应过的 agent 触碰时间戳（防同一动作反复弹跳）
  function webStartAgentWatch() {
    // 浏览器插件停用时不开这条常驻轮询：能力已经关了，还每 2s 去撞 403 是白跑
    // （插件开关=能力总闸，关了就什么都不该动）
    if (!browserOn) return;
    if (webAgentWatch) return;
    webAgentWatch = setInterval(async () => {
      try {
        const s = await webApi('/api/browser/state', { cache: 'no-store' });
        // 这条轮询也刷新浏览器状态——就绪门在这里同样要放行：agent 自动唤起面板
        // 走的就是这条路（不经过 loadWebState），不置标志的话视口同步被永远挡死，
        // 拖面板宽度时壳里的 webview 纹丝不动（2026-09-27 真机实测「宽度不同步」）。
        // 注意放在标签清单更新**之后**：补发的那一发必须量到标签条挂出后的定型几何。
        // 宿主模式也随时同步：webMode 以前只在 loadWebState 时更新，而 agent 自动开面板
        // 走的是这条路——不同步的话面板按**无头模式**渲染（截图 img 分支），壳里的真
        // webview 随后才_pop_进来，用户看到的就是「先闪几下」（坏截图/暗区→真页面）。
        // 壳模式面板直接渲染 .web-native 占位，等 webview 就位即可。
        if (s.mode && s.mode !== webMode) webMode = s.mode;
        if (s.dialog !== undefined) webDialog = s.dialog || null;
        // 标签清单随时同步：agent 开新页/导航之后，页签条上的标题和地址必须跟着变。
        // 以前只有 loadWebState 才刷这份清单——agent 操作完面板里留着旧标题，用户看到的
        // 就是「一个 DeepSeek 标签显示着 Google」这种灵异现象
        const tabs = s.tabs || [];
        const stale =
          tabs.length !== webTabs.length ||
          tabs.some((t, i) => {
            const o = webTabs[i];
            return !o || o.id !== t.id || o.title !== t.title || o.url !== t.url || o.agent !== t.agent;
          });
        if (stale) {
          webTabs = tabs;
          // 活动页兜底：onMount 那条加载路径漏了时（面板开着但状态没加载过）、或服务重启后
          // 老 tab id 已失效时，这里补上——不补的话 webActive 一直是 null，面板显示空态、
          // 视口同步也永远不发（壳里 webview 跟着一直藏着）
          if (!tabs.some((t) => t.id === webActiveId)) {
            webActiveId = tabs.length ? tabs[tabs.length - 1].id : '';
          }
          const act = tabs.find((t) => t.id === webActiveId);
          if (act && document.activeElement !== document.querySelector('.web-address')) {
            webAddress = act.url === 'about:blank' ? '' : act.url || '';
          }
        }
        if (!webStateReady) {
          webStateReady = true;
          webQueueViewport();
        }
        if (!s.agentTouched || Date.now() - s.agentTouched > 10000) return;
        if (s.agentTouched <= webAgentSeen) return; // 这次触碰已响应过
        webAgentSeen = s.agentTouched;
        rightPanelOpen = true;
        // 「打开侧栏不自己冒面板」（2026-10-30）之后默认 rpTabs=[]——这里只设 rpTab='web'
        // 会被「可见页签为空→回选择页」的钳制反应式立刻打回 ''，浏览器面板永远出不来
        // （2026-10-31 真机实测「只弹右侧栏没开浏览器面板」）。自动唤起=把 web 页签
        // 真加进列表再激活（addRpTab 幂等：已在列表就只激活）。
        addRpTab({ key: 'web', kind: 'web', title: '网页' });
        if (s.agentTab && s.agentTab !== webActiveId && (s.tabs || []).some((t) => t.id === s.agentTab)) {
          webActiveId = s.agentTab;
          const act = (s.tabs || []).find((t) => t.id === webActiveId);
          if (act && document.activeElement !== document.querySelector('.web-address')) {
            webAddress = act.url === 'about:blank' ? '' : act.url || '';
          }
          webActivate(webActiveId);
        }
      } catch {}
    }, 2000);
  }
  webStartAgentWatch();
  // 能力开着→关的过渡：已在跑的轮询要收掉，不然插件停用后它还在撞 403。
  // 开着的反向不用管（webStartAgentWatch 幂等，面板那侧的响应式会再拉）。
  $: if (!browserOn && webAgentWatch) {
    clearInterval(webAgentWatch);
    webAgentWatch = null;
  } else if (browserOn && !webAgentWatch) {
    webStartAgentWatch();
  }
  onDestroy(() => {
    if (webAgentWatch) clearInterval(webAgentWatch); // N2：常开轮询随组件销毁收掉
  });


  // 帧自动刷新：面板开着且有页面时 600ms 一帧（screencast 内存直出，零 CDP 往返）。
  // 原生托管不看帧——壳里就是真页面，帧流停掉省编码开销
  let webFrameTimer = null;
  $: if (rpTab === 'web' && rightPanelOpen && webShowShot && (!webNative || shellModalLive) && !webFrameTimer) {
    webFrameTimer = setInterval(() => { webShotT += 1; }, 600);
  } else if ((rpTab !== 'web' || !rightPanelOpen || !webShowShot || (webNative && !shellModalLive)) && webFrameTimer) {
    clearInterval(webFrameTimer);
    webFrameTimer = null;
  }

  // 面板 → 后端的响应统一解析：旧版服务（没有新接口）会返回 404 或 SPA 兜底的 HTML，
  // 直接 r.json() 只会报天书——这里翻成「请重启服务」的明确提示。
  // missingMsg 说明缺的是哪类接口（浏览器 / 用量台账…）。
  async function apiJson(path, opts, missingMsg) {
    let resp;
    try {
      resp = await fetch(path, opts);
    } catch (e) {
      throw new Error('服务连不上：' + String(e.message || e));
    }
    const text = await resp.text();
    let data = null;
    try {
      data = text ? JSON.parse(text) : null;
    } catch {
      // 接口路径落到 SPA 兜底 = 服务端没这个路由（旧版程序），HTML 开头就是铁证
      if (text.trimStart().startsWith('<')) throw new Error(missingMsg);
      if (resp.status === 404) throw new Error(missingMsg);
      throw new Error('服务返回异常（HTTP ' + resp.status + '），请重启服务后重试');
    }
    if (!resp.ok) throw new Error((data && data.error) || '请求失败（HTTP ' + resp.status + '）');
    return data || {};
  }
  async function webApi(path, opts) {
    return apiJson(path, opts, '服务端没有浏览器接口，跑的还是旧版程序——请重启服务（先 cargo build --release 再 start.bat）');
  }

  // 浏览器视口跟随面板内容区（16ms 节流≈每帧；截图铺满无黑边，网页按真实窗口宽度排版）。
  // x/y 一并上报——shell 模式下后端据此把壳里的 child webview 摆到这块区域。
  // **触发不能只靠 webBodyW/H 的响应式**：窗口横向拉伸时面板宽高都不变、只有 x 变，
  // 响应式不重算 → 壳里 webview 停在旧位置（真机错位 bug）。改用 ResizeObserver 看
  // 尺寸 + window resize 看位置，两条都汇进同一个防抖队列。
  let webViewportTimer = null;
  let webViewportLast = 0; // 最近一次实际发出的时间戳（节流窗口用）
  let webViewportKey = ''; // 已应用的「标签|x,y|宽x高」，防止重复请求打爆 CDP actor
  let webRectObs = null;
  // 拖面板宽度/拉伸窗口时**马上跟**：旧实现是 350ms 尾防抖——拖的过程中定时器被不断
  // 重置、一次都不发，停手才补一发，用户看到的就是"网页宽度不跟着变、很迟钝"。
  // 100ms 节流仍能看出台阶，压到 16ms（≈每帧一次）：首变化立即发，拖动期间至多每
  // 16ms 一发，停手补最后一发。
  function webQueueViewport() {
    const now = Date.now();
    const wait = 16 - (now - webViewportLast);
    if (wait <= 0) {
      if (webViewportTimer) {
        clearTimeout(webViewportTimer);
        webViewportTimer = null;
      }
      webSyncViewport();
    } else if (!webViewportTimer) {
      webViewportTimer = setTimeout(() => {
        webViewportTimer = null;
        webSyncViewport();
      }, wait);
    }
  }
  function webWatchRect(on) {
    // 根容器一并观察：内容区自身尺寸不变、只有 x 平移时（主滚动条出现/消失把弹性行
    // 挤窄 16px、窗口拉伸后面板左缘横移）ResizeObserver 不会为内容区开火——不观察
    // 根容器这种"只挪不变"的位移就漏一次同步，壳里 webview 卡在旧 x 上（真机实测）
    const rootEl = document.querySelector('.app');
    if (on) {
      if (!webRectObs && typeof ResizeObserver !== 'undefined') {
        webRectObs = new ResizeObserver(() => webQueueViewport());
      }
      if (webRectObs) {
        if (webBodyEl) webRectObs.observe(webBodyEl);
        if (rootEl && rootEl !== webBodyEl) webRectObs.observe(rootEl);
      }
      window.addEventListener('resize', webOnWindowResize);
    } else if (webRectObs) {
      if (webBodyEl) webRectObs.unobserve(webBodyEl);
      if (rootEl) webRectObs.unobserve(rootEl);
      window.removeEventListener('resize', webOnWindowResize);
    }
  }  function webOnWindowResize() {
    webQueueViewport();
  }
  // 面板在原生托管且停在「网页」时才需要跟（壳里 webview 的显隐与它联动）
  $: if (webNative && rpTab === 'web' && rightPanelOpen && webBodyEl) webWatchRect(true);
  $: if (!(webNative && rpTab === 'web' && rightPanelOpen)) webWatchRect(false);
  // 自愈兜底：上面的同步全是**事件驱动**的，漏一个事件壳里 webview 就永久卡在
  // 旧矩形上——用户看到的就是「网页浮在错误位置、盖住聊天列右侧」。漏事件的路径不止
  // 一种：只挪不变的位移（观察根容器已补）、渲染竞态量到的瞬态矩形、面板切走期间发生
  // 的布局变化（那时 webSyncViewport 直接 return）。所以再加一条 1s 心跳：重量一次
  // 矩形，**变了才发**（去重键挡着，不变就是零请求），任何漏掉的变化最多 1s 内归位。
  let webSyncHeartbeat = null;
  $: if (rpTab === 'web' && rightPanelOpen && webActive && webStateReady && !webSyncHeartbeat) {
    webSyncHeartbeat = setInterval(() => webQueueViewport(), 1000);
  } else if (!(rpTab === 'web' && rightPanelOpen && webActive && webStateReady) && webSyncHeartbeat) {
    clearInterval(webSyncHeartbeat);
    webSyncHeartbeat = null;
  }
  function webSyncViewport() {
    if (!webActive || rpTab !== 'web') return;
    // 拖右面板手柄期间：**壳模式照常同步**（那只是把矩形经控制通道发过去挪 child
    // webview，一个本地写，很便宜）。2026-10-07 曾为治「拖侧栏巨卡」整段 return，
    // 结果壳里网页整个拖拽过程都不跟手、松手才跳一下——用户 2026-10-30 复述
    // 「调宽不丝滑、画面和实际宽度一直不同步」，就是这条让路挡的。
    // 真正贵的是**截图流**（无头模式：每发一次视口就触发一次 <img> 重载 + CDP
    // 往返），所以只让无头模式让路；截图刷新（webShotT）在拖动期也一起压掉。
    if (rpResizing && !webNative) return;
    // 就绪门：浏览器状态没到手前，web-tabbar 行未定型（有标签=多一行 / 无标签=整行消失），
    // 量的矩形是瞬态——发出去 webview 会先跳错位再跳回。等 loadWebState 就绪后补发。
    if (!webStateReady) return;
    const el = webBodyEl;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const w = Math.max(320, Math.min(1600, Math.round(r.width)));
    const h = Math.max(400, Math.min(2400, Math.round(r.height)));
    if (!w || !h) return;
    const x = Math.round(r.left);
    const y = Math.round(r.top);
    // 弹窗期间冻结矩形上报（2026-10-31 快照方案）：壳 webview 已藏、弹窗底下垫的是
    // 快照帧，此时上报矩形会触发 set_showing(true) 把 webview 又显示出来盖住弹窗。
    // 恢复由 shellSuppressSync 的关闭分支统一做（清快照 + suppress off + 重发矩形）。
    if (shellModalLive) return;
    const key = webActive.id + '|' + x + ',' + y + '|' + w + 'x' + h;
    if (key === webViewportKey) return;
    webViewportKey = key;
    webViewportLast = Date.now();
    webApi('/api/browser/viewport', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      // native：本客户端就是壳里那个（壳模式 + 跑在 Tauri 里）时，量的 x/y 才是壳窗口
      // 坐标，服务端据此挪壳里的 child webview。浏览器直开 5173 的第二个客户端也走这个
      // 接口（截图要跟它自己的面板尺寸），但它的矩形是属于那个浏览器窗口的——不标明白
      // 就会被当成壳的矩形，壳里网页整体漂到错误位置（真机实测「界面卡位」）。
      body: JSON.stringify({ id: webActive.id, x, y, w, h, native: !!webNative }),
    })
      .then((res) => {
        // 服务端拒收（越界等）：清 key，等下一次观察器/节流开火重试，别把坏矩形当成已应用
        if (res && res.skipped) webViewportKey = '';
        else if (!rpResizing) webShotT += 1; // 拖动中不触发截图重载（那是卡顿的来源）
      })
      .catch(() => {
        webViewportKey = ''; // 失败重试
      });
  }
  function webMenuAction(kind) {
    webCloseMenu();
    if (!webActive) return;
    webApi(`/api/browser/${kind}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ id: webActive.id }),
    }).catch((e) => {
      webError = String(e.message || e);
    });
  }

  // 处理页面 JS 弹窗（确定/取消；prompt 类只能确定或取消，输入内容走后端默认值）
  async function webDialogAction(accept) {
    if (!webActive) return;
    try {
      await webApi('/api/browser/dialog', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ id: webActive.id, accept }),
      });
      webDialog = null;
      webShotT += 1;
      await loadWebState();
    } catch (e) {
      webError = String(e.message || e);
    }
  }
  async function webNewTab() {
    if (webBusy) return;
    webBusy = true;
    webError = '';
    try {
      const r = await webApi('/api/browser/new', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ native: !!webNative }),
      });
      if (r.id) {
        webActiveId = r.id;
      } else {
        webError = r.error || '新建标签页失败';
      }
      await loadWebState(true);
      webShotT += 1;
    } catch (e) {
      webError = String(e.message || e);
    }
    webBusy = false;
  }

  async function webCloseTab(id) {
    webBusy = true;
    webError = '';
    try {
      await webApi(`/api/browser/tab?id=${encodeURIComponent(id)}`, { method: 'DELETE' });
      await loadWebState();
      webShotT += 1;
    } catch {}
    webBusy = false;
  }

  async function webGo() {
    const url = webNormalizeUrl(webAddress);
    if (!url || webBusy) return;
    webBusy = true;
    webError = '';
    try {
      // 没有标签页、或当前是空白页（含壳重启后连接已断的失效页）时，直接开新页导航：
      // 空白页没什么可丢的，也顺带绕开「往死连接上导航 → CDP actor 已退出」
      if (!webActive || !webActive.url || webActive.url === 'about:blank') {
        const r = await webApi('/api/browser/new', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ url, native: !!webNative }),
        });
        if (r.id) webActiveId = r.id;
        else webError = r.error || '打开网页失败';
      } else {
        const r = await webApi('/api/browser/navigate', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ id: webActive.id, url, native: !!webNative }),
        });
        if (r.error) webError = r.error;
      }
      webShotT += 1;
      await loadWebState();
    } catch (e) {
      webError = String(e.message || e);
    }
    webBusy = false;
  }

  async function webAction(action) {
    if (!webActive || webBusy) return;
    webBusy = true;
    try {
      await webApi(`/api/browser/${action}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ id: webActive.id }),
      });
      webShotT += 1;
      await loadWebState();
    } catch {}
    webBusy = false;
  }

  async function webShotClick(ev) {
    if (!webActive || webBusy) return;
    const rect = ev.currentTarget.getBoundingClientRect();
    const nw = ev.currentTarget.naturalWidth || 1280;
    const nh = ev.currentTarget.naturalHeight || 900;
    const x = Math.round(((ev.clientX - rect.left) * nw) / rect.width);
    const y = Math.round(((ev.clientY - rect.top) * nh) / rect.height);
    webBusy = true;
    try {
      await webApi('/api/browser/click', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ id: webActive.id, x, y }),
      });
      webShotT += 1;
      await loadWebState();
    } catch {}
    webBusy = false;
  }

  // 滚轮合并：连续滚动只按 120ms 批量发一次（deltaY 累加），滚动不独占 busy，
  // 截图在批次结束后刷新——一次一来回会把快速滚动卡成幻灯片。
  let webScrollPending = 0;
  let webScrollTimer = null;
  function webShotWheel(ev) {
    ev.preventDefault();
    const dy = Math.max(-800, Math.min(800, ev.deltaY || 0));
    if (!dy || !webActive) return;
    webScrollPending += dy;
    if (webScrollTimer) return;
    webScrollTimer = setTimeout(() => {
      webScrollTimer = null;
      const batch = webScrollPending;
      webScrollPending = 0;
      if (!batch || !webActive || webBusy) return;
      webApi('/api/browser/scroll', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ id: webActive.id, deltaY: batch }),
      })
        .then(() => {
          webShotT += 1;
        })
        .catch (() => {});
    }, 120);
  }

  async function runTerm(cmd) {
    termRunning = true;
    termLines = [...termLines, { cmd, out: '', ok: true }];
    termInput = '';
    try {
      const r = await fetch('/api/shell', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ command: cmd }),
      }).then((r) => r.json());
      termLines = termLines.map((l, i) =>
        i === termLines.length - 1 ? { ...l, out: (r.output || '').trim(), ok: r.ok !== false } : l
      );
    } catch (e) {
      termLines = termLines.map((l, i) => (i === termLines.length - 1 ? { ...l, out: `请求失败: ${e.message || e}`, ok: false } : l));
    }
    termRunning = false;
  }

  // 当前任务基线 = 消息流里最近一次 checkpoint
  function currentBaseline() {
    for (let i = messages.length - 1; i >= 0; i--) {
      if (messages[i].cp && messages[i].cp.id) return messages[i].cp.id;
    }
    return '';
  }

  /// 产物基线：本会话消息流里的检查点 → 本会话自己的检查点（mine）。
  /// **不再兜底「全局最新」**（2026-10-02 实锤）：别的会话的任务起点当基线，本会话
  /// 早于该起点的改动（如上一轮会话造的 test.pdf）全被算成「基线里已有的」——
  /// 产物面板假空。会话确实没有检查点（早于 2026-10-02 非 git 工作区修复前跑的）
  /// 就明说没有基线，别拿别人的起点充数。
  async function resolveBaseline(opts = {}) {
    // 用户手动重拍的基线最优先（基线过期后点「重新以当前状态为起点」就是它）
    if (manualBaseline) return { id: manualBaseline, note: '' };
    if (!opts.sessionFirst) {
      const inStream = currentBaseline();
      if (inStream) return { id: inStream, note: '' };
    }
    try {
      const q = currentSessionId ? `?session=${encodeURIComponent(currentSessionId)}` : '';
      const r = await fetch(`/api/checkpoints${q}`).then((r) => r.json());
      // 产物面板（sessionFirst）取会话**最早**的检查点=会话起点：「这个会话产出过什么」
      // 按整条会话算。危险工具的执行前快照会把 mine 刷爆（2026-10-31 实锤：16 个全是
      // shell/edit 快照），最新一条只是最后一个任务的切片——用它当基线，会话早先的
      // 产物全部隐身（真机实测「切回产生 index.html 的会话看不到产物」）。
      if (opts.sessionFirst) {
        if (r.first && r.first.id) return { id: r.first.id, note: '' };
        const mineAll = r.mine || [];
        if (mineAll.length) return { id: mineAll[mineAll.length - 1].id, note: '' };
        return { id: '', note: currentSessionId ? '该会话运行时还没有检查点（产物归因需要任务起点快照）' : '新对话还没有任务记录' };
      }
      const mine = (r.mine || [])[0];
      if (mine) return { id: mine.id, note: '' };
      return { id: '', note: currentSessionId ? '该会话运行时还没有检查点（产物归因需要任务起点快照）' : '新对话还没有任务记录' };
    } catch (e) {
      return { id: '', note: `读取检查点失败: ${e.message || e}` };
    }
  }

  async function loadArtifacts() {
    rpLoading = true;
    rpWarn = '';
    rpStale = false;
    rpStaleErr = '';
    // 「和 ZCode 一样那套」：产物面板**不再用影子基线**——直接看
    // 工作区真 git 状态（相对 HEAD）。基线过期这类病在设计上就不存在了：你删/挪什么，
    // 面板如实显示什么；agent 造的新文件走 --others 追加，同样在清单里。
    // 非 git 工作区才退化到影子基线（git 都没有，只能靠快照）。
    let usedBaseline = false;
    try {
      let r = await fetch('/api/changes?origin=workspace').then((x) => x.json());
      // 非 git 工作区 / 仓库还没有任何提交（无 HEAD）→ 退回影子基线（会话起点）。
      // 判据要认 git 的真实报错原文（2026-10-31 实锤：只认「不在 git 仓库」漏掉了
      // 「git diff --numstat HEAD 失败: ambiguous argument 'HEAD'」，空仓库直接白屏）。
      const errText = String(r.error || '');
      if (errText) {
        // 非 git 工作区兜底：退回影子基线（会话起点）
        let { id: baseline, note } = await resolveBaseline({ sessionFirst: true });
        if (!baseline) {
          // **只在「还没点进任何会话」时**借工作区最早的检查点兜底。
          // 有会话但会话自己没有检查点时绝不跨会话借——借来的可能是别的会话在
          // 空文件夹里拍的空树基线，产物面板就会把整个工作区（历史截图、别的项目
          // 文件）当成"本任务产物"全列出来。2026-11-03 真机实测「改 bug 改着改着
          // 产物又变成这样」的根因：每轮产生的文件都堆进这个假产物清单。
          if (!currentSessionId) {
            try {
              const all = await fetch('/api/checkpoints').then((x) => x.json());
              const list = all.checkpoints || [];
              if (list.length) baseline = list[list.length - 1].id;
            } catch {
              /* 拿不到就走下面的空态 */
            }
          }
        }
        if (!baseline) {
          // 检查点历史缺失（数据目录迁移/老会话）时用会话里的写入记录兜底：
          // 产物面板是「这个会话产出过什么」，有写入就该有清单，别让用户看到一片空
          // （2026-11-03 真机实测「产物列表刷不出之前生成过的」）。
          const derived = filesTouchedInMessages(messages, workspace);
          if (derived.length) {
            rpFiles = derived;
            rpLoading = false;
            return;
          }
          rpFiles = [];
          rpArtifactsNote = note || '当前会话还没有任务起点快照——产物从下一次任务开始记录，也可以点「重拍基线」以当前状态为起点。';
          rpLoading = false;
          return;
        }
        usedBaseline = true;
        // 非 git 工作区的再一层过滤：只认**会话开始之后**改过的文件。
        // 空树基线（会话在空工作区开工）会把整个工作区都算成"本会话产物"——历史截图、
        // 用户自己拷进来的项目文件全在列表里（2026-11-03 真机实测「没产物的对话反倒
        // 是一坨乱数据」）。agent 写的文件 mtime 都在会话内；早先拷入的文件 mtime 是旧时间。
        const meta = (sessions || []).find((x) => x.id === currentSessionId);
        const sinceSec = meta && meta.created_at ? Math.floor(Date.parse(meta.created_at) / 1000) || 0 : 0;
        const sinceQ = sinceSec ? `&since=${sinceSec}` : '';
        r = await fetch(`/api/changes?baseline=${encodeURIComponent(baseline)}${sinceQ}`).then((x) => x.json());
      }
      rpFiles = r.files || [];
      if (usedBaseline) {
        // 兜底路径（非 git 工作区）不再弹「基线过期」气泡（
        // 「气泡点了没用/一直留着」）：直接只留**还在工作区里的东西**——
        // 删除类条目(D) 在「这个会话产出了什么」这个语义下没有意义（文件都没了，
        // 点开也是空的），静默丢掉即可，不必让用户确认。
        rpFiles = rpFiles.filter((f) => f.status !== 'D');
      }
      rpWarn = '';
      rpStale = false;
      if (r.error) rpArtifactsNote = `读取失败: ${r.error}`;
    } catch (e) {
      rpFiles = [];
      rpArtifactsNote = `读取失败: ${e.message || e}`;
    }
    rpLoading = false;
  }

  // 会话/工作区范围变了：git 与产物面板的状态整体作废并重拉。哪条路径少了这套
  // 重置，面板就停留在上一个范围的清单上——2026-11-04 真机实测「新对话里产物
  // 还是上一个会话的 campusnet」：startNewSession 漏了这套，切换会话有、新建没有
  function resetScopedPanels() {
    rvBaseline = '';
    rvFiles = [];
    rvOpen = {};
    rvDiff = {};
    rvWarn = '';
    rpFiles = [];
    rpArtifactsNote = '';
    rpWarn = '';
    manualBaseline = ''; // 重拍的基线只属于当时的会话/工作区，切走即作废
    rpStale = false;
    rvStale = false;
    rpStaleClosed = false;
    rvStaleClosed = false;
    rpStaleErr = '';
    rvStaleErr = '';
    if (rpTab === 'git') loadReview();
    else if (rpTab === 'artifacts') loadArtifacts();
  }

  // 手动重拍基线：POST /api/checkpoints 以当前工作区状态为新起点，产物/git 面板回到真实增量
  async function rebaseline() {
    const q = currentSessionId ? `?session=${encodeURIComponent(currentSessionId)}` : '';
    rpStaleErr = '';
    rvStaleErr = '';
    try {
      const r = await fetch('/api/checkpoints' + q, { method: 'POST' }).then((x) => x.json());
      if (r.error) {
        // 失败必须在气泡上就地可见（藏在 note 里=用户「点了没用」的教训）
        rpStaleErr = `重拍失败: ${r.error}`;
        rvStaleErr = rpStaleErr;
        return;
      }
      if (!r.ok) {
        rpStaleErr = r.note || '拍不了基线';
        rvStaleErr = rpStaleErr;
        return;
      }
      manualBaseline = r.checkpoint.id;
      rvBaseline = r.checkpoint.id;
      rpStale = false;
      rvStale = false;
      rpStaleClosed = false;
      rvStaleClosed = false;
      // 无变更重拍：服务端会带人话（直接以最近检查点为起点），空清单时有处可说
      rpArtifactsNote = r.note || '';
      loadArtifacts();
      if (rpTab === 'git') loadReview();
    } catch (e) {
      rpStaleErr = `重拍失败: ${e.message || e}`;
      rvStaleErr = rpStaleErr;
    }
  }

  // —— 右侧标签页：增 / 关 / 激活 / 文件预览标签 ——
  function activateRpTab(key) {
    rpTab = key;
    rpAddOpen = false;
    const t = rpTabs.find((x) => x.key === key);
    if (!t) return;
    if (t.kind === 'artifacts') loadArtifacts();
    if (t.kind === 'git') {
      loadReview();
      refreshGit(); // 状态行（分支/HEAD/脏数）要跟清单一起新
    }
    if (t.kind === 'web') loadWebState();
    if (t.kind === 'file') loadPreview(t.path);
  }
  function addRpTab(p) {
    if (!rpTabs.some((t) => t.key === p.key)) rpTabs = [...rpTabs, { key: p.key, kind: p.kind, name: p.name, title: p.title }];
    activateRpTab(p.key);
  }
  async function closeRpTab(key) {
    const i = rpTabs.findIndex((t) => t.key === key);
    if (i < 0) return;
    const t = rpTabs[i];
    const c = t.kind === 'file' ? rpPreviewCache[t.path] : null;
    if (c && c.editing && (c.draft || '') !== (c.content || '')) {
      const ok = await confirmAsk(`「${t.title || t.path}」有未保存的修改，仍要关闭？`);
      if (!ok) return;
      forgetPreviewDraft(t.path);
    }
    const rest = rpTabs.filter((t) => t.key !== key);
    rpTabs = rest;
    if (rpTab === key) {
      const next = rest[i] || rest[i - 1]; // 优先右邻，没有取左邻；都没有 = 关光 → 选择页
      rpTab = next ? next.key : '';
      if (next) activateRpTab(next.key);
    }
  }
  // 插件面板（时间线/项目速览/用量）开成标签：点「+」菜单或空态选择页进来的
  function rpExtTitle(name) {
    const e = enabledExts.find((x) => x.name === name);
    if (!e) return name;
    const u = (e.ui || []).find((x) => x.slot === 'dock' || x.slot === 'toolbar');
    return (u && u.title) || e.title || name;
  }
  function openPreviewTab(path) {
    const key = 'file:' + path;
    if (!rpTabs.some((t) => t.key === key)) {
      rpTabs = [...rpTabs, { key, kind: 'file', path, title: path.split('/').pop() || path }];
    }
    rpPreviewPath = path;
    rightPanelOpen = true;
    terminalOpen = false;
    activateRpTab(key);
  }
  // —— 文件预览标签：文本可编辑保存；文档类（docx/xlsx/pptx/pdf）只能预览不能编辑，
  // xlsx/pptx/pdf 连预览组件也不留、内容直出（用户 2026-10-03）——
  const RP_IMG_EXT = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'bmp', 'ico', 'avif']);
  const RP_PDF_EXT = new Set(['pdf']);
  const RP_MEDIA_EXT = new Set(['mp4', 'm4v', 'webm', 'mov', 'mp3', 'wav', 'ogg', 'm4a', 'flac']);
  // 文档容器：docx→mammoth 排版、xlsx→SheetJS 表格、pptx→分页文本（后端结构化）；
  // ppt(旧版)/odf 走后端文本抽取
  const RP_DOCX_EXT = new Set(['docx']);
  const RP_XLSX_EXT = new Set(['xlsx']);
  const RP_PPTX_EXT = new Set(['pptx']);
  const RP_DOC_EXT = new Set(['ppt', 'odt', 'ods', 'odp']);
  function extOf(path) {
    const i = String(path).lastIndexOf('.');
    return i > 0 ? String(path).slice(i + 1).toLowerCase() : '';
  }
  // 预览形态：image / pdf / media / docx / xlsx / pptx / doc / text
  function previewKind(path) {
    const e = extOf(path);
    if (RP_IMG_EXT.has(e)) return 'image';
    if (RP_PDF_EXT.has(e)) return 'pdf';
    if (RP_MEDIA_EXT.has(e)) return 'media';
    if (RP_DOCX_EXT.has(e)) return 'docx';
    if (RP_XLSX_EXT.has(e)) return 'xlsx';
    if (RP_PPTX_EXT.has(e)) return 'pptx';
    if (RP_DOC_EXT.has(e)) return 'doc';
    return 'text';
  }
  // 后端 200KB 头尾压缩标记（context::OMITTED_MARK）：压缩过的内容拿去保存会把中间
  // 省略掉的部分整段写没——这类文件不给编辑，宁可不让改也不许写坏文件
  function previewTruncated(text) {
    return String(text || '').includes('……[中间省略');
  }
  function rpSet(path, patch) {
    rpPreviewCache = { ...rpPreviewCache, [path]: { ...(rpPreviewCache[path] || {}), ...patch } };
  }
  // docx → HTML：mammoth 转出完整排版（标题层级/表格/列表/加粗/图片 data URI）
  async function loadDocxHtml(path) {
    rpSet(path, { loading: true, error: '' });
    try {
      const buf = await fetch(`/api/workspace/raw?path=${encodeURIComponent(path)}`).then((r) => {
        if (!r.ok) throw new Error(`读取失败: HTTP ${r.status}`);
        return r.arrayBuffer();
      });
      const mammoth = (await import('mammoth/mammoth.browser.js')).default;
      const res = await mammoth.convertToHtml(
        { arrayBuffer: buf },
        {
          convertImage: mammoth.images.imgElement((image) => image.read('base64').then((b64) => ({ src: `data:${image.contentType};base64,${b64}` }))),
        }
      );
      rpSet(path, { loading: false, content: res.value, error: '' });
    } catch (e) {
      rpSet(path, { loading: false, content: '', error: `docx 渲染失败: ${e.message || e}` });
    }
  }
  // xlsx → 二维文本网格（SheetJS；每 sheet 一个 { name, rows[][] }，渲染侧全走文本插值）。
  // 2026-10-04 审计：原先 sheet_to_html 直出 {@html}——SheetJS 的 cell.h 存在时绕过
  // 转义、超链接 Target 原样拼进 href，模型往工作区写一个恶意 xlsx，打开预览就在
  // 宿主 origin 执行任意脚本（等同放任）。现在只取显示文本（raw:false），不含任何标签。
  async function loadXlsxHtml(path) {
    rpSet(path, { loading: true, error: '' });
    try {
      const buf = await fetch(`/api/workspace/raw?path=${encodeURIComponent(path)}`).then((r) => {
        if (!r.ok) throw new Error(`读取失败: HTTP ${r.status}`);
        return r.arrayBuffer();
      });
      const XLSX = await import('xlsx/dist/xlsx.full.min.js');
      const wb = XLSX.read(new Uint8Array(buf), { type: 'array' });
      const sheets = wb.SheetNames.map((name) => {
        const rows = XLSX.utils.sheet_to_json(wb.Sheets[name], { header: 1, raw: false, blankrows: true, defval: '' });
        return { name, rows: rows.map((row) => (Array.isArray(row) ? row : []).map((c) => String(c ?? ''))) };
      });
      rpSet(path, { loading: false, content: JSON.stringify(sheets), error: '' });
    } catch (e) {
      rpSet(path, { loading: false, content: '', error: `xlsx 渲染失败: ${e.message || e}` });
    }
  }
  // pptx → 分页 HTML（后端抽 slide 结构，slides:[{n,text}]）
  async function loadPptxHtml(path) {
    rpSet(path, { loading: true, error: '' });
    try {
      const r = await fetch(`/api/workspace/doc-slides?path=${encodeURIComponent(path)}`).then((r) => r.json());
      if (!r.ok) throw new Error(r.error || '读取失败');
      rpSet(path, { loading: false, content: JSON.stringify(r.slides || []), error: '' });
    } catch (e) {
      rpSet(path, { loading: false, content: '', error: `pptx 渲染失败: ${e.message || e}` });
    }
  }
  async function loadPreview(path, force) {
    const kind = previewKind(path);
    const hit = rpPreviewCache[path];
    if (hit && !hit.loading && !hit.error && hit.content !== undefined && !force) return;
    rpSet(path, { loading: true, error: '', editing: false, draft: '', base: '', editNote: '', savedAt: '' });
    // 图片/PDF/音视频走 /api/workspace/raw 直出渲染，不去拉文本（拉了必 400，白留一条控制台报错）
    if (kind === 'image' || kind === 'pdf' || kind === 'media') {
      rpSet(path, { loading: false, content: '', error: '' });
      return;
    }
    try {
      if (kind === 'docx') {
        // docx 拉原始字节走 mammoth（不走 doc 抽取端点）
        rpSet(path, { loading: false, content: undefined, error: '' });
        await loadDocxHtml(path);
        return;
      }
      if (kind === 'xlsx') {
        rpSet(path, { loading: false, content: undefined, error: '' });
        await loadXlsxHtml(path);
        return;
      }
      if (kind === 'pptx') {
        rpSet(path, { loading: false, content: undefined, error: '' });
        await loadPptxHtml(path);
        return;
      }
      // 文档容器走文本抽取端点，其余按文本读
      const url = kind === 'doc' ? `/api/workspace/doc?path=${encodeURIComponent(path)}` : `/api/workspace/file?path=${encodeURIComponent(path)}`;
      const r = await fetch(url).then((r) => r.json());
      const content = r.ok ? (kind === 'doc' ? r.text : r.content) : '';
      rpSet(path, { loading: false, content, error: r.ok ? '' : r.error || '读取失败' });
      // 刷新/重开前只改了一半的文件：内容读回来之后把草稿铺回去（编辑态 + 可见提示）
      if (r.ok && kind === 'text' && content && !previewTruncated(content)) restorePreviewDraft(path, content);
    } catch (e) {
      rpSet(path, { loading: false, content: '', error: e.message || String(e) });
    }
  }
  function startEditPreview(path) {
    const base = (rpPreviewCache[path] || {}).content || '';
    rpSet(path, { editing: true, draft: base, base, editNote: '', error: '' });
  }
  function cancelEditPreview(path) {
    rpSet(path, { editing: false, draft: '', base: '', editNote: '' });
    forgetPreviewDraft(path);
  }
  async function savePreview(path) {
    const hit = rpPreviewCache[path] || {};
    if (hit.saving) return;
    rpSet(path, { saving: true, error: '' });
    try {
      const r = await fetch('/api/workspace/file', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ path, content: hit.draft || '' }),
      }).then((x) => x.json());
      if (!r.ok) {
        rpSet(path, { saving: false, error: r.error || '保存失败' });
        return;
      }
      forgetPreviewDraft(path);
      rpSet(path, {
        saving: false,
        content: hit.draft || '',
        editing: false,
        draft: '',
        base: '',
        editNote: '',
        savedAt: new Date().toLocaleTimeString('zh-CN', { hour12: false }),
      });
    } catch (e) {
      rpSet(path, { saving: false, error: e.message || String(e) });
    }
  }
  function openRightPanel() {
    // 打开面板时把激活标签的数据拉起来（首启无标签则显示「打开标签页」选择页）
    const t = rpTabs.find((x) => x.key === rpTab);
    if (t) activateRpTab(t.key);
  }

  // —— git 面板数据：dockCp = 「本任务改动」的基线（实时会话用消息流里的检查点；
  // 旧会话回退到影子仓库基线）。基线优先取**本会话自己的**检查点（带 Agent-Session
  // 归属），没有才用全局最新一个——同一项目多条会话来回切时互不串。
  let dockCp = null; // { id, files, additions, deletions } additions/deletions 可为 null（旧会话未知）
  let gitInfo = null; // { is_repo, git_missing, branch, head, dirty, upstream, ahead, behind, files: [{status, path}] }
  async function refreshGit() {
    try {
      gitInfo = await fetch('/api/git/status').then((r) => r.json());
    } catch (e) {
      gitInfo = null;
    }
  }
  // 缺 git（git.exe 六步发现链全空）与「工作区不是 git 仓库」是两回事，界面都要照实说
  // （没检测到 git 时不许静默变样，要连安装方法一起讲）
  $: gitMissing = !!(gitInfo && gitInfo.git_missing);
  function gitSourceLabel(s) {
    return s === 'system' ? '系统安装' : s === 'portable' ? '便携版（exe 同级）' : s === 'bundled' ? '内置运行时' : s || '未知';
  }
  const GIT_INSTALL_HINT = '装一个 Git for Windows（https://git-scm.com/downloads）后重启本软件即可恢复';

  // —— 手动提交（git 面板「未提交改动」档内联完成，不必弹窗、不必让 AI 代劳）——
  // 勾选态挂在未提交清单上：已跟踪改动默认勾，未跟踪文件（日志、构建产物等）默认不勾
  let commitMsg = '';
  let commitSel = {}; // path → 是否勾选（暂存）
  let commitBusy = false;
  let commitErr = '';
  // 清单重拉后按 path 沿用旧勾选；新出现的文件按上面的默认规则补
  function initCommitSel(files) {
    const next = {};
    for (const f of files || []) {
      next[f.path] = f.path in commitSel ? commitSel[f.path] : !f.status.includes('?');
    }
    commitSel = next;
  }
  function toggleCommitFile(path) {
    commitSel = { ...commitSel, [path]: !commitSel[path] };
    commitErr = '';
  }
  // Svelte4 坑：模板里调函数不建立函数体内变量的依赖——计数必须用 $: 派生，
  // 直接调 commitSelCount() 的话勾选变了按钮文字不重算（真机实锤）
  $: commitSelN = rvSource === 'work' ? rvFiles.filter((f) => commitSel[f.path]).length : 0;
  async function doCommit() {
    if (commitBusy) return;
    const msg = commitMsg.trim();
    if (!msg) {
      commitErr = '写一句提交信息';
      return;
    }
    const files = rvFiles.filter((f) => commitSel[f.path]).map((f) => f.path);
    if (!files.length) {
      commitErr = '至少勾一个文件';
      return;
    }
    commitBusy = true;
    commitErr = '';
    try {
      const r = await fetch('/api/git/commit', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ message: msg, files }),
      }).then((x) => x.json());
      if (!r.ok) {
        commitBusy = false;
        commitErr = r.error || '提交失败';
        return;
      }
      // 用户 2026-10-07：提交成功不再往聊天区塞「已提交 xxx」提示——面板状态行自己会变
      commitBusy = false;
      commitMsg = '';
      commitErr = '';
      await refreshGit();
      refreshDock();
      if (rpTabs.some((t) => t.kind === 'git')) loadReview(); // 提完清单重拉：勾选态按新清单重建
    } catch (e) {
      commitBusy = false;
      commitErr = `提交失败: ${e.message || e}`;
    }
  }
  async function refreshDock() {
    refreshGit(); // git 面板状态行（分支/脏数）一并刷新
    for (let i = messages.length - 1; i >= 0; i--) {
      if (messages[i].cp && messages[i].cp.id) {
        dockCp = messages[i].cp;
        return;
      }
    }
    // 消息流里没有检查点（cp 不落盘，刷新后就没有）→ 走 resolveBaseline 同一套兜底，
    // 与「产物」标签共用（分叉过一次：产物列空而改动条有数，真机实测）
    const { id: baseline } = await resolveBaseline();
    if (!baseline) {
      dockCp = null;
      return;
    }
    try {
      const c = await fetch(`/api/changes?baseline=${encodeURIComponent(baseline)}`).then((x) => x.json());
      dockCp = { id: baseline, files: (c.files || []).length, additions: null, deletions: null };
    } catch (e) {
      dockCp = null;
    }
  }

  // —— CSS 注入器：用户自定义 CSS，实时生效并记住 ——
  let customCss = localStorage.getItem('customCss') || '';
  function applyCustomCss(css) {
    // 设置分区的注入框搬进 AppearanceTab 后，输入框 bind 的是组件 prop——根这份值
    // 由 prop 绑定回传；显式带参保证同一事件里拿到的就是刚敲进去的文本
    if (css !== undefined) customCss = css;
    let el = document.getElementById('user-custom-css');
    if (!el) {
      el = document.createElement('style');
      el.id = 'user-custom-css';
      document.head.appendChild(el);
    }
    // 微调规则在前，手动 CSS 在后——注入框永远有最终话语权，手写 background: transparent 能盖掉微调。
    // frame:* 键（同源 iframe / 跨源插件）不进主文档——它们的选择器是相对插件文档的，
    // 落到主文档会误伤同名类（2026-11-03 接跨源微调时收口）
    const mainTweaks = Object.fromEntries(Object.entries(microTweaks || {}).filter(([k]) => !k.startsWith('frame:')));
    const micro = microCss(mainTweaks);
    el.textContent = (micro ? `${micro}\n` : '') + escalateCss(customCss);
    localStorage.setItem('customCss', customCss);
    applyFrameMicro();
    applyExtMicroAll();
  }

  // —— 界面微调模式：右击元素弹出属性面板，实时预览，生成微调规则 ——
  // 2026-11-03 起同源 iframe 面板（用量/时间线/网页/插件页）也能微调：编辑模式给每个
  // 可达的 contentDocument 挂右击监听；规则 key 加 `frame:<iframe路径>|` 前缀，
  // applyFrameMicro 把这类规则注入对应 iframe 文档（跨域 iframe 够不着，照旧管不了）。
  // 2026-11-03 深夜再加跨源插件 iframe：插件是 sandbox 不透明源，主文档结构够不着，
  // 微调走桥协议（pawch:micro enter/exit/apply 下发，插件 pawch:micro-pick 上报右击元素），
  // 规则 key = `frame:ext:<插件名>|<选择器>`。
  let editMode = localStorage.getItem('microEditMode') === '1';
  // editMode 记住状态（用户被「重载界面就重置」坑过一次）：微调模式开着重启界面仍开着
  $: localStorage.setItem('microEditMode', editMode ? '1' : '0');
  // 轻量 toast（微调应用/重置的回执）：插件面板里的改动可能不明显（改文字色对容器
  // 无效等），给一句「已应用」让用户知道规则真的落了，1.6s 自动消失
  let toastMsg = '';
  let toastTimer = 0;
  function toast(text) {
    toastMsg = text;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toastMsg = ''), 1600);
  }
  let microPanel = null; // { x, y, key, name, sel, el, color, bg, bgOpacity, radius, blur, fontSize, bgTransparent, *Dirty }
  let microTweaks = (() => {
    try {
      const raw = JSON.parse(localStorage.getItem('microTweaks') || '{}');
      // 一次性迁移（老代码残留）：旧版"应用"会把当时算出的背景色一并落盘，
      // 深色主题下就是 #17171a 这类近黑实色，带 #app 前缀后压过壁纸半透明规则，
      // 表现为"消息里几块死黑、写 CSS 也去不掉"。清掉全部存量背景键（只清一次，
      // 迁移后用户手动改的背景会正常保留），其他属性（圆角/字号/文字色）保留。
      if (localStorage.getItem('microTweaksBgMigrated') !== '2') {
        for (const k of Object.keys(raw || {})) {
          const t = raw[k];
          if (t && typeof t === 'object') {
            delete t.bg;
            delete t.bgOpacity;
          }
        }
        try { localStorage.setItem('microTweaks', JSON.stringify(raw)); } catch (e) {}
        try { localStorage.setItem('microTweaksBgMigrated', '2'); } catch (e) {}
      }
      return raw;
    } catch (e) {
      return {};
    }
  })();



  // 补盖标记：只在元素还匹配原选择器时才盖，避免路径漂移后错标到别的元素
  function stampMicroUids() {
    for (const t of Object.values(microTweaks || {})) {
      if (!t || !t.uid || !t.path) continue;
      if (document.querySelector(`[data-mt="${t.uid}"]`)) continue;
      const el = findByPath(t.path);
      if (!el) continue;
      try {
        if (!t.sel || el.matches(t.sel)) el.setAttribute('data-mt', t.uid);
      } catch (e) {}
    }
  }
  function unstampAll() {
    for (const el of document.querySelectorAll('#app [data-mt]')) el.removeAttribute('data-mt');
  }

  function onMicroContext(e) {
    if (!editMode) return;
    if (e.target.closest && e.target.closest('.micro-panel')) return;
    // 目标归一化：SVG/文本节点先收敛到可调元素——点中图标里的 path 先升到 button，
    // 点中按钮里的文字/图标先升到该按钮，避免结构选择器落到 svg/path/span 碎片上
    let target = e.target;
    if (target && target.nodeType !== 1) target = target.parentElement;
    if (target && target.closest) {
      const svgHit = target.closest('svg');
      if (svgHit && svgHit.closest('button')) target = svgHit.closest('button');
      else if (target instanceof SVGElement) target = target.closest('button') || target.parentElement || target;
    }
    if (!target || !target.closest) return;
    // 跨源插件 iframe：右击 iframe 元素本体（边框/空隙）——真正的元素级右击由插件桥
    // 在面板内部接管（新桥插件）。这里给个说明面板，别让右击没反应（2026-11-03）
    if (target.tagName === 'IFRAME') {
      const hit = Object.entries(extFrameEls).find(([, el]) => el === target);
      if (hit) {
        e.preventDefault();
        const extName = hit[0];
        const supported = extMicroCap[extName] === true;
        microPanel = {
          unsupported: true,
          msg: supported
            ? `${extName}：请在面板内容上右击选择要微调的元素（这个 iframe 的边框上选不到内部元素）。`
            : `${extName}：该插件版本暂不支持微调（桥客户端还没有微调协议）。更新/重装这个插件后，在面板内容上右击即可微调。`,
          extName,
          name: `插件面板 · ${extName}`,
          sel: '',
          el: null,
          x: e.clientX,
          y: e.clientY,
          key: '',
          bgHost: null,
          scope: 'all',
          color: '#000000',
          bg: '#000000',
          bgTransparent: true,
          bgOpacity: 0,
          radius: 0,
          border: 0,
          borderColor: '#000000',
          blur: 0,
          fontSize: 14,
        };
        return;
      }
    }
    // 候选收集：所有命中的别名都算上，按"离点击最近"排序；
    // pick:'self' 的大容器只有点中自身（空隙/padding）时才参选，点中子元素时让位给具体控件；
    // pick:选择器的容器先收敛到内部最近控件，找不到才回落容器自身
    const cands = [];
    for (const a of ELEMENT_ALIASES) {
      const el = target.closest(a.sel);
      if (!el) continue;
      let depth = 0;
      let cur = target;
      while (cur && cur !== el) {
        cur = cur.parentElement;
        depth++;
      }
      if (a.pick === 'self' && depth > 0) continue;
      if (typeof a.pick === 'string' && a.pick !== 'self') {
        const inner = target.closest(a.pick);
        if (inner && el.contains(inner)) {
          let d2 = 0;
          let c2 = target;
          while (c2 && c2 !== inner) {
            c2 = c2.parentElement;
            d2++;
          }
          cands.push({ alias: { sel: a.pick, name: `${a.name} · 子项` }, el: inner, depth: d2 });
          continue;
        }
      }
      cands.push({ alias: a, el, depth });
    }
    cands.sort((x, y) => x.depth - y.depth);
    e.preventDefault();
    // 兜底一提前：目标元素自己有语义类名（md 表格行/单元格、代码块内部行……）就永远
    // 选它自己——旧顺序是 3 层内的登记容器先抢（点表格 td 被外层 .rp-body 拿走），
    // 报修一个登记一个永远修不完（2026-11-03 用户「估计还有其他的，我不知道原因」实锤）
    const auto0 = autoClassTarget(target);
    if (auto0) {
      openMicroPanel(e.clientX, e.clientY, auto0.name, auto0.sel, auto0.el);
      return;
    }
    if (cands.length && cands[0].depth <= 3) {
      const win = cands[0];
      openMicroPanel(e.clientX, e.clientY, win.alias.name, win.alias.sel, win.el);
      return;
    }
    // 兜底二：附近没有别名 → 放宽 pick:'self'，回退到最近的"容器别名"（任意深度）。
    // 比结构选择器更稳：点右侧面板内部就选中「产物面板」，不会再出"未登记元素"的
    // div:nth-of-type 长链（那种选择器一改布局就失效）
    const relaxed = [];
    for (const a of ELEMENT_ALIASES) {
      const el = target.closest(a.sel);
      if (!el) continue;
      let depth = 0;
      let cur = target;
      while (cur && cur !== el) {
        cur = cur.parentElement;
        depth++;
      }
      relaxed.push({ alias: a, el, depth });
    }
    relaxed.sort((x, y) => x.depth - y.depth);
    if (relaxed.length) {
      const win = relaxed[0];
      openMicroPanel(e.clientX, e.clientY, win.alias.name, win.alias.sel, win.el);
      return;
    }
    // 兜底三：整条链都没有登记过 → 结构选择器（止于 #app 的序号链）
    openMicroPanel(e.clientX, e.clientY, '', structuralSelector(target), target);
  }

  // 元素自身是否画了背景（透明度 >5% 才算；transparent/全透明都不算）
  function paintsBg(el) {
    const c = parseCssColor(getComputedStyle(el).backgroundColor);
    if (c && c.a > 0.05) return true;
    // 渐变底也算自己画背景：主按钮的橙画在 linear-gradient 上，backgroundColor 全透明，
    // 只看计算值会误报「底色由上级绘制」，把用户引去改无关的层
    const src = findBgSource(el);
    return !!src && src.kind === 'image';
  }
  // 往上层找第一个真正画背景的祖先：用户常右键在透明的内容层上（如 .rp-body），
  // 改它当然没反应——真正那块"黑"是父层画的，这里把父层找出来供一键跳转
  function findBgHost(el) {
    let cur = el.parentElement;
    while (cur && cur.id !== 'app') {
      if (paintsBg(cur)) {
        for (const a of ELEMENT_ALIASES) {
          try {
            if (cur.matches(a.sel)) return { name: a.name, sel: a.sel, el: cur };
          } catch (e) {}
        }
        return { name: '', sel: structuralSelector(cur), el: cur };
      }
      cur = cur.parentElement;
    }
    return null;
  }

  // 通用兜底：按元素自身的语义类名自动生成选择器。跳过 svelte 作用域类与状态/通用类，
  // 优先带连字符的语义类（term-head、rp-body 这类），这样任何"小条/小按钮"不用逐个登记
  // 也能精确命中，而不是一路回退到整个大面板
  const GENERIC_CLASSES = new Set([
    'active', 'on', 'sel', 'expanded', 'open', 'haskey', 'danger', 'primary', 'ghost', 'stop',
    'running', 'fail', 'ok', 'dim', 'err', 'warn', 'crit', 'archived', 'pinned', 'empty', 'more',
    'group-head', 'wp-pick', 'css-injector', 'micro-readonly', 'content', 'user', 'assistant',
    'boxed', 'left', 'right', 'center', 'small', 'large', 'selected', 'checked', 'risk-0', 'risk-1', 'risk-2',
    // md-* 里只有微调无意义的排除（横线/图片自己有面板兜底），其余都是合法可调目标——
    // 上一轮全量豁免 md-*，长按行内代码 .md-code 被迫回退到 .md-p（2026-11-03 用户二轮实锤）
    'md-hr', 'md-img',
  ]);
  function autoClassTarget(el) {
    // 只认被点元素自己的类名（不再向上爬）：上一层是登记容器时不能反过来抢，
    // 上两层爬上去会把「点 A 选 B」的老毛病换个姿势带回来。没类名就交给登记表/结构兜底。
    // svelte-xxxx 必须一起滤掉：它带连字符，会被「优先带连字符的类」选中，
    // 选择器就变成整个组件的作用域类（长按按钮选中 svelte-1a2xagz，调了等于没调，
    // 2026-11-03 用户「设置里的按钮还是不行」实锤）。
    // 状态类（primary/active 等）不当锚点，但保留作限定：选择器写成 .btn.primary——
    // 「同类全部」才不会把渐变底泼到 ghost/danger 兄弟身上（2026-11-04 默认改同类全部后的配套收口）
    const raw = typeof el.className === 'string' ? el.className : '';
    const all = raw
      .split(/\s+/)
      .filter((c) => c && !/^s-[A-Za-z0-9_-]+$/.test(c) && !/^svelte-[A-Za-z0-9_-]+$/i.test(c));
    const anchors = all.filter((c) => !GENERIC_CLASSES.has(c));
    if (!anchors.length) return null;
    const anchor = anchors.find((c) => c.includes('-')) || anchors[0];
    const qualifiers = all.filter((c) => c !== anchor);
    return { name: anchor, sel: `.${CSS.escape(anchor)}${qualifiers.map((c) => `.${CSS.escape(c)}`).join('')}`, el };
  }

  function openMicroPanel(x, y, name, sel, el, keyOverride) {
    // 右击链路兜底：读源头要遍历样式表，任何一环抛错都不许变成"右击没反应"
    // （2026-11-04 教训：漏 import 直接 ReferenceError，用户看到的就是毫无动静）
    try {
      openMicroPanelInner(x, y, name, sel, el, keyOverride);
    } catch (err) {
      console.error('[micro]', err);
      toast(`微调面板打开失败：${(err && err.message) || err}`);
    }
  }
  function openMicroPanelInner(x, y, name, sel, el, keyOverride) {
    const cs = getComputedStyle(el);
    const key = keyOverride || name || sel;
    // 优先用该元素已存的微调值（面板显示与实际生效一致）；没有才读 computed。
    // frame 规则的选择器可能和主文档撞车，只按 key 精确找，不做 sel 模糊匹配
    const saved = microTweaks[key] || (keyOverride ? null : Object.values(microTweaks).find((t) => t && t.sel === sel)) || null;
    const numOf = (v, fallback) => {
      const n = parseFloat(v);
      return Number.isFinite(n) ? n : fallback;
    };
    // computed backdrop-filter: 'none' 或 'blur(Npx)'，提取 N
    const blurNow = (() => {
      const m = String(cs.backdropFilter || '').match(/blur\(([\d.]+)px\)/);
      return m ? parseFloat(m[1]) : 0;
    })();
    // 背景读数（2026-11-04 重做，DevTools 思路）：先找层叠胜出的背景声明原文，
    // var() 按元素解析成实色——主按钮/发送键的橙色画在 linear-gradient 上，
    // backgroundColor 全透明，老读法会显示「透明度 0%」且拖滑杆没反应（色被渐变盖住）。
    // 渐变原文存进 bgGradient，预览/应用时整条重建（背景色/透明度都写进 background-image）
    const bgParsed = parseCssColor(cs.backgroundColor);
    const bgSrc = findBgSource(el);
    let bgNow = rgbToHex(cs.backgroundColor);
    let bgAlphaPct = bgParsed ? Math.round(bgParsed.a * 100) : 0;
    let bgGradient = null;
    if (bgSrc && bgSrc.kind === 'image') {
      const resolved = resolveCssVars(bgSrc.text, (n) => cs.getPropertyValue(n));
      const first = gradientFirstColor(resolved);
      if (first) {
        bgGradient = resolved;
        bgNow = parsedToHex(first);
        bgAlphaPct = Math.round(first.a * 100);
      }
    }
    // 已存规则优先（面板显示与实际生效一致）；但老存量里"渐变按钮上的实色微调"
    // 是被渐变盖住看不见的死值，现场读到渐变时以现场为准（带 bgImage 的新规则除外）
    const savedBgValid = saved && saved.bg != null && (!bgGradient || !!saved.bgImage);
    microPanel = {
      // 位置：右侧留出面板宽度，下方至少留 340px 可用高度（面板自身 max-height 按 y 计算，
      // 配合内部滚动，保证底部「应用/重置/取消」始终可见可点）
      x: Math.max(8, Math.min(x, window.innerWidth - 300)),
      y: Math.max(8, Math.min(y, window.innerHeight - 340)),
      key,
      name: name || '未登记元素',
      sel,
      el,
      color: saved?.color || rgbToHex(cs.color) || '#000000',
      bg: savedBgValid ? saved.bg : bgNow || '#000000',
      // 没有任何来源画背景（bgTransparent=true）时不填黑：预览与保存都跳过背景，
      // 只有用户真的点了颜色框（dirty）才落盘
      bgTransparent: !savedBgValid && !bgGradient && (!bgParsed || bgParsed.a === 0),
      bgDirty: false,
      colorDirty: false,
      radiusDirty: false,
      blurDirty: false,
      fontSizeDirty: false,
      bgOpacity: savedBgValid && saved.bgOpacity != null ? saved.bgOpacity : bgAlphaPct,
      bgGradient: bgGradient || null,
      radius: numOf(saved?.radius, Math.round(parseFloat(cs.borderTopLeftRadius) || 0)),
      // 描边：宽度 + 颜色（用户显式改过才落盘；只改颜色时尊重元素原有边框样式）
      border: numOf(saved?.border, Math.round(parseFloat(cs.borderTopWidth) || 0)),
      borderColor: saved?.borderColor || rgbToHex(cs.borderTopColor) || '#000000',
      borderDirty: false,
      borderColorDirty: false,
      blur: numOf(saved?.blur, blurNow),
      fontSize: numOf(saved?.fontSize, Math.round(parseFloat(cs.fontSize) || 14)),
      // 自身透明但上层有实底时，面板给出"背景其实由上级绘制"的跳转入口
      bgHost: !paintsBg(el) ? findBgHost(el) : null,
      // 作用范围：默认「同类全部」（同类的就该一起变，行内代码
      // 调一个全跟着透明）。已存的"仅此元素"规则且点的正是那个元素时保持不变；
      // 点的是同类里别的元素 → 默认「同类全部」，应用后整类统一（含旧那个）
      scope: saved && saved.uid && el.getAttribute && el.getAttribute('data-mt') === saved.uid ? 'only' : 'all',
    };
  }

  // 跳到真正绘制背景的上级（用户右键在透明内容层时的救急入口）
  function useBgHost() {
    const p = microPanel;
    if (!p || !p.bgHost) return;
    const h = p.bgHost;
    // 跨源插件元素：改上级也是插件文档里的元素——重新开一块面板（按选择器命中，整类生效）
    if (p.extName) {
      openExtMicroPanel(p.extName, { sel: h.sel, label: h.name || h.sel, styles: h.styles || {} }, { x: p.x, y: p.y });
      return;
    }
    clearMicroInline(p.el);
    openMicroPanel(p.x, p.y, h.name || '上级元素', h.sel, h.el);
  }

  // 直达面板最底层（body）：深色底就是它画的，一层层「改上级」不如一键跳
  function usePanelRoot() {
    const p = microPanel;
    if (!p || !p.extName || !p.root) return;
    openExtMicroPanel(p.extName, { sel: p.root.sel, label: p.root.name, styles: p.root.styles }, { x: p.x, y: p.y });
  }

  function microPreview(prop, value) {
    if (!microPanel) return;
    // 脏标记：只记录用户亲手改过的属性，"应用"时只存这些
    const dirtyKey = { bg: 'bgDirty', bgOpacity: 'bgDirty', color: 'colorDirty', radius: 'radiusDirty', blur: 'blurDirty', fontSize: 'fontSizeDirty', border: 'borderDirty', borderColor: 'borderColorDirty' }[prop];
    microPanel = { ...microPanel, [prop]: value, ...(dirtyKey ? { [dirtyKey]: true, ...(prop === 'bg' || prop === 'bgOpacity' ? { bgTransparent: false } : {}) } : {}) };
    // 广播：自绘下拉等浮层收到后重新贴合定位（预览会改动样式/布局，菜单要跟着挪）
    try {
      window.dispatchEvent(new CustomEvent('micro-preview', { detail: { sel: microPanel.sel } }));
    } catch (e) {}
    // 跨源插件元素：整包 CSS 经桥下发预览（主文档的 el.style 路径够不着 iframe）。
    // 追加「选中高亮」——用户得看得见自己右击选中了哪个元素（2026-11-03 真机实测
    // 「微调了没反应」一半是不知道选中的是容器：改文字色对子元素自有颜色的容器无效）
    if (microPanel.extName) {
      const sel = microPanel.sel;
      const hl = sel ? `/* 选中高亮 */\n${sel} { outline: 2px dashed rgba(186,138,120,.85) !important; outline-offset: -1px; }\n` : '';
      sendExtCss(microPanel.extName, extCssFor(microPanel.extName, extPreviewEntry()) + hl);
      return;
    }
    const el = microPanel.el;
    if (!el || !el.isConnected) return;
    if (prop === 'border') {
      // 拖宽度必须同时给 border-style，否则 border:none 的元素看不出变化
      el.style.borderStyle = 'solid';
      el.style.borderWidth = `${value}px`;
      return;
    }
    if (prop === 'borderColor') {
      el.style.borderStyle = 'solid';
      el.style.borderColor = value;
      return;
    }
    if (prop === 'bg' || prop === 'bgOpacity') {
      // 渐变底：整条重建渐变（保留结构，hex 与首 stop 同色时只调透明度不拍平）。
      // 只写 backgroundColor 会被原渐变盖住，永远看不出变化
      if (microPanel.bgGradient) {
        el.style.backgroundImage = rebuildGradient(microPanel.bgGradient, microPanel.bg, microPanel.bgOpacity);
        return;
      }
      // 透明底元素：预览也不烤实色，避免"还没点应用就先黑一块"吓到人
      if (microPanel.bgTransparent) return;
      el.style.backgroundColor = hexWithAlpha(microPanel.bg, microPanel.bgOpacity);
      return;
    }
    if (prop === 'blur') {
      el.style.backdropFilter = value > 0 ? `blur(${value}px)` : 'none';
      return;
    }
    if (prop === 'color') {
      el.style.color = value;
      return;
    }
    el.style[prop === 'radius' ? 'borderRadius' : 'fontSize'] = `${value}px`;
  }


  function clearMicroInline(el) {
    for (const k of ['color', 'backgroundColor', 'backgroundImage', 'borderRadius', 'backdropFilter', 'fontSize', 'borderWidth', 'borderColor', 'borderStyle']) el.style[k] = '';
  }

  function applyMicro() {
    const p = microPanel;
    if (!p) return;
    // 从面板控件读当前值（按 data-m 属性显式取值，不依赖顺序；不依赖组件状态的事件同步）
    const panelEl = document.querySelector('.micro-panel');
    const val = (name) => (panelEl ? panelEl.querySelector(`[data-m="${name}"]`)?.value : undefined);
    const num = (name) => {
      const v = val(name);
      return v == null || v === '' ? undefined : +v;
    };
    if (p.extName) {
      // 跨源插件元素：规则 key=frame:ext:<插件名>|<选择器>，按 dirty 落盘后经桥整包下发
      const prev = (microTweaks[p.key] && typeof microTweaks[p.key] === 'object') || {};
      const entry = { sel: p.sel, name: p.name };
      if (p.colorDirty) entry.color = val('color') ?? p.color;
      else if (prev.color) entry.color = prev.color;
      if (p.bgTransparent) {
        if (prev.bg) {
          entry.bg = prev.bg;
          if (prev.bgOpacity != null) entry.bgOpacity = prev.bgOpacity;
        }
      } else if (p.bgDirty) {
        entry.bg = val('bg') ?? p.bg;
        entry.bgOpacity = num('bgOpacity') ?? p.bgOpacity;
      } else if (prev.bg) {
        entry.bg = prev.bg;
        if (prev.bgOpacity != null) entry.bgOpacity = prev.bgOpacity;
      }
      if (p.radiusDirty) entry.radius = `${num('radius') ?? p.radius}px`;
      else if (prev.radius) entry.radius = prev.radius;
      if (p.borderDirty) entry.border = `${num('border') ?? p.border}px`;
      else if (prev.border) entry.border = prev.border;
      if (p.borderColorDirty) entry.borderColor = val('borderColor') ?? p.borderColor;
      else if (prev.borderColor) entry.borderColor = prev.borderColor;
      const blurN = num('blur') ?? p.blur;
      const bgEffOpaque = p.bgTransparent
        ? false
        : p.bgDirty
          ? (num('bgOpacity') ?? p.bgOpacity ?? 100) >= 85
          : prev.bg
            ? (prev.bgOpacity ?? 100) >= 85
            : (p.bgOpacity ?? 100) >= 85;
      if (p.blurDirty && !((parseInt(blurN, 10) || 0) > 0 && bgEffOpaque)) entry.blur = blurN;
      else if (prev.blur != null && !(bgEffOpaque && (parseInt(prev.blur, 10) || 0) > 0)) entry.blur = prev.blur;
      if (p.fontSizeDirty) entry.fontSize = `${num('fontSize') ?? p.fontSize}px`;
      else if (prev.fontSize) entry.fontSize = prev.fontSize;
      const cur = { ...microTweaks };
      cur[p.key] = entry;
      microTweaks = cur;
      localStorage.setItem('microTweaks', JSON.stringify(cur));
      microPanel = null;
      applyCustomCss();
      toast(`已应用微调：${p.name}`);
      return;
    }
    const color = val('color') ?? p.color;
    const bg = val('bg') ?? p.bg;
    const bgOpacity = num('bgOpacity') ?? p.bgOpacity;
    const blurN = num('blur') ?? p.blur;
    const radiusN = num('radius') ?? p.radius;
    const fontSizeN = num('fontSize') ?? p.fontSize;
    // 只落盘用户亲手改过的属性（dirty 标记），但保留该元素上一次已存的其他属性——
    // 老存量/别次改的背景不会因为"这次只改圆角"就被弄丢；没动背景就不碰 bg，透明底元素不会再被误写黑底
    const prev = microTweaks[p.key] && typeof microTweaks[p.key] === 'object' ? microTweaks[p.key] : {};
    const entry = { sel: p.sel, name: p.name };
    if (p.colorDirty) entry.color = color;
    else if (prev.color) entry.color = prev.color;
    if (p.bgTransparent) {
      if (prev.bg) {
        entry.bg = prev.bg;
        if (prev.bgOpacity != null) entry.bgOpacity = prev.bgOpacity;
        if (prev.bgImage) entry.bgImage = prev.bgImage;
      }
    } else if (p.bgDirty) {
      entry.bg = bg;
      entry.bgOpacity = bgOpacity;
      // 渐变底：整条重建后的渐变落盘（microCss 走 background-image，
      // 不再叠 background-color——半透明渐变下面垫实色会把透明度吃掉）
      if (p.bgGradient) entry.bgImage = rebuildGradient(p.bgGradient, bg, bgOpacity);
    } else if (prev.bg) {
      entry.bg = prev.bg;
      if (prev.bgOpacity != null) entry.bgOpacity = prev.bgOpacity;
      if (prev.bgImage) entry.bgImage = prev.bgImage;
    }
    if (p.radiusDirty) entry.radius = `${radiusN}px`;
    else if (prev.radius) entry.radius = prev.radius;
    // 描边：宽度/颜色各自独立落盘；只改颜色时尊重原有边框样式
    const borderN = num('border') ?? p.border;
    const borderColorV = val('borderColor') ?? p.borderColor;
    if (p.borderDirty) entry.border = `${borderN}px`;
    else if (prev.border) entry.border = prev.border;
    if (p.borderColorDirty) entry.borderColor = borderColorV;
    else if (prev.borderColor) entry.borderColor = prev.borderColor;
    // 毛玻璃护栏：背景不透明（opacity>=85%）时存 blur 没有视觉意义，反而会在弹窗/遮罩下发灰叠影；
    // 这种组合直接丢弃 blur，只保留其他属性。想要毛玻璃先把背景透明度降下来。
    const bgEffOpaque = p.bgTransparent
      ? false
      : p.bgDirty
        ? (bgOpacity ?? 100) >= 85
        : prev.bg
          ? (prev.bgOpacity ?? 100) >= 85
          : (p.bgOpacity ?? 100) >= 85;
    if (p.blurDirty && !((parseInt(blurN, 10) || 0) > 0 && bgEffOpaque)) entry.blur = blurN;
    else if (prev.blur != null && !(bgEffOpaque && (parseInt(prev.blur, 10) || 0) > 0)) entry.blur = prev.blur;
    if (p.fontSizeDirty) entry.fontSize = `${fontSizeN}px`;
    else if (prev.fontSize) entry.fontSize = prev.fontSize;
    // 作用范围：仅此元素 → 盖唯一标记并记结构路径（重渲染后按路径补盖）；
    // 同类全部 → 清掉标记字段，规则改回元素选择器
    if (p.scope === 'all') {
      if (prev.uid) {
        const old = document.querySelector(`[data-mt="${prev.uid}"]`);
        if (old) old.removeAttribute('data-mt');
      }
    } else {
      // 仅此元素：uid 只在"还是同一个元素"时复用（换了元素就发新的——复用会让
      // 旧元素挂着一个新规则不再引用的悬空标记，重渲染后又来回跳）
      const pathNow = pathOf(p.el);
      entry.uid = prev.uid && prev.path === pathNow ? prev.uid : makeUid();
      entry.path = pathNow;
      try {
        p.el.setAttribute('data-mt', entry.uid);
      } catch (e) {}
    }
    clearMicroInline(p.el);
    const cur = { ...microTweaks };
    cur[p.key] = entry;
    microTweaks = cur;
    localStorage.setItem('microTweaks', JSON.stringify(cur));
    applyCustomCss();
    microPanel = null;
  }

  function resetMicro() {
    const p = microPanel;
    if (!p) return;
    if (p.extName) {
      const cur = { ...microTweaks };
      delete cur[p.key];
      microTweaks = cur;
      localStorage.setItem('microTweaks', JSON.stringify(cur));
      const n = p.extName;
      microPanel = null;
      applyCustomCss();
      sendExtCss(n, extCssFor(n));
      toast('已重置该元素的微调');
      return;
    }
    clearMicroInline(p.el);
    const prevUid = microTweaks[p.key] && microTweaks[p.key].uid;
    if (prevUid) {
      const stamped = document.querySelector(`[data-mt="${prevUid}"]`);
      if (stamped) stamped.removeAttribute('data-mt');
    }
    const cur = { ...microTweaks };
    delete cur[p.key];
    microTweaks = cur;
    localStorage.setItem('microTweaks', JSON.stringify(cur));
    applyCustomCss();
    microPanel = null;
  }

  // 一键清空所有微调规则（手写 CSS 不动）；用于清掉历史遗留的实色深底等顽固样式
  function clearAllMicro() {
    microTweaks = {};
    localStorage.setItem('microTweaks', '{}');
    unstampAll();
    applyCustomCss();
    settingsMsg = '已清空全部微调规则';
  }

  function cancelMicro() {
    if (!microPanel) return;
    if (microPanel.extName) {
      // 插件元素：撤销预览 = 回到已存规则（整包重下发）
      const n = microPanel.extName;
      microPanel = null;
      applyExtMicro(n);
      return;
    }
    clearMicroInline(microPanel.el);
    microPanel = null;
  }

  // —— iframe 面板的微调接入 ——
  function frameKeyOf(doc) {
    const fr = [...document.querySelectorAll('iframe')].find((f) => {
      try {
        return f.contentDocument === doc;
      } catch (e) {
        return false;
      }
    });
    if (!fr) return null;
    try {
      return `frame:${new URL(fr.src).pathname}|`;
    } catch (e) {
      return null;
    }
  }
  function onMicroFrameContext(e) {
    if (!editMode) return;
    if (e.target.closest && e.target.closest('.micro-panel')) return;
    const prefix = frameKeyOf(e.target.ownerDocument);
    if (!prefix) return;
    e.preventDefault();
    let target = e.target;
    if (target && target.nodeType !== 1) target = target.parentElement;
    if (target && target.closest) {
      const svgHit = target.closest('svg');
      if (svgHit && svgHit.closest('button')) target = svgHit.closest('button');
      else if (target instanceof SVGElement) target = target.closest('button') || target.parentElement || target;
    }
    if (!target || !target.closest) return;
    // 候选：别名表对插件页大概率不命中，命中就用语义名，否则走语义类名/结构选择器兜底
    for (const a of ELEMENT_ALIASES) {
      const el = target.closest(a.sel);
      if (el) {
        openMicroPanel(e.clientX, e.clientY, a.name, a.sel, el, prefix + a.name);
        return;
      }
    }
    const auto = autoClassTarget(target);
    if (auto) {
      openMicroPanel(e.clientX, e.clientY, auto.name, auto.sel, auto.el, prefix + auto.name);
      return;
    }
    openMicroPanel(e.clientX, e.clientY, '元素', structuralSelector(target), target, `${prefix}元素`);
  }
  const microFramesBound = new Set(); // 已挂右击监听的 iframe 文档
  function bindMicroFrames() {
    for (const f of document.querySelectorAll('iframe')) {
      let doc = null;
      try {
        doc = f.contentDocument;
      } catch (e) {
        continue; // 跨域（外部网页）：够不着，也不该管
      }
      if (!doc || microFramesBound.has(doc)) continue;
      try {
        doc.addEventListener('contextmenu', onMicroFrameContext);
        microFramesBound.add(doc);
        // 悬停虚线高亮：主文档走 body.micro-edit-mode 的全局规则，iframe 文档没有这套
        // 样式，单独注入一份（颜色取主题 brand 的固定值）
        let hs = doc.getElementById('pawch-micro-hover');
        if (!hs) {
          hs = doc.createElement('style');
          hs.id = 'pawch-micro-hover';
          hs.textContent = '*:hover { outline: 1px dashed rgba(186,138,120,.55); outline-offset: -1px; }';
          (doc.head || doc.documentElement).appendChild(hs);
        }
      } catch (e) {}
    }
  }
  // frame:<路径>| 前缀的规则注入对应 iframe 文档；iframe 重加载后样式与 data-mt 标记补回
  function applyFrameMicro() {
    const frames = {};
    for (const [k, t] of Object.entries(microTweaks || {})) {
      if (!k.startsWith('frame:') || !t) continue;
      const sep = k.indexOf('|');
      if (sep < 7) continue;
      const path = k.slice(6, sep);
      (frames[path] = frames[path] || {})[k] = t;
    }
    for (const f of document.querySelectorAll('iframe')) {
      let doc = null;
      try {
        doc = f.contentDocument;
      } catch (e) {
        continue;
      }
      if (!doc) continue;
      let path = null;
      try {
        path = new URL(f.src).pathname;
      } catch (e) {}
      const rules = path && frames[path];
      const old = doc.getElementById('pawch-micro-frame');
      if (!rules) {
        if (old) old.remove();
        continue;
      }
      const cssText = microCss(rules, true);
      if (old) old.textContent = cssText;
      else {
        const st = doc.createElement('style');
        st.id = 'pawch-micro-frame';
        st.textContent = cssText;
        (doc.head || doc.documentElement).appendChild(st);
      }
      // 仅此元素的规则：iframe 重载后按路径补盖 data-mt 标记
      for (const t of Object.values(rules)) {
        if (t.uid && t.path) {
          try {
            const el = findByPath(t.path, doc.documentElement);
            if (el) el.setAttribute('data-mt', t.uid);
          } catch (e) {}
        }
      }
    }
  }
  // iframe（重）加载：编辑模式开着就补挂监听；微调规则回放到新文档（无论编辑模式开关）
  document.addEventListener(
    'load',
    (e) => {
      if (!e.target || e.target.tagName !== 'IFRAME') return;
      if (editMode) bindMicroFrames();
      setTimeout(() => applyFrameMicro(), 150);
    },
    true
  );

  // —— 跨源插件 iframe 的微调（桥协议扩展，2026-11-03 深夜立项）——
  // 插件 iframe 是 sandbox 不透明源：主文档 contentDocument 够不着（抛异常），
  // 右击/悬停/注入都要插件桥配合。协议：宿主→插件 {type:'pawch:micro',op:enter|exit|apply}，
  // 插件→宿主 {type:'pawch:micro-ack'}（enter 应答=支持微调）与
  // {type:'pawch:micro-pick',pick:{sel,label,styles,x,y}}（微调监听态下右击上报）。
  // 旧插件没有这套 op：ack 不回来 = 面板显示「暂不支持微调」，不崩、不受影响。
  let extMicroCap = {}; // 插件名 → true(已应答支持) / false(超时未认) / undefined(待定)
  const EXT_RULE_PREFIX = 'frame:ext:';
  function extRulePrefix(name) {
    return `${EXT_RULE_PREFIX}${name}|`;
  }
  function extCssFor(name, overrideEntry) {
    const prefix = extRulePrefix(name);
    const bare = {};
    for (const [k, t] of Object.entries(microTweaks || {})) {
      if (k.startsWith(prefix) && t) bare[k.slice(prefix.length)] = t;
    }
    if (overrideEntry && overrideEntry.sel) bare[overrideEntry.sel] = overrideEntry;
    return microCss(bare, true); // 插件文档里的选择器，不挂 #app
  }
  function sendExtCss(name, css) {
    const el = extFrameEls[name];
    if (el && el.contentWindow) el.contentWindow.postMessage({ type: 'pawch:micro', op: 'apply', css }, '*');
  }
  function sendExtMicro(name, op, extra) {
    const el = extFrameEls[name];
    if (el && el.contentWindow) el.contentWindow.postMessage({ type: 'pawch:micro', op, ...(extra || {}) }, '*');
  }
  // 编辑模式开关同步到所有已登记的插件 iframe：enter(listen) / exit。
  // 600ms 内没 ack 的标记「不支持」——面板右击 iframe 时给出说明而不是没反应。
  function syncExtMicro() {
    for (const e of enabledExts) {
      if (!extFrameEls[e.name]) continue;
      if (editMode) {
        if (extMicroCap[e.name] !== true) extMicroCap = { ...extMicroCap, [e.name]: undefined };
        sendExtMicro(e.name, 'enter', { listen: true });
      } else {
        sendExtMicro(e.name, 'exit');
      }
    }
    if (editMode) {
      setTimeout(() => {
        const next = {};
        for (const e of enabledExts) next[e.name] = extMicroCap[e.name] === true;
        extMicroCap = next;
      }, 600);
    }
  }
  $: syncExtMicro(editMode);
  function applyExtMicro(name) {
    sendExtCss(name, extCssFor(name));
  }
  function applyExtMicroAll() {
    // 对所有在跑的插件面板都发一遍（没有规则的发空 CSS=清掉旧样式），
    // 否则删光规则后 iframe 里残留上一次的注入样式。
    // 启动早期 applyCustomCss() 会先于 enabledExts/extFrameEls 的 let 声明跑到这里
    // （TDZ）——跨源注入只在桥就位后有意义，早期直接跳过。
    try {
      for (const e of enabledExts) {
        if (!extFrameEls[e.name]) continue;
        sendExtCss(e.name, extCssFor(e.name));
      }
    } catch (e) {
      /* 启动早期插件桥未就位：跳过 */
    }
  }
  // 微调面板打开时按面板当前值整包预览（应用才按 dirty 落盘）
  function extPreviewEntry() {
    const p = microPanel;
    const prev = (microTweaks[p.key] && typeof microTweaks[p.key] === 'object') || {};
    const entry = { sel: p.sel, name: p.name };
    if (p.bgTransparent) {
      if (prev.bg) {
        entry.bg = prev.bg;
        entry.bgOpacity = prev.bgOpacity ?? 100;
      }
    } else {
      entry.bg = p.bg;
      entry.bgOpacity = p.bgOpacity;
    }
    entry.color = p.color;
    entry.radius = `${p.radius}px`;
    entry.border = `${p.border}px`;
    entry.borderColor = p.borderColor;
    const opaque = !p.bgTransparent && p.bgOpacity >= 85;
    if ((parseInt(p.blur, 10) || 0) > 0 && !opaque) entry.blur = p.blur;
    entry.fontSize = `${p.fontSize}px`;
    return entry;
  }
  function openExtMicroPanel(name, pick, at) {
    const el = extFrameEls[name];
    const rect = at ? { left: 0, top: 0 } : (el ? el.getBoundingClientRect() : { left: 0, top: 0 });
    const st = pick.styles || {};
    const cs = (k, d) => (st[k] != null && st[k] !== '' ? st[k] : d);
    const numOr = (v, fallback) => {
      const n = parseFloat(v);
      return Number.isFinite(n) ? n : fallback;
    };
    const key = extRulePrefix(name) + (pick.sel || '元素');
    const saved = microTweaks[key] || null;
    const bgParsed = parseCssColor(cs('bg', ''));
    const bgNow = rgbToHex(cs('bg', ''));
    microPanel = {
      extName: name,
      x: Math.max(8, Math.min(at ? at.x : rect.left + (pick.x || 40), window.innerWidth - 300)),
      y: Math.max(8, Math.min(at ? at.y : rect.top + (pick.y || 40), window.innerHeight - 340)),
      key,
      name: `${pick.label || pick.sel || '插件元素'} · ${name}`,
      sel: pick.sel || '',
      el: null,
      color: saved?.color || rgbToHex(cs('color', '')) || '#000000',
      bg: saved?.bg || bgNow || '#000000',
      bgTransparent: !saved?.bg && (!bgParsed || bgParsed.a === 0),
      bgDirty: false,
      colorDirty: false,
      radiusDirty: false,
      blurDirty: false,
      fontSizeDirty: false,
      borderDirty: false,
      borderColorDirty: false,
      bgOpacity: saved != null && saved.bgOpacity != null ? saved.bgOpacity : bgParsed ? Math.round(bgParsed.a * 100) : 0,
      radius: numOr(saved?.radius, Math.round(numOr(cs('radius', 0), 0))),
      border: numOr(saved?.border, Math.round(numOr(cs('border', 0), 0))),
      borderColor: saved?.borderColor || rgbToHex(cs('borderColor', '')) || '#000000',
      blur: numOr(saved?.blur, numOr(cs('blur', 0), 0)),
      fontSize: numOr(saved?.fontSize, Math.round(numOr(cs('fontSize', 14), 14))),
      // 插件元素本身透明时，桥会一并上报真正画底色的祖先（body/卡片壳）——给「改上级」入口
      bgHost: pick.bgHost
        ? { name: pick.bgHost.label || pick.bgHost.sel || '上级元素', sel: pick.bgHost.sel || '', styles: pick.bgHost.styles || {}, el: null }
        : null,
      // 面板最底层的 body（插件页整块深色就是它画的）：给「改整个面板底色」直达入口
      root: pick.root ? { name: pick.root.label || '面板底色', sel: pick.root.sel || 'body', styles: pick.root.styles || {} } : null,
      // 跨源拿不到稳定节点路径，「仅此元素」的 data-mt 标记补盖不回来——插件元素按选择器整类生效
      scope: 'all',
    };
  }

  // —— CSS 规则导出 / 导入：把「微调规则」与「手写 CSS」打包成一个 JSON 文件，可备份、换机迁移 ——
  let cssImportInput;
  const CSS_RULES_KIND = 'agent-ui-css-rules';

  function exportCssRules() {
    const payload = {
      type: CSS_RULES_KIND,
      version: 1,
      exportedAt: new Date().toISOString(),
      customCss,
      microTweaks,
    };
    const blob = new Blob([JSON.stringify(payload, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = 'ui-css-rules.json';
    document.body.appendChild(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
    const n = Object.keys(microTweaks || {}).length;
    settingsMsg = `已导出 CSS 规则（微调 ${n} 条${customCss.trim() ? ' + 手写 CSS' : ''}）`;
  }

  function onImportCssRules(e) {
    const file = e.target.files && e.target.files[0];
    if (e.target) e.target.value = ''; // 复位，允许再次选择同一个文件
    if (!file) return;
    const reader = new FileReader();
    reader.onerror = () => (settingsMsg = '导入失败：文件读取错误');
    reader.onload = async (ev) => {
      const text = String(ev.target.result || '');
      let css = '';
      let tweaks = null; // null 表示"这次不动微调规则"（导入纯 CSS 文本时）
      try {
        const data = JSON.parse(text);
        if (!data || typeof data !== 'object' || Array.isArray(data)) throw new Error('不是有效的规则文件');
        if ('customCss' in data || 'microTweaks' in data || data.type === CSS_RULES_KIND) {
          css = typeof data.customCss === 'string' ? data.customCss : '';
          tweaks = data.microTweaks && typeof data.microTweaks === 'object' && !Array.isArray(data.microTweaks) ? data.microTweaks : {};
        } else {
          tweaks = data; // 兼容只含微调规则对象的旧文件
        }
      } catch (err) {
        // 看起来想写 JSON 却解析失败 → 报错；否则当作纯手写 CSS 文本导入
        if (/^[{[]/.test(text.trim())) {
          settingsMsg = `导入失败：${err.message || err}`;
          return;
        }
        css = text;
      }
      let n = 0;
      if (tweaks != null) {
        // 只收下结构合法的微调条目，避免脏数据让微调模式崩溃
        const clean = {};
        for (const [k, v] of Object.entries(tweaks)) {
          if (v && typeof v === 'object' && typeof v.sel === 'string' && v.sel) clean[k] = v;
        }
        tweaks = clean;
        n = Object.keys(tweaks).length;
      }
      const existing = Object.keys(microTweaks || {}).length;
      const overwriteMicro = tweaks != null;
      const scope = overwriteMicro ? `当前的 ${existing} 条微调规则和手写 CSS` : '当前的手写 CSS';
      if ((existing || customCss.trim()) && !(await confirmAsk(`导入将覆盖${scope}，继续？`))) return;
      customCss = css;
      if (overwriteMicro) {
        microTweaks = tweaks;
        try { localStorage.setItem('microTweaks', JSON.stringify(tweaks)); } catch (err) {}
        unstampAll();
      }
      applyCustomCss();
      settingsMsg = overwriteMicro
        ? `已导入 CSS 规则（微调 ${n} 条${css.trim() ? ' + 手写 CSS' : ''}）`
        : '已导入手写 CSS';
    };
    reader.readAsText(file);
  }

  // 启动时应用：微调规则 + 手动 CSS
  applyCustomCss();
  // (hmr-probe)

  // —— 对话 ——
  // 发送（027）：当前显示会话没在跑 → 直接发；正在跑 → **排队**（不发出去），
  // 气泡上出现「立即发送」按钮，点它 = 引导当前这一轮（并进去、不打断）。
  // 别的会话在跑不影响这里（每条会话各有自己的流）。
  async function send() {
    const msg = input.trim();
    const atts = [...pendingAttachments];
    const convs = [...pendingConvs];
    if (!msg && atts.length === 0 && convs.length === 0) return;
    const key = curKey();
    input = '';
    // F4：发送后清空是程序化赋值，不触发 input 事件，高度得手动回收。
    // **必须先 await tick()**：DOM 的 value 要等 tick 才刷成空，不等就量高度，
    // 量到的是清空前的长内容——框停在半截高（真机实测「长消息发出去输入框不收回」）。
    // 插件 session.create 预填路径（下方 bridge）同一个坑，那边早有 await tick()。
    await tick();
    resetComposerHeight();
    pendingAttachments = [];
    pendingConvs = [];
    persistComposer(key); // 已发出去：该会话的草稿要清掉，别切回来又冒出来
    if (streams[key]) {
      queueOut(key, msg, atts, convs);
      return;
    }
    await deliver(key, msg, atts, { convs });
  }

  // 排队：先显示成「排队中」气泡，当前这一轮结束后自动按序发出
  function queueOut(key, text, atts, convs = []) {
    const item = { id: nextId(), text, atts, convs, ts: Date.now() };
    outbox = { ...outbox, [key]: [...(outbox[key] || []), item] };
    pushMsg(key, { id: item.id, role: 'user', content: text, attachments: atts, convs, queued: true, ts: item.ts });
  }

  // 把「已经上屏的那条用户气泡」转成排队（服务端说这个会话在跑时用；不新增气泡）
  function markQueued(key, text, atts, convs = []) {
    const arr = sessionMsgs[key] || [];
    const hit = [...arr].reverse().find((m) => m.role === 'user' && m.content === text && !m.queued);
    const id = hit ? hit.id : nextId();
    updMsgs(key, (a) =>
      hit ? a.map((m) => (m.id === id ? { ...m, queued: true } : m)) : [...a, { id, role: 'user', content: text, attachments: atts, convs, queued: true, ts: Date.now() }]
    );
    outbox = { ...outbox, [key]: [...(outbox[key] || []), { id, text, atts, convs, ts: Date.now() }] };
    return id;
  }

  // 真正发一轮（key 会话；调用时该会话没在跑）。返回 false = 这条没送进服务端。
  async function deliver(key, text, atts, opts = {}) {
    const convs = opts.convs || [];
    // 引用对话块只在发送侧拼（模型拿全文），用户气泡保持「我打的那句 + 引用 chip」
    const fullMsg = (text || '') + convBlocks(convs);
    const effectiveMsg = fullMsg.trim() || (atts.length > 0 ? '请查看分析附带的媒体文件' : '');
    if (!effectiveMsg) return true;
    // 发送时收起所有输入区菜单
    plusMenuOpen = false;
    abilityMenuOpen = false;
    modelMenuOpen = false;
    // 新会话：顶栏标题即时取首条消息（与后端初始标题一致），不等标题生成
    if (!currentSessionId && effectiveMsg) currentTitle = effectiveMsg.slice(0, 50);
    const history = (sessionMsgs[key] || [])
      .filter((m) => (m.role === 'user' && m.content && !m.queued) || (m.role === 'assistant' && m.content && !m.streaming))
      .map((m) => ({ role: m.role, content: m.content }));
    history.push({ role: 'user', content: effectiveMsg, ts: Date.now() });
    const asstId = nextId();
    // 出队 / 立即发送的那条：原地把「排队中」气泡转正（保持顺序），不新增用户气泡。
    // 引导（steer + silentSteer）反过来：**撤掉**排队气泡——
    // 引导是静默的，不占对话气泡位，只显示「已引导」标记。
    const hasBubble = opts.bubbleId && (sessionMsgs[key] || []).some((m) => m.id === opts.bubbleId);
    if (opts.silentSteer) {
      updMsgs(key, (a) => [
        ...a.filter((m) => m.id !== opts.bubbleId),
        { id: asstId, role: 'assistant', content: '', thinking: '', tools: [], parts: [], streaming: true, workStartedAt: Date.now(), ts: Date.now() },
      ]);
    } else {
      updMsgs(key, (a) => [
        ...a.map((m) => (opts.bubbleId && m.id === opts.bubbleId ? { ...m, queued: false, steered: !!opts.steer, interrupted: !!opts.interrupt } : m)),
        ...(hasBubble ? [] : [{ id: nextId(), role: 'user', content: text, attachments: atts, convs, ts: Date.now() }]),
        { id: asstId, role: 'assistant', content: '', thinking: '', tools: [], parts: [], streaming: true, workStartedAt: Date.now(), ts: Date.now() },
      ]);
    }
    const ac = new AbortController();
    // 引导/打断不是一轮对话：**不登记 streams**——登记会顶掉运行中那轮的
    // 流条目，引导响应一结束 finally 又把它删掉，原轮还在跑本页却以为空闲了
    // （再发消息走错分支、出队逻辑乱序）。引导的 SSE 只回执不产出，用不上这些。
    const ownsTurn = opts.steer !== true && opts.interrupt !== true;
    if (ownsTurn) streams = { ...streams, [key]: { asstId, controller: ac } };
    syncCur();
    const stream = { asstId, controller: ac };
    let skey = key; // 会话 id 到手后 key 会从 '@new' 变成真 id
    const patch = (fn) => patchAsst(skey, stream.asstId, fn);
    let ok = false;
    try {
      const res = await fetch('/api/chat', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          messages: history,
          message: effectiveMsg,
          attachments: atts.length > 0 ? atts.map((a) => ({ name: a.name, mime: a.mime, data: a.data })) : undefined,
          model,
          base_url: baseUrl,
          approval_mode: approvalBySession[key] || approvalMode,
          session_id: skey === '@new' ? '' : skey,
          steer: opts.steer === true,
          interrupt: opts.interrupt === true,
        }),
        signal: ac.signal,
      });
      if (!res.ok) {
        // 服务端拒绝（如 409 同会话并发）：带出服务端文案，不伪装成连接错误
        let detail = '';
        try {
          detail = (await res.json()).error || '';
        } catch {}
        const err = new Error(detail || `HTTP ${res.status}`);
        err.busy = res.status === 409;
        throw err;
      }
      ok = true;
      const reader = res.body.getReader();
      const decoder = new TextDecoder();
      let buf = '';
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        buf += decoder.decode(value, { stream: true });
        let idx;
        while ((idx = buf.indexOf('\n\n')) !== -1) {
          const raw = buf.slice(0, idx);
          buf = buf.slice(idx + 2);
          for (const line of raw.split('\n')) {
            if (!line.startsWith('data: ')) continue;
            try {
              const d = JSON.parse(line.slice(6));
              if (d.session) {
                // 新会话：服务端分配了 id → 缓冲 / 流 / 队列挪到真 id 下（显示中的话直接切过去）
                rekey(skey, d.session);
                skey = d.session;
              } else if (d.queued) {
                // 服务端判成「这个会话在跑」（流断了/别的窗口刚起了一轮）：本条转排队、撤掉空回答气泡。
                // **服务端接管了这条的投递**（本轮收尾它自己排水跑掉）——必须把它从出队队列摘掉：
                // 否则「排队→流结束→出队→再发→又排队」就是无限请求风暴
                // （2026-09-27 真机踩到：SSE 断流后发一条消息，10 秒被重发 2000 次塞满队列）。
                // 跑完的答案也拿不到（d.turn 走的是已死的原流）——由 reconcileServerQueued 对账。
                updMsgs(skey, (a) => a.filter((m) => m.id !== stream.asstId));
                const qid = markQueued(skey, effectiveMsg, atts);
                outbox = { ...outbox, [skey]: (outbox[skey] || []).filter((x) => x.id !== qid) };
                reconcileServerQueued(skey);
              } else if (d.steered) {
                // 这条已被并进正在跑的一轮（引导）：本页不再给它单开回答气泡
                updMsgs(skey, (a) => a.filter((m) => m.id !== stream.asstId));
              } else if (d.turn) {
                // 排队的消息开始跑了：气泡从「排队中」转正式 + 新开一个回答气泡
                // （已引导过的也算命中——正常到不了这里，但收尾竞态下别让它再补一条重复气泡）
                const adds = [];
                for (const m of d.turn.messages || []) {
                  const q = (sessionMsgs[skey] || []).find((x) => (x.queued || x.steered) && x.role === 'user' && x.content === m.content);
                  if (q) updMsgs(skey, (a) => a.map((x) => (x.id === q.id ? { ...x, queued: false } : x)));
                  else adds.push({ id: nextId(), role: 'user', content: m.content, ts: m.ts || Date.now() });
                }
                const nid = nextId();
                stream.asstId = nid;
                updMsgs(skey, (a) => [...a, ...adds, { id: nid, role: 'assistant', content: '', thinking: '', tools: [], parts: [], streaming: true, workStartedAt: Date.now(), ts: Date.now() }]);
              } else if (d.content) {
                // 正文 = 本段思考结束：把这段的跨度累进总时长并停表（实时数字不再走）。
                // **不能清计时锚点 了事**——真渠道的思考常以少量大分片到达，按「分片间隔」
                // 累加会得 0，时长就显示不出来（真机实测）；按阶段跨度算才对。
                // parts 同步追加：正文把前后的「过程块」隔开，按真实顺序渲染。
                patch((m) => {
                  const closed = m.thinkPhaseStart ? Date.now() - m.thinkPhaseStart : 0;
                  return {
                    ...m,
                    content: m.content + d.content,
                    parts: appendContent(settleThinking(m.parts, closed), d.content),
                    thinkMs: (m.thinkMs || 0) + closed,
                    thinkPhaseStart: 0,
                  };
                });
              }
              // 思考分片：文字累积；首片开表（阶段跨度计时），多步任务里后续思考再开新表
              else if (d.reasoning)
                patch((m) => ({
                  ...m,
                  thinking: m.thinking + d.reasoning,
                  parts: appendThinking(m.parts, d.reasoning),
                  thinkPhaseStart: m.thinkPhaseStart || Date.now(),
                }));
              // 降级链换渠道重试：丢掉本步已推的半截内容（长度由服务端按 UTF-16 码元数给出）
              else if (d.reset)
                patch((m) => ({
                  ...m,
                  content: m.content.slice(0, Math.max(0, m.content.length - (d.reset.content || 0))),
                  thinking: (m.thinking || '').slice(0, Math.max(0, (m.thinking || '').length - (d.reset.reasoning || 0))),
                  parts: trimParts(trimParts(m.parts, 'content', d.reset.content || 0), 'thinking', d.reset.reasoning || 0),
                }));
              else if (d.steer) {
                // 引导/打断的消息并入了本轮。本页自己发过的（localSteers 按序配对）跳过——
                // 引导的已在发送时显示「已引导」标记（不发气泡），打断的气泡已转正。
                // 别的窗口发的在这里补：打断=正常用户气泡，引导=「已引导」静默标记（不占气泡位）。
                const add = [];
                for (const m of d.steer.messages || []) {
                  // content 可能是内容块数组（2026-11-03 引导带附件）：取文本部分比对/展示；
                  // 带附件时服务端会在原文后追加「附件已落盘/已内联」的提示，比对用前缀
                  const st = contentText(m.content);
                  if (localSteers.length > 0 && (st === localSteers[0] || st.startsWith(localSteers[0]))) localSteers.shift();
                  else if (m.interrupted) add.push({ id: nextId(), role: 'user', content: st, interrupted: true, ts: m.ts || Date.now() });
                  else guided = { ...guided, [skey]: [...(guided[skey] || []), { text: st }] };
                }
                if (add.length) updMsgs(skey, (a) => [...a, ...add]);
              } else if (d.tool_call)
                patch((m) => {
                  const closed = m.thinkPhaseStart ? Date.now() - m.thinkPhaseStart : 0;
                  const tools = [...(m.tools || [])];
                  // 幂等（2026-10-31 流式预显示）：模型吐参数阶段已建过同名 running 行
                  //（tool_call_args 流进来的），这里只更新参数不再新增，否则一行变两行
                  let dup = -1;
                  for (let i = tools.length - 1; i >= 0; i--) {
                    if (tools[i].name === d.tool_call.name && tools[i].status === 'running') { dup = i; break; }
                  }
                  if (dup >= 0) {
                    tools[dup] = { ...tools[dup], args: d.tool_call.args };
                    // 执行前的完整参数必须同步进 parts：流式参数有节流，末尾一小段
                    // 可能从没发过 tool_call_args，不同步的话 todo 清单卡会一直
                    // 解析半截 JSON（2026-11-03 真机实测「自始至终没看到清单」）
                    return {
                      ...m,
                      tools,
                      parts: settleToolArgs(m.parts, dup, d.tool_call.name, d.tool_call.args),
                      thinkMs: (m.thinkMs || 0) + closed,
                      thinkPhaseStart: 0,
                    };
                  }
                  const toolIndex = (m.tools || []).length;
                  return {
                    ...m,
                    tools: [...tools, { name: d.tool_call.name, args: d.tool_call.args, status: 'running' }],
                    parts: addToolPart(settleThinking(m.parts, closed), {
                      name: d.tool_call.name,
                      args: d.tool_call.args,
                      status: 'running',
                      toolIndex,
                    }),
                    thinkMs: (m.thinkMs || 0) + closed, // 调工具 = 本段思考结束，停表
                    thinkPhaseStart: 0,
                  };
                });
              else if (d.tool_call_args)
                patch((m) => {
                  // 流式参数补长：只动最后一个同名 running 行（工具还没真正开跑）
                  const tools = [...(m.tools || [])];
                  let hit = -1;
                  for (let i = tools.length - 1; i >= 0; i--) {
                    if (tools[i].name === d.tool_call_args.name && tools[i].status === 'running') {
                      hit = i;
                      break;
                    }
                  }
                  if (hit < 0) return m;
                  tools[hit] = { ...tools[hit], args: d.tool_call_args.args };
                  return {
                    ...m,
                    tools,
                    parts: settleToolArgs(m.parts, hit, d.tool_call_args.name, d.tool_call_args.args),
                  };
                });
              else if (d.tool_result)
                patch((m) => {
                  const tools = [...(m.tools || [])];
                  for (let i = tools.length - 1; i >= 0; i--) {
                    if (tools[i].name === d.tool_result.name && tools[i].status === 'running') {
                      tools[i] = { ...tools[i], status: d.tool_result.ok ? 'ok' : 'fail', preview: d.tool_result.preview };
                      break;
                    }
                  }
                  const parts = settleToolPart(m.parts, d.tool_result.name, d.tool_result.ok, d.tool_result.preview);
                  // ask 工具收工 = 这个提问已终结（已答/已跳过/已作废）：dock 上的提问卡收掉。
                  // 答案记录已由 answerAsk 挂回 ask 工具段；这里只兜「没从 dock 走」的路径。
                  if (d.tool_result.name === 'ask' && m.question && m.question.answered === null) {
                    return { ...m, tools, parts, question: { ...m.question, answered: { dismissed: true } } };
                  }
                  return { ...m, tools, parts };
                });
              else if (d.approval_request)
                patch((m) => {
                  const appr = {
                    id: d.approval_request.id,
                    tool: d.approval_request.tool,
                    args: d.approval_request.args,
                    risk: d.approval_request.risk ?? 1,
                    riskLabel: d.approval_request.risk_label || '',
                    reason: d.approval_request.reason || '',
                    decided: null,
                  };
                  // 待批的浮到输入框上方；已决定的记录挂回时间线的工具卡（2026-11-03）
                  const parts = attachApprovalToToolPart(m.parts, appr);
                  const inFlow = Array.isArray(parts) && parts.some((p) => p && p.approval && p.approval.id === appr.id);
                  return { ...m, approval: inFlow ? { ...appr, inFlow: true } : appr, parts };
                });
              else if (d.approval_decided)
                patch((m) => {
                  if (!(m.approval && m.approval.id === d.approval_decided.id)) return m;
                  return {
                    ...m,
                    approval: { ...m.approval, decided: d.approval_decided.approved },
                    parts: settleApprovalOnParts(m.parts, d.approval_decided.id, d.approval_decided.approved),
                  };
                });
              else if (d.ask_request)
                // 阻塞式提问（ZCode AskUserQuestion 同款）：挂到当前流式消息上，
                // 与审批共用输入框上方的 dock 槽位派生；回答经 /api/ask 回传给挂起中的工具
                patch((m) => ({ ...m, question: { ...d.ask_request, answered: null, picked: [], other: '' } }));
              else if (d.checkpoint) {
                pushMsg(skey, { id: nextId(), role: 'cp', cp: d.checkpoint });
                refreshDock(); // git 面板数据（基线/分支/脏数）更新
                // git 面板正开着（且看的是本任务档）：新检查点到了就地重拉，跑完就能看见
                if (rpTabs.some((t) => t.kind === 'git') && rvSource === 'task') loadReview();
                // 回滚锚点（2026-10-27）：任务起点基线记到触发的那条用户消息上，
                // 之后「回滚」按它恢复工作区文件
                updMsgs(skey, (a) => {
                  for (let i = a.length - 1; i >= 0; i--) {
                    if (a[i].role === 'user' && !a[i].queued && !a[i].cp) {
                      const next = [...a];
                      next[i] = { ...next[i], cp: d.checkpoint.id };
                      return next;
                    }
                  }
                  return a;
                });
              }
              else if (d.notice)
                pushMsg(skey, { id: nextId(), role: 'system', content: '[通知] ' + d.notice });
              else if (d.notice_clear)
                // 临时通知撤回（「正在原渠道自动重试…」重试成功）：删掉最后一条同文系统消息，
                // 别让它一直挂在对话里（2026-11-01 真机实测）
                updMsgs(skey, (a) => {
                  const want = '[通知] ' + d.notice_clear;
                  for (let i = a.length - 1; i >= 0; i--) {
                    if (a[i].role === 'system' && a[i].content === want) {
                      const next = [...a];
                      next.splice(i, 1);
                      return next;
                    }
                  }
                  return a;
                });
              else if (d.title) {
                loadSessions(true); //  TITLE：标题已生成，刷新会话列表
                if (d.title && d.title !== currentTitle) currentTitle = d.title; // 顶栏同步模型生成的标题
              }
              else if (d.context_usage) {
                // 用量按会话记账（后台会话写各自账本）；正在看的才刷到显示。
                // 圆环的**唯一**写入口：后端按「实报锚点 + 带符号增量」算好 projected 推来，
                // 前端只做展示，不再自己加减（旧写法与 api_usage 两处都写，口径会打架）
                const ctx = {
                  projected: d.context_usage.projected || 0,
                  pressure: d.context_usage.pressure ?? null,
                  window: d.context_usage.window || 0,
                  breakdown: d.context_usage.breakdown || null,
                };
                usageBySession = {
                  ...usageBySession,
                  [skey]: { ctx, api: (usageBySession[skey] || {}).api || { prompt: 0, cached: 0 } },
                };
                if (skey === curKey()) ctxUsage = ctx;
              }
              else if (d.api_usage) {
                // 渠道实报只进「累计流量」账（平均缓存命中率行）；圆环不在这里改
                const prev = (usageBySession[skey] || {}).api || { prompt: 0, cached: 0 };
                const api = { prompt: prev.prompt + (d.api_usage.prompt_tokens || 0), cached: prev.cached + (d.api_usage.cached_tokens || 0) };
                usageBySession = {
                  ...usageBySession,
                  [skey]: { ctx: (usageBySession[skey] || {}).ctx || ctxUsage, api },
                };
                if (skey === curKey()) apiTotals = api;
              } else if (d.turn_stats) {
                // 消息信息卡（2026-10-26）：计时/渠道/模型挂到正在流式的那条回答上
                patch((m) => ({ ...m, stats: { ...(m.stats || {}), ...d.turn_stats } }));
              } else if (d.step_limit) {
                patch(endTurn);
                pushMsg(skey, { id: nextId(), role: 'error', content: `已达步数上限 ${d.step_limit}，本轮先停在这（进度都在，重启不丢）——回复「继续」我会接着跑；想一次跑完更多，去 config.toml 把 max_steps 调大或设 0（不限）` });
              } else if (d.error) {
                patch(endTurn);
                pushMsg(skey, { id: nextId(), role: 'error', content: d.error });
              } else if (d.cancelled) {
                patch(endTurn);
                pushMsg(skey, { id: nextId(), role: 'system', content: '已停止' });
              } else if (d.done) {
                patch(endTurn);
              }
            } catch (e) {
              // 忽略坏行
            }
          }
        }
      }
    } catch (e) {
      if (e.name !== 'AbortError') {
        pushMsg(skey, { id: nextId(), role: 'error', content: e.busy ? e.message : `[连接错误] ${e}` });
      }
      // 停止后把还挂着的审批框标为失效，避免界面停在「等待决定」
      updMsgs(skey, (a) =>
        a.map((m) => (m.approval && m.approval.decided === null ? { ...m, approval: { ...m.approval, decided: false } } : m))
      );
      patch(endTurn);
    } finally {
      if (ownsTurn) {
        // 这条流结束（本会话可再次发送）；只清本会话的流登记，别的会话在跑不受影响。
        // 引导请求（ownsTurn=false）绝不能走这段：它的流条目根本没登记，误删会把
        // 还在跑的那一轮登记清掉（本页从此以为会话空闲，出队/重发全部乱套）。
        const nextStreams = { ...streams };
        delete nextStreams[skey];
        streams = nextStreams;
        localSteers = [];
        // 本轮结束：「已引导」标记完成使命（指令都已并入并被处理）
        const nextGuided = { ...guided };
        delete nextGuided[skey];
        guided = nextGuided;
        syncCur();
        // 流死了没产出任何内容（网络断/服务重启等）：撤掉空回答气泡，别留一个空壳。
        // 缓冲已被清掉的会话（切工作区 abort）不复活条目——否则空数组是 truthy，
        // 切回来会被当成「本地缓冲」用，历史照样全不见。
        if (sessionMsgs[skey]) {
          updMsgs(skey, (a) =>
            a.filter((m) => m.id !== stream.asstId || m.content || (m.tools && m.tools.length) || m.approval)
          );
        }
        loadSessions(); // 刷新会话列表（新会话出现/标题更新）
        // 时间线面板跟着本轮走：它只显示当前对话，不广播
        // 的话跑完一轮面板还是旧数据，得用户手点刷新
        notifyExtsSession(skey);
        // 出队（027）：这一轮跑完了，把排在最前的排队消息自动发出去（发完它再发下一条）
        drainOutbox(skey);
      }
    }
    return ok;
  }

  // 「立即发送 / 引导」（027 + 2026-10-22 分家）：把排队的这条马上并进正在跑的那一轮。
  // mode='interrupt'（立即发送）= 打断：审批/工具/流式让位，新指令立即生效（消息正常上气泡）；
  // mode='steer'（引导）= 不打断：跑完当前这步就并入，**静默**——撤掉排队气泡，
  // 只显示「已引导」标记，输入框立即可继续干别的。
  // 跑完了/没送进去会自动退回队列。
  async function sendNow(itemId, mode = 'interrupt') {
    const key = curKey();
    // 「服务端接管」的排队气泡不从 outbox 走：前端以为空闲时发消息，服务端回 queued
    // 后气泡留在列表里、outbox 里那条已被摘掉（见 deliver 的 d.queued 分支）。
    // 这时按钮要还能用——从气泡本身取回内容，否则点了没反应（引导一直发不出去）。
    const queuedMsg = (sessionMsgs[key] || []).find((m) => m.id === itemId && m.queued);
    const item =
      (outbox[key] || []).find((x) => x.id === itemId) ||
      (queuedMsg
        ? { id: queuedMsg.id, text: queuedMsg.content, atts: queuedMsg.attachments || [], convs: queuedMsg.convs || [] }
        : null);
    if (!item) return;
    const label = mode === 'steer' ? '引导' : '立即发送';
    // 先出队（免得本轮结束时又自动发一遍）
    outbox = { ...outbox, [key]: (outbox[key] || []).filter((x) => x.id !== itemId) };
    // 本页没有流不代表这一轮跑完了：页面重载/切回来后 streams 是空的，但服务端还在跑。
    // 这时按普通消息发出去只会再被服务端排进队列（点了引导还是「排队中」）。
    // 问一下服务端，真在跑就照常带 steer/interrupt 合进去；确实空闲才走普通一轮。
    let running = !!streams[key];
    if (!running && key !== '@new') {
      try {
        const c = await fetch(`/api/cancel?session=${encodeURIComponent(key)}`).then((r) => r.json());
        running = !!c.running;
      } catch {}
    }
    if (!running) {
      // 这一轮已经跑完了：按正常一轮发出去
      await deliver(key, item.text, item.atts, { bubbleId: item.id, convs: item.convs });
      return;
    }
    if (item.convs && item.convs.length) {
      pushMsg(key, { id: nextId(), role: 'error', content: `「${label}」暂不支持带引用对话的消息：这一轮结束后会自动发出去。` });
      return;
    }
    const nAtt = (item.atts || []).length;
    const sentText = item.text || (nAtt ? '请查看分析附带的媒体文件' : '');
    const guidedText = nAtt ? `${item.text || '附件消息'}（附件 ${nAtt}）` : item.text;
    localSteers.push(sentText);
    if (mode === 'steer') {
      // 引导：静默——撤气泡 + 「已引导」标记（带附件时标明数量）
      guided = { ...guided, [key]: [...(guided[key] || []), { text: guidedText }] };
    }
    const ok = await deliver(key, item.text, item.atts || [], {
      bubbleId: item.id,
      steer: mode === 'steer',
      silentSteer: mode === 'steer',
      interrupt: mode === 'interrupt',
    });
    if (!ok) {
      // 没送进服务端：退回队列，等这一轮结束自动发
      localSteers = localSteers.filter((t) => t !== sentText);
      if (mode === 'steer') {
        guided = { ...guided, [key]: (guided[key] || []).filter((g) => g.text !== guidedText) };
        // 静默撤掉的气泡补回来（恢复排队中状态）
        updMsgs(key, (a) => [...a, { id: item.id, role: 'user', content: item.text, queued: true }]);
      }
      outbox = { ...outbox, [key]: [item, ...(outbox[key] || [])] };
      if (mode === 'interrupt') {
        updMsgs(key, (a) => a.map((m) => (m.id === item.id ? { ...m, queued: true, steered: false, interrupted: false } : m)));
      }
    }
  }

  function stop() {
    // 会话级停止（D2）：只停当前会话在跑的这一轮，别的会话不受影响；
    // 会话 id 未知（极早期）时退化为停全部
    const q = currentSessionId ? `?session=${encodeURIComponent(currentSessionId)}` : '';
    fetch('/cancel' + q, { method: 'POST' });
    if (controller) controller.abort();
  }

  // —— 消息操作（回滚/编辑/删除/多选，2026-10-26 用户需求批）——
  function fmtTs(ts) {
    if (!ts) return '';
    const d = new Date(ts);
    const p = (n) => String(n).padStart(2, '0');
    return `${p(d.getHours())}:${p(d.getMinutes())}`;
  }
  function fmtMs(ms) {
    if (ms == null) return '—';
    return ms >= 1000 ? (ms / 1000).toFixed(1) + ' 秒' : ms + ' ms';
  }

  // 会话在跑就先停（编辑的前提：上下文要改写，继续输出没有意义），等服务端收尾
  async function ensureIdle(key) {
    if (!streams[key]) return;
    if (key === curKey()) stop();
    else {
      const q = key === '@new' ? '' : `?session=${encodeURIComponent(key)}`;
      fetch('/cancel' + q, { method: 'POST' });
    }
    for (let i = 0; i < 100; i++) {
      await new Promise((r) => setTimeout(r, 100));
      if (!streams[key]) return;
    }
  }

  // 服务端截断（409=服务端还在收尾，退避重试）。cp=任务起点基线 id：带上时先恢复
  // 工作区文件再截对话（回滚=文件+对话一起）
  async function truncateRequest(key, userIndex, mode, cp = '') {
    for (let i = 0; i < 25; i++) {
      const r = await fetch('/api/sessions/truncate', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ id: key, user_index: userIndex, mode, restore_checkpoint: cp || undefined }),
      });
      const d = await r.json().catch(() => ({}));
      if (r.ok) return d;
      if (r.status !== 409) throw new Error(d.error || `HTTP ${r.status}`);
      await new Promise((res) => setTimeout(res, 400));
    }
    throw new Error('会话还在收尾，稍后再试');
  }

  // 目标消息在会话文件里的「第几条 user 消息」序数（0 基）。
  // 前端视图比文件多出排队气泡（没发出去）与检查点行（纯前端）——排除 queued 即对齐；
  // 引导消息（steered）在文件里真有一条 user，必须计入。
  function fileUserIndex(key, m) {
    const arr = sessionMsgs[key] || [];
    const at = arr.findIndex((x) => x.id === m.id);
    let n = 0;
    for (let k = 0; k < at; k++) {
      const x = arr[k];
      if (x.role === 'user' && !x.queued) n++;
    }
    return n;
  }

  // 从服务端重拉当前会话视图（截断后以文件为真源自愈）
  async function reloadSessionView(key) {
    const r = await fetch(`/api/session?id=${encodeURIComponent(key)}`).then((x) => x.json());
    sessionMsgs = { ...sessionMsgs, [key]: rebuildFromSession(r.messages || []) };
    syncCur();
    loadSessions();
  }

  async function rollbackMsg(m) {
    const key = curKey();
    editingMsg = null;
    if (!(await confirmAsk(m.cp
      ? '回滚到这条消息？\n工作区文件会恢复到它发出前的检查点状态，这条和它之后的所有消息都会删除，不可恢复。'
      : '回滚到这条消息？\n这条消息没有文件基线记录（旧会话），只回滚对话、文件不动；这条和它之后的所有消息都会删除。'))) return;
    await ensureIdle(key);
    try {
      await truncateRequest(key, fileUserIndex(key, m), 'rollback', m.cp || '');
      await reloadSessionView(key);
    } catch (e) {
      pushMsg(key, { id: nextId(), role: 'error', content: `[回滚失败] ${e.message || e}` });
    }
  }

  async function deleteMsg(m) {
    const key = curKey();
    // 删除类纪律：不可恢复的删除先弹确认（「弹窗在哪」）。
    // 只删点中的这条；同一问答的另一半保留（不许一问一答一起删）。
    if (!(await confirmAsk('删除这条消息？\n只删这一条，同一问答里的回答会保留；删除后不可恢复，工作区文件不受影响。'))) return;
    await ensureIdle(key);
    try {
      await truncateRequest(key, fileUserIndex(key, m), 'delete_user');
      await reloadSessionView(key);
    } catch (e) {
      pushMsg(key, { id: nextId(), role: 'error', content: `[删除失败] ${e.message || e}` });
    }
  }

  function beginEdit(m) {
    editingMsg = { id: m.id, text: m.content };
  }

  async function saveEdit() {
    const key = curKey();
    const text = (editingMsg?.text || '').trim();
    const target = (sessionMsgs[key] || []).find((x) => x.id === editingMsg?.id);
    editingMsg = null;
    if (!text || !target) return;
    await ensureIdle(key);
    try {
      await truncateRequest(key, fileUserIndex(key, target), 'rollback');
      await reloadSessionView(key);
      // 编辑后的文本走正常发送路（时间戳/流式/落盘全在既有链路上）
      await deliver(key, text, [], {});
    } catch (e) {
      pushMsg(key, { id: nextId(), role: 'error', content: `[编辑重发失败] ${e.message || e}` });
    }
  }

  function enterSelect() {
    msgSelectMode = true;
    msgSelected = [];
    editingMsg = null;
  }
  function exitSelect() {
    msgSelectMode = false;
    msgSelected = [];
  }
  function toggleSelect(id) {
    msgSelected = msgSelected.includes(id) ? msgSelected.filter((x) => x !== id) : [...msgSelected, id];
  }
  // 多选模式：点消息整行即勾选（「点击消息那一行也会选中」）。
  // 排队气泡还没进文件、流式回答也还不能删，不参与多选。
  function selectRow(m) {
    if (!msgSelectMode) return;
    if (m.queued || (m.role === 'assistant' && m.streaming)) return;
    toggleSelect(m.id);
  }

  // 勾选消息 → (role, content) 目标表（多选精确删除：勾谁删谁，不连带问答对另一方）
  function selectedDeleteTargets() {
    const arr = sessionMsgs[curKey()] || [];
    return msgSelected
      .map((id) => arr.find((m) => m.id === id))
      .filter((m) => m && m.role === 'user' ? !m.queued : m && m.role === 'assistant')
      .map((m) => ({ role: m.role, content: m.content || '' }));
  }

  async function deleteSelected() {
    const key = curKey();
    const targets = selectedDeleteTargets();
    if (!targets.length) return;
    // 删除类纪律：不可恢复的删除先弹确认（只删勾选的，文件不动）
    if (!(await confirmAsk(`删除所选的 ${targets.length} 条消息？\n只删勾选的这些，问答对里的另一半不连带；删除后不可恢复，工作区文件不受影响。`))) return;
    await ensureIdle(key);
    try {
      const r = await fetch('/api/sessions/truncate', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ id: key, mode: 'delete_selected', targets }),
      });
      const d = await r.json().catch(() => ({}));
      if (!r.ok) throw new Error(d.error || `HTTP ${r.status}`);
      exitSelect();
      await reloadSessionView(key);
    } catch (e) {
      pushMsg(key, { id: nextId(), role: 'error', content: `[删除所选失败] ${e.message || e}` });
    }
  }

  // 生成上下文摘录：按对话顺序拼 markdown，进剪贴板——用户去别的对话粘贴即可当参考
  async function copySelectedAsContext() {
    const key = curKey();
    const arr = sessionMsgs[key] || [];
    const picked = arr.filter((m) => msgSelected.includes(m.id) && (m.role === 'user' || m.role === 'assistant'));
    if (!picked.length) return;
    const lines = [`【上下文摘录】来自对话「${currentTitle || '未命名'}」（${new Date().toLocaleString()}）`, ''];
    for (const m of picked) {
      lines.push(m.role === 'user' ? `【用户】${m.content}` : `【助手】${m.content}`);
    }
    try {
      await putClipboard(lines.join('\n\n'));
      ctxCopied = true;
      setTimeout(() => (ctxCopied = false), 2000);
    } catch (e) {
      pushMsg(key, { id: nextId(), role: 'error', content: `[复制失败] ${e.message || e}` });
    }
  }

  // 服务端接管排队消息后的对账（2026-09-27）：本页的 SSE 流已死，服务端跑这条消息时
  // 发的 d.turn / 回答流进的是那条死连接——轮询到服务端空闲后从会话文件重载，
  // 把「排队中」气泡对成真消息 + 补上错过的一切。不重发任何东西（消息已在服务端队列里）。
  function reconcileServerQueued(skey) {
    if (reconcileTimers[skey]) return;
    // 连续失败计数：服务没起/断网时 fetch 抛错走 catch，原来无限重试到页面关闭
    // （每 2s 一次永久空转）。攒够次数就自清定时器，把对账留给用户重连后的正常流程
    // （2026-10-08 修）。
    let misses = 0;
    reconcileTimers[skey] = setInterval(async () => {
      try {
        if (streams[skey]) return; // 本页又起了流：对账交给正常流程
        const c = await fetch(`/api/cancel?session=${encodeURIComponent(skey)}`).then((r) => r.json());
        if (c.running) return; // 服务端还在跑：继续等
        clearInterval(reconcileTimers[skey]);
        delete reconcileTimers[skey];
        // 这一轮已经结束：刷新快照的使命完成，清掉避免下次误用旧内容
        clearLiveStash();
        if (currentSessionId !== skey) {
          // 不在这个会话的页面：丢掉本地陈旧缓冲，下次打开时从服务端重建
          const next = { ...sessionMsgs };
          delete next[skey];
          sessionMsgs = next;
          return;
        }
        if (streams[skey]) return;
        const r = await fetch(`/api/session?id=${encodeURIComponent(skey)}`).then((r) => r.json());
        if ((r.messages || []).length > 0) {
          sessionMsgs = { ...sessionMsgs, [skey]: rebuildFromSession(r.messages || []) };
          syncCur();
          loadSessions();
        }
        // 文件重载会把「排队中」气泡冲掉：先把 outbox 里的消息铺回气泡，再按序补发
        // （刷新前排的队不能在重载后无声消失）。
        const q = outbox[skey] || [];
        if (q.length) {
          restoreQueuedOutbox(skey, q);
          drainOutbox(skey);
        }
      } catch (e) {
        // 网络抖动：下一跳再试；但服务长时间不可达时不能无限转——攒够次数收工
        misses += 1;
        if (misses >= 30) {
          clearInterval(reconcileTimers[skey]);
          delete reconcileTimers[skey];
        }
      }
    }, 2000);
  }


  // —— 顶栏标题（仅对话标题；审批/备份状态在输入区与改动条上，不再重复展示）——
  // currentTitle：直接跟随当前会话（新会话首条消息即时上屏，不等标题生成）
  let currentTitle = '';
  // 注意：必须用 $: 响应式声明——模板里直接调 currentSessionTitle() 不会因 currentTitle 变化而重渲染（Svelte 4 不跨函数追踪依赖）
  $: displayTitle =
    currentTitle ||
    (sessions.find((x) => x.id === currentSessionId)?.title ?? '') ||
    '新的对话';

  // —— 多会话：列表 / 切换 / 新建（新建不清空历史）——
  async function loadSessions(autoPick = false, restoreSaved = false) {
    try {
      const r = await fetch('/api/sessions').then((r) => r.json());
      projectName = r.project || '';
      sessions = r.sessions || [];
      workspaceBorn = r.workspace_born || {};
      // 启动/切项目时恢复最近一条会话：只认当前工作区里的会话。
      // 不筛会打开别的项目的会话，而 /api/sessions/open 会把工作区切回那个项目（侧栏项目来回弹）。
      // 无 workspace 标记的遗留会话不许自动弹：新建工作区会翻出最老的一批测试会话
      // （2026-09-29 真机实测「只回复两个字：收到」轮）；侧栏「未分组」里仍可手动打开。
      if (autoPick && currentSessionId === null) {
        // 刷新/重新加载界面（整页导航）先回到本标签页上次看的那条，而不是每次都被
        // 「最近一条非空会话」顶走。只有应用启动这条路传 restoreSaved=true；切工作区
        // 仍走原来的自动选最近一条（那是「换项目」的语义，不是刷新）。
        let restored = false;
        if (restoreSaved) {
          let saved = '';
          try {
            saved = sessionStorage.getItem('curSession') || '';
          } catch {}
          if (saved === '@new') {
            restored = true; // 上次停在「新对话」：保持空白新对话，不跳历史
          } else if (saved) {
            const s = sessions.find((x) => x.id === saved);
            if (
              s &&
              !s.archived &&
              s.workspace &&
              normPath(s.workspace).toLowerCase() === normPath(workspace).toLowerCase() &&
              !isLegacySession(s)
            ) {
              await loadSession(saved);
              restored = true;
            }
          }
        }
        if (!restored) {
          const mine = sessions.filter(
            (s) =>
              s.workspace &&
              normPath(s.workspace).toLowerCase() === normPath(workspace).toLowerCase() &&
              !isLegacySession(s),
          );
          // 重启后默认打开最近一条**有内容**的会话：侧栏最上面可能是一条空的新对话
          // （用户刚点过「新对话」），直接打开它看起来就像「历史/思考/工具调用被清掉了」。
          // 全空时才退回首条（保持原有的空项目行为）。
          const pick = mine.find((s) => (s.count || 0) > 0) || mine[0];
          if (pick) await loadSession(pick.id);
        }
      }
    } catch (e) {
      sessions = [];
      // 启动路径失败必须可见：否则侧栏空着、看起来像历史全丢（2026-10-27 实锤）
      if (autoPick) startupError = `[启动失败] 读取会话列表失败：${e.message || e}`;
    }
  }

  /// 会话清单刷新时顺手处理「内容被后台改过」的会话：
  /// - 其它会话：丢掉本地消息缓存，下次打开从文件重建
  /// - 当前正看着的会话：本页没在跑就直接从文件重载视图（定时/CLI 唤醒跑完要当场看见）
  /// 只认 count 变化——改标题也会动 updated_at，但消息没变，不该白重拉。
  async function refreshSessionsAndDropStale() {
    const before = new Map(sessions.map((s) => [s.id, s.count === undefined ? -1 : s.count]));
    await loadSessions();
    if (!before.size) return; // 首次加载没有旧值可比
    const changed = [];
    for (const s of sessions) {
      const prev = before.get(s.id);
      if (prev === undefined) continue; // 新会话：本来就没有缓存
      if ((s.count === undefined ? -1 : s.count) !== prev) changed.push(s.id);
    }
    if (!changed.length) return;
    const next = { ...sessionMsgs };
    for (const id of changed) if (!streams[id]) delete next[id];
    sessionMsgs = next;
    const cur = currentSessionId;
    if (cur && changed.includes(cur) && !streams[cur]) {
      try {
        const r = await fetch(`/api/session?id=${encodeURIComponent(cur)}`).then((x) => x.json());
        if (Array.isArray(r.messages) && r.messages.length) {
          sessionMsgs = { ...sessionMsgs, [cur]: rebuildFromSession(r.messages) };
          syncCur();
        }
      } catch {
        /* 读不到就保持屏幕上的旧内容，下一轮通知再试 */
      }
    }
  }

  // 侧栏「运行中」轮询：服务端在跑的会话 id（含别的窗口 / 页面重载后本页没流的那些）。
  // 3 秒一次、只读注册表 keys，开销可忽略；页面隐藏时不拉。
  async function refreshRunning() {
    try {
      const r = await fetch('/api/running').then((x) => x.json());
      const next = {};
      for (const id of r.running || []) if (id) next[id] = true;
      serverRunning = next;
    } catch {
      /* 服务没起/断网：保持上次状态，下一跳再试 */
    }
  }

  // 定时任务「完成记录 / 上次完成」里的「去查看」：先关掉设置弹窗，再打开对应会话
  // （/api/sessions/open 会在会话属于别的项目时先切过去）。id 空则不动。
  async function openScheduleSession(id) {
    if (!id) return;
    settingsOpen = false;
    await loadSession(id);
  }

  // 服务端用量快照（/api/sessions/open 的 r.usage）→ 圆环/缓存行同形。
  // 旧会话文件里是 tokens/budget（改造前口径）——后端已用 serde alias 收下并改写成
  // projected/window，这里对老服务端载荷也做一次兜底，免得切回旧会话圆环空掉
  function usageFromServer(su) {
    return {
      ctx: {
        projected: su.projected ?? su.tokens ?? 0,
        pressure: su.pressure ?? null,
        window: su.window ?? su.budget ?? 0,
        breakdown: {
          messages: su.messages || 0,
          tools: su.tools || 0,
          system: su.system || 0,
          other: su.other || 0,
          tools_schema: su.tools_schema || 0,
        },
      },
      api: { prompt: su.api_prompt || 0, cached: su.api_cached || 0 },
    };
  }

  // 切到某会话：从按会话账本恢复用量；没账本的新会话清零（窗口保留——圆环常驻）
  function restoreUsage(key) {
    // 活数据（本页收到的 SSE 事件）最新；没有才落回服务端快照（重启/重开后的恢复路径），
    // 并扶正为会话账本，后续切换不再反复读盘
    const u = usageBySession[key] || serverUsage[key];
    if (u && !usageBySession[key]) usageBySession = { ...usageBySession, [key]: u };
    ctxUsage = u ? u.ctx : { projected: 0, pressure: null, window: ctxUsage.window, breakdown: null };
    apiTotals = u ? u.api : { prompt: 0, cached: 0 };
  }

  // 打开会话：后端会在其所属项目与当前项目不同时先切过去（切会话 = 回到那个项目），
  // 所以走 /api/sessions/open 而不是只读的 /api/session。
  async function loadSession(id) {
    // 027：切会话不再被别的会话的运行挡住（不同对话互不影响）；
    // 该会话自己有在跑的流 → 直接用本地缓冲（服务端文件此刻还没有最新内容）
    if (streams[id]) {
      setCurSession(id);
      currentTitle = sessions.find((x) => x.id === id)?.title ?? currentTitle;
      syncCur();
      restoreUsage(id); // 从按会话账本恢复（跑完的会话不重发事件，不恢复就显示 0）
      dockCp = null;
      refreshDock();
      return;
    }
    try {
      const r = await fetch('/api/sessions/open', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ id }),
      }).then((r) => r.json());
      if (r.error) {
        // 侧栏点会话打不开：settingsMsg 只在设置弹窗里渲染，这里必须可见反馈
        await confirmAsk(`打开会话失败：${r.error}`, true);
        return;
      }
      setCurSession(id);
      currentTitle = r.title || (sessions.find((x) => x.id === id)?.title ?? '');
      // 服务端用量快照（会话文件里随事件落盘的那份）：重启/重开程序后圆环和缓存行
      // 从这里恢复，不用等下一轮请求再攒。活数据优先，这里只兜底
      if (r.usage) serverUsage = { ...serverUsage, [id]: usageFromServer(r.usage) };
      // 本地已有缓冲就用本地（含流式思考等尚未落盘的新内容），没有才从服务端重建
      if (!sessionMsgs[id]) {
        sessionMsgs = { ...sessionMsgs, [id]: rebuildFromSession(r.messages || []) };
      }
      // 对账（2026-09-27 真机实测「切界面/开关程序丢对话内容」）：本页没有这条会话的流，
      // 但服务端可能还有一轮在跑——切工作区回来/重开程序都会走到这里，拿到的是**旧文件**
      // 快照，运行中那轮的产出从此没人补。挂上对账轮询：服务端空闲后从会话文件重载，
      // 缺的尾巴自动回来。（本页有自己的流在跑时不进这个分支，用不到对账。）
      fetch(`/api/cancel?session=${encodeURIComponent(id)}`)
        .then((r) => r.json())
        .then((c) => {
          const stashed = readLiveStash(id);
          if (c.running && !streams[id]) {
            // 刷新恢复：服务端文件此刻通常只有用户消息（助手内容还在流里），先把刷新前的
            // 内存快照铺回去；跑完由 reconcileServerQueued 用落盘结果替换。
            if (stashed && Array.isArray(stashed.msgs) && stashed.msgs.length) {
              sessionMsgs = {
                ...sessionMsgs,
                [id]: stashed.msgs.map((m) => (m.streaming ? { ...m, streaming: false } : m)),
              };
              if (currentSessionId === id) syncCur();
            }
            reconcileServerQueued(id);
          }
          // 刷新前排队、还没发出去的消息：会话文件里没有它们，铺回气泡并补发。
          // 服务端还在跑就先等对账（reconcile 收尾时会接上出队）。
          if (stashed && Array.isArray(stashed.outbox) && stashed.outbox.length) {
            restoreQueuedOutbox(id, stashed.outbox);
            if (!c.running) {
              clearLiveStash();
              drainOutbox(id);
            }
          }
        })
        .catch(() => {});
      syncCur();
      restoreUsage(id); // 从按会话账本恢复（跑完的会话不重发事件，不恢复就显示 0）
      dockCp = null;
      if (r.warning) await confirmAsk(r.warning, true);
      if (r.switched) {
        // 项目换了：配置（工作区路径/AGENTS.md）与会话分组都要跟着刷新
        await loadSettings();
        await loadSessions();
      }
      refreshDock();
      // git/产物面板开着时跟着换会话/换工作区重拉（否则还显示上一个范围的清单）
      resetScopedPanels();
    } catch (e) {
      // 加载失败不阻塞
    }
  }

  // 新建会话：立即在后端落一条空会话（侧栏当前项目下马上可见），首条消息写进同一条。
  // 同一项目已有一条空会话时后端会复用，连点不会堆出一排。
  async function startNewSession() {
    if (streams['@new']) return; // 还没落 id 的新会话正在跑：等它拿到 id 再说
    setCurSession(null);
    currentTitle = '';
    dockCp = null;
    sessionMsgs = { ...sessionMsgs, '@new': [] };
    syncCur();
    restoreUsage('@new'); // 新会话：账本里没有它 → 清零
    try {
      const r = await fetch('/api/sessions/new', { method: 'POST' }).then((r) => r.json());
      if (r.ok && r.id) {
        rekey('@new', r.id);
        // 新会话也是一次范围切换：产物/git 面板必须作废重拉，否则停留显示上一个
        // 会话的清单（必须放在 rekey 之后——新会话还没有检查点，重拉走「空清单+人话」）
        resetScopedPanels();
        currentTitle = r.title || '';
        if (workspace) {
          // 新会话就在当前项目下：展开该项目，马上能看到这条空会话（手风琴：
          // 只留当前项目展开，其余工作区的列表收起）
          expandedFlags = { [workspace]: true };
          localStorage.setItem('projExpanded', JSON.stringify(expandedFlags));
        }
        await loadSessions();
      } else if (r.error) {
        await confirmAsk(`新建会话失败：${r.error}`, true);
      }
    } catch (e) {
      await confirmAsk(`新建会话失败：${e.message || e}`, true);
    }
  }

  async function deleteSession(id) {
    if (!(await confirmAsk('删除这条会话？其余会话不受影响'))) return;
    await fetch(`/api/session?id=${encodeURIComponent(id)}`, { method: 'DELETE' });
    if (id === currentSessionId) {
      await startNewSession();
      return; // startNewSession 里已刷新列表
    }
    loadSessions();
  }

  // —— DSH 式侧边栏（项目=工作区；置顶/归档/分叉/搜索/批量管理/视图切换）——
  let workspaceAliases = {}; // 工作区路径 → 项目显示名（重命名项目）
  let sidebarView = localStorage.getItem('sidebarView') === 'list' ? 'list' : 'workspace'; // 按工作区 | 单列表
  let expandedFlags = { __ungrouped__: true, ...(JSON.parse(localStorage.getItem('projExpanded') || '{}') || {}) }; // path → 展开？
  let showAllFlags = {}; // path → 展开全部会话（默认每项目先显 5 条）
  let searchOpen = false;
  let searchQuery = '';
  let manageMode = false;
  let checkedIds = [];
  let sidebarCollapsed = false;
  let sidebarW = Number(localStorage.getItem('sidebarW')) || 300; // 侧栏宽度（右缘拖拽可调，240–520）
  let resizing = false;

  // 侧栏右缘拖拽调宽：按住手柄移动鼠标，松手持久化（240–720）。
  // 指针事件 + setPointerCapture：捕获后所有 pointermove/pointerup/pointercancel
  // 都路由到手柄元素——光标挪到 iframe 上、窗口外、别的元素上都不会丢事件。
  // 早两版用 window 级 mousemove/mouseup 监听，真机（Tauri WebView2）实锤两类
  // 症状：拖到某处像被卡住要来回蹭、松手后侧栏还跟着没按住的鼠标跑（mouseup 丢了）。
  // 宽度在手柄里**直接**应用：Chromium 对 mousemove/pointermove 按帧合并（规范动作），
  // 125Hz+ 鼠标也是一帧一次写入；不做 rAF 中转——rAF 在壳里可能被节流，中转一下
  // 反而表现成「侧栏冻住不跟手」（同一批真机反馈）。
  function startResize(e) {
    if (e.button !== 0) return; // 只认左键（触摸/笔的 button 也是 0，一并支持）
    e.preventDefault();
    resizing = true;
    const handle = e.currentTarget;
    try {
      handle.setPointerCapture(e.pointerId);
    } catch (err) {
      /* 捕获失败（极端环境）下退回 DOM 冒泡也能拖，不阻断 */
    }
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
    const move = (ev) => {
      sidebarW = Math.min(720, Math.max(240, Math.round(ev.clientX)));
    };
    const up = () => {
      resizing = false;
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      localStorage.setItem('sidebarW', String(sidebarW));
      handle.removeEventListener('pointermove', move);
      handle.removeEventListener('pointerup', up);
      handle.removeEventListener('pointercancel', up);
    };
    handle.addEventListener('pointermove', move);
    handle.addEventListener('pointerup', up);
    handle.addEventListener('pointercancel', up);
  }
  // 终端面板高度（上缘拖拽可调，120–560，持久化）
  let termH = Number(localStorage.getItem('termH')) || 260;
  let termResizing = false;
  function startTermResize(e) {
    if (e.button !== 0) return;
    e.preventDefault();
    termResizing = true;
    const handle = e.currentTarget;
    try {
      handle.setPointerCapture(e.pointerId);
    } catch (err) {
      /* 捕获失败退回冒泡，不阻断 */
    }
    document.body.style.cursor = 'row-resize';
    document.body.style.userSelect = 'none';
    const move = (ev) => {
      termH = Math.min(560, Math.max(120, Math.round(window.innerHeight - ev.clientY)));
    };
    const up = () => {
      termResizing = false;
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      localStorage.setItem('termH', String(termH));
      handle.removeEventListener('pointermove', move);
      handle.removeEventListener('pointerup', up);
      handle.removeEventListener('pointercancel', up);
    };
    handle.addEventListener('pointermove', move);
    handle.addEventListener('pointerup', up);
    handle.addEventListener('pointercancel', up);
  }
  let sessMenuId = null; // 打开了操作菜单的会话 id
  let projMenuPath = null; // 打开了操作菜单的项目 path
  let sideMenuOpen = false; // 工具条合并菜单（两个视图 + 会话管理）
  let renameModal = null; // { kind: 'session'|'project', target, value }
  // 通用确认弹窗：桌面壳（Tauri WebView2）不实现 window.confirm/alert（静默无操作），
  // 全部确认/提示走应用内弹窗。{ text, okOnly, resolve }。
  let confirmModal = null;
  function confirmAsk(text, okOnly = false) {
    return new Promise((resolve) => {
      confirmModal = { text, okOnly, resolve };
    });
  }
  const CONV_PREVIEW = 5; // 项目收起态先显 5 条，其余走「展开其余 N 个会话」

  // 当前工作区**变化**时独占展开它的分组（切项目/外部改工作区）：手风琴语义下
  // 一次只展开一个组，新工作区上线、其余工作区的对话列表一并收起。
  // 只在 workspace 真的变了时展开——早先的版本是「当前工作区不在展开集里就补上」，
  // 用户手风琴点开别的组时会把它立刻加回来，两个组同时展开（实锤过）。
  let lastExpandedWs = workspace;
  $: if (workspace && workspace !== lastExpandedWs) {
    lastExpandedWs = workspace;
    expandedFlags = { [workspace]: true };
  }


  // 项目列表 = 当前工作区 + 最近工作区（去重，按新→旧）
  $: projectList = (() => {
    const list = [];
    const seen = new Set();
    if (workspace) {
      list.push({ path: workspace, active: true });
      seen.add(normPath(workspace).toLowerCase());
    }
    for (const w of recentWorkspaces) {
      if (!seen.has(normPath(w).toLowerCase())) {
        list.push({ path: w, active: false });
        seen.add(normPath(w).toLowerCase());
      }
    }
    return list.map((p) => ({
      ...p,
      name: workspaceAliases[p.path] || p.path.split(/[\\/]/).filter(Boolean).pop() || p.path,
    }));
  })();

  $: searchQ = searchQuery.trim().toLowerCase();
  $: visibleSessions = sessions;

  /** 会话最后一次活动早于工作区目录创建时间 = 这条会话来自以前的同名文件夹。
   *  归属按路径字符串做，文件夹删掉再建（Windows 最爱默认名「新建文件夹」）时
   *  旧会话会被一起翻出来（2026-11-04 实报）；标成历史会话，默认收起，不删数据。
   *  用 updated_at 而不是 created_at：目录可能被移动/复制过，创建时间会晚于
   *  会话的创建时间，但不会晚于会话的最后活动时间——用 created 判会把正常
   *  项目里的会话误判成历史（2026-11-04 误删事故的根因）。 */
  function isLegacySession(s) {
    const born = workspaceBorn[s.workspace];
    if (!born) return false;
    const created = Date.parse(s.created_at);
    const updated = Date.parse(s.updated_at) || created;
    return Number.isFinite(updated) && updated < born;
  }

  // 分组：会话按创建时工作区归项目；无工作区记录的旧会话进「未分组」
  $: sessionsByProject = (() => {
    const map = {};
    for (const s of visibleSessions) {
      if (searchQ && !(s.title || '').toLowerCase().includes(searchQ)) continue;
      if (isLegacySession(s)) continue; // 同名文件夹的历史会话单独一组
      const hit = projectList.find((p) => normPath(p.path).toLowerCase() === normPath(s.workspace).toLowerCase());
      const k = hit ? hit.path : '__ungrouped__';
      (map[k] = map[k] || []).push(s);
    }
    return map;
  })();

  $: ungroupedSessions = sessionsByProject['__ungrouped__'] || [];
  $: legacySessions = visibleSessions.filter(
    (s) => isLegacySession(s) && (!searchQ || (s.title || '').toLowerCase().includes(searchQ)),
  );

  // 单列表视图：全部会话平铺（后端已按置顶→更新时间排序）
  $: flatSessions = visibleSessions.filter((s) => !searchQ || (s.title || '').toLowerCase().includes(searchQ));

  // 渲染行序列：单列表=纯会话流；工作区视图=项目行+缩进会话行+「展开其余」行
  $: displayRows = (() => {
    const rows = [];
    if (sidebarView === 'list') {
      for (const s of flatSessions) rows.push({ type: 'conv', s });
      if (!rows.length) rows.push({ type: 'empty' });
      return rows;
    }
    const pushGroup = (path, name, active, items) => {
      // 项目行也带一个转圈：正在跑的会话可能属于别的项目（侧栏只展开当前项目），
      // 不标在项目上就完全看不见它还在跑（2026-11-05 实锤「切走就以为结束了」）
      rows.push({ type: 'proj', path, name, active, count: items.length, running: items.some((s) => runningIds.has(s.id)) });
      const expanded = !!searchQ || !!expandedFlags[path];
      if (!expanded) return;
      const capped = searchQ
        ? items
        : items.slice(0, showAllFlags[path] ? items.length : CONV_PREVIEW);
      for (const s of capped) rows.push({ type: 'conv', s });
      if (!searchQ && items.length > CONV_PREVIEW && !showAllFlags[path]) {
        rows.push({ type: 'more', path, rest: items.length - CONV_PREVIEW });
      }
    };
    for (const p of projectList) pushGroup(p.path, p.name, p.active, sessionsByProject[p.path] || []);
    if (ungroupedSessions.length) pushGroup('__ungrouped__', '未分组', false, ungroupedSessions);
    // 同名文件夹的历史会话：单独一组、默认收起——空文件夹不该一打开就冒出一排旧对话
    if (legacySessions.length) pushGroup('__legacy__', '历史会话（同名文件夹）', false, legacySessions);
    if (!rows.length) rows.push({ type: 'empty' });
    return rows;
  })();

  function toggleProjectRow(path) {
    if (searchQ) return; // 搜索态全展开
    // 手风琴（选了一个工作区，其他工作区取消选中状态并自动收起
    // 对话）：展开一个就收起其余——侧栏同时只看到一个项目的会话列表。
    // showAllFlags 不动：重新展开时恢复「显示全部」，收起不等于忘掉看过。
    const willExpand = !expandedFlags[path];
    expandedFlags = willExpand ? { [path]: true } : { ...expandedFlags, [path]: false };
    localStorage.setItem('projExpanded', JSON.stringify(expandedFlags));
  }

  async function switchProject(path) {
    if (normPath(path).toLowerCase() === normPath(workspace).toLowerCase()) return;
    if (Object.keys(streams).length > 0) {
      // 切项目会重置所有会话缓冲。同上：拒绝可以，但要让人看见原因
      await confirmAsk('有会话正在运行，等它跑完再切换项目。', true);
      return;
    }
    await applyWorkspace(path);
  }

  async function newSessionInProject(path, active) {
    if (!active) {
      await switchProject(path);
      // 没真正切过去（流式输出中/目录不存在）就别新建，否则会话会落到别的项目里
      if (normPath(workspace).toLowerCase() !== normPath(path).toLowerCase()) return;
    }
    await startNewSession();
  }


  // —— 工具栏四键：搜索 / 管理 / 视图 / 添加项目 ——
  function toggleSearch() {
    searchOpen = !searchOpen;
    if (!searchOpen) searchQuery = '';
  }

  function enterManage() {
    sideMenuOpen = false;
    manageMode = true;
    checkedIds = [];
  }
  function exitManage() {
    manageMode = false;
    checkedIds = [];
  }

  function toggleChecked(id) {
    checkedIds = checkedIds.includes(id) ? checkedIds.filter((x) => x !== id) : [...checkedIds, id];
  }

  function manageVisibleIds() {
    return sidebarView === 'list' ? flatSessions.map((s) => s.id) : displayRows.filter((r) => r.type === 'conv').map((r) => r.s.id);
  }
  // Svelte4 坑（同 commitSelN）：模板里调 manageVisibleIds().length 不建立函数体内
  // 变量的依赖——后台任务跑完/搜索过滤/批量操作后 loadSessions 重拉，分母冻在旧值，
  // 「已选 4 / 12」而列表只有 4 条。计数必须 $: 派生（2026-10-08 修）。
  $: manageTotal =
    sidebarView === 'list' ? flatSessions.length : displayRows.filter((r) => r.type === 'conv').length;

  async function batchDelete() {
    if (!checkedIds.length || !(await confirmAsk(`删除选中的 ${checkedIds.length} 条会话？不可恢复`))) return;
    for (const id of checkedIds) {
      await fetch(`/api/session?id=${encodeURIComponent(id)}`, { method: 'DELETE' });
    }
    const hadCurrent = checkedIds.includes(currentSessionId);
    checkedIds = [];
    await loadSessions();
    if (hadCurrent) await settleCurrentSessionAfterListChange();
  }

  function pickView(v) {
    sidebarView = v;
    sideMenuOpen = false;
    localStorage.setItem('sidebarView', v);
  }

  // 添加项目：server 弹 Win 原生选文件夹窗口 → 选中的目录注册为项目（=切换工作区）
  async function addProject() {
    try {
      const r = await fetch('/api/workspace/picker', { method: 'POST' }).then((r) => r.json());
      if (r.canceled || !r.path) return;
      await applyWorkspace(r.path);
    } catch (e) {
      // settingsMsg 只在设置弹窗里渲染——侧栏操作时不可见，改应用内提示弹窗
      await confirmAsk(`选择文件夹失败：${e.message || e}`, true);
    }
  }

  // —— 多文件夹项目（2026-11-04，对齐 ZCode）——
  function folderNameOf(p) {
    return String(p || '').split(/[\\/]/).filter(Boolean).pop() || p || '';
  }
  async function addWorkspaceFolder() {
    if (foldersBusy) return;
    foldersBusy = true;
    foldersMsg = '';
    try {
      const r = await fetch('/api/workspace/picker', { method: 'POST' }).then((x) => x.json());
      if (r && r.canceled) return;
      const path = r && r.path;
      if (!path) {
        foldersMsg = (r && r.error) || '没有选择文件夹';
        return;
      }
      const r2 = await fetch('/api/workspace/folders', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ op: 'add', path }),
      }).then((x) => x.json());
      if (r2.error) {
        foldersMsg = r2.error;
        return;
      }
      await loadSettings();
      foldersMsg = '已添加';
      // 文件面板/插件树即时跟上（复用工作区事件，board 收到即重拉树）
      notifyExtsWorkspace(workspace);
    } catch (e) {
      foldersMsg = `添加失败: ${e.message || e}`;
    } finally {
      foldersBusy = false;
    }
  }
  async function removeWorkspaceFolder(path) {
    if (!(await confirmAsk(`从项目移除文件夹「${folderNameOf(path)}」？\n只移出项目，不删除磁盘上的文件。`))) return;
    foldersMsg = '';
    try {
      const r = await fetch('/api/workspace/folders', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ op: 'remove', path }),
      }).then((x) => x.json());
      if (r.error) {
        foldersMsg = r.error;
        return;
      }
      await loadSettings();
      notifyExtsWorkspace(workspace);
    } catch (e) {
      foldersMsg = `移除失败: ${e.message || e}`;
    }
  }

  async function removeProject(path, name) {
    if (normPath(path).toLowerCase() === normPath(workspace).toLowerCase()) {
      // 列表最上面那条就是当前项目（projectList 把它固定放首位）。拦下来可以，但理由
      // 必须可见：settingsMsg 只在设置弹窗里渲染，侧栏操作时写了等于没写（真机实测
      // 「最上面的删除项目无效」）——应用内提示弹窗说清楚。
      await confirmAsk(`「${name}」是当前正在使用的项目，不能移除。\n先切换到其他项目，再来移除它。`, true);
      return;
    }
    if (!(await confirmAsk(`移除项目「${name}」？\n这个项目下的会话会一起删除（不可恢复）；磁盘上的文件夹保留。`))) return;
    projMenuPath = null;
    await fetch('/api/workspace/remove', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path }),
    });
    await loadSettings();
    await loadSessions(); // 会话是级联删的：列表要立刻刷新，别把已删的还挂在那
  }

  // —— 会话菜单四项：置顶 / 重命名 / 分叉 / 归档 ——
  async function togglePin(s) {
    sessMenuId = null;
    await fetch('/api/sessions/meta', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ id: s.id, pinned: !s.pinned }),
    });
    await loadSessions();
  }

  // 归档/删除当前会话后的落点：只切到**同工作区**最近一条未归档会话；没有就
  // 在当前工作区新开一条。2026-10-27 真机实测：归档当前会话时 next 用了全局
  // sessions.find，切到了另一个（刚被移除的）工作区的会话——loadSession 会经
  // /api/sessions/open 把工作区切回去，看起来像已删工作区被复原。绝不跨工作区。
  async function settleCurrentSessionAfterListChange() {
    // 当前会话还在列表里就不动；没了才切到同工作区最近一条，没有再开新会话。
    // 2026-10-27 实锤过跨工作区乱切：绝不拿全局列表找「下一条」。
    if (!currentSessionId) return;
    const cur = sessions.find((x) => x.id === currentSessionId);
    if (cur) return;
    if (!workspace) {
      await startNewSession();
      return;
    }
    const ws = normPath(workspace).toLowerCase();
    const next = sessions.find(
      (x) => x.id !== currentSessionId && x.workspace && normPath(x.workspace).toLowerCase() === ws,
    );
    if (next) await loadSession(next.id);
    else await startNewSession();
  }

  async function deleteSessionRow(s) {
    sessMenuId = null;
    if (!(await confirmAsk('删除会话「' + (s.title || '无标题') + '」？不可恢复'))) return;
    await fetch('/api/session?id=' + encodeURIComponent(s.id), { method: 'DELETE' });
    await loadSessions();
    // 删的正是当前会话：切到同项目最近一条，没有则回新会话空态。
    // 不切的话服务端 current 仍指向已删会话——输入还在往里写。
    if (s.id === currentSessionId) await settleCurrentSessionAfterListChange();
  }

  /** 「历史会话（同名文件夹）」组：这些是以前同名文件夹留下的旧会话。
   *  删除要做成真删会话文件——它不是项目，走「删除项目」只会删一个不存在的路径，
   *  就是用户实报「一直删不掉」的根因。 */
  async function clearLegacySessions() {
    const ids = legacySessions.map((s) => s.id);
    if (!ids.length) return;
    const sample = legacySessions.slice(0, 3).map((s) => '「' + (s.title || '无标题') + '」').join('、');
    if (!(await confirmAsk(`清空 ${ids.length} 条历史会话？\n\n${sample}${ids.length > 3 ? ' 等' : ''}\n\n这些是以前同名文件夹留下的旧对话，删除后不可恢复（其余会话不受影响）。`))) return;
    for (const id of ids) {
      await fetch('/api/session?id=' + encodeURIComponent(id), { method: 'DELETE' });
    }
    await loadSessions();
    if (ids.includes(currentSessionId)) await settleCurrentSessionAfterListChange();
  }

  async function forkSession(id) {
    sessMenuId = null;
    try {
      const r = await fetch('/api/sessions/fork', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ id }),
      }).then((r) => r.json());
      await loadSessions();
      if (r.id) await loadSession(r.id);
    } catch (e) {
      await confirmAsk(`分叉失败：${e.message || e}`, true);
    }
  }

  function submitRename() {
    if (!renameModal) return;
    const v = renameModal.value.trim();
    const modal = renameModal;
    renameModal = null;
    if (modal.kind === 'session') {
      // orig = 打开弹窗时的原标题。输入框 bind 直接改 modal.value，拿它比较
      // 恒为假——请求永远发不出（F1），必须拿原值比
      if (v && v !== modal.orig) {
        fetch('/api/sessions/meta', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ id: modal.target, title: v }),
        }).then(() => {
          // F5：改的就是当前会话时同步顶栏（否则侧栏变了顶栏仍旧名）
          if (modal.target === currentSessionId) currentTitle = v.slice(0, 60); // 与后端 60 字截断对齐
          loadSessions();
        });
      }
    } else {
      fetch('/api/workspace/alias', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ path: modal.target, name: v }),
      }).then(() => loadSettings());
    }
  }

  // —— 改动统计条：撤销 = 工作树回滚到任务起点基线 ——
  async function undoCheckpoint(cp) {
    if (!(await confirmAsk(`撤销本次任务的改动？\n${cp.files} 个文件将恢复到任务开始前（含删除本次新建的已跟踪文件）。\nshell 副作用不可回滚。`)))
      return;
    try {
      const r = await fetch('/api/checkpoints/restore', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ id: cp.id }),
      });
      const d = await r.json();
      pushMsg(curKey(), {
        id: nextId(),
        role: 'system',
        content: d.ok ? `已撤销：${cp.files} 个文件恢复到任务开始前` : `撤销失败：${d.error}`,
      });
      refreshDock(); // 撤销后改动数变化，刷新钉条
    } catch (e) {
      pushMsg(curKey(), { id: nextId(), role: 'error', content: `[撤销请求失败] ${e}` });
    }
  }

  function rebuildFromSession(msgs) {
    const out = [];
    for (const m of msgs) {
      if (m.role === 'user') {
        // steered=引导消息（2026-10-22）：渲染成「已引导」轻量行，不占气泡位
        out.push({ id: nextId(), role: 'user', content: contentText(m.content), steered: !!m.steered, interrupted: !!m.interrupted, ts: m.ts, cp: m.cp });
      } else if (m.role === 'assistant') {
        const asst = {
          id: nextId(),
          role: 'assistant',
          content: contentText(m.content),
          // 027：思考内容已随消息落盘（reasoning_content），切回会话时恢复
          thinking: m.reasoning_content || '',
          tools: (m.tool_calls || []).map((tc) => ({
            id: tc.id,
            name: tc.function?.name || '?',
            args: tc.function?.arguments || '',
            status: 'ok',
            preview: '',
          })),
          // 2026-10-30：按真实发生顺序的分段（思考 → 正文 → 工具）；多步再用
          // mergeAssistantRun 合并。落盘结构不动，parts 只活在展示层。
          parts: partsFromStoredMessage({
            reasoning_content: m.reasoning_content,
            reasoning_ms: m.reasoning_ms,
            content: contentText(m.content),
            tool_calls: m.tool_calls,
          }),
          streaming: false,
          // 时长随消息落盘（服务端写）：整轮「工作用时」只在收尾消息上，中间步骤有「用时」
          workMs: m.work_ms || 0,
          stepMs: m.step_ms || 0,
          thinkMs: m.reasoning_ms || 0,
          // 信息卡数据（渠道/模型/首包/耗时/tokens）与发送时间随消息落盘
          stats: m.stats || null,
          ts: m.ts,
        };
        out.push(asst);
      } else if (m.role === 'system') {
        // 通知/错误随会话落盘（display_only），重启/切回会话后气泡还在
        out.push({ id: nextId(), role: 'system', content: contentText(m.content), ts: m.ts });
      } else if (m.role === 'tool') {
        const asst = [...out].reverse().find(
          (x) => x.role === 'assistant' && (x.tools || []).some((t) => t.id === m.tool_call_id)
        );
        const t = asst && asst.tools.find((t) => t.id === m.tool_call_id);
        if (t) {
          t.preview = contentText(m.content).slice(0, 300);
          // 工具结果也回填到顺序分段的对应工具段（展示详情用）
          asst.parts = attachToolPreview(asst.parts, m.tool_call_id, t.preview);
        }
      }
    }
    return mergeAssistantRun(out);
  }

  // 同一轮连续 assistant 合并 / 分段规则见 lib/chat.js 的 mergeAssistantRun：
  // 流式路径一轮一个气泡，落盘一步一条 assistant；展示层合并，落盘结构不动
  // （2026-10-30 实锤过「重启后工具调用被拆成 N 个气泡」）。规则有单测兜底。

  // 审批决定：本地乐观标记 + 通知后端（后端也会回 approval_decided，幂等）。
  // remember=true → 本会话内「同类操作」不再询问（A3）；2026-11-03 起高危档也允许记忆。
  async function decideApproval(id, approved, remember = false) {
    patchMessageApproval(id, approved);
    try {
      const r = await fetch('/api/approvals', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ id, approved, remember }),
      });
      if (!r.ok) throw new Error(`HTTP ${r.status}`);
    } catch (e) {
      // F13：失败回滚本地标记（后端没收到决定，UI 不能显示已决定）+ 人话提示
      patchMessageApproval(id, null);
      pushMsg(curKey(), { id: nextId(), role: 'error', content: `[审批决定提交失败，已恢复待决定] ${e}` });
    }
  }
  function patchMessageApproval(id, approved) {
    updMsgs(curKey(), (a) =>
      a.map((m) => {
        if (!(m.approval && m.approval.id === id)) return m;
        return {
          ...m,
          approval: { ...m.approval, decided: approved },
          parts: settleApprovalOnParts(m.parts, id, approved),
        };
      })
    );
  }

  // 待审批的执行询问：浮在输入框上方，不占对话流（用户 2026-11-03）；
  // 决定后记录由 attachApprovalToToolPart 挂回时间线的工具卡
  $: pendingApprovals = (messages || [])
    .filter((m) => m.approval && m.approval.decided === null)
    .map((m) => m.approval);

  // —— 阻塞式提问（ask 工具，ZCode AskUserQuestion 同款）——
  // 与审批共用输入框上方的 dock 槽位、同一套「挂消息上 + 派生过滤」的数据流；
  // 区别：审批是权限闸（允许/拒绝），提问是方案决策（选项/Other/跳过），后端 /api/ask。
  $: pendingQuestions = (messages || [])
    .filter((m) => m.question && m.question.answered === null)
    .map((m) => m.question);

  // 提问卡的本地勾选/自由输入状态（按提问 id 存，直接改消息对象不触发 Svelte 更新）
  let askUi = {};
  function askUiOf(q) {
    return askUi[q.id] || { picked: [], other: '' };
  }
  function setAskUi(id, patch) {
    askUi = { ...askUi, [id]: { ...(askUi[id] || { picked: [], other: '' }), ...patch } };
  }
  function pickAskOption(q, label) {
    const ui = askUiOf(q);
    if (q.multiSelect) {
      const picked = ui.picked.includes(label) ? ui.picked.filter((x) => x !== label) : [...ui.picked, label];
      setAskUi(q.id, { picked });
    } else {
      answerAsk(q, { selected: [label] });
    }
  }
  async function answerAsk(q, payload) {
    // 乐观收卡；后端确认后再把回答记录挂回时间线的 ask 工具卡（失败回滚 + 人话提示）
    updMsgs(curKey(), (a) =>
      a.map((m) => (m.question && m.question.id === q.id ? { ...m, question: { ...m.question, answered: payload } } : m))
    );
    try {
      const r = await fetch('/api/ask', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ id: q.id, ...payload }),
      });
      if (!r.ok) throw new Error(`HTTP ${r.status}`);
      updMsgs(curKey(), (a) => a.map((m) => (m.question && m.question.id === q.id ? { ...m, parts: attachAskToToolPart(m.parts, { id: q.id, ...payload }) } : m)));
    } catch (e) {
      updMsgs(curKey(), (a) =>
        a.map((m) => (m.question && m.question.id === q.id ? { ...m, question: { ...m.question, answered: null } } : m))
      );
      pushMsg(curKey(), { id: nextId(), role: 'error', content: `[回答提交失败，已恢复待回答] ${e}` });
    }
  }
  function submitAskOther(q) {
    const ui = askUiOf(q);
    const other = (ui.other || '').trim();
    if (!other) return;
    answerAsk(q, { selected: q.multiSelect ? ui.picked : [], other });
  }

  // —— 主题系统（深/浅/跟随系统）——
  const sysDark = window.matchMedia('(prefers-color-scheme: dark)');
  let themeMode = ['dark', 'light', 'system'].includes(localStorage.getItem('theme'))
    ? localStorage.getItem('theme')
    : 'dark';
  let theme = resolveTheme(themeMode);
  // 组件初始化阶段就会调 applyThemeMode，那时插件桥还没建（extFrameEls 在 TDZ 里会抛）
  let extsBridgeReady = false;
  function resolveTheme(mode) {
    return mode === 'system' ? (sysDark.matches ? 'dark' : 'light') : mode;
  }
  function applyThemeMode(m) {
    themeMode = m;
    localStorage.setItem('theme', m);
    theme = resolveTheme(m);
    document.documentElement.dataset.theme = theme;
    // 主题变了，已在跑的插件 iframe 也要换肤：重发一次 init（插件按 payload.theme 切 body.light）
    if (extsBridgeReady) {
      for (const name of Object.keys(extFrameEls)) {
        const ext = extList.find((e) => e.name === name);
        postToNameBoth(name, { type: 'pawch:init', payload: extInitPayload(ext) });
      }
    }
  }
  function toggleTheme() {
    applyThemeMode(theme === 'dark' ? 'light' : 'dark');
  }
  sysDark.addEventListener('change', () => {
    if (themeMode === 'system') applyThemeMode('system'); // 走同一条路：换肤 + 通知插件
  });
  applyThemeMode(themeMode); // 首屏即应用（组件初始化阶段，早于首帧渲染）

  // —— 自定义背景壁纸（dataURL 存 localStorage；选图时降采样防超配额）——
  let wallpaper = localStorage.getItem('wallpaper') || '';
  // 背景媒体类型（2026-11-03 视频壁纸）：video=视频文件走 /api/wallpaper/file，image=图片
  // （data: URL 或 /api/workspace/raw 载入的工作区图片都算 image）
  let wallpaperKind = localStorage.getItem('wallpaperKind') || 'image';
  let wallpaperOpacity = Number(localStorage.getItem('wallpaperOpacity') ?? 30);
  let wallpaperMsg = '';
  $: document.body.classList.toggle('has-wallpaper', !!wallpaper);
  // 微调模式：body 标记（hover 虚线高亮由 CSS 接管）；关闭时收起面板
  $: {
    document.body.classList.toggle('micro-edit-mode', editMode);
    if (!editMode) {
      microPanel = null;
      // 悬停高亮随模式关掉（右击监听留着无妨，handler 首行就 return）
      for (const doc of microFramesBound) {
        try {
          const hs = doc.getElementById('pawch-micro-hover');
          if (hs) hs.remove();
        } catch (e) {}
      }
    } else bindMicroFrames();
  }
  $: localStorage.setItem('wallpaperOpacity', String(wallpaperOpacity));
  function onWallpaperPick(e) {
    const file = e.currentTarget.files && e.currentTarget.files[0];
    e.currentTarget.value = '';
    if (!file) return;
    // 视频：localStorage 装不下，走后端存 <数据目录>\cache\wallpaper.<ext> + URL 引用
    if (file.type.startsWith('video/')) {
      wallpaperMsg = '保存中…';
      fetch('/api/wallpaper', {
        method: 'POST',
        headers: { 'Content-Type': file.type || 'video/mp4' },
        body: file,
      })
        .then(async (r) => {
          const d = await r.json().catch(() => ({}));
          if (!r.ok) throw new Error(d.error || `HTTP ${r.status}`);
          wallpaper = d.url;
          wallpaperKind = 'video';
          localStorage.setItem('wallpaper', wallpaper);
          localStorage.setItem('wallpaperKind', 'video');
          wallpaperMsg = '';
        })
        .catch((err) => {
          wallpaperMsg = `视频背景保存失败：${err.message || err}`;
        });
      return;
    }
    // 图片：原路径（canvas 压到 1920 存 data URL，localStorage 直接装）
    const reader = new FileReader();
    reader.onload = () => {
      const img = new Image();
      img.onload = () => {
        const max = 1920;
        const scale = Math.min(1, max / Math.max(img.naturalWidth, img.naturalHeight));
        const w = Math.max(1, Math.round(img.naturalWidth * scale));
        const h = Math.max(1, Math.round(img.naturalHeight * scale));
        const cv = document.createElement('canvas');
        cv.width = w;
        cv.height = h;
        const ctx = cv.getContext('2d');
        ctx.fillStyle = '#ffffff';
        ctx.fillRect(0, 0, w, h);
        ctx.drawImage(img, 0, 0, w, h);
        try {
          wallpaper = cv.toDataURL('image/jpeg', 0.85);
          wallpaperKind = 'image';
          localStorage.setItem('wallpaper', wallpaper);
          localStorage.setItem('wallpaperKind', 'image');
          wallpaperMsg = '';
        } catch (err) {
          wallpaperMsg = '壁纸保存失败（图片过大），换张小图试试';
        }
      };
      img.src = String(reader.result);
    };
    reader.readAsDataURL(file);
  }
  function clearWallpaper() {
    if (wallpaperKind === 'video') fetch('/api/wallpaper', { method: 'DELETE' }).catch(() => {});
    wallpaper = '';
    wallpaperKind = 'image';
    localStorage.removeItem('wallpaper');
    localStorage.removeItem('wallpaperKind');
    wallpaperMsg = '';
  }

  // —— 设置分区（左侧导航）——
  // 'params' 页签已并入「模型配置」；旧 localStorage 值回退到 model
  let settingsTab = ['appearance', 'model', 'roles', 'memory', 'skills', 'mcp', 'ext', 'schedule', 'data'].includes(localStorage.getItem('settingsTab'))
    ? localStorage.getItem('settingsTab')
    : 'model';
  function pickSettingsTab(t) {
    settingsTab = t;
    localStorage.setItem('settingsTab', t);
    if (t === 'mcp' || t === 'plugins') loadMcp();
    if (t === 'skills') loadSkills();
    if (t === 'ext') { loadGitRuntime(); loadExt(); loadSandboxInfo(); }
  }
  // 功能桥接被关掉时若正停在该页签，自动切走（features 随 applyState 回来，可能晚于首屏）
  $: if (settingsTab === 'roles' && features && features.roles === false) settingsTab = 'model';

  // 内置功能开关（插件页功能行）：写 settings.features，联动收起受影响的入口
  function toggleFeature(k) {
    features = { ...features, [k]: !features[k] };
    autoSave({ features }, 0);
    if (k === 'css_tool' && !features.css_tool && editMode) editMode = false;
    if (k === 'galaxy' && !features.galaxy && galaxyOpen) galaxyOpen = false;
  }

  // 同上：技能/MCP/插件分区的数据原先只在 pickSettingsTab 的点击里拉，页签从 localStorage
  // 恢复时（上次停在「技能」再开机）打开设置看到的是空列表，点一下页签才有——照记忆分区补齐。
  $: if (settingsOpen && settingsTab === 'skills') loadSkills();
  $: if (settingsOpen && settingsTab === 'mcp') loadMcp();
  $: if (settingsOpen && settingsTab === 'ext') {
    loadGitRuntime();
    loadSandboxInfo();
  }

  // 操作/删除类提示只在本页停留期间有效：下次进入该页（切页签或重开设置）就清掉，
  // 不许旧通知挂在页首过夜（用户 2026-11-03 统一口径，技能/MCP/定时/数据页同理）
  $: if (settingsOpen && settingsTab === 'ext') extMsg = '';
  $: if (settingsOpen || settingsTab) settingsMsg = '';


  // —— 技能（Skill / SKILL.md）分区 ——
  let skillsData = [];
  let skillsLoading = false;
  async function loadSkills() {
    skillsLoading = true;
    try {
      const r = await fetch('/api/skills').then((r) => r.json());
      skillsData = r.skills || [];
    } catch (e) {
      skillsData = [];
    }
    skillsLoading = false;
  }

  // —— MCP 插件分区
  let mcpServers = [];
  let wasmPlugins = []; // WASM 工具插件清单（loadMcp 灌；此前漏声明，能力菜单一开就 ReferenceError）
  // git 运行时状态（插件分区顶部卡片）：系统装的 / 内置包解压的 / 不可用
  let gitRuntime = null;
  // 021：沙箱自检（真拉起受限进程跑探针套件）——结果由沙箱插件的 action.run 桥拉走；
  // 插件页沙箱行的「检测」按钮也走这里（插件页不再给沙箱面板，用户 2026-10-03）
  let sandboxProbe = null;
  let sandboxProbeBusy = false;
  // 021：应用版本（侧栏「关于」行）
  let appVersion = null;
  async function loadVersion() {
    try {
      appVersion = await fetch('/api/version', { cache: 'no-store' }).then((r) => r.json());
    } catch (e) {
      appVersion = null;
    }
  }
  async function runSandboxProbe() {
    sandboxProbeBusy = true;
    try {
      sandboxProbe = await fetch('/api/sandbox/selftest', { cache: 'no-store' }).then((r) => r.json());
    } catch (e) {
      sandboxProbe = { error: String(e.message || e) };
    }
    sandboxProbeBusy = false;
  }
  async function loadGitRuntime() {
    try {
      gitRuntime = await fetch('/api/git/info', { cache: 'no-store' }).then((r) => r.json());
    } catch (e) {
      gitRuntime = null;
    }
  }
  // 沙箱档位（运行状态卡首卡）：配置档 / 生效档（含 AC→受限回退与原因）/ 探测链
  let sandboxInfo = null;
  async function loadSandboxInfo(refresh) {
    try {
      sandboxInfo = await fetch(`/api/sandbox/info${refresh ? '?refresh=1' : ''}`, { cache: 'no-store' }).then((r) => r.json());
    } catch (e) {
      sandboxInfo = null;
    }
  }
  async function loadMcp() {
    try {
      const r = await fetch('/api/mcp').then((r) => r.json());
      mcpServers = r.servers || [];
      wasmPlugins = r.wasm || [];
    } catch (e) {
      mcpServers = [];
      wasmPlugins = [];
    }
  }
  async function mcpAction(action, name, server) {
    // 删除类先弹确认（技能/MCP/插件/定时任务等删除一律先确认）
    if (action === 'remove' && !(await confirmAsk(`删除 MCP 服务「${name}」？\n配置与已授权状态会一并移除，不可恢复。`))) return;
    try {
      const r = await fetch('/api/mcp', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ action, name, server }),
      }).then((r) => r.json());
      if (r.error) settingsMsg = `MCP ${action} 失败: ${r.error}`;
      if (action === 'refresh' && r.ok) settingsMsg = `已连接，发现 ${r.tools_found} 个工具`;
      // 授权发起后带回事务快照（pending 时卡片里会给可点的授权链接）
      if (action === 'oauth_authorize' && r.tx) settingsMsg = r.tx.message || '授权已发起';
      if (action === 'oauth_revoke') settingsMsg = '已撤销授权';
    } catch (e) {
      settingsMsg = `MCP ${action} 失败: ${e.message || e}`;
    }
    await loadMcp();
  }

  // —— 021 组件总线 v0：UI 扩展（iframe 插槽 + 白名单数据桥）——
  // 隔离：iframe sandbox="allow-scripts"（无 allow-same-origin）→ 不透明源，拿不到宿主
  // DOM / localStorage / cookie；唯一通道是 postMessage，父侧只放行下面这几个只读方法。
  let extList = [];
  let extInfo = { app_dir: '', workspace_dir: '' };
  let extOpen = false;
  let extCurrent = null; // 当前打开的扩展（{ name, title, ... }）
  let extFrame = null; // 面板里那个 iframe 元素（当前打开的扩展）
  // 2026-10-08：弹窗型插件（manifest popup:true）——设置→插件「打开」开居中小窗口，
  // 不开右侧大面板。定下的交互：点打开 → 弹小窗 → 确定 → 落进新对话。
  let extPopup = null;
  let extPopupFrame = null;
  // 插件页 post 过来的内容高度（0 = 用 CSS 默认）：弹窗贴着内容长，不在下面留空白
  let extPopupFrameH = 0;
  let extMsg = '';
  let extSeq = 0;
  const extPending = new Map(); // 请求 id → 发起方（这次是父侧主动调用扩展，留口）

  // 内置插件的图标（24×24 stroke 内联标记，画在 .plug-ico 里；未知插件回退首字）
  // 2026-10-06 插件重整：5 个纯技能插件（Word/PDF/演示/表格/GitHub）降级成全局技能、
  // git-panel/sandbox-panel 移出插件位，它们的图标一并收掉；「中枢」（原自改）补图标
  const EXT_ICONS = {
    board: '<path d="M4.6 16.5a7.6 7.6 0 1 1 14.8 0"/><path d="M12 14.5 15.5 9.5"/><circle cx="12" cy="15" r="0.9" fill="currentColor" stroke="none"/>',
    trace: '<path d="M5 6.5h11.5M5 12h6.5M5 17.5h13.5"/><circle cx="19.6" cy="6.5" r="1.4"/><circle cx="14.4" cy="12" r="1.4"/><circle cx="21.4" cy="17.5" r="1.4"/>',
    usage: '<circle cx="12" cy="12" r="8.4"/><path d="M8.8 7.5 12 12l3.2-4.5"/><path d="M12 12v5.2"/><path d="M9.3 13.8h5.4"/>',
    browser: '<circle cx="12" cy="12" r="8.4"/><path d="M3.6 12h16.8"/><ellipse cx="12" cy="12" rx="3.8" ry="8.4"/>',
    selfmod: '<circle cx="12" cy="12" r="3.2"/><path d="M12 3.2v3M12 17.8v3M3.2 12h3M17.8 12h3M5.8 5.8l2.1 2.1M16.1 16.1l2.1 2.1M18.2 5.8l-2.1 2.1M7.9 16.1l-2.1 2.1"/>',
    roles: '<circle cx="6" cy="12" r="2.6"/><circle cx="18" cy="12" r="2.6"/><path d="M8.6 12h6.8M8.6 12l2.2-2.2M8.6 12l2.2 2.2M15.4 12l-2.2-2.2M15.4 12l2.2 2.2"/>',
    'css-ui': '<path d="M4 20h4L19 9l-4-4L4 16z"/><path d="M14.5 6.5l3 3"/>',
    nebula: '<circle cx="12" cy="12" r="1.4" fill="currentColor" stroke="none"/><ellipse cx="12" cy="12" rx="8.4" ry="3.2" transform="rotate(-24 12 12)"/><circle cx="18.8" cy="9" r="1" fill="currentColor" stroke="none"/><circle cx="5.6" cy="14.8" r="0.9" fill="currentColor" stroke="none"/>',
    'disconnect-cancel': '<path d="M10 13a4 4 0 0 0 5.7 0l2.3-2.3a4 4 0 0 0-5.7-5.7L11 6.3"/><path d="M14 11a4 4 0 0 0-5.7 0L6 13.3a4 4 0 0 0 5.7 5.7l1.3-1.3"/><path d="M4 4l16 16"/>',
    computer: '<rect x="3" y="4.5" width="18" height="12" rx="2"/><path d="M9 20.5h6M12 16.5v4"/>',
  };
  // 未知（用户自建）插件的通用图标：插头。菜单里不回落首字——「/」菜单插件行
  // 曾经整列都是硬编码的「P」徽标（真机实测），现在已知插件各画 SVG，未知的也画 SVG
  const EXT_ICON_FALLBACK =
    '<path d="M9 3.5v3.5M15 3.5v3.5"/><path d="M6.5 7h11v3.4a5.5 5.5 0 0 1-11 0z"/><path d="M12 15.9v4.6"/>';
  function extIcon(name) {
    return EXT_ICONS[name] || '';
  }
  // 菜单用的图标：已知插件各画各的，未知的回退插头（空串由调用方判）
  function menuExtIcon(name) {
    return EXT_ICONS[name] || EXT_ICON_FALLBACK;
  }
  // 侧栏标签的内置类型图标（插件标签用各自己的，见 rpTabIcon）——文字左边要有对应符号（用户 2026-10-03）
  const RP_TAB_ICONS = {
    artifacts:
      '<path d="M21 8.2 12 3.4 3 8.2v7.6l9 4.8 9-4.8z"/><path d="M3 8.2l9 4.8 9-4.8"/><path d="M12 13v7.6"/>',
    git: '<circle cx="6.5" cy="6" r="2.5"/><circle cx="6.5" cy="18" r="2.5"/><circle cx="17.5" cy="12" r="2.5"/><path d="M6.5 8.5v7M9 6h3.5a3 3 0 0 1 3 3v1"/>',
    web: '<circle cx="12" cy="12" r="8.4"/><path d="M3.6 12h16.8"/><ellipse cx="12" cy="12" rx="3.8" ry="8.4"/>',
    file: '<path d="M7 3.5h6.5L18 8v12.5H7z"/><path d="M13.5 3.5V8H18"/><path d="M9.5 12h5M9.5 15h5"/>',
    diff: '<rect x="3.5" y="4.5" width="7" height="15" rx="1.2"/><rect x="13.5" y="4.5" width="7" height="15" rx="1.2"/><path d="M5.3 12h3.4"/><path d="M15.3 12h3.4M17 10.3v3.4"/>',
  };
  function rpTabIcon(t) {
    return t.kind === 'ext' ? menuExtIcon(t.name) : RP_TAB_ICONS[t.kind] || RP_TAB_ICONS.file;
  }

  // 插件清单加载中的 promise（含失败重试的那几次）：早到的桥调用等它落地再判
  // 启用/权限，否则空清单会把 perm 类方法误判成「未声明权限」
  let extListPromise = null;
  function loadExt() {
    extListPromise = (async () => {
    try {
      const r = await fetch('/api/plugins', { cache: 'no-store' }).then((r) => r.json());
      extList = r.plugins || [];
      extInfo = { app_dir: r.app_dir || '', workspace_dir: r.workspace_dir || '' };
    } catch (e) {
      extList = [];
      extMsg = `读取插件失败: ${e.message || e}`;
      // 壳可能先于服务就绪（start.bat 是壳拉起服务）：拉不到就 2s 后重试几次，
      // 否则 dock 插件页签 / 后台 iframe 要等用户手动进一次设置→插件才出现
      if (extLoadRetry < 5) {
        extLoadRetry += 1;
        setTimeout(loadExt, 2000);
      }
    }
    })();
    return extListPromise;
  }
  let extLoadRetry = 0;

  // —— 服务端 → 前端 UI 事件流（2026-10-03：agent 的 ui 工具 / settings 工具推送）——
  // open_panel/close_panel：开/关侧栏面板标签（侧栏状态在前端，后端只能靠事件驱动）；
  // settings_changed：agent 改了设置，插件页/侧栏即时重拉清单——不然 agent 改完用户
  // 看到的还是旧状态（真机实锤：agent 停用用量插件落盘成功，插件页还显示开着）。
  function rpOpenExtByName(name) {
    if (!RP_EXT_PANELS.has(name)) return;
    const e = enabledExts.find((x) => x.name === name);
    if (!e) return; // 未启用或清单没到手：不硬开（渲染层本来也不显示）
    const u = (e.ui || []).find((x) => x.slot === 'dock' || x.slot === 'toolbar');
    addRpTab({ key: 'ext:' + name, kind: 'ext', name, title: (u && u.title) || e.title || name });
    rightPanelOpen = true;
    if (terminalOpen) terminalOpen = false;
  }

  // —— UI_OPS 注册表（2026-10-03 重构轮）：界面动作的自描述清单，服务两条入口 ——
  // 原先是 SSE 里逐 kind 的 if/else 长链 + 插件桥逐 method 的 switch，两套手写分支，
  // 各自独立长（阶段1 收了 SSE 侧，阶段4 把桥侧也并进来）。现在一张表按通道取形：
  // · SSE 事件（后端 fire_event 推来）：{ kind, title, risk, internal?, args, run(m) }
  //   —— m 是整条事件对象；
  // · 插件桥（扩展 iframe postMessage 调用）：{ bridge: true, perm?, title, run(ctx) }
  //   —— ctx = { params, from, ext, perms, reply, get }，run 负责恰好 reply 一次；
  //   perm = manifest 权限名，省略 = 只读信息类不占权限白名单（与旧 switch 口径一致）。
  // ① 新增界面/桥能力 = 本表加一行（run 闭包里调既有函数），不再改后端白名单/长分支；
  // ② 表可序列化（剥掉 run、滤掉 internal 与 bridge）成 ops 清单随 /api/ui/state 上报给
  //    后端，模型可读「界面能干什么、参数什么形状」——桥方法是插件私货，不上报；
  // ③ 破坏类（delete_session/undo_checkpoint）risk 标 destructive：后端 denylist 永不
  //    收进通用 fire_event，必须走具名方法保分级——前端清单说了不算（见 tools/ui.rs）。
  // risk 口径：safe=无持久破坏面（纯界面态）；normal=有持久副作用但可逆/常规；
  // destructive=不可逆，任何档位必问。settings_changed 是后端→前端内部同步，标 internal。
  const UI_OPS = {
    open_panel: {
      kind: 'open_panel', title: '打开右侧边栏面板', risk: 'safe', internal: false,
      args: 'name: 面板插件 id（trace=时间线/board=项目文件/usage=用量价格）',
      run(m) { rpOpenExtByName(m.name); },
    },
    close_panel: {
      kind: 'close_panel', title: '关闭右侧边栏面板', risk: 'safe', internal: false,
      args: 'name: 面板插件 id',
      run(m) { closeRpTab('ext:' + m.name); },
    },
    sidebar: {
      kind: 'sidebar', title: '开合左右侧边栏', risk: 'safe', internal: false,
      args: 'side: left=左侧会话列表 / right=右侧面板栏；open: true=显示 false=收起',
      run(m) {
        if (m.side === 'right') {
          if (m.open) {
            terminalOpen = false;
            rightPanelOpen = true;
            openRightPanel();
          } else {
            rightPanelOpen = false;
          }
        } else if (m.side === 'left') {
          sidebarCollapsed = !m.open;
        }
      },
    },
    terminal: {
      kind: 'terminal', title: '开合终端面板', risk: 'safe', internal: false,
      args: 'open: true=弹终端并收起右侧面板 false=收起终端',
      run(m) {
        if (m.open) {
          terminalOpen = true;
          rightPanelOpen = false;
        } else {
          terminalOpen = false;
        }
      },
    },
    settings: {
      kind: 'settings', title: '开关设置弹窗', risk: 'safe', internal: false,
      args: 'open: true/false；tab 可选（appearance/model/roles/memory/skills/mcp/ext/schedule）',
      run(m) {
        if (m.open) {
          settingsOpen = true;
          if (m.tab) settingsTab = m.tab;
        } else {
          settingsOpen = false;
        }
      },
    },
    theme: {
      kind: 'theme', title: '切换主题', risk: 'safe', internal: false,
      args: 'mode: dark（深色）/light（浅色）/system（跟随系统）',
      run(m) {
        if (['dark', 'light', 'system'].includes(m.mode)) applyThemeMode(m.mode);
      },
    },
    // 壁纸（2026-10-04 用户真机反馈「帮我改壁纸」agent 说没通道——能力审计漏项，
    // 与主题同族补上；持久但可逆所以 normal 档，read-only/auto 会问、完全访问直放）
    wallpaper: {
      kind: 'wallpaper', title: '设置/清除应用壁纸', risk: 'normal', internal: false,
      args: 'value: 工作区内图片相对路径（自动走 /api/workspace/raw 载入）或 data: 开头的图片 dataURL；空串=清除壁纸',
      run(m) {
        const v = String(m.value ?? '');
        wallpaper = v === '' || v.startsWith('data:') ? v : `/api/workspace/raw?path=${encodeURIComponent(v)}`;
        wallpaperKind = 'image'; // 这个桥只支持图片；视频背景由设置页「选择媒体」走 /api/wallpaper
        if (v === '') {
          localStorage.removeItem('wallpaper');
          localStorage.removeItem('wallpaperKind');
        } else {
          localStorage.setItem('wallpaper', wallpaper);
          localStorage.setItem('wallpaperKind', 'image');
        }
      },
    },
    open_tab: {
      kind: 'open_tab', title: '打开内置标签页', risk: 'safe', internal: false,
      args: 'key: artifacts=产物/git=git（审查+提交）/web=网页（白名单 RP_BUILTIN，别开乱七八糟的标签）',
      run(m) {
        // web 也受能力闸管：浏览器插件停用时 agent 开不出「网页」标签
        //（插件开关=能力总闸）。这里不加的话，rpVisibleTabs 会在
        // 渲染时把它滤掉，agent 侧却收到成功——「说做了其实没做」。
        if (m.key === 'web' && !browserOn) return;
        const b = RP_BUILTIN.find((x) => x.key === m.key);
        if (b) {
          addRpTab(b);
          rightPanelOpen = true;
          if (terminalOpen) terminalOpen = false;
        }
      },
    },
    close_tab: {
      kind: 'close_tab', title: '关闭标签页', risk: 'safe', internal: false,
      args: 'key: artifacts/git/web/ext:usage/file:<路径>',
      run(m) { if (m.key) closeRpTab(m.key); },
    },
    open_file: {
      kind: 'open_file', title: '打开文件预览标签', risk: 'safe', internal: false,
      args: 'path: 工作区相对或绝对路径',
      run(m) { if (m.path) openPreviewTab(String(m.path)); },
    },
    open_memory: {
      kind: 'open_memory', title: '打开记忆库（星云视图）', risk: 'safe', internal: false,
      args: '无（入口关着时前端不收）',
      run() { if (features.galaxy) galaxyOpen = true; },
    },
    css_inject: {
      kind: 'css_inject', title: '注入自定义 CSS', risk: 'normal', internal: false,
      args: 'css: CSS 内容（空内容报错，清空走 clear）；append: true=追加在原有手写规则后；clear: true=清空手写规则',
      run(m) {
        // 与用户粘进 CSS 注入框同一条路（applyCustomCss 里有 escalate/转义与
        // localStorage 落盘；css-ui 插件只是编辑器，应用端在宿主）
        if (typeof m.css === 'string') {
          customCss = m.append ? `${customCss}\n${m.css}` : m.css;
          applyCustomCss();
        }
      },
    },
    new_session: {
      kind: 'new_session', title: '新建会话并切过去', risk: 'normal', internal: false,
      args: '无',
      async run() { await startNewSession(); },
    },
    open_session: {
      kind: 'open_session', title: '切换到某条会话', risk: 'normal', internal: false,
      args: 'id: 会话 id（侧栏没有这条时会没反应）',
      async run(m) {
        const s = flatSessions.find((x) => x.id === m.id);
        if (s) await loadSession(m.id);
      },
    },
    rename_session: {
      kind: 'rename_session', title: '改会话标题', risk: 'normal', internal: false,
      args: 'id: 会话 id；title: 新标题（空串不理会）',
      async run(m) {
        const v = String(m.title || '').trim();
        if (v) {
          await fetch('/api/sessions/meta', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ id: m.id, title: v }),
          });
          // 改的是当前会话时同步顶栏（与 submitRename 同规）
          if (m.id === currentSessionId) currentTitle = v.slice(0, 60);
          loadSessions();
        }
      },
    },
    fork_session: {
      kind: 'fork_session', title: '分叉会话', risk: 'normal', internal: false,
      args: 'id: 要分叉的会话 id',
      async run(m) { await forkSession(m.id); },
    },
    pin_session: {
      kind: 'pin_session', title: '置顶/取消置顶会话', risk: 'normal', internal: false,
      args: 'id: 会话 id；on: true=执行 false=取消（默认 true）',
      async run(m) {
        await fetch('/api/sessions/meta', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ id: m.id, pinned: m.on !== false }),
        });
        loadSessions();
      },
    },
    delete_session: {
      kind: 'delete_session', title: '删除会话（不可逆）', risk: 'destructive', internal: false,
      args: 'id: 会话 id',
      async run(m) {
        // 确认已在工具层审批卡完成（Dangerous 必问），这里不再弹二次确认
        await fetch(`/api/session?id=${encodeURIComponent(m.id)}`, { method: 'DELETE' });
        if (m.id === currentSessionId) await startNewSession();
        else loadSessions();
      },
    },
    undo_checkpoint: {
      kind: 'undo_checkpoint', title: '撤销检查点（回滚工作区文件，不可逆）', risk: 'destructive', internal: false,
      args: 'id: 检查点 id',
      async run(m) {
        // 与改动条「撤销」按钮同一条路（自带二次确认）
        const cp = dockCp && dockCp.id === m.id ? dockCp : null;
        if (!cp) {
          const r = await fetch('/api/checkpoints').then((r2) => r2.json()).catch(() => null);
          const found = (r && r.checkpoints || []).find((c) => c.id === m.id);
          if (found) await undoCheckpoint(found);
        } else {
          await undoCheckpoint(cp);
        }
      },
    },
    // 后端改完设置推来的同步动作：插件清单/侧栏可见性要刷，settings 派生的界面状态
    // （模型卡片/思考档/配置名）也要刷——不然「实际改了、界面还显示旧的」
    settings_changed: {
      kind: 'settings_changed', title: '（内部）设置变更同步', risk: 'safe', internal: true,
      args: 'keys: 变更键清单（仅通知用）',
      run() { loadExt(); loadSettings(); },
    },
    // 顶层无头轮（定时任务/CLI 唤醒）跑完推来的同步动作：刷新侧栏会话列表，新建的会话
    // 即时出现，不必重启软件（2026-10-05 真机实测「重启才看得到」）。autoPick 传 false
    // ——后台刷新不许抢夺当前打开的会话。
    sessions_changed: {
      kind: 'sessions_changed', title: '（内部）会话清单刷新', risk: 'safe', internal: true,
      async run() {
        await refreshSessionsAndDropStale();
        // 2026-10-08 修：多窗口改名后本窗口顶栏标题停在旧值——displayTitle 优先取
        // currentTitle（本地快照），只刷列表不刷它。列表标题本就是权威值（含首条
        // 消息兜底），从新列表里取当前会话的标题对齐（本窗口自己改名走本地更新）。
        const cur = sessions.find((x) => x.id === currentSessionId);
        if (cur && cur.title && cur.title !== currentTitle) currentTitle = cur.title;
      },
    },

    // —— 插件桥方法（扩展 iframe postMessage；阶段4 并表）——
    // perm = manifest 声明的权限（省略 = 只读信息类，与 workspace.info 同级不占白名单）。
    // run(ctx) 自己 reply 恰好一次；抛错由桥分发兜底翻成 reply(false, e.message)。
    'app.info': {
      bridge: true, title: '插件握手（拿初始载荷）',
      run({ reply }) { reply(true, extInitPayload()); },
    },
    'workspace.info': {
      bridge: true, title: '读工作区信息',
      async run({ reply, get }) { reply(true, await get('/api/workspace')); },
    },
    'workspace.files': {
      bridge: true, title: '列工作区文件',
      async run({ reply, get }) { reply(true, await get('/api/workspace/files')); },
    },
    // 读工作区文本文件（「项目文件」文件预览用）：与 workspace.files 同为只读信息类，
    // 不占权限白名单；读不到的（二进制/越界）由 apiJson 以 reject 带回人话
    'workspace.file': {
      bridge: true, title: '读工作区文本文件',
      async run({ reply, get, params }) {
        const path = String(params.path || '').trim();
        if (!path) return reply(false, '缺少 path');
        reply(true, await get(`/api/workspace/file?path=${encodeURIComponent(path)}`));
      },
    },
    // 列工作区某目录的直接子项（「项目文件」树面板折叠/展开按需拉一层）
    'workspace.dir': {
      bridge: true, title: '列工作区目录子项',
      async run({ reply, get, params }) {
        const path = String(params.path || '').trim();
        reply(true, await get(`/api/workspace/dir?path=${encodeURIComponent(path)}`));
      },
    },
    'git.status': {
      bridge: true, title: '读 git 状态',
      async run({ reply, get }) { reply(true, await get('/api/git/status')); },
    },
    // 记忆检索（星云插件用）：会话级口径，插件拿不到别的会话的也不该看；不透明源本来
    // 也够不着 /api/memory/*。兼容扩展作者两种写法：top_k（后端字段）或 limit。
    // （旧 switch 里曾有两个同 method 的 case——后者带校验但是死代码且把后端字段 top_k
    // 错发成 limit，并表时按「活的那个」收编，死分支就地清除）
    'memory.search': {
      bridge: true, title: '记忆向量检索',
      async run({ reply, params }) {
        reply(
          true,
          await fetch('/api/memory/search', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ query: params.query || '', top_k: params.top_k || params.limit || 10 }),
          }).then((r) => r.json()),
        );
      },
    },
    'memory.library': {
      bridge: true, title: '读记忆库清单',
      async run({ reply, get }) { reply(true, await get('/api/memory/library')); },
    },
    'session.info': {
      bridge: true, title: '读当前会话信息',
      run({ reply }) { reply(true, { id: currentSessionId, title: displayTitle }); },
    },
    // —— 写操作（都要 manifest 里先声明权限）——
    'composer.insert': {
      bridge: true, perm: 'composer.insert', title: '往输入框插入文本',
      run({ reply, params }) {
        const text = String(params.text || '');
        input = input ? `${input}\n${text}` : text;
        reply(true, { ok: true });
      },
    },
    // 插件制作器：在指定工作区新建对话并把提示词预填进输入框（不代发——用户自己审自己回车）。
    // 机制全是现成管道的拼装：切工作区走 applyWorkspace（与「在项目内新建会话」同路，
    // 没切成就停下，否则会话会落到别的项目里），建会话走 startNewSession，预填就是
    // 给 bind:value={input} 赋值。有会话在跑时拒绝：切项目会断流，那轮就白跑了。
    'session.create': {
      bridge: true, perm: 'session.create', title: '在指定工作区新建对话并预填输入框',
      async run({ reply, params }) {
        if (Object.keys(streams).length > 0) {
          return reply(false, '有会话正在运行，等它跑完再试');
        }
        const ws = String(params.workspace || '').trim();
        if (ws && normPath(ws).toLowerCase() !== normPath(workspace).toLowerCase()) {
          await applyWorkspace(ws);
          // 没真正切过去（目录不存在/失败）就别新建，否则会话会落到别的项目里
          if (normPath(workspace).toLowerCase() !== normPath(ws).toLowerCase()) {
            return reply(false, `没能切换到工作区：${ws}`);
          }
        }
        await startNewSession();
        const text = String(params.text || '');
        if (text) {
          input = text;
          // 程序化赋值不触发 input 事件，autoRows 不会跑；且 DOM 要等 tick 才刷——
          // 不 await 就量高度，量到的是旧内容（空），框停在 1 行（真机实测）
          await tick();
          resetComposerHeight();
          composerInputEl?.focus();
        }
        // 弹窗型插件（插件制作器）：事已成，小窗让位——用户直接落进新对话看预填
        if (extPopup) extPopup = null;
        reply(true, { id: currentSessionId, workspace });
      },
    },
    'notify': {
      bridge: true, perm: 'notify', title: '发系统消息',
      run({ reply, params, ext, from }) {
        pushMsg(curKey(), {
          id: nextId(),
          role: 'system',
          content: `[${ext.title || from}] ${String(params.text || '')}`,
        });
        reply(true, { ok: true });
      },
    },
    'bus.publish': {
      bridge: true, perm: 'bus.publish', title: '扩展间广播（宿主转发）',
      run({ reply, params, from }) {
        // 扩展间通信：宿主转发给其它**已启用**扩展（发布者自己不发回）
        const topic = String(params.topic || '').trim();
        if (!topic) return reply(false, '缺少 topic');
        let delivered = 0;
        for (const e of enabledExts) {
          if (e.name === from) continue;
          const el = extFrameEls[e.name];
          if (el && el.contentWindow) {
            el.contentWindow.postMessage(
              { type: 'bus.message', from, topic, payload: params.payload ?? null },
              '*'
            );
            delivered += 1;
          }
        }
        reply(true, { delivered });
      },
    },
    'workspace.write': {
      bridge: true, perm: 'workspace.write', title: '写工作区文件（宿主替扩展问用户）',
      async run({ reply, params, ext, from }) {
        const path = String(params.path || '').trim();
        const content = String(params.content ?? '');
        if (!path) return reply(false, '缺少 path');
        // 写工作区是敏感动作：宿主替扩展问一次用户
        if (!(await confirmAsk(`插件「${ext.title || from}」要写工作区文件：\n\n${path}（${content.length} 字节）\n\n允许吗？`))) {
          return reply(false, '用户拒绝');
        }
        const r = await fetch('/api/workspace/file', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ path, content }),
        });
        const j = await r.json().catch(() => ({}));
        reply(!!r.ok, r.ok ? { ok: true, path: j.path, bytes: j.bytes } : j.error || '写入失败');
      },
    },
    // 写剪贴板（插件右键「复制路径」用）：iframe 是不透明源，剪贴板 API 不归它使，
    // 由宿主页代写；用户右键触发 + manifest 声明 clipboard 权限才放行
    'clipboard.write': {
      bridge: true, perm: 'clipboard', title: '写剪贴板（宿主页代写）',
      async run({ reply, params }) {
        const text = String(params.text ?? '');
        if (!text) return reply(false, '缺少 text');
        await putClipboard(text);
        reply(true, { ok: true });
      },
    },
    // —— 存储一门（027）：插件命名空间内的结构化读写（须声明 storage 权限）——
    // 后端按插件名隔离目录（plugin_data/<name>/），内核保留区（会话/设置/记忆）不在任何
    // 插件写路径上；键名白名单在前后端双重校验。
    'storage.read': {
      bridge: true, perm: 'storage', title: '读插件命名空间数据',
      async run({ reply, params, from }) {
        const key = String(params.key || '');
        const r = await fetch(`/api/plugin/data?name=${encodeURIComponent(from)}&key=${encodeURIComponent(key)}`, { cache: 'no-store' });
        if (r.status === 404) return reply(true, null); // 键不存在 = 空值，不算错
        const j = await r.json();
        reply(r.ok && j.ok, j.ok ? j.value : j.error || '读取失败');
      },
    },
    'storage.write': {
      bridge: true, perm: 'storage', title: '写插件命名空间数据',
      async run({ reply, params, from }) {
        const key = String(params.key || '');
        if (!/^[A-Za-z0-9_-]{1,64}$/.test(key)) return reply(false, '键名只能用字母数字与 - _（≤64 字符）');
        const r = await fetch('/api/plugin/data', {
          method: 'PUT',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ name: from, key, value: params.value ?? null }),
        });
        const j = await r.json().catch(() => ({}));
        reply(!!r.ok, r.ok ? { ok: true, bytes: j.bytes } : j.error || '写入失败');
      },
    },
    'storage.delete': {
      bridge: true, perm: 'storage', title: '删插件命名空间数据',
      async run({ reply, params, from }) {
        const key = String(params.key || '');
        if (!/^[A-Za-z0-9_-]{1,64}$/.test(key)) return reply(false, '键名只能用字母数字与 - _（≤64 字符）');
        const r = await fetch(`/api/plugin/data?name=${encodeURIComponent(from)}&key=${encodeURIComponent(key)}`, { method: 'DELETE' });
        const j = await r.json().catch(() => ({}));
        reply(!!r.ok, r.ok ? { ok: true, found: j.found } : j.error || '删除失败');
      },
    },
    // —— 只读桥：「用量价格」插件（plugins/usage 试点）拉当前会话的用量统计 ——
    'usage.session': {
      bridge: true, title: '读当前会话用量',
      async run({ reply, get, params }) {
        const group = params.group === 'model' ? 'model' : 'provider';
        const range = ['today', '7d', '30d', 'all'].includes(params.range) ? params.range : 'all';
        // 面板口径 = 当前对话：session 由宿主注入（插件拿不到别的会话的也不该看）。
        // 没有当前会话（新对话还没落 id / 刚启动）时**不能**拿空串去查——后端的空
        // session 是「全量」口径，面板会显示全部历史会话的累计（2026-10-30 真机实测：
        // 还没聊过一句就显示 20 亿输入 token、¥781 花费）。这里回零值 + 真实价格表口径，
        // 让面板走「本次对话还没有模型调用」空态。
        if (!currentSessionId) {
          let prices = {};
          try { prices = await get('/api/usage/prices'); } catch (e) { prices = {}; }
          reply(true, {
            currency: prices.currency || 'USD',
            usd_cny_rate: prices.usd_cny_rate || 7.2,
            totals: { prompt_tokens: 0, cached_tokens: 0, completion_tokens: 0, requests: 0, errors: 0, cost_usd: 0 },
            priced: true,
            spend_total_usd: prices.spend_total_usd || 0,
            spend_cap_usd: prices.spend_cap_usd || 0,
            session: '',
          });
          return;
        }
        const sid = encodeURIComponent(currentSessionId);
        reply(true, await get(`/api/usage?group=${group}&range=${range}&session=${sid}`));
      },
    },
    // 各模型历史占比（面板中间的圆环）：全量台账按模型聚合，**不带 session**——
    // 与上面「本次对话」是两个口径，同一页并排对照（用户 2026-11-01 要求）。
    'usage.models': {
      bridge: true, title: '读各模型历史用量占比',
      async run({ reply, get }) {
        reply(true, await get('/api/usage?group=model&range=all'));
      },
    },
    // 圆环同源的当前上下文占用（活数据）：面板的「输入 token」是各次请求累计，
    // 和圆环的窗口占用是两个口径——同一张卡里并排放，对照关系自明
    'session.usage': {
      bridge: true, title: '读上下文窗口占用（活数据）',
      run({ reply }) {
        reply(true, {
          projected: ctxUsage.projected,
          pressure: ctxUsage.pressure,
          window: ctxUsage.window,
          breakdown: ctxUsage.breakdown,
          api: apiTotals,
        });
      },
    },
    // —— 价格设置（保存/重算是写操作，须声明 usage.prices 权限）——
    'usage.prices': {
      bridge: true, title: '读价格表',
      async run({ reply, get }) { reply(true, await get('/api/usage/prices')); },
    },
    'usage.prices.save': {
      bridge: true, perm: 'usage.prices', title: '保存价格表',
      async run({ reply, params }) {
        const body = {
          prices: Array.isArray(params.prices) ? params.prices : [],
          currency: params.currency === 'CNY' ? 'CNY' : 'USD',
          usd_cny_rate: Number(params.usd_cny_rate) || 7.2,
        };
        const r = await fetch('/api/usage/prices', {
          method: 'PUT',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(body),
        });
        const j = await r.json().catch(() => ({}));
        reply(r.ok && j.ok !== false, r.ok ? j : j.error || '保存失败');
      },
    },
    'usage.recompute': {
      bridge: true, perm: 'usage.prices', title: '按当前价格重算历史费用（宿主替扩展问用户）',
      async run({ reply }) {
        if (!(await confirmAsk(`用当前价格表重算全部历史记录的费用？

之前已记的账会按新价改写（累计花费同步刷新）。`))) {
          return reply(false, '用户取消');
        }
        const r = await fetch('/api/usage/recompute', { method: 'POST' });
        const j = await r.json().catch(() => ({}));
        reply(r.ok && j.ok !== false, r.ok ? j : j.error || '重算失败');
      },
    },
    // —— 只读桥：trace 时间线面板（plugins/trace 试点）拉最近任务事件流 ——
    // 时间线**只显示当前对话**：session 参数就是过滤范围，
    // 没有当前会话（新对话还没落 id）时后端返回空——不漏别会话的任务进来。
    'trace.recent': {
      bridge: true, title: '读最近任务事件流',
      async run({ reply, get, params }) {
        const limit = Math.min(Math.max(Number(params.limit) || 10, 1), 50);
        const sid = currentSessionId ? `&session=${encodeURIComponent(currentSessionId)}` : '';
        reply(true, await get(`/api/trace?limit=${limit}${sid}`));
      },
    },
    // —— 权限放开轮：功能类插件读自己的功能状态、改自己的
    //    功能设置、触发宿主动作。三道口子都白名单化：能读的只有脱敏快照，能改的只有
    //    settings.features / roles（密钥/渠道/审批档/对外发送永不开放），
    //    能触发的只有 action.run 里枚举的那几个动作（2026-10-06：沙箱 CPU 可调项已撤）——
    'features.get': {
      bridge: true, title: '读功能脱敏快照',
      run({ reply }) {
        reply(true, {
          features,
          roles: roles || {},
          // 只给渠道的 id/名字/模型——密钥、base_url 一个字节都不出这道门
          configs: configs.map((c) => ({ id: c.id, label: configLabel(c), model: c.model || '' })),
          git: gitRuntime,
          sandbox: sandboxInfo,
        });
      },
    },
    'features.set': {
      bridge: true, perm: 'settings.feature', title: '开关功能',
      run({ reply, params }) {
        const k = String(params.feature || '');
        if (!['roles', 'css_tool', 'galaxy', 'cancel_on_disconnect', 'self_mod'].includes(k)) return reply(false, `未知功能键 ${k}`);
        const v = params.value !== false;
        // toggleFeature 是「取反」语义：先比再翻，别翻两次
        if ((features[k] || false) !== v) toggleFeature(k);
        reply(true, { features });
      },
    },
    'settings.feature.save': {
      bridge: true, perm: 'settings.feature', title: '存功能设置（roles 全量替换语义）',
      run({ reply, params }) {
        const patch = {};
        if (params.roles && typeof params.roles === 'object') {
          // 白名单必须与 RolesSettings 的字段一一对应（summary/title/vision/video/audio/
          // embedding/budget/subagent）——漏一个键，插件每存一次就静默抹掉那条路由
          // （真机验收时漏了多模态三个，用户 settings.json 里的 vision 被冲掉，实锤）。
          // PUT /api/settings 对 roles 是全量替换，所以这里先把现有 roles 合进来再发。
          const allow = ['summary', 'title', 'vision', 'video', 'audio', 'embedding', 'budget', 'subagent'];
          const next = { ...roles };
          for (const k of allow) {
            if (params.roles[k] === null) delete next[k];
            else if (typeof params.roles[k] === 'string') next[k] = params.roles[k];
          }
          patch.roles = next;
        }
        if (!Object.keys(patch).length) return reply(false, '没有可写的字段');
        autoSave(patch, 0);
        reply(true, { roles: patch.roles ?? roles });
      },
    },
    'action.run': {
      bridge: true, perm: 'action.run', title: '触发宿主动作（白名单枚举）',
      async run({ reply, params }) {
        const act = String(params.action || '');
        // 白名单逐个放行：都是设置页上用户本人也能点的同款动作（探测/检测/刷新），
        // 不新增任何宿主能力；沙箱探测会真拉受限进程，与点「检测」按钮等价
        if (act === 'git.detect') {
          await loadGitRuntime(true);
          reply(true, { git: gitRuntime });
        } else if (act === 'sandbox.detect') {
          await loadSandboxInfo(true);
          reply(true, { sandbox: sandboxInfo });
        } else if (act === 'sandbox.probe') {
          await runSandboxProbe();
          reply(true, { probe: sandboxProbe });
        } else if (act === 'skills.reload') {
          await loadSkills();
          reply(true, { ok: true });
        } else if (act === 'ext.reload') {
          await loadExt();
          reply(true, { ok: true });
        } else {
          reply(false, `未知动作 ${act}（git.detect / sandbox.detect / sandbox.probe / skills.reload / ext.reload）`);
        }
      },
    },
    // 在右侧边栏新开文件预览标签（「项目速览」点文件用）：纯界面动作，不占权限；
    // 只读来源是 workspace.file，内容安全由它兜
    'preview.open': {
      bridge: true, title: '右侧栏开文件预览标签',
      run({ reply, params }) {
        const path = String(params.path || '').trim();
        if (!path) return reply(false, '缺少 path');
        openPreviewTab(path);
        reply(true, { ok: true });
      },
    },

    // —— 子代理生命周期（2026-10-20 免打扰模式第 2 轮；server/subagent.rs ui_push）——
    // 实时条按 dispatch id 聚合：queued → spawned(running) → done/cancelled。
    // 只推给当时开着的页面，跑完的回看走 GET /api/subagents（面板打开时拉）。
    'subagent.queued': {
      kind: 'subagent.queued', title: '（内部）子代理排队', risk: 'safe', internal: true,
      args: 'dispatch/parent/label/task',
      run(m) { subagentUpsert(m, 'queued'); },
    },
    'subagent.spawned': {
      kind: 'subagent.spawned', title: '（内部）子代理派发', risk: 'safe', internal: true,
      args: 'dispatch/parent/child/label/model/task',
      run(m) { subagentUpsert(m, 'running'); },
    },
    'subagent.done': {
      kind: 'subagent.done', title: '（内部）子代理完成', risk: 'safe', internal: true,
      args: 'dispatch/parent/child/label/status/tokens/duration_ms',
      run(m) { subagentUpsert(m, m.status || 'done'); },
    },
    'subagent.cancelled': {
      kind: 'subagent.cancelled', title: '（内部）子代理取消', risk: 'safe', internal: true,
      args: 'dispatch/parent/child',
      run(m) { subagentUpsert(m, 'cancelled'); },
    },

    // —— 操作电脑实时动作（2026-10-20 第 4 轮；server computer 模块 ui_push）——
    // 每步键鼠/截图都推一条：聊天区实时条显示当前动作，点「查看屏幕」开主区面板
    // （最新截图 + 动作流水）。UI 不落侧边栏。
    'computer.action': {
      kind: 'computer.action', title: '（内部）电脑操作动作', risk: 'safe', internal: true,
      args: 'action/screenshot',
      run(m) {
        computerLive = {
          action: m.action || '',
          screenshot: m.screenshot || computerLive.screenshot,
          at: Date.now(),
        };
        computerMark();
      },
    },
  };

  // —— 子代理 UI（2026-10-20 免打扰模式第 2 轮）——  // 落点纪律：**不落侧边栏**（左侧会话栏/右侧 aside 都不进）——实时条贴在聊天区
  // 输入框上方（主区），回看面板是居中小弹窗（插件制作器同款形态）。
  // liveMap：本页生命周期内收到的派发事件（dispatch id → 记录）；跑完的回看从
  // GET /api/subagents 补（SSE 只推当时开着的页面，刷新/重开就丢）。
  let subagentLive = {};
  let subagentPanelOpen = false;
  let subagentHistory = [];

  function subagentUpsert(m, status) {
    const prev = subagentLive[m.dispatch] || {};
    // 整键替换（Svelte4 赋值才触发响应式；改属性不保险）
    subagentLive = {
      ...subagentLive,
      [m.dispatch]: {
        dispatch: m.dispatch,
        parent: m.parent || prev.parent || '',
        child: m.child || prev.child || '',
        label: m.label || prev.label || '子代理',
        model: m.model || prev.model || '',
        task: m.task || prev.task || '',
        status,
        tokens: m.tokens ?? prev.tokens ?? 0,
        duration_ms: m.duration_ms ?? prev.duration_ms ?? 0,
      },
    };
  }

  // 本会话的派发：实时（liveMap 里 parent 对得上的）+ 历史（面板打开时拉的全量）
  $: subagentSessionLive = Object.values(subagentLive).filter((d) => d.parent && d.parent === currentSessionId);
  $: subagentPanelRows = (() => {
    const liveIds = new Set(subagentSessionLive.map((d) => d.dispatch));
    const hist = (subagentHistory || []).filter((d) => d.parent === currentSessionId && !liveIds.has(d.dispatch));
    return [...subagentSessionLive, ...hist];
  })();
  // 实时条只露排队中/执行中；终态不占常驻位（要回看走面板）
  $: subagentActive = subagentSessionLive.filter((d) => d.status === 'queued' || d.status === 'running');

  async function openSubagentPanel() {
    subagentPanelOpen = true;
    await loadSubagentHistory();
  }

  // 启动/重连时拉一次派发历史：**chip 入口靠它出现**——原来只在打开面板时拉，
  // 而 chip 是开面板的唯一入口（实时条跑完就撤），刷新/重开页面后 chip 永远不出现，
  // 历史彻底看不到（死结，2026-10-21 实修）。
  async function loadSubagentHistory() {
    const r = await fetch('/api/subagents').then((r2) => r2.json()).catch(() => null);
    if (r) subagentHistory = r.dispatches || [];
  }

  async function cancelSubagent(dispatch) {
    const r = await fetch('/api/subagents/cancel', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ dispatch }),
    }).then((r2) => r2.json()).catch(() => null);
    // 本地即时翻态（UI 事件也会到，双保险；失败就等事件来纠）
    if (r && r.ok && subagentLive[dispatch]) {
      subagentLive = { ...subagentLive, [dispatch]: { ...subagentLive[dispatch], status: 'cancelled' } };
    }
  }

  function subagentStatusText(s) {
    return { queued: '排队中', running: '执行中', done: '完成', cancelled: '已取消', timeout: '超时', error: '失败' }[s] || s || '未知';
  }

  // —— 操作电脑 UI（第 4 轮；同子代理套路：实时条 + 居中小弹窗，不落侧边栏）——
  let computerLive = { action: '', screenshot: null, at: 0 };
  let computerPanelOpen = false;
  let computerPanelTimer = null;
  let computerState = { last_screenshot: null, last_size: null, actions: [] };
  // 面板改版：不做实时预览、不留常驻大图，也不放「最后一次
  // 操作」摘要卡——全部动作平铺成历史记录，最新的带高亮边，截图按条展开看。
  let computerOpenShots = {};
  // 动作文本里抠截图路径：server 记的格式是「截图 W×H → screenshots/xxx.png」
  function shotPathOf(line) {
    const m = String(line || '').match(/screenshots\/[^\s]+\.png/);
    return m ? m[0] : null;
  }
  function toggleComputerShot(i) {
    computerOpenShots = { ...computerOpenShots, [i]: !computerOpenShots[i] };
  }
  // 实时条只在本页生命周期内有效（事件流断线就断）；5 秒无新动作自动收起，
  // 之后留一颗 chip 开面板回看（别常驻占输入框上方的位）
  let computerFresh = false;
  let computerTimer = null;
  // 叉掉常驻 chip 后永久不再露（localStorage 持久化，刷新/重启也算数）。
  // 实时动作条（computer-strip）仍会在新动作后短暂出现，那是操作反馈，不是提示。
  let computerChipDismissed = (() => {
    try { return localStorage.getItem('computerChipDismissed') === '1'; } catch { return false; }
  })();
  function dismissComputerChip() {
    computerChipDismissed = true;
    try { localStorage.setItem('computerChipDismissed', '1'); } catch {}
  }
  function computerMark() {
    computerFresh = true;
    clearTimeout(computerTimer);
    computerTimer = setTimeout(() => (computerFresh = false), 5000);
  }

  async function openComputerPanel() {
    computerPanelOpen = true;
    const r = await fetch('/api/computer/state').then((r2) => r2.json()).catch(() => null);
    if (r) computerState = r;
    // 面板开着时每 2s 重拉动作流水（关掉即停）：agent 还在动的话，历史记录
    // 要跟着走。截图按条展开，不常驻。
    clearInterval(computerPanelTimer);
    computerPanelTimer = setInterval(() => {
      if (!computerPanelOpen) return;
      fetch('/api/computer/state')
        .then((r2) => r2.json())
        .then((s) => { if (s) computerState = s; })
        .catch(() => {});
    }, 2000);
  }
  function closeComputerPanel() {
    computerPanelOpen = false;
    clearInterval(computerPanelTimer);
    computerPanelTimer = null;
  }

  // 启动/重连时拉一次（刷新后「电脑操作」chip 入口也能出现；没有操作记录就不占位）
  async function loadComputerState() {
    const r = await fetch('/api/computer/state').then((r2) => r2.json()).catch(() => null);
    if (r && (r.last_screenshot || (r.actions || []).length)) computerState = r;
  }

  // 可序列化的 ops 清单（剥掉 run 函数）——随 /api/ui/state 上报给后端读。
  // 这就是「界面自报家门」：模型问「你能干什么」有清单可查，不用开发者替界面写死。
  // 桥条目（bridge: true）是插件私货不在此列——agent 的 ui 通道与插件通道各走各的。
  function uiOpsManifest() {    return Object.values(UI_OPS)
      .filter((o) => !o.internal && !o.bridge)
      .map((o) => ({ kind: o.kind, title: o.title, risk: o.risk, args: o.args }));
  }

  // 界面状态上报（agent 的 ui state 读它；open/close 识别 no-op）：边栏/终端/主题/设置弹窗
  // 一变就报——reactive 覆盖所有变更源（顶栏手点、恢复钮、agent 事件），多窗口后上报者覆盖前上报者
  let uiStateSeq = 0;
  async function reportUiState() {
    const seq = ++uiStateSeq;
    try {
      await fetch('/api/ui/state', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          sidebars: { left: !sidebarCollapsed, right: rightPanelOpen },
          terminal_open: terminalOpen,
          settings_open: settingsOpen,
          theme: themeMode,
          tabs_open: rpTabs.length,
          active_tab: rpTab || '',
          // 用户手写 CSS（agent inject_css 前先读，才能做保留式追加）
          custom_css: customCss,
          // 自描述动作清单：后端据此喂给模型「界面能干什么」+ 通用通道准入参考
          ops: uiOpsManifest(),
          seq,
        }),
      });
    } catch {}
  }
  $: reportUiState(sidebarCollapsed, rightPanelOpen, rpTabs, terminalOpen, settingsOpen, themeMode);

  try {
    const uiEventsSrc = new EventSource('/api/ui/events');
    // 连上/重连即重拉一次设置与插件清单（2026-10-05）：服务重启（换二进制/崩溃恢复）
    // 后事件流会断线自动重连，但断连期间推的事件全丢了——页面会一直显示重启前的旧态，
    // 用户以为「又没刷新」。onopen 在首次连接也会触发，多一次 GET 无害。
    uiEventsSrc.onopen = () => { loadExt(); loadSettings(); loadComputerState(); loadSubagentHistory(); };
    uiEventsSrc.onmessage = async (ev) => {
      let m;
      try { m = JSON.parse(ev.data); } catch { return; }
      // 查表执行（UI_OPS 通用入口）：新界面事件只在前端注册表加一行，后端 fire_event
      // 白名单/本处都不再为它写分支。未登记的 kind 静默忽略（老服务推的/别的来源）；
      // 桥条目（bridge: true）只归 postMessage 分发，SSE 侧够不着。
      const op = UI_OPS[m.kind];
      if (op && !op.bridge) await op.run(m);
    };
  } catch {}

  // 已启用的扩展（停用的不加载、不给按钮）
  $: enabledExts = extList.filter((e) => e.enabled !== false && e.ok);
  // 电脑操作现在住在插件列表里（2026-11-04）：插件停用 = 聊天区的「电脑操作」入口一并收起
  $: computerOnUi = enabledExts.some((e) => e.name === 'computer');
  // 分组（Git/沙箱/内置面板插件统一归「内置」一组，别分两层）；
  // 第三方（工作区来源 / user 来源 / 草稿）单独一组
  $: builtInExts = extList.filter((e) => e.source !== 'workspace' && e.origin !== 'user' && !e.draft);
  $: thirdPartyExts = extList.filter((e) => !builtInExts.includes(e));
  // —— 右侧边栏标签：可见（开着的）+ 可添加（+ 号菜单 / 空态选择页）——
  // 插件面板只收 RP_EXT_PANELS 三件套（时间线/项目文件/用量，旧 toolbar 视同面板并入），
  // 其余插件不再进侧栏——+ 菜单与选择页都不出（六个够了）。
  // 停用插件的标签渲染时隐藏；插件清单没到手（extList 空）时不判 ext 标签失效——
  // 那只是「还没加载」，不是「被停用」；否则默认激活的时间线标签会被提前顶成「产物」（真机冒烟实锤）。
  // 「网页」是宿主内置面板，但能力归「浏览器」插件管（插件开关=能力总闸，
  // 关掉浏览器 = agent 调不动 + 面板入口也没了）。同一套 extList 空载豁免也适用于它。
  $: browserOn = extList.length === 0 || enabledExts.some((e) => e.name === 'browser');
  $: rpVisibleTabs = rpTabs.filter((t) => {
    if (t.kind === 'web') return browserOn;
    return t.kind !== 'ext' || (RP_EXT_PANELS.has(t.name) && (!extList.length || enabledExts.some((e) => e.name === t.name)));
  });
  $: rpAddable = [
    ...RP_BUILTIN.filter((b) => (b.kind !== 'web' || browserOn)),
    ...enabledExts
      .filter((e) => RP_EXT_PANELS.has(e.name))
      .flatMap((e) =>
        (e.ui || [])
          .filter((u) => u.slot === 'dock' || u.slot === 'toolbar')
          .map((u) => ({ key: 'ext:' + e.name, kind: 'ext', name: e.name, title: u.title || e.title || e.name }))
      ),
  ].filter((p) => !rpTabs.some((t) => t.key === p.key));
  // 激活标签掉出可见集合（插件被停用/标签被关）→ 退回第一个可见标签
  $: if (rpVisibleTabs.length && !rpVisibleTabs.some((t) => t.key === rpTab)) rpTab = rpVisibleTabs[0].key;
  $: if (!rpVisibleTabs.length && rpTab) rpTab = '';
  // —— 标签条溢出滑杆（分隔线中间加根很细的，别占布局顶起标签）——
  // 原生滚动条藏掉（占布局），自绘一根压在下边框线上；没溢出不出杆
  let rpStripEl = null;
  let rpStripBar = null; // { track, w, x } px；null = 没溢出
  function rpStripSync() {
    const el = rpStripEl;
    if (!el || el.scrollWidth <= el.clientWidth + 1) {
      if (rpStripBar) rpStripBar = null;
      return;
    }
    const track = el.clientWidth;
    const w = Math.max(24, Math.round((track * track) / el.scrollWidth));
    const x = Math.round(((track - w) * el.scrollLeft) / (el.scrollWidth - track));
    rpStripBar = { track, w, x };
  }
  afterUpdate(rpStripSync); // 标签增删/面板拖宽都会触发组件更新，顺带重算；拖动中由 on:scroll 补
  afterUpdate(webStripSync); // 网页页签同款：增删/拖宽重算滑杆
  let rpStripDrag = null; // { x0, s0 } 按下时指针 x 与 scrollLeft
  function rpStripDown(e) {
    const el = rpStripEl;
    if (!el || !rpStripBar) return;
    const r = el.getBoundingClientRect();
    const ratio = Math.min(1, Math.max(0, (e.clientX - r.left - rpStripBar.w / 2) / (rpStripBar.track - rpStripBar.w)));
    el.scrollLeft = ratio * (el.scrollWidth - rpStripBar.track);
    rpStripDrag = { x0: e.clientX, s0: el.scrollLeft };
    e.currentTarget.setPointerCapture(e.pointerId);
  }
  function rpStripMove(e) {
    const el = rpStripEl;
    if (!el || !rpStripDrag || !rpStripBar) return;
    const span = Math.max(1, rpStripBar.track - rpStripBar.w);
    el.scrollLeft = rpStripDrag.s0 + ((e.clientX - rpStripDrag.x0) * (el.scrollWidth - el.clientWidth)) / span;
  }
  function rpStripUp() {
    rpStripDrag = null;
  }
  // 网页页签滑杆（「和右侧边栏那一条一样」）：同 rp-strip 模式，
  // 压在 .web-tabbar 下边框线上，页签多到溢出时出现，可点可拖。
  let webStripEl = null;
  let webStripBar = null; // { track, w, x } px；null = 没溢出
  function webStripSync() {
    const el = webStripEl;
    if (!el || el.scrollWidth <= el.clientWidth + 1) {
      if (webStripBar) webStripBar = null;
      return;
    }
    const track = el.clientWidth;
    const w = Math.max(24, Math.round((track * track) / el.scrollWidth));
    const x = Math.round(((track - w) * el.scrollLeft) / (el.scrollWidth - track));
    webStripBar = { track, w, x };
  }
  let webStripDrag = null; // { x0, s0 }
  function webStripDown(e) {
    const el = webStripEl;
    if (!el || !webStripBar) return;
    const r = el.getBoundingClientRect();
    const ratio = Math.min(1, Math.max(0, (e.clientX - r.left - webStripBar.w / 2) / (webStripBar.track - webStripBar.w)));
    el.scrollLeft = ratio * (el.scrollWidth - webStripBar.track);
    webStripDrag = { x0: e.clientX, s0: el.scrollLeft };
    e.currentTarget.setPointerCapture(e.pointerId);
  }
  function webStripMove(e) {
    const el = webStripEl;
    if (!el || !webStripDrag || !webStripBar) return;
    const span = Math.max(1, webStripBar.track - webStripBar.w);
    el.scrollLeft = webStripDrag.s0 + ((e.clientX - webStripDrag.x0) * (el.scrollWidth - el.clientWidth)) / span;
  }
  function webStripUp() {
    webStripDrag = null;
  }
  // 面板打开时待投递的插槽事件（iframe 还没 load 完就先排队）
  let extPendingMsgs = [];

  // 用户 2026-11-03：「新建插件」不再就地建空壳，直接开插件制作器的弹窗——
  // 要求写清楚、选工作区、确定就落进新对话。制作器插件不在列表里（被删/老包）才退回旧表单。
  function openMaker() {
    const maker = [...builtInExts, ...thirdPartyExts].find((e) => e.name === 'plugin-maker');
    if (maker && maker.popup && maker.entry) openExt(maker);
    else extCreateOpen = true;
  }

  function openExt(e, slotEvent) {
    // 弹窗型插件（manifest popup:true）：开居中小窗口，不开侧面板；设置弹窗同时让位，
    // 小窗确定后用户直接落进新对话（不再被设置弹窗压着）
    if (e.popup && e.entry) {
      extPopup = e;
      extMsg = '';
      settingsOpen = false;
      return;
    }
    // 面板此刻是不是已经开着同一个扩展（决定插槽事件直接投还是排队等面板 iframe load）
    const wasOpenSame = extOpen && extCurrent && extCurrent.name === e.name;
    extCurrent = e;
    extMsg = '';
    extOpen = true;
    // 2026-10-25 E2E 补测实锤：从设置弹窗里点「打开」，面板开在弹窗背后、看起来像没反应
    // （iframe 已加载只是被盖住）——面板分支和 popup 分支一样让设置弹窗让位
    settingsOpen = false;
    // 与右侧产物面板互斥，避免三个面板挤在一起
    rightPanelOpen = false;
    terminalOpen = false;
    if (slotEvent) {
      const msg = { type: 'pawch:slot', slot: slotEvent.slot, id: slotEvent.id || '', title: slotEvent.title || '' };
      // 注意：打开的瞬间后台那个常驻 iframe 会被卸载，别往里投——排队等**面板** iframe load 时投
      if (wasOpenSame && extFrameEls[e.name] && extFrameEls[e.name].contentWindow) postToNameBoth(e.name, msg);
      else extPendingMsgs.push({ name: e.name, msg });
    }
  }

  /// 面板 iframe 加载完成：登记元素 + 发 init + 补投排队中的插槽事件
  function onExtFrameLoad(name) {
    // 右侧标签里的插件 iframe（bind:this 进 dockPaneFrames）也要登记：postToName 靠
    // extFrameEls 找它（标签切换时 iframe 常驻不重载，状态不丢）
    if (dockPaneFrames[name]) extFrameEls[name] = dockPaneFrames[name];
    // 独立插件面板那个 iframe 绑的是 extFrame，这里补登记：否则 postToName 找不到它，
    // init / 插槽事件都投不进去（后台 iframe 此时已经卸载）
    if (extFrame && extCurrent && extCurrent.name === name) extFrameEls[name] = extFrame;
    // 弹窗型插件那个 iframe 绑的是 extPopupFrame，同样要登记（后台 iframe 此时已卸载）
    if (extPopupFrame && extPopup && extPopup.name === name) extFrameEls[name] = extPopupFrame;
    const ext = extList.find((e) => e.name === name);
    postToNameBoth(name, { type: 'pawch:init', payload: extInitPayload(ext) });
    // 微调桥探针：桥脚本就位后会回 micro-ack，宿主随之补下发已存的微调规则；
    // 旧插件不认识这条消息（无 ack）→ extMicroCap 标 false，右击 iframe 时给「暂不支持」说明
    setTimeout(() => {
      if (extFrameEls[name]) {
        sendExtMicro(name, 'enter', { listen: editMode });
        setTimeout(() => {
          if (extMicroCap[name] !== true) extMicroCap = { ...extMicroCap, [name]: false };
        }, 600);
      }
    }, 250);
    extPendingMsgs = extPendingMsgs.filter((p) => {
      if (p.name !== name) return true;
      postToName(name, p.msg);
      return false;
    });
  }

  /// 打开插件的指定页（manifest settings_entry，如「用量价格」插件的价格设置页）
  function openExtPage(e, page) {
    const wasOpenSame = extOpen && extCurrent && extCurrent.name === e.name;
    extCurrent = { ...e, entry: page };
    extMsg = '';
    extOpen = true;
    rightPanelOpen = false;
    terminalOpen = false;
    if (!wasOpenSame) extPendingMsgs.push({ name: e.name, msg: { type: 'pawch:init', payload: extInitPayload(e) } });
  }

  /// 扩展启用/停用：写设置 + 刷新清单
  // 行内 tooltip：把版本/目录/工具/权限/界面插槽收进悬停提示，行体只留一句描述（单行紧凑，别占地方）
  function extTip(e) {
    const parts = [];
    if (e.version) parts.push(`v${e.version}`);
    if (e.dir) parts.push(e.dir);
    if (e.tools?.length) parts.push(`工具: ${e.tools.map((t) => `wasm__${e.name}__${t[0] ?? t.name}`).join('、')}`);
    if (e.permissions?.length) parts.push(`权限: ${e.permissions.join('、')}`);
    if (e.ui?.length) parts.push(`界面: ${e.ui.map((u) => u.slot).join('、')}`);
    return parts.join('\n');
  }
  // dock/toolbar 面板插件（时间线/项目文件/用量 + 其余面板插件）：插件页不再给「打开/设置」
  // 入口——面板归侧边栏，不在这重复给一份界面
  function isDockExt(e) {
    return (e.ui || []).some((u) => u.slot === 'dock' || u.slot === 'toolbar');
  }
  function sandboxProbeTip(p) {
    return (p.results || []).map((r) => `${r.ok ? '✓' : '✗'} ${r.name}${r.detail ? '：' + r.detail : ''}`).join('\n');
  }

  async function toggleExt(e) {
    try {
      // 开关显示的是「功能实际状态」：带 feature 的插件看 feature_on（停用名单只是 enabled），
      // 切换也按同一个口径算，否则会出现「点了亮/灭但行为不变」
      const on = e.feature ? e.feature_on === true : e.enabled !== false;
      const r = await fetch('/api/plugins/toggle', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: e.name, enabled: !on }),
      }).then((r) => r.json());
      if (r.plugins) extList = r.plugins;
      if (on && extCurrent && extCurrent.name === e.name) extOpen = false; // 停用后关面板
    } catch (err) {
      extMsg = `切换失败: ${err.message || err}`;
    }
  }

  // 契约 v1（027）：draft 转正（落盘持久化、来源改 user）/ 丢弃（内存即清）
  async function promoteExt(e) {
    try {
      const r = await fetch('/api/plugins/promote', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: e.name }),
      }).then((r) => r.json());
      if (r.error) extMsg = `转正失败: ${r.error}`;
      else extMsg = `已转正「${e.title || e.name}」（来源已改为 user，重启后仍在）`;
      if (r.plugins) extList = r.plugins;
    } catch (err) {
      extMsg = `转正失败: ${err.message || err}`;
    }
  }

  async function discardExt(e) {
    if (!(await confirmAsk(`丢弃草稿插件「${e.title || e.name}」？内存内容将立即消失。`))) return;
    try {
      const r = await fetch(`/api/plugins/draft?name=${encodeURIComponent(e.name)}`, { method: 'DELETE' }).then((r) => r.json());
      if (r.error) extMsg = `丢弃失败: ${r.error}`;
      else extMsg = `已丢弃「${e.title || e.name}」`;
      if (r.plugins) extList = r.plugins;
    } catch (err) {
      extMsg = `丢弃失败: ${err.message || err}`;
    }
  }

  // 2026-10-06 插件重整：插件删除三件套的界面半边（「新建只建不删」缺口的收口）。
  // 删除类一律先弹确认（delete-class-confirm 纪律）；只有 user/draft 来源能删，built_in
  // 后端拒删并说人话。停用名单/信任表里的残留由后端一并清掉。
  async function removeExt(e) {
    if (!(await confirmAsk(`删除插件「${e.title || e.name}」？\n插件目录与全部文件会一并删除，不可恢复。`))) return;
    try {
      const r = await fetch('/api/plugins/remove', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: e.name }),
      }).then((r) => r.json());
      if (r.error) {
        extMsg = `删除失败: ${r.error}`;
        return;
      }
      extMsg = `已删除插件「${e.title || e.name}」（目录 ${r.dir || ''}）`;
      // 2026-10-25 E2E 补测实锤：删完插件它的面板标签还挂着，点开是死页——一并回收。
      // 两个面：右侧 rpTab（时间线这类进侧栏的）+ 插件面板本体（extOpen 正展示着它的话）
      closeRpTab('ext:' + e.name);
      if (extCurrent && extCurrent.name === e.name) extOpen = false;
      if (r.plugins) extList = r.plugins;
      else await loadExt();
    } catch (err) {
      extMsg = `删除失败: ${err.message || err}`;
    }
  }

  // 用户 2026-10-05：插件页「新建插件」——和技能/MCP 同款，表单默认收起。创建只是在
  // 工作区 plugins/ 下生成脚手架（manifest + index.html），不等于放行：新插件出现在
  // 第三方分组，仍要走既有「审核/信任」链路才加载
  let extCreateOpen = false;
  let extCreateForm = { name: '', description: '' };
  function cancelCreateExt() {
    extCreateForm = { name: '', description: '' };
    extCreateOpen = false;
  }
  async function createExt() {
    const name = extCreateForm.name.trim();
    if (!name) {
      extMsg = '插件名不能为空';
      return;
    }
    try {
      const resp = await fetch('/api/plugins/create', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name, description: extCreateForm.description.trim() }),
      });
      // 旧版本服务没有这个路由时返回的是非 JSON 空体——直接 .json() 会炸出一段
      // 「Unexpected end of JSON input」洋文，这里兜住说人话（2026-10-05 真机实测）
      const r = await resp.json().catch(() => null);
      if (!r) {
        extMsg = `新建插件失败: 服务没有返回数据（HTTP ${resp.status}）。如果刚更新过应用，重启后再试`;
        return;
      }
      if (r.error) {
        extMsg = `新建插件失败: ${r.error}`;
        return;
      }
      extMsg = `已在工作区 plugins/ 下建好「${name}」，到第三方分组点「信任」后即可打开`;
      if (r.plugins) extList = r.plugins;
      cancelCreateExt();
    } catch (err) {
      extMsg = `新建插件失败: ${err.message || err}`;
    }
  }

  function extInitPayload(ext) {
    return {
      version: appVersion?.version || '',
      root: appVersion?.root || '',
      theme: theme, // 已解析的实际主题（dark/light），system 模式也下发真值，插件别自己猜
      theme_mode: themeMode,
      sessionId: currentSessionId,
      title: displayTitle,
      // 告诉扩展「你是谁」（权限 / 插槽声明它自己在 manifest 里写）
      ext: ext ? { name: ext.name, title: ext.title, permissions: ext.permissions || [] } : null,
    };
  }

  /// 扩展 iframe 元素表：面板里那个 + 后台常驻的那些（扩展间通信要它们都活着）。
  let extFrameEls = {};
  extsBridgeReady = true; // 插件桥就位：此后主题切换才会通知到插件 iframe
  function extWindowName(win) {
    for (const [name, el] of Object.entries(extFrameEls)) {
      if (el && el.contentWindow === win) return name;
    }
    return null;
  }
  function postToName(name, msg) {
    const el = extFrameEls[name];
    if (el && el.contentWindow) el.contentWindow.postMessage(msg, '*');
  }

  /// 插件桥协议双前缀发送（改名迁移轮 2026-10-22：agent:* → pawch:*，兼容一个版本）：
  /// init / slot / event 这类「不等请求」的消息两种前缀都发——在役旧插件（改名轮之前
  /// 用户自建的）只听 agent:，新插件只听 pawch:，各得其所。请求回复不走这里（必须按
  /// 请求方的前缀回，见 onExtMessage）。
  function postToNameBoth(name, msg) {
    postToName(name, msg);
    postToName(name, { ...msg, type: msg.type.replace(/^pawch:/, 'agent:') });
  }

  /// 广播「当前会话变了」给所有已启用插件面板：时间线只显示当前对话
  /// 切会话/切工作区/新对话后面板必须跟着换数据——
  /// iframe 常驻不重载，不广播就还停在旧对话上。通用事件，不认的类型按插件
  /// 契约忽略。
  function notifyExtsSession(id) {
    for (const e of enabledExts) {
      postToNameBoth(e.name, { type: 'pawch:event', event: 'session', id: id || '' });
    }
  }
  /// 广播「当前工作区变了」：插件面板常驻不重载，工作区范围的数据（文件树、
  /// 工作区下拉）必须跟着刷新，否则切项目后还显示旧项目的清单。
  function notifyExtsWorkspace(path) {
    for (const e of enabledExts) {
      postToNameBoth(e.name, { type: 'pawch:event', event: 'workspace', path: path || '' });
    }
  }
  /// 刷新/重载时把「正在跑的会话」的内存消息打个快照：整页导航会丢内存态，
  /// 服务端此刻通常只落了用户消息（助手内容还在流里），不兜这一层用户会以为内容丢了。
  /// 只存当前标签页（sessionStorage），只认 10 分钟内、同一会话、且服务端仍 running。
  /// 排队待发的消息（outbox）一起存：服务端文件里没有它们，不存就永远发不出去了。
  const LIVE_STASH_KEY = 'pawchLiveStash';
  function stashLiveState() {
    try {
      const key = curKey();
      // 草稿与文件预览的未保存修改跟「有没有流」无关：每次离开页面都落一次
      persistComposer(key);
      persistFileDrafts();
      if (!streams[key]) return; // 没有在跑的流，消息快照不需要兜
      const msgs = sessionMsgs[key];
      if (!msgs || !msgs.length) return;
      const queued = (outbox[key] || []).map((it) => ({
        id: it.id,
        text: it.text,
        atts: it.atts || [],
        convs: it.convs || [],
        ts: it.ts || Date.now(),
      }));
      const raw = JSON.stringify({ sid: currentSessionId || '', ts: Date.now(), msgs, outbox: queued });
      if (raw.length > 2_000_000) return; // 超配额就放弃（会话太大，宁可不兜）
      sessionStorage.setItem(LIVE_STASH_KEY, raw);
    } catch {}
  }
  function readLiveStash(id) {
    try {
      const raw = sessionStorage.getItem(LIVE_STASH_KEY);
      if (!raw) return null;
      const d = JSON.parse(raw);
      if (!d || d.sid !== id || !Array.isArray(d.msgs) || !d.msgs.length) return null;
      if (Date.now() - (d.ts || 0) > 10 * 60 * 1000) return null;
      return d;
    } catch {
      return null;
    }
  }
  function clearLiveStash() {
    try {
      sessionStorage.removeItem(LIVE_STASH_KEY);
    } catch {}
  }
  /// 刷新前排队、但还没发出去的消息：服务端会话文件里没有它们。
  /// 从快照恢复时先把「排队中」气泡铺回去，再接上正常出队流程。
  function restoreQueuedOutbox(key, items) {
    const clean = items
      .filter((it) => it && it.id)
      .map((it) => ({ id: it.id, text: it.text || '', atts: it.atts || [], convs: it.convs || [], ts: it.ts || Date.now() }));
    if (!clean.length) return;
    outbox = { ...outbox, [key]: clean };
    const have = new Set((sessionMsgs[key] || []).map((m) => m.id));
    const bubbles = clean
      .filter((it) => !have.has(it.id))
      .map((it) => ({ id: it.id, role: 'user', content: it.text, attachments: it.atts, convs: it.convs, queued: true, ts: it.ts }));
    if (bubbles.length) updMsgs(key, (a) => [...a, ...bubbles]);
  }
  /// 出队一条：这一轮已经结束，把排在最前的排队消息发出去（发完它再发下一条）。
  function drainOutbox(key) {
    const q = outbox[key] || [];
    if (!q.length || streams[key]) return;
    const [item, ...rest] = q;
    outbox = { ...outbox, [key]: rest };
    deliver(key, item.text, item.atts, { bubbleId: item.id, convs: item.convs });
  }

  // —— 输入区草稿：按会话隔离，切会话/刷新/重开程序都不丢 ——
  // 存 localStorage（不是 sessionStorage）：关掉窗口再开，草稿也该还在。
  // 附件是 dataURL，可能很大：先试全量，超配额就退化成只存文字/引用并记下丢掉的附件数。
  const COMPOSER_KEY = 'pawchComposerDrafts';
  let composerNotices = {}; // key → 待补的通知（等这条会话的消息载入后再 push，避免顶掉历史）
  function composerMap() {
    try {
      const m = JSON.parse(localStorage.getItem(COMPOSER_KEY) || '{}');
      return m && typeof m === 'object' ? m : {};
    } catch {
      return {};
    }
  }
  function composerWrite(map) {
    try {
      localStorage.setItem(COMPOSER_KEY, JSON.stringify(map));
    } catch {
      /* 配额满：本次不落盘，内存里的草稿仍在 */
    }
  }
  /// 把当前输入区存到 key 名下；空输入 = 删除该会话的草稿。
  function persistComposer(key) {
    const map = composerMap();
    const entry = { text: input, atts: pendingAttachments, convs: pendingConvs, ts: Date.now() };
    const empty = !entry.text.trim() && !entry.atts.length && !entry.convs.length;
    if (empty) {
      if (map[key]) {
        delete map[key];
        composerWrite(map);
      }
      return;
    }
    try {
      localStorage.setItem(COMPOSER_KEY, JSON.stringify({ ...map, [key]: entry }));
    } catch {
      map[key] = { text: entry.text, atts: [], convs: entry.convs, dropped: entry.atts.length, ts: entry.ts };
      composerWrite(map);
    }
  }
  /// 载入 key 名下的草稿（没有就清空输入区）。附件被配额挤掉时留一句可见说明。
  function restoreComposer(key) {
    const map = composerMap();
    const e = map[key];
    input = e && typeof e.text === 'string' ? e.text : '';
    pendingAttachments = e && Array.isArray(e.atts) ? e.atts : [];
    pendingConvs = e && Array.isArray(e.convs) ? e.convs : [];
    if (e && e.dropped) {
      // 不能在这里直接 pushMsg：会话消息还没载入时 push 会建出一个「只有通知」的数组，
      // loadSession 见到 sessionMsgs[key] 已存在就不再从文件重建——历史会整个不显示。
      // 先挂起，等消息载入后由 syncCur 补上。
      composerNotices[key] = `[通知] 刷新前的 ${e.dropped} 个附件体积较大，未能随草稿保留；请重新添加`;
      delete map[key];
      composerWrite(map);
    }
    tick().then(() => resetComposerHeight());
  }
  /// 切换当前会话的唯一入口：赋值 + 广播（草稿跟着会话走：先存旧的，再载入新的）。
  function setCurSession(id, skipComposer = false) {
    const prev = currentSessionId || '@new';
    const next = id || '@new';
    if (!skipComposer && prev !== next) {
      persistComposer(prev);
      restoreComposer(next);
    }
    currentSessionId = id;
    // 本标签页「上次看的是哪条」——sessionStorage 刷新保留、重开软件/新标签页不继承，
    // 正好给「重新加载界面」兜底：整页导航会丢内存态，启动时靠它原地恢复（D6 期间
    // 真机反馈：左上角刷新按钮每次都会跳到最近一条对话）。null = 停在「新对话」。
    try {
      sessionStorage.setItem('curSession', id || '@new');
    } catch {}
    notifyExtsSession(id);
  }

  /// 写剪贴板：插件 iframe 是 sandbox 不透明源（非安全上下文），navigator.clipboard 与
  /// execCommand 都不归它使——复制一律到宿主页执行（localhost 是真源）。用户右键触发，
  /// 权限 clipboard 在 manifest 声明；navigator.clipboard 失败回落 execCommand。
  async function putClipboard(text) {
    try {
      await navigator.clipboard.writeText(text);
      return;
    } catch {
      /* 走回落 */
    }
    const ta = document.createElement('textarea');
    ta.value = text;
    ta.setAttribute('readonly', '');
    ta.style.position = 'fixed';
    ta.style.opacity = '0';
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand('copy');
    ta.remove();
    if (!ok) throw new Error('浏览器拒绝了剪贴板写入');
  }

  /// 扩展 → 宿主：查 UI_OPS 桥条目（bridge: true）分发。只读信息类不占权限白名单，
  /// 写操作按条目 perm 声明的 manifest 权限放行，没声明一律拒；写工作区文件等敏感动作
  /// 在条目里额外弹确认（宿主替扩展问）。新桥方法 = 表加一行，这里不再写分支。
  async function onExtMessage(ev) {
    const from = extWindowName(ev.source);
    if (!from) return; // 只认我们挂出去的扩展 iframe
    // 早到的桥调用常比插件清单先到（面板一拿到 init 就 call，宿主启动流水线还在
    // await）：空清单会把 perm 类方法误判「未声明权限」。等清单落地再判；
    // 从未触发或已失败时 promise 已落定，不卡。
    if (!extList.length && extListPromise) {
      try { await extListPromise; } catch (e) { /* 加载失败也照常判，别卡死桥 */ }
    }
    const ext = extList.find((e) => e.name === from) || {};
    if (ext.enabled === false) return; // 停用的扩展不服务
    const perms = ext.permissions || [];
    const d = ev.data || {};
    // 协议类型去前缀归一（改名迁移轮兼容一个版本：agent:* 与 pawch:* 都认）
    const protoBase = typeof d.type === 'string' ? d.type.replace(/^(agent|pawch):/, '') : '';
    if (protoBase === 'hello') {
      postToNameBoth(from, { type: 'pawch:init', payload: extInitPayload(ext) });
      return;
    }
    // 弹窗型插件的内容高度上报（通用协议，不占权限）：插件页把自身高度 post 过来，
    // 宿主让弹窗下边缘贴着内容——固定高度会在内容下方留一条空白（真机实测：
    // 「下边缘为什么不直接到确定取消刚好下面」）。夹在 [260, 视口-140] 防爆框。
    if (protoBase === 'resize' && extPopup && ev.source === extPopupFrame?.contentWindow) {
      const h = Number(d.height) || 0;
      if (h > 0) extPopupFrameH = Math.max(200, Math.min(h, window.innerHeight - 140));
      return;
    }
    // —— 微调桥（2026-11-03）：ack=支持微调（补下发已存规则），pick=监听态右击上报元素 ——
    if (protoBase === 'micro-ack') {
      extMicroCap = { ...extMicroCap, [from]: true };
      applyExtMicro(from); // 面板重载后规则回放
      return;
    }
    if (protoBase === 'micro-pick') {
      if (editMode) openExtMicroPanel(from, d.pick || {});
      return;
    }
    if (protoBase !== 'request') return;
    const id = d.id;
    // 回复按请求方的前缀回（agent:request ↔ agent:response，pawch: 同理）——消息按
    // type 配对，前缀错了插件侧 pending 永远等不到 reply
    const respType = String(d.type).replace(/:request$/, ':response');
    const reply = (ok, result) => postToName(from, { type: respType, id, ok, result });
    // 桥上的 GET 回传的必须是**解析后的 JSON**：fetch 的 Response 过不了 postMessage 的
    // 结构化克隆——真机原话 "Response object could not be cloned"，插件侧只看到这句天书，
    // 面板整个瘫掉（用量/价格/trace/工作区/git 六个只读方法全走这里）。
    // 旧版服务没有这些接口时请求落进 SPA 兜底返回 HTML，统一翻成「请重启服务」的人话。
    const get = (url) =>
      apiJson(url, { cache: 'no-store' }, '服务端没有这个接口，跑的还是旧版程序——请重启服务（先 cargo build --release 再 start.bat）');
    try {
      const op = UI_OPS[d.method];
      if (!op || !op.bridge) return reply(false, `未知方法: ${d.method}`);
      if (op.perm && !perms.includes(op.perm)) return reply(false, `未声明权限 ${op.perm}`);
      await op.run({ params: d.params || {}, from, ext, perms, reply, get });
    } catch (e) {
      reply(false, String(e.message || e));
    }
  }

  // —— 记忆库星云：每个光点 = 一次会话；搜索走向量检索 ——
  let galaxyOpen = false;
  // 入口被关时（插件开关联动写 features.galaxy，事件经 settings_changed 回来）弹层跟着收——
  // 否则侧栏按钮没了，星云弹层还开着，里面数据照样在跑，与「已停用」对不上。
  // 与上面 roles 页签的让位同一套路（features 是异步回来的，晚于首屏）。
  $: if (features && features.galaxy === false && galaxyOpen) galaxyOpen = false;
  function openSessionFromGalaxy(e) {
    galaxyOpen = false;
    loadSession(e.detail);
  }

  // trace 时间线已插件化：plugins/trace 面板 + 宿主只读桥 trace.recent，
  // 原内置弹窗（状态/函数/标记/样式）整体撤下——顶栏的插件按钮即入口。

  function nextId() {
    return ++seq;
  }
</script>

<svelte:window on:contextmenu={onMicroContext} on:click={onGlobalClick} />

{#if wallpaper}
  {#if wallpaperKind === 'video'}
    <!-- 视频背景：静音自动循环，定位与遮罩口径同图片壁纸。
         浅色主题淡出到 1/4：壁纸图多是按深色底挑的，浅色下全强度会压碎可读性
         （2026-11-04 浅色走查定案：浅色以可读为先，壁纸隐约透一点即可） -->
    <video class="wallpaper wallpaper-video" src={wallpaper} autoplay muted loop playsinline style="opacity:{(wallpaperOpacity / 100) * (theme === 'light' ? 0.25 : 1)}"></video>
  {:else}
    <div class="wallpaper" style="background-image:url('{wallpaper}'); opacity:{(wallpaperOpacity / 100) * (theme === 'light' ? 0.25 : 1)}"></div>
  {/if}
{/if}

<div
  class="app"
  on:dragenter={onDragEnter}
  on:dragover={onDragOver}
  on:dragleave={onDragLeave}
  on:drop={onDropZone}
>
  {#if dropHover}
    <!-- 拖文件进窗口时的投放提示：pointer-events:none，绝不挡住 drop 自身 -->
    <div class="drop-veil" aria-hidden="true">松开即可添加附件</div>
  {/if}
  <aside class="sidebar" class:collapsed={sidebarCollapsed} class:resizing={resizing} style="width:{sidebarW}px">
    <div class="side-head">
      <div class="brand-row">
        <span class="brand-mark" aria-hidden="true"><img src="/pawch-icon.svg" alt="" /></span>
        <span class="brand-name">Pawch Agent</span>
        <button class="icon-btn" title="收起侧栏" on:click={() => (sidebarCollapsed = true)}>
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><rect x="3" y="4.5" width="18" height="15" rx="3.2"/><line x1="9.8" y1="4.5" x2="9.8" y2="19.5"/></svg>
        </button>
        <!-- 重新加载界面（铁律 1 配套，2026-10-31）：前端换新构建后点一下丢旧代码。
             带时间戳强制拉最新入口（assets 带 hash，不受缓存影响）。 -->
        <button class="icon-btn" title="重新加载界面（前端更新后点这个）" on:click={() => (location.href = location.pathname + '?r=' + Date.now())}>
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M20 11a8 8 0 1 0-2.3 6"/><path d="M20 5v6h-6"/></svg>
        </button>
      </div>

      <button class="new-chat" on:click={startNewSession}>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><circle cx="12" cy="12" r="9"/><path d="M12 8v8M8 12h8" stroke-linecap="round"/></svg>
        新对话
      </button>

      <div class="section-row bar-menu">
        <span class="section-label">工作区</span>
        <span class="section-tools">
          <button class="icon-btn" class:on={searchOpen} title="搜索会话" on:click={toggleSearch}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><circle cx="11" cy="11" r="7"/><line x1="21" y1="21" x2="16.3" y2="16.3"/></svg>
          </button>
          <button class="icon-btn" class:on={sideMenuOpen || manageMode} title="视图与会话管理" on:click={() => (sideMenuOpen = !sideMenuOpen)}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><line x1="4" y1="7.5" x2="20" y2="7.5"/><circle cx="9.5" cy="7.5" r="2.2"/><line x1="4" y1="16.5" x2="20" y2="16.5"/><circle cx="14.5" cy="16.5" r="2.2"/></svg>
          </button>
          <button class="icon-btn" title="添加项目" on:click={addProject}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M20 11.2v5.8a2 2 0 0 1-2 2H5.5a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2h3.2l1.8 2H18a2 2 0 0 1 2 2v.2"/><line x1="17" y1="2.5" x2="17" y2="8"/><line x1="14.2" y1="5.2" x2="19.8" y2="5.2"/></svg>
          </button>
        </span>
        {#if sideMenuOpen}
          <div class="side-menu view-menu">
            <button on:click={() => pickView('workspace')}>
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"><path d="M20 19.5a1.8 1.8 0 0 0 1.8-1.8V8.3a1.8 1.8 0 0 0-1.8-1.8h-7.4a1.8 1.8 0 0 1-1.5-.8l-.8-1.2a1.8 1.8 0 0 0-1.5-.8H4a1.8 1.8 0 0 0-1.8 1.8v12.2A1.8 1.8 0 0 0 4 19.5Z"/></svg>
              <span>按工作区分组</span>
              {#if sidebarView === 'workspace'}<svg class="sel-check" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg>{/if}
            </button>
            <button on:click={() => pickView('list')}>
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><line x1="4" y1="6.5" x2="20" y2="6.5"/><line x1="4" y1="12" x2="20" y2="12"/><line x1="4" y1="17.5" x2="20" y2="17.5"/></svg>
              <span>单列表</span>
              {#if sidebarView === 'list'}<svg class="sel-check" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg>{/if}
            </button>
            <div class="menu-sep"></div>
            <button on:click={enterManage}>
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="3.5" y="4" width="7" height="7" rx="1.5"/><path d="m5.2 7.6 1.4 1.4 2.4-2.6"/><rect x="13.5" y="4" width="7" height="7" rx="1.5"/><rect x="3.5" y="13.5" width="7" height="7" rx="1.5"/><rect x="13.5" y="13.5" width="7" height="7" rx="1.5"/></svg>
              <span>会话管理</span>
            </button>
          </div>
        {/if}
      </div>

      {#if searchOpen}
        <input class="side-search" bind:value={searchQuery} placeholder="搜索会话" />
      {/if}

      {#if manageMode}
        <div class="manage-bar">
          <span class="m-count">已选 {checkedIds.length} / {manageTotal}</span>
          <button class="m-btn" on:click={() => (checkedIds = manageVisibleIds())}>全选</button>
          <button class="m-btn danger" disabled={!checkedIds.length} on:click={batchDelete}>删除所选</button>
          <button class="m-btn primary" on:click={exitManage}>完成</button>
        </div>
      {/if}
    </div>

    <div class="side-scroll">
      {#each displayRows as row}
        {#if row.type === 'proj'}
          <div
            class="proj"
            class:expanded={!!searchQ || !!expandedFlags[row.path]}
            class:active={row.active}
            role="button"
            tabindex="0"
            title={row.path === '__ungrouped__' ? '无工作区记录的历史会话' : row.path === '__legacy__' ? '以前同名文件夹留下的旧会话（文件夹删掉重建后归到这里）' : `${row.path}（点行切换项目，点左侧图标展开/收起列表）`}
            on:click={() => ((row.path === '__ungrouped__' || row.path === '__legacy__' || row.active) ? toggleProjectRow(row.path) : switchProject(row.path))}
            on:keydown={(e) => e.key === 'Enter' && ((row.path === '__ungrouped__' || row.path === '__legacy__' || row.active) ? toggleProjectRow(row.path) : switchProject(row.path))}
          >
            <!-- 展开/收起：独立按钮，任何项目都能点开看会话（点行本身仍是切换项目） -->
            <button
              class="proj-toggle"
              title={!!searchQ || !!expandedFlags[row.path] ? '收起会话列表' : '展开会话列表'}
              on:click|stopPropagation={() => toggleProjectRow(row.path)}
            >
              {#if !!searchQ || !!expandedFlags[row.path]}
                <svg viewBox="0 0 24 24"><path d="M6.5 9.2 12 14.7l5.5-5.5z" fill="currentColor"/></svg>
              {:else}
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"><path d="M20 19.5a1.8 1.8 0 0 0 1.8-1.8V8.3a1.8 1.8 0 0 0-1.8-1.8h-7.4a1.8 1.8 0 0 1-1.5-.8l-.8-1.2a1.8 1.8 0 0 0-1.5-.8H4a1.8 1.8 0 0 0-1.8 1.8v12.2A1.8 1.8 0 0 0 4 19.5Z"/></svg>
              {/if}
            </button>
            <span class="proj-name">{row.name}{row.count ? '' : ''}</span>
            {#if row.running}<span class="conv-spin" title="这个项目里有会话正在跑" aria-label="有会话正在运行"></span>{/if}
            {#if row.active}<span class="proj-cur" title="当前项目"></span>{/if}
            {#if row.path === '__legacy__'}
              <span class="proj-side">
                <button class="row-btn" title="清空这些历史会话" on:click|stopPropagation={clearLegacySessions}>
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6.5h18"/><path d="M8 6.5V4.8A1.3 1.3 0 0 1 9.3 3.5h5.4A1.3 1.3 0 0 1 16 4.8v1.7"/><path d="M5.5 6.5 6.4 19a2 2 0 0 0 2 1.9h7.2a2 2 0 0 0 2-1.9l.9-12.5"/></svg>
                </button>
              </span>
            {:else if row.path !== '__ungrouped__'}
              <span class="proj-side">
                <button class="row-btn" class:on={projMenuPath === row.path} title="项目操作" on:click|stopPropagation={() => (projMenuPath = projMenuPath === row.path ? null : row.path)}>
                  <svg viewBox="0 0 24 24" fill="currentColor"><circle cx="5.5" cy="12" r="1.5"/><circle cx="12" cy="12" r="1.5"/><circle cx="18.5" cy="12" r="1.5"/></svg>
                </button>
                <button class="row-btn" title="在项目内新建会话" on:click|stopPropagation={() => newSessionInProject(row.path, row.active)}>
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M12 5v14M5 12h14"/></svg>
                </button>
              </span>
            {/if}
            {#if projMenuPath === row.path}
              <div class="side-menu proj-menu">
                <button on:click|stopPropagation={() => { projMenuPath = null; renameModal = { kind: 'project', target: row.path, value: row.name }; }}>
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M16.8 3.7a2.1 2.1 0 0 1 3 3L7.3 19.2 3 20.3l1.1-4.3Z"/><path d="M13.5 20h7"/></svg>
                  重命名项目
                </button>
                {#if row.active}
                  <!-- 多文件夹项目（2026-11-04）：只有当前项目能改文件夹（端点按当前工作区生效） -->
                  <button on:click|stopPropagation={() => { projMenuPath = null; foldersMsg = ''; foldersModal = true; }}>
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"><path d="M3.5 7.2c0-1 .8-1.7 1.7-1.7h4l1.9 2.2h8.2c1 0 1.7.8 1.7 1.7v8.4c0 1-.8 1.7-1.7 1.7H5.2c-1 0-1.7-.8-1.7-1.7Z"/><path d="M3.5 10.5h17"/></svg>
                    项目文件夹{workspaceFolders.length ? `（${workspaceFolders.length}）` : ''}
                  </button>
                {/if}
                <button on:click|stopPropagation={() => removeProject(row.path, row.name)}>
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6.5h18"/><path d="M8 6.5V4.8A1.3 1.3 0 0 1 9.3 3.5h5.4A1.3 1.3 0 0 1 16 4.8v1.7"/><path d="M5.5 6.5 6.4 19a2 2 0 0 0 2 1.9h7.2a2 2 0 0 0 2-1.9l.9-12.5"/></svg>
                  删除项目
                </button>
              </div>
            {/if}
          </div>
        {:else if row.type === 'conv'}
          {@const s = row.s}
          <div
            class="conv"
            class:active={s.id === currentSessionId}
            role="button"
            tabindex="0"
            on:click={() => (manageMode ? toggleChecked(s.id) : loadSession(s.id))}
            on:keydown={(e) => e.key === 'Enter' && (manageMode ? toggleChecked(s.id) : loadSession(s.id))}
          >
            {#if manageMode}
              <span class="conv-check" class:on={checkedIds.includes(s.id)}><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg></span>
            {/if}
            {#if runningIds.has(s.id)}
              <!-- 运行中：不管本页有没有这条会话的流（切走的/别的窗口起的）都转圈 -->
              <span class="conv-spin" title="正在运行" aria-label="正在运行"></span>
            {/if}
            <span class="conv-title" title={s.title}>{s.title}</span>
            {#if !manageMode && s.pinned}
              <!-- 置顶标记：标准图钉，放标题右侧（放最左会把标题整体挤开） -->
              <svg class="conv-pin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-label="已置顶"><path d="M12 17v5"/><path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z"/></svg>
            {/if}
            {#if !manageMode}
              <button class="conv-more" title="会话操作" on:click|stopPropagation={() => (sessMenuId = sessMenuId === s.id ? null : s.id)}>
                <svg viewBox="0 0 24 24" fill="currentColor"><circle cx="5.5" cy="12" r="1.6"/><circle cx="12" cy="12" r="1.6"/><circle cx="18.5" cy="12" r="1.6"/></svg>
              </button>
            {/if}
            <span class="conv-time">{relShort(s.updated_at)}</span>
            {#if sessMenuId === s.id}
              <div class="side-menu sess-menu">
                <button on:click|stopPropagation={() => togglePin(s)}>
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 17v5"/><path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z"/></svg>
                  {s.pinned ? '取消置顶' : '置顶'}
                </button>
                <button on:click|stopPropagation={() => { sessMenuId = null; renameModal = { kind: 'session', target: s.id, value: s.title, orig: s.title }; }}>
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M16.8 3.7a2.1 2.1 0 0 1 3 3L7.3 19.2 3 20.3l1.1-4.3Z"/><path d="M13.5 20h7"/></svg>
                  重命名
                </button>
                <button on:click|stopPropagation={() => forkSession(s.id)}>
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M5 6.5h11"/><circle cx="18.3" cy="6.5" r="1.7"/><path d="M5 6.5c5.5 0 3.5 11 9.5 11"/><circle cx="18.3" cy="17.5" r="1.7"/></svg>
                  分叉会话
                </button>
                <button on:click|stopPropagation={() => deleteSessionRow(s)}>
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18"/><path d="M8 6V4h8v2"/><path d="M19 6l-1 14H6L5 6"/><path d="M10 11v6"/><path d="M14 11v6"/></svg>
                  删除会话
                </button>
              </div>
            {/if}
          </div>
        {:else if row.type === 'more'}
          <button class="expand-more" on:click={() => (showAllFlags = { ...showAllFlags, [row.path]: true })}>
            展开其余 {row.rest} 个会话
          </button>
        {:else}
          <div class="side-empty">{searchQ ? '没有匹配的会话' : '暂无会话，点上方「新对话」开始'}</div>
        {/if}
      {/each}
    </div>

    <div class="side-foot">
      {#if features.galaxy}
      <button class="foot-btn galaxy-btn" on:click={() => (galaxyOpen = true)} title="记忆库：以星云看每一次会话，支持语义搜索">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="1.6" fill="currentColor" stroke="none"/><ellipse cx="12" cy="12" rx="9.2" ry="3.6" transform="rotate(-24 12 12)"/><circle cx="19.4" cy="8.6" r="1.1" fill="currentColor" stroke="none"/><circle cx="5.2" cy="15.2" r="0.9" fill="currentColor" stroke="none"/></svg>
        记忆库
      </button>
      {/if}
      <button class="foot-btn" on:click={() => (settingsOpen = true)}>设置</button>
      {#if appVersion}
        <div class="side-version" title={`运行根：${appVersion.root}`}>v{appVersion.version}</div>
      {/if}
    </div>
    <div class="resize-handle" on:pointerdown={startResize}></div>
  </aside>

  <main class="chat" class:side-collapsed={sidebarCollapsed}>
    {#if sidebarCollapsed}
      <button class="sidebar-restore" title="展开侧栏" on:click={() => (sidebarCollapsed = false)}>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><rect x="3" y="4.5" width="18" height="15" rx="3.2"/><line x1="9.8" y1="4.5" x2="9.8" y2="19.5"/></svg>
      </button>
    {/if}
    <header class="chat-head" data-tauri-drag-region>
      <div class="head-main">
        <div class="chat-title">{displayTitle}</div>
      </div>
      <div class="head-side">
        <!-- 顶栏只留「终端 + 右侧边栏」：插件面板（时间线/项目速览/
             用量）不再放顶栏按钮，全在右侧边栏以标签页打开（标签栏 + 号添加）。 -->
        <button
          class="head-btn"
          class:active={terminalOpen}
          title="终端（沙箱内执行命令）"
          on:click={() => {
            terminalOpen = !terminalOpen;
            if (terminalOpen) rightPanelOpen = false;
          }}
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4.5" width="18" height="15" rx="3.2"/><path d="m7.5 9.5 3.2 2.8-3.2 2.8"/><path d="M12.8 15.4h4"/></svg>
        </button>
        <button
          class="head-btn"
          class:active={rightPanelOpen}
          title="侧边面板（产物/网页/时间线/项目速览/用量…）"
          on:click={() => {
            rightPanelOpen = !rightPanelOpen;
            if (rightPanelOpen) {
              terminalOpen = false;
              openRightPanel();
            }
          }}
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round"><rect x="3" y="4.5" width="18" height="15" rx="3.2"/><path d="M14.5 4.5v15"/><path d="M17.5 9.5h1M17.5 12.5h1" stroke-linecap="round"/></svg>
        </button>

        <!-- 窗口控制（无边框壳：原生黑条撤掉，按钮做进内容行）。
             只有桌面壳里才有 agentWinApi（壳每 2s 注入）；浏览器打开的第二个客户端不显示。
             侧栏展开时按钮在侧栏右端（见 brand 行），这里只兜收起态 -->
        {#if winApi && !rightPanelOpen}
          <div class="win-ctrls">
            <button class="head-btn win-btn" title="最小化" aria-label="最小化" on:click={() => winApi.minimize()}>
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M6 12h12"/></svg>
            </button>
            <button class="head-btn win-btn" title={winMax ? '还原窗口' : '最大化'} aria-label={winMax ? '还原窗口' : '最大化'} on:click={() => winApi.toggleMax().then(refreshWinMax, refreshWinMax)}>
              {#if winMax}
                <!-- 全屏态：两个叠放矩形 = 还原 -->
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="4.5" y="7.5" width="12" height="12" rx="2"/><path d="M8 7.5V6a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2h-1.5"/></svg>
              {:else}
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"><rect x="5.5" y="5.5" width="13" height="13" rx="2"/></svg>
              {/if}
            </button>
            <button class="head-btn win-btn win-close" title="关闭到托盘（服务保持运行）" aria-label="关闭到托盘" on:click={() => winApi.hide()}>
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M6.5 6.5l11 11M17.5 6.5l-11 11"/></svg>
            </button>
          </div>
        {/if}

      </div>
    </header>

    <!-- 无边框窗边缘拉伸热区（Windows 无原生 resize 边框）：6px 四边+四角，按下起拖 -->
    {#if winApi}
      <div class="win-rz win-rz-n" on:mousedown={() => winApi.startResize('North')} title=""></div>
      <div class="win-rz win-rz-s" on:mousedown={() => winApi.startResize('South')} title=""></div>
      <div class="win-rz win-rz-w" on:mousedown={() => winApi.startResize('West')} title=""></div>
      <div class="win-rz win-rz-e" on:mousedown={() => winApi.startResize('East')} title=""></div>
      <div class="win-rz win-rz-nw" on:mousedown={() => winApi.startResize('NorthWest')} title=""></div>
      <div class="win-rz win-rz-ne" on:mousedown={() => winApi.startResize('NorthEast')} title=""></div>
      <div class="win-rz win-rz-sw" on:mousedown={() => winApi.startResize('SouthWest')} title=""></div>
      <div class="win-rz win-rz-se" on:mousedown={() => winApi.startResize('SouthEast')} title=""></div>
    {/if}

    <div class="chat-main">
      <div class="chat-body" bind:this={chatBody} on:scroll={onChatScroll}>
      {#if startupError}
        <!-- 启动失败必须可见：服务未就绪时不许只留一个空对话（2026-10-27 实锤） -->
        <div class="msg error-row startup-error">
          <div class="error-bubble">{startupError}</div>
          <button class="btn ghost" on:click={() => location.reload()}>重试</button>
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
          <div class="msg user" class:selecting={msgSelectMode} on:click={() => selectRow(m)}>
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
                <div class="edit-actions">
                  <button class="btn primary" on:click={saveEdit}>保存并重发</button>
                  <button class="btn ghost" on:click={() => (editingMsg = null)}>取消</button>
                  <span class="edit-hint">保存会先停止正在跑的输出</span>
                </div>
              </div>
            {:else}
              <div class="bubble user-bubble">
              {#if m.attachments && m.attachments.length > 0}
                <div class="msg-attachments">
                  {#each m.attachments as a}
                    {#if a.mime && a.mime.startsWith('image/')}
                      <img
                        class="msg-att-img"
                        src={a.data}
                        alt={a.name}
                        title="点击查看原图"
                        on:click={() => (previewImageModal = a.data)}
                      />
                    {:else if a.mime && a.mime.startsWith('video/')}
                      <video class="msg-att-video" controls src={a.data}>
                        <track kind="captions" />
                      </video>
                    {:else if a.mime && a.mime.startsWith('audio/')}
                      <audio class="msg-att-audio" controls src={a.data}></audio>
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
                <!-- 引用对话只在气泡上挂 chip（正文发送时已整块拼给模型，不上屏刷屏） -->
                <div class="msg-convs">
                  {#each m.convs as cv}
                    <span class="conv-tag" title={`已把对话「${cv.title}」作为上下文发给模型`}>对话 · {cv.title}</span>
                  {/each}
                </div>
              {/if}
              {#if m.queued}
                <div class="queued-row">
                  <span class="queued-badge">排队中 · 这一轮结束后自动发送</span>
                  <button class="queued-send" on:click={() => sendNow(m.id, 'interrupt')} title="中断当前操作（工具/回答让位），新指令立即生效">立即发送</button>
                  <button class="queued-send" on:click={() => sendNow(m.id, 'steer')} title="不打断：跑完当前这步就并入本轮。静默生效，不占对话气泡">引导</button>
                </div>
              {:else if m.steered}
                <div class="queued-row"><span class="queued-badge steered">已引导当前回答</span></div>
              {:else if m.interrupted}
                <div class="queued-row"><span class="queued-badge steered">已中断当前操作并立即发送</span></div>
              {/if}
            </div>
            {#if !msgSelectMode && !m.queued}
              <!-- 操作条在消息下方：时间贴行边、功能贴另一边；图标+tooltip，不占正文 -->
              <div class="msg-meta user-meta">
                {#if m.ts}<span class="msg-ts">{fmtTs(m.ts)}</span>{/if}
                <span class="msg-acts" on:click|stopPropagation>
                  <button class="msg-act" title="回滚：文件恢复到这条消息之前，并删掉这条和它之后的所有消息" aria-label="回滚" on:click|stopPropagation={() => rollbackMsg(m)}>
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 12a9 9 0 1 0 3-6.7"/><path d="M3 4v5h5"/></svg>
                  </button>
                  <button class="msg-act" title="编辑并重发（会先停止正在跑的输出）" aria-label="编辑" on:click|stopPropagation={() => beginEdit(m)}>
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z"/></svg>
                  </button>
                  <button class="msg-act" title="删除这条消息（同一问答的另一半保留；不可恢复）" aria-label="删除" on:click|stopPropagation={() => deleteMsg(m)}>
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18"/><path d="M8 6V4h8v2"/><path d="M19 6l-1 14H6L5 6"/><path d="M10 11v6"/><path d="M14 11v6"/></svg>
                  </button>
                  <button class="msg-act" title="多选消息：批量删除或生成上下文摘录" aria-label="多选" on:click|stopPropagation={enterSelect}>
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
          <div class="msg assistant" class:selecting={msgSelectMode} on:click={() => selectRow(m)}>
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
                   流式期间过程默认展开；收尾后默认收起，只留总结与结尾语（用户 2026-10-02）。 -->
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
                                  use:thinkFollow={m.streaming && !it.done}
                                  bind:this={thinkBoxEls[ik]}
                                  on:followchange={(e) => (thinkFollowMap = { ...thinkFollowMap, [ik]: e.detail.following })}
                                >{it.text}</div>
                                {#if thinkFollowMap[ik] === false}
                                  <button class="think-jump" title="回到底部（继续跟随）" on:click={() => jumpThink(ik)}>
                                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 5v13"/><path d="m6 12 6 6 6-6"/></svg>
                                  </button>
                                {/if}
                              {/if}
                            </div>
                          {:else if it.type === 'tool'}
                            {@const tm = toolMeta(it.name, it.args)}
                            <!-- 直接读 toolFold（Svelte 只对模板里直接引用的变量建依赖；
                                 包进函数读不到变更——2026-10-26 真机实锤过同款坑） -->
                            {@const tOpen = toolFold[`${m.id}:${it.toolIndex}`] === true}
                            <div class="tool-item">
                              <button
                                class="tool-row"
                                on:click={() => toggleToolDetail(m.id, it.toolIndex, tOpen)}
                                title={tOpen ? '收起详情' : '展开参数与结果'}
                              >
                                <span class="tool-ico" class:running={it.status === 'running'}>{@html tm.iconSvg}</span>
                                <span class="tool-name">{tm.label}</span>
                                {#if tm.subject}<span class="tool-subject" title={tm.subject}>{tm.subject}</span>{/if}
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
                                      <pre class="tool-pre">{@html highlightText(prettyArgs(it.args), 'json')}</pre>
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
                            <div class="content process-note" on:click={onContentClick}>{@html md(it.text)}</div>
                          {/if}
                        {/each}
                      </div>
                    {/if}
                  </div>
                {:else if b.text}
                  <!-- doc-cite：agent 回复里 /api/workspace/... 的文件引用，点击落到右侧预览标签 -->
                  <div class="content" on:click={onContentClick}>{@html md(b.text)}</div>
                {/if}
              {/each}
              {#if m.streaming && !messageParts(m).length}
                <div class="typing"><span class="typing-dot"></span><span class="typing-dot"></span><span class="typing-dot"></span></div>
              {/if}
            </div>
            {#if m.ts || (m.stats && !m.streaming)}
            <div class="msg-meta">
              {#if m.ts}<span class="msg-ts">{fmtTs(m.ts)}</span>{/if}
              {#if m.stats && !m.streaming}
                <!-- 信息卡入口：首 token 延迟/输出耗时/渠道/模型/tokens/缓存命中。
                     放回答下方右下角（用户 2026-10-27 点名布局） -->
                <span class="msg-acts" on:click|stopPropagation>
                  <button class="msg-act" title="这条回复的详细信息" aria-label="回答详情" on:click|stopPropagation={() => (statsModal = m)}>
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><circle cx="12" cy="12" r="9"/><path d="M12 11v5"/><path d="M12 8h.01"/></svg>
                  </button>
                </span>
              {/if}
            </div>
            {/if}
            </div>
          </div>
        {:else if m.role === 'error'}
          <!-- 报错行：与重启后落盘恢复的 [错误] 系统消息同款灰字（
               实时红气泡和重启后灰字两副面孔太突兀，统一成后者）。
               落盘侧已带 [错误] 前缀（server 落 [错误] {e}），实时侧这里补齐前缀 -->
          <div class="msg system"><div class="sys-text">[错误] {m.content}</div></div>
        {:else if m.role === 'cp'}
          <!-- 检查点消息只给改动条提供基线（dockCp），不渲染 -->
        {:else}
          <div class="msg system"><div class="sys-text">{m.content ?? ''}</div></div>
        {/if}
        {/each}
        </div>
      {/each}
    </div>

    {#if !chatPinned}
      <button class="jump-bottom" on:click={scrollToBottom} title="回到底部">回到底部</button>
    {/if}

    <footer class="chat-input" bind:this={chatInputEl}>
      <!-- 多选操作条：必须放 footer 内部（输入框是绝对定位浮层，放普通流里整条被
           盖住、按钮看得见点不着——subagent-strip 同款坑，2026-10-26 实测抓到） -->
      {#if msgSelectMode}
        <div class="select-bar">
          <span>已选 {msgSelected.length} 条</span>
          <button class="btn" disabled={!msgSelected.length} on:click={deleteSelected}>删除所选</button>
          <button class="btn" disabled={!msgSelected.length} on:click={copySelectedAsContext} title="把选中的消息拼成上下文摘录复制到剪贴板，去别的对话粘贴即可">复制</button>
          {#if ctxCopied}<span class="select-copied">已复制，去别的对话粘贴即可</span>{/if}
          <span class="select-flex"></span>
          <button class="btn ghost" on:click={exitSelect}>退出多选</button>
        </div>
      {/if}
      <!-- 子代理实时条（主区、输入框上方；不落侧边栏）。必须放在 footer 里：
           输入框是 position:absolute 浮层，放它外面（普通流）会被整个盖住（实测 y 重叠
           82px，命中测试全落到 composer 上）。footer 子元素自带 pointer-events:auto。
           只在有排队/执行中的派发时占位，终态即撤——回看走「查看全部」小弹窗 -->
      {#if subagentActive.length}
        <div class="subagent-strip">
          <div class="subagent-strip-head">
            <span class="subagent-strip-title">子代理 · {subagentActive.length} 个在跑</span>
            <button class="subagent-strip-more" on:click={openSubagentPanel}>查看全部</button>
          </div>
          {#each subagentActive as d (d.dispatch)}
            <div class="subagent-row">
              <span class="subagent-badge {d.status}">{d.status === 'queued' ? '排队中' : '执行中'}</span>
              <span class="subagent-label" title={d.task}>{d.label}</span>
              <button class="subagent-row-cancel" title="取消这个子代理" on:click={() => cancelSubagent(d.dispatch)}>×</button>
            </div>
          {/each}
        </div>
      {:else if subagentPanelRows.length}
        <!-- 本会话派发过子代理（都跑完了）：留一颗不占地的小 chip，想回看就点 -->
        <div class="subagent-chiprow">
          <button class="subagent-chip" on:click={openSubagentPanel}>子代理 ×{subagentPanelRows.length}</button>
        </div>
      {/if}
      <!-- 操作电脑实时条（第 4 轮；同子代理纪律：主区、输入框上方、不落侧边栏）。
           只在新动作后 5 秒内占位；之后撤成 chip（有历史截图/动作就能开面板回看） -->
      {#if computerOnUi && computerFresh && computerLive.action}
        <div class="computer-strip">
          <span class="computer-badge">操作电脑</span>
          <span class="computer-action" title={computerLive.action}>{computerLive.action}</span>
          <button class="computer-strip-more" on:click={openComputerPanel}>查看屏幕</button>
        </div>
      {:else if computerOnUi && !computerChipDismissed && (computerLive.action || computerState.last_screenshot)}
        <div class="subagent-chiprow">
          <!-- 一颗可收起的 chip：左边点开面板，右边 × 收起（收起后不再出现） -->
          <span class="chip-pill">
            <button class="subagent-chip" on:click={openComputerPanel}>电脑操作 ×{computerState.actions.length || 1} · 查看屏幕</button>
            <button class="chip-x" title="收起（不再出现）" aria-label="收起" on:click={dismissComputerChip}>×</button>
          </span>
        </div>
      {/if}
      <div class="composer">
        {#if todoDockVisible}
          <!-- 任务清单 dock：贴输入框上方、展开向上长；
               与审批/提问上下堆叠——清单在最上，阻塞交互的提问/审批贴输入框 -->
          <div class="todo-dock">
            <div class="todo-dock-row">
              <button
                class="todo-head"
                on:click={toggleTodoDock}
                title={todoDockOpen ? '收起任务清单' : '展开任务清单'}
              >
                <svg class="todo-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M9 6h11M9 12h11M9 18h11"/><path d="m3.5 5.5 1.5 1.5 3-3"/><path d="m3.5 11.5 1.5 1.5 3-3"/><circle cx="5" cy="18" r="1.1" fill="currentColor" stroke="none"/></svg>
                <span class="todo-title">任务清单</span>
                {#if !todoDockOpen}<span class="todo-cur" title={todoCur.content}>{todoCur.content}</span>{/if}
                <span class="todo-count">{todoDoneN}/{currentTodo.todos.length}</span>
                <svg class="todo-caret" class:open={todoDockOpen} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6"/></svg>
              </button>
              <button class="todo-x" on:click={dismissTodoDock} title="收起任务清单（本任务内不再出现）" aria-label="收起任务清单">×</button>
            </div>
            {#if todoDockOpen}
              <div class="todo-body">
                {#each currentTodo.todos as t, ti}
                  <div class="todo-item {t.status}">
                    <span class="todo-mark">
                      {#if t.status === 'completed'}<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="m8.5 12.5 2.5 2.5 4.5-5"/></svg>
                      {:else if t.status === 'in_progress'}<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12h13"/><path d="m13 7 5 5-5 5"/></svg>
                      {:else}<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><circle cx="12" cy="12" r="9"/></svg>{/if}
                    </span>
                    <span class="todo-text">{ti + 1}. {t.content}</span>
                  </div>
                {/each}
              </div>
            {/if}
          </div>
        {/if}
        {#if pendingApprovals.length > 0}
          <!-- 执行询问贴在输入框上方，不钉在对话顶部；
               决定后记录回到时间线的工具卡 -->
          <div class="approval-dock">
            {#each pendingApprovals as pa}
              <div class="approval-box">
                <div class="approval-head">
                  <span class="approval-title"
                    ><span class="approval-kicker">审批请求</span><b>{pa.tool}</b>
                    <span class="approval-risk risk-{pa.risk}"
                      >{pa.riskLabel || riskLabelFor(pa.risk)}</span
                    ></span
                  >
                  {#if pa.reason}<span class="approval-reason">{pa.reason}</span>{/if}
                  <span class="tool-args" title={pa.args}>{pa.args}</span>
                </div>
                <div class="approval-actions">
                  <button class="btn" on:click={() => decideApproval(pa.id, true)}>允许</button>
                  <button class="btn ghost" on:click={() => decideApproval(pa.id, true, true)}
                    >同类不再问</button
                  >
                  <button class="btn ghost" on:click={() => decideApproval(pa.id, false)}>拒绝</button>
                  <span class="approval-hint">等你的决定，120 秒不选自动拒绝</span>
                </div>
              </div>
            {/each}
          </div>
        {/if}
        {#if pendingQuestions.length > 0}
          <!-- 阻塞式提问（ask 工具）：与审批同一个槽位、同一主题；提问是方案决策不是权限闸 -->
          <div class="approval-dock">
            {#each pendingQuestions as q (q.id)}
              {@const ui = askUi[q.id] || { picked: [], other: '' }}
              <div class="approval-box ask-box">
                <div class="approval-head">
                  <span class="approval-title"
                    ><span class="approval-kicker">{q.header || '提问'}</span
                    >{#if q.multiSelect}<span class="ask-multi-tag">可多选</span>{/if}</span
                  >
                  <b class="ask-question">{q.question}</b>
                </div>
                <div class="ask-options">
                  {#each q.options as o}
                    <button
                      class="ask-opt"
                      class:sel={ui.picked.includes(o.label)}
                      on:click={() => pickAskOption(q, o.label)}
                      title={o.description}
                    >
                      <span class="ask-opt-label">{o.label}</span>
                      {#if o.description}<span class="ask-opt-desc">{o.description}</span>{/if}
                    </button>
                  {/each}
                </div>
                <div class="ask-other">
                  <input
                    class="ask-other-in"
                    placeholder="其他（自由输入）"
                    value={ui.other}
                    on:input={(e) => setAskUi(q.id, { other: e.target.value })}
                    on:keydown={(e) => { if (e.isComposing) return; if (e.key === 'Enter') submitAskOther(q); }}
                  />
                  {#if q.multiSelect}
                    <button class="btn" disabled={!ui.picked.length} on:click={() => answerAsk(q, { selected: [...ui.picked] })}>确认所选</button>
                  {/if}
                  <button class="btn" disabled={!ui.other.trim()} on:click={() => submitAskOther(q)}>提交</button>
                  <button class="btn ghost" on:click={() => answerAsk(q, { dismissed: true })}>跳过</button>
                </div>
                <span class="approval-hint">回答前这一轮会挂起等你；跳过 = 让它自己拿主意</span>
              </div>
            {/each}
          </div>
        {/if}
        {#if pendingAttachments.length > 0}
          <div class="composer-attachments">
            {#each pendingAttachments as att, idx}
              <div class="att-chip" title={`${att.name} (${fmtBytes(att.size)})`}>
                {#if att.mime.startsWith('image/')}
                  <img class="att-thumb" src={att.data} alt={att.name} />
                {:else if att.mime.startsWith('video/')}
                  <svg class="att-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><rect x="3" y="5.5" width="18" height="13" rx="2.5"/><path d="M3 9.5h18M7.5 5.5v4M16.5 5.5v4"/></svg>
                {:else if att.mime.startsWith('audio/')}
                  <svg class="att-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><circle cx="7.5" cy="17.5" r="2.5"/><circle cx="17.5" cy="15.5" r="2.5"/><path d="M10 17.5V6l10-2.5V15.5"/></svg>
                {:else}
                  <svg class="att-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="m8 12.5 6.5-6.5a2.8 2.8 0 0 1 4 4L9 19.5a4.5 4.5 0 0 1-6.4-6.4l8.5-8.5"/></svg>
                {/if}
                <span class="att-name">{att.name}</span>
                <span class="att-size">{fmtBytes(att.size)}</span>
                <button class="att-del" on:click={() => removeAttachment(idx)} title="移除" aria-label="移除"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M6 6l12 12M18 6 6 18"/></svg></button>
              </div>
            {/each}
          </div>
        {/if}
        {#if pendingConvs.length > 0}
          <div class="composer-attachments">
            {#each pendingConvs as cv}
              <div class="att-chip conv-chip" title={`引用对话：${cv.title}（发送时拼进消息正文${cv.omitted ? '，只保留最近部分' : ''}）`}>
                <svg class="att-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M21 11.5a8.4 8.4 0 0 1-9 8.4 8.9 8.9 0 0 1-3.8-.8L3 21l1.9-4.6a8.4 8.4 0 1 1 16.1-4.9"/><path d="M8.5 10.5h7M8.5 14h4.5"/></svg>
                <span class="att-name">{cv.title}</span>
                <button class="att-del" on:click={() => removePendingConv(cv.id)} title="移除" aria-label="移除"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M6 6l12 12M18 6 6 18"/></svg></button>
              </div>
            {/each}
          </div>
        {/if}
        {#if (guided[curKey()] || []).length > 0}
          <!-- 已引导标记（2026-10-22）：静默提示,不占对话气泡位;本轮结束自动消失 -->
          <div class="guided-row">
            {#each guided[curKey()] as g, gi}
              <span class="guided-chip" title={`不打断当前操作,跑完这步就并入:${g.text}`}>
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M22 2 11 13"/><path d="M22 2 15 22l-4-9-9-4Z"/></svg>
                已引导 · {g.text.length > 40 ? g.text.slice(0, 40) + '…' : g.text}
                <button
                  class="guided-x"
                  title="移除这条提示"
                  aria-label="移除这条提示"
                  on:click={() => dismissGuided(curKey(), gi)}
                ><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"><path d="M6 6l12 12M18 6 6 18"/></svg></button>
              </span>
            {/each}
          </div>
        {/if}
        <!-- 引用高亮（2026-11-01 用户对照 ZCode）：textarea 没法给部分文字上色，叠一层
             同字号同排版的浮层，只把 /能力、@文件 这些引用画成强调色；其余字符透明，
             纯为对齐换行。textarea 的文字照旧显示（浮层在上，视觉上正好盖住同款字形），
             这样浮层即使有亚像素偏差也只是轻微描边，不会出现"字没了"。 -->
        <div class="composer-field" class:composing={composerComposing}>
          <div class="composer-hl" aria-hidden="true" bind:this={composerHlEl}>{@html composerHl}</div>
          <textarea
            class="composer-input"
            rows="1"
            bind:this={composerInputEl}
            bind:value={input}
            placeholder={streaming ? '可发送消息以进行对话引导或打断' : ''}
            on:keydown={handleKey}
            on:input={autoRows}
            on:scroll={syncComposerHl}
            on:compositionstart={() => (composerComposing = true)}
            on:compositionend={() => (composerComposing = false)}
            on:paste={handlePaste}
          ></textarea>
        </div>
        <div class="composer-bar">
          <div class="composer-left">
            <div class="bar-menu">
              <button
                class="icon-btn"
                title="添加上下文 / 选择能力"
                on:click={() => {
                  plusMenuOpen = !plusMenuOpen;
                  abilityMenuOpen = false;
                }}
              ><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M12 5v14M5 12h14"/></svg></button>
              {#if plusMenuOpen}
                <div class="bar-dropdown">
                  <button class="bar-item" on:click={openCtxPicker}>
                    <span class="menu-sym">@</span> 添加上下文
                  </button>
                  <button class="bar-item" on:click={openAbilityMenu}>
                    <span class="menu-sym">/</span> 选择能力
                  </button>
                </div>
              {/if}
              {#if abilityMenuOpen}
                <!-- 用户 2026-10-02 第五轮实锤：①三类平铺，插件全列、不许展开；
                     ②引用前缀统一用「/」（技能/MCP/插件都 /，不再用 $） -->
                <div class="bar-dropdown">
                  <div class="bar-group">技能</div>
                  {#if ownSkills.length}
                    {#each ownSkills as sk (sk.name)}
                      <button class="bar-item" on:click={() => { abilityMenuOpen = false; insertAtCursor(`/${sk.name} `); }}>
                        <span class="menu-sym">/</span> {sk.name}
                      </button>
                    {/each}
                  {/if}
                  <div class="bar-group">MCP</div>
                  <!-- MCP 只列服务本体，工具不逐条刷屏——
                       插服务级引用，模型按本体名找到并用它下面的工具 -->
                  {#each mcpServers as srv}
                    {#if srv.online !== false && srv.tools && srv.tools.length}
                      <button class="bar-item" on:click={() => { abilityMenuOpen = false; insertAtCursor(`/mcp__${srv.name} `); }} title={srv.transport + ` · ${srv.tools.length} 个工具`}>
                        <span class="menu-sym">/</span> {srv.name}<span class="menu-sub">{srv.tools.length} 工具</span>
                      </button>
                    {/if}
                  {/each}
                  {#each wasmPlugins as w}
                    {#each w.tools || [] as t}
                      <button class="bar-item" on:click={() => { abilityMenuOpen = false; insertAtCursor(`/wasm__${w.stem}__${t.name} `); }}>
                        <span class="menu-sym">/</span> {t.name}
                      </button>
                    {/each}
                  {/each}
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
                      on:click={() => {
                        abilityMenuOpen = false;
                        insertAtCursor(`/${ref} `);
                      }}
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
              on:change={onFileSelect}
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
                class="pill"
                class:danger={approvalEffective === 'full-access'}
                on:click={() => (approvalMenuOpen = !approvalMenuOpen)}
                title="审批档位（按会话记忆，运行中切换下一步即生效）：只读=写/执行逐次问 · 自动=可动工作区与电脑高危先问 · 完全访问=全免审本机直跑"
              >
                <span class="pill-label">审批:</span> {APPROVAL_LABELS[approvalEffective] || approvalEffective}
                <svg class="caret-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6"/></svg>
              </button>
              {#if approvalMenuOpen}
                <div class="bar-dropdown">
                  {#each Object.entries(APPROVAL_LABELS) as [val, label]}
                    <button class="bar-item" class:active={val === approvalEffective} on:click={() => pickApproval(val)}>
                      {label}
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
                  <div class="ctx-bar"><div class="ctx-bar-fill" style:width={`${ctxPct}%`}></div></div>
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
                on:click={() => {
                  modelMenuOpen = !modelMenuOpen;
                  if (modelMenuOpen) menuExpandedCfg = activeConfigId;
                }}
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
                      on:click={() => (menuExpandedCfg = menuExpandedCfg === cfg.id ? '' : cfg.id)}
                    >
                      <span>{configLabel(cfg)}{cfg.id === activeConfigId ? ' · 当前' : ''}</span>
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
                          on:click={() => pickCrossModel(cfg, m)}
                        >
                          {m}
                        </button>
                      {/each}
                    {/if}
                  {/each}
                  <div class="menu-sep"></div>
                  <button class="bar-item" on:click={() => { modelMenuOpen = false; settingsOpen = true; }}>
                    管理模型…
                  </button>
                </div>
              {/if}
            </div>
            <div class="bar-menu">
              <button
                class="pill think-pill"
                class:active={thinkingOn}
                on:click={() => (thinkMenuOpen = !thinkMenuOpen)}
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
                    <button class="think-state" class:active={thinking === 'off'} on:click={() => pickThinking('off')}>关</button>
                    <button class="think-state" class:active={thinking === ''} on:click={() => pickThinking('')}>默认</button>
                    <button class="think-state" class:active={thinkingOn} on:click={pickThinkOn}>开</button>
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
                                  {#each EFFORT_BUBBLES as b}
                                    <span
                                      class="ts-bubble"
                                      style:top={`${b.top}px`}
                                      style:animation-duration={`${(b.duration * BUBBLE_SPEED * bubbleSpeedMult).toFixed(2)}s`}
                                      style:animation-delay={`${b.delay}s`}
                                    >
                                      <span class="ts-bubble-dot" style:width={`${b.size}px`} style:height={`${b.size}px`}></span>
                                    </span>
                                  {/each}
                                </div>
                              {/key}
                            </div>
                          </div>
                          <div class="ts-stops">
                            {#each thinkLevels as lv, i}
                              <span class="ts-stop" class:filled={i <= thinkLevelIdx} class:hovered={tsHoverIdx === i}></span>
                            {/each}
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
                          on:input={(e) => thinkRangeInput(+e.target.value)}
                          on:pointermove={tsHoverMove}
                          on:pointerleave={() => (tsHoverIdx = null)}
                        />
                      </div>
                    </div>
                  {/if}
                </div>
              {/if}
            </div>
            {#if streaming}
              <button class="send-btn stop" on:click={stop} title="停止" aria-label="停止"><svg viewBox="0 0 24 24" fill="currentColor"><rect x="7" y="7" width="10" height="10" rx="2"/></svg></button>
            {/if}
            <button class="send-btn" on:click={send} disabled={!input.trim()} title={streaming ? '发送（进入队列，这一轮结束后自动发出）' : '发送（Enter）'} aria-label="发送"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 19V5M5 12l7-7 7 7"/></svg></button>
          </div>
        </div>
      </div>
    </footer>
    </div>
    <TerminalPanel
      open={terminalOpen}
      lines={termLines}
      running={termRunning}
      height={termH}
      resizing={termResizing}
      bind:value={termInput}
      on:resizestart={(e) => startTermResize(e.detail)}
      on:run={(e) => runTerm(e.detail)}
      on:clear={() => (termLines = [])}
      on:close={() => (terminalOpen = false)}
    />
  </main>

  {#if rvMenu}
    <!-- git 面板行的右键菜单（预览 / 复制路径；行点击就地展开 diff） -->
    <!-- svelte-ignore a11y-click-events-have-key-events a11y-no-static-element-interactions -->
    <div class="rv-menu-back" on:click={() => (rvMenu = null)} role="presentation"></div>
    <!-- svelte-ignore a11y-no-static-element-interactions -->
    <div class="rv-menu" style="left:{rvMenu.x}px; top:{rvMenu.y}px" on:click|stopPropagation role="menu">
      <button title="在右侧栏预览这个文件" on:click={() => { openPreviewTab(rvMenu.path); rvMenu = null; }}>预览文件</button>
      <button
        title="复制工作区相对路径"
        on:click={() => {
          putClipboard(rvMenu.path);
          rvMenu = null;
        }}>复制相对路径</button>
    </div>
  {/if}

  {#if rightPanelOpen}
    <aside class="right-panel" class:resizing={rpResizing} style="width:{rpWidth}px">
      <div class="rp-resize" title="拖拽调整宽度" on:pointerdown={startRpResize}></div>
      <div class="rp-tabs">
        <!-- 标签条可横向滚动（标签多了不挤爆），+ 号固定在条外不被滚走 -->
        <div class="rp-tabstrip" bind:this={rpStripEl} on:scroll={rpStripSync}>
          {#each rpVisibleTabs as t (t.key)}
            <button
              class="rp-tab"
              class:active={rpTab === t.key}
              title={t.kind === 'file' ? t.path : t.kind === 'ext' ? rpExtTitle(t.name) : t.title}
              on:click={() => activateRpTab(t.key)}
            >
              <svg class="rp-tab-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">{@html rpTabIcon(t)}</svg>
              <span class="rp-tab-title">{t.kind === 'ext' ? rpExtTitle(t.name) : t.title}</span>
              <!-- × 只在激活标签上且顶到标签最右：五个标签也放得下，关别的先点它 -->
              {#if rpTab === t.key}
                <span class="rp-tab-x" title="关闭标签" role="button" tabindex="-1" on:click|stopPropagation={() => closeRpTab(t.key)}>×</span>
              {/if}
            </button>
          {/each}
        </div>
        {#if rpStripBar}
          <!-- 溢出滑杆：压在 .rp-tabs 下边框线上，不占布局（「就在那条线中间加很细的」） -->
          <i
            class="rp-strip-track"
            style="width:{rpStripBar.track}px"
            on:pointerdown={rpStripDown}
            on:pointermove={rpStripMove}
            on:pointerup={rpStripUp}
            on:pointercancel={rpStripUp}
          >
            <i class="rp-strip-thumb" style="width:{rpStripBar.w}px; transform:translateX({rpStripBar.x}px)"></i>
          </i>
        {/if}
        <span class="rp-add-wrap">
          <button class="rp-add" title="添加标签页" on:click|stopPropagation={() => (rpAddOpen = !rpAddOpen)}>+</button>
          {#if rpAddOpen}
            <div class="rp-add-menu">
              {#each rpAddable as p (p.key)}
                <button on:click|stopPropagation={() => addRpTab(p)}>
                  {#if p.kind === 'ext' && extIcon(p.name)}
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">{@html extIcon(p.name)}</svg>
                  {:else if p.kind === 'web'}
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"><circle cx="12" cy="12" r="8.4"/><path d="M3.6 12h16.8"/><ellipse cx="12" cy="12" rx="3.8" ry="8.4"/></svg>
                  {:else if p.kind === 'git'}
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><circle cx="6.5" cy="6" r="2.5"/><circle cx="6.5" cy="18" r="2.5"/><circle cx="17.5" cy="12" r="2.5"/><path d="M6.5 8.5v7M9 6h3.5a3 3 0 0 1 3 3v1"/></svg>
                  {:else}
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M7 3.5h6.5L18 8v12.5H7z"/><path d="M13.5 3.5V8H18"/><path d="M9.5 12h5M9.5 15h5"/></svg>
                  {/if}
                  <span>{p.title}</span>
                </button>
              {/each}
              {#if !rpAddable.length}<div class="rp-add-empty">面板都开着了</div>{/if}
            </div>
          {/if}
        </span>
        <span class="flex-sp"></span>
        {#if rpTab === 'artifacts'}
          <button class="rp-refresh" title="刷新产物清单" on:click={loadArtifacts}>↻</button>
        {/if}
        <!-- 窗口控制（无边框壳）：右面板开着时贴右侧栏右缘（「侧边栏右边」指这栏）；
             右面板没开时回顶栏 head-side -->
        {#if winApi}
          <span class="rp-win-gap"></span>
          <div class="win-ctrls">
            <button class="head-btn win-btn" title="最小化" aria-label="最小化" on:click={() => winApi.minimize()}>
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M6 12h12"/></svg>
            </button>
            <button class="head-btn win-btn" title={winMax ? '还原窗口' : '最大化'} aria-label={winMax ? '还原窗口' : '最大化'} on:click={() => winApi.toggleMax().then(refreshWinMax, refreshWinMax)}>
              {#if winMax}
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="4.5" y="7.5" width="12" height="12" rx="2"/><path d="M8 7.5V6a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2h-1.5"/></svg>
              {:else}
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"><rect x="5.5" y="5.5" width="13" height="13" rx="2"/></svg>
              {/if}
            </button>
            <button class="head-btn win-btn win-close" title="关闭到托盘（服务保持运行）" aria-label="关闭到托盘" on:click={() => winApi.hide()}>
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M6.5 6.5l11 11M17.5 6.5l-11 11"/></svg>
            </button>
          </div>
        {/if}
      </div>
      <div class="rp-body">
        {#if !rpVisibleTabs.length}
          <!-- 标签全关掉后的选择页：点卡片即开该面板 -->
          <div class="rp-open">
            <div class="rp-open-title">打开标签页</div>
            <div class="rp-open-sub">选择要在侧边面板中打开的标签。</div>
            <div class="rp-open-grid">
              {#each rpAddable as p (p.key)}
                <button class="rp-open-card" on:click={() => addRpTab(p)}>
                  {#if p.kind === 'ext' && extIcon(p.name)}
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">{@html extIcon(p.name)}</svg>
                  {:else if p.kind === 'web'}
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"><circle cx="12" cy="12" r="8.4"/><path d="M3.6 12h16.8"/><ellipse cx="12" cy="12" rx="3.8" ry="8.4"/></svg>
                  {:else if p.kind === 'git'}
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><circle cx="6.5" cy="6" r="2.5"/><circle cx="6.5" cy="18" r="2.5"/><circle cx="17.5" cy="12" r="2.5"/><path d="M6.5 8.5v7M9 6h3.5a3 3 0 0 1 3 3v1"/></svg>
                  {:else}
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M7 3.5h6.5L18 8v12.5H7z"/><path d="M13.5 3.5V8H18"/><path d="M9.5 12h5M9.5 15h5"/></svg>
                  {/if}
                  <span>{p.title}</span>
                </button>
              {/each}
            </div>
          </div>
        {:else}
          {#each rpVisibleTabs as t (t.key)}
            <div class="rp-pane" class:hidden={t.key !== rpTab}>
              {#if t.kind === 'artifacts'}
                {#if rpLoading}
                  <div class="trace-empty">加载中…</div>
                {:else if !rpFiles.length}
                  <div class="rp-empty">{rpArtifactsNote || '本任务还没有产物。'}<br />agent 修改/创建文件后，这里会列出改动清单（相对本次任务起点）。</div>
                {:else}
                  {#each rpFiles as f (f.path)}
                    <button class="rp-file" class:sel={f.path === rpPreviewPath} title={f.path} on:click={() => openPreviewTab(f.path)}>
                      {f.path}
                    </button>
                  {/each}
                {/if}
              {:else if t.kind === 'git'}
                <!-- git 面板（提交跟审查合成一个标签，聊天区底部
                     那根常驻条撤掉）。自上而下：状态行（分支/HEAD/领先落后）→ 范围选择器
                     （本任务改动 = 影子仓库检查点 / 未提交改动 = 工作区相对 HEAD）→ 提交盒
                     （未提交档：清单行前的勾选即暂存）→ 文件行点开就地看 diff -->
                <div class="git-pane">
                  {#if gitMissing}
                    <!-- 缺 git 明说：撤销/检查点/改动/提交/审查 diff 都用不了 -->
                    <div class="git-missing-note">没检测到 git：撤销、检查点、改动清单、提交、审查 diff 都用不了。{GIT_INSTALL_HINT}。</div>
                  {:else if gitInfo && !gitInfo.is_repo}
                    <div class="git-missing-note">当前工作区不是 git 仓库：撤销、改动清单、提交、审查 diff 都用不了。</div>
                  {:else if gitInfo && gitInfo.is_repo}
                    <div class="git-status">
                      <span class="git-ref" title={gitInfo.upstream ? `上游：${gitInfo.upstream}` : '未设置上游分支'}>
                        <svg class="git-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="6.5" cy="6" r="2.5"/><circle cx="6.5" cy="18" r="2.5"/><circle cx="17.5" cy="12" r="2.5"/><path d="M6.5 8.5v7M9 6h3.5a3 3 0 0 1 3 3v1"/></svg>
                        <b>{gitInfo.branch}</b>{#if gitInfo.head}<span class="git-head">@{gitInfo.head}</span>{/if}
                      </span>
                      {#if gitInfo.ahead || gitInfo.behind}
                        <span class="git-ab" title="相对上游的领先/落后提交数">{#if gitInfo.ahead}↑{gitInfo.ahead}{/if}{#if gitInfo.behind}↓{gitInfo.behind}{/if}</span>
                      {/if}
                    </div>
                  {/if}
                  <!-- svelte-ignore a11y-label-has-associated-control -->
                  <div class="rv-head">
                    <select
                      class="rv-src"
                      aria-label="切换范围"
                      bind:value={rvSource}
                      title="切换范围"
                      on:change={() => {
                        rvFiles = [];
                        rvOpen = {};
                        rvDiff = {};
                        loadReview();
                      }}
                    >
                      <option value="task">本任务改动</option>
                      <option value="work">未提交改动</option>
                    </select>
                    {#if rvFiles.length}
                      <span class="rv-totals">
                        <b>{rvFiles.length}</b> 个文件<span class="rv-tsep">·</span><span class="review-add">+{rvFiles.reduce((s, f) => s + (f.adds || 0), 0)}</span><span class="rv-tsep">·</span><span class="review-del">-{rvFiles.reduce((s, f) => s + (f.dels || 0), 0)}</span>
                      </span>
                    {/if}
                    <span class="flex-sp"></span>
                    {#if dockCp && rvSource === 'task' && rvBaseline === dockCp.id}
                      <button class="rv-btn" title="回滚到本任务起点（删除其后新建的已跟踪文件）" on:click={() => undoCheckpoint(dockCp)}>撤销</button>
                    {/if}
                    <button class="rv-btn" class:spin={rvLoading} title="重新读取（清单 + 分支状态）" on:click={() => { loadReview(); refreshGit(); }}>
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M19 12a7 7 0 1 1-2.05-4.95" /><path d="M19 3.5V7h-3.5" /></svg>
                    </button>
                  </div>
                  {#if rvSource === 'work' && gitInfo && gitInfo.is_repo && !gitMissing && rvFiles.length}
                    <!-- 提交盒：跟清单同档出现（未提交改动 = 暂存区），勾选即「这次提交带上它」 -->
                    <div class="commit-box">
                      <textarea
                        class="agents-editor commit-msg"
                        rows="2"
                        spellcheck="false"
                        placeholder="提交信息（一句话说清这次改了什么）"
                        bind:value={commitMsg}
                      ></textarea>
                      <div class="commit-bar">
                        <button class="commit-go" disabled={commitBusy} on:click={doCommit}>
                          {commitBusy ? '提交中…' : `提交 ${commitSelN} 个文件`}
                        </button>
                        <span class="commit-hint">只做本地 commit，不推送；未跟踪文件默认不勾</span>
                      </div>
                      {#if commitErr}<div class="commit-err">{commitErr}</div>{/if}
                    </div>
                  {/if}
                  {#if rvLoading && !rvFiles.length}
                    <div class="trace-empty">加载中…</div>
                  {:else if rvError && !rvFiles.length}
                    <div class="rp-empty">{rvError}</div>
                  {:else if !rvFiles.length}
                    <div class="rv-empty">
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9" /><path d="M8.5 12h7" /></svg>
                      <div class="rv-empty-t">{rvSource === 'work' ? '没有未提交的改动' : '本任务还没有改动'}</div>
                      <div class="rv-empty-d">{rvSource === 'work' ? 'agent 或你自己改动文件后，这里会列出来（相对 HEAD）。' : 'agent 修改/创建文件后，这里会列出改动清单（相对本次任务起点）。'}</div>
                    </div>
                  {:else}
                    <div class="rv-list">
                      {#each rvFiles as f (f.path)}
                        {@const name = f.path.split('/').pop()}
                        {@const dir = f.path.split('/').slice(0, -1).join('/')}
                        {@const dc = rvDiff[f.path] || {}}
                        <div class="rv-row" class:open={!!rvOpen[f.path]}>
                          {#if rvSource === 'work'}
                            <input
                              class="rv-check"
                              type="checkbox"
                              checked={!!commitSel[f.path]}
                              title={`提交这个文件（${gitStatusLabel(f.status)}）`}
                              on:change={() => toggleCommitFile(f.path)}
                            />
                          {/if}
                          <button
                            class="rv-file"
                            class:added={f.status === 'A' || f.status === '?'}
                            class:deleted={f.status === 'D'}
                            title={`${f.path}\n${gitStatusLabel(f.status)}\n右键：预览 / 复制路径`}
                            on:click={() => toggleRv(f)}
                            on:contextmenu={(e) => {
                              e.preventDefault();
                              rvMenu = { x: e.clientX, y: e.clientY, path: f.path };
                            }}
                          >
                            <svg class="rv-fico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M7 3.5h6.5L18 8v12.5H7z" /><path d="M13.5 3.5V8H18" /></svg>
                            <span class="rv-fname">{name}</span>
                            {#if dir}<span class="rv-fdir">{dir}/</span>{/if}
                            {#if f.status !== '?'}
                              <span class="rv-stat"><span class="review-add">+{f.adds}</span><span class="review-del">-{f.dels}</span></span>
                            {/if}
                            <svg class="rv-chev" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6" /></svg>
                          </button>
                          {#if rvOpen[f.path]}
                            <div class="rv-diffwrap">
                              {#if dc.loading}
                                <div class="rv-diffnote">加载中…</div>
                              {:else if dc.error}
                                <div class="rv-diffnote">读取失败: {dc.error}</div>
                              {:else if !dc.text}
                                <div class="rv-diffnote">没有可显示的 diff（该文件是未跟踪新增或无文本改动）。</div>
                              {:else}
                                <pre class="rv-diff">{#each rvParse(dc.text) as l, i (i)}<span class="rv-dl rv-dl-{l.t}">{#if l.t === 'add' || l.t === 'del' || l.t === 'ctx'}<span class="rv-dno">{l.o}</span><span class="rv-dno">{l.n}</span>{/if}<span class="rv-dtx">{l.s}</span></span>{/each}</pre>
                              {/if}
                            </div>
                          {/if}
                        </div>
                      {/each}
                    </div>
                  {/if}
                </div>
              {:else if t.kind === 'file'}
                <div class="rp-preview-head">
                  <span class="rp-preview-path" title={t.path}>{t.path}</span>
                  <span class="flex-sp"></span>
                  {#if (rpPreviewCache[t.path] || {}).savedAt}
                    <span class="rp-saved">已保存 {(rpPreviewCache[t.path] || {}).savedAt}</span>
                  {/if}
                  {#if (rpPreviewCache[t.path] || {}).editing}
                    <button
                      class="rp-btn"
                      title="保存（写回工作区文件）"
                      disabled={(rpPreviewCache[t.path] || {}).saving}
                      on:click={() => savePreview(t.path)}
                    >
                      {(rpPreviewCache[t.path] || {}).saving ? '保存中…' : '保存'}
                    </button>
                    <button class="rp-btn" title="取消编辑（丢弃改动）" on:click={() => cancelEditPreview(t.path)}>取消</button>
                  {:else if previewKind(t.path) === 'text' && (rpPreviewCache[t.path] || {}).content && !previewTruncated((rpPreviewCache[t.path] || {}).content)}
                    <button class="rp-btn" title="编辑内容" on:click={() => startEditPreview(t.path)}>编辑</button>
                  {/if}
                  <button class="rp-refresh" title="重新读取" on:click={() => loadPreview(t.path, true)}>↻</button>
                </div>
                {#if (rpPreviewCache[t.path] || {}).editing}
                  {#if (rpPreviewCache[t.path] || {}).editNote}
                    <div class="rp-edit-note">{(rpPreviewCache[t.path] || {}).editNote}</div>
                  {/if}
                  <textarea
                    class="rp-edit"
                    spellcheck="false"
                    value={(rpPreviewCache[t.path] || {}).draft || ''}
                    on:input={(e) => rpSet(t.path, { draft: e.currentTarget.value })}
                  ></textarea>
                {:else if previewKind(t.path) === 'image'}
                  <div class="rp-image-wrap">
                    <img class="rp-image" src={`/api/workspace/raw?path=${encodeURIComponent(t.path)}`} alt={t.path} />
                  </div>
                {:else if previewKind(t.path) === 'pdf'}
                  <!-- PDF 内嵌（WebView 自带查看器；#toolbar=0 藏掉查看器工具条，纯预览） -->
                  <iframe class="rp-pdf" title={t.path} src={`/api/workspace/raw?path=${encodeURIComponent(t.path)}#toolbar=0`}></iframe>
                {:else if previewKind(t.path) === 'docx'}
                  <!-- docx：mammoth 排版直出，纯预览（工具条也去，无缩放/打印） -->
                  {#if (rpPreviewCache[t.path] || {}).loading}
                    <pre class="rp-preview">渲染中…</pre>
                  {:else if (rpPreviewCache[t.path] || {}).error}
                    <pre class="rp-preview">{`渲染失败: ${(rpPreviewCache[t.path] || {}).error}`}</pre>
                  {:else}
                    <iframe
                      class="rp-pdf rp-doc-paper"
                      title={t.path}
                      sandbox=""
                      srcdoc={`<!doctype html><html><head><meta charset="utf-8"><style>body{font-family:'Segoe UI','Microsoft YaHei',sans-serif;padding:28px;margin:0;line-height:1.75;color:#24292f;background:#fff}img{max-width:100%}table{border-collapse:collapse;width:100%}td,th{border:1px solid #999;padding:6px 10px}h1,h2,h3{line-height:1.4}blockquote{border-left:3px solid #999;margin:0;padding-left:12px;color:#666}pre{background:#f3f4f6;padding:10px;border-radius:6px}</style></head><body>${((rpPreviewCache[t.path] || {}).content || '').replace(/<script/gi, '&lt;script')}</body></html>`}
                    ></iframe>
                  {/if}
                {:else if previewKind(t.path) === 'xlsx'}
                  <!-- xlsx：SheetJS 表格直出，纯预览（组件全去，无工具条/缩放/表名标注） -->
                  {#if (rpPreviewCache[t.path] || {}).loading}
                    <pre class="rp-preview">渲染中…</pre>
                  {:else if (rpPreviewCache[t.path] || {}).error}
                    <pre class="rp-preview">{`渲染失败: ${(rpPreviewCache[t.path] || {}).error}`}</pre>
                  {:else}
                    <div class="rp-xlsx">
                      {#each (() => { try { return JSON.parse((rpPreviewCache[t.path] || {}).content || '[]'); } catch { return []; } })() as sh}
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
                {:else if previewKind(t.path) === 'pptx'}
                  <!-- pptx：后端分页文本直出，纯预览（组件全去，无工具条/缩放/页码） -->
                  {#if (rpPreviewCache[t.path] || {}).loading}
                    <pre class="rp-preview">渲染中…</pre>
                  {:else if (rpPreviewCache[t.path] || {}).error}
                    <pre class="rp-preview">{`渲染失败: ${(rpPreviewCache[t.path] || {}).error}`}</pre>
                  {:else}
                    <div class="rp-pptx">
                      {#each (() => { try { return JSON.parse((rpPreviewCache[t.path] || {}).content || '[]'); } catch { return []; } })() as sl}
                        <div class="rp-slide"><pre class="rp-slide-text">{sl.text || '（空白页）'}</pre></div>
                      {/each}
                    </div>
                  {/if}
                {:else if previewKind(t.path) === 'media'}
                  <div class="rp-media-wrap">
                    {#if ['mp4', 'm4v', 'webm', 'mov'].includes(extOf(t.path))}
                      <video class="rp-media" controls autoplay={false} src={`/api/workspace/raw?path=${encodeURIComponent(t.path)}`}>
                        <track kind="captions" />
                      </video>
                    {:else}
                      <audio class="rp-media rp-audio" controls src={`/api/workspace/raw?path=${encodeURIComponent(t.path)}`}></audio>
                    {/if}
                  </div>
                {:else if (rpPreviewCache[t.path] || {}).loading}
                  <pre class="rp-preview">读取中…</pre>
                {:else if (rpPreviewCache[t.path] || {}).error}
                  <pre class="rp-preview">{`读取失败: ${(rpPreviewCache[t.path] || {}).error}`}</pre>
                {:else}
                  {#if previewKind(t.path) === 'doc'}
                    <div class="rp-note">文档类预览：这里是抽取出来的文本（排版与图表不在其中），不提供编辑</div>
                  {:else if previewTruncated((rpPreviewCache[t.path] || {}).content)}
                    <div class="rp-note">文件较大，预览是头尾压缩过的（中间省略）——为避免把省略部分写没，不提供编辑</div>
                  {/if}
                  <pre class="rp-preview">{((rpPreviewCache[t.path] || {}).content) || '（空文件）'}</pre>
                {/if}
              {:else if t.kind === 'ext'}
                <div class="web-wrap">
                  {#if enabledExts.find((e) => e.name === t.name)}
                    <iframe
                      class="ext-frame"
                      title={rpExtTitle(t.name)}
                      sandbox="allow-scripts"
                      bind:this={dockPaneFrames[t.name]}
                      src={`/plugins/${encodeURIComponent(t.name)}/${enabledExts.find((e) => e.name === t.name).entry || 'index.html'}`}
                      on:load={() => onExtFrameLoad(t.name)}
                    ></iframe>
                  {:else}
                    <div class="rp-empty">{extList.length || extMsg ? '插件未加载或已停用' : '加载中…'}</div>
                  {/if}
                </div>
              {:else}
          <div class="web-wrap">
            {#if webTabs.length}
              <div class="web-tabbar">
                <div class="web-tabs" bind:this={webStripEl} on:scroll={webStripSync}>
                {#each webTabs as t (t.id)}
                  <button
                    class="web-tab"
                    class:active={t.id === webActiveId}
                    title={t.url || 'about:blank'}
                    on:click={() => { webActiveId = t.id; if (!webBusy) webAddress = t.url === 'about:blank' ? '' : t.url || ''; webShotT += 1; webActivate(t.id); }}
                  >
                    <span class="web-tab-title">{t.title || (t.url === 'about:blank' ? '空白页' : t.url) || '空白页'}</span>
                    {#if t.agent}<span class="web-tab-agent" title="agent 浏览器工具的工作页">agent</span>{/if}
                    <span
                      class="web-tab-x"
                      title="关闭标签页"
                      role="button"
                      tabindex="-1"
                      on:click|stopPropagation={() => webCloseTab(t.id)}
                    >×</span>
                  </button>
                {/each}
                </div>
                {#if webStripBar}
                  <!-- 溢出滑杆：压在 .web-tabbar 下边框线上，同右侧标签页那条（用户 2026-10-31） -->
                  <i
                    class="web-strip-track"
                    style="width:{webStripBar.track}px"
                    on:pointerdown={webStripDown}
                    on:pointermove={webStripMove}
                    on:pointerup={webStripUp}
                    on:pointercancel={webStripUp}
                  >
                    <i class="web-strip-thumb" style="width:{webStripBar.w}px; transform:translateX({webStripBar.x}px)"></i>
                  </i>
                {/if}
              </div>
            {/if}
            <div class="web-toolbar">
              <button class="web-nav" title="后退" disabled={!webActive || webBusy} on:click={() => webAction('back')}>
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="m14.5 6-6 6 6 6"/></svg>
              </button>
              <button class="web-nav" title="前进" disabled={!webActive || webBusy} on:click={() => webAction('forward')}>
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="m9.5 6 6 6-6 6"/></svg>
              </button>
              <button class="web-nav" title="刷新" disabled={!webActive || webBusy} on:click={() => webAction('reload')}>
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M19 12a7 7 0 1 1-2.05-4.95"/><path d="M19 3.5V7h-3.5"/></svg>
              </button>
              <input
                class="web-address"
                placeholder="输入网址后回车"
                bind:value={webAddress}
                disabled={webBusy}
                spellcheck="false"
                on:keydown={(e) => {
                  if (e.isComposing || e.keyCode === 229) return;
                  if (e.key === 'Enter') { e.preventDefault(); webGo(); }
                }}
              />
              {#if webNative && webActive && webActive.loading}
                <span class="web-native-loading" title="页面加载中">●</span>
              {/if}
              <button class="web-add" title="新建标签页" on:click={webNewTab}>+</button>
              <button class="web-nav" title="更多" on:click={(e) => webToggleMenu(e)}>
                <svg viewBox="0 0 24 24" fill="currentColor"><circle cx="5" cy="12" r="1.8"/><circle cx="12" cy="12" r="1.8"/><circle cx="19" cy="12" r="1.8"/></svg>
              </button>
              {#if webMenuOpen && !webNative}
                <!-- svelte-ignore a11y-click-events-have-key-events a11y-no-static-element-interactions -->
                <div class="web-menu-backdrop" on:click={webCloseMenu}></div>
                <div class="web-menu">
                  <button on:click={() => webMenuAction('open-external')}>
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M14 4h6v6"/><path d="M20 4 11 13"/><path d="M19 14v5a2 2 0 0 1-2 2H6a2 2 0 0 1 2-2h5"/></svg>
                    在默认浏览器中打开
                  </button>
                  <!-- 无头模式没有「开发者工具」项：面板里是截图流，没有可挂真 DevTools 的
                       实体（服务端 /api/browser/devtools 对无头模式返回 opened:false 并引导
                       「在默认浏览器中打开后按 F12」）。壳模式的「⋯」菜单是壳里浮层
                       （web/public/menu.html），「开发者工具」项在那边——child webview
                       才有真 DevTools 可开。 -->
                </div>
              {/if}
            </div>
            {#if webError}
              <div class="web-error">{webError}</div>
            {/if}
            {#if webDialog}
              <!-- 页面 JS 弹窗（alert/confirm/prompt）：无头页面的原生弹窗在这处理，不卡死页面 -->
              <!-- svelte-ignore a11y-click-events-have-key-events a11y-no-static-element-interactions -->
              <div class="web-dialog" on:click|stopPropagation>
                <span class="web-dialog-msg">页面弹窗〔{webDialog.kind}〕{webDialog.message || '（无内容）'}</span>
                <span class="flex-sp"></span>
                <button class="btn ghost" on:click={() => webDialogAction(true)}>确定</button>
                <button class="btn ghost" on:click={() => webDialogAction(false)}>取消</button>
              </div>
            {/if}
            <div class="web-body" bind:this={webBodyEl} bind:clientWidth={webBodyW} bind:clientHeight={webBodyH}>
              {#if !webShowShot}
                <div class="web-empty">
                  <svg class="web-globe" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"><circle cx="12" cy="12" r="8.5"/><path d="M3.5 12h17M12 3.5c2.6 2.3 3.9 5.1 3.9 8.5s-1.3 6.2-3.9 8.5c-2.6-2.3-3.9-5.1-3.9-8.5s1.3-6.2 3.9-8.5Z"/></svg>
                  <div class="web-empty-title">网页</div>
                  <div class="web-empty-sub">粘贴或输入 URL 以打开网页。</div>
                </div>
              {:else if webNative && !shellModalLive}
                <!-- 壳窗口里的 child webview 就浮在这块区域上（原生 HWND 盖在 Svelte 之上），
                     这里只留占位——点击/滚动/输入全部走真实网页，不再坐标回传。
                     弹窗打开时 webview 移出屏幕保活、走下面的帧流分支（画面实时垫在弹窗下） -->
                <div class="web-native" aria-hidden="true"></div>
              {:else}
                {#if webBusy || (webActive && webActive.loading)}<div class="web-loading">加载中…</div>{/if}
                <!-- svelte-ignore a11y-click-events-have-key-events a11y-no-static-element-interactions -->
                <img
                  class="web-shot"
                  class:busy={webBusy}
                  src={`/api/browser/frame?id=${encodeURIComponent(webActive.id)}&t=${webShotT}&native=0`}
                  alt="页面画面"
                  draggable="false"
                  on:click={webShotClick}
                  on:wheel={webShotWheel}
                />
              {/if}
            </div>
          </div>
              {/if}
            </div>
          {/each}
        {/if}
      </div>
    </aside>
  {/if}

  {#if extOpen && extCurrent}
    <aside class="ext-panel">
      <div class="rp-tabs">
        <b class="ext-title">{extCurrent.title}</b>
        <span class="mcp-status">{extCurrent.source === 'workspace' ? '工作区' : '内置'}</span>
        <span class="flex-sp"></span>
        <button
          class="rp-refresh"
          title="重新加载插件"
          on:click={() => {
            if (extFrame) extFrame.src = extFrame.src;
          }}>↻</button
        >
        <button class="btn ghost" on:click={() => (extOpen = false)}>关闭</button>
      </div>
      <iframe
        class="ext-frame"
        bind:this={extFrame}
        title={extCurrent.title}
        sandbox="allow-scripts"
        src={`/plugins/${encodeURIComponent(extCurrent.name)}/${extCurrent.entry || 'index.html'}`}
        on:load={() => onExtFrameLoad(extCurrent.name)}
      ></iframe>
    </aside>
  {/if}

  {#if extPopup}
    <div class="modal-backdrop" on:click={() => (extPopup = null)}>
      <div class="ext-popup" on:click|stopPropagation>
        <div class="modal-head">
          <b>{extPopup.title}</b>
          <span class="flex-sp"></span>
          <button class="btn ghost" on:click={() => (extPopup = null)}>关闭</button>
        </div>
        <iframe
          class="ext-popup-frame"
          bind:this={extPopupFrame}
          title={extPopup.title}
          sandbox="allow-scripts"
          style={extPopupFrameH ? `height:${extPopupFrameH}px` : ''}
          src={`/plugins/${encodeURIComponent(extPopup.name)}/${extPopup.entry || 'index.html'}`}
          on:load={() => onExtFrameLoad(extPopup.name)}
        ></iframe>
      </div>
    </div>
  {/if}

  <!-- 扩展间通信：其它已启用扩展常驻后台（不可见），消息由宿主转发。 -->
  {#each enabledExts.filter((e) => e.entry && !(e.ui || []).some((u) => u.slot === 'dock')) as e (e.name)}
    {#if !(extOpen && extCurrent && extCurrent.name === e.name) && !(extPopup && extPopup.name === e.name)}
      <iframe
        class="ext-hidden"
        title={`插件后台：${e.title}`}
        sandbox="allow-scripts"
        bind:this={extFrameEls[e.name]}
        src={`/plugins/${encodeURIComponent(e.name)}/${e.entry || 'index.html'}`}
        on:load={() => onExtFrameLoad(e.name)}
      ></iframe>
    {/if}
  {/each}

</div>

{#if microPanel}
  <div
    class="micro-panel"
    style="left:{microPanel.x}px; top:{microPanel.y}px; max-height: calc(100vh - {microPanel.y}px - 10px);"
  >
    <div class="micro-head">
      <div class="micro-name">{microPanel.name}</div>
      {#if microPanel.sel}<div class="micro-sel">{microPanel.sel}</div>{/if}
    </div>
    {#if microPanel.unsupported}
      <!-- 跨源插件右击 iframe 本体：能力说明（旧桥插件不支持 / 提示去面板内容上右击） -->
      <div class="micro-body">
        <div class="micro-warn">{microPanel.msg}</div>
      </div>
      <div class="micro-foot">
        <div class="micro-actions">
          <button class="btn ghost" on:click={cancelMicro}>关闭</button>
        </div>
      </div>
    {:else}
    <div class="micro-body">
      <div class="micro-row">
        <label>文字色</label>
        <input type="color" data-m="color" value={microPanel.color} on:input={(e) => microPreview('color', e.target.value)} />
        <span class="micro-val">{microPanel.color}</span>
      </div>
      <div class="micro-row">
        <label>背景色</label>
        <input type="color" data-m="bg" value={microPanel.bg} on:input={(e) => microPreview('bg', e.target.value)} />
        <span class="micro-val">{microPanel.bgTransparent && !microPanel.bgDirty ? '透明（不动）' : microPanel.bg}</span>
      </div>
      <div class="micro-row">
        <label>背景透明</label>
        <input type="range" min="0" max="100" data-m="bgOpacity" value={microPanel.bgOpacity} on:input={(e) => microPreview('bgOpacity', +e.target.value)} />
        <span class="micro-val">{microPanel.bgOpacity}%</span>
      </div>
      {#if !microPanel.bgTransparent && microPanel.bgOpacity < 40}
        <!-- 对比度陷阱：主按钮的文字色是配着橙底定的深色，底一透明深字就沉进深底里，
             看起来像"文字跟着背景一起消失"（2026-11-04 真机实测）。文字色其实没被改动 -->
        <div class="micro-warn">背景快全透明了：文字本身没被改动，但失去底色衬托可能看不清——要调就调上面的「文字色」。</div>
      {/if}
      {#if microPanel.bgHost}
        <div class="micro-warn">这个元素本身是透明的，你看到的底色其实由上级「{microPanel.bgHost.name || microPanel.bgHost.sel}」绘制。改本层背景不会有变化。</div>
        <div class="micro-actions" style="margin-top:0">
          <button class="btn primary" on:click={useBgHost}>改上级「{microPanel.bgHost.name || microPanel.bgHost.sel}」</button>
        </div>
      {/if}
      {#if microPanel.extName && microPanel.root && microPanel.sel !== microPanel.root.sel}
        <div class="micro-actions" style="margin-top:0">
          <button class="btn ghost" on:click={usePanelRoot}>改整个面板底色（body）</button>
        </div>
      {/if}
      <div class="micro-row">
        <label>模糊</label>
        <input type="range" min="0" max="30" data-m="blur" value={microPanel.blur} on:input={(e) => microPreview('blur', +e.target.value)} />
        <span class="micro-val">{microPanel.blur}px</span>
      </div>
      {#if microPanel.blur > 0 && microPanel.bgOpacity >= 85 && !microPanel.bgTransparent}
        <div class="micro-warn">毛玻璃需要半透明底：模糊要透出下层，背景不透明度建议先降到 85% 以下，否则模糊不可见还会在弹窗下发灰。现在这条模糊不会被保存（半透明后可再加）。</div>
      {/if}
      <div class="micro-row">
        <label>圆角</label>
        <input type="range" min="0" max="40" data-m="radius" value={microPanel.radius} on:input={(e) => microPreview('radius', +e.target.value)} />
        <span class="micro-val">{microPanel.radius}px</span>
      </div>
      <div class="micro-row">
        <label>描边</label>
        <input type="range" min="0" max="8" data-m="border" value={microPanel.border} on:input={(e) => microPreview('border', +e.target.value)} />
        <span class="micro-val">{microPanel.border}px</span>
      </div>
      <div class="micro-row">
        <label>描边色</label>
        <input type="color" data-m="borderColor" value={microPanel.borderColor} on:input={(e) => microPreview('borderColor', e.target.value)} />
        <span class="micro-val">{microPanel.borderColor}</span>
      </div>
      <div class="micro-row">
        <label>字号</label>
        <input type="number" min="9" max="24" data-m="fontSize" value={microPanel.fontSize} on:input={(e) => microPreview('fontSize', +e.target.value)} />
        <span class="micro-val">{microPanel.fontSize}px</span>
      </div>
      {#if !microPanel.extName}
        <div class="micro-row">
          <label>作用范围</label>
          <div class="scope-opts">
            <button class="scope-opt" class:active={microPanel.scope !== 'all'} on:click={() => (microPanel = { ...microPanel, scope: 'only' })}>仅此元素</button>
            <button class="scope-opt" class:active={microPanel.scope === 'all'} on:click={() => (microPanel = { ...microPanel, scope: 'all' })}>同类全部</button>
          </div>
        </div>
      {/if}
      <div class="micro-hint">改动实时预览；「应用」只保存你动过的属性。{microPanel.extName ? '插件面板的改动按选择器整类生效，规则随面板重载自动回放。' : '作用范围默认「同类全部」，同类元素一起变；只想改这一个就切「仅此元素」。'}生成的规则见下方 CSS 注入器里的只读预览，手写 CSS 永远优先。</div>
    </div>
    <div class="micro-foot">
      <div class="micro-actions">
        <button class="btn primary" on:click={applyMicro}>应用</button>
        <button class="btn" on:click={resetMicro}>重置此元素</button>
        <button class="btn ghost" on:click={cancelMicro}>取消</button>
      </div>
    </div>
    {/if}
  </div>
{/if}

{#if toastMsg}
  <!-- 微调应用/重置的回执（1.6s 自动消失，不点不挡） -->
  <div class="toast">{toastMsg}</div>
{/if}

{#if settingsOpen}
  <div class="modal-backdrop" on:click={() => (settingsOpen = false)}>
    <div class="modal settings-modal" on:click|stopPropagation>
      <div class="modal-head">
        <b>设置</b>
        <span class="hint">{settingsMsg}</span>
        <button class="btn ghost" on:click={() => (settingsOpen = false)}>关闭</button>
      </div>
      <div class="settings-body">
        <div class="settings-nav">
          <button class:active={settingsTab === 'appearance'} on:click={() => pickSettingsTab('appearance')}><svg class="nav-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><circle cx="12" cy="12" r="8.5"/><circle cx="9" cy="10" r="1.2" fill="currentColor"/><circle cx="12.5" cy="8.5" r="1.2" fill="currentColor"/><circle cx="15.5" cy="11" r="1.2" fill="currentColor"/></svg> 外观</button>
          <button class:active={settingsTab === 'model'} on:click={() => pickSettingsTab('model')}><svg class="nav-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M9 7V3.5M15 7V3.5M7 7h10v4a5 5 0 0 1-10 0Z"/><path d="M12 16v5"/></svg> 模型配置</button>
          {#if features.roles}<button class:active={settingsTab === 'roles'} on:click={() => pickSettingsTab('roles')}><svg class="nav-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><circle cx="9" cy="8.5" r="3"/><path d="M3.5 19.5c.6-3 2.8-4.5 5.5-4.5s4.9 1.5 5.5 4.5"/><circle cx="17" cy="9.5" r="2.4"/><path d="M16 15.2c2.4.2 3.9 1.6 4.4 4"/></svg> 功能桥接</button>{/if}
          <button class:active={settingsTab === 'memory'} on:click={() => pickSettingsTab('memory')}><svg class="nav-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M12 4.5A7.5 7.5 0 0 0 4.5 12c0 2.4 1.1 4.2 2.8 5.5.4.3.7.9.7 1.5v1.5h9V19c0-.6.3-1.2.7-1.5 1.7-1.3 2.8-3.1 2.8-5.5A7.5 7.5 0 0 0 12 4.5Z"/></svg> 记忆</button>
          <button class:active={settingsTab === 'skills'} on:click={() => pickSettingsTab('skills')}><svg class="nav-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M13 2.5 5.5 13.5H11l-1 8L18.5 10H13l1-7.5Z"/></svg> 技能</button>
          <button class:active={settingsTab === 'mcp'} on:click={() => pickSettingsTab('mcp')}><svg class="nav-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M9 7V3.5M15 7V3.5M7 7h10v4a5 5 0 0 1-10 0Z"/><path d="M12 16v5"/></svg> MCP</button>
          <button class:active={settingsTab === 'ext'} on:click={() => pickSettingsTab('ext')}><svg class="nav-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"><rect x="3.5" y="4.5" width="17" height="15" rx="2.6"/><path d="M3.5 9.5h17"/><path d="M8 14h5" stroke-linecap="round"/></svg> 插件</button>
          <button class:active={settingsTab === 'schedule'} on:click={() => pickSettingsTab('schedule')}><svg class="nav-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="8.5"/><path d="M12 7.5V12l3 2"/></svg> 定时</button>
          <button class:active={settingsTab === 'data'} on:click={() => pickSettingsTab('data')}><svg class="nav-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><ellipse cx="12" cy="6" rx="7" ry="3"/><path d="M5 6v6c0 1.7 3.1 3 7 3s7-1.3 7-3V6"/><path d="M5 12v6c0 1.7 3.1 3 7 3s7-1.3 7-3v-6"/></svg> 数据</button>
        </div>
        <div class="settings-content">
      <AppearanceTab
        tab={settingsTab}
        {themeMode}
        {applyThemeMode}
        {wallpaper}
        bind:wallpaperOpacity
        {wallpaperMsg}
        {onWallpaperPick}
        {clearWallpaper}
        cssTool={features.css_tool}
        bind:customCss
        {applyCustomCss}
        {microTweaks}
        {clearAllMicro}
        bind:editMode
        {exportCssRules}
        {onImportCssRules}
      />
      <ModelTab
        tab={settingsTab}
        {providers}
        {configs}
        {activeConfigId}
        {configLabel}
        {manageConfig}
        {newConfig}
        {deleteConfig}
        bind:providerId
        {onProviderChange}
        bind:baseUrl
        bind:apiKey
        {typingHint}
        {clearKey}
        bind:configName
        bind:model
        {savedModels}
        {fetchModels}
        bind:ctxLen
        bind:maxOut
        bind:temperature
        bind:topP
        bind:capabilities
        {autoSave}
      />
      <RolesTab tab={settingsTab} rolesShown={features.roles} {configs} {configLabel} bind:roles {autoSave} />
      <MemoryTab tab={settingsTab} showGalaxy={features.galaxy} on:openGalaxy={() => { settingsOpen = false; galaxyOpen = true; }} />
      <SkillsTab tab={settingsTab} skills={skillsData} loading={skillsLoading} confirm={confirmAsk} on:reload={loadSkills} />
      <McpTab tab={settingsTab} servers={mcpServers} on:action={(e) => mcpAction(e.detail.action, e.detail.name, e.detail.server)} on:reload={loadMcp} />
      <ExtTab
        tab={settingsTab}
        {extMsg}
        {builtInExts}
        {thirdPartyExts}
        {extInfo}
        {loadExt}
        {createExt}
        {cancelCreateExt}
        bind:extCreateOpen
        bind:extCreateForm
        {openExt}
        {openExtPage}
        {isDockExt}
        {extTip}
        {extIcon}
        {toggleExt}
        {promoteExt}
        {discardExt}
        {removeExt}
        {gitRuntime}
        {gitSourceLabel}
        {loadGitRuntime}
        {sandboxInfo}
        {sandboxProbe}
        {sandboxProbeBusy}
        {runSandboxProbe}
        {sandboxProbeTip}
        {openMaker}
      />

      <ScheduleTab tab={settingsTab} workspace={workspace} recentWorkspaces={recentWorkspaces} on:open-session={(e) => openScheduleSession(e.detail.id)} />
      <DataTab tab={settingsTab} confirm={confirmAsk} />

        </div>
      </div>
    </div>
  </div>
{/if}

{#if ctxPickerOpen}
  <div class="modal-backdrop" on:click={() => (ctxPickerOpen = false)}>
    <div class="modal ctx-modal" on:click|stopPropagation>
      <div class="modal-head">
        <b>添加上下文</b>
        <span class="hint">{ctxTab === 'conv' ? '把对话内容作为上下文发给模型' : '插入 @相对路径（模型自行读取）'}</span>
        <button class="btn ghost" on:click={() => (ctxPickerOpen = false)}>关闭</button>
      </div>
      <div class="ctx-tabs">
        <button class="ctx-tab" class:active={ctxTab === 'conv'} on:click={() => { ctxTab = 'conv'; loadCtxSessions(); }}>对话</button>
        <button class="ctx-tab" class:active={ctxTab === 'files'} on:click={() => (ctxTab = 'files')}>文件</button>
      </div>
      {#if ctxTab === 'conv'}
        <input class="ctx-search" placeholder="搜索对话标题…" bind:value={ctxQuery} />
        <div class="ctx-file-list">
          {#if ctxSessionsLoading}
            <div class="trace-empty">加载中…</div>
          {:else}
            {#each ctxSessions.filter((s) => !ctxQuery || (s.title || '').toLowerCase().includes(ctxQuery.toLowerCase())) as s (s.id)}
              <!-- 用户 2026-10-02 第五轮：上下两行结构（标题上、时间下）——行内 flex
                   挤压问题反复踩，改成块级堆叠最稳，标题超长省略 -->
              <button class="ctx-file ctx-file-conv" on:click={() => pickCtxSession(s)}>
                <span class="ctx-conv-title">{s.title || s.id}</span>
                <span class="ctx-conv-ts">{(s.updated_at || '').slice(0, 16).replace('T', ' ')}</span>
              </button>
            {:else}
              <div class="menu-empty">{ctxQuery ? '没有匹配的对话' : '当前工作区还没有别的对话'}</div>
            {/each}
          {/if}
        </div>
      {:else}
        <input class="ctx-search" placeholder="搜索文件…" bind:value={ctxQuery} />
        <div class="ctx-file-list">
          {#if ctxLoading}
            <div class="trace-empty">加载中…</div>
          {:else}
            {#each ctxFiles.filter((f) => !ctxQuery || f.toLowerCase().includes(ctxQuery.toLowerCase())) as f (f)}
              <button class="ctx-file" on:click={() => pickCtxFile(f)}>{f}</button>
            {:else}
              <div class="menu-empty">没有匹配的文件</div>
            {/each}
          {/if}
        </div>
      {/if}
      {#if ctxMsg}<div class="menu-empty">{ctxMsg}</div>{/if}
    </div>
  </div>
{/if}

<WorkspaceModal
    open={wsModalOpen}
    msg={wsMsg}
    workspace={workspace}
    recent={recentWorkspaces}
    on:apply={(e) => applyWorkspace(e.detail)}
    on:close={() => (wsModalOpen = false)}
  />
<RenameModal modal={renameModal} on:close={() => (renameModal = null)} on:submit={submitRename} />
{#if subagentPanelOpen}
  <!-- 子代理回看面板：居中小弹窗（与插件制作器同款形态，不落侧边栏）。
       行内信息 = 实时事件（本页）+ GET /api/subagents 历史（跑完的/刷新前的） -->
  <div class="modal-backdrop" on:click={() => (subagentPanelOpen = false)}>
    <div class="modal subagent-modal" on:click|stopPropagation>
      <div class="modal-head">
        <b>子代理</b>
        <span class="subagent-modal-count">{subagentPanelRows.length} 次派发</span>
      </div>
      <div class="subagent-list">
        {#if !subagentPanelRows.length}
          <div class="subagent-empty">
            本会话还没有派发过子代理。模型遇到可并行的独立调研、多方案对比、批量同类小任务时会自动派发——
            子代理跑在独立会话里，最终结论作为工具结果回到主对话，成本记在本任务上。
          </div>
        {/if}
        {#each subagentPanelRows as d (d.dispatch)}
          <div class="subagent-item">
            <div class="subagent-item-head">
              <span class="subagent-badge {d.status}">{subagentStatusText(d.status)}</span>
              <span class="subagent-label">{d.label}</span>
              {#if d.model}<span class="subagent-model" title={d.model}>{d.model}</span>{/if}
            </div>
            <div class="subagent-item-meta">
              {#if d.tokens}<span class="subagent-meta">token {d.tokens}</span>{/if}
              {#if d.duration_ms}<span class="subagent-meta">{(d.duration_ms / 1000).toFixed(1)}s</span>{/if}
              <span class="subagent-meta dim">{d.child ? d.child.slice(0, 8) : ''}</span>
              {#if d.status === 'queued' || d.status === 'running'}
                <button class="subagent-item-cancel" on:click={() => cancelSubagent(d.dispatch)}>取消</button>
              {:else if d.child}
                <button class="subagent-item-open" on:click={() => { subagentPanelOpen = false; loadSession(d.child); }}>打开子会话</button>
              {/if}
            </div>
          </div>
        {/each}
      </div>
      <div class="subagent-foot">子代理无人值守：需要审批的操作按拒绝处理；取消只停这一个，父任务停止键会级联停全部。</div>
    </div>
  </div>
{/if}
{#if foldersModal}
  <!-- 多文件夹项目管理（2026-11-04，对齐 ZCode）：主目录固定，额外根可增删 -->
  <div class="modal-backdrop" on:click={() => (foldersModal = false)}>
    <div class="modal folders-modal" on:click|stopPropagation>
      <div class="modal-head">
        <b>项目文件夹</b>
        <span class="hint">{foldersMsg}</span>
        <button class="btn ghost" on:click={() => (foldersModal = false)}>关闭</button>
      </div>
      <div class="folders-body">
        <div class="folders-row">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"><path d="M3.5 7.2c0-1 .8-1.7 1.7-1.7h4l1.9 2.2h8.2c1 0 1.7.8 1.7 1.7v8.4c0 1-.8 1.7-1.7 1.7H5.2c-1 0-1.7-.8-1.7-1.7Z"/></svg>
          <div class="folders-info">
            <b>主目录</b>
            <div class="folders-path">{workspace || '（未设置工作区）'}</div>
          </div>
        </div>
        {#each workspaceFolders as f (f)}
          <div class="folders-row">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"><path d="M3.5 7.2c0-1 .8-1.7 1.7-1.7h4l1.9 2.2h8.2c1 0 1.7.8 1.7 1.7v8.4c0 1-.8 1.7-1.7 1.7H5.2c-1 0-1.7-.8-1.7-1.7Z"/><path d="M3.5 10.5h17"/></svg>
            <div class="folders-info">
              <b>{folderNameOf(f)}</b>
              <div class="folders-path">{f}</div>
            </div>
            <button class="row-btn" title="从项目移除此文件夹（不删磁盘文件）" on:click={() => removeWorkspaceFolder(f)}>
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><path d="M5 5l14 14M19 5 5 19"/></svg>
            </button>
          </div>
        {:else}
          <div class="folders-empty">还没有额外文件夹</div>
        {/each}
      </div>
      <div class="folders-foot">
        <button class="btn" on:click={addWorkspaceFolder} disabled={foldersBusy}>{foldersBusy ? '选择中…' : '添加文件夹…'}</button>
      </div>
    </div>
  </div>
{/if}
{#if computerPanelOpen}
  <!-- 操作电脑面板（第 4 轮）：居中小弹窗。左=最近截图（全尺寸落盘那份，经
       /api/workspace/raw 直出），右=动作流水。刷新/断线后的回看数据来自
       GET /api/computer/state；实时的那张来自 SSE 事件 -->
  <div class="modal-backdrop" on:click={closeComputerPanel}>
    <div class="modal computer-modal" on:click|stopPropagation>
      <div class="modal-head">
        <b>操作电脑</b>
        <span class="subagent-modal-count">{computerState.actions.length || (computerLive.action ? 1 : 0)} 个动作</span>
      </div>
      <div class="computer-body">
        <div class="computer-log">
          {#each computerState.actions as a, i}
            <div class="computer-log-item">
              {#if shotPathOf(a)}
                <!-- 截图条目：默认只留一行字，点开才加载这张图（不常驻） -->
                <button
                  class="computer-log-row shot"
                  class:last={i === 0}
                  on:click={() => toggleComputerShot(i)}
                  title={computerOpenShots[i] ? '收起截图' : '展开看这张截图'}
                >
                  <span class="clr-text">{a}</span>
                  <span class="clr-more">{computerOpenShots[i] ? '收起 ▲' : '看图 ▼'}</span>
                </button>
                {#if computerOpenShots[i]}
                  <img
                    class="computer-shot-img"
                    src={`/api/workspace/raw?path=${encodeURIComponent(shotPathOf(a))}`}
                    alt="操作时的屏幕截图"
                    loading="lazy"
                  />
                {/if}
              {:else}
                <div class="computer-log-row" class:last={i === 0}>{a}</div>
              {/if}
            </div>
          {:else}
            {#if computerLive.action}
              <div class="computer-log-row last">{computerLive.action}</div>
            {:else}
              <div class="computer-empty">还没有操作动作</div>
            {/if}
          {/each}
        </div>
      </div>
    </div>
  </div>
{/if}
<ConfirmModal
  modal={confirmModal}
  on:answer={(e) => {
    const resolve = confirmModal && confirmModal.resolve;
    confirmModal = null;
    if (resolve) resolve(e.detail);
  }}
/>

<ModelPicker
    open={modelModalOpen}
    models={fetchedModels}
    saved={savedModels}
    on:close={() => (modelModalOpen = false)}
    on:toggle={(e) => toggleModel(e.detail)}
    on:selectall={selectAllModels}
    on:selectnone={selectNoModels}
  />

<ImagePreview src={previewImageModal} on:close={() => (previewImageModal = null)} />

{#if statsModal}
  <!-- 消息信息卡（2026-10-26）：这条回复从哪来、多快、多少 token -->
  <div class="modal-backdrop" on:click={() => (statsModal = null)}>
    <div class="modal stats-modal" on:click|stopPropagation>
      <div class="modal-head">
        <b>这条回复的信息</b>
        <button class="btn ghost" on:click={() => (statsModal = null)}>关闭</button>
      </div>
      <div class="stats-grid">
        <span>模型</span><b>{statsModal.stats?.model || '—'}</b>
        <span>来源配置</span><b>{statsModal.stats?.config || '—'}</b>
        <span>供应商</span><b>{statsModal.stats?.provider_id || '—'}</b>
        <span>首 token 延迟</span><b>{fmtMs(statsModal.stats?.ttft_ms)}</b>
        <span>输出耗时</span><b>{fmtMs(statsModal.stats?.gen_ms)}</b>
        <span>输出速度</span><b>{statsModal.stats?.completion_tokens && statsModal.stats?.gen_ms ? (statsModal.stats.completion_tokens / (statsModal.stats.gen_ms / 1000)).toFixed(1) + ' tokens/秒' : '—'}</b>
        <span>输入 tokens</span><b>{statsModal.stats?.prompt_tokens ?? '—'}</b>
        <span>输出 tokens</span><b>{statsModal.stats?.completion_tokens ?? '—'}</b>
        <span>缓存输入</span><b>{statsModal.stats?.cached_tokens ?? '—'}{statsModal.stats?.prompt_tokens > 0 && statsModal.stats?.cached_tokens != null ? `（命中 ${((statsModal.stats.cached_tokens / statsModal.stats.prompt_tokens) * 100).toFixed(1)}%）` : ''}</b>
      </div>
    </div>
  </div>
{/if}

{#if galaxyOpen}
  <!-- 记忆库星云：光点 = 会话；搜索 = 向量检索过滤 -->
  <Galaxy currentSessionId={currentSessionId || ''} on:close={() => (galaxyOpen = false)} on:open={openSessionFromGalaxy} />
{/if}

<style>

  .app {
    display: flex;
    height: 100vh;
    position: relative;
    z-index: 1;
  }
  /* —— 背景壁纸层：fixed 铺满，压在内容之下；开启后主要表面改半透明透出壁纸 —— */
  .wallpaper {
    position: fixed;
    inset: 0;
    z-index: 0;
    background-size: cover;
    background-position: center;
    background-repeat: no-repeat;
    pointer-events: none;
  }
  .wallpaper-video {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  :global(body.has-wallpaper) {
    background: transparent;
  }
  :global(body.has-wallpaper) .sidebar {
    background: color-mix(in srgb, var(--g0) 60%, transparent);
    border-right-color: color-mix(in srgb, var(--g4) 30%, transparent);
  }
  :global(body.has-wallpaper) .chat-head {
    background: color-mix(in srgb, var(--g1) 62%, transparent);
    border-bottom-color: color-mix(in srgb, var(--g4) 30%, transparent);
  }
  /* 消息流内的深色面全部转半透明：88% 实色在壁纸上就是一块黑，用户 CSS 想盖也因为
     规则分散（.process/.cp-bar/...）而显得"去不掉"，这里统一收口 */
  :global(body.has-wallpaper) .asst-bubble {
    background: color-mix(in srgb, var(--g1) 72%, transparent);
    border-color: color-mix(in srgb, var(--g2) 50%, transparent);
  }
  :global(body.has-wallpaper) .composer {
    background: color-mix(in srgb, var(--g2) 72%, transparent);
  }
  :global(body.has-wallpaper) .process,
  :global(body.has-wallpaper) .tool-detail,
  :global(body.has-wallpaper) .tool-pre,
  :global(body.has-wallpaper) .approval-box,
  :global(body.has-wallpaper) .ctx-card {
    background: color-mix(in srgb, var(--g1) 66%, transparent);
    border-color: color-mix(in srgb, var(--g3) 50%, transparent);
  }
  :global(body.has-wallpaper) .new-chat {
    background: color-mix(in srgb, var(--g1) 66%, transparent);
  }
  :global(body.has-wallpaper) .conv.active {
    background: color-mix(in srgb, var(--g2) 66%, transparent);
  }
  :global(body.has-wallpaper) .conv:hover {
    background: color-mix(in srgb, var(--g1) 55%, transparent);
  }
  :global(body.has-wallpaper) .proj,
  :global(body.has-wallpaper) .side-search {
    background: color-mix(in srgb, var(--g2) 60%, transparent);
  }
  /* 自绘下拉的弹出菜单：壁纸模式下同样半透明，保持与其他浮层一致 */
  :global(body.has-wallpaper) .csel-menu {
    background: color-mix(in srgb, var(--g1) 80%, transparent);
    border-color: color-mix(in srgb, var(--g3) 60%, transparent);
  }
  /* 右侧产物/预览面板：整块 g0 实色，之前漏了壁纸半透明，表现为"改了内部元素透明度还是黑" */
  :global(body.has-wallpaper) .right-panel {
    background: color-mix(in srgb, var(--g0) 60%, transparent);
    border-left-color: color-mix(in srgb, var(--g4) 30%, transparent);
  }
  :global(body.has-wallpaper) .rp-preview {
    background: color-mix(in srgb, var(--g1) 66%, transparent);
    border-color: color-mix(in srgb, var(--g3) 50%, transparent);
  }
  .sidebar {
    width: 300px;
    box-sizing: border-box; /*  修复：宽=sidebarW 语义对齐（渲染宽不再多出 padding+border 25px，拖拽手柄贴手指） */
    flex: none;
    background: var(--g0);
    border-right: 1px solid var(--g1);
    display: flex;
    flex-direction: column;
    padding: 14px 12px 10px;
    position: relative;
  }
  .sidebar.resizing {
    border-right-color: var(--g3);
  }
  .resize-handle {
    position: absolute;
    top: 0;
    bottom: 0;
    right: -3px;
    width: 6px;
    cursor: col-resize;
    z-index: 20;
    /* 指针拖拽：禁掉触摸的滚动/缩放手势，pointermove 才会连续到手柄 */
    touch-action: none;
  }
  .resize-handle:hover {
    background: var(--veil);
  }
  .sidebar.collapsed {
    display: none;
  }
  /* 侧栏收起时展开按钮浮在左上角，标题要让位，否则两者叠在一起 */
  .chat.side-collapsed .chat-head {
    padding-left: 60px;
  }
  .side-head {
    flex: none;
  }
  .brand-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 2px 4px 14px;
  }
  .brand-mark {
    width: 30px;
    height: 30px;
    border-radius: 8px;
    /* 2026-11-01：图标换成无背景线稿，不再压阴影（透明底上阴影是一圈方框） */
    overflow: hidden;
    display: flex;
    align-items: center;
    justify-content: center;
    flex: none;
  }
  .brand-mark img {
    width: 100%;
    height: 100%;
    display: block;
  }
  .brand-name {
    flex: 1;
    font-size: 16px;
    font-weight: 650;
    color: var(--g7);
    letter-spacing: 0.2px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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
  .icon-btn.on {
    background: var(--g3);
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
  .new-chat {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    width: 100%;
    height: 40px;
    border-radius: 10px;
    border: 1px solid var(--g3);
    background: var(--g2);
    color: var(--g7);
    font-size: 13.5px;
    font-weight: 600;
    cursor: pointer;
    transition: background 0.15s ease, border-color 0.15s ease;
  }
  .new-chat:hover {
    background: var(--g3);
    border-color: var(--g4);
  }
  .new-chat:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .new-chat svg {
    width: 19px;
    height: 19px;
  }
  .section-row {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 14px 4px 6px;
  }
  .section-label {
    color: var(--g5);
    font-size: 14px;
    padding-left: 4px;
  }
  .section-tools {
    display: flex;
    gap: 3px;
  }
  .section-tools .icon-btn {
    width: 30px;
    height: 30px;
    border-radius: 8px;
    color: var(--g5);
  }
  .section-tools .icon-btn:hover,
  .section-tools .icon-btn.on {
    color: var(--g7);
  }
  .section-tools .icon-btn svg {
    width: 18px;
    height: 18px;
  }
  .side-search {
    margin: 2px 2px 8px;
    width: calc(100% - 4px);
    box-sizing: border-box;
    background: var(--g1);
    border: 1px solid var(--g2);
    border-radius: 10px;
    color: var(--g7);
    font-size: 14px;
    padding: 9px 12px;
    outline: none;
  }
  .side-search:focus {
    border-color: var(--g3);
  }
  .manage-bar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 2px 10px;
    flex-wrap: wrap;
  }
  .m-count {
    color: var(--g5);
    font-size: 13px;
    margin-right: auto;
    padding-left: 4px;
  }
  .m-btn {
    background: var(--g1);
    border: 1px solid var(--g2);
    color: var(--g6);
    font-size: 13px;
    border-radius: 8px;
    padding: 5px 10px;
    cursor: pointer;
  }
  .m-btn:hover:not(:disabled) {
    background: var(--g2);
  }
  .m-btn:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .m-btn.danger {
    color: var(--err, #e5697a);
    border-color: color-mix(in srgb, var(--err, #e5697a) 38%, transparent);
  }
  .m-btn.danger:hover:not(:disabled) {
    background: color-mix(in srgb, var(--err, #e5697a) 12%, transparent);
  }
  .m-btn.primary {
    background: linear-gradient(180deg,
      color-mix(in srgb, var(--brand) 72%, #fff),
      color-mix(in srgb, var(--brand-deep) 64%, #fff));
    color: var(--brand-ink);
    border-color: transparent;
    font-weight: 620;
  }
  .side-scroll {
    flex: 1;
    overflow-y: auto;
    margin: 0 -4px;
    padding: 0 4px;
  }
  .proj {
    position: relative;
    display: flex;
    align-items: center;
    gap: 10px;
    height: 48px;
    padding: 0 10px;
    border-radius: 12px;
    cursor: pointer;
    color: var(--g7);
    font-size: 15.5px;
    transition: background 0.15s ease;
  }
  .proj:hover {
    background: var(--g1);
  }
  .proj.expanded {
    background: var(--g2);
  }
  .proj > svg {
    width: 19px;
    height: 19px;
    color: var(--g6);
    flex: none;
  }
  /* 展开/收起的独立热区（在项目行内部，点它不切项目） */
  .proj-toggle {
    width: 22px;
    height: 22px;
    margin: 0 -3px 0 -2px;
    padding: 0;
    border: none;
    background: transparent;
    color: var(--g6);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    flex: none;
    border-radius: 6px;
  }
  .proj-toggle:hover {
    color: var(--g7);
    background: var(--g3);
  }
  .proj-toggle svg {
    width: 19px;
    height: 19px;
  }
  .proj-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .proj.active .proj-name {
    font-weight: 600;
  }
  /* 当前项目的小圆点标记 */
  .proj-cur {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--g5);
    flex: none;
    margin-left: 2px;
  }
  .proj-side {
    display: none;
    align-items: center;
    gap: 4px;
  }
  .proj:hover .proj-side,
  .proj.expanded .proj-side {
    display: flex;
  }
  .row-btn {
    width: 28px;
    height: 28px;
    border-radius: 8px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    color: var(--g6);
    cursor: pointer;
    flex: none;
  }
  .row-btn:hover {
    background: var(--g2);
    color: var(--g7);
  }
  .row-btn.on {
    border-color: var(--g7);
    color: var(--g7);
  }
  .row-btn svg {
    width: 17px;
    height: 17px;
  }
  .conv {
    position: relative;
    display: flex;
    align-items: center;
    gap: 8px;
    height: 42px;
    padding: 0 10px 0 38px;
    border-radius: 10px;
    cursor: pointer;
    color: var(--g6);
    font-size: 14.5px;
    transition: background 0.15s ease, color 0.15s ease;
  }
  .conv:hover {
    background: var(--g1);
  }
  .conv.active {
    background: color-mix(in srgb, var(--brand) 11%, var(--g2));
    color: var(--g7);
  }
  /* 运行中：会话行/项目行前的小转圈（本页的流即时，服务端在跑的靠 3 秒轮询补） */
  .conv-spin {
    flex: none;
    width: 11px;
    height: 11px;
    border-radius: 50%;
    border: 1.5px solid var(--g4);
    border-top-color: var(--g7);
    animation: conv-spin 0.8s linear infinite;
  }
  @keyframes conv-spin {
    to {
      transform: rotate(360deg);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .conv-spin {
      animation-duration: 2s;
    }
  }
  .conv-pin {
    width: 15px;
    height: 15px;
    color: var(--g5);
    flex: none;
  }
  .conv-check {
    position: relative;
    width: 16px;
    height: 16px;
    border: 1.5px solid var(--g4);
    border-radius: 4.5px;
    flex: none;
  }
  .conv-check svg {
    position: absolute;
    inset: 1px;
    width: calc(100% - 2px);
    height: calc(100% - 2px);
    color: var(--g1);
    opacity: 0;
  }
  .conv-check.on {
    background: var(--g7);
    border-color: var(--g7);
  }
  .conv-check.on svg {
    opacity: 1;
  }
  .conv-title {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .conv-more {
    display: none;
    width: 26px;
    height: 26px;
    border-radius: 7px;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    color: var(--g6);
    cursor: pointer;
    flex: none;
  }
  .conv-more:hover {
    background: var(--g2);
    color: var(--g7);
  }
  .conv-more svg {
    width: 16px;
    height: 16px;
  }
  .conv:hover .conv-more {
    display: flex;
  }
  .conv:hover .conv-time {
    display: none;
  }
  .conv-time {
    font-size: 12.5px;
    color: var(--g5);
    flex: none;
  }
  .expand-more {
    display: block;
    width: 100%;
    text-align: left;
    background: none;
    border: none;
    color: var(--g5);
    font-size: 14px;
    padding: 10px 10px 10px 38px;
    cursor: pointer;
    border-radius: 10px;
  }
  .expand-more:hover {
    color: var(--g6);
    background: var(--g1);
  }
  .side-empty {
    color: var(--g4);
    font-size: 14px;
    padding: 18px 12px;
  }
  .side-menu {
    position: absolute;
    z-index: 40;
    min-width: 172px;
    background: var(--g1);
    border: 1px solid var(--g2);
    border-radius: 14px;
    padding: 6px;
    box-shadow: 0 12px 32px var(--overlay);
  }
  .sess-menu,
  .proj-menu {
    top: calc(100% + 4px);
    right: 6px;
  }
  .view-menu {
    top: calc(100% + 2px);
    right: 0;
  }
  .menu-sep {
    height: 1px;
    margin: 5px 8px;
    background: var(--g2);
  }
  .side-menu button {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    background: none;
    border: none;
    color: var(--g7);
    font-size: 14px;
    padding: 10px 12px;
    border-radius: 9px;
    cursor: pointer;
    text-align: left;
    white-space: nowrap;
  }
  .side-menu button:hover {
    background: var(--g2);
  }
  .side-menu button svg {
    width: 17px;
    height: 17px;
    color: var(--g6);
    flex: none;
  }
  .side-menu .sel-check {
    margin-left: auto;
    color: var(--g7);
  }
  .sidebar-restore {
    position: absolute;
    left: 12px;
    top: 12px;
    z-index: 25;
    width: 36px;
    height: 36px;
    border-radius: 9px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--g1);
    border: 1px solid var(--g2);
    color: var(--g6);
    cursor: pointer;
  }
  .sidebar-restore:hover {
    color: var(--g7);
    background: var(--g2);
  }
  .sidebar-restore svg {
    width: 20px;
    height: 20px;
  }
  .side-foot {
    flex: none;
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    padding-top: 8px;
  }
  .foot-btn {
    flex: 1;
    background: transparent;
    border: 1px solid var(--g2);
    color: var(--g5);
    font-size: 13px;
    border-radius: 10px;
    padding: 8px 0;
    cursor: pointer;
  }
  .foot-btn:hover {
    color: var(--g6);
    background: var(--g1);
  }
  .galaxy-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
  }
  .galaxy-btn svg {
    width: 15px;
    height: 15px;
  }
  .chat {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
    position: relative;
  }
  /* 拖拽文件进聊天列的提示层（2026-11-01）：只做视觉，pointer-events 必须为 none，
     否则它会自己成为 drop 目标、把投放事件吃掉 */
  .drop-veil {
    position: absolute;
    inset: 8px;
    z-index: 45;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 2px dashed color-mix(in srgb, var(--brand) 72%, transparent);
    border-radius: 14px;
    background: color-mix(in srgb, var(--g1) 76%, transparent);
    color: var(--g7);
    font-size: 15px;
    pointer-events: none;
  }
  .jump-bottom {
    position: absolute;
    bottom: 130px; /* 悬浮输入框上方 */
    left: 50%;
    transform: translateX(-50%);
    z-index: 6;
    padding: 6px 14px;
    border-radius: 16px;
    border: 1px solid var(--g3);
    background: var(--g1);
    color: var(--tint-blue-fg);
    font-size: 12px;
    cursor: pointer;
    box-shadow: 0 4px 12px var(--overlay);
  }
  .jump-bottom:hover {
    border-color: var(--g4);
  }
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
    background: var(--g1);
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
    color: var(--tint-blue-fg);
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
  .ctx-bar-fill {
    height: 100%;
    border-radius: 4px;
    background: var(--tint-blue-fg);
    transition: width 0.3s;
  }
  .ctx-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 0;
    color: var(--tint-blue-fg);
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
  .ctx-card-foot {
    margin-top: 10px;
    padding-top: 8px;
    border-top: 1px solid var(--g2);
    color: var(--g5);
    font-size: 11px;
  }
  .cache-row {
    margin-top: 8px;
    padding-top: 8px;
    border-top: 1px solid var(--g2);
  }
  .chat-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
    padding: 12px 20px;
    border-bottom: 1px solid var(--g2);
    background: var(--g1);
    /* 无边框壳：头部空白处可拖拽移动窗口（data-tauri-drag-region 已加） */
    -webkit-app-region: drag;
  }
  .chat-head button, .chat-head input, .chat-head .plug-chip {
    -webkit-app-region: no-drag;
  }
  /* 窗口控制按钮组（─/▢/✕）：纯文字 tooltip，hover 才显底色 */
  .win-ctrls {
    display: flex;
    align-items: center;
    gap: 2px;
    margin-left: 4px;
  }
  .win-btn {
    width: 30px;
    height: 30px;
    padding: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: 6px;
    color: var(--g5);
  }
  .rp-win-gap { width: 8px; flex: none; }
  /* 右栏 tabs 行里的窗口按钮：.rp-tabs button 的特异性(0,1,1)会压过 .win-btn(0,1,0)，
     把 padding 撑坏、图标缩成小不点——这里用更高特异性钉死尺寸 */
  .rp-tabs .win-btn {
    width: 28px;
    height: 28px;
    padding: 0;
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }
  .rp-tabs .win-btn svg {
    width: 17px;
    height: 17px;
  }
  .win-btn:hover {
    background: var(--g3);
    color: var(--g7);
  }
  .win-close:hover {
    background: #c42b1c;
    color: #fff;
  }
  /* 无边框边缘拉伸热区：四边 6px + 四角 10px 见方，fixed 贴着视口边缘 */
  .win-rz { position: fixed; z-index: 900; }
  .win-rz-n { top: 0; left: 10px; right: 10px; height: 6px; cursor: ns-resize; }
  .win-rz-s { bottom: 0; left: 10px; right: 10px; height: 6px; cursor: ns-resize; }
  .win-rz-w { left: 0; top: 10px; bottom: 10px; width: 6px; cursor: ew-resize; }
  .win-rz-e { right: 0; top: 10px; bottom: 10px; width: 6px; cursor: ew-resize; }
  .win-rz-nw { top: 0; left: 0; width: 10px; height: 10px; cursor: nwse-resize; }
  .win-rz-ne { top: 0; right: 0; width: 10px; height: 10px; cursor: nesw-resize; }
  .win-rz-sw { bottom: 0; left: 0; width: 10px; height: 10px; cursor: nesw-resize; }
  .win-rz-se { bottom: 0; right: 0; width: 10px; height: 10px; cursor: nwse-resize; }
  .head-main {
    min-width: 0;
  }
  .chat-title {
    font-size: 15px;
    font-weight: 650;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .head-side {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: none;
  }
  .chat-main {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    position: relative;
  }
  .head-btn {
    width: 32px;
    height: 30px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: 1px solid transparent;
    border-radius: 8px;
    color: var(--g5);
    cursor: pointer;
    flex: none;
  }
  .head-btn svg {
    width: 19px;
    height: 19px;
  }
  .head-btn:hover {
    color: var(--g7);
    background: var(--g2);
  }
  .head-btn.active {
    color: var(--brand-text);
    border-color: var(--g3);
    background: var(--g2);
  }
  /* —— 右侧栏：产物 / 预览 —— */
  .right-panel {
    flex: none;
    position: relative;
    display: flex;
    flex-direction: column;
    background: var(--g0);
    border-left: 1px solid var(--g3);
    min-height: 0;
  }
  /* 左缘拖拽调宽手柄 */
  .rp-resize {
    position: absolute;
    left: -3px;
    top: 0;
    bottom: 0;
    width: 7px;
    cursor: col-resize;
    z-index: 6;
    touch-action: none;
  }
  .right-panel.resizing {
    border-left-color: var(--g5);
  }
  .rp-tabs {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 8px 10px;
    border-bottom: 1px solid var(--g3);
    position: relative; /* 溢出滑杆的锚点 */
  }
  /* 常驻后台的扩展 iframe（扩展间通信要用；不显示但保持运行） */
  .ext-hidden {
    position: absolute;
    width: 1px;
    height: 1px;
    border: 0;
    opacity: 0;
    pointer-events: none;
    left: -9999px;
  }
  /* 021 组件总线：UI 扩展面板（iframe 插槽；与产物面板同构，但不共用滚动容器） */
  .ext-panel {
    width: 460px;
    flex: none;
    display: flex;
    flex-direction: column;
    background: var(--g0);
    border-left: 1px solid var(--g3);
    min-height: 0;
  }
  .ext-title {
    color: var(--g7);
    font-size: 13px;
    margin-right: 2px;
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
  /* 网页面板（内置浏览器：截图渲染 + 坐标回传，与 agent 工具共享实例） */
  .web-wrap {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .web-error {
    color: var(--tint-red-fg, #ff6b6b);
    font-size: 12px;
    padding: 4px 2px 0;
  }
  .web-dialog {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 4px 0;
    padding: 6px 8px;
    border: 1px solid var(--g3);
    border-radius: 6px;
    background: var(--g1);
    font-size: 12px;
  }
  .web-dialog-msg {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 60%;
  }
  .web-tabbar {
    position: relative; /* 溢出滑杆的锚点（压在下边框线上） */
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 6px 8px;
    border-bottom: 1px solid var(--g3);
  }
  .web-tabs {
    display: flex;
    align-items: center;
    gap: 4px;
    overflow-x: auto;
    flex: 1;
    min-width: 0;
    scrollbar-width: none; /* 原生滚动条藏掉，溢出用自绘滑杆（同 rp-tabstrip） */
  }
  .web-tabs::-webkit-scrollbar {
    display: none;
  }
  .web-strip-track {
    position: absolute;
    left: 8px;
    bottom: -3px;
    height: 7px;
    z-index: 2;
    cursor: pointer;
  }
  .web-strip-thumb {
    display: block;
    height: 2px;
    margin-top: 2.5px;
    border-radius: 1px;
    background: var(--g4);
  }
  .web-strip-track:hover .web-strip-thumb {
    background: var(--g6);
  }
  .web-tab {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    max-width: 150px;
    padding: 3px 6px;
    border: 1px solid var(--g3);
    border-radius: 7px;
    background: var(--g1);
    color: var(--g6);
    font-size: 12px;
    cursor: pointer;
    white-space: nowrap;
    flex: none;
  }
  .web-tab.active {
    background: var(--g3);
    color: var(--g7);
  }
  .web-tab-title {
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 96px;
  }
  .web-tab-agent {
    font-size: 10px;
    color: var(--tint-blue-fg, #6aa9ff);
    border: 1px solid currentColor;
    border-radius: 4px;
    padding: 0 3px;
    flex: none;
  }
  .web-tab-x {
    opacity: 0.55;
    padding: 0 2px;
  }
  .web-tab-x:hover {
    opacity: 1;
  }
  .web-add {
    flex: none;
    width: 22px;
    height: 22px;
    border: 1px solid var(--g3);
    border-radius: 7px;
    background: var(--g1);
    color: var(--g6);
    font-size: 15px;
    line-height: 1;
    cursor: pointer;
  }
  .web-add:hover {
    color: var(--g7);
  }
  .web-menu-backdrop {
    position: fixed;
    inset: 0;
    z-index: 30;
  }
  .web-menu {
    position: absolute;
    top: calc(100% + 2px);
    right: 8px;
    z-index: 31;
    background: var(--g1);
    border: 1px solid var(--g3);
    border-radius: 9px;
    padding: 4px;
    min-width: 180px;
    box-shadow: 0 8px 28px rgba(0, 0, 0, 0.35);
  }
  /* 「⋯」菜单（仅无头模式：HTML 下拉盖在截图上，z-index 正常）。
     shell 模式不用这个浮层——菜单是壳里一个透明置顶的 webview（见 webToggleMenu：
     浮在网页之上、网页不动），因为 child webview 是 HWND，HTML 永远盖不过它。 */
  .web-menu button {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 7px 10px;
    border: none;
    background: transparent;
    color: var(--g7);
    font-size: 12.5px;
    border-radius: 6px;
    cursor: pointer;
    text-align: left;
  }
  .web-menu button:hover {
    background: var(--g3);
  }
  .web-menu svg {
    width: 14px;
    height: 14px;
    flex: none;
    color: var(--g6);
  }
  .web-toolbar {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 6px 8px;
    border-bottom: 1px solid var(--g3);
    position: relative;
  }
  .web-nav {
    flex: none;
    width: 26px;
    height: 26px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border: none;
    background: transparent;
    color: var(--g6);
    border-radius: 7px;
    cursor: pointer;
  }
  .web-nav:hover:not(:disabled) {
    background: var(--g2);
    color: var(--g7);
  }
  .web-nav:disabled {
    opacity: 0.4;
    cursor: default;
  }
  .web-nav svg {
    width: 15px;
    height: 15px;
  }
  .web-address {
    flex: 1;
    min-width: 0;
    padding: 5px 10px;
    border: 1px solid var(--g3);
    border-radius: 8px;
    background: var(--g1);
    color: var(--g7);
    font-size: 12.5px;
    outline: none;
  }
  .web-address:focus {
    border-color: var(--g5);
  }
  .web-body {
    flex: 1;
    min-height: 0;
    position: relative;
    overflow: auto;
    background: var(--g1);
    display: flex;
    align-items: flex-start;
    justify-content: center;
  }
  .web-empty {
    margin: auto;
    text-align: center;
    color: var(--g5);
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 40px 10px;
  }
  .web-globe {
    width: 52px;
    height: 52px;
    opacity: 0.5;
  }
  .web-empty-title {
    font-size: 14px;
    color: var(--g6);
  }
  .web-empty-sub {
    font-size: 12px;
  }
  .web-shot {
    width: 100%;
    height: auto;
    display: block;
    cursor: pointer;
    user-select: none;
  }
  /* 壳里 child webview 的占位区（原生 HWND 盖在这块上面，这里只是撑出尺寸） */
  .web-native {
    position: absolute;
    inset: 0;
    cursor: default;
  }
  .web-native-loading {
    color: var(--g-accent, #6cf);
    font-size: 9px;
    line-height: 1;
    animation: web-pulse 1s ease-in-out infinite;
  }
  @keyframes web-pulse {
    0%, 100% { opacity: 0.25; }
    50% { opacity: 1; }
  }
  .web-shot.busy {
    opacity: 0.6;
  }
  .web-loading {
    position: absolute;
    top: 8px;
    right: 10px;
    font-size: 11px;
    color: var(--g6);
    background: var(--g2);
    border: 1px solid var(--g3);
    border-radius: 6px;
    padding: 2px 8px;
    z-index: 2;
  }
  .ext-panel .rp-tabs {
    flex-wrap: nowrap;
  }
  .side-version {
    flex: 0 0 100%;
    text-align: center;
    font-size: 11px;
    color: var(--g4);
    letter-spacing: 0.3px;
    padding-top: 2px;
  }
  .rp-tabs button {
    padding: 5px 14px;
    border-radius: 7px;
    border: none;
    background: transparent;
    color: var(--g5);
    font-size: 13px;
    cursor: pointer;
    transition: background 0.15s ease, color 0.15s ease;
  }
  .rp-tabs button:hover {
    color: var(--g7);
    background: var(--g1);
  }
  .rp-tabs button.active {
    background: var(--g2);
    color: var(--g7);
    font-weight: 600;
  }
  /* —— 右侧边栏：统一标签页（2026-10-01）—— */
  .rp-tabstrip {
    display: flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
    flex: 0 1 auto; /* 条随内容收缩，+ 号贴着最后一个标签；标签多了内部横向滚动 */
    overflow-x: auto;
    scrollbar-width: none; /* 原生滚动条藏掉：它占布局会把标签顶起来；溢出指示用自绘滑杆压在边框线上 */
  }
  .rp-tabstrip::-webkit-scrollbar {
    display: none;
  }
  /* 自绘滑杆（就在分隔线中间加根很细的，别把标签顶上去）：2px 视觉细条骑在 1px 下边框上，7px 命中区 */
  .rp-strip-track {
    position: absolute;
    left: 10px; /* 对齐 .rp-tabs 左 padding；宽度由测量写入 */
    bottom: -3px;
    height: 7px;
    z-index: 2;
    cursor: pointer;
  }
  .rp-strip-thumb {
    display: block;
    height: 2px;
    margin-top: 2.5px;
    border-radius: 1px;
    background: var(--g4);
  }
  .rp-strip-track:hover .rp-strip-thumb {
    background: var(--g6);
  }
  .rp-tab {
    display: inline-flex !important;
    align-items: center;
    gap: 4px;
    padding: 4px 6px 4px 10px !important;
    width: 120px;
    flex: none; /* 固定宽：不随标签个数/面板宽/字数变（「就一直不变」），120px 连激活态带 × 也放得下 4 个汉字不省略 */
  }
  .rp-tab .rp-tab-ico {
    width: 13px;
    height: 13px;
    flex: none;
  }
  .rp-tab .rp-tab-title {
    flex: 1 1 auto; /* 吃掉余量，把激活标签的 × 顶到标签最右（用户：别靠在文字边上） */
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .rp-tab .rp-tab-x {
    color: var(--g4);
    font-size: 13px;
    line-height: 1;
    padding: 1px 3px;
    border-radius: 4px;
  }
  .rp-tab .rp-tab-x:hover {
    color: var(--g7);
    background: var(--g3);
  }
  .rp-add-wrap {
    position: relative;
    flex: none;
    display: inline-flex;
  }
  .rp-add {
    padding: 3px 10px !important;
    font-size: 16px;
    line-height: 1.2;
  }
  .rp-add-menu {
    position: absolute;
    top: calc(100% + 6px);
    left: 0;
    z-index: 30;
    min-width: 168px;
    display: flex;
    flex-direction: column;
    padding: 4px;
    background: var(--g1);
    border: 1px solid var(--g3);
    border-radius: 8px;
    box-shadow: 0 10px 26px rgb(0 0 0 / 45%);
  }
  .rp-add-menu button {
    display: flex !important;
    align-items: center;
    gap: 8px;
    padding: 7px 10px !important;
    border-radius: 6px;
    color: var(--g6);
    font-size: 12.5px;
    text-align: left;
    white-space: nowrap;
  }
  .rp-add-menu button:hover {
    background: var(--g2);
    color: var(--g7);
  }
  .rp-add-menu svg {
    width: 15px;
    height: 15px;
    flex: none;
  }
  .rp-add-empty {
    padding: 7px 10px;
    color: var(--g5);
    font-size: 12px;
  }
  /* 面板容器：一标签一 pane，隐藏的 display:none（iframe 常驻不重载，状态不丢） */
  .rp-pane {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }
  .rp-pane.hidden {
    display: none;
  }
  /* 标签全关掉后的「打开标签页」选择页（太紧凑 → 撑满面板宽度） */
  .rp-open {
    margin: auto;
    text-align: center;
    /* 关键：margin:auto 下容器是收缩宽度，卡片只有内容那么宽（真机实测"还是很紧凑"）；
       撑满（限宽 480）让两列卡片真正变大 */
    width: 100%;
    max-width: 480px;
    padding: 0 14px;
  }
  .rp-open-title {
    font-size: 17px;
    font-weight: 600;
    color: var(--g7);
    margin-bottom: 8px;
  }
  .rp-open-sub {
    font-size: 13px;
    color: var(--g5);
    margin-bottom: 22px;
  }
  .rp-open-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 14px;
  }
  .rp-open-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 14px;
    padding: 30px 16px;
    border-radius: 12px;
    border: 1px solid var(--g3);
    background: var(--g1);
    color: var(--g6);
    font-size: 14px;
    cursor: pointer;
  }
  .rp-open-card:hover {
    background: var(--g2);
    color: var(--g7);
    border-color: var(--g4);
  }
  .rp-open-card svg {
    width: 24px;
    height: 24px;
  }
  /* 文件预览标签的头部（路径吸顶 + 编辑/保存/重读） */
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
  .rp-btn {
    flex: none;
    padding: 2px 10px;
    border-radius: 6px;
    border: 1px solid var(--g3);
    background: transparent;
    color: var(--g6);
    font-size: 12px;
    cursor: pointer;
  }
  /* 下载按钮是 <a>：去掉下划线、行高对齐 */
  a.rp-btn {
    text-decoration: none;
    line-height: 1.6;
  }
  .rp-btn:hover {
    color: var(--g7);
    border-color: var(--g4);
  }
  .rp-btn:disabled {
    opacity: 0.5;
    cursor: default;
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
  /* docx 纸张预览：细边框把白纸和深色界面隔开 */
  .rp-doc-paper {
    border: 1px solid var(--g3);
    border-radius: 6px;
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
    border-color: var(--brand, #e18c6c);
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
  .rp-body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 8px;
    display: flex;
    flex-direction: column;
  }
  .rp-empty {
    margin: auto;
    text-align: center;
    color: var(--g5);
    font-size: 12.5px;
    line-height: 1.7;
    padding: 0 16px;
  }
  .rp-file {
    text-align: left;
    padding: 7px 10px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--g6);
    font-family: Consolas, monospace;
    font-size: 12.5px;
    cursor: pointer;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .rp-file:hover {
    background: var(--g2);
    color: var(--g7);
  }
  .rp-file.sel {
    background: var(--g2);
    color: var(--brand-text);
  }
  /* 路径标签吸顶：代码滚动时它不跟着滚，也不会被裁掉半个字 */
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
  /* —— 界面微调模式 —— */
  :global(body.micro-edit-mode *:hover) {
    outline: 1px dashed color-mix(in srgb, var(--brand) 55%, transparent);
    outline-offset: -1px;
  }
  /* 微调回执 toast：底部居中、不挡点击、1.6s 自动消失 */
  .toast {
    position: fixed;
    left: 50%;
    bottom: 30px;
    transform: translateX(-50%);
    z-index: 260;
    padding: 7px 14px;
    border-radius: 8px;
    background: var(--g2);
    border: 1px solid var(--g3);
    border-left: 2px solid var(--brand);
    color: var(--g7);
    font-size: 12.5px;
    box-shadow: 0 8px 24px var(--overlay);
    pointer-events: none;
  }
  .micro-panel {
    position: fixed;
    z-index: 200; /* 盖过设置弹窗（z-20）等一切浮层 */
    width: 270px;
    box-sizing: border-box; /* max-height 必须把 padding/border 一起算进去，否则底部会溢出视口 */
    background: var(--g2);
    border: 1px solid var(--brand);
    border-radius: 10px;
    padding: 10px;
    box-shadow: 0 8px 24px var(--overlay);
    font-size: 12px;
    /* 纵向弹性布局：头部与底部按钮固定，中间属性区滚动——
       面板比视口高时下面的「应用/重置/取消」也始终可见可点 */
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .micro-head {
    flex: none;
  }
  .micro-body {
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
    scrollbar-width: thin;
    scrollbar-color: var(--g4) transparent;
    padding-right: 2px;
  }
  .micro-foot {
    flex: none;
    padding-top: 8px;
    margin-top: 4px;
    border-top: 1px solid var(--g3);
  }
  .micro-foot .micro-actions {
    margin-top: 0;
  }
  .micro-name {
    font-weight: 650;
    color: var(--g7);
    margin-bottom: 4px;
  }
  .micro-sel {
    font-family: Consolas, monospace;
    font-size: 10.5px;
    color: var(--g5);
    margin-bottom: 8px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .micro-row {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 6px;
  }
  .micro-row label {
    width: 52px;
    flex: none;
    color: var(--g5);
  }
  .micro-row input[type='color'] {
    width: 34px;
    height: 24px;
    padding: 0;
    border: 1px solid var(--g3);
    border-radius: 5px;
    background: transparent;
    cursor: pointer;
  }
  .micro-row input[type='range'] {
    flex: 1;
    accent-color: var(--brand);
  }
  .micro-row input[type='number'] {
    width: 64px;
    background: var(--g1);
    color: var(--g7);
    border: 1px solid var(--g3);
    border-radius: 5px;
    padding: 3px 6px;
  }
  .micro-val {
    color: var(--g6);
    font-family: Consolas, monospace;
    font-size: 11px;
    min-width: 52px;
  }
  .micro-actions {
    display: flex;
    gap: 8px;
    margin-top: 8px;
  }
  .micro-hint {
    margin-top: 6px;
    color: var(--g5);
    font-size: 11px;
    line-height: 1.5;
  }
  .micro-warn {
    margin: 6px 0;
    padding: 6px 8px;
    border-radius: 6px;
    background: var(--tint-yellow-bg);
    color: var(--tint-yellow-fg);
    font-size: 11px;
    line-height: 1.5;
  }
  .scope-opts {
    display: flex;
    gap: 4px;
    flex: 1;
  }
  .scope-opt {
    flex: 1;
    padding: 4px 6px;
    border-radius: 7px;
    border: 1px solid var(--g3);
    background: var(--g1);
    color: var(--g6);
    font-size: 11.5px;
    cursor: pointer;
  }
  .scope-opt:hover {
    border-color: var(--g4);
    color: var(--g7);
  }
  .scope-opt.active {
    background: var(--brand);
    border-color: var(--brand);
    color: var(--brand-ink);
  }
  .chat-body {
    flex: 1;
    overflow-y: auto;
    padding: 20px 0 170px; /* 底部留出悬浮输入框 + 改动条的高度，最后一条消息不被遮挡 */
    /* 长会话性能（2026-10-07 真机实测：7000+ 条消息的会话，拖侧栏调宽每帧 120ms+，
       卡成一帧一顿）：这里**不能**用 flex 列——flex 子项不认 contain-intrinsic-size，
       被 content-visibility 跳过的消息会按 0 高度占位，滚动条直接废掉。改 block 布局，
       gap 用相邻 margin 补；空态（没有消息）才临时切回 flex 居中。 */
    display: block;
  }
  .chat-body > * + * {
    margin-top: 14px; /* 原 flex gap: 14px 的等价物 */
  }
  .chat-body:has(> .empty-hint) {
    display: flex; /* 空态：没有消息，flex 居中不影响 content-visibility 的收益 */
  }
  /* 屏幕外的消息块跳过布局与绘制——重排时只有可见的那几条参与。拖拽调宽 / 窗口
     resize / 面板开关都不再整树重排，初次加载同样受益（以前 7778 条全排）。
     「块」而不是「条」：content-visibility 的每个盒都被浏览器内部
     IntersectionObserver 跟踪，7778 个盒 = 每帧重算交叉 18ms+（CDP trace 实锤），
     收拢成 20 条一块后 IO 开销降一个数量级。块的估位高度由模板按块内条数内联
     （contain-intrinsic-size: auto <条数×140>px，沿用单条 140px 估位口径）。
     口径与「#each messageChunks」一致：cv-chunk 是 chat-body 直接子级 */
  .cv-chunk {
    content-visibility: auto;
  }
  /* 原 flex gap: 14px 的块内等价物 */
  .cv-chunk > .msg + .msg {
    margin-top: 14px;
  }
  .empty-hint {
    margin: auto;
    color: var(--g4);
    font-size: 13px;
    text-align: center;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .empty-title {
    font-size: 17px;
    font-weight: 600;
    color: var(--g6);
  }
  .empty-sub {
    font-size: 12.5px;
    color: var(--g5);
  }
  /* —— 成熟化图标体系：单色线性，尺寸统一 —— */
  .nav-ico {
    width: 16px;
    height: 16px;
    flex: none;
  }
  .settings-nav button {
    display: flex;
    align-items: center;
    gap: 9px;
  }
  .att-ico {
    width: 15px;
    height: 15px;
    flex: none;
    color: var(--g6);
  }
  .msg-att-file .att-ico {
    vertical-align: -2px;
    margin-right: 2px;
  }
  .att-ico-lg {
    width: 17px;
    height: 17px;
    display: block;
  }

  .att-del svg {
    width: 13px;
    height: 13px;
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
  .approval-kicker {
    display: inline-block;
    font-size: 11px;
    font-weight: 600;
    color: var(--g6);
    border: 1px solid var(--g3);
    border-radius: 4px;
    padding: 1px 6px;
    margin-right: 6px;
    vertical-align: 1px;
  }
  .typing-dot {
    display: inline-block;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--g5);
    margin-right: 4px;
    animation: typing-bounce 1.2s ease-in-out infinite;
  }
  .typing-dot:nth-child(2) {
    animation-delay: 0.15s;
  }
  .typing-dot:nth-child(3) {
    animation-delay: 0.3s;
    margin-right: 0;
  }
  @keyframes typing-bounce {
    0%,
    60%,
    100% {
      transform: translateY(0);
      opacity: 0.5;
    }
    30% {
      transform: translateY(-4px);
      opacity: 1;
    }
  }
  .msg {
    display: flex;
    width: 100%;
    max-width: 820px;
    margin: 0 auto;
    padding: 0 24px;
    box-sizing: border-box;
  }
  .msg.user {
    justify-content: flex-end; /* 只去气泡，位置还在右边（用户 2026-10-02 纠正） */
  }
  /* 排队中的消息（027）：气泡里带「立即发送」按钮——并进正在跑的那一轮，引导它的回答 */
  .queued-row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    margin-top: 6px;
    padding-top: 6px;
    border-top: 1px dashed var(--line, rgba(128, 128, 128, 0.35));
  }
  .queued-badge {
    font-size: 12px;
    opacity: 0.75;
  }
  .queued-badge.steered {
    color: var(--accent, #4a9);
  }
  .queued-send {
    font-size: 12px;
    padding: 2px 10px;
    border-radius: 999px;
    border: 1px solid var(--accent, #4a9);
    color: var(--accent, #4a9);
    background: transparent;
    cursor: pointer;
  }
  .queued-send:hover {
    background: color-mix(in srgb, var(--accent, #4a9) 14%, transparent);
  }
  /* 引导消息的静默轻量行（历史回放）：不占气泡位，居中小字 */
  .steer-line {
    display: flex;
    align-items: baseline;
    gap: 8px;
    justify-content: center;
    margin: 4px 0;
    font-size: 12px;
    color: var(--text-dim, #888);
    max-width: 78%;
    margin-left: auto;
    margin-right: 0;
  }
  .steer-line-badge {
    flex: none;
    padding: 1px 8px;
    border-radius: 8px;
    background: color-mix(in srgb, var(--accent, #4a9) 10%, transparent);
    color: var(--accent, #4a9);
  }
  .steer-line-text {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* 「已引导」静默提示（输入区上方，本轮结束消失） */
  .guided-row {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 2px 2px 6px;
  }
  .guided-chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    max-width: 100%;
    padding: 3px 10px;
    border-radius: 10px;
    font-size: 12px;
    color: var(--accent, #4a9);
    background: color-mix(in srgb, var(--accent, #4a9) 10%, transparent);
  }
  .guided-chip > svg {
    width: 12px;
    height: 12px;
    flex: none;
  }
  .guided-x {
    display: grid;
    place-items: center;
    width: 16px;
    height: 16px;
    padding: 0;
    border: 0;
    border-radius: 4px;
    background: transparent;
    color: inherit;
    opacity: 0.55;
    cursor: pointer;
    flex: none;
  }
  .guided-x:hover {
    opacity: 1;
    background: color-mix(in srgb, var(--accent, #4a9) 16%, transparent);
  }
  .guided-x svg {
    width: 10px;
    height: 10px;
  }
  .msg.assistant {
    justify-content: flex-start;
  }
  .msg.system {
    justify-content: center;
  }
  /* pre-wrap 只放在真正承载文本的叶子节点上；放 .bubble 上会把模板标签间的
     换行缩进也渲染成一个空行（用户气泡开头凭空多一行） */
  .bubble {
    max-width: 78%;
    padding: 10px 14px;
    border-radius: 12px;
    font-size: 14.5px;
    line-height: 1.7;
    word-break: break-word;
  }
  .user-text,
  .content {
    /* markdown 渲染（2026-10-04）：结构标签自带节奏，不再 pre-wrap 平铺 */
    word-break: break-word;
  }
  /* 用户消息是纯文本：换行/连续空格要保留（assistant 的 .content 是 markdown，不加） */
  .user-text {
    white-space: pre-wrap;
    line-height: 1.7;
  }
  /* {@html} 注入的 span 不带作用域类，必须 :global（同入下输入框浮层那个坑） */
  .user-text :global(.ref) {
    color: var(--brand-text);
  }
  /* 2026-10-30 排版重做：正文是主角——15px/1.78 的阅读节奏，标题/列表/代码块
     各自有清晰层级，不再是一坨同灰度小字。 */
  .content {
    font-size: 14.5px;
    line-height: 1.78;
    color: var(--g7);
  }
  /* ⚠️ markdown 是 {@html} 运行时生成的，元素上没有 Svelte 作用域类：
     所有 md-* 选择器必须用 :global()，否则会被编译器当「未使用选择器」整段裁掉
     （2026-10-30 实锤：md 样式在构建产物里一条都没有，代码块/表格一直吃浏览器默认样式）。 */
  .content > :first-child {
    margin-top: 0;
  }
  .content :global(.md-p) {
    margin: 0 0 10px;
  }
  .content :global(.md-p:last-child) {
    margin-bottom: 0;
  }
  .content :global(.md-h) {
    margin: 20px 0 8px;
    font-size: 15.5px;
    font-weight: 650;
    line-height: 1.45;
    letter-spacing: 0.01em;
  }
  .content :global(.md-h:first-child) {
    margin-top: 0;
  }
  .content :global(.md-ul),
  .content :global(.md-ol) {
    margin: 6px 0 12px;
    padding-left: 22px;
  }
  .content :global(.md-li),
  .content :global(li) {
    margin: 4px 0;
  }
  .content :global(.md-code) {
    background: var(--g2);
    border: 1px solid color-mix(in srgb, var(--g3) 70%, transparent);
    border-radius: 5px;
    padding: 1px 5px;
    font: 12.5px/1.5 Consolas, 'Cascadia Code', monospace;
    color: var(--g7);
  }
  /* 代码块：独立卡片 + 语言标签 + 复制按钮（md.js 生成结构，点击委托在 onContentClick） */
  .content :global(.md-codeblock) {
    margin: 10px 0 14px;
    border: 1px solid var(--g2);
    border-radius: 10px;
    overflow: hidden;
    background: var(--g0);
  }
  .content :global(.md-codehead) {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 5px 8px 5px 12px;
    background: var(--g1);
    border-bottom: 1px solid var(--g2);
  }
  .content :global(.md-codelang) {
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--g5);
  }
  .content :global(.md-copy) {
    border: none;
    background: transparent;
    color: var(--g5);
    font-size: 11px;
    padding: 2px 7px;
    border-radius: 5px;
    cursor: pointer;
  }
  .content :global(.md-copy:hover) {
    background: var(--g2);
    color: var(--g7);
  }
  .content :global(.md-pre) {
    background: transparent;
    border: 0;
    border-radius: 0;
    padding: 12px 14px;
    margin: 0;
    overflow: auto;
    max-height: 520px;
  }
  .content :global(.md-table) {
    border-collapse: collapse;
    margin: 8px 0 14px;
    font-size: 13px;
    display: block;
    overflow-x: auto;
    white-space: normal;
  }
  .content :global(.md-table th),
  .content :global(.md-table td) {
    border: 1px solid var(--g2);
    padding: 7px 12px;
    text-align: left;
    vertical-align: top;
  }
  .content :global(.md-table th) {
    background: var(--g2);
    font-weight: 600;
  }
  /* 表格行斑马纹（奇数行浅一档）：md.js 给 td/th 发了稳定类，这里顺带补上 ZCode 式
     隔行底色；微调钩子见下条 */
  .content :global(.md-table tbody tr):nth-child(odd) :global(.md-td) {
    background: color-mix(in srgb, var(--g2) 45%, transparent);
  }
  .content :global(.md-pre code) {
    font: 12.5px/1.7 Consolas, 'Cascadia Code', monospace;
    white-space: pre;
  }
  /* 模型把 <tool_call> 协议当正文吐出来的历史消息：折叠成可展开块，不铺满屏幕 */
  .content :global(.md-toolcall) {
    margin: 10px 0 14px;
    border: 1px solid var(--g2);
    border-radius: 10px;
    background: color-mix(in srgb, var(--g2) 45%, transparent);
    overflow: hidden;
  }
  .content :global(.md-toolcall summary) {
    padding: 7px 12px;
    font-size: 12.5px;
    color: var(--g5);
    cursor: pointer;
    user-select: none;
  }
  .content :global(.md-toolcall summary:hover) {
    color: var(--g7);
  }
  .content :global(.md-toolcall pre) {
    margin: 0;
    padding: 10px 12px;
    max-height: 320px;
    overflow: auto;
    border-top: 1px solid var(--g2);
    background: var(--g0);
    color: var(--g5);
    font: 12px/1.6 Consolas, 'Cascadia Code', monospace;
    white-space: pre-wrap;
    word-break: break-all;
  }
  .content :global(.md-quote) {
    margin: 10px 0;
    padding: 2px 14px;
    border-left: 2px solid var(--g4);
    color: var(--g6);
  }
  .content :global(.md-hr) {
    border: none;
    border-top: 1px solid var(--g2);
    margin: 16px 0;
  }
  .content :global(.md-a) {
    color: var(--brand-text);
    text-decoration: none;
    border-bottom: 1px solid color-mix(in srgb, var(--brand) 45%, transparent);
  }
  .content :global(.md-a:hover) {
    border-bottom-color: var(--brand);
  }
  /* markdown 图片（2026-10-31）：助手回复里的 ![alt](url) 直接显示，点开看大图。
     限制最大高度避免长图撑爆消息流；圆角与代码卡同一语言。 */
  .content :global(.md-img) {
    display: block;
    max-width: 100%;
    max-height: 420px;
    width: auto;
    height: auto;
    margin: 10px 0 14px;
    border: 1px solid var(--g2);
    border-radius: 10px;
    background: var(--g0);
    cursor: zoom-in;
    object-fit: contain;
  }
  .content :global(.md-img:hover) {
    border-color: var(--g4);
  }
  /* 用户消息不再画气泡（去掉气泡，纯文本）；
     模型回复也没有边框底色，工具/审批/思考块自己带边框即可 */
  .user-bubble {
    color: var(--g7);
    padding: 2px 0;
  }
  .asst-bubble {
    padding: 2px 0;
    max-width: 100%;
    width: 100%;
  }
  .msg.user .bubble,
  .msg.assistant .bubble {
    background: transparent;
    border: none;
    border-radius: 6px;
  }
  .error-row {
    justify-content: flex-start;
  }
  .startup-error {
    align-items: center;
    gap: 8px;
  }
  /* —— 消息操作/边缘时间/多选/信息卡（2026-10-26 用户需求批）——
     2026-10-27 布局重排：操作条从消息左侧/右侧挪到消息下方一行；时间贴行边，
     功能图标贴另一边。用户消息右对齐时时间仍在整行最左（视觉上是一条时间轴），
     模型回答的「回答详情」则落右下角。 */
  .msg-stack {
    display: flex;
    flex-direction: column;
    flex: 1 1 auto;
    min-width: 0;
  }
  .msg.user .msg-stack {
    align-items: flex-end;
  }
  .msg-meta {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    width: 100%;
    min-height: 18px;
    margin-top: 2px;
  }
  /* 用户消息的时间贴着编辑操作组，放它们左边（用户 2026-10-30）；
     助手消息保持时间在行左、详情在行右。 */
  .msg-meta.user-meta {
    justify-content: flex-end;
    gap: 6px;
  }
  .msg-meta.user-meta .msg-acts {
    margin-left: 0;
  }
  .msg-ts {
    font-size: 11px;
    color: var(--g5);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .msg-acts {
    display: flex;
    align-items: center;
    gap: 1px;
    margin-left: auto; /* 没有时间戳时也贴右边（回答详情的右下角） */
    opacity: 0.5;
    transition: opacity 0.12s;
  }
  .msg:hover .msg-acts,
  .msg:focus-within .msg-acts {
    opacity: 1;
  }
  .msg-act {
    display: flex;
    align-items: center;
    justify-content: center;
    border: none;
    background: transparent;
    color: var(--g4);
    font-size: 11px;
    padding: 3px 4px;
    border-radius: 6px;
    cursor: pointer;
    line-height: 1.4;
    white-space: nowrap;
  }
  .msg-act:hover {
    background: var(--g2);
    color: var(--g7);
  }
  .msg-act svg {
    width: 14px;
    height: 14px;
    display: block;
  }
  .msg.selecting {
    cursor: pointer;
  }
  .msg-check {
    display: flex;
    align-items: center;
    margin-right: 8px;
    align-self: center;
    flex-shrink: 0;
  }
  .msg-check input {
    width: 15px;
    height: 15px;
    cursor: pointer;
    /* 整行点击负责勾选：复选框只做指示，避免 label/change + 行 click 双触发 */
    pointer-events: none;
  }
  .edit-box {
    display: flex;
    flex-direction: column;
    gap: 8px;
    width: 78%;
  }
  .edit-area {
    width: 100%;
    background: var(--g1);
    color: var(--g7);
    border: 1px solid var(--g3);
    border-radius: 8px;
    padding: 8px 10px;
    font-size: 13.5px;
    line-height: 1.55;
    resize: vertical;
    min-height: 64px;
    box-sizing: border-box;
    font-family: inherit;
  }
  .edit-actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .edit-hint {
    font-size: 11px;
    color: var(--g4);
  }
  .select-bar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 24px;
    border-top: 1px solid var(--g2);
    background: var(--g1);
    font-size: 12.5px;
    color: var(--g6);
  }
  .select-flex {
    flex: 1;
  }
  .select-copied {
    color: var(--tint-green-fg);
  }
  .stats-modal {
    width: min(430px, 92vw);
  }
  .stats-grid {
    display: grid;
    grid-template-columns: 108px 1fr;
    gap: 9px 14px;
    padding: 14px 4px 6px;
    font-size: 13px;
    align-content: start;
  }
  .stats-grid > span {
    color: var(--g4);
  }
  .stats-grid > b {
    color: var(--g7);
    font-weight: 500;
    word-break: break-all;
    font-variant-numeric: tabular-nums;
  }
  .error-bubble {
    background: var(--tint-red-bg);
    border: 1px solid var(--tint-red-bg);
    color: var(--tint-red-fg);
    white-space: pre-wrap;
    max-width: 100%;
    padding: 4px 10px;
    border-radius: 8px;
    font-size: 12.5px;
    line-height: 1.55;
  }
  .sys-text {
    font-size: 12px;
    color: var(--g5);
    /* 系统行（[通知]/[错误]）统一居中：外层 .msg.system 只是把盒子居中，长句换行后
       盒内文字仍是左对齐，两条并排看就是「一条居中一条左齐」（2026-11-01 真机实测） */
    text-align: center;
  }
  /* —— 一轮的元信息与过程块（2026-10-30 排布重做）——
     思考/工具不再各占一堆卡片：按真实顺序合成「过程」，完成收成一行。 */
  .turn-bar {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0 0 8px;
    font-size: 11.5px;
    color: var(--g5);
    font-variant-numeric: tabular-nums;
    line-height: 1.4;
  }
  .turn-bar.live {
    color: var(--g6);
  }
  .turn-time {
    color: var(--g5);
  }
  .live-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--brand);
    flex: none;
    animation: live-pulse 1.6s ease-out infinite;
  }
  @keyframes live-pulse {
    0% {
      box-shadow: 0 0 0 0 color-mix(in srgb, var(--brand) 55%, transparent);
    }
    70% {
      box-shadow: 0 0 0 6px color-mix(in srgb, var(--brand) 0%, transparent);
    }
    100% {
      box-shadow: 0 0 0 0 color-mix(in srgb, var(--brand) 0%, transparent);
    }
  }
  .process {
    margin: 8px 0 12px;
    border: 1px solid var(--g2);
    border-radius: 10px;
    background: color-mix(in srgb, var(--g2) 52%, transparent);
    overflow: hidden;
  }
  .process.live {
    border-color: color-mix(in srgb, var(--brand) 22%, var(--g2));
  }
  .process-head {
    display: flex;
    align-items: center;
    gap: 7px;
    width: 100%;
    padding: 7px 10px;
    border: 0;
    background: transparent;
    color: var(--g6);
    font-size: 12.5px;
    font-family: inherit;
    text-align: left;
    cursor: pointer;
    user-select: none;
  }
  .process-head:hover {
    background: color-mix(in srgb, var(--g3) 45%, transparent);
    color: var(--g7);
  }
  /* 纯问答轮的同行样式（无过程可展开）：长得和过程行一样，只是不可点、无悬停反馈 */
  .process-head.static {
    cursor: default;
    user-select: text;
  }
  .process-head.static:hover {
    background: transparent;
    color: var(--g6);
  }
  .process-ico {
    width: 14px;
    height: 14px;
    flex: none;
    color: var(--g5);
  }
  .process-title {
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .process-live {
    display: inline-flex;
    margin-left: 2px;
  }
  .process-caret {
    width: 12px;
    height: 12px;
    flex: none;
    margin-left: auto;
    color: var(--g5);
    transition: transform 0.15s;
  }
  .process-caret.open {
    transform: rotate(180deg);
  }
  .process-body {
    padding: 0 10px 8px;
  }
  /* 完成态折叠进来的中途说明：保留 markdown 结构，但压暗、缩小，别和最终结语抢 */
  .process-note {
    margin: 4px 0 8px;
    padding-left: 2px;
    font-size: 13px;
    line-height: 1.7;
    color: var(--g5);
  }
  .process-note :global(strong) {
    color: var(--g6);
  }
  .think-item {
    position: relative;
    padding: 4px 2px 8px;
  }
  .think-label {
    display: flex;
    align-items: center;
    gap: 5px;
    width: 100%;
    margin: 0 0 2px;
    padding: 2px 0;
    border: 0;
    background: transparent;
    color: var(--g5);
    font-size: 11.5px;
    font-family: inherit;
    text-align: left;
    cursor: pointer;
    user-select: none;
  }
  .think-label:hover {
    color: var(--g7);
  }
  .think-caret {
    width: 11px;
    height: 11px;
    flex: none;
    margin-left: 1px;
    color: var(--g5);
    transition: transform 0.15s;
  }
  .think-caret.open {
    transform: rotate(180deg);
  }
  .think-live {
    padding: 0 5px;
    border-radius: 4px;
    background: color-mix(in srgb, var(--brand) 12%, transparent);
    color: var(--brand-text);
    font-size: 10.5px;
  }
  /* 流式期间封顶 240px（不随吐字跳变）；读完后超长封顶 280。
     开始没多少字时就是内容高度——攒到一定量才固定并滚动（用户 2026-10-01） */
  .think-text {
    padding: 1px 0 2px 12px;
    border-left: 2px solid var(--g3);
    font-size: 13px;
    line-height: 1.7;
    color: var(--g6);
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 280px;
    overflow-y: auto;
  }
  .think-text.streaming {
    max-height: 240px;
  }
  .think-jump {
    position: absolute;
    /* 让出 .think-text 自身滚动条的宽度（~12px），不然回到底部按钮压在滑条上（2026-10-31 实锤） */
    right: 18px;
    bottom: 12px;
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    padding: 0;
    border-radius: 50%;
    border: 1px solid var(--g4);
    background: var(--g2);
    color: var(--g7);
    cursor: pointer;
    box-shadow: 0 4px 12px rgb(0 0 0 / 28%);
  }
  .think-jump svg {
    width: 13px;
    height: 13px;
  }
  .think-jump:hover {
    background: var(--g3);
  }
  /* —— 工具行：一行一个动作，点开才是参数/结果 —— */
  .tool-item {
    border-radius: 7px;
  }
  .tool-item:hover {
    background: color-mix(in srgb, var(--g3) 30%, transparent);
  }
  .tool-row {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 5px 5px;
    border: 0;
    background: transparent;
    color: inherit;
    font-family: inherit;
    text-align: left;
    cursor: pointer;
    border-radius: 7px;
  }
  .tool-row:hover {
    background: color-mix(in srgb, var(--g3) 34%, transparent);
  }
  .tool-ico {
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    flex: none;
    border-radius: 6px;
    background: color-mix(in srgb, var(--g1) 80%, transparent);
    border: 1px solid color-mix(in srgb, var(--g3) 60%, transparent);
    color: var(--g5);
  }
  .tool-ico svg {
    width: 13px;
    height: 13px;
  }
  .tool-ico.running {
    color: var(--brand-text);
  }
  .tool-name {
    flex: none;
    font-size: 12.5px;
    font-weight: 550;
    color: var(--g7);
  }
  .tool-subject {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
    color: var(--g5);
    font-family: Consolas, 'Cascadia Code', monospace;
  }
  .tool-status {
    display: grid;
    place-items: center;
    width: 15px;
    height: 15px;
    flex: none;
    margin-left: auto;
    color: var(--g5);
  }
  .tool-status svg {
    width: 13px;
    height: 13px;
  }
  .tool-status.ok {
    color: var(--tint-green-fg);
    opacity: 0.85;
  }
  .tool-status.fail {
    color: var(--tint-red-fg);
  }
  .tool-spin {
    width: 11px;
    height: 11px;
    border-radius: 50%;
    border: 1.6px solid var(--g4);
    border-top-color: var(--brand);
    animation: tool-spin 0.8s linear infinite;
  }
  @keyframes tool-spin {
    to {
      transform: rotate(360deg);
    }
  }
  .tool-caret {
    width: 12px;
    height: 12px;
    flex: none;
    color: var(--g5);
    transition: transform 0.15s;
  }
  .tool-caret.open {
    transform: rotate(90deg);
  }
  .tool-detail {
    margin: 2px 4px 8px 34px;
    padding: 8px 10px;
    border: 1px solid var(--g2);
    border-radius: 8px;
    background: var(--g1);
  }
  .tool-sec + .tool-sec {
    margin-top: 8px;
  }
  .tool-sec-label {
    margin-bottom: 3px;
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.06em;
    color: var(--g5);
  }
  .tool-pre {
    margin: 0;
    padding: 8px 10px;
    border-radius: 6px;
    background: var(--g0);
    color: var(--g6);
    font: 12px/1.6 Consolas, 'Cascadia Code', monospace;
    white-space: pre-wrap;
    word-break: break-all;
    max-height: 200px;
    overflow: auto;
  }
  .tool-wait {
    font-size: 11.5px;
    color: var(--g5);
  }
  /* 审批卡（2026-11-03 重塑：跟随柔和主题，不再红框警示——严重度由档位徽章自己说话） */
  .approval-box {
    background: var(--g1);
    border: 1px solid var(--g3);
    border-left: 2px solid var(--brand);
    border-radius: 10px;
    padding: 10px 12px;
    font-size: 13px;
  }
  /* —— 阻塞式提问卡（ask 工具，2026-11-03）：与审批同一个 dock 槽位、同一主题 —— */
  .ask-box .ask-question {
    width: 100%;
    margin-top: 2px;
    font-size: 13.5px;
    line-height: 1.45;
  }
  .ask-multi-tag {
    padding: 1px 6px;
    border-radius: 4px;
    background: var(--tint-purple-bg);
    color: var(--tint-purple-fg);
    font-size: 11px;
  }
  .ask-options {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: 6px;
    margin-top: 8px;
  }
  .ask-opt {
    display: flex;
    flex-direction: column;
    gap: 2px;
    text-align: left;
    padding: 7px 9px;
    border: 1px solid var(--g3);
    border-radius: 8px;
    background: var(--g2);
    color: var(--g6);
    cursor: pointer;
    font-size: 12.5px;
  }
  .ask-opt:hover {
    border-color: var(--brand);
  }
  .ask-opt.sel {
    border-color: var(--brand);
    background: var(--tint-orange-bg, var(--g2));
    color: var(--g7);
  }
  .ask-opt-label {
    font-weight: 600;
    color: var(--g7);
  }
  .ask-opt-desc {
    font-size: 11.5px;
    color: var(--g6);
    line-height: 1.4;
  }
  .ask-other {
    display: flex;
    gap: 6px;
    align-items: center;
    margin-top: 8px;
  }
  .ask-other-in {
    flex: 1;
    min-width: 0;
    padding: 6px 9px;
    border: 1px solid var(--g3);
    border-radius: 8px;
    background: var(--g2);
    color: var(--g7);
    font-size: 12.5px;
  }
  /* —— 任务清单 dock（ZCode TodoWrite 同款语义，2026-11-03 贴输入框上方）——
     清单在最上、展开向上长；审批/提问在它下面贴输入框（阻塞交互优先）。
     展开体限高内部滚动，避免把对话区挤没或盖住提问。 */
  .todo-dock {
    margin: 0 0 8px;
    padding-bottom: 2px;
    border-bottom: 1px solid var(--g3);
    font-size: 13px;
    min-width: 0;
  }
  .todo-dock-row {
    display: flex;
    align-items: center;
    gap: 2px;
    min-width: 0;
  }
  .todo-head {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    min-width: 0;
    flex: 1;
    padding: 4px 2px;
    background: none;
    border: none;
    color: var(--g6);
    cursor: pointer;
    font-size: 12.5px;
  }
  .todo-x {
    width: 22px;
    height: 22px;
    flex: none;
    padding: 0;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--g6);
    font-size: 16px;
    line-height: 1;
    cursor: pointer;
  }
  .todo-x:hover {
    color: var(--g7);
    background: var(--g3);
  }
  .todo-dock .todo-body {
    max-height: min(32vh, 260px);
    overflow-y: auto;
    padding: 0 2px 6px;
  }
  .todo-head:hover {
    color: var(--g7);
  }
  .todo-ico {
    width: 15px;
    height: 15px;
    color: var(--brand);
  }
  .todo-title {
    font-weight: 600;
    color: var(--g7);
  }
  .todo-cur {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    text-align: left;
  }
  .todo-count {
    font-family: ui-monospace, monospace;
    font-size: 11.5px;
    color: var(--g6);
  }
  .todo-caret {
    width: 14px;
    height: 14px;
    transition: transform 0.15s;
  }
  .todo-caret.open {
    transform: rotate(180deg);
  }
  .todo-body {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 2px 12px 10px 12px;
  }
  .todo-item {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    line-height: 1.5;
  }
  .todo-mark {
    width: 15px;
    height: 15px;
    flex: none;
    margin-top: 2px;
    color: var(--g6);
  }
  .todo-mark svg {
    width: 15px;
    height: 15px;
  }
  .todo-item.completed .todo-mark {
    color: var(--tint-green-fg);
  }
  .todo-item.in_progress .todo-mark {
    color: var(--brand);
  }
  .todo-text {
    min-width: 0;
  }
  .todo-item.pending .todo-text {
    color: var(--g6);
  }
  .todo-item.in_progress .todo-text {
    color: var(--g7);
    font-weight: 600;
  }
  .todo-item.completed .todo-text {
    color: var(--g6);
    text-decoration: line-through;
    opacity: 0.75;
  }
  .approval-dock {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-bottom: 8px;
  }
  .approval-head {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
    color: var(--g7);
  }
  .approval-reason {
    display: inline-block;
    padding: 1px 6px;
    border-radius: 4px;
    background: var(--g2);
    color: var(--g6);
    font-size: 11px;
  }
  /* A4：风险档位徽章（安全/常规改动/高危操作，与 RiskLevel::label 对齐） */
  .approval-risk {
    display: inline-block;
    padding: 1px 6px;
    border-radius: 4px;
    font-size: 11px;
    font-weight: 600;
  }
  .approval-risk.risk-0 {
    background: var(--tint-green-bg);
    color: var(--tint-green-fg);
  }
  .approval-risk.risk-1 {
    background: var(--tint-blue-bg);
    color: var(--tint-blue-fg);
  }
  .approval-risk.risk-2 {
    background: var(--tint-red-bg);
    color: var(--tint-red-fg);
  }
  .approval-actions {
    display: flex;
    gap: 8px;
    margin-top: 8px;
    align-items: center;
  }
  .approval-state {
    margin-top: 6px;
    color: var(--g6);
    font-size: 12px;
  }
  .approval-hint {
    color: var(--g5);
    font-size: 11px;
    align-self: center;
  }
  /* 工具卡下的审批记录行：时间线位置还原（2026-11-03） */
  .tool-approval {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 12px 6px 36px;
    font-size: 11.5px;
    color: var(--g6);
    min-width: 0;
  }
  .tool-approval .ta-state {
    flex: none;
    color: var(--g7);
  }
  .tool-approval .ta-reason {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--g5);
  }

  /* 待发送附件条 */
  .composer-attachments {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    padding: 8px 12px 4px;
    border-bottom: 1px solid var(--g2);
  }
  .att-chip {
    display: flex;
    align-items: center;
    gap: 6px;
    background: var(--g2);
    border: 1px solid var(--g3);
    border-radius: 6px;
    padding: 3px 8px;
    font-size: 12px;
    color: var(--tint-blue-fg);
    max-width: 240px;
  }
  .att-thumb {
    width: 22px;
    height: 22px;
    object-fit: cover;
    border-radius: 4px;
  }
  .att-badge {
    font-size: 13px;
  }
  .att-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 120px;
  }
  .att-size {
    font-size: 10.5px;
    color: var(--g5);
  }
  .att-del {
    background: transparent;
    border: none;
    color: var(--g6);
    cursor: pointer;
    font-size: 11px;
    padding: 0 2px;
    line-height: 1;
  }
  .att-del:hover {
    color: var(--tint-red-fg);
  }

  /* 气泡内的附件渲染 */
  .msg-attachments {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end; /* 用户消息整体靠右，附件也要贴右缘 */
    gap: 8px;
    margin-bottom: 6px;
  }
  .msg-att-img {
    max-width: 280px;
    max-height: 200px;
    border-radius: 6px;
    border: 1px solid var(--veil);
    cursor: zoom-in;
    object-fit: contain;
    background: var(--g0);
  }
  .msg-att-video {
    max-width: 320px;
    max-height: 220px;
    border-radius: 6px;
  }
  .msg-att-audio {
    max-width: 320px;
    height: 38px;
    border-radius: 6px;
  }
  .msg-att-file {
    font-size: 12px;
    color: var(--tint-blue-fg);
    padding: 4px 8px;
    background: var(--veil);
    border-radius: 4px;
  }

  /* 缺 git 明说条（2026-10-06）：git 面板顶部直接讲清楚功能为什么没了 */
  .git-missing-note {
    padding: 8px 12px;
    font-size: 12.5px;
    line-height: 1.6;
    color: var(--tint-orange-fg);
    border-bottom: 1px solid var(--g2);
  }
  /* 基线陈旧警告（顶层目录被挪走造成的幽灵删除）：与 git-missing-note 同一只警示色，
     不带底边——它挂在清单上方，不是把横幅顶掉 */
  .rv-warn {
    margin: 8px 10px 0;
    padding: 8px 10px;
    font-size: 12px;
    line-height: 1.6;
    color: var(--tint-orange-fg);
    background: color-mix(in srgb, var(--tint-orange-fg) 10%, transparent);
    border: 1px solid color-mix(in srgb, var(--tint-orange-fg) 35%, transparent);
    border-radius: 6px;
  }
  .commit-msg {
    width: 100%;
    box-sizing: border-box;
    resize: vertical;
  }
  .commit-err {
    font-size: 12px;
    color: var(--tint-red-fg);
    white-space: pre-wrap;
    word-break: break-word;
  }
  /* —— git 面板状态行（分支 / HEAD / 领先落后）—— */
  .git-ref {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--g7);
    font-size: 12px;
    max-width: 260px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .git-ref b {
    font-weight: 600;
  }
  .git-ico {
    width: 13px;
    height: 13px;
    flex: 0 0 auto;
    color: var(--g5);
  }
  .git-head {
    color: var(--g5);
    font-family: monospace;
    font-size: 11px;
  }
  .git-ab {
    color: var(--g6);
    font-family: monospace;
    font-size: 11px;
  }
  /* —— git 面板（状态行 + 范围选择 + 提交盒 + 文件行）—— */
  .git-pane {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .git-status {
    flex: none;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px 6px;
  }
  /* 提交盒：未提交档顶部——一句话 + 「提交 N 个文件」；勾选在下面清单行上 */
  .commit-box {
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px 10px;
    border-bottom: 1px solid var(--g2);
    background: var(--g1);
  }
  .commit-msg {
    font-size: 12.5px;
    line-height: 1.5;
  }
  .commit-bar {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .commit-go {
    flex: none;
    background: var(--g3);
    border: 1px solid var(--g5);
    color: var(--g7);
    font-size: 12px;
    font-weight: 650;
    cursor: pointer;
    padding: 4px 12px;
    border-radius: 6px;
    font-family: inherit;
  }
  .commit-go:hover:not(:disabled) {
    background: var(--g4);
    border-color: var(--g6);
  }
  .commit-go:disabled {
    opacity: 0.55;
    cursor: default;
  }
  .commit-hint {
    font-size: 11px;
    color: var(--g5);
  }
  /* —— git 面板：范围选择/撤销/刷新 + 紧凑行 + 就地展开 diff —— */
  .rv-head {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    border-bottom: 1px solid var(--g2);
  }
  .rv-src {
    flex: none;
    max-width: 100%;
    background: var(--g1);
    color: var(--g7);
    border: 1px solid var(--g3);
    border-radius: 6px;
    padding: 4px 6px;
    font-size: 12px;
    font-family: inherit;
    cursor: pointer;
  }
  .rv-src:hover {
    border-color: var(--g4);
  }
  .rv-totals {
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11.5px;
    color: var(--g5);
    white-space: nowrap;
  }
  .rv-totals b {
    color: var(--g6);
  }
  .rv-tsep {
    flex: none;
    color: var(--g4);
    padding: 0 5px;
  }
  /* 增删计数（git 面板行与合计行共用） */
  .review-add {
    flex: none;
    color: var(--tint-green-fg);
    font-family: monospace;
    font-size: 11.5px;
  }
  .review-del {
    flex: none;
    color: var(--tint-red-fg);
    font-family: monospace;
    font-size: 11.5px;
  }
  /* 刷新：转圈图标，加载中不重复点 */
  .rv-btn {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 5px;
    background: transparent;
    border: none;
    color: var(--g6);
    font-size: 12px;
    font-family: inherit;
    padding: 4px 6px;
    border-radius: 6px;
    cursor: pointer;
  }
  .rv-btn:hover {
    background: var(--g2);
    color: var(--g7);
  }
  .rv-btn svg {
    width: 15px;
    height: 15px;
  }
  .rv-btn.spin svg {
    animation: rv-spin 0.9s linear infinite;
  }
  @keyframes rv-spin {
    to {
      transform: rotate(360deg);
    }
  }
  .rv-list {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 2px 0;
  }
  .rv-empty {
    margin: auto;
    text-align: center;
    color: var(--g5);
    font-size: 12.5px;
    line-height: 1.7;
    padding: 24px 16px;
  }
  .rv-empty svg {
    width: 30px;
    height: 30px;
    color: var(--g4);
    margin-bottom: 8px;
  }
  .rv-empty-t {
    font-size: 13.5px;
    font-weight: 600;
    color: var(--g6);
  }
  .rv-empty-d {
    font-size: 12px;
    margin-top: 3px;
  }
  /* 行：紧凑行——文件名粗体 + 目录暗色，右侧增删 + 展开箭头。
     未提交档行首多一个提交勾选框：行改 flex，勾选框不参与伸缩、文件按钮吃掉余量、
     diff 展开块整行换行（flex-basis:100%），收起/展开两种态都不破版 */
  .rv-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    border-bottom: 1px solid var(--g2);
  }
  .rv-row:last-child {
    border-bottom: none;
  }
  .rv-check {
    flex: none;
    margin: 0 2px 0 8px;
    cursor: pointer;
    accent-color: var(--g5);
  }
  .rv-file {
    flex: 1 1 auto;
    display: flex;
    align-items: center;
    gap: 7px;
    width: 100%;
    min-width: 0;
    height: 30px;
    padding: 0 10px;
    background: transparent;
    border: none;
    color: var(--g7);
    font-size: 12.5px;
    font-family: inherit;
    text-align: left;
    cursor: pointer;
  }
  .rv-file:hover {
    background: var(--g2);
  }
  .rv-row.open .rv-file {
    background: var(--g2);
  }
  .rv-file .rv-fico {
    flex: none;
    width: 14px;
    height: 14px;
    color: var(--g5);
  }
  .rv-file.added .rv-fico {
    color: var(--tint-green-fg);
  }
  .rv-file.deleted .rv-fico {
    color: var(--tint-red-fg);
  }
  .rv-fname {
    flex: none;
    font-weight: 650;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .rv-fdir {
    flex: 1 1 auto;
    min-width: 0;
    font-size: 11px;
    color: var(--g5);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .rv-stat {
    flex: none;
    display: flex;
    align-items: center;
    gap: 7px;
    font-family: monospace;
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
  }
  .rv-chev {
    flex: none;
    width: 13px;
    height: 13px;
    color: var(--g5);
    transition: transform 0.12s;
  }
  .rv-row.open .rv-chev {
    transform: rotate(180deg);
  }
  /* 就地展开的 diff：左右行号列 + 增删着色 */
  .rv-diffwrap {
    flex: 1 1 100%; /* 父行是 flex-wrap：展开的 diff 独占一行，不跟勾选框/文件名并排 */
    background: var(--g0);
    border-top: 1px solid var(--g2);
    max-height: 460px;
    overflow: auto;
  }
  .rv-diffnote {
    padding: 10px 12px;
    font-size: 12px;
    color: var(--g5);
  }
  .rv-diff {
    margin: 0;
    padding: 8px 0;
    font-family: Consolas, 'Courier New', monospace;
    font-size: 12px;
    line-height: 1.6;
    min-width: max-content;
  }
  .rv-dl {
    display: flex;
    align-items: stretch;
  }
  .rv-dno {
    flex: none;
    width: 38px;
    padding-right: 9px;
    text-align: right;
    color: var(--g4);
    user-select: none;
    font-variant-numeric: tabular-nums;
  }
  .rv-dtx {
    white-space: pre;
    padding-right: 14px;
    color: var(--g6);
  }
  .rv-dl-add {
    background: color-mix(in srgb, var(--tint-green-bg) 55%, transparent);
  }
  .rv-dl-add .rv-dtx {
    color: var(--tint-green-fg);
  }
  .rv-dl-add .rv-dno {
    color: color-mix(in srgb, var(--tint-green-fg) 55%, var(--g4));
  }
  .rv-dl-del {
    background: color-mix(in srgb, var(--tint-red-bg) 55%, transparent);
  }
  .rv-dl-del .rv-dtx {
    color: var(--tint-red-fg);
  }
  .rv-dl-del .rv-dno {
    color: color-mix(in srgb, var(--tint-red-fg) 55%, var(--g4));
  }
  .rv-dl-hunk {
    background: var(--g2);
  }
  .rv-dl-hunk .rv-dtx {
    color: var(--tint-blue-fg);
    padding-left: 4px;
  }
  .rv-dl-hunk .rv-dno {
    color: var(--g4);
  }
  .rv-dl-meta .rv-dtx {
    color: var(--g5);
    padding-left: 4px;
  }
  /* 行右键菜单 */
  .rv-menu-back {
    position: fixed;
    inset: 0;
    z-index: 90;
  }
  .rv-menu {
    position: fixed;
    z-index: 91;
    min-width: 132px;
    background: var(--g1);
    border: 1px solid var(--g3);
    border-radius: 8px;
    box-shadow: 0 10px 30px var(--overlay);
    padding: 4px;
    display: flex;
    flex-direction: column;
  }
  .rv-menu button {
    background: transparent;
    border: none;
    color: var(--g7);
    font-size: 12.5px;
    font-family: inherit;
    text-align: left;
    padding: 6px 9px;
    border-radius: 5px;
    cursor: pointer;
  }
  .rv-menu button:hover {
    background: var(--g2);
  }
  /* 审批卡里仍要显示原始工具参数（工具调用行已改成人话说明） */
  .approval-box .tool-args {
    color: var(--g5);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 320px;
  }
  /* —— 子代理（2026-10-20 第 2 轮；主区实时条 + 居中小弹窗，不落侧边栏）—— */
  .subagent-strip {
    margin: 0 16px 8px;
    padding: 8px 10px;
    background: var(--g2);
    border: 1px solid var(--g3);
    border-radius: 8px;
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .subagent-strip-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    font-size: 12px;
    color: var(--g6);
  }
  .subagent-strip-title {
    font-weight: 600;
  }
  .subagent-strip-more {
    background: none;
    border: none;
    color: var(--tint-blue-fg);
    font-size: 12px;
    cursor: pointer;
    padding: 2px 4px;
  }
  .subagent-strip-more:hover {
    text-decoration: underline;
  }
  .subagent-row {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
  }
  .subagent-row-cancel {
    margin-left: auto;
    background: none;
    border: none;
    color: var(--g5);
    font-size: 14px;
    line-height: 1;
    cursor: pointer;
    padding: 0 4px;
  }
  .subagent-row-cancel:hover {
    color: var(--tint-red-fg);
  }
  .subagent-chiprow {
    display: flex;
    justify-content: center;
    padding: 0 16px 6px;
  }
  /* 可收起的 chip：整颗是一枚胶囊，左边按钮开面板、右边 × 收起 */
  .chip-pill {
    display: inline-flex;
    align-items: center;
    background: var(--g2);
    border: 1px solid var(--g3);
    border-radius: 999px;
  }
  .chip-pill .subagent-chip {
    background: transparent;
    border: 0;
    padding-right: 4px;
  }
  .chip-x {
    background: none;
    border: 0;
    color: var(--g5);
    font-size: 13px;
    line-height: 1;
    padding: 2px 9px 2px 0;
    cursor: pointer;
  }
  .chip-x:hover {
    color: var(--tint-red-fg);
  }
  .subagent-chip {
    background: var(--g2);
    border: 1px solid var(--g3);
    border-radius: 999px;
    color: var(--g6);
    font-size: 11px;
    padding: 2px 10px;
    cursor: pointer;
  }
  .subagent-chip:hover {
    color: var(--tint-blue-fg);
    border-color: var(--tint-blue-fg);
  }
  .subagent-badge {
    flex: none;
    padding: 1px 6px;
    border-radius: 4px;
    font-size: 11px;
  }
  .subagent-badge.queued {
    background: var(--g3);
    color: var(--tint-amber-fg, #d4a017);
  }
  .subagent-badge.running {
    background: var(--g3);
    color: var(--tint-blue-fg);
  }
  .subagent-badge.done {
    background: var(--tint-green-bg);
    color: var(--tint-green-fg);
  }
  .subagent-badge.cancelled {
    background: var(--g3);
    color: var(--g5);
  }
  .subagent-badge.timeout,
  .subagent-badge.error {
    background: var(--tint-red-bg);
    color: var(--tint-red-fg);
  }
  .subagent-label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--g6);
  }
  .subagent-modal {
    min-width: 460px;
    max-width: 640px;
  }
  .subagent-modal-count {
    margin-left: auto;
    font-size: 12px;
    font-weight: 400;
    color: var(--g5);
  }
  .subagent-list {
    max-height: 55vh;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .subagent-empty {
    color: var(--g5);
    font-size: 12px;
    line-height: 1.7;
    padding: 8px 2px;
  }
  .subagent-item {
    border: 1px solid var(--g3);
    border-radius: 8px;
    padding: 8px 10px;
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .subagent-item-head {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
  }
  .subagent-model {
    margin-left: auto;
    font-size: 11px;
    color: var(--g5);
    font-family: Consolas, monospace;
    max-width: 180px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .subagent-item-meta {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 11px;
    color: var(--g5);
  }
  .subagent-meta.dim {
    font-family: Consolas, monospace;
    opacity: 0.7;
  }
  .subagent-item-cancel,
  .subagent-item-open {
    margin-left: auto;
    background: none;
    border: 1px solid var(--g4);
    border-radius: 5px;
    font-size: 11px;
    padding: 2px 8px;
    cursor: pointer;
    color: var(--g6);
  }
  .subagent-item-cancel:hover {
    color: var(--tint-red-fg);
    border-color: var(--tint-red-fg);
  }
  .subagent-item-open:hover {
    color: var(--tint-blue-fg);
    border-color: var(--tint-blue-fg);
  }
  .subagent-foot {
    margin-top: 10px;
    font-size: 11px;
    color: var(--g5);
    line-height: 1.6;
  }
  /* —— 操作电脑（2026-10-20 第 4 轮；同子代理纪律：主区、不落侧边栏）—— */
  .computer-strip {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    padding: 7px 10px;
    background: var(--g2);
    border: 1px solid var(--g3);
    border-radius: 8px;
  }
  .computer-badge {
    flex: none;
    padding: 1px 6px;
    border-radius: 4px;
    font-size: 11px;
    background: var(--g3);
    color: var(--tint-blue-fg);
  }
  .computer-action {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--g6);
  }
  .computer-strip-more {
    margin-left: auto;
    flex: none;
    background: none;
    border: none;
    color: var(--tint-blue-fg);
    font-size: 12px;
    cursor: pointer;
    padding: 2px 4px;
  }
  .computer-strip-more:hover {
    text-decoration: underline;
  }
  .computer-modal {
    min-width: 640px;
    max-width: 860px;
  }
  /* —— 多文件夹项目管理弹窗 —— */
  .folders-modal {
    width: 560px;
  }
  .folders-body {
    margin-top: 6px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .folders-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px;
    border: 1px solid var(--g3);
    border-radius: 8px;
    background: var(--g1);
  }
  .folders-row > svg {
    width: 17px;
    height: 17px;
    color: var(--g5);
    flex-shrink: 0;
  }
  .folders-info {
    flex: 1;
    min-width: 0;
  }
  .folders-info b {
    font-size: 12.5px;
    color: var(--g7);
  }
  .folders-path {
    font-size: 11px;
    color: var(--g5);
    font-family: Consolas, monospace;
    word-break: break-all;
    line-height: 1.5;
  }
  .folders-tag {
    font-size: 11px;
    color: var(--g5);
    flex-shrink: 0;
  }
  .folders-empty {
    color: var(--g5);
    font-size: 12px;
    line-height: 1.7;
    padding: 10px 2px;
  }
  .folders-foot {
    margin-top: 10px;
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .folders-foot .hint {
    flex: 1;
    text-align: right;
  }
  .computer-body {
    margin-top: 4px;
    min-height: 240px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .computer-empty {
    color: var(--g5);
    font-size: 12px;
    padding: 24px;
    text-align: center;
    line-height: 1.7;
  }
  .computer-log {
    max-height: 50vh;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .computer-log-item {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .computer-log-row {
    font-size: 11px;
    color: var(--g6);
    font-family: Consolas, monospace;
    padding: 4px 6px;
    background: var(--g2);
    border-radius: 5px;
    word-break: break-all;
    text-align: left;
    border: none;
    cursor: default;
  }
  /* 截图条目可点（展开/收起）；最后一次操作描个高亮边，一眼看到最新的 */
  .computer-log-row.shot {
    cursor: pointer;
    display: flex;
    gap: 8px;
    align-items: baseline;
  }
  .computer-log-row.shot:hover {
    color: var(--g7);
  }
  .computer-log-row.last {
    border: 1px solid var(--brand);
    color: var(--g7);
  }
  .clr-text {
    flex: 1;
    min-width: 0;
  }
  .clr-more {
    flex: none;
    color: var(--brand);
    font-size: 10.5px;
    white-space: nowrap;
  }
  .computer-shot-img {
    max-width: 100%;
    max-height: 46vh;
    display: block;
    border: 1px solid var(--g3);
    border-radius: 8px;
    margin: 0 2px 4px;
  }
  .typing {
    color: var(--g6);
  }
  /* —— 输入区：悬浮在对话区底部（无分隔线，消息从其下方穿过）—— */
  .chat-input {
    position: absolute;
    bottom: 0;
    /* 左右各让 14px（输入框离侧边栏/边缘左右间隔对称）；
       右侧 14px 同时给聊天区滚动条留位——输入框是悬浮层，滑条从背后穿过 */
    left: 14px;
    right: 14px;
    z-index: 5;
    padding: 12px 0 16px;
    background: transparent;
    pointer-events: none; /* 空白区域点击穿透到底下的消息 */
  }
  .chat-input > * {
    pointer-events: auto;
    width: 100%;
    max-width: 880px;
    margin-left: auto;
    margin-right: auto;
    padding-left: 24px;
    padding-right: 24px;
    box-sizing: border-box;
  }
  .composer {
    background: var(--g2);
    border: 1px solid var(--g3);
    border-radius: 16px;
    padding: 6px 12px 10px;
    transition: border-color 0.15s, box-shadow 0.15s;
    box-shadow: 0 4px 16px var(--overlay);
  }
  .composer:focus-within {
    border-color: color-mix(in srgb, var(--brand) 45%, var(--g3));
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--brand) 12%, transparent), 0 6px 20px var(--overlay);
  }
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
    color: var(--brand-text);
  }
  .composer-input:disabled {
    opacity: 0.6;
  }
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
  .icon-btn {
    width: 28px;
    height: 28px;
    border-radius: 7px;
    border: none;
    background: transparent;
    color: var(--g6);
    font-size: 16px;
    line-height: 1;
    cursor: pointer;
  }
  .icon-btn:hover {
    /* 输入框自身是 g2 底：hover 再用 g2 完全看不见，改半透明提亮 */
    background: color-mix(in srgb, var(--g4) 45%, transparent);
    color: var(--g7);
  }
  .icon-btn:disabled {
    opacity: 0.4;
    cursor: default;
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
    border: none;
    background: transparent;
    color: var(--g6);
    font-size: 12.5px;
    cursor: pointer;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .pill:hover,
  .pill.active {
    background: var(--g2);
    color: var(--g7);
  }
  /* 完全访问档只留红字作静态警示：边框去掉（2026-10-07 真机实测「把红色框
     去掉」——整个输入栏就这一块挂红框太扎眼；档位含义有 title 与菜单说明讲）。 */
  .pill.danger {
    color: var(--tint-red-fg);
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
    background: var(--g2);
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
    background: var(--g3);
    color: var(--g7);
  }
  /* —— 档位滑杆：结构/动效自 ZCode ComposerModelMenu + composer-thought-slider.css 复刻 ——
     几何：滑块块 32px、轨道 24px（top 4）、圆钮 28px（行程 100%-28px）、档点 4px inset 14px；
     动效：圆钮/填充 120ms ease-out（拖动也保留——离散档位切换不跳变，ZCode 源码注释同款）、
     渐变层 200ms 透明度、档点悬停 scale 1.75。
     2026-11-01 两处定稿偏离 ZCode：①配色跟随主色橙（非 ZCode 绿蓝）；
     ②粒子全档流动，速度随档位升高加快（ZCode 只在最高档有气泡）。 */
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
    background: var(--brand); /* 主色调橙（颜色跟随主色，不用 ZCode 的绿蓝） */
    overflow: hidden;
    transition: width 120ms ease-out;
  }
  /* 最高档渐变层：陶土橙由深锈到亮橙（同族色相收紧，不掺白——掺白会发粉发淡）。
     纯色与渐变无法直接插值，用常驻层透明度过渡（200ms），反向调档也不瞬切（ZCode 结构同款） */
  .ts-fill::before {
    content: '';
    position: absolute;
    inset: 0;
    border-radius: inherit;
    opacity: 0;
    transition: opacity 200ms ease-out;
    background: linear-gradient(90deg, #8f5a44 0%, #ba8a78 45%, #cf9367 100%);
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
    animation: ts-bubble-flow linear infinite;
  }
  .ts-bubble-dot {
    display: block;
    border-radius: 50%;
    background: #fff;
  }
  @keyframes ts-bubble-flow {
    0% {
      transform: translateX(104%);
      opacity: 0;
    }
    15%,
    80% {
      opacity: 0.55;
    }
    100% {
      transform: translateX(-4%);
      opacity: 0;
    }
  }
  /* 档点：未过=灰、已过=白 0.3、悬停放大点亮（ZCode 同款三态） */
  .ts-stops {
    position: absolute;
    left: 14px;
    right: 14px;
    top: 0;
    bottom: 0;
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .ts-stop {
    width: 4px;
    height: 4px;
    border-radius: 50%;
    flex-shrink: 0;
    background: color-mix(in srgb, var(--g7) 30%, transparent);
    scale: 1;
    transition:
      scale 120ms ease-out,
      opacity 120ms ease-out;
  }
  .ts-stop.filled {
    background: #fff;
    opacity: 0.3;
  }
  .ts-stop.hovered {
    scale: 1.75;
    opacity: 1;
  }
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
  /* @ 弹窗的 对话/文件 页签 */
  .ctx-tabs {
    display: flex;
    gap: 6px;
    margin-bottom: 8px;
  }
  .ctx-tab {
    border: 1px solid var(--g3);
    background: transparent;
    color: var(--g6);
    border-radius: 999px;
    padding: 4px 14px;
    font-size: 12.5px;
    cursor: pointer;
  }
  .ctx-tab.active {
    background: var(--g3);
    color: var(--g7);
    border-color: var(--g4);
  }
  /* 对话行（用户 2026-10-02 第五轮）：块级上下两行——标题一行（超长省略），
     时间一行小字。不再用行内 flex（挤压问题反复踩） */
  .ctx-file-conv {
    display: block;
    padding: 7px 10px;
    font-family: inherit;
  }
  .ctx-conv-title {
    display: block;
    width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12.5px;
    line-height: 1.45;
  }
  .ctx-conv-ts {
    display: block;
    margin-top: 1px;
    font-size: 11px;
    line-height: 1.3;
    color: var(--g5);
  }
  /* 用户气泡上的「对话 · 标题」引用标签 */
  .msg-convs {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 6px;
  }
  .conv-tag {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-size: 11.5px;
    padding: 2px 9px;
    border-radius: 999px;
    border: 1px solid var(--g4);
    color: var(--g6);
    max-width: 260px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .conv-chip .att-name {
    max-width: 200px;
  }
  .bar-dropdown.compact {
    min-width: 90px;
  }
  /* 插件行 ▾ 箭头 + 展开后的技能子项（2026-10-02） */
  .menu-caret {
    width: 13px;
    height: 13px;
    flex: none;
    align-self: center;
    color: var(--g5);
    transition: transform 0.12s;
  }
  .bar-item.active .menu-caret {
    transform: rotate(90deg);
  }
  .bar-item-sub {
    padding-left: 28px;
  }
  /* 菜单行右侧的弱化小字（MCP 行显示工具数） */
  .menu-sub {
    margin-left: 8px;
    font-size: 11px;
    color: var(--g4, #8a8f98);
  }
  .bar-dropdown.compact .bar-item {
    justify-content: center;
    padding: 6px 14px;
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
    background: var(--g2);
  }
  /* 模型快选：配置分组头 + 子项 */
  .bar-item.group-head {
    font-weight: 650;
    color: var(--g6);
    align-items: center;
  }
  .bar-item.group-head.active {
    color: var(--g7);
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
  .ctx-modal {
    width: 620px;
    height: 70vh;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .ctx-search {
    background: var(--g1);
    color: var(--g7);
    border: 1px solid var(--g3);
    border-radius: 8px;
    padding: 8px 10px;
    font-size: 13px;
    outline: none;
  }
  .ctx-search:focus {
    border-color: var(--brand);
  }
  .ctx-file-list {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .ctx-file {
    display: block;
    width: 100%;
    text-align: left;
    padding: 7px 10px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--g6);
    font-family: Consolas, monospace;
    font-size: 12.5px;
    line-height: 1.5;
    cursor: pointer;
    /* flex 列容器里 overflow:hidden 的子项自动最小尺寸归零，会把行压成一条
       横切片（标题与时间戳糊成乱码、且列表滚不动）；不参与压缩才保持原高 */
    flex-shrink: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* 对话行用正文字体（列表是会话标题不是代码路径） */
  .ctx-file:hover {
    background: var(--g2);
    color: var(--g7);
  }
  .bar-item.active {
    background: var(--g3);
    color: var(--g7);
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
  /* —— 工作区切换 —— */
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
  /* 设置分区（左导航 + 内容区） */
  .settings-modal {
    width: 880px;
    max-width: 94vw;
    /* 固定高度：所有设置页统一窗口大小（以最高的「模型配置」页为准），短页留白，内容区自行滚动 */
    height: 85vh;
  }
  .settings-body {
    display: flex;
    gap: 14px;
    flex: 1;
    min-height: 0;
  }
  .settings-nav {
    display: flex;
    flex-direction: column;
    gap: 6px;
    width: 150px;
    flex-shrink: 0;
  }
  .settings-nav button {
    text-align: left;
    padding: 9px 12px;
    border-radius: 8px;
    background: transparent;
    border: none;
    color: var(--g6);
    cursor: pointer;
    font-size: 13px;
  }
  .settings-nav button:hover {
    background: var(--g2);
    color: var(--g7);
  }
  .settings-nav button.active {
    background: var(--g3);
    color: var(--g7);
    font-weight: 600;
  }
  .settings-content {
    flex: 1;
    overflow-y: auto;
    padding-right: 4px;
    min-width: 0;
    /* 各分区之间留白，避免标题/说明/控件挤成一团 */
    display: flex;
    flex-direction: column;
    gap: 20px;
  }
  .mem-stats {
    font-size: 12px;
    color: var(--g6);
    display: flex;
    flex-direction: column;
    gap: 7px;
  }
  .agents-editor {
    width: 100%;
    box-sizing: border-box;
    font-family: Consolas, 'Courier New', monospace;
    font-size: 12.5px;
    line-height: 1.7;
    background: var(--g1);
    color: var(--g7);
    border: 1px solid var(--g3);
    border-radius: 8px;
    padding: 12px 14px;
    resize: vertical;
  }
  .agents-editor:focus {
    outline: none;
    border-color: var(--g4);
  }
  /* MCP 插件分区 */
  .mcp-card {
    border: 1px solid var(--g3);
    border-radius: 10px;
    padding: 10px 12px;
  }
  .mcp-head {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    color: var(--g7);
  }
  .mcp-status {
    font-size: 11px;
    color: var(--g5);
  }
  .mcp-risk {
    font-size: 11px;
    color: var(--tint-yellow-fg);
    background: var(--tint-yellow-bg);
    border-radius: 4px;
    padding: 1px 6px;
  }
  .mcp-tools-list {
    font-size: 12px;
    color: var(--g6);
    display: flex;
    flex-direction: column;
    gap: 3px;
    margin: 6px 0;
  }
  .mcp-confirm {
    margin-top: 6px;
  }
  .mcp-risk-label {
    font-size: 12px;
    color: var(--g6);
  }
  .modal-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 10px;
  }
  /* 标题 + 刷新等操作靠左成组（刷新按钮原来被 space-between 挤到中间） */
  .mh-left {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  /* —— 插件页（用户 2026-09-30 指示：紧凑行——单行 + 开关，Git/沙箱并入「内置能力」，撤独立「运行状态」卡） ——
     plug-row / switch 一族样式已下沉 ui.css 全局：技能页、MCP 页（用户 2026-10-02 要求
     与插件页同形式）也在用，跨组件共用 */
</style>


