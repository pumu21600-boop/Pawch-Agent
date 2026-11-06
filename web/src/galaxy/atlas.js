// 记忆星图的 2D 布局：项目 = 星座（锚点 + 星点链），会话 = 星点。
// 全部确定性（项目名 / 会话 id 哈希），刷新、重开都不跳位。
// 与旧的 3D 星云布局的区别：没有体积、没有相机，只有平面坐标与像素级星点。

const TAU = Math.PI * 2;

// 项目配色：暖色为主，掺几个冷色做区分；同一项目名永远同色
const CLUSTER_COLORS = [
  '#d09a7d',
  '#d5b57e',
  '#c6b490',
  '#c4a2b5',
  '#b7a894',
  '#b8a6bd',
  '#b5b0a0',
  '#c8a3a3',
];

export function hashStr(s) {
  let h = 2166136261;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

export function mulberry32(a) {
  return function () {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** 均匀最佳候选采样 + 最小间距松弛：每颗星试 14 个候选、挑离已有点最远的一个，
 *  再跑几轮斥力把贴脸的点推开——稀疏时不秃、多会话时不成糊团。 */
function clusterPoints(count, span, seed) {
  const rnd = mulberry32(seed);
  if (count <= 1) return [{ x: 0, y: 0 }];
  const rx = span * 0.5;
  const ry = span * 0.5 * 0.85;
  const pts = [];
  for (let i = 0; i < count; i++) {
    let best = null;
    let bestD = -1;
    for (let k = 0; k < 14; k++) {
      const a = rnd() * TAU;
      const rr = Math.sqrt(rnd());
      const c = { x: Math.cos(a) * rr * rx, y: Math.sin(a) * rr * ry };
      let dmin = Infinity;
      for (const p of pts) {
        const d = (p.x - c.x) ** 2 + (p.y - c.y) ** 2;
        if (d < dmin) dmin = d;
      }
      if (dmin > bestD) {
        bestD = dmin;
        best = c;
      }
    }
    pts.push(best);
  }
  const minD = Math.min(28, (span * 0.8) / Math.sqrt(count));
  for (let it = 0; it < 14; it++) {
    let moved = false;
    for (let i = 0; i < pts.length; i++) {
      for (let j = i + 1; j < pts.length; j++) {
        let dx = pts[j].x - pts[i].x;
        let dy = pts[j].y - pts[i].y;
        let d = Math.hypot(dx, dy);
        if (d < 1e-6) {
          dx = 0.6;
          dy = 0.8;
          d = 1;
        }
        if (d < minD) {
          const push = (minD - d) / 2;
          const ux = dx / d;
          const uy = dy / d;
          pts[i].x -= ux * push;
          pts[i].y -= uy * push;
          pts[j].x += ux * push;
          pts[j].y += uy * push;
          moved = true;
        }
      }
    }
    for (const p of pts) {
      const nx = p.x / (rx * 1.12);
      const ny = p.y / (ry * 1.12);
      const r2 = nx * nx + ny * ny;
      if (r2 > 1) {
        const k = 1 / Math.sqrt(r2);
        p.x = nx * k * rx * 1.12;
        p.y = ny * k * ry * 1.12;
      }
    }
    if (!moved) break;
  }
  return pts;
}

/** 无环多叉树（取代 MST）：质心最近的星当枢纽，先接最近的 3~5 颗开叉；
 *  其余星按「离已连星最近 + 偏好多子节点」生长。全树 n-1 条边，必有分叉、绝无闭环。 */
function buildTree(points) {
  const n = points.length;
  if (n < 2) return [];
  let cx = 0;
  let cy = 0;
  for (const p of points) {
    cx += p.x;
    cy += p.y;
  }
  cx /= n;
  cy /= n;
  let hub = 0;
  let hd = Infinity;
  points.forEach((p, i) => {
    const d = (p.x - cx) ** 2 + (p.y - cy) ** 2;
    if (d < hd) {
      hd = d;
      hub = i;
    }
  });
  const deg = new Array(n).fill(0);
  const inT = new Array(n).fill(false);
  const edges = [];
  inT[hub] = true;
  const k = n >= 4 ? Math.min(n - 1, Math.max(3, Math.min(5, Math.round(n / 5) + 1))) : n - 1;
  const order = [];
  for (let i = 0; i < n; i++) {
    if (i === hub) continue;
    const d = (points[i].x - points[hub].x) ** 2 + (points[i].y - points[hub].y) ** 2;
    order.push({ i, d });
  }
  order.sort((a, b) => a.d - b.d);
  const seeded = Math.min(k, order.length);
  for (let s = 0; s < seeded; s++) {
    const j = order[s].i;
    inT[j] = true;
    deg[j] = 1;
    deg[hub] += 1;
    edges.push([points[hub], points[j]]);
  }
  let connected = 1 + seeded;
  while (connected < n) {
    let best = null;
    for (let a = 0; a < n; a++) {
      if (!inT[a]) continue;
      for (let b = 0; b < n; b++) {
        if (inT[b]) continue;
        const d2 = (points[a].x - points[b].x) ** 2 + (points[a].y - points[b].y) ** 2;
        const score = d2 / (1 + 0.3 * (deg[a] - 1));
        if (!best || score < best.score) best = { a, b, score };
      }
    }
    if (!best) break;
    inT[best.b] = true;
    deg[best.b] = 1;
    deg[best.a] += 1;
    edges.push([points[best.a], points[best.b]]);
    connected += 1;
  }
  return edges;
}

/** 项目团在平面上的摆位：按大小贪心塞进螺旋，避免大团互相压住把画面撑爆 */
function packClusters(clusters) {
  const placed = [];
  for (const c of clusters) {
    if (!placed.length) {
      c.x = 0;
      c.y = 0;
      placed.push(c);
      continue;
    }
    const rad = c.span * 0.55;
    let bestX = 0;
    let bestY = 0;
    let found = false;
    for (let k = 1; k < 6000 && !found; k++) {
      const r = 26 * Math.sqrt(k);
      const a = k * 2.399963229728653;
      const x = Math.cos(a) * r;
      const y = Math.sin(a) * r * 0.82;
      let ok = true;
      for (const p of placed) {
        const min = rad + p.span * 0.55 + 64;
        if (Math.hypot(x - p.x, y - p.y) < min) {
          ok = false;
          break;
        }
      }
      if (ok) {
        bestX = x;
        bestY = y;
        found = true;
      }
    }
    c.x = bestX;
    c.y = bestY;
    placed.push(c);
  }
}

/**
 * items → { clusters, stars, bounds }
 * clusters：项目名/数量/跨度/锚点/颜色/本团星点与连线
 * stars：扁平星点表（交互与渲染共用）
 */
export function buildAtlas(items) {
  const byProj = new Map();
  for (const it of items) {
    const p = it.project || '未分组';
    if (!byProj.has(p)) byProj.set(p, []);
    byProj.get(p).push(it);
  }
  const names = [...byProj.keys()].sort(
    (a, b) => byProj.get(b).length - byProj.get(a).length || a.localeCompare(b, 'zh'),
  );

  // 上下文长度因子：按全部消息字符数的 25/90 分位归一（任何规模的库都有大小层次）
  const ests = items.map((x) => (x.chars > 0 ? x.chars : (x.count || 1) * 120));
  const logs = ests.map((e) => Math.log10(Math.max(1, e))).sort((a, b) => a - b);
  const lo = logs[Math.floor(logs.length * 0.25)] ?? 3;
  const hi = Math.max(lo + 0.8, logs[Math.floor(logs.length * 0.9)] ?? 5);
  const ctxOf = (it) => {
    const est = it.chars > 0 ? it.chars : (it.count || 1) * 120;
    return Math.min(1, Math.max(0, (Math.log10(Math.max(1, est)) - lo) / (hi - lo)));
  };

  const clusters = names.map((name) => {
    const count = byProj.get(name).length;
    return {
      name,
      count,
      span: 50 + 60 * Math.sqrt(count),
      x: 0,
      y: 0,
      color: CLUSTER_COLORS[hashStr(name) % CLUSTER_COLORS.length],
      stars: [],
      edges: [],
    };
  });
  packClusters(clusters);

  const stars = [];
  const now = Date.now();
  for (const c of clusters) {
    const list = byProj.get(c.name);
    const pts = clusterPoints(list.length, c.span, hashStr(`cluster:${c.name}`));
    let cx = 0;
    let cy = 0;
    for (const p of pts) {
      cx += p.x;
      cy += p.y;
    }
    cx /= pts.length || 1;
    cy /= pts.length || 1;
    c.cx = c.x + cx;
    c.cy = c.y + cy;

    list.forEach((it, i) => {
      const rnd = mulberry32(hashStr(`star:${it.id}`));
      const pt = pts[i];
      const age = it.updated_at ? (now - Date.parse(it.updated_at)) / 86400000 : 45;
      const rec = Number.isFinite(age) ? Math.max(0, Math.min(1, 1 - age / 60)) : 0;
      const star = {
        ...it,
        cluster: c.name,
        color: c.color,
        x: c.x + pt.x,
        y: c.y + pt.y,
        // 上下文越长星点越大；亮度跟最近更新时间走（新会话更亮）
        size: 1.5 + 3.6 * ctxOf(it),
        bright: 0.66 + 0.34 * rec,
        // 星等：总览时只亮最重要的十来颗，放大后逐级显现
        importance: it.chars > 0 ? it.chars : (it.count || 1) * 120,
        phase: rnd() * TAU,
      };
      c.stars.push(star);
      stars.push(star);
    });
    // 三档连线：总览 / 中等 / 全量。各档独立建「无环多叉树」保持分叉感。
    const ranked = [...c.stars].sort((a, b) => b.importance - a.importance);
    ranked.forEach((s, i) => {
      s.rank = i;
      // 纵深：前两颗近、过半之前中、其余远（配合渲染层的大小/亮度差）
      s.tier = i < 2 ? 'near' : i < Math.ceil(c.stars.length * 0.55) ? 'mid' : 'far';
      // 衍射亮星：每个项目内第一颗四芒、第二颗细四芒（全图最亮升六芒，见下）
      s.spike = i === 0 ? 2 : i === 1 ? 1 : null;
    });
    c.ranked = ranked;
    c.topEdges = buildTree(ranked.slice(0, Math.min(14, ranked.length)));
    c.midEdges = buildTree(ranked.slice(0, Math.min(30, ranked.length)));
    c.edges = buildTree(c.stars);
  }

  // 全图最亮的星升成六芒主角（只一颗，避免满屏星芒）
  let globalTop = null;
  for (const s of stars) {
    if (!globalTop || s.importance > globalTop.importance) globalTop = s;
  }
  if (globalTop) globalTop.spike = 3;

  let minX = 0;
  let maxX = 0;
  let minY = 0;
  let maxY = 0;
  if (stars.length) {
    minX = Infinity;
    maxX = -Infinity;
    minY = Infinity;
    maxY = -Infinity;
    for (const s of stars) {
      minX = Math.min(minX, s.x);
      maxX = Math.max(maxX, s.x);
      minY = Math.min(minY, s.y);
      maxY = Math.max(maxY, s.y);
    }
  }
  return { clusters, stars, bounds: { minX, maxX, minY, maxY } };
}
