<script>
  // 记忆星图（2D 版）：项目 = 星座，会话 = 星点。
  // 数据/检索走 /api/memory/*；布局与绘制全在 2D canvas，坐标由 galaxy/atlas.js 决定。
  import { onMount, onDestroy, createEventDispatcher, tick } from 'svelte';
  import { buildAtlas, mulberry32 } from './galaxy/atlas.js';

  const dispatch = createEventDispatcher();
  export let currentSessionId = '';

  const TAU = Math.PI * 2;
  const reduceMotion =
    typeof window !== 'undefined' && window.matchMedia
      ? window.matchMedia('(prefers-reduced-motion: reduce)').matches
      : false;

  // 暖夜色：全屏恒为夜幕，不跟主题切换
  const PAL = {
    core: '#fff2de',
    grid: 'rgba(219, 205, 186, 0.042)',
    lineAlpha: 0.3,
    vig: 'rgba(8, 6, 5, 0.62)',
    halo: 'rgba(104, 54, 26, 0.40)',
  };

  let hostEl;
  let uiCanvas;
  let ctx = null;

  let loading = true;
  let errMsg = '';
  let notice = '';
  let items = [];
  let stats = null;

  // 场景
  let clusters = [];
  let stars = [];
  let starById = new Map();
  let clusterByName = new Map();
  let bounds = { minX: 0, maxX: 0, minY: 0, maxY: 0 };
  let dust = [];
  let sprites = new Map();

  // 视图
  let W = 0;
  let H = 0;
  let DPR = 1;
  let bgGrad = null;
  let haloGrad = null;
  let vigGrad = null;
  let grainPattern = null;
  const cam = { cx: 0, cy: 0, s: 1 };
  const camT = { cx: 0, cy: 0, s: 1 };
  let fitS = 1;
  let userMoved = false;
  let screenPos = new Map();
  let fitted = false;

  // 交互
  let hoverId = null;
  let selected = null;
  let dragging = false;
  let dragMoved = 0;
  let lastX = 0;
  let lastY = 0;

  // 检索
  let query = '';
  let searching = false;
  let searched = false;
  let results = [];
  let matchIds = null;
  let focusProj = null;
  let searchInfo = '';
  let searchNotice = '';

  let reindexing = false;
  let destroyed = false;
  let raf = 0;
  let lastDraw = 0;

  function rgba(hex, a) {
    const h = hex.replace('#', '');
    const r = parseInt(h.slice(0, 2), 16);
    const g = parseInt(h.slice(2, 4), 16);
    const b = parseInt(h.slice(4, 6), 16);
    return `rgba(${r},${g},${b},${a})`;
  }

  function mixHex(a, b, t) {
    const pa = a.replace('#', '');
    const pb = b.replace('#', '');
    const mix = (i) =>
      Math.round(
        parseInt(pa.slice(i, i + 2), 16) + (parseInt(pb.slice(i, i + 2), 16) - parseInt(pa.slice(i, i + 2), 16)) * t,
      )
        .toString(16)
        .padStart(2, '0');
    return `#${mix(0)}${mix(2)}${mix(4)}`;
  }

  function clamp(v, lo, hi) {
    return Math.max(lo, Math.min(hi, v));
  }

  function relTime(iso) {
    if (!iso) return '';
    const t = Date.parse(iso);
    if (Number.isNaN(t)) return '';
    const m = Math.floor((Date.now() - t) / 60000);
    if (m < 1) return '刚刚';
    if (m < 60) return `${m} 分钟前`;
    const h = Math.floor(m / 60);
    if (h < 24) return `${h} 小时前`;
    const d = Math.floor(h / 24);
    if (d < 30) return `${d} 天前`;
    return new Date(t).toLocaleDateString();
  }

  function nodeById(id) {
    return starById.get(id) || null;
  }

  // —— 场景与视图 ——
  function rebuild() {
    const sc = buildAtlas(items);
    clusters = sc.clusters;
    stars = sc.stars;
    bounds = sc.bounds;
    starById = new Map(stars.map((s) => [s.id, s]));
    clusterByName = new Map(clusters.map((c) => [c.name, c]));
    for (const c of clusters) c.core = mixHex(PAL.core, c.color, 0.26);
    for (const s of stars) {
      const c = clusterByName.get(s.cluster);
      s.core = c ? c.core : PAL.core;
    }
    buildSprites();
    buildDust();
  }

  function buildSprites() {
    sprites = new Map();
    for (const c of clusters) {
      const cv = document.createElement('canvas');
      cv.width = 64;
      cv.height = 64;
      const g = cv.getContext('2d');
      const grad = g.createRadialGradient(32, 32, 0, 32, 32, 32);
      grad.addColorStop(0, rgba(c.color, 0.75));
      grad.addColorStop(0.28, rgba(c.color, 0.22));
      grad.addColorStop(1, rgba(c.color, 0));
      g.fillStyle = grad;
      g.fillRect(0, 0, 64, 64);
      sprites.set(c.color, cv);
    }
  }

  function buildDust() {
    const rnd = mulberry32(777);
    const cx = (bounds.minX + bounds.maxX) / 2;
    const cy = (bounds.minY + bounds.maxY) / 2;
    const w = Math.max(900, (bounds.maxX - bounds.minX) * 1.8);
    const h = Math.max(700, (bounds.maxY - bounds.minY) * 1.8);
    dust = [];
    for (let i = 0; i < 420; i++) {
      dust.push({
        x: cx + (rnd() - 0.5) * w,
        y: cy + (rnd() - 0.5) * h,
        r: 0.5 + rnd() * 0.8,
        a: 0.12 + rnd() * 0.26,
        phase: rnd() * TAU,
      });
    }
  }

  function buildGradients() {
    if (!ctx || !W || !H) return;
    bgGrad = ctx.createRadialGradient(W * 0.5, H * 0.42, 0, W * 0.5, H * 0.42, Math.max(W, H) * 0.78);
    bgGrad.addColorStop(0, '#181310');
    bgGrad.addColorStop(1, '#090706');
    haloGrad = ctx.createRadialGradient(W * 0.52, H * 0.06, 0, W * 0.52, H * 0.06, H * 0.95);
    haloGrad.addColorStop(0, PAL.halo);
    haloGrad.addColorStop(1, 'rgba(0,0,0,0)');
    vigGrad = ctx.createRadialGradient(W * 0.5, H * 0.5, Math.min(W, H) * 0.32, W * 0.5, H * 0.5, Math.max(W, H) * 0.76);
    vigGrad.addColorStop(0, 'rgba(0,0,0,0)');
    vigGrad.addColorStop(1, PAL.vig);
  }

  function buildGrain() {
    if (!ctx) return;
    const size = 128;
    const cv = document.createElement('canvas');
    cv.width = size;
    cv.height = size;
    const g = cv.getContext('2d');
    const img = g.createImageData(size, size);
    const rnd = mulberry32(2026);
    for (let i = 0; i < img.data.length; i += 4) {
      if (rnd() > 0.72) {
        img.data[i] = 255;
        img.data[i + 1] = 255;
        img.data[i + 2] = 255;
        img.data[i + 3] = Math.floor(4 + rnd() * 12);
      }
    }
    g.putImageData(img, 0, 0);
    grainPattern = ctx.createPattern(cv, 'repeat');
  }

  function w2s(x, y) {
    return { x: (x - cam.cx) * cam.s + W / 2, y: (y - cam.cy) * cam.s + H / 2 };
  }

  /** 可视区（扣掉顶栏、左栏、底部提示），返回 { left, top, aw, ah } */
  function viewportBox() {
    const narrow = W < 760;
    const left = narrow ? 20 : 224;
    const right = 20;
    const top = narrow ? 118 : 82;
    const bottom = 66;
    return { left, top, aw: Math.max(80, W - left - right), ah: Math.max(80, H - top - bottom) };
  }

  function fit() {
    if (!W || !stars.length) return;
    const { left, top, aw, ah } = viewportBox();
    const bw = Math.max(120, bounds.maxX - bounds.minX) + 120;
    const bh = Math.max(120, bounds.maxY - bounds.minY) + 120;
    fitS = clamp(Math.min(aw / bw, ah / bh), 0.06, 4);
    camT.s = fitS;
    camT.cx = (bounds.minX + bounds.maxX) / 2 - (left + aw / 2 - W / 2) / fitS;
    camT.cy = (bounds.minY + bounds.maxY) / 2 - (top + ah / 2 - H / 2) / fitS;
    cam.cx = camT.cx;
    cam.cy = camT.cy;
    cam.s = camT.s;
    userMoved = false;
  }

  function fitToStars(list, factor = 1.6) {
    if (!W || !list.length) return;
    let minX = Infinity;
    let maxX = -Infinity;
    let minY = Infinity;
    let maxY = -Infinity;
    for (const s of list) {
      minX = Math.min(minX, s.x);
      maxX = Math.max(maxX, s.x);
      minY = Math.min(minY, s.y);
      maxY = Math.max(maxY, s.y);
    }
    const { left, top, aw, ah } = viewportBox();
    const bw = Math.max(160, maxX - minX);
    const bh = Math.max(160, maxY - minY);
    const s = clamp(Math.min(aw / (bw * factor), ah / (bh * factor)), fitS * 0.55, fitS * 6);
    camT.s = s;
    camT.cx = (minX + maxX) / 2 - (left + aw / 2 - W / 2) / s;
    camT.cy = (minY + maxY) / 2 - (top + ah / 2 - H / 2) / s;
    userMoved = true;
  }

  function resetView() {
    selected = null;
    fit();
  }

  function resize() {
    if (!hostEl || !uiCanvas) return;
    const r = hostEl.getBoundingClientRect();
    W = Math.max(320, r.width);
    H = Math.max(240, r.height);
    DPR = Math.min(2, window.devicePixelRatio || 1);
    uiCanvas.width = Math.round(W * DPR);
    uiCanvas.height = Math.round(H * DPR);
    uiCanvas.style.width = `${W}px`;
    uiCanvas.style.height = `${H}px`;
    buildGradients();
    if (!fitted && stars.length) {
      fitted = true;
      fit();
    } else if (!userMoved) {
      fit();
    }
    // 改画布尺寸会清空内容：当场补一帧，避免尺寸变化后闪一下空屏
    if (ctx && stars.length) drawFrame(performance.now());
  }

  // —— 数据 ——
  const MODE_LABEL = (m, model) =>
    m === 'external' ? `向量检索 · ${model}` : m === 'local' ? '本地检索（未配置嵌入渠道）' : '关键词检索';

  async function load(force = false) {
    loading = true;
    errMsg = '';
    try {
      const r = await fetch(force ? '/api/memory/library/reindex' : '/api/memory/library', {
        method: force ? 'POST' : 'GET',
      });
      const d = await r.json();
      if (destroyed) return;
      items = d.items || [];
      stats = d.stats || null;
      notice = d.notice || '';
      rebuild();
      await tick();
      if (destroyed) return;
      resize();
      resetView();
    } catch (e) {
      errMsg = `记忆库加载失败：${e}`;
    } finally {
      loading = false;
    }
  }

  async function doSearch() {
    const q = query.trim();
    if (!q) {
      clearSearch();
      return;
    }
    searching = true;
    searchNotice = '';
    try {
      const r = await fetch('/api/memory/search', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ query: q, top_k: 60 }),
      });
      const d = await r.json();
      if (destroyed) return;
      results = d.results || [];
      searchNotice = d.notice || '';
      matchIds = new Set(results.map((x) => x.id));
      focusProj = null;
      searched = true;
      selected = null;
      const label = MODE_LABEL(d.mode, d.model);
      searchInfo = `${results.length} 个星点 · ${label}`;
      if (d.mode === 'local') searchNotice = '设置 → 功能桥接 → 向量嵌入，可开启真语义检索';
      const pts = results.map((x) => nodeById(x.id)).filter(Boolean);
      if (pts.length) fitToStars(pts, 1.55);
      if (!results.length) searchInfo = `没有匹配「${q}」的会话 · ${label}`;
    } catch (e) {
      searchInfo = `检索失败：${e}`;
    } finally {
      searching = false;
    }
  }

  async function doSimilar() {
    if (!selected) return;
    const src = selected;
    searching = true;
    searchNotice = '';
    query = '';
    focusProj = null;
    try {
      const r = await fetch('/api/memory/search', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ like: src.id, top_k: 24 }),
      });
      const d = await r.json();
      if (destroyed) return;
      results = d.results || [];
      matchIds = new Set(results.map((x) => x.id));
      matchIds.add(src.id);
      searched = true;
      searchInfo = `与「${src.title}」相似的会话 · ${results.length} 个`;
      const pts = [...results.map((x) => nodeById(x.id)).filter(Boolean), src];
      if (pts.length > 1) fitToStars(pts, 1.55);
      if (!results.length) searchInfo = `没有找到与「${src.title}」相近的会话`;
    } catch (e) {
      searchInfo = `相似检索失败：${e}`;
    } finally {
      searching = false;
    }
  }

  function clearSearch() {
    query = '';
    searched = false;
    results = [];
    matchIds = null;
    focusProj = null;
    searchInfo = '';
    searchNotice = '';
    selected = null;
    resetView();
  }

  function focusProject(c) {
    matchIds = new Set(c.stars.map((s) => s.id));
    focusProj = c.name;
    searched = false;
    results = [];
    selected = null;
    searchInfo = `${c.name} · ${c.count} 个会话`;
    fitToStars(c.stars, 1.5);
  }

  function pickProject(c) {
    if (focusProj === c.name) clearSearch();
    else focusProject(c);
  }

  function focusResult(hit) {
    const s = nodeById(hit.id);
    if (!s) return;
    camT.cx = s.x;
    camT.cy = s.y;
    camT.s = Math.max(camT.s, fitS * 2.1);
    selected = s;
    hoverId = null;
  }

  function openNode(s) {
    dispatch('open', s.id);
  }

  // —— 亮度 ——
  function clusterAlpha(c) {
    let a = 1;
    if (focusProj && c.name !== focusProj) a *= 0.16;
    if (matchIds && !c.stars.some((s) => matchIds.has(s.id))) a *= 0.16;
    return a;
  }

  function starAlpha(s) {
    let a = s.bright;
    if (focusProj && s.cluster !== focusProj) a *= 0.14;
    if (matchIds) a *= matchIds.has(s.id) ? 1 : 0.14;
    return a;
  }

  // —— 绘制 ——
  function drawFrame(now) {
    const t = reduceMotion ? 0 : now / 1000;
    ctx.setTransform(DPR, 0, 0, DPR, 0, 0);
    ctx.clearRect(0, 0, W, H);
    ctx.fillStyle = bgGrad;
    ctx.fillRect(0, 0, W, H);
    ctx.fillStyle = haloGrad;
    ctx.fillRect(0, 0, W, H);

    // 每个项目一团极淡的环境光，给天空冷暖层次
    for (const c of clusters) {
      const ca = clusterAlpha(c);
      if (ca < 0.06) continue;
      const p = w2s(c.cx ?? c.x, c.cy ?? c.y);
      const rad = c.span * 0.95 * cam.s;
      if (rad < 24) continue;
      const g = ctx.createRadialGradient(p.x, p.y, 0, p.x, p.y, rad);
      g.addColorStop(0, rgba(c.color, 0.07 * ca));
      g.addColorStop(0.55, rgba(c.color, 0.03 * ca));
      g.addColorStop(1, rgba(c.color, 0));
      ctx.fillStyle = g;
      ctx.fillRect(p.x - rad, p.y - rad, rad * 2, rad * 2);
    }

    // 坐标环：以场景中心为极点，随星图平移缩放
    const ox = (bounds.minX + bounds.maxX) / 2;
    const oy = (bounds.minY + bounds.maxY) / 2;
    const o = w2s(ox, oy);
    ctx.strokeStyle = PAL.grid;
    ctx.lineWidth = 0.7;
    for (const r of [180, 360, 540, 720, 900, 1080, 1260, 1440]) {
      ctx.beginPath();
      ctx.arc(o.x, o.y, r * cam.s, 0, TAU);
      ctx.stroke();
    }
    for (let a = 0; a < 16; a++) {
      const ang = (a / 16) * TAU;
      ctx.beginPath();
      ctx.moveTo(o.x + Math.cos(ang) * 80 * cam.s, o.y + Math.sin(ang) * 80 * cam.s);
      ctx.lineTo(o.x + Math.cos(ang) * 1480 * cam.s, o.y + Math.sin(ang) * 1480 * cam.s);
      ctx.stroke();
    }

    if (grainPattern) {
      ctx.fillStyle = grainPattern;
      ctx.fillRect(0, 0, W, H);
    }

    // 背景细星
    for (const d of dust) {
      const p = w2s(d.x, d.y);
      if (p.x < -4 || p.x > W + 4 || p.y < -4 || p.y > H + 4) continue;
      const tw = 0.75 + 0.25 * Math.sin(t * 0.5 + d.phase);
      ctx.fillStyle = `rgba(236,226,211,${d.a * tw})`;
      ctx.beginPath();
      ctx.arc(p.x, p.y, d.r, 0, TAU);
      ctx.fill();
    }

    // 星座连线（按缩放分档：总览只连最重要的十几颗，放大后逐级补齐）
    const ratio = fitS ? cam.s / fitS : 1;
    const budget = ratio < 1.4 ? 14 : ratio < 2.0 ? 30 : 1e9;
    screenPos.clear();
    for (const c of clusters) {
      const ca = clusterAlpha(c);
      const edges = budget <= 14 ? c.topEdges : budget <= 30 ? c.midEdges : c.edges;
      const glow = 0.1 * ca;
      if (ca > 0.12 && glow > 0.004) {
        ctx.strokeStyle = rgba(c.color, glow);
        ctx.lineWidth = 4;
        ctx.beginPath();
        for (const e of edges) {
          const a = w2s(e[0].x, e[0].y);
          const b = w2s(e[1].x, e[1].y);
          ctx.moveTo(a.x, a.y);
          ctx.lineTo(b.x, b.y);
        }
        ctx.stroke();
      }
      if (ca > 0.12) {
        ctx.strokeStyle = rgba(c.color, PAL.lineAlpha * ca);
        ctx.lineWidth = 0.9;
        ctx.beginPath();
        for (const e of edges) {
          const a = w2s(e[0].x, e[0].y);
          const b = w2s(e[1].x, e[1].y);
          ctx.moveTo(a.x, a.y);
          ctx.lineTo(b.x, b.y);
        }
        ctx.stroke();
      }
    }

    // 星点：星等之外的会话只留一点淡尘，放大后才点亮
    for (const s of stars) {
      const p = w2s(s.x, s.y);
      if (p.x < -40 || p.x > W + 40 || p.y < -40 || p.y > H + 40) continue;
      let a = starAlpha(s);
      if (a < 0.02) continue;
      const hot =
        (matchIds && matchIds.has(s.id)) ||
        selected?.id === s.id ||
        hoverId === s.id ||
        s.id === currentSessionId;
      if (!(s.rank < budget) && !hot) {
        ctx.fillStyle = `rgba(236,226,211,${0.07 * a})`;
        ctx.beginPath();
        ctx.arc(p.x, p.y, 0.8, 0, TAU);
        ctx.fill();
        continue;
      }
      const tw = reduceMotion ? 1 : 0.86 + 0.14 * Math.sin(t * 0.9 + s.phase);
      a *= tw;
      const isHover = hoverId === s.id;
      const isSel = selected?.id === s.id;
      const r = s.size * (isHover ? 1.45 : 1) * (isSel ? 1.2 : 1);
      screenPos.set(s.id, { x: p.x, y: p.y, r });

      const sp = sprites.get(s.color);
      if (sp) {
        const sz = r * 12;
        ctx.globalAlpha = Math.min(1, a * 1.05);
        ctx.drawImage(sp, p.x - sz / 2, p.y - sz / 2, sz, sz);
        ctx.globalAlpha = 1;
      }
      // 2026-11-05：亮星原来画一对横竖「十字星芒」线，看起来像被划了一刀，
      // 和星座连线混在一起分不清——已去掉。层次交给 sprite 光晕 + 实心核。

      ctx.fillStyle = s.core;
      ctx.globalAlpha = Math.min(1, a);
      ctx.beginPath();
      ctx.arc(p.x, p.y, r, 0, TAU);
      ctx.fill();
      if (r > 3.4) {
        ctx.fillStyle = `rgba(255,252,246,${0.85 * a})`;
        ctx.beginPath();
        ctx.arc(p.x, p.y, r * 0.42, 0, TAU);
        ctx.fill();
      }
      ctx.globalAlpha = 1;

      if (isSel) {
        ctx.strokeStyle = rgba(s.color, 0.55);
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.arc(p.x, p.y, r + 7, 0, TAU);
        ctx.stroke();
        ctx.strokeStyle = rgba(s.color, 0.18);
        ctx.beginPath();
        ctx.arc(p.x, p.y, r + 11, 0, TAU);
        ctx.stroke();
      } else if (s.id === currentSessionId) {
        ctx.strokeStyle = rgba(s.color, 0.3);
        ctx.lineWidth = 0.9;
        ctx.beginPath();
        ctx.arc(p.x, p.y, r + 4, 0, TAU);
        ctx.stroke();
      }
    }

    // 暗角
    ctx.fillStyle = vigGrad;
    ctx.fillRect(0, 0, W, H);

  }

  function draw(now) {
    raf = requestAnimationFrame(draw);
    if (!now || !ctx) return;
    if (now - lastDraw < 1000 / 30) return;
    lastDraw = now;
    cam.cx += (camT.cx - cam.cx) * 0.16;
    cam.cy += (camT.cy - cam.cy) * 0.16;
    cam.s += (camT.s - cam.s) * 0.16;
    if (Math.abs(camT.cx - cam.cx) < 0.02) cam.cx = camT.cx;
    if (Math.abs(camT.cy - cam.cy) < 0.02) cam.cy = camT.cy;
    if (Math.abs(camT.s - cam.s) < 0.0004) cam.s = camT.s;
    drawFrame(now);
  }

  // —— 指针 ——
  function hitTest(clientX, clientY) {
    const rect = uiCanvas.getBoundingClientRect();
    const mx = clientX - rect.left;
    const my = clientY - rect.top;
    let best = null;
    let bestD = Infinity;
    for (const s of stars) {
      const p = screenPos.get(s.id);
      if (!p) continue;
      if (starAlpha(s) < 0.05) continue;
      const d = Math.hypot(p.x - mx, p.y - my);
      const lim = Math.max(12, p.r * 2.4);
      if (d < lim && d < bestD) {
        bestD = d;
        best = s;
      }
    }
    return best;
  }

  function pointerDown(e) {
    dragging = true;
    dragMoved = 0;
    lastX = e.clientX;
    lastY = e.clientY;
    uiCanvas.setPointerCapture?.(e.pointerId);
  }

  function pointerMove(e) {
    if (dragging) {
      const dx = e.clientX - lastX;
      const dy = e.clientY - lastY;
      lastX = e.clientX;
      lastY = e.clientY;
      dragMoved += Math.abs(dx) + Math.abs(dy);
      cam.cx -= dx / cam.s;
      cam.cy -= dy / cam.s;
      camT.cx = cam.cx;
      camT.cy = cam.cy;
      userMoved = true;
      hoverId = null;
      return;
    }
    const s = hitTest(e.clientX, e.clientY);
    hoverId = s ? s.id : null;
  }

  function pointerUp(e) {
    if (!dragging) return;
    dragging = false;
    uiCanvas.releasePointerCapture?.(e.pointerId);
    if (dragMoved < 6) {
      const s = hitTest(e.clientX, e.clientY);
      selected = s || null;
    }
  }

  function wheel(e) {
    e.preventDefault();
    const rect = uiCanvas.getBoundingClientRect();
    const mx = e.clientX - rect.left;
    const my = e.clientY - rect.top;
    const wx = camT.cx + (mx - W / 2) / camT.s;
    const wy = camT.cy + (my - H / 2) / camT.s;
    const next = clamp(camT.s * Math.exp(-e.deltaY * 0.0015), fitS * 0.45, fitS * 6);
    camT.s = next;
    camT.cx = wx - (mx - W / 2) / next;
    camT.cy = wy - (my - H / 2) / next;
    userMoved = true;
  }

  function onKey(e) {
    if (e.key === 'Escape') {
      if (selected) selected = null;
      else if (searched) clearSearch();
      else dispatch('close');
    }
  }

  $: modeLabel = stats
    ? (stats.mode === 'external' ? `嵌入 ${stats.model}` : '本地向量') +
      ` · 已索引 ${stats.indexed}/${stats.sessions}` +
      (stats.pending > 0 ? `（${stats.pending} 条待索引）` : '')
    : '';

  $: statusText = searchInfo || modeLabel;

  onMount(() => {
    ctx = uiCanvas.getContext('2d');
    buildGrain();
    resize();
    load();
    const onResize = () => resize();
    window.addEventListener('resize', onResize);
    raf = requestAnimationFrame(draw);
    return () => {
      window.removeEventListener('resize', onResize);
      cancelAnimationFrame(raf);
      destroyed = true;
    };
  });

  onDestroy(() => {
    destroyed = true;
    cancelAnimationFrame(raf);
  });
