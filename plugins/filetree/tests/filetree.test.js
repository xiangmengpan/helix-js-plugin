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

// helix 最小桩(plugin.js 顶层会调 helix.plugin / define_config / path_join)
// 注:`path_join` 是 2026-10 才加进加载期路径的(IS_WINDOWS 判定),之前没写进桩 ⇒ 本文件
// 整份 require 就抛错(测试红但没人复跑)。桩里按 POSIX 语义实现就够了,与真实主机无关。
global.helix = {
  plugin() {},
  define_config() {},
  register_command() {},
  echo() {},
  // persist=true：让 save_state/load_state 的**接线**也被测到（下面用内存文件系统兜住写盘）
  get_config: () => ({ persist: true }),
  plugins_dir: () => '/cfg/plugins',
  write_plugin_file: (rel, text) => {
    global.__state_file = text;
    return '/cfg/plugins/' + rel;
  },
  read_file: () => {
    if (global.__state_file === undefined) throw new Error('ENOENT');
    return global.__state_file;
  },
  icons: { dir: () => '▸', file: () => '·' },
  read_dir: () => [],
  path_join: (a, b) => (b === '' ? a + '/' : a.replace(/\/+$/, '') + '/' + b),
  path_name: (p) => String(p).split('/').pop() || null,
  path_parent: (p) => String(p).replace(/\/?[^/]+$/, '') || null,
  on() {},          // 注册期只登记事件,node 侧空转
  export() {},
  // 加载期就会调 `picker.define`（注册 `:filetree-find` 的源）⇒ 桩必须有
  picker: { define() {}, run() {} },
  draw: {},         // render 在 node 侧不跑(端到端归集成测试)
};

const {
  window_rows, scroll_top, match_name, filter_rows, highlight_segments, flat_items,
  literal_hits, grep_groups, grep_rows,
  compile_filter, dsl_hit, match_tree, glob_to_regex, parse_git_status, git_letter,
  opened_marker, worst_severity, diag_hit, diag_mark, new_panel, G,
  target_panel, panel_for_path,
  state_encode, state_decode, save_state, load_state, state_for_root,
} = require('../plugin.js');

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

// ============================ 搜索 / 过滤（P0）============================
//
// 为什么测这三个:`match_name`（匹配语义）、`filter_rows`（祖先保留 + 深度 + 树序）、
// `highlight_segments`（高亮分段）。渲染与按键路由归集成测试；这里钉住"算错一格就整棵树错位"
// 或"搜到了但缩进塌掉 / 祖先没了"这类纯逻辑错。

const n = (name, path, is_dir, children = null, expanded = false) => ({
  name, path, is_dir, expanded, children,
});

// /r
// ├─ src
// │  ├─ api
// │  │  └─ handler.rs
// │  └─ main.rs
// ├─ README.md
// └─ .git（空目录）
const fixture = () => n('r', '/r', true, [
  n('src', '/r/src', true, [
    n('api', '/r/src/api', true, [
      n('handler.rs', '/r/src/api/handler.rs', false),
    ]),
    n('main.rs', '/r/src/main.rs', false),
  ]),
  n('README.md', '/r/README.md', false),
  n('.git', '/r/.git', true, []),
], true);

const shape = (rows) => rows.map((r) => [r.node.name, r.depth]);

test('match_name:大小写不敏感的子序列,命中返回下标（贪心取最早）', () => {
  // "handler.rs": h0 a1 n2 d3 l4 e5 r6 .7 r8 s9
  assert.deepEqual(match_name('handler.rs', 'hrs'), [0, 6, 9]);
  assert.deepEqual(match_name('Handler.RS', 'hrs'), [0, 6, 9]);
  assert.deepEqual(match_name('abc', 'abc'), [0, 1, 2]);
  // 顺序不对 ⇒ 不命中（子序列不是"字符集合")
  assert.equal(match_name('abc', 'acb'), null);
  // 空查询不算命中（调用方在 query 为空时根本不走过滤路径）
  assert.equal(match_name('abc', ''), null);
});

test('filter_rows:命中节点的祖先全部保留,depth 保持真实层级', () => {
  const rows = filter_rows(fixture(), false, 'handler');
  assert.deepEqual(shape(rows), [['src', 0], ['api', 1], ['handler.rs', 2]]);
  // 祖先不高亮（hit=null），命中行给下标
  assert.equal(rows[0].hit, null);
  assert.equal(rows[1].hit, null);
  assert.deepEqual(rows[2].hit, [0, 1, 2, 3, 4, 5, 6]);
});

