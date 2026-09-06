// arsenal node 级测试——版本输入/提交链路 + update 重询(e) + 批量 + 汇总 echo 边界(g)
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

// ────────────────────────── I2:过滤非空时字母一律即搜(防误卸载/误弹窗/误关窗) ──────────────────────────

test('I2:过滤非空时 x 进 filter 不触发 remove(不卸载)', () => {
  S.rows = [{ ...NV_MANAGED }]; // installed 非 local:x 本会 remove
  S.filter = 'a';
  M.handle_key({ name: 'x' });
  assert.equal(S.filter, 'ax', 'x 应追加进过滤');
  assert.equal(calls.tasks.length, 0, '不得触发 remove');
  assert.deepEqual(calls.popups, [], '不得开任何弹窗');
});

test('I2:过滤非空时 u/i 进 filter 不弹版本输入/信息', () => {
  S.rows = [{ ...NV_MANAGED }]; // u 本会开版本输入(needs_version update),i 本会开信息
  S.filter = 'n';
  M.handle_key({ name: 'u' });
  M.handle_key({ name: 'i' });
  assert.equal(S.filter, 'nui', 'u/i 均应进过滤');
  assert.deepEqual(calls.popups, [], '不得弹窗');
  assert.equal(S.vinput, null);
});

test('I2:过滤非空时 f/t/j/k/r/q/C 均不触发快捷键', () => {
  S.rows = [{ ...NV_MANAGED, upgradable: true }];
  S.vinputs['rust-analyzer'] = '2024-09-16';
  S.kind = 'all';
  S.sel = 0;
  const before_rows = S.rows.length;
  S.filter = 'q';
  for (const k of ['f', 't', 'j', 'k', 'r', 'q']) M.handle_key({ name: k });
  // C(shift)也进搜索
  M.handle_key({ name: 'C', shift: true });
  assert.equal(S.filter, 'qftjkrqC', '全字母(含 q/f/t/j/k/r/C)应追加,got: ' + S.filter);
  assert.equal(S.kind, 'all', 'f 不得循环分类');
  assert.equal(S.marks.size, 0, 't 不得标记');
  assert.equal(S.sel, 0, 'j/k 不得移动选中');
  assert.ok('rust-analyzer' in S.vinputs, 'C 不得清记忆');
  assert.equal(calls.rowsFetch.length, 0, 'r 不得触发刷新');
  assert.equal(S.rows.length, before_rows);
});

test('I2:过滤非空时 q 不关闭(return handled);Esc 清过滤;空过滤 q 才 close', () => {
  S.filter = 'd';
  assert.equal(M.handle_key({ name: 'q' }), 'handled', '过滤中 q 是搜索字符');
  assert.equal(S.filter, 'dq');
  // Esc 清过滤
  assert.equal(M.handle_key({ name: 'Esc' }), 'handled');
  assert.equal(S.filter, '', 'Esc 先清过滤');
  // 空过滤:q close,Esc close
  assert.equal(M.handle_key({ name: 'q' }), 'close');
  assert.equal(M.handle_key({ name: 'Esc' }), 'close');
});

test('I2:过滤非空时 ↑↓ 仍可移动(导航例外)', () => {
  S.rows = [FIXED, { ...FIXED, name: 'gopls2' }];
  S.filter = 'go';
  M.handle_key({ name: 'Down' });
  assert.equal(S.sel, 1, 'Down 在过滤中仍导航');
  assert.equal(S.filter, 'go', 'filter 不变');
  M.handle_key({ name: 'Up' });
  assert.equal(S.sel, 0);
});

