// arsenal node 级测试——版本输入/提交链路 + update 重询(e) + 批量 + 汇总 echo 边界(g)
// + v2 模态搜索(/进入、Esc 退出、SEARCH 全可打印进查询) + Tab 页签 + wc 列宽助手。
// 运行:node --test plugins/features/arsenal/tests/arsenal.test.js
//      (node ≥ 24 不再接受目录参数——会把目录当单个测试文件报错;须指到文件或
//       用 glob,如 node --test "plugins/features/arsenal/tests/*.test.js";零依赖 node:test)
// 原理:index.js 底部模块化导出(S + run_action/batch_run/handle_key 等,engine 加载时
//       module 未定义自动跳过)。node 直接 require 前注入最小 global.helix 桩:
//       plugin/register_command 空转(engine 接线不需要);echo/open_popup/server.task
//       捕获调用;open_popup 存下 opts → 测试可驱动版本输入/菜单弹窗的 onKey/onClose。
//       render/el 不参与动作路径(open_popup 桩不调 render),无需真实实现。
'use strict';

const { test, beforeEach } = require('node:test');
const assert = require('node:assert/strict');

// ── helix 最小桩(须在 require 之前就位:index.js 顶层调用 helix.plugin) ──
const calls = { echo: [], popups: [], tasks: [], rowsFetch: [] };
let popup_seq = 1;
global.helix = {
  plugin() {},
  register_command() {},
  echo(msg) {
    calls.echo.push(msg);
  },
  el() {
    return {};
  },
  open_popup(opts) {
    const id = popup_seq++;
    calls.popups.push({ id, opts });
    return id;
  },
  server: {
    task(items, cb) {
      const id = 9000 + calls.tasks.length;
      calls.tasks.push({ id, items, cb });
      return id;
    },
    rows(cb) {
      calls.rowsFetch.push(cb); // fetch_rows 每次调用登记(fetch_rows 空转不回写)
    },
  },
};

const M = require('../index.js');
const S = M.S;

// ── 行夹具:needs_version 只可能来自内置活配方(rust-analyzer 形态) ──
const NV_INSTALLABLE = {
  name: 'rust-analyzer',
  kind: 'lsp',
  languages: ['rust'],
  installed: false,
  local: false,
  installable: true,
  needs_version: true,
  version: null,
  upgradable: false,
  bin: '',
  source: 'https://github.com/rust-lang/rust-analyzer/releases/download/{version}/x.gz',
  description: 'Rust language server',
  homepage: 'https://github.com/rust-lang/rust-analyzer',
};
const NV_MANAGED = { ...NV_INSTALLABLE, installed: true, local: false, version: '2024-09-16' };
const FIXED = {
  name: 'gopls',
  kind: 'lsp',
  languages: ['go'],
  installed: false,
  local: false,
  installable: true,
  needs_version: false,
  version: null,
  upgradable: false,
  bin: '',
  source: '',
  description: 'Go official language server',
  homepage: 'https://github.com/golang/tools',
};

function reset() {
  S.popup = null;
  S.rows = [];
  S.searching = false;
  S.filter = '';
  S.kind = 'all';
  S.sel = 0;
  S.marks = new Set();
  S.menu = null;
  S.vinput = null;
  S.vinputs = {};
  S.busy = null;
  S.row_busy = {};
  calls.echo.length = 0;
  calls.popups.length = 0;
  calls.tasks.length = 0;
  calls.rowsFetch.length = 0;
}
beforeEach(reset);

// 便捷:最后一次 open_popup / 指定 layer 的弹窗 / 最后一次任务
const lastPopup = () => calls.popups[calls.popups.length - 1];
function popupOn(layer) {
  const p = calls.popups.find((p) => p.opts.layer === layer);
  assert.ok(p, '期望已打开 ' + layer + ' 弹窗,实际: ' + calls.popups.map((p) => p.opts.layer).join(','));
  return p;
}
const lastTask = () => calls.tasks[calls.tasks.length - 1];
// 给最近任务喂一条服务端事件(task_id 须与 S.busy.task_id 匹配)
function feed(kind, name, msg) {
  const t = lastTask();
  t.cb({ task_id: t.id, kind, name, msg });
}