</script>

<svelte:window on:keydown={onKey} />

<div class="galaxy" bind:this={hostEl}>
  <canvas
    class="gx-canvas"
    class:hover={!!hoverId}
    bind:this={uiCanvas}
    on:pointerdown={pointerDown}
    on:pointermove={pointerMove}
    on:pointerup={pointerUp}
    on:pointerleave={() => (hoverId = null)}
    on:contextmenu|preventDefault={() => {}}
    on:wheel={wheel}
    on:dblclick={resetView}
    aria-label="记忆星图：每个项目是一组星座，每个会话是一颗星"
  ></canvas>

  <header class="gx-head">
    <div class="gx-title">
      <span class="gx-mark" aria-hidden="true"></span>
      <b>记忆库</b>
      <span class="gx-sub">{stats ? `${stats.sessions} 个会话 · ${stats.projects} 个项目` : ''}</span>
    </div>
    <div class="gx-search">
      <input
        bind:value={query}
        placeholder="搜索记忆，例如「安装器单文件」"
        on:keydown={(e) => e.key === 'Enter' && doSearch()}
        spellcheck="false"
      />
      <button class="gx-btn" on:click={doSearch} disabled={searching}>{searching ? '检索中…' : '搜索'}</button>
      {#if searched}
        <button class="gx-btn ghost" on:click={clearSearch}>清除</button>
      {/if}
    </div>
    <div class="gx-tools">
      <button
        class="gx-btn"
        on:click={() => {
          reindexing = true;
          load(true).finally(() => (reindexing = false));
        }}
        disabled={reindexing}
      >
        {reindexing ? '重建中…' : '重新索引'}
      </button>
      <button class="gx-btn ghost" on:click={() => dispatch('close')} title="关闭（Esc）">✕</button>
    </div>
  </header>

  {#if clusters.length}
    <aside class="gx-rail" aria-label="项目列表">
      <div class="gx-rail-head">
        <span>项目</span>
        {#if matchIds}
          <button class="gx-mini" on:click={clearSearch}>显示全部</button>
        {/if}
      </div>
      <div class="gx-rail-list">
        {#each clusters as c (c.name)}
          <button class="gx-proj" class:on={focusProj === c.name} on:click={() => pickProject(c)} title={`只看「${c.name}」的会话`}>
            <span class="gx-dot" style="background:{c.color};box-shadow:0 0 8px {c.color}"></span>
            <span class="gx-proj-name">{c.name}</span>
            <span class="gx-count">{c.count}</span>
          </button>
        {/each}
      </div>
    </aside>
  {/if}

  <div class="gx-status" class:warn={!!notice}>
    {statusText}{#if notice}<span class="gx-warn"> · {notice}</span>{/if}
  </div>

  <div class="gx-hint">拖动平移 · 滚轮放大看更多星 · 双击复位 · 点星看详情</div>

  <div class="gx-side">
    {#if searched}
      <aside class="gx-results">
        <div class="gx-results-head">
          检索结果 <b>{results.length}</b>
          <span class="flex-sp"></span>
          <button class="gx-mini" on:click={clearSearch}>显示全部</button>
        </div>
        <div class="gx-results-body">
          {#each results as r, i (r.id)}
            {@const n = nodeById(r.id)}
            <button class="gx-hit" class:active={selected?.id === r.id} on:click={() => focusResult(r)} on:dblclick={() => n && openNode(n)}>
              <span class="gx-rank">{i + 1}</span>
              <span class="gx-hit-main">
                <span class="gx-hit-title">{n ? n.title : r.id}</span>
                <span class="gx-hit-snip">{r.snippet}</span>
                <span class="gx-hit-meta">{n ? `${n.project} · ${relTime(n.updated_at)}` : ''}</span>
              </span>
              <span class="gx-score" title={`综合 ${r.score.toFixed(2)} · 向量 ${r.vector.toFixed(2)} · 词元 ${r.lexical.toFixed(2)}`}>{Math.round(r.score * 100)}</span>
            </button>
          {:else}
            <div class="gx-empty">没有匹配的会话</div>
          {/each}
        </div>
      </aside>
    {/if}

    {#if selected}
      <aside class="gx-card">
        <div class="gx-card-title">{selected.title}</div>
        <div class="gx-card-meta">
          {selected.project} · {relTime(selected.updated_at)} · {selected.count} 条消息{#if selected.archived} · 已归档{/if}
        </div>
        <div class="gx-card-snip">{selected.snippet || '（这条会话还没有摘要）'}</div>
        <div class="gx-card-actions">
          <button class="gx-btn primary" on:click={() => openNode(selected)}>打开会话</button>
          <button class="gx-btn" on:click={doSimilar} disabled={searching} title="用向量找和这条会话相近的记忆">相似会话</button>
          <button class="gx-btn ghost" on:click={() => (selected = null)}>关闭</button>
        </div>
      </aside>
    {/if}
  </div>


  {#if loading}
    <div class="gx-center">正在铺开星图…</div>
  {:else if errMsg}
    <div class="gx-center err">{errMsg}</div>
  {:else if !items.length}
    <div class="gx-center">还没有会话记录，去聊一句，它会成为星图里的一颗星</div>
  {/if}
</div>

<style>
  /* 记忆星图恒为暖夜色（不随主题切换） */
  .galaxy {
    --gx-ink: #ece2d3;
    --gx-dim: #a2958a;
    --gx-faint: #786d62;
    --gx-line: rgba(236, 226, 211, 0.1);
    --gx-line-strong: rgba(236, 226, 211, 0.2);
    --gx-panel: rgba(21, 17, 14, 0.72);
    --gx-panel-solid: #1a1512;
    --gx-accent: #c98d72;
    --gx-accent-soft: rgba(201, 141, 114, 0.14);
    --gx-accent-line: rgba(201, 141, 114, 0.36);
    position: fixed;
    inset: 0;
    z-index: 300;
    background: #0d0b09;
    overflow: hidden;
    user-select: none;
    -webkit-user-select: none;
    color: var(--gx-ink);
  }
  .gx-canvas {
    position: absolute;
    inset: 0;
    display: block;
    cursor: grab;
    touch-action: none;
  }
  .gx-canvas.hover {
    cursor: pointer;
  }
  .gx-canvas:active {
    cursor: grabbing;
  }
  .flex-sp {
    flex: 1;
  }

  .gx-head {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    height: 64px;
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 0 18px;
    background: linear-gradient(180deg, rgba(13, 11, 9, 0.94), rgba(13, 11, 9, 0.55) 70%, rgba(13, 11, 9, 0));
    z-index: 4;
  }
  .gx-title {
    display: flex;
    align-items: center;
    gap: 9px;
    color: var(--gx-ink);
    font-size: 14.5px;
    letter-spacing: 0.06em;
    white-space: nowrap;
  }
  .gx-title b {
    font-weight: 500;
  }
  .gx-mark {
    width: 15px;
    height: 15px;
    border-radius: 50%;
    border: 1px solid var(--gx-accent-line);
    position: relative;
    flex: none;
  }
  .gx-mark::after {
    content: '';
    position: absolute;
    inset: 4px;
    border-radius: 50%;
    background: var(--gx-accent);
    box-shadow: 0 0 9px var(--gx-accent);
  }
  .gx-sub {
    color: var(--gx-faint);
    font-size: 11px;
    letter-spacing: 0;
  }
  .gx-search {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: 0 1 380px;
    min-width: 0;
  }
  .gx-search input {
    flex: 1;
    min-width: 0;
    height: 30px;
    padding: 0 10px;
    background: var(--gx-panel);
    border: 1px solid var(--gx-line);
    border-radius: 8px;
    color: var(--gx-ink);
    font-family: inherit;
    font-size: 12.5px;
    outline: none;
    backdrop-filter: blur(10px);
    transition: border-color 0.15s;
  }
  .gx-search input::placeholder {
    color: var(--gx-faint);
  }
  .gx-search input:focus {
    border-color: var(--gx-accent-line);
  }
  .gx-tools {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-left: auto;
  }
  .gx-btn {
    height: 28px;
    padding: 0 10px;
    border-radius: 7px;
    border: 1px solid var(--gx-line);
    background: var(--gx-panel);
    color: var(--gx-dim);
    font-family: inherit;
    font-size: 12px;
    cursor: pointer;
    backdrop-filter: blur(10px);
    transition: color 0.15s, border-color 0.15s, background 0.15s;
  }
  .gx-btn:hover:not(:disabled) {
    color: var(--gx-ink);
    border-color: var(--gx-line-strong);
  }
  .gx-btn:disabled {
    opacity: 0.55;
    cursor: default;
  }
  .gx-btn.ghost {
    background: transparent;
    border-color: transparent;
  }
  .gx-btn.ghost:hover {
    background: var(--gx-panel);
  }
  .gx-btn.primary {
    background: var(--gx-accent-soft);
    border-color: var(--gx-accent-line);
    color: var(--gx-ink);
  }

  .gx-rail {
    position: absolute;
    left: 18px;
    top: 80px;
    width: 194px;
    max-height: calc(100% - 160px);
    display: flex;
    flex-direction: column;
    padding: 10px 8px;
    border-radius: 12px;
    background: var(--gx-panel);
    border: 1px solid var(--gx-line);
    backdrop-filter: blur(14px);
    z-index: 3;
  }
  .gx-rail-head {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 8px 6px;
    font-size: 10px;
    letter-spacing: 0.18em;
    color: var(--gx-faint);
  }
  .gx-mini {
    background: none;
    border: none;
    color: var(--gx-faint);
    font-family: inherit;
    font-size: 11px;
    letter-spacing: 0;
    cursor: pointer;
    padding: 0;
    margin-left: auto;
  }
  .gx-mini:hover {
    color: var(--gx-ink);
    text-decoration: underline;
  }
  .gx-rail-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
    overflow: auto;
  }
  .gx-proj {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    height: 30px;
    padding: 0 8px;
    border: 0;
    border-radius: 7px;
    background: transparent;
    color: var(--gx-dim);
    font-family: inherit;
    font-size: 12.5px;
    text-align: left;
    cursor: pointer;
  }
  .gx-proj:hover {
    background: rgba(255, 245, 230, 0.05);
    color: var(--gx-ink);
  }
  .gx-proj.on {
    background: var(--gx-accent-soft);
    color: var(--gx-ink);
  }
  .gx-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex: none;
  }
  .gx-proj-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .gx-count {
    font-size: 11px;
    color: var(--gx-faint);
    font-variant-numeric: tabular-nums;
  }

  .gx-status {
    position: absolute;
    left: 18px;
    bottom: 16px;
    max-width: min(560px, 50vw);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 11.5px;
    color: var(--gx-faint);
    z-index: 3;
  }
  .gx-status.warn {
    color: #d8b48a;
  }
  .gx-warn {
    color: #d8b48a;
  }
  .gx-hint {
    position: absolute;
    left: 50%;
    bottom: 16px;
    transform: translateX(-50%);
    font-size: 11px;
    color: var(--gx-faint);
    white-space: nowrap;
    z-index: 3;
  }

  .gx-side {
    position: absolute;
    right: 18px;
    top: 80px;
    width: min(340px, 32vw);
    max-height: calc(100% - 150px);
    display: flex;
    flex-direction: column;
    gap: 10px;
    pointer-events: none;
    z-index: 3;
  }
  .gx-results,
  .gx-card {
    pointer-events: auto;
    background: var(--gx-panel);
    border: 1px solid var(--gx-line);
    border-radius: 14px;
    backdrop-filter: blur(14px);
    box-shadow: 0 18px 50px rgba(0, 0, 0, 0.35);
    min-height: 0;
  }
  .gx-results {
    flex: 1;
    max-height: min(64vh, 620px);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .gx-results-head {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 11px 13px;
    color: var(--gx-dim);
    font-size: 12px;
    border-bottom: 1px solid var(--gx-line);
  }
  .gx-results-head b {
    color: var(--gx-accent);
    font-weight: 500;
  }
  .gx-results-body {
    overflow: auto;
    padding: 6px;
  }
  .gx-empty {
    padding: 16px 10px;
    text-align: center;
    font-size: 12px;
    color: var(--gx-faint);
  }
  .gx-hit {
    display: flex;
    gap: 9px;
    width: 100%;
    padding: 8px 9px;
    border: 1px solid transparent;
    border-radius: 10px;
    background: transparent;
    color: inherit;
    text-align: left;
    cursor: pointer;
  }
  .gx-hit:hover,
  .gx-hit.active {
    background: var(--gx-accent-soft);
    border-color: var(--gx-accent-line);
  }
  .gx-rank {
    flex: none;
    width: 18px;
    padding-top: 2px;
    color: var(--gx-faint);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }
  .gx-hit-main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .gx-hit-title {
    color: var(--gx-ink);
    font-size: 12.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .gx-hit-snip {
    color: var(--gx-dim);
    font-size: 11.5px;
    line-height: 1.5;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .gx-hit-meta {
    color: var(--gx-faint);
    font-size: 10.5px;
  }
  .gx-score {
    flex: none;
    padding-top: 1px;
    color: var(--gx-accent);
    font-size: 11.5px;
  }

  .gx-card {
    flex: none;
    max-height: min(46vh, 420px);
    overflow: auto;
    padding: 14px 15px;
  }
  .gx-card-title {
    font-size: 13.5px;
    font-weight: 500;
    line-height: 1.5;
  }
  .gx-card-meta {
    margin-top: 5px;
    font-size: 11px;
    color: var(--gx-faint);
  }
  .gx-card-snip {
    margin-top: 10px;
    font-size: 11.8px;
    line-height: 1.7;
    color: var(--gx-dim);
    max-height: 132px;
    overflow: auto;
  }
  .gx-card-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 12px;
  }

  .gx-center {
    position: absolute;
    left: 50%;
    top: 50%;
    transform: translate(-50%, -50%);
    color: var(--gx-dim);
    font-size: 14px;
    text-align: center;
    line-height: 1.9;
    max-width: 460px;
    z-index: 2;
  }
  .gx-center.err {
    color: #e0a08f;
  }

  @media (max-width: 760px) {
    .gx-sub {
      display: none;
    }
    .gx-search {
      flex: 1;
    }
    .gx-rail {
      left: 10px;
      right: 10px;
      top: 68px;
      width: auto;
      max-height: none;
      padding: 5px;
    }
    .gx-rail-head {
      display: none;
    }
    .gx-rail-list {
      flex-direction: row;
      flex-wrap: nowrap;
      gap: 2px;
      overflow-x: auto;
      overflow-y: hidden;
      scrollbar-width: none;
    }
    .gx-rail-list::-webkit-scrollbar {
      display: none;
    }
    .gx-proj {
      flex: none;
      width: auto;
      height: 26px;
      padding: 0 8px;
      font-size: 11.5px;
      white-space: nowrap;
    }
    .gx-proj-name {
      max-width: 148px;
    }
    .gx-count {
      display: none;
    }
    .gx-side {
      left: 10px;
      right: 10px;
      top: auto;
      bottom: 34px;
      width: auto;
      max-height: 52%;
    }
    .gx-results {
      max-height: 40vh;
    }
    .gx-hint {
      display: none;
    }
    .gx-status {
      left: 12px;
      bottom: 10px;
      max-width: calc(100% - 24px);
      font-size: 11px;
    }
  }
</style>
