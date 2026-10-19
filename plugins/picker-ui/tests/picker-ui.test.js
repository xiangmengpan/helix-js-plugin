// picker-ui node 级测试 —— **路径纪律**:条目存绝对、显示/匹配用相对。
//
// 运行:node --test plugins/picker-ui/tests/picker-ui.test.js
//      (node ≥ 24 不接受目录参数;零依赖 node:test)
//
// 由来:源(内建 filetree 是同一个病)把"当前 cwd"钉进模块状态 ⇒ `:cd` 之后
// 缓存下来的相对路径会**指错文件**。修复的判据是机械的:
//   · load 出来的条目(payload)必须是**绝对路径**;
//   · 显示/匹配可以削 cwd 前缀,但**不参与打开**。
// node 侧注入最小 helix 桩即可直接 require(engine 加载时 module 未定义会自动跳过导出)。
'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');

// ── helix 最小桩(须在 require 之前就位:plugin.js 顶层会调 plugin/define_config)──
const st = { cwd: '/proj/sub' };
global.helix = {
  plugin() {},
  define_config() {},
  register_command() {},
  echo() {},
  get_config: () => ({}),
  cwd: () => st.cwd,
};

const { to_abs, view, normalize, refresh_cwd_view } = require('../plugin.js');

test('to_abs:相对/./x 拼成绝对,绝对路径原样', () => {
  assert.equal(to_abs('a/b.rs', '/proj/sub'), '/proj/sub/a/b.rs');
  assert.equal(to_abs('./a/b.rs', '/proj/sub'), '/proj/sub/a/b.rs');
  assert.equal(to_abs('/etc/hosts', '/proj/sub'), '/etc/hosts', '已绝对 ⇒ 不要再拼前缀');
  assert.equal(to_abs('a.rs', ''), 'a.rs', '拿不到 cwd ⇒ 至少不要拼出 /a.rs');
});

test('normalize:字符串行与 {path|name} 行都归一化成**绝对**', () => {
  st.cwd = '/proj/sub';
  assert.deepEqual(normalize(['a.rs', './b/c.rs', '/abs/d.rs']), [
    '/proj/sub/a.rs',
    '/proj/sub/b/c.rs',
    '/abs/d.rs',
  ]);
  assert.deepEqual(normalize([{ path: './x/y.rs' }, { name: 'z.rs' }, null, 42]), [
    '/proj/sub/x/y.rs',
    '/proj/sub/z.rs',
  ]);
  assert.deepEqual(normalize(null), [], '形状不符 ⇒ 空表而不是抛错');
});

test('view:只削当前 cwd 前缀(cwd 之外的保持绝对)', () => {
  st.cwd = '/proj/sub';
  refresh_cwd_view();
  assert.equal(view('/proj/sub/a/b.rs'), 'a/b.rs');
  assert.equal(view('/proj/other/a.rs'), '/proj/other/a.rs', 'cwd 之外不削');
  assert.equal(view('/proj/submarine/a.rs'), '/proj/submarine/a.rs', '不是整段前缀 ⇒ 不削');
});

test('存储与显示解耦:同一个条目,payload 绝对、显示相对', () => {
  st.cwd = '/proj/sub';
  const items = normalize(['src/main.rs']);
  refresh_cwd_view();
  assert.equal(items[0], '/proj/sub/src/main.rs', 'payload(打开用)是绝对');
  assert.equal(view(items[0]), 'src/main.rs', '显示(和匹配)是相对');
});

test('cwd 变了以后,旧条目仍然指向原来的文件(这就是这次修复的全部意义)', () => {
  st.cwd = '/proj/sub';
  const items = normalize(['src/main.rs']); // load 时的 cwd
  st.cwd = '/elsewhere';                    // 之后 :cd
  refresh_cwd_view();
  assert.equal(items[0], '/proj/sub/src/main.rs', '旧条目不该跟着新 cwd 走');
  assert.equal(view(items[0]), '/proj/sub/src/main.rs', '新 cwd 之外 ⇒ 显示绝对,不再误导');
});