test('filter_rows:树序不减员 —— 多个命中按原顺序展开', () => {
  const rows = filter_rows(fixture(), false, 'rs');
  assert.deepEqual(shape(rows), [['src', 0], ['api', 1], ['handler.rs', 2], ['main.rs', 1]]);
});

test('filter_rows:隐藏目录跟随 show_hidden,不会靠搜索泄露 .git', () => {
  assert.deepEqual(filter_rows(fixture(), false, 'git'), []);
  assert.deepEqual(shape(filter_rows(fixture(), true, 'git')), [['.git', 0]]);
});

test('filter_rows:query 含 "/" 时按相对路径做子串匹配（不算自身,只钻内容）', () => {
  const rows = filter_rows(fixture(), false, 'src/api/');
  // src 与 api 都是"祖先行",真正命中只有 handler.rs
  assert.deepEqual(shape(rows), [['src', 0], ['api', 1], ['handler.rs', 2]]);
  assert.deepEqual(rows[2].hit, []); // 路径命中不高亮
});

test('filter_rows:空 query / 无命中都返回空表,不炸', () => {
  assert.deepEqual(filter_rows(fixture(), false, ''), []);
  assert.deepEqual(filter_rows(fixture(), false, 'zzzzzz'), []);
  assert.deepEqual(filter_rows(null, false, 'a'), []);
});

test('highlight_segments:连续命中合并成一段', () => {
  assert.deepEqual(highlight_segments('abc', [1]), [
    { text: 'a', hit: false }, { text: 'b', hit: true }, { text: 'c', hit: false },
  ]);
  assert.deepEqual(highlight_segments('abc', [0, 1]), [
    { text: 'ab', hit: true }, { text: 'c', hit: false },
  ]);
  // 无命中（含"仅祖先"与 tree 模式下的 undefined）→ 单段原文
  assert.deepEqual(highlight_segments('abc', []), [{ text: 'abc', hit: false }]);
  assert.deepEqual(highlight_segments('abc', undefined), [{ text: 'abc', hit: false }]);
  // 段拼回去必须与原名字逐字相等（高亮不能吞字/重复字）
  const segs = highlight_segments('handler.rs', match_name('handler.rs', 'hrs'));
  assert.equal(segs.map((s) => s.text).join(''), 'handler.rs');
});

// ============================ 全库索引（P1：rg --files + nucleo）============================
//
// 这三件事只能在这里钉：`flat_items` 的解析（CRLF/空行/根外路径）、全库行的**顺序来自 nucleo**
// 而不是树序、以及"同一 query 不重跑 nucleo"（`rows_for_view` 一次按键会被调好几次）。

test('flat_items:去空行/去 CR、按根算相对显示名,根外路径原样保留', () => {
  const raw = '/r/src/a.rs\r\n/r/b.rs\n\n  \n/other/x.rs\n';
  assert.deepEqual(flat_items(raw, '/r'), [
    { path: '/r/src/a.rs', rel: 'src/a.rs' },
    { path: '/r/b.rs', rel: 'b.rs' },
    { path: '/other/x.rs', rel: '/other/x.rs' }, // 不在根下：显示原路径（不猜）
  ]);
  assert.deepEqual(flat_items('', '/r'), []);
});

