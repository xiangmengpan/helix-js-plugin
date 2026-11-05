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
  const cap = { hint: null, schema: null, exported: null, echoes: [], command: null, popup: null };
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
    // ── 插件作用域键位（`:which-key`）的接线所需 ──
    register_command: (name, fn, doc) => {
      cap.command = { name, fn, doc };
    },
    open_popup: (opts) => {
      cap.popup = opts;
      return 1;
    },
    el: (kind, children, opts) => ({ kind, children: children || [], opts: opts || {} }),
    keymap: {
      active_scope: () => (o.scope === undefined ? null : o.scope),
      hints: (scope) => (o.hints ? o.hints(scope) : null),
    },
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

// ───────────── 插件作用域键位：`:which-key`（B 阶段） ─────────────
// 由来：面板/弹层的键位表在 `helix.keymap`（与 `?` 帮助、`:keys` 同源），
// which-key 只多给一个**统一入口** —— 不用记每个插件把帮助放在哪个键上。
const SCOPE_HINTS = 'q  关闭面板\nj  下移\nk  上移\n?  显示键位';

test(':which-key 注册成命令', () => {
  const cap = load({});
  assert.equal(cap.command.name, 'which-key');
  assert.ok(cap.command.doc.includes('插件层'));
});

test(':which-key 在插件层里弹窗显示作用域表', () => {
  const cap = load({}, null, { scope: 'filetree', hints: () => SCOPE_HINTS });
  assert.equal(cap.command.fn(), true, '应弹出弹窗');
  assert.ok(cap.popup, '应调 open_popup');
  const tree = cap.popup.render();
  const text = JSON.stringify(tree.children);
  assert.ok(text.includes('关闭面板'), `内容应来自作用域表: ${text}`);
  assert.ok(text.includes('[plugins.filetree.keys] 可改'), '标题要点明新写法');
});

test(':which-key 不在插件层 ⇒ 状态栏一句话，不弹窗', () => {
  const cap = load({}, null, { scope: null });
  assert.equal(cap.command.fn(), false);
  assert.equal(cap.popup, null, '不该弹窗');
  assert.ok(cap.echoes.at(-1).includes('不在插件层'), cap.echoes.at(-1));
});

test(':which-key 作用域没有键位表 ⇒ 提示而不是空弹窗', () => {
  const cap = load({}, null, { scope: 'empty', hints: () => null });
  assert.equal(cap.command.fn(), false);
  assert.ok(cap.echoes.at(-1).includes('没有可显示'), cap.echoes.at(-1));
});

test('scope_box:小终端收窄并标记滚动', () => {
  const { scope_box } = load({}).exported;
  const big = scope_box(30, 40, 120, 40, 30);
  assert.equal(big.need_scroll, false);
  const small = scope_box(30, 40, 80, 20, 30);
  assert.equal(small.need_scroll, true);
  assert.ok(small.height <= 20, `高度 ≤ 视口: ${small.height}`);
});

// ───────── 前缀键提示里带上「当前插件层的键位」（在插件层里也能触发）─────────
// 面板是非模态的：未绑定的前缀键（如 `space`）会冒泡给编辑器触发 which-key。
// 那时提示里**追加**一段作用域表 —— 既看 space 子表，也看"这个面板里还能按什么"。
const SCOPE_TABLE = 'q  关闭面板\nj  下移';

test('前缀键提示：插件层里追加作用域段（不替换原生表）', () => {
  // `language: "en"`：默认 zh-CN 会把 doc 换成中文表，这里要断言"原生那几行还在"
  const cap = load({ mode: 'box', language: 'en' }, null, {
    scope: 'filetree',
    hints: () => SCOPE_TABLE,
  });
  const out = cap.hint({ title: 'space', entries: goto() });
  const lines = out.text.split('\n');
  assert.ok(
    lines.some((l) => l.includes('Goto definition')),
    `原生子表仍在: ${out.text}`,
  );
  assert.ok(
    lines.some((l) => l.includes('filetree 面板键位')),
    `应追加作用域段: ${out.text}`,
  );
  assert.ok(lines.some((l) => l.includes('关闭面板')), '作用域键位应在里面');
});

test('前缀键提示：不在插件层时**不**追加任何东西', () => {
  const cap = load({ mode: 'box' }, null, { scope: null });
  const out = cap.hint({ title: 'space', entries: goto() });
  assert.ok(!out.text.includes('面板键位'), `不该有作用域段: ${out.text}`);
});

test('show_scope_keys = false ⇒ 关掉追加（只看原生表）', () => {
  const cap = load({ mode: 'box', show_scope_keys: false }, null, {
    scope: 'filetree',
    hints: () => SCOPE_TABLE,
  });
  const out = cap.hint({ title: 'space', entries: goto() });
  assert.ok(!out.text.includes('面板键位'), `配置关掉后不该追加: ${out.text}`);
});

test('status 模式：作用域段也进那一行摘要', () => {
  const cap = load({ mode: 'status' }, null, { scope: 'dapui', hints: () => SCOPE_TABLE });
  cap.hint({ title: 'space', entries: goto() });
  const line = cap.echoes.at(-1);
  assert.ok(line.includes('dapui 面板键位'), `状态栏摘要应含作用域段: ${line}`);
});

test('原生子表很长时，截断只截原生表 —— 作用域段必须留着', () => {
  // 实测踩过：space 有 60 个子键，`cap_lines` 把 "… 还有 N 个键" 放在末尾，
  // 追加在后面的作用域段就被一起截掉了（面板键位一行都看不到）。
  const many = Array.from({ length: 60 }, (_, i) => ({ keys: `k${i}`, doc: `doc${i}` }));
  const cap = load({ mode: 'box', max_lines: 6, language: 'en' }, null, {
    scope: 'filetree',
    hints: () => SCOPE_TABLE,
  });
  const out = cap.hint({ title: 'space', entries: many });
  assert.ok(out.text.includes('还有'), `原生表应被截断: ${out.text}`);
  assert.ok(
    out.text.includes('filetree 面板键位') && out.text.includes('关闭面板'),
    `作用域段不能被截断挤掉: ${out.text}`,
  );
});