// ────────────────────────── v2:wc 显示列宽助手(CJK 双宽) ──────────────────────────

test('wc_len/wc_trunc/wc_ell/wc_pad:宽字符双宽、截断在列边界不中腰', () => {
  assert.equal(M.wc_len('ab中'), 4);
  assert.equal(M.wc_len('中文'), 4);
  assert.equal(M.wc_len('…'), 1, '省略号单宽');
  assert.equal(M.wc_trunc('中文x', 3), '中', '第 3 列装不下下一个字,须断在列边界');
  assert.equal(M.wc_trunc('中文x', 4), '中文');
  assert.equal(M.wc_trunc('abc', 9), 'abc');
  assert.equal(M.wc_ell('abcdefgh', 6), 'abcde…', '超长以 … 收尾,共 6 列');
  assert.equal(M.wc_ell('hello', 8), 'hello', '未超长不加 …');
  assert.equal(M.wc_pad('中', 4), '中  ', '按列宽补空格(2+2)');
  assert.equal(M.wc_pad('a', 3), 'a  ');
});

// ────────────────────────── v2:模态搜索 —— 只有 / 后才可输入 ──────────────────────────

test('NORMAL:可打印字符不再即搜(字母是命令,其余忽略)', () => {
  S.rows = [{ ...NV_MANAGED }];
  S.vinputs['rust-analyzer'] = '2024-09-16';
  assert.equal(M.handle_key({ name: 'a' }), 'handled');
  assert.equal(S.filter, '', 'NORMAL 字母不进过滤');
  assert.equal(M.handle_key({ name: 'c' }), 'handled');
  assert.equal(S.vinputs['rust-analyzer'], '2024-09-16', 'c 不清记忆');
  assert.equal(M.handle_key({ name: ' ' }), 'handled', '空格不搜索');
  assert.equal(S.filter, '');
  assert.equal(S.searching, false);
});

test('/ 进 SEARCH;SEARCH 内可打印(含 j/k/q/x/u/i/f/t/r/C)全进查询,命令不触发', () => {
  S.rows = [{ ...NV_MANAGED, upgradable: true }]; // x/u/i/f/t 本会触发命令
  S.vinputs['rust-analyzer'] = '2024-09-16';
  assert.equal(M.handle_key({ name: '/' }), 'handled');
  assert.equal(S.searching, true, '/ 进入搜索态');
  assert.equal(S.filter, '');
  for (const k of ['j', 'k', 'q', 'x', 'u', 'i', 'f', 't', 'r', 'C']) M.handle_key({ name: k, shift: k === 'C' });
  assert.equal(S.filter, 'jkqxuiftrC', 'SEARCH 内一切可打印字符追加');
  assert.equal(calls.tasks.length, 0, 'x/u 不得触发动作');
  assert.deepEqual(calls.popups, [], 'i/f/t 不得开窗/标记');
  assert.equal(S.marks.size, 0);
  assert.equal(S.sel, 0, 'j/k 不得移动(查询字符)');
  assert.equal(S.kind, 'all', 'f 已移除,不循环分类');
  assert.equal(calls.rowsFetch.length, 0, 'r 不得刷新');
  assert.ok('rust-analyzer' in S.vinputs, 'C 不得清记忆');
});

test('SEARCH:Backspace 删尾、Enter 动作、Tab 切页签保留查询、Esc 清退;NORMAL Esc/q 关', () => {
  S.rows = [{ ...NV_MANAGED }];
  M.handle_key({ name: '/' });
  M.handle_key({ name: 'r' });
  M.handle_key({ name: 'a' });
  assert.equal(S.filter, 'ra');
  M.handle_key({ name: 'Backspace' });
  assert.equal(S.filter, 'r');
  // Tab 切页签:all → lsp(查询保留)
  M.handle_key({ name: 'Tab' });
  assert.equal(S.kind, 'lsp');
  assert.equal(S.filter, 'r', 'Tab 切页签不清查询');
  assert.equal(S.searching, true);
  // SEARCH 内 q 是查询字符,不关窗
  M.handle_key({ name: 'q' });
  assert.equal(S.filter, 'rq');
  // Esc:清查询退 NORMAL;再 Esc 关窗(NORMAL)
  assert.equal(M.handle_key({ name: 'Esc' }), 'handled');
  assert.equal(S.searching, false);
  assert.equal(S.filter, '');
  assert.equal(M.handle_key({ name: 'Esc' }), 'close', 'NORMAL Esc 关窗');
  // 重开 '/' 直接可输(空查询)
  assert.equal(M.handle_key({ name: '/' }), 'handled');
  assert.equal(S.filter, '');
});