test('rows_for_view(全库):顺序来自 nucleo、会话只建一次、同一 query 不重跑、drop 释放', () => {
  const { rows_for_view, drop_flat } = require('../plugin.js');
  const calls = { session: 0, run: 0, drop: 0 };
  global.helix.fuzzy = {
    session: (items) => {
      calls.session++;
      assert.ok(items.every((i) => typeof i === 'string'), 'session 只收字符串');
      return 7;
    },
    // 故意让"后面的条目"排前面：断言顺序必须来自 nucleo，不是 items 顺序
    run: () => { calls.run++; return [{ index: 2, score: 9 }, { index: 0, score: 1 }]; },
    drop: (id) => { calls.drop++; assert.equal(id, 7); },
  };
  const panel = new_panel('/r');
  try {
    panel.root = '/r';
    panel.tree = fixture();
    panel.mode = 'find';
    panel.scope = 'all';
    panel.query = 'rs';
    panel.flat = {
      root: '/r',
      items: flat_items('/r/src/a.rs\n/r/b.rs\n/r/c.rs', '/r'),
      loading: false, error: null, sid: null, cache_key: null, cache_rows: [],
    };

    const rows1 = rows_for_view(panel);
    assert.deepEqual(rows1.map((r) => r.node.name), ['c.rs', 'src/a.rs'], '顺序 = nucleo 分数');
    assert.deepEqual(rows1[0].hit, match_name('c.rs', 'rs'), '全库行也要有高亮下标');
    assert.equal(rows1[0].depth, 0, '全库行是扁平的');

    rows_for_view(panel); // 第二次（同一个 query）⇒ 必须命中缓存
    assert.equal(calls.run, 1, '同一 query 不能重跑 nucleo');
    assert.equal(calls.session, 1, '会话只建一次');

    panel.query = 'c'; // 换 query ⇒ 重跑，但复用会话
    rows_for_view(panel);
    assert.equal(calls.run, 2);
    assert.equal(calls.session, 1);

    drop_flat(panel);
    assert.equal(calls.drop, 1, 'drop 要真的调 helix.fuzzy.drop');
    assert.equal(panel.flat, null);
  } finally {
    delete global.helix.fuzzy;
  }
});

test('rows_for_view(全库):空 query 给前 N 条且不高亮(还没输字,不是"全部命中")', () => {
  const { rows_for_view } = require('../plugin.js');
  const panel = new_panel('/r');
  try {
    panel.root = '/r';
    panel.tree = fixture();
    panel.mode = 'find';
    panel.scope = 'all';
    panel.query = '';
    panel.flat = {
      root: '/r',
      items: flat_items('/r/a.rs\n/r/b.rs', '/r'),
      loading: false, error: null, sid: null, cache_key: null, cache_rows: [],
    };
    const rows = rows_for_view(panel);
    assert.deepEqual(rows.map((r) => r.node.name), ['a.rs', 'b.rs']);
    assert.equal(rows[0].hit, null);
  } finally {
    // 无全局副作用
  }
});

// ============================ 内容搜索（P2：rg --json → 结果树）============================
//
// 这一段钉的是**解析**与**行形状**：`rg --json` 的 0 起始行号换算、事件类型过滤、
// 分组顺序、以及"文件行为父、命中行为子"的行形状（渲染与跳行都建立在它上面）。

test('literal_hits:每次字面出现都标上、大小写不敏感、重叠往后走', () => {
  assert.deepEqual(literal_hits('console.log(1)', 'console'), [0, 1, 2, 3, 4, 5, 6]);
  assert.deepEqual(literal_hits('Console.CONSOLE', 'console'), [0, 1, 2, 3, 4, 5, 6, 8, 9, 10, 11, 12, 13, 14]);
  assert.deepEqual(literal_hits('aaaa', 'aa'), [0, 1, 2, 3]); // 匹配位置 0 与 2
  assert.equal(literal_hits('abc', 'zz'), null);
  assert.equal(literal_hits('abc', ''), null);
  assert.equal(literal_hits('', 'a'), null);
});

// rg --json 的一小段真实形状（begin/end/summary 与 match 混在一起；CRLF 行尾）
const rg_out = [
  '{"type":"begin","data":{"path":{"text":"/r/src/a.rs"}}}',
  '{"type":"match","data":{"path":{"text":"/r/src/a.rs"},"lines":{"text":"let console = 1;\\n"},"line_number":3,"submatches":[{"match":{"text":"console"},"start":4,"end":11}]}}',
  '{"type":"match","data":{"path":{"text":"/r/src/a.rs"},"lines":{"text":"console.log(2)\\r\\n"},"line_number":9}}',
  '{"type":"end","data":{"path":{"text":"/r/src/a.rs"}}}',
  '{"type":"match","data":{"path":{"text":"/r/b.rs"},"lines":{"text":"x console\\n"},"line_number":1}}',
  '{"type":"summary","data":{"elapsed_total":{"secs":0,"nanos":1}}}',
  '这不是 JSON（不该出现，但也不能弄死整个搜索）',
].join('\n');

test('grep_groups:按文件分组、行号换成 0 起始、只认 match 事件、CRLF 去掉', () => {
  const g = grep_groups(rg_out, '/r');
  assert.deepEqual(g.map((x) => x.rel), ['src/a.rs', 'b.rs']);
  assert.deepEqual(g[0].matches, [
    { line: 2, text: 'let console = 1;' },   // 3 → 2
    { line: 8, text: 'console.log(2)' },     // 9 → 8，且 \r\n 去掉
  ]);
  assert.deepEqual(g[1].matches, [{ line: 0, text: 'x console' }]);
});

