// arsenal node 级测试——版本输入/提交链路 + update 重询(e) + 批量 + 汇总 echo 边界(g)
// 运行:node --test plugins/features/arsenal/tests/(node ≥ 18,内置 node:test,零依赖)
// 原理:index.js 底部模块化导出(S + run_action/batch_run/handle_key 等,engine 加载时
//       module 未定义自动跳过)。node 直接 require 前注入最小 global.helix 桩:
//       plugin/register_command 空转(engine 接线不需要);echo/open_popup/server.task
//       捕获调用;open_popup 存下 opts → 测试可驱动版本输入/菜单弹窗的 onKey/onClose。
//       render/el 不参与动作路径(open_popup 桩不调 render),无需真实实现。
'use strict';

const { test, beforeEach } = require('node:test');
const assert = require('node:assert/strict');

// ── helix 最小桩(须在 require 之前就位:index.js 顶层调用 helix.plugin) ──
const calls = { echo: [], popups: [], tasks: [] };
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
    rows() {}, // fetch_rows 桩:测试直接写 S.rows,不回写
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
  description: 'Rust 语言服务器',
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
  description: 'Go 语言服务器',
  homepage: 'https://github.com/golang/tools',
};

function reset() {
  S.popup = null;
  S.rows = [];
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

// ── e:update 重询(记忆存在也恒弹版本输入,绝不直发旧记忆) ──
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
  // 小写 c 是搜索字符,不清记忆
  S.vinputs['rust-analyzer'] = '1.0';
  M.handle_key({ name: 'c' });
  assert.equal(S.vinputs['rust-analyzer'], '1.0');
  assert.equal(S.filter, 'c');
});

// ── f:批量提交(marks 快照)version 正确携带 ──
test('batch_run:needs_version 用记忆、固定配方不带 version', () => {
  const B = { ...FIXED, name: 'black', kind: 'formatter', description: 'Python 格式化器' };
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
  feed('error', 'gopls', '下载源未配置(registry 缺 url)');
  assert.deepEqual(calls.echo, ['arsenal: 批量完成 0/1, 失败: gopls(下载源未配置(registry 缺 url))']);
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
    'arsenal: 批量完成 2/2',
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
