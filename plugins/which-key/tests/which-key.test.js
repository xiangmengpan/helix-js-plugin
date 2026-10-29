// which-key node 级测试 —— 配置面(language / show_docs / position)的纯 JS 行为。
// 运行:node --test plugins/which-key/tests/which-key.test.js
// 原理:插件不是 CommonJS 模块(顶层直接跑),所以 require 之前注入最小 `global.helix` 桩,
//      从 `set_keymap_hint(fn)` 捕获 render,再拿自造 ctx 直接调它。
'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');

const PLUGIN = path.join(__dirname, '..', 'plugin.js');

function load(cfg, pane_mode, opts) {
  const o = opts || {};
  const cap = { hint: null, schema: null, exported: null, echoes: [] };
  global.helix = {
    plugin() {},
    define_config(name, schema) {
      cap.schema = { name, schema };
    },
    get_config: () => cfg,
    set_keymap_hint(fn) {
      cap.hint = fn;
    },
    export(obj) {
      cap.exported = obj;
    },
    pane: o.pane,
    echo: (msg) => {
      cap.echoes.push(msg);
    },
    text: { truncate: (t, w) => String(t).slice(0, w) },
    viewport: () => o.viewport || [80, 24],
    // v2：which-key 用它排候选（测试可通过 opts.frecency 注入）
    key_frecency: () => o.frecency || {},
    pane_mode,
  };
  delete require.cache[require.resolve(PLUGIN)];
  require(PLUGIN);
  assert.ok(cap.hint, '插件应注册 set_keymap_hint');
  return cap;
}

/** "d  跳转定义" → { d: "跳转定义" }(键与说明之间用两个空格分隔) */
function map(text) {
  const out = {};
  for (const line of text.split('\n')) {
    const i = line.indexOf('  ');
    if (i < 0) out[line.trim()] = '';
    else out[line.slice(0, i)] = line.slice(i + 2);
  }
  return out;
}

const goto = () => [
  { keys: 'd', doc: 'Goto definition' },
  { keys: 'Z', doc: 'Some engine doc' }, // CN 里没有 → 走 show_docs 的回退
];
const dap = () => [
  { keys: 'l', doc: 'Launch debug target' },
  { keys: 'b', doc: 'Toggle breakpoint' },
  { keys: 'v', doc: 'List variables' },
  { keys: 's', doc: 'Switch' },
  { keys: 'S', doc: 'Switch debug session' },
];

test('config schema 声明了 language(枚举 zh-CN / en,默认 zh-CN)', () => {
  const { name, schema } = load({}).schema;
  assert.equal(name, 'which-key');
  assert.deepEqual(schema.language.options, ['zh-CN', 'en']);
  assert.equal(schema.language.default, 'zh-CN');
  assert.equal(schema.show_docs.default, true);
  assert.equal(schema.position.default, 'bottom-right');
});

test('默认(没写 language)= zh-CN:中文说明', () => {
  const { hint } = load({});
  assert.deepEqual(map(hint({ title: 'Goto', entries: goto() }).text), {
    d: '跳转定义',
    Z: 'Some engine doc', // 未命中映射 → 英文回退
  });
});

test('language = "zh-CN":与默认一致', () => {
  const { hint } = load({ language: 'zh-CN', show_docs: true });
  assert.deepEqual(map(hint({ title: 'Goto', entries: goto() }).text), {
    d: '跳转定义',
    Z: 'Some engine doc',
  });
});

test('language = "en":直接用引擎 doc,一个中文都不出现', () => {
  const { hint } = load({ language: 'en' });
  const out = hint({ title: 'Goto', entries: goto() });
  assert.deepEqual(map(out.text), { d: 'Goto definition', Z: 'Some engine doc' });
  assert.ok(!/[\u4e00-\u9fff]/.test(out.text), `不该有中文: ${out.text}`);
});

test('language = "en" 时 show_docs 不参与判断(en 下英文说明必显示)', () => {
  const { hint } = load({ language: 'en', show_docs: false });
  assert.deepEqual(map(hint({ title: 'Goto', entries: goto() }).text), {
    d: 'Goto definition',
    Z: 'Some engine doc',
  });
});