test('grep_groups:单文件条数 / 总条数 / 总文件数都有上限(大仓库不炸)', () => {
  const g = grep_groups(rg_out, '/r', { files: 10, matches: 10, per_file: 1 });
  assert.deepEqual(g.map((x) => x.matches.length), [1, 1]);
  const only = grep_groups(rg_out, '/r', { files: 1, matches: 10, per_file: 5 });
  assert.equal(only.length, 1, '文件数上限生效');
  // 非 UTF-8 路径只有 bytes 形式 ⇒ 跳过（宁缺不编）
  const bytes_only = '{"type":"match","data":{"path":{"bytes":"/w=="},"lines":{"text":"x\\n"},"line_number":1}}';
  assert.deepEqual(grep_groups(bytes_only, '/r'), []);
  assert.deepEqual(grep_groups('', '/r'), []);
});

test('grep_rows:文件行为父(depth 0)、命中行为子(depth 1, 带 0 起始行号),折叠只留父行', () => {
  const { grep_rows } = require('../plugin.js');
  const panel = new_panel('/r');
  try {
    panel.mode = 'grep';
    panel.query = 'console';
    panel.grep = { groups: grep_groups(rg_out, '/r'), loading: false, error: null, seq: 1 };
    const rows = grep_rows(panel);
    assert.deepEqual(rows.map((r) => [r.depth, r.node.name]), [
      [0, 'src/a.rs'],
      [1, 'let console = 1;'],
      [1, 'console.log(2)'],
      [0, 'b.rs'],
      [1, 'x console'],
    ]);
    assert.equal(rows[0].node.line, undefined, '文件行没有行号（Enter 语义不同）');
    assert.equal(rows[1].node.line, 2, '行号必须 0 起始');
    assert.deepEqual(rows[1].hit, literal_hits('let console = 1;', 'console'));

    // 折叠第一个文件组 ⇒ 只剩父行（状态挂在 group 上）
    panel.grep.groups[0].expanded = false;
    assert.deepEqual(grep_rows(panel).map((r) => r.node.name), ['src/a.rs', 'b.rs', 'x console']);
  } finally {
    // 无全局副作用
  }
});

// ============================ 过滤表达式（P3：DSL + git 谓词）============================
//
// DSL 是"用户会直接看见"的东西：globs 的 `/` 语义（basename vs 相对路径）、`**/` 的零层、
// `!` 排除、`#` 谓词的口径。这些都是纯逻辑，全在这里钉。

test('glob_to_regex:`*` 不跨目录、`**` 跨、`**/` 含零层、`?` 单字符', () => {
  assert.ok(glob_to_regex('*.rs').test('a.rs'));
  assert.ok(!glob_to_regex('*.rs').test('src/a.rs'), '`*` 不跨 /');
  assert.ok(glob_to_regex('src/*.rs').test('src/a.rs'));
  assert.ok(!glob_to_regex('src/*.rs').test('src/x/a.rs'));
  assert.ok(glob_to_regex('src/**').test('src/x/y/a.rs'), '`**` 跨目录');
  assert.ok(glob_to_regex('src/**/a.rs').test('src/a.rs'), '`**/` 要能命中零层');
  assert.ok(glob_to_regex('src/**/a.rs').test('src/x/y/a.rs'));
  assert.ok(glob_to_regex('test_?.js').test('test_1.js'));
  assert.ok(!glob_to_regex('test_?.js').test('test_12.js'));
  assert.ok(glob_to_regex('a+b.rs').test('a+b.rs'), '正则元字符要当字面量');
  assert.ok(!glob_to_regex('a+b.rs').test('aab.rs'));
  assert.ok(glob_to_regex('*.RS').test('a.rs'), '大小写不敏感');
});

test('compile_filter:空表达式 → null;三种词形态 + `!` 排除', () => {
  assert.equal(compile_filter(''), null);
  assert.equal(compile_filter('   '), null);
  const f = compile_filter('*.rs !*.gen.rs #M src/**');
  assert.deepEqual(f.items.map((x) => x.kind), ['glob', 'glob', 'git', 'glob']);
  assert.deepEqual(f.items.map((x) => !!x.negate), [false, true, false, false]);
  assert.equal(f.items[0].path_mode, false, '不含 / 的 glob 比 basename');
  assert.equal(f.items[3].path_mode, true, '含 / 的 glob 比相对路径');
  assert.equal(f.items[2].code, 'M');
  // `!` 单独一个词 = 空词 ⇒ 没有任何有效词 ⇒ 当作"不过滤"
  assert.equal(compile_filter('!'), null);
});