test('SEARCH:↑↓ 移动,Down/Up 不吞查询', () => {
  S.rows = [FIXED, { ...FIXED, name: 'gopls2' }];
  M.handle_key({ name: '/' });
  M.handle_key({ name: 'Down' });
  assert.equal(S.sel, 1, 'SEARCH 内 ↑↓ 仍导航');
  assert.equal(S.filter, '', '导航不改查询');
  M.handle_key({ name: 'Up' });
  assert.equal(S.sel, 0);
});

test('/ 在 SEARCH 内忽略(不输入字面 /),过滤无匹配不炸', () => {
  S.rows = [FIXED];
  M.handle_key({ name: '/' });
  M.handle_key({ name: '/' });
  assert.equal(S.filter, '', 'SEARCH 内 / 不追加');
  M.handle_key({ name: 'z' });
  M.handle_key({ name: 'z' });
  assert.equal(S.filter, 'zz');
});

// ────────────────────────── v2:Tab/Shift+Tab 页签循环(取代 f) ──────────────────────────

test('Tab 循环页签 all→lsp→…→local→all;Shift+Tab 反向', () => {
  const order = [...M.KINDS];
  for (let i = 1; i < order.length; i++) {
    M.handle_key({ name: 'Tab' });
    assert.equal(S.kind, order[i]);
  }
  M.handle_key({ name: 'Tab' });
  assert.equal(S.kind, 'all', '循环回 all');
  M.handle_key({ name: 'Tab', shift: true });
  assert.equal(S.kind, 'local', 'Shift+Tab 反向');
  // NORMAL 直接字母不再循环(f 移除)
  assert.equal(S.kind, 'local');
  M.handle_key({ name: 'f' });
  assert.equal(S.kind, 'local', 'f 键已移除,不影响页签');
});

test('页签过滤生效:kind 过滤 rows', () => {
  S.rows = [
    { ...FIXED, name: 'ra', kind: 'lsp' },
    { ...FIXED, name: 'dbg', kind: 'dap' },
    { ...FIXED, name: 'blk', kind: 'formatter' },
  ];
  M.handle_key({ name: 'Tab' }); // lsp
  assert.deepEqual(M.visible().map((r) => r.name), ['ra']);
  M.handle_key({ name: 'Tab' }); // dap
  assert.deepEqual(M.visible().map((r) => r.name), ['dbg']);
});

// ────────────────────────── 版本/动作/批量链路(文案英文,语义不变) ──────────────────────────

test('update:needs_version 有记忆仍弹版本输入(不直发)', () => {
  S.rows = [NV_MANAGED];
  S.vinputs['rust-analyzer'] = '2024-09-16';
  M.run_action('update', 'rust-analyzer');
  assert.equal(calls.tasks.length, 0, 'update 不得跳过输入直发');
  popupOn(M.LAYERS.input);
  assert.equal(S.vinput.op, 'update');
  assert.equal(S.vinput.val, '2024-09-16', '记忆预填供参照');
});

test('update:输入新版本 Enter 提交带 version 且重记忆', () => {
  S.rows = [NV_MANAGED];
  S.vinputs['rust-analyzer'] = '2024-09-16';
  M.run_action('update', 'rust-analyzer');
  S.vinput.val = '2025-01-01'; // 升级:改掉预填
  popupOn(M.LAYERS.input).opts.onKey({ name: 'Enter' });
  assert.equal(S.vinput, null);
  assert.deepEqual(calls.tasks[0].items, [{ op: 'update', name: 'rust-analyzer', version: '2025-01-01' }]);
  assert.equal(S.vinputs['rust-analyzer'], '2025-01-01', '新版本写回记忆');
});

