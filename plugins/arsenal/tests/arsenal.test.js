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
// 哪些 popup id 仍存活(popup_exists 桩的依据)
const alivePopups = new Set();
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
    alivePopups.add(id);
    return id;
  },
  // 核心弹层（动作菜单现在走 helix.dialog.select）
  dialog: {
    select(opts) {
      const id = popup_seq++;
      calls.dialog = opts;
      calls.dialog_id = id;
      return id;
    },
    panel(opts) {
      const id = popup_seq++;
      calls.panel = opts;
      calls.panel_id = id;
      alivePopups.add(id);
      return id;
    },
  },
  // popup_exists:默认按 alivePopups 判定(测试可模拟"层被外部关闭")
  popup_exists(id) {
    return alivePopups.has(id);
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

const M = require('../plugin.js');
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

// ────────────────────────── 主窗已交给核心弹层（`dialog.panel`）──────────────────────────
// 迁移前这里测的是"自维护的 SEARCH 态"；现在搜索/导航/Enter/Esc 归核心，
// 本插件只负责：数据（items(query)）· 行/头/脚注画法 · 动作键（on_key）。

test('panel_spec:交给核心弹层的规格（title/layer/wrap/search/items/render/…）', () => {
  S.rows = [{ ...NV_MANAGED, name: 'rust-analyzer', kind: 'lsp' }];
  const spec = M.panel_spec();
  assert.equal(spec.title, 'arsenal');
  assert.equal(spec.layer, M.LAYERS.main, '主窗与菜单分层（菜单关掉后主窗还在）');
  assert.equal(spec.wrap, true);
  assert.equal(spec.search, true, '搜索由核心负责');
  // items(query)：查询过滤在插件侧，但**由核心按当前查询调用**
  assert.deepEqual(spec.items('').map((i) => i.value), ['rust-analyzer']);
  assert.deepEqual(spec.items('zzz'), []);
  assert.ok(Array.isArray(spec.render({ label: 'rust-analyzer', cursor: true, width: 80 })));
  assert.ok(Array.isArray(spec.header({ width: 80 })));
  assert.ok(String(spec.footer({ width: 80 })).length > 0);
});

test('panel_spec.on_key:把核心给的游标行同步到 S.sel 后走动作键', () => {
  S.rows = [
    { ...NV_MANAGED, name: 'a-server', kind: 'lsp', upgradable: true },
    { ...NV_MANAGED, name: 'b-server', kind: 'lsp', upgradable: true },
  ];
  const spec = M.panel_spec();
  // t = 标记当前行；核心把"按在哪一行"通过 ctx 传进来
  spec.on_key('t', { index: 1, value: 'b-server', label: 'b-server', query: '', searching: false });
  assert.equal(S.sel, 1, '游标同步');
  assert.ok(S.marks.has('b-server'), '标记的是核心游标那一行');
  assert.ok(!S.marks.has('a-server'));
  // i = 开信息弹窗（针对该行）
  spec.on_key('i', { index: 0, value: 'a-server' });
  popupOn(M.LAYERS.info);
});

test('panel_spec.on_pick:Enter 的行为（有标记→批量；否则开动作菜单）', () => {
  // 可批量行：受管已装且不需要显式版本（needs_version 的行走单行安装）
  S.rows = [{ ...NV_MANAGED, name: 'x', kind: 'lsp', upgradable: true, needs_version: false }];
  const spec = M.panel_spec();
  S.marks = new Set(['x']);
  spec.on_pick(['x']);
  assert.equal(calls.tasks.length, 1, '有标记 ⇒ 批量提交');
  S.marks = new Set();
  calls.dialog = null;
  spec.on_pick(['x']);
  assert.ok(calls.dialog, '无标记 ⇒ 打开动作菜单（核心列表）');
});

test('Tab 循环页签 all→lsp→…→local→all;Shift+Tab 反向', () => {
  const order = [...M.KINDS];
  for (let i = 1; i < order.length; i++) {
    M.handle_action_key({ name: 'Tab' });
    assert.equal(S.kind, order[i]);
  }
  M.handle_action_key({ name: 'Tab' });
  assert.equal(S.kind, 'all', '循环回 all');
  M.handle_action_key({ name: 'Tab', shift: true });
  assert.equal(S.kind, 'local', 'Shift+Tab 反向');
  // NORMAL 直接字母不再循环(f 移除)
  assert.equal(S.kind, 'local');
  M.handle_action_key({ name: 'f' });
  assert.equal(S.kind, 'local', 'f 键已移除,不影响页签');
});

test('页签过滤生效:kind 过滤 rows', () => {
  S.rows = [
    { ...FIXED, name: 'ra', kind: 'lsp' },
    { ...FIXED, name: 'dbg', kind: 'dap' },
    { ...FIXED, name: 'blk', kind: 'formatter' },
  ];
  M.handle_action_key({ name: 'Tab' }); // lsp
  assert.deepEqual(M.visible().map((r) => r.name), ['ra']);
  M.handle_action_key({ name: 'Tab' }); // dap
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
  M.handle_action_key({ name: 'C', shift: true });
  assert.ok(!('rust-analyzer' in S.vinputs));
  assert.equal(S.vinputs.gopls, 'v0.14', '只清选中行');
  // 选中无记忆行(gopls)→ 清全部
  S.sel = 1;
  M.handle_action_key({ name: 'C', shift: true });
  assert.deepEqual(S.vinputs, {});
  // 小写 c 不是动作键：什么都清不掉（搜索/输入归核心，轮不到插件）
  S.vinputs['rust-analyzer'] = '1.0';
  M.handle_action_key({ name: 'c' });
  assert.equal(S.vinputs['rust-analyzer'], '1.0');
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

// ── f5:open_arsenal 自愈(S.popup 可能指向已被外部关闭的层)──
test('open_arsenal:句柄失效(层被外部关掉)→ 自愈并重新打开', () => {
  S.rows = [FIXED];
  S.popup = 4242; // 一个早已不存在的层
  alivePopups.clear();
  calls.panel = null;
  calls.echo.length = 0;
  M.open_arsenal();
  assert.ok(calls.panel, '应当重新打开（走 helix.dialog.panel），而不是回 already open');
  assert.ok(!calls.echo.some((m) => m.includes('already open')));
  assert.notEqual(S.popup, 4242, '句柄应更新为新层');
});

test('open_arsenal:句柄仍有效 → 不重复开层,提示 already open', () => {
  S.rows = [FIXED];
  S.popup = 777;
  alivePopups.add(777);
  const before = calls.popups.length;
  M.open_arsenal();
  assert.equal(calls.popups.length, before, '不得重复开层');
  assert.ok(calls.echo.some((m) => m.includes('already open')), JSON.stringify(calls.echo));
});

// ── f4:搜索覆盖命令名 / 首帧文案 / 批中不刷新(2026-10)──
test('visible:查询匹配 bin(命令名),不只 name/描述', () => {
  S.rows = [
    { ...FIXED, name: 'pyright', bin: 'pyright-langserver', languages: ['python'], description: 'type checker' },
    { ...FIXED, name: 'gopls', bin: 'gopls', languages: ['go'], description: 'go lang server' },
  ];
  S.kind = 'all';
  // 查询串现在由核心弹层传给 `visible(query)`
  assert.deepEqual(M.visible('langserver').map((r) => r.name), ['pyright'], '按 bin 命中');
});

test('empty_list_text:未回传过数据时是 loading,回传后才说没有服务器', () => {
  S.loaded = false;
  assert.ok(M.empty_list_text().includes('loading servers'), '首帧应是 loading');
  S.loaded = true;
  assert.ok(M.empty_list_text().includes('no servers yet'), '加载完才算"没有服务器"');
  assert.ok(M.empty_list_text('zzz').includes("no matches for 'zzz'"), '搜索无匹配优先');
});

test('批量:每个 done/error 不再各自全量 fetch,只在批次结束刷新一次', () => {
  S.rows = [FIXED, { ...FIXED, name: 'b' }];
  S.marks = new Set(['gopls', 'b']);
  const before = calls.rowsFetch.length;
  M.batch_run();
  feed('done', 'gopls', "installed 'gopls'");
  assert.equal(calls.rowsFetch.length, before, '批中不刷新');
  feed('done', 'b', "installed 'b'");
  assert.equal(calls.rowsFetch.length, before + 1, '批次结束刷新一次');
});

// ── f3:x 卸载确认(2026-10:原实现单键即卸,手滑就丢受管安装)──
test('x:受管已装行 → 先弹确认,不立即提交任务', () => {
  S.rows = [{ ...FIXED, name: 'gopls', installed: true, local: false, version: 'v1' }];
  M.handle_action_key({ name: 'x' });
  assert.equal(calls.tasks.length, 0, '确认前不得提交');
  const last = calls.popups.at(-1);
  assert.equal(last.opts.layer, 'arsenal-confirm', JSON.stringify(last.opts.layer));
  // Enter 确认 → 提交 remove
  last.opts.onKey({ name: 'Enter' });
  assert.deepEqual(calls.tasks[0].items, [{ op: 'remove', name: 'gopls' }]);
});

test('x:确认弹层 Esc/n 取消 → 不提交', () => {
  S.rows = [{ ...FIXED, name: 'gopls', installed: true, local: false }];
  M.handle_action_key({ name: 'x' });
  const last = calls.popups.at(-1);
  last.opts.onKey({ name: 'Esc' });
  assert.equal(calls.tasks.length, 0, 'Esc 取消不得提交');
  M.handle_action_key({ name: 'x' });
  calls.popups.at(-1).opts.onKey({ name: 'n' });
  assert.equal(calls.tasks.length, 0, 'n 取消不得提交');
});

test('x:local 行确认后走 unmanage(不是 remove)', () => {
  S.rows = [{ ...FIXED, name: 'local-tool', installed: true, local: true }];
  M.handle_action_key({ name: 'x' });
  calls.popups.at(-1).opts.onKey({ name: 'y' });
  assert.deepEqual(calls.tasks[0].items, [{ op: 'unmanage', name: 'local-tool' }]);
});

// ── f2:batch 规划(2026-10 修三处 bug:看不见的标记 / 只装不升 / 版本缺失逐项失败)──
test('plan_batch:隐藏标记(当前页签/搜索看不见)不提交', () => {
  S.rows = [
    { ...FIXED, name: 'a', kind: 'lsp' },
    { ...FIXED, name: 'b', kind: 'dap' },
  ];
  const r = M.plan_batch(S.rows, new Set(['a', 'b']), new Set(['a']), {});
  assert.deepEqual(r.items, [{ op: 'install', name: 'a' }]);
  assert.deepEqual(r.skipped.hidden, ['b']);
});

test('plan_batch:受管已装行 → update(不再"只装未装")', () => {
  S.rows = [
    { ...FIXED, name: 'managed', installed: true, local: false, version: 'v1' },
  ];
  const r = M.plan_batch(S.rows, new Set(['managed']), null, {});
  assert.deepEqual(r.items, [{ op: 'update', name: 'managed' }]);
});

test('plan_batch:local 行 / 无源行 / needs_version 无记忆 → 各自进 skipped', () => {
  S.rows = [
    { ...FIXED, name: 'local', installed: true, local: true },
    { ...FIXED, name: 'nosrc', installable: false },
    { ...NV_INSTALLABLE, name: 'nv' },
  ];
  const r = M.plan_batch(S.rows, new Set(['local', 'nosrc', 'nv']), null, {});
  assert.deepEqual(r.items, []);
  assert.deepEqual(r.skipped.local, ['local']);
  assert.deepEqual(r.skipped.noSource, ['nosrc']);
  assert.deepEqual(r.skipped.needsVersion, ['nv']);
});

test('plan_batch:needs_version 有记忆 → install 带 version', () => {
  S.rows = [{ ...NV_INSTALLABLE, name: 'nv' }];
  const r = M.plan_batch(S.rows, new Set(['nv']), null, { nv: '3.1' });
  assert.deepEqual(r.items, [{ op: 'install', name: 'nv', version: '3.1' }]);
});

test('batch_run:只清已提交的标记,跳过的保留并 echo 原因', () => {
  S.rows = [
    { ...FIXED, name: 'ok' },
    { ...NV_INSTALLABLE, name: 'nv' },
  ];
  S.marks = new Set(['ok', 'nv']);
  M.batch_run();
  assert.deepEqual(calls.tasks[0].items, [{ op: 'install', name: 'ok' }]);
  assert.deepEqual([...S.marks], ['nv'], '跳过的保留标记,便于逐行处理');
  assert.ok(
    calls.echo.some((m) => m.includes('need an explicit version')),
    '跳过的原因必须说出来:' + JSON.stringify(calls.echo)
  );
});

test('batch_run:没有标记 → 明确提示,不空提交', () => {
  S.rows = [FIXED];
  S.marks = new Set();
  M.batch_run();
  assert.equal(calls.tasks.length, 0, '不得提交空批次');
  assert.ok(calls.echo.some((m) => m.includes('no rows marked')), JSON.stringify(calls.echo));
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
  // 失败摘要改成一行 `name — <首行>`（核心错误可能多行，旧断言是括号包裹）
  assert.deepEqual(calls.echo, [
    'arsenal: batch done 0/1, failed: gopls — source not configured (registry missing url)',
  ]);
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
test('菜单:两动作行 Enter 开核心列表;render 画富行;菜单内 i 直达信息(不关层)', () => {
  const R = { ...NV_MANAGED, upgradable: true }; // update/remove 两动作
  S.rows = [R];
  M.panel_spec().on_pick(['rust-analyzer']);
  const sel = calls.dialog;
  assert.ok(sel, '应调用 helix.dialog.select');
  assert.equal(sel.title, 'rust-analyzer · actions');
  assert.equal(sel.wrap, true, '菜单保持回绕手感（迁移前就是 % 长度）');
  assert.deepEqual(sel.items, [
    { label: 'update', value: '0' },
    { label: 'remove', value: '1' },
  ]);
  // 富行：游标行带 ▸ 与动作名，非游标行不带 ▸
  const cursorRow = sel.render({ label: 'update', cursor: true, picked: false });
  assert.equal(cursorRow[0].text, '▸ ');
  assert.ok(cursorRow[1].text.startsWith('update'), JSON.stringify(cursorRow));
  const plainRow = sel.render({ label: 'remove', cursor: false, picked: false });
  assert.equal(plainRow[0].text, '  ');
  // i 看信息：不关层；q 关层
  assert.equal(sel.on_key('i'), undefined, 'i 不关层');
  const info = popupOn(M.LAYERS.info);
  assert.ok(info, '信息叠于菜单之上');
  assert.equal(sel.on_key('q'), 'close', 'q 关层');
  // 选中后执行对应动作
  sel.on_pick(['1']);
});

// 占位配方(未装 + 无下载源)Enter:提示原因并回退打开信息弹窗(不再只 echo)
test('无动作行 Enter:echo 原因并打开信息弹窗', () => {
  const PLACEHOLDER = { ...FIXED, name: 'jdtls', installable: false, source: null };
  S.rows = [PLACEHOLDER];
  M.panel_spec().on_pick(['jdtls']);
  popupOn(M.LAYERS.info); // 回退到信息弹窗(解释/引导)
  assert.equal(S.menu, null, '不建空菜单');
  assert.ok(
    calls.echo.some((m) => m.includes('no download source')),
    'echo 应说明无下载源: ' + calls.echo
  );
});

test('子弹窗 onClose 自愈:引擎非按键关闭时 S.vinput 置 null', () => {
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

test('I4:主窗 r 触发 fetch_rows(刷新)；搜索字符不再经过插件（归核心）', () => {
  S.rows = [FIXED];
  M.handle_action_key({ name: 'r' });
  assert.equal(calls.rowsFetch.length, 1, 'r 应触发一次 fetch_rows');
  // 搜索态里核心不会把字符转发给 on_key，所以这里只需确认"插件没有搜索状态"
  assert.equal('searching' in M.S, false, '搜索状态已归属核心');
  assert.equal('filter' in M.S, false, '查询串已归属核心');
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