test('dsl_hit:全部词命中才算、`!` 取反、子串走相对路径、`#` 看 git 表', () => {
  const node = (name, path) => ({ name, path, is_dir: false });
  const git = new Map([['/r/src/a.rs', 'M'], ['/r/b.rs', '?']]);

  const only_rs = compile_filter('*.rs');
  assert.equal(dsl_hit(only_rs, node('a.rs', '/r/src/a.rs'), 'src/a.rs', git), true);
  assert.equal(dsl_hit(only_rs, node('b.md', '/r/b.md'), 'b.md', git), false);

  const not_gen = compile_filter('*.rs !*.gen.rs');
  assert.equal(dsl_hit(not_gen, node('a.gen.rs', '/r/a.gen.rs'), 'a.gen.rs', git), false);
  assert.equal(dsl_hit(not_gen, node('a.rs', '/r/a.rs'), 'a.rs', git), true);

  const mod_only = compile_filter('#M');
  assert.equal(dsl_hit(mod_only, node('a.rs', '/r/src/a.rs'), 'src/a.rs', git), true);
  assert.equal(dsl_hit(mod_only, node('b.rs', '/r/b.rs'), 'b.rs', git), false, '? 不是 M');
  assert.equal(
    dsl_hit(mod_only, node('a.rs', '/r/src/a.rs'), 'src/a.rs', null),
    false,
    '没有 git 表时 # 谓词一律不命中（不能静默放行）',
  );

  const untracked = compile_filter('#?');
  assert.equal(dsl_hit(untracked, node('b.rs', '/r/b.rs'), 'b.rs', git), true);

  const nested = compile_filter('src/'); // 子串（无通配符）走相对路径
  assert.equal(dsl_hit(nested, node('a.rs', '/r/src/a.rs'), 'src/a.rs', null), true);
  assert.equal(dsl_hit(nested, node('a.rs', '/r/lib/a.rs'), 'lib/a.rs', null), false);

  const excl_git = compile_filter('!#M');
  assert.equal(dsl_hit(excl_git, node('a.rs', '/r/src/a.rs'), 'src/a.rs', git), false);
  assert.equal(dsl_hit(excl_git, node('b.rs', '/r/b.rs'), 'b.rs', git), true);
});

test('match_tree:通用过滤遍历（祖先保留 / depth / 隐藏文件）—— filter_rows 现在是它的特例', () => {
  const rows = match_tree(fixture(), false, (node, rel) =>
    node.name.indexOf('handler') >= 0 ? [] : null,
  );
  assert.deepEqual(rows.map((r) => [r.node.name, r.depth]), [
    ['src', 0], ['api', 1], ['handler.rs', 2],
  ]);
  assert.equal(rows[0].hit, null, '祖先行不高亮');
  assert.deepEqual(rows[2].hit, []);
  // 隐藏文件仍受 show_hidden 管（DSL 不该绕开它）
  assert.deepEqual(match_tree(fixture(), false, (n) => (n.name === '.git' ? [] : null)), []);
  assert.equal(match_tree(fixture(), true, (n) => (n.name === '.git' ? [] : null)).length, 1);
});

test('parse_git_status:XY→单字母、重命名占两段、路径原样保留(空格/中文)、0 起始行号无关', () => {
  // 真实 -z 形状：`XY path\0`；重命名是 `R  new\0old\0`
  const out = [' M src/a.rs', '?? b.md', 'A  c.rs', 'D  gone.rs', 'R  new name.rs', 'old', ''].join('\0');
  const map = parse_git_status(out, '/r');
  assert.deepEqual(
    [...map.entries()].sort(),
    [
      ['/r/b.md', '?'],
      ['/r/c.rs', 'A'],
      ['/r/gone.rs', 'D'],
      ['/r/new name.rs', 'R'],
      ['/r/src/a.rs', 'M'],
    ],
    '重命名的原路径不能被当成本次条目',
  );
  assert.equal(git_letter(' ', 'M'), 'M');
  assert.equal(git_letter('?', '?'), '?');
  assert.equal(git_letter('A', ' '), 'A');
  assert.equal(git_letter('R', ' '), 'R');
  assert.equal(git_letter('M', 'M'), 'M');
  assert.deepEqual(parse_git_status('', '/r'), new Map());
});