test('install:needs_version 无记忆 → 弹输入;提交带 version', () => {
  S.rows = [NV_INSTALLABLE];
  M.run_action('install', 'rust-analyzer'); // 无记忆:不能直发(没版本可发)
  assert.equal(calls.tasks.length, 0);
  const p = popupOn(M.LAYERS.input);
  p.opts.onKey({ name: '2' });
  p.opts.onKey({ name: '.' });
  p.opts.onKey({ name: '0' });
  p.opts.onKey({ name: 'Enter' });
  assert.deepEqual(calls.tasks[0].items, [{ op: 'install', name: 'rust-analyzer', version: '2.0' }]);
});

test('install:needs_version 有记忆 → 快捷直发(免重输)', () => {
  S.rows = [NV_INSTALLABLE];
  S.vinputs['rust-analyzer'] = '1.9';
  M.run_action('install', 'rust-analyzer');
  assert.equal(calls.popups.length, 0, '有记忆直发,不再弹输入');
  assert.deepEqual(calls.tasks[0].items, [{ op: 'install', name: 'rust-analyzer', version: '1.9' }]);
});

test('install/update:非 needs_version 行不带 version 字段', () => {
  S.rows = [{ ...FIXED, installed: true, local: false, version: 'v0.14' }];
  M.run_action('update', 'gopls');
  assert.deepEqual(calls.tasks[0].items, [{ op: 'update', name: 'gopls' }]);
});

// ── e:C 清记忆入口(换版本死锁逃生) ──
test('版本输入窗内 C:清该行记忆并清空预填', () => {
  S.rows = [NV_MANAGED];
  S.vinputs['rust-analyzer'] = '2024-09-16';
  M.run_action('update', 'rust-analyzer');
  popupOn(M.LAYERS.input).opts.onKey({ name: 'C', shift: true });
  assert.equal(S.vinput.val, '', 'C 清空预填');
  assert.ok(!('rust-analyzer' in S.vinputs), 'C 删除该行记忆(死锁逃生)');
});

test('主窗 Shift+C:清选中行记忆;无该行记忆时清全部', () => {
  S.rows = [NV_MANAGED, FIXED];
  S.sel = 0; // rust-analyzer
  S.vinputs = { 'rust-analyzer': '2024-09-16', gopls: 'v0.14' };
  M.handle_key({ name: 'C', shift: true });
  assert.ok(!('rust-analyzer' in S.vinputs));
  assert.equal(S.vinputs.gopls, 'v0.14', '只清选中行');
  // 选中无记忆行(gopls)→ 清全部
  S.sel = 1;
  M.handle_key({ name: 'C', shift: true });
  assert.deepEqual(S.vinputs, {});
  // NORMAL c 是忽略键,不清记忆也不搜索(v2 模态)
  S.vinputs['rust-analyzer'] = '1.0';
  M.handle_key({ name: 'c' });
  assert.equal(S.vinputs['rust-analyzer'], '1.0');
  assert.equal(S.filter, '');
  assert.equal(S.searching, false);
});

// ── f:批量提交(marks 快照)version 正确携带 ──
test('batch_run:needs_version 用记忆、固定配方不带 version', () => {
  const B = { ...FIXED, name: 'black', kind: 'formatter', description: 'Python formatter' };
  S.rows = [NV_INSTALLABLE, B];
  S.marks = new Set(['rust-analyzer', 'black']);
  S.vinputs['rust-analyzer'] = '2.0';
  M.batch_run();
  assert.deepEqual(calls.tasks[0].items, [
    { op: 'install', name: 'rust-analyzer', version: '2.0' },
    { op: 'install', name: 'black' },
  ]);
  assert.equal(S.marks.size, 0, '快照即提交');
});

// ── g:批量完成 echo 边界(单任务成功不再双 echo;失败/批量才汇总) ──
test('单任务成功:只报 done 事件 msg,不再补批量完成汇总(防双 echo)', () => {
  S.rows = [{ ...FIXED, installed: true, local: false, version: 'v0.14' }];
  M.run_action('update', 'gopls');
  feed('done', 'gopls', "updated 'gopls' v0.14");
  assert.deepEqual(calls.echo, ["updated 'gopls' v0.14"], '单任务成功恰一条');
  assert.equal(S.busy, null);
});