test('zh-CN + show_docs = false:未命中的键只留键名', () => {
  const { hint } = load({ language: 'zh-CN', show_docs: false });
  assert.deepEqual(map(hint({ title: 'Goto', entries: goto() }).text), {
    d: '跳转定义',
    Z: '',
  });
});

// ── 前缀标题必须与引擎里的 keymap 名一致 ──
// 引擎给的标题是 `{ "Debug (experimental)" ... }` 里的名字(default.rs),不是"Debug"。
// 写错 → 前缀丢失 → 退回**单键**匹配 → 整张表的中文全是别人的(v → 进入选择模式、
// b → 后退一词)。这条钉住它。
test('space G 子菜单(标题 "Debug (experimental)")显示 DAP 中文', () => {
  const { hint } = load({});
  assert.deepEqual(map(hint({ title: 'Debug (experimental)', entries: dap() }).text), {
    l: '启动调试',
    b: '切换断点',
    v: '变量视图',
    s: '切换线程/栈帧',
    S: '切换调试会话',
  });
});

test('space G s 子菜单(标题 "Switch")显示切换线程/栈帧', () => {
  const { hint } = load({});
  const out = map(
    hint({
      title: 'Switch',
      entries: [
        { keys: 't', doc: 'Switch current thread' },
        { keys: 'f', doc: 'Switch stack frame' },
      ],
    }).text
  );
  assert.deepEqual(out, { t: '切换线程', f: '切换栈帧' });
});

test('平级模式:引擎的 keymap() 优先,language 不改变它', () => {
  const pm = {
    keymap: () => ({
      mode: 'C-p',
      keys: [{ key: 'h j k l', desc: '聚焦(左/下/上/右)', enabled: true }],
    }),
  };
  for (const language of ['zh-CN', 'en']) {
    const { hint } = load({ language }, pm);
    const out = hint({ title: 'C-p', entries: [{ keys: 'x', doc: 'close' }] });
    assert.equal(out.text, 'h j k l'.padEnd(12) + ' 聚焦(左/下/上/右)', language);
  }
});

test('avoid_panels:右侧面板 ⇒ 提示挪到左边', () => {
  const rightPanel = { id: 1, kind: 'PluginPanel', area: { x: 60, y: 0, w: 20, h: 24 } };
  const { hint } = load({ position: 'bottom-right' }, undefined, {
    pane: { list: () => [{ id: 0, kind: 'EditorView', area: { x: 0, y: 0, w: 60, h: 24 } }, rightPanel] },
    viewport: [80, 24],
  });
  const out = hint({ title: 'Goto', entries: goto() });
  assert.equal(out.position, 'bottom-left', `右侧被占 ⇒ 左下: ${out.position}`);
});

test('avoid_panels:右侧+底部都被占 ⇒ 左上', () => {
  const panels = [
    { id: 1, kind: 'PluginPanel', area: { x: 60, y: 0, w: 20, h: 18 } },
    { id: 2, kind: 'PluginPanel', area: { x: 60, y: 18, w: 20, h: 6 } },
  ];
  // 让底部整条也被占：再加一个横跨的面板
  panels.push({ id: 3, kind: 'PluginPanel', area: { x: 0, y: 20, w: 80, h: 4 } });
  const { hint } = load({ position: 'bottom-right' }, undefined, {
    pane: { list: () => panels },
    viewport: [80, 24],
  });
  assert.equal(hint({ title: 'Goto', entries: goto() }).position, 'top-left');
});

test('avoid_panels = false ⇒ 完全按配置', () => {
  const { hint } = load({ position: 'bottom-right', avoid_panels: false }, undefined, {
    pane: { list: () => [{ id: 1, kind: 'PluginPanel', area: { x: 60, y: 0, w: 20, h: 24 } }] },
    viewport: [80, 24],
  });
  assert.equal(hint({ title: 'Goto', entries: goto() }).position, 'bottom-right');
});