// ============================ 行尾标记（③ 多打开标记）============================
test('opened_marker:● 当前 / · 已打开非当前 / null 未打开', () => {
  const opened = new Set(['/r/a.rs', '/r/b.rs']);
  assert.deepEqual(opened_marker('/r/a.rs', '/r/a.rs', opened), { ch: '●', style: 'ui.text.focus' });
  assert.deepEqual(opened_marker('/r/b.rs', '/r/a.rs', opened), { ch: '·', style: 'ui.virtual' });
  assert.equal(opened_marker('/r/c.rs', '/r/a.rs', opened), null);
  // 没有 path / 还没播种（opened=null）都不能炸
  assert.equal(opened_marker(null, '/r/a.rs', opened), null);
  assert.deepEqual(opened_marker('/r/a.rs', '/r/a.rs', null), { ch: '●', style: 'ui.text.focus' });
  assert.equal(opened_marker('/r/b.rs', '/r/a.rs', null), null);
  // S.current 只在 buffer-open 更新 ⇒ 关掉当前 buffer 后它陈旧：
  // 已不在 opened 集合里就不该再画任何标记（否则给已关闭的文件留一个假 ●）
  assert.equal(opened_marker('/r/a.rs', '/r/a.rs', new Set(['/r/b.rs'])), null);
});

// ============================ 诊断标记 / #E #W 谓词（②）============================
//
// 核心给的 `severity` 是**字符串**（"error"/"warning"/"info"/"hint"）—— 这点很容易写错成数字，
// 所以单独钉住；谓词的语义也钉住（`#W` = warning **或更严重**）。

test('worst_severity:取最严重的字符串级别,未知/空数组 → null', () => {
  assert.equal(worst_severity([{ severity: 'hint' }, { severity: 'error' }]), 'error');
  assert.equal(worst_severity([{ severity: 'info' }, { severity: 'warning' }]), 'warning');
  assert.equal(worst_severity([{ severity: 'ERROR' }]), 'error', '大小写不敏感');
  assert.equal(worst_severity([{ severity: 1 }]), null, '数字级别不算（核心给的是字符串）');
  assert.equal(worst_severity([{ severity: null }, {}]), null);
  assert.equal(worst_severity([]), null);
  assert.equal(worst_severity(null), null);
});

test('diag_hit:#E 精确 / #W 含更严重 / #H 任意级别 / 空条目一律不命中', () => {
  const err = { severity: 'error', count: 1 };
  const warn = { severity: 'warning', count: 2 };
  const info = { severity: 'info', count: 1 };
  assert.equal(diag_hit(err, 'E'), true);
  assert.equal(diag_hit(warn, 'E'), false, '#E 只认 error');
  assert.equal(diag_hit(warn, 'W'), true);
  assert.equal(diag_hit(err, 'W'), true, '#W = warning 或更严重');
  assert.equal(diag_hit(info, 'W'), false);
  assert.equal(diag_hit(info, 'I'), true);
  assert.equal(diag_hit(info, 'H'), true);
  assert.equal(diag_hit(null, 'H'), false, '没有诊断信息 ≠ 命中');
  assert.equal(diag_hit(err, 'X'), false, '未知谓词字母不命中（不静默放行）');
});

test('diag_mark:最严重级别 → 单字母 + 主题 scope', () => {
  const saved = G.diag;
  try {
    G.diag = new Map([
      ['/r/e.rs', { severity: 'error', count: 3 }],
      ['/r/w.rs', { severity: 'warning', count: 1 }],
      ['/r/i.rs', { severity: 'info', count: 1 }],
      ['/r/h.rs', { severity: 'hint', count: 1 }],
    ]);
    assert.deepEqual(diag_mark('/r/e.rs'), { ch: 'E', style: 'error' });
    assert.deepEqual(diag_mark('/r/w.rs'), { ch: 'W', style: 'warning' });
    assert.deepEqual(diag_mark('/r/i.rs'), { ch: 'i', style: 'ui.virtual' });
    assert.deepEqual(diag_mark('/r/h.rs'), { ch: 'h', style: 'ui.virtual' });
    assert.equal(diag_mark('/r/none.rs'), null);
    G.diag = null;
    assert.equal(diag_mark('/r/e.rs'), null, '没播种也不能炸');
  } finally {
    G.diag = saved;
  }
});