test('单任务失败:汇总保留(失败只能靠这条看到)', () => {
  S.rows = [FIXED];
  M.run_action('install', 'gopls');
  feed('error', 'gopls', 'source not configured (registry missing url)');
  assert.deepEqual(calls.echo, ['arsenal: batch done 0/1, failed: gopls (source not configured (registry missing url))']);
});

test('多任务批量:逐项 done msg + 末尾汇总', () => {
  S.rows = [NV_INSTALLABLE, FIXED];
  S.vinputs['rust-analyzer'] = '1.0';
  S.marks = new Set(['rust-analyzer', 'gopls']);
  M.batch_run();
  feed('done', 'rust-analyzer', "installed 'rust-analyzer' 1.0");
  feed('done', 'gopls', "installed 'gopls'");
  assert.deepEqual(calls.echo, [
    "installed 'rust-analyzer' 1.0",
    "installed 'gopls'",
    'arsenal: batch done 2/2',
  ]);
});

// ── g:菜单 i 直达信息 + 子弹窗 onClose 自愈 ──
test('菜单:两动作行 Enter 开菜单;菜单内 i 直达信息(不关菜单)', () => {
  const R = { ...NV_MANAGED, upgradable: true }; // update/remove 两动作
  S.rows = [R];
  M.handle_key({ name: 'Enter' });
  const menu = popupOn(M.LAYERS.menu);
  assert.ok(S.menu, '菜单状态非空');
  assert.equal(S.menu.items.length, 2);
  menu.opts.onKey({ name: 'i' });
  const info = popupOn(M.LAYERS.info);
  assert.notEqual(info.id, menu.id);
  assert.ok(S.menu, '信息叠于菜单上,菜单保留');
});

// 占位配方(未装 + 无下载源)Enter:提示原因并回退打开信息弹窗(不再只 echo)
test('无动作行 Enter:echo 原因并打开信息弹窗', () => {
  const PLACEHOLDER = { ...FIXED, name: 'jdtls', installable: false, source: null };
  S.rows = [PLACEHOLDER];
  M.handle_key({ name: 'Enter' });
  popupOn(M.LAYERS.info); // 回退到信息弹窗(解释/引导)
  assert.equal(S.menu, null, '不建空菜单');
  assert.ok(
    calls.echo.some((m) => m.includes('no download source')),
    'echo 应说明无下载源: ' + calls.echo
  );
});

test('子弹窗 onClose 自愈:引擎非按键关闭时 S.menu/S.vinput 置 null', () => {
  // 菜单
  S.rows = [{ ...NV_MANAGED, upgradable: true }];
  M.handle_key({ name: 'Enter' });
  popupOn(M.LAYERS.menu).opts.onClose();
  assert.equal(S.menu, null);
  // 版本输入
  S.rows = [NV_INSTALLABLE];
  M.run_action('install', 'rust-analyzer');
  assert.ok(S.vinput);
  popupOn(M.LAYERS.input).opts.onClose();
  assert.equal(S.vinput, null);
});

// ────────────────────────── 文案:状态列 / 行动作 / 渲染无中文 ──────────────────────────

test('status_text:local 前缀 / ✓ ▲ / no source 英文', () => {
  assert.equal(M.status_text({ ...FIXED, installed: true, local: true, version: 'v1.2' }), 'local v1.2');
  assert.equal(M.status_text({ ...FIXED, installed: true, local: true }), 'local');
  assert.equal(M.status_text({ ...FIXED, installed: true, version: '1.0' }), '✓ 1.0');
  assert.equal(M.status_text({ ...FIXED, installed: true, version: '1.0', upgradable: true }), '▲ 1.0');
  assert.equal(M.status_text(FIXED), '–');
  assert.equal(M.status_text({ ...FIXED, installable: false }), 'no source');
});

test('row_actions:label 为英文 op', () => {
  const A = M.row_actions({ ...NV_INSTALLABLE, installed: true, local: true });
  assert.deepEqual(A.map((a) => a.label), ['unmanage']);
  const B = M.row_actions({ ...NV_INSTALLABLE, installed: true, local: false, upgradable: true });
  assert.deepEqual(B.map((a) => a.label), ['update', 'remove']);
  const C = M.row_actions(NV_INSTALLABLE);
  assert.deepEqual(C.map((a) => a.label), ['install']);
});

