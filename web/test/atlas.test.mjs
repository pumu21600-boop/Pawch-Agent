import test from 'node:test';
import assert from 'node:assert/strict';
import { buildAtlas } from '../src/galaxy/atlas.js';

function item(id, project, extra = {}) {
  return {
    id,
    project,
    title: `会话 ${id}`,
    updated_at: '2026-11-01T00:00:00Z',
    count: 4,
    chars: 1200,
    ...extra,
  };
}

test('buildAtlas 按项目分组并保留全部会话', () => {
  const items = [
    item('a1', '甲'),
    item('a2', '甲'),
    item('b1', '乙'),
    item('c1', '未分组项目'),
  ];
  const { clusters, stars } = buildAtlas(items);
  assert.equal(clusters.length, 3);
  assert.equal(stars.length, 4);
  const jia = clusters.find((c) => c.name === '甲');
  assert.equal(jia.count, 2);
  assert.equal(jia.stars.length, 2);
  // 每个项目的星点都是扁平表里的同一批对象
  for (const c of clusters) {
    for (const s of c.stars) assert.ok(stars.includes(s));
  }
});

test('buildAtlas 确定性：同一份数据两次布局坐标一致', () => {
  const items = [];
  for (let i = 0; i < 12; i++) items.push(item(`x${i}`, i % 3 === 0 ? '甲' : '乙'));
  const a = buildAtlas(items);
  const b = buildAtlas(items);
  for (let i = 0; i < a.stars.length; i++) {
    assert.equal(a.stars[i].x, b.stars[i].x);
    assert.equal(a.stars[i].y, b.stars[i].y);
  }
});

test('buildAtlas 空数据不炸', () => {
  const sc = buildAtlas([]);
  assert.deepEqual(sc.clusters, []);
  assert.deepEqual(sc.stars, []);
  assert.equal(sc.bounds.minX, 0);
});

test('buildAtlas 单会话项目：一颗星、没有连线', () => {
  const { clusters, stars } = buildAtlas([item('solo', '独苗')]);
  assert.equal(stars.length, 1);
  assert.equal(clusters[0].edges.length, 0);
  assert.ok(Number.isFinite(stars[0].x) && Number.isFinite(stars[0].y));
});

test('buildAtlas 大项目用星云盘，坐标与边界都有限', () => {
  const items = [];
  for (let i = 0; i < 40; i++) items.push(item(`big${i}`, '大项目'));
  const { clusters, stars, bounds } = buildAtlas(items);
  assert.equal(stars.length, 40);
  assert.ok(clusters[0].edges.length >= 1);
  for (const s of stars) {
    assert.ok(Number.isFinite(s.x) && Number.isFinite(s.y));
    assert.ok(s.x >= bounds.minX && s.x <= bounds.maxX);
    assert.ok(s.y >= bounds.minY && s.y <= bounds.maxY);
  }
});

test('buildAtlas 星点大小落在可见范围', () => {
  const items = [item('s1', '甲', { chars: 10 }), item('s2', '乙', { chars: 500000 })];
  const { stars } = buildAtlas(items);
  for (const s of stars) {
    assert.ok(s.size >= 1.5 && s.size <= 5.2);
    assert.ok(s.bright >= 0.6 && s.bright <= 1.01);
  }
});