test('编译期：#E/#W 是 diag 谓词、#M/#? 仍是 git 谓词,且 dsl_hit 能按诊断过滤', () => {
  const f = compile_filter('#E #W !#H');
  assert.deepEqual(f.items.map((x) => [x.kind, x.code, !!x.negate]), [
    ['diag', 'E', false],
    ['diag', 'W', false],
    ['diag', 'H', true],
  ]);
  const node = (name, path) => ({ name, path, is_dir: false });
  const diag = new Map([['/r/e.rs', { severity: 'warning', count: 1 }], ['/r/x.rs', { severity: 'error', count: 1 }]]);
  assert.equal(dsl_hit(compile_filter('#E'), node('x.rs', '/r/x.rs'), 'x.rs', null, diag), true);
  assert.equal(dsl_hit(compile_filter('#E'), node('e.rs', '/r/e.rs'), 'e.rs', null, diag), false);
  assert.equal(dsl_hit(compile_filter('#W'), node('e.rs', '/r/e.rs'), 'e.rs', null, diag), true);
  assert.equal(dsl_hit(compile_filter('#E'), node('x.rs', '/r/x.rs'), 'x.rs', null, null), false);
  // git 谓词没被误当成诊断谓词
  assert.equal(compile_filter('#M').items[0].kind, 'git');
});

// ============================ 双实例（第 4 步：注册表 / 目标解析 / 隔离）============================
//
// 这一段钉三件事：① `target_panel()` 的优先级阶梯；② `panel_for_path()` 按 root 归属；
// ③ 两个实例的状态**物理隔离**（各自的 tree/mode/query/索引互不可见）。

/** 临时把 G 的面板表换掉（用完还原） */
function with_panels(panels, active, fn) {
  const saved = { panels: G.panels, active: G.active };
  try {
    G.panels = new Map(panels.map((p, i) => [i + 1, p]));
    G.active = active;
    return fn();
  } finally {
    G.panels = saved.panels;
    G.active = saved.active;
  }
}

test('target_panel 阶梯：active 优先 → 只剩一个 → null', () => {
  const a = new_panel('/a');
  const b = new_panel('/b');
  const single = new_panel('/s');
  with_panels([a, b], 2, () => assert.equal(target_panel(), b, 'active 优先'));
  with_panels([a, b], null, () => assert.equal(target_panel(), null, '两个但没 active ⇒ 无法判定'));
  with_panels([single], null, () => assert.equal(target_panel(), single, '只剩一个就是它'));
  with_panels([], 99, () => assert.equal(target_panel(), null, 'active 指向已关闭的 id ⇒ null'));
});

test('panel_for_path：root 是祖先才算；不在任何 root 下 → null', () => {
  const a = new_panel('/a');
  const b = new_panel('/b');
  with_panels([a, b], 1, () => {
    assert.equal(panel_for_path('/a/src/x.rs'), a);
    assert.equal(panel_for_path('/b/y.rs'), b);
    assert.equal(panel_for_path('/a'), a, 'root 本身也算');
    assert.equal(panel_for_path('/c/z.rs'), null);
    assert.equal(panel_for_path(null), null);
  });
});

test('双实例隔离：rows/索引/模式各归各，drop 一个不动另一个', () => {
  const { rows_for_view, drop_flat } = require('../plugin.js');
  const a = new_panel('/r');
  const b = new_panel('/r');
  a.tree = fixture();
  b.tree = fixture();
  a.mode = 'find';
  a.query = 'readme';
  b.mode = 'tree';

  const rows_a = rows_for_view(a);
  const rows_b = rows_for_view(b);
  assert.deepEqual(rows_a.map((r) => r.node.name), ['README.md'], 'A 被过滤（fixture 里是大写）');
  assert.deepEqual(rows_b.map((r) => r.node.name), ['src', 'README.md'], 'B 不受 A 的 query 影响');

  // 索引/会话也是每实例一份
  const calls = { drop: 0 };
  global.helix.fuzzy = { session: () => 1, run: () => [], drop: () => { calls.drop++; } };
  try {
    a.flat = { root: '/r', items: [], loading: false, error: null, sid: 7, cache_key: null, cache_rows: [] };
    b.flat = { root: '/r', items: [], loading: false, error: null, sid: 8, cache_key: null, cache_rows: [] };
    drop_flat(a);
    assert.equal(calls.drop, 1, '只释放 A 的会话');
    assert.equal(a.flat, null);
    assert.ok(b.flat, 'B 的索引不能被 A 的关闭带走');
  } finally {
    delete global.helix.fuzzy;
  }
});