test('I2:过滤空时直达快捷键仍生效(x 卸载/u 版本输入/i 信息/f 分类/t 标记/q 关)', () => {
  S.rows = [{ ...NV_MANAGED }];
  M.handle_key({ name: 'x' }); // installed 非 local → remove
  assert.equal(calls.tasks.length, 1, 'x 直发 remove');
  assert.deepEqual(calls.tasks[0].items, [{ op: 'remove', name: 'rust-analyzer' }]);
  reset();
  S.rows = [{ ...NV_MANAGED }];
  M.handle_key({ name: 'u' });
  popupOn(M.LAYERS.input); // update needs_version 恒弹输入
  reset();
  S.rows = [FIXED];
  M.handle_key({ name: 'i' });
  popupOn(M.LAYERS.info);
  reset();
  S.rows = [FIXED];
  S.kind = 'all';
  M.handle_key({ name: 'f' });
  assert.equal(S.kind, 'lsp', 'f 循环分类');
  reset();
  S.rows = [FIXED];
  M.handle_key({ name: 't' });
  assert.deepEqual([...S.marks], ['gopls']);
  reset();
  assert.equal(M.handle_key({ name: 'q' }), 'close', '空过滤 q 关闭');
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

test('I4:busy 时 run_action(install) 拒绝并提示任务进行中', () => {
  S.rows = [FIXED];
  setBusy();
  M.run_action('install', 'gopls');
  assert.equal(calls.tasks.length, 0, 'busy 中不得再提交');
  assert.ok(calls.echo.some((m) => m.includes('任务进行中')), '应提示任务进行中');
});

test('I4:busy 时 batch_run 拒绝;解除后可再提交', () => {
  S.rows = [FIXED];
  S.marks = new Set(['gopls']);
  setBusy();
  M.batch_run();
  assert.equal(calls.tasks.length, 0, 'busy 中批量不得提交');
  assert.ok(calls.echo.some((m) => m.includes('任务进行中')));
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
  assert.ok(calls.echo.some((m) => m.includes('任务进行中')), '应提示任务进行中');
  // 已开的版本输入在 busy 中 Enter 也被拒且保留弹窗(输入不丢)
  S.busy = null;
  calls.echo.length = 0;
  M.run_action('install', 'rust-analyzer');
  popupOn(M.LAYERS.input).opts.onKey({ name: '2' });
  popupOn(M.LAYERS.input).opts.onKey({ name: 'Enter' }); // 正常提交成功
  assert.equal(calls.tasks.length, 1);
});

test('I4:主窗 r 无过滤时触发 fetch_rows(刷新);过滤时 r 进搜索', () => {
  S.rows = [FIXED];
  M.handle_key({ name: 'r' });
  assert.equal(calls.rowsFetch.length, 1, 'r 应触发一次 fetch_rows');
  assert.equal(S.filter, '', 'r 不进 filter');
  S.filter = 'g';
  M.handle_key({ name: 'r' });
  assert.equal(S.filter, 'gr', '过滤中 r 是搜索字符');
  assert.equal(calls.rowsFetch.length, 1, '不再触发刷新');
});

// ────────────────────────── M-f:批级 panic(name="")补齐剩余项 error 终态,防 busy 卡底栏 ──────────────────────────

test('M-f:批级 error(name 空)对剩余项逐个补 error 终态并收尾 busy', () => {
  S.rows = [FIXED, { ...FIXED, name: 'black2' }];
  S.marks = new Set(['gopls', 'black2']);
  M.batch_run();
  assert.equal(S.busy.items.length, 2);
  // 第一项正常 error(具名)→ idx 到 1
  feed('error', 'gopls', '下载失败');
  assert.equal(S.busy.idx, 1);
  // 批级 panic(name=""):worker 不再发剩余事件 → 补齐并收尾
  feed('error', '', 'batch worker panic: boom');
  assert.equal(S.busy, null, '批级 panic 后 busy 必须收尾(不再等永不来的事件)');
  const last = calls.echo[calls.echo.length - 1];
  assert.ok(last.includes('批量完成 0/2'), '汇总应有: ' + last);
  assert.ok(last.includes('black2'), '剩余项 black2 应出现在失败名单: ' + last);
});