test('没有 pane API / 没有插件面板 ⇒ 保持配置位置(向后兼容)', () => {
  for (const pane of [undefined, { list: () => [] }]) {
    const { hint } = load({ position: 'top-right' }, undefined, { pane, viewport: [80, 24] });
    assert.equal(hint({ title: 'Goto', entries: goto() }).position, 'top-right');
  }
});

test('max_lines:超出就收成「前 n-1 行 + … 还有 N 个键」', () => {
  const many = Array.from({ length: 25 }, (_, i) => ({ keys: 'k' + i, doc: 'doc' + i }));
  const { hint } = load({ max_lines: 5 }, undefined, { pane: { list: () => [] } });
  const out = hint({ title: 'Goto', entries: many });
  const lines = out.text.split('\n');
  assert.equal(lines.length, 5, `应有 5 行: ${lines.length}`);
  assert.match(lines[4], /还有 21 个键/, lines[4]);
});

test('max_lines = 0 ⇒ 不限行数', () => {
  const many = Array.from({ length: 25 }, (_, i) => ({ keys: 'k' + i, doc: 'doc' + i }));
  const { hint } = load({ max_lines: 0 }, undefined, { pane: { list: () => [] } });
  assert.equal(hint({ title: 'Goto', entries: many }).text.split('\n').length, 25);
});

test('mode = "line" ⇒ 1 行浮层(只列键名) + 状态栏带说明摘要', () => {
  const entries = Array.from({ length: 9 }, (_, i) => ({ keys: 'k' + i, doc: '说明' + i }));
  const cap = load({ mode: 'line' }, undefined, { pane: { list: () => [] } });
  const out = cap.hint({ title: 'Goto', entries });
  assert.equal(typeof out.text, 'string', '必须返回文本(→ 1 行浮层,不会被消息盖掉)');
  assert.equal(out.text.split('\n').length, 1, '只有一行');
  assert.ok(!out.text.includes('说明'), '浮层只放键名: ' + out.text);
  assert.match(out.text, /^k0 k1 k2/, out.text);
  assert.equal(cap.echoes.length, 1, '状态栏另有一行带说明的摘要');
  assert.match(cap.echoes[0], /^Goto ▸ k0 说明0/);
  // sticky 每次按键都会重算 ⇒ 状态栏不重复刷屏（浮层反正每帧重画）
  cap.hint({ title: 'Goto', entries });
  assert.equal(cap.echoes.length, 1, '相同内容不重复 echo');
});

test('mode = "status" ⇒ 完全不出浮层({hide:true}),只有状态栏', () => {
  const entries = [{ keys: 'd', doc: '跳转定义' }];
  const cap = load({ mode: 'status' }, undefined, { pane: { list: () => [] } });
  assert.deepEqual(cap.hint({ title: 'Goto', entries }), { hide: true });
  assert.match(cap.echoes[0], /Goto ▸ d 跳转定义/);
});

test('position 取自配置;没有 entries 时返回 null', () => {
  const { hint } = load({ position: 'center' });
  assert.equal(hint({ title: 'Goto', entries: goto() }).position, 'center');
  assert.equal(hint({ title: 'Goto', entries: [] }), null);
});

test('g 菜单的 a/c/r 是 goto 语义,不是 space 前缀的意思', () => {
  const { hint } = load({});
  const out = map(
    hint({
      title: 'Goto',
      entries: [
        { keys: 'a', doc: 'Goto last accessed file' },
        { keys: 'c', doc: 'Goto window center' },
        { keys: 'r', doc: 'Goto reference' },
        { keys: 'x', doc: 'Extend line below' },
      ],
    }).text
  );
  assert.deepEqual(out, {
    a: '上次访问文件',
    c: '窗口中间',
    r: '跳转引用',
    x: '向下扩展行',
  });
});