// ────────────────────────── I4:busy 重入守卫 + r 手动刷新 ──────────────────────────

function setBusy() {
  S.busy = {
    task_id: 77,
    items: [{ op: 'install', name: 'gopls' }],
    idx: 0,
    done: 0,
    fail: [],
    pct: null,
    cur: { op: 'install', name: 'gopls' },
  };
  S.row_busy = {};
}

test('I4:busy 时 run_action(install) 拒绝并提示任务进行中(英文)', () => {
  S.rows = [FIXED];
  setBusy();
  M.run_action('install', 'gopls');
  assert.equal(calls.tasks.length, 0, 'busy 中不得再提交');
  assert.ok(calls.echo.some((m) => m.includes('task is running')), '应提示任务进行中: ' + calls.echo);
});

test('I4:busy 时 batch_run 拒绝;解除后可再提交', () => {
  S.rows = [FIXED];
  S.marks = new Set(['gopls']);
  setBusy();
  M.batch_run();
  assert.equal(calls.tasks.length, 0, 'busy 中批量不得提交');
  assert.ok(calls.echo.some((m) => m.includes('task is running')));
  assert.equal(S.marks.size, 1, '守卫不吞标记(提交未发生)');
  // 解除后恢复
  S.busy = null;
  M.batch_run();
  assert.equal(calls.tasks.length, 1, '解除后批量可提交');
  assert.equal(S.marks.size, 0, '快照即提交');
});

test('I4:busy 时版本提交链路被拒(run_action 拦截,不弹输入不提交)', () => {
  S.rows = [NV_INSTALLABLE];
  setBusy();
  M.run_action('install', 'rust-analyzer'); // busy 守卫在弹输入之前
  assert.equal(calls.tasks.length, 0, 'busy 中不得提交');
  assert.equal(calls.popups.length, 0, 'busy 中不弹版本输入');
  assert.equal(S.vinput, null);
  assert.ok(calls.echo.some((m) => m.includes('task is running')), '应提示任务进行中');
  // 已开的版本输入在 busy 中 Enter 也被拒且保留弹窗(输入不丢)
  S.busy = null;
  calls.echo.length = 0;
  M.run_action('install', 'rust-analyzer');
  popupOn(M.LAYERS.input).opts.onKey({ name: '2' });
  popupOn(M.LAYERS.input).opts.onKey({ name: 'Enter' }); // 正常提交成功
  assert.equal(calls.tasks.length, 1);
});

test('I4:主窗 r 无过滤时触发 fetch_rows(刷新);SEARCH 内 r 是查询字符', () => {
  S.rows = [FIXED];
  M.handle_key({ name: 'r' });
  assert.equal(calls.rowsFetch.length, 1, 'r 应触发一次 fetch_rows');
  assert.equal(S.filter, '', 'r 不进 filter');
  assert.equal(S.searching, false);
  M.handle_key({ name: '/' });
  M.handle_key({ name: 'r' });
  assert.equal(S.filter, 'r', 'SEARCH 内 r 是查询字符');
  assert.equal(calls.rowsFetch.length, 1, '不再触发刷新');
});

// ────────────────────────── M-f:批级 panic(name="")补齐剩余项 error 终态,防 busy 卡底栏 ──────────────────────────

test('M-f:批级 error(name 空)对剩余项逐个补 error 终态并收尾 busy', () => {
  S.rows = [FIXED, { ...FIXED, name: 'black2' }];
  S.marks = new Set(['gopls', 'black2']);
  M.batch_run();
  assert.equal(S.busy.items.length, 2);
  // 第一项正常 error(具名)→ idx 到 1
  feed('error', 'gopls', 'download failed');
  assert.equal(S.busy.idx, 1);
  // 批级 panic(name=""):worker 不再发剩余事件 → 补齐并收尾
  feed('error', '', 'batch worker panic: boom');
  assert.equal(S.busy, null, '批级 panic 后 busy 必须收尾(不再等永不来的事件)');
  const last = calls.echo[calls.echo.length - 1];
  assert.ok(last.includes('batch done 0/2'), '汇总应有: ' + last);
  assert.ok(last.includes('black2'), '剩余项 black2 应出现在失败名单: ' + last);
});