// ============================ 持久化 v2（第 5 步）============================
//
// v2 按 **root** 一条（与"哪个面板"解耦：下次开机按 root 匹配回来）。
// 这里钉：往返、v1 迁移、坏文件降级、按 root 去重、条数/展开数封顶 —— 都是纯逻辑。

test('state v2：往返 + 按 root 去重（后者胜）+ 条数封顶', () => {
  const raw = state_encode(
    [
      { root: '/a', filter: '*.rs', expanded: ['/a/src'] },
      { root: '/b', filter: '', expanded: [] },
      { root: '/a', filter: '*.md', expanded: ['/a/docs'] }, // 同 root 后者胜
    ],
    2,
  );
  const parsed = JSON.parse(raw);
  assert.equal(parsed.version, 2, '写的是 v2');
  assert.deepEqual(
    parsed.panels.map((p) => [p.root, p.filter]),
    [['/a', '*.md'], ['/b', '']],
    '去重 + 保持首次出现顺序',
  );
  assert.deepEqual(state_decode(raw).map((p) => p.root), ['/a', '/b'], '往返一致');

  // 条数封顶
  const many = state_encode(
    Array.from({ length: 5 }, (_, i) => ({ root: '/p' + i, filter: '', expanded: [] })),
    2,
  );
  assert.equal(JSON.parse(many).panels.length, 2, 'max 条封顶');

  // 展开目录封顶（200/条）
  const fat = state_encode([{ root: '/a', filter: '', expanded: Array.from({ length: 500 }, (_, i) => '/a/' + i) }], 2);
  assert.equal(JSON.parse(fat).panels[0].expanded.length, 200);
});

test('state v1 迁移：老文件（单面板）读成一条,不白丢', () => {
  const v1 = JSON.stringify({ version: 1, root: '/old', filter: '*.rs', expanded: ['/old/src'] });
  assert.deepEqual(state_decode(v1), [{ root: '/old', filter: '*.rs', expanded: ['/old/src'] }]);
  // 没有 version 字段但有 root：同样按 v1 处理
  assert.deepEqual(state_decode(JSON.stringify({ root: '/x', expanded: [] })), [
    { root: '/x', filter: '', expanded: [] },
  ]);
});

test('state_decode：坏文件/空文件/形状不对一律空表（读盘路径不能崩）', () => {
  assert.deepEqual(state_decode(''), []);
  assert.deepEqual(state_decode('{not json'), []);
  assert.deepEqual(state_decode('null'), []);
  assert.deepEqual(state_decode('[]'), []);
  assert.deepEqual(state_decode(JSON.stringify({ version: 2, panels: 'nope' })), []);
  assert.deepEqual(state_decode(JSON.stringify({ version: 2, panels: [{ root: '' }, { nope: 1 }] })), []);
});

test('save_state / load_state：写**全部**实例,按 root 能取回；关到空不抹记忆', () => {
  const saved = { panels: G.panels, active: G.active };
  const savedFile = global.__state_file;
  try {
    const a = new_panel('/a');
    const b = new_panel('/b');
    a.tree = fixture();
    b.tree = fixture();
    a.filter = '*.rs';
    G.panels = new Map([[1, a], [2, b]]);
    G.active = 2;

    save_state();
    const decoded = state_decode(global.__state_file);
    assert.deepEqual(decoded.map((e) => [e.root, e.filter]), [['/a', '*.rs'], ['/b', '']], '两个实例都写进去了');
    assert.deepEqual(state_for_root('/a'), decoded[0], '按 root 取回');
    assert.equal(state_for_root('/nope'), null);

    // 关到空（注册表为空）时**不要**用空表覆盖旧文件
    G.panels = new Map();
    save_state();
    assert.deepEqual(state_decode(global.__state_file).map((e) => e.root), ['/a', '/b'], '关到空不抹记忆');
  } finally {
    G.panels = saved.panels;
    G.active = saved.active;
    global.__state_file = savedFile;
  }
});

test('load_state：没有文件（首次运行）⇒ 空表,不炸', () => {
  const savedFile = global.__state_file;
  try {
    global.__state_file = undefined;
    assert.deepEqual(load_state(), []);
  } finally {
    global.__state_file = savedFile;
  }
});