// CN 是**对象字面量**:同一个键写两遍时后写者静默覆盖前者,而覆盖的方向往往就是错的
// (曾经 g a 先写成「上次访问文件」、后面又写成 space 的「代码操作」⇒ g 菜单显示错的)。
// 这类错没有任何运行时症状,所以只能在源码上钉死。
test('CN 表里没有重复键(重复 = 静默覆盖 = 描述错)', () => {
  const src = require('node:fs').readFileSync(PLUGIN, 'utf8');
  const body = src.split('const CN = {')[1].split('\n  };')[0];
  const keys = [...body.matchAll(/"((?:[^"\\]|\\.)*)"\s*:/g)].map((m) => m[1]);
  assert.ok(keys.length > 150, `CN 解析应当拿到上百个键,实际 ${keys.length}`);
  const seen = new Map();
  const dups = [];
  for (const k of keys) {
    if (seen.has(k)) dups.push(k);
    seen.set(k, true);
  }
  assert.deepEqual(dups, [], `CN 里有重复键(后写覆盖前者): ${dups.join(', ')}`);
});

test('v2：截断文案要指路「（:keys 看全部）」', () => {
  const cap = load({ language: 'zh-CN', mode: 'box', max_lines: 3 });
  const entries = Array.from({ length: 12 }, (_, i) => ({ keys: 'k' + i, doc: 'cmd ' + i }));
  const got = cap.hint({ title: 'Space', entries });
  assert.ok(got && got.text, '应返回浮层文本');
  assert.ok(
    String(got.text).includes('（:keys 看全部）'),
    `截断行应指路 :keys: ${JSON.stringify(got.text)}`,
  );
});

test('v2：min_entries ⇒ 当前层子键太少就不弹', () => {
  const cap = load({ language: 'zh-CN', mode: 'box', min_entries: 3 });
  const got = cap.hint({ title: 'Space', entries: [{ keys: 'f', doc: 'Files' }] });
  assert.deepEqual(got, { hide: true }, '少于 min_entries 应不显示');
  // 够数就正常显示
  const ok = cap.hint({
    title: 'Space',
    entries: [
      { keys: 'f', doc: 'Files' },
      { keys: 'b', doc: 'Buffers' },
      { keys: 'g', doc: 'Changed files' },
    ],
  });
  assert.ok(ok && ok.text, '够了就应该显示');
});

test('v2：按 helix.key_frecency() 重排（高频键排前面）', () => {
  const cap = load(
    { language: 'en', mode: 'line', sort_by_frecency: true, min_entries: 0 },
    undefined,
    { frecency: { g: 50, f: 1, b: 5 } },
  );
  const got = cap.hint({
    title: 'Space',
    entries: [
      { keys: 'f', doc: 'file' },
      { keys: 'b', doc: 'buffer' },
      { keys: 'g', doc: 'changed' },
    ],
  });
  assert.ok(got && got.text, '应有提示文本');
  const bar = String(got.text);
  const iG = bar.indexOf('g');
  const iB = bar.indexOf('b');
  const iF = bar.indexOf('f');
  assert.ok(iG < iB && iB < iF, `应按频次降序: ${bar}`);
});

test('调试命中提示：on_debug_hint=off 什么都不画、status 只 echo、full 按 mode', () => {
  const entries = [
    { keys: 'n', doc: 'Next' },
    { keys: 'c', doc: 'Continue' },
  ];
  // off ⇒ 一个格子都不占（hide），也不 echo
  let cap = load({ language: 'zh-CN', mode: 'box', on_debug_hint: 'off' });
  let got = cap.hint({ title: 'Debug', entries });
  assert.deepEqual(got, { hide: true });
  assert.equal(cap.echoes.length, 0, 'off 不应写状态栏');

  // status ⇒ 只写一行状态栏（hide，但 echo 有内容）
  cap = load({ language: 'zh-CN', mode: 'box', on_debug_hint: 'status' });
  got = cap.hint({ title: 'Debug', entries });
  assert.deepEqual(got, { hide: true });
  assert.ok(cap.echoes.some((m) => String(m).includes('单步跳过')), `应 echo 中文摘要: ${cap.echoes}`);

  // full ⇒ 走常规路径（box 模式有正文）
  cap = load({ language: 'zh-CN', mode: 'box', on_debug_hint: 'full' });
  got = cap.hint({ title: 'Debug', entries });
  assert.ok(got && got.text, `full 应有正文: ${JSON.stringify(got)}`);
});
