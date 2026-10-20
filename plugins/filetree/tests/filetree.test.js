// filetree node 级测试 —— 渲染用的两个**纯逻辑**(窗口切片 / 滚动位置)。
//
// 运行:node --test plugins/filetree/tests/filetree.test.js
//      (node ≥ 24 不接受目录参数;零依赖 node:test)
//
// 为什么只测这两个:roll 自己的 draw 代码依赖 `helix.draw`(引擎侧),端到端由
// `helix-term` 的集成测试覆盖;而"窗口算错一格就整棵树错位"这类错,全部落在
// `window_rows` / `scroll_top` 这两个纯函数上 ⇒ 在这里钉死。
'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');

// helix 最小桩(plugin.js 顶层会调 helix.plugin / define_config)
global.helix = {
  plugin() {},
  define_config() {},
  register_command() {},
  echo() {},
  get_config: () => ({}),
  icons: { dir: () => '▸', file: () => '·' },
  read_dir: () => [],
  on() {},          // 注册期只登记事件,node 侧空转
  export() {},
  draw: {},         // render 在 node 侧不跑(端到端归集成测试)
};

const { window_rows, scroll_top } = require('../plugin.js');

const rows = (n) => Array.from({ length: n }, (_, i) => ({ node: { path: 'p' + i }, depth: 0 }));

test('window_rows:按内容行号切片,idx 是内容坐标(不含屏幕偏移)', () => {
  const r = rows(10);
  assert.deepEqual(window_rows(r, 0, 3).map((x) => x.idx), [0, 1, 2]);
  assert.deepEqual(window_rows(r, 5, 3).map((x) => x.idx), [5, 6, 7]);
  // 到底了只给剩下的
  assert.deepEqual(window_rows(r, 8, 3).map((x) => x.idx), [8, 9]);
  // 空表 / 高度 0 不能炸
  assert.deepEqual(window_rows([], 0, 5), []);
  assert.deepEqual(window_rows(r, 0, 0), []);
  // 切出来的 row 就是原来那个对象(不能被复制,否则展开状态不同步)
  assert.equal(window_rows(r, 2, 1)[0].row, r[2]);
});

test('scroll_top:以光标为中心,夹在 [0, total-list_h]', () => {
  // 装得下 ⇒ 不滚
  assert.equal(scroll_top(0, 5, 10), 0);
  assert.equal(scroll_top(4, 5, 10), 0);
  // 顶部附近不滚(centered 为负)
  assert.equal(scroll_top(0, 100, 10), 0);
  assert.equal(scroll_top(3, 100, 10), 0);
  // 中间:光标大致居中
  assert.equal(scroll_top(15, 100, 10), 10);
  assert.equal(scroll_top(50, 100, 10), 45);
  // 底部:夹到最后 list_h 项
  assert.equal(scroll_top(99, 100, 10), 90);
  // 病态输入不炸
  assert.equal(scroll_top(0, 100, 0), 0);
});

test('窗口 + 滚动配合:光标永远落在可见窗口内', () => {
  const total = 37;
  const list_h = 6;
  const r = rows(total);
  for (const cursor of [0, 1, 5, 6, 7, 20, 33, 36]) {
    const top = scroll_top(cursor, total, list_h);
    const visible = window_rows(r, top, list_h).map((x) => x.idx);
    assert.ok(
      visible.includes(cursor),
      `光标 ${cursor} 应可见(top=${top}, 可见=${visible})`,
    );
    assert.equal(visible.length, Math.min(list_h, total - top), '窗口不能被撑破');
  }
});
