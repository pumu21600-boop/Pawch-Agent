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

/** 小星座用「2~3 个局部星群 + 抖动」散开（再交给 MST 连线，自然分叉）；
 *  大星座改用等密度星云盘。早先是单条随机游走链，MST 只会顺着链连，
 *  整团看起来就是一根折线（2026-11-05 用户实感「全是一条一条的线」）。 */
function clusterPoints(count, span, seed) {
  const rnd = mulberry32(seed);
  if (count <= 1) return [{ x: 0, y: 0 }];
  const pts = [];
  if (count > 28) {
    for (let i = 0; i < count; i++) {
      const t = (i + 0.5) / count;
      const r = (span / 2) * Math.sqrt(t) * (0.72 + rnd() * 0.36);
      const a = i * 2.399963229728653 + (rnd() - 0.5) * 0.6;
      pts.push({ x: Math.cos(a) * r, y: Math.sin(a) * r * 0.82 });
    }
    return pts;
  }
  // 2~3 个局部星群：每群至少一颗，其余随机落群；群内再抖开。
  // 锚点半径压小、群内半径放大 → 群与群之间的「桥线」短，团内更像一簇星，
  // 不会出现一根长直线横穿半个星座。
  const groups = count <= 3 ? 1 : count <= 9 ? 2 + (rnd() < 0.4 ? 1 : 0) : 3;
  const anchors = [];
  for (let g = 0; g < groups; g++) {
    const a = (g / groups) * TAU + (rnd() - 0.5) * 0.9;
    const rr = span * (0.13 + rnd() * 0.07);
    anchors.push({ x: Math.cos(a) * rr, y: Math.sin(a) * rr * 0.8 });
  }
  for (let i = 0; i < count; i++) {
    const g = i < groups ? i : Math.floor(rnd() * groups);
    const a = rnd() * TAU;
    const rr = span * 0.105 * Math.sqrt(rnd());
    pts.push({
      x: anchors[g].x + Math.cos(a) * rr,
      y: anchors[g].y + Math.sin(a) * rr * 0.82,
    });
  }
  let minX = Infinity;
  let maxX = -Infinity;
  let minY = Infinity;
  let maxY = -Infinity;
  for (const p of pts) {
    minX = Math.min(minX, p.x);
    maxX = Math.max(maxX, p.x);
    minY = Math.min(minY, p.y);
    maxY = Math.max(maxY, p.y);
  }
  const k = span / Math.max(1, maxX - minX, maxY - minY);
  const cx = (minX + maxX) / 2;
  const cy = (minY + maxY) / 2;
  for (const p of pts) {
    p.x = (p.x - cx) * k;
    p.y = (p.y - cy) * k;
  }
  return pts;
}

/** 最小生成树：星座连线（只连 n-1 条，不会糊成一团） */
function mst(points) {
  const n = points.length;
  if (n < 2) return [];
  const used = new Array(n).fill(false);
  used[0] = true;
  const tree = [0];
  const edges = [];
  while (tree.length < n) {
    let best = Infinity;
    let bi = -1;
    let bj = -1;
    for (const i of tree) {
      for (let j = 0; j < n; j++) {
        if (used[j]) continue;
        const d = Math.hypot(points[i].x - points[j].x, points[i].y - points[j].y);
        if (d < best) {
          best = d;
          bi = i;
          bj = j;
        }
      }
    }
    if (bj < 0) break;
    used[bj] = true;
    tree.push(bj);
    edges.push([points[bi], points[bj]]);
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
    // 三档连线：总览 / 中等 / 全量。按星等排序后各取前 N 做最小生成树。
    const ranked = [...c.stars].sort((a, b) => b.importance - a.importance);
    ranked.forEach((s, i) => {
      s.rank = i;
    });
    c.ranked = ranked;
    c.topEdges = mst(ranked.slice(0, Math.min(14, ranked.length)));
    c.midEdges = mst(ranked.slice(0, Math.min(30, ranked.length)));
    c.edges = mst(c.stars);
  }

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
