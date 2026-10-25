// dapui node 级测试 —— 变量树懒加载 / 底部 REPL·Output 双 tab / 状态栏段 / 键位穿透。
// 运行:node --test plugins/dapui/tests/dapui.test.js
// 原理:插件顶层直接用 `helix.*`，所以 require 之前注入最小桩；桩里的 `helix.dap.*`
//      返回立刻 resolve 的 Promise（异步链路用 `await tick()` 等一拍）。
'use strict';

const { test, beforeEach } = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');

const PLUGIN = path.join(__dirname, '..', 'plugin.js');
const tick = () => new Promise((r) => setTimeout(r, 0));

function make_host(opts) {
  const o = opts || {};
  const calls = {
    variables: [],
    eval: [],
    panels: [],
    closed: [],
    draws: [],
    statusline: null,
    commands: {},
    handlers: {},
    picker: null,
    exported: null,
    highlights: [],
    vt: [],
    theme: [],
  };
  const host = {
    plugin() {},
    define_config() {},
    get_config() {
      return o.cfg || {};
    },
    on(name, fn) {
      calls.handlers[name] = fn;
    },
    register_command(name, fn) {
      calls.commands[name] = fn;
    },
    register_statusline_segment(name, fn) {
      calls.statusline = { name, fn };
    },
    export(obj) {
      calls.exported = obj;
    },
    echo(msg) {
      calls.echo = msg;
    },
    open_panel(opts) {
      calls.panels.push(opts);
      return calls.panels.length; // 1-based 面板 id
    },
    close_panel(id) {
      calls.closed.push(id);
    },
    viewport: () => [80, 24],
    text: {
      truncate: (s, w) => String(s).slice(0, w),
      pad: (s, w) => String(s).padEnd(w),
    },
    draw: {
      text: (x, y, t, o2) => calls.draws.push({ x, y, t, style: o2 && o2.style }),
      hline: () => {},
      done: () => 'DONE',
    },
    picker: {
      define: (n, o2) => (calls.picker = { n, o: o2 }),
      run: () => {},
    },
    pane: { focus: () => {} },
    open_file: () => {},
    set_virtual_text: (...args) => calls.vt.push({ args }),
    set_highlight: (path, sr, sc, er, ec, style) =>
      calls.highlights.push({ path, sr, sc, er, ec, style }),
    set_theme: (obj) => calls.theme.push(obj),
    get_style: (scope) => (o.styles && o.styles[scope]) || null,
    dap: {
      sessions: () => (o.sessions ? o.sessions() : []),
      stack_frames: (thread) => Promise.resolve(o.frames ? o.frames(thread) : []),
      scopes: (frame) => Promise.resolve(o.scopes ? o.scopes(frame) : []),
      variables: (ref) => {
        calls.variables.push(ref);
        const v = o.variables ? o.variables(ref) : [];
        return v instanceof Error ? Promise.reject(v) : Promise.resolve(v);
      },
      eval: (expr) => {
        calls.eval.push(expr);
        const v = o.eval ? o.eval(expr) : { result: '1', type: 'i32' };
        return v instanceof Error ? Promise.reject(v) : Promise.resolve(v);
      },
      list_breakpoints: () => o.breakpoints || [],
    },
  };
  return { host, calls };
}

function load(opts) {
  const { host, calls } = make_host(opts);
  global.helix = host;
  delete require.cache[require.resolve(PLUGIN)];
  require(PLUGIN);
  return { calls, api: calls.exported };
}

const stopped = {
  id: 1,
  name: 'lldb-dap',
  state: 'stopped',
  active: true,
  thread: 1,
  frame: { id: 11, name: 'main', line: 41, column: 0 },
};

beforeEach(() => {
  delete global.helix;
});

// ── 变量树 ──

test('命中停止 ⇒ 自动开右侧面板 + 树里出现栈帧/作用域', async () => {
  const { calls, api } = load({
    cfg: { auto_open: true, max_depth: 0 },
    sessions: () => [stopped],
    frames: () => [{ id: 11, name: 'main', source: '/tmp/main.rs', line: 41, column: 0 }],
    scopes: () => [{ name: 'Local', variablesReference: 100, expensive: false }],
    variables: () => [{ name: 'x', value: '42', variablesReference: 0 }],
  });
  calls.handlers['dap-stopped']();
  await tick();
  await tick();
  assert.equal(calls.panels.length, 1, 'auto_open ⇒ 开一次面板');
  assert.equal(calls.panels[0].side, 'right', '变量树在右侧');
  const labels = api.T.nodes.map((n) => n.label);
  assert.ok(labels.some((l) => l.includes('main')), `应有栈帧: ${JSON.stringify(labels)}`);
  assert.ok(labels.some((l) => l.includes('Local')), `应有作用域: ${JSON.stringify(labels)}`);
});

test('展开节点只拉一次 variables(再展开用缓存)', async () => {
  const { calls, api } = load({
    cfg: { auto_open: false, max_depth: 0 },
    sessions: () => [stopped],
    frames: () => [{ id: 11, name: 'main', source: '/tmp/main.rs', line: 0, column: 0 }],
    scopes: () => [{ name: 'Local', variablesReference: 100, expensive: false }],
    variables: () => [{ name: 'x', value: '1', variablesReference: 0 }],
  });
  calls.handlers['dap-stopped']();
  await tick();
  await tick();
  const scope = api.T.nodes.find((n) => n.label === 'Local');
  api.T.cursor = api.T.nodes.indexOf(scope);
  api.key_tree({ name: 'l' });
  await tick();
  assert.deepEqual(calls.variables, [100], '第一次展开拉一次');
  api.key_tree({ name: 'h' });
  const scope2 = api.T.nodes.find((n) => n.label === 'Local');
  api.T.cursor = api.T.nodes.indexOf(scope2);
  api.key_tree({ name: 'l' });
  await tick();
  assert.deepEqual(calls.variables, [100], '第二次展开命中缓存');
});

test('max_nodes 截断 + 截断提示', async () => {
  const many = Array.from({ length: 30 }, (_, i) => ({
    name: 'v' + i,
    value: String(i),
    variablesReference: 0,
  }));
  const { calls, api } = load({
    cfg: { auto_open: false, max_nodes: 10, max_depth: 0 },
    sessions: () => [stopped],
    frames: () => [{ id: 11, name: 'main', source: '/tmp/main.rs', line: 0, column: 0 }],
    scopes: () => [{ name: 'Local', variablesReference: 100, expensive: false }],
    variables: () => many,
  });
  calls.handlers['dap-stopped']();
  await tick();
  await tick();
  const scope = api.T.nodes.find((n) => n.label === 'Local');
  api.T.cursor = api.T.nodes.indexOf(scope);
  api.key_tree({ name: 'l' });
  await tick();
  assert.ok(api.T.nodes.length <= 11, `节点应被压住: ${api.T.nodes.length}`);
  assert.ok(api.T.nodes.some((n) => n.label.includes('truncated')), '应有截断提示');
});

// ── 底部面板：REPL / Output ──

test('底部面板:两个 tab 手绘 + 切 tab', () => {
  const { calls, api } = load({ cfg: {}, sessions: () => [stopped] });
  api.open_bottom('repl');
  assert.equal(calls.panels[0].side, 'bottom', 'REPL/Output 在底部');
  calls.draws.length = 0;
  api.render_bottom(null, { width: 80, height: 12 });
  let texts = calls.draws.map((d) => d.t).join('|');
  assert.ok(texts.includes('▸REPL'), `tab 条应标出当前 tab: ${texts}`);
  assert.ok(texts.includes('Output'), `tab 条应有 Output: ${texts}`);
  // Tab 切换
  api.key_bottom({ name: 'Tab' });
  calls.draws.length = 0;
  api.render_bottom(null, { width: 80, height: 12 });
  texts = calls.draws.map((d) => d.t).join('|');
  assert.ok(texts.includes('▸Output'), `切到 Output: ${texts}`);
  // Esc 关面板
  api.key_bottom({ name: 'Esc' });
  assert.deepEqual(calls.closed, [1]);
});

test('REPL: 输入 → Enter 求值,结果进转录', async () => {
  const { calls, api } = load({ cfg: {}, sessions: () => [stopped] });
  api.open_bottom('repl');
  for (const ch of 'total') api.key_bottom({ name: ch });
  assert.equal(api.B.input, 'total');
  api.key_bottom({ name: 'Enter' });
  await tick();
  assert.deepEqual(calls.eval, ['total']);
  const last = api.B.transcript[api.B.transcript.length - 1];
  assert.match(last.text, /total => 1 \(i32\)/, `求值结果: ${last.text}`);
});

test('REPL: 求值失败 ⇒ 一行红字，不抛', async () => {
  const { calls, api } = load({
    cfg: {},
    sessions: () => [stopped],
    eval: () => new Error('no such symbol'),
  });
  api.open_bottom('repl');
  for (const ch of 'foo') api.key_bottom({ name: ch });
  api.key_bottom({ name: 'Enter' });
  await tick();
  const last = api.B.transcript[api.B.transcript.length - 1];
  assert.match(last.text, /!!/, `错误行: ${last.text}`);
  assert.match(last.text, /no such symbol/);
  assert.equal(last.style, 'error');
});

test('REPL: ↑↓ 翻历史', () => {
  const { api } = load({ cfg: {}, sessions: () => [stopped] });
  api.open_bottom('repl');
  api.B.history = ['a', 'b'];
  api.key_bottom({ name: 'Up' });
  assert.equal(api.B.input, 'b', '最近的一条');
  api.key_bottom({ name: 'Up' });
  assert.equal(api.B.input, 'a');
  api.key_bottom({ name: 'Down' });
  assert.equal(api.B.input, 'b');
});

test('Output: dap-output 进环形缓冲,超 500 丢最旧,滚动会停跟随', () => {
  const { calls, api } = load({ cfg: {}, sessions: () => [stopped] });
  api.open_bottom('output');
  for (let i = 0; i < 520; i++) calls.handlers['dap-output']({ id: 1, output: 'line' + i });
  assert.equal(api.B.output.length, 500, '环形上限');
  assert.equal(api.B.output[0].text, 'line20', '丢最旧 20 条');
  assert.equal(api.B.follow, true, '默认跟随尾部');
  api.key_bottom({ name: 'k' }); // 往上翻
  assert.equal(api.B.follow, false, '翻看后停跟随');
  api.key_bottom({ name: 'G' });
  assert.equal(api.B.follow, true, 'G 回尾部并恢复跟随');
});

test('Output: stderr 用 error 样式', () => {
  const { calls, api } = load({ cfg: {}, sessions: () => [stopped] });
  calls.handlers['dap-output']({ id: 1, category: 'stderr', output: 'boom\n' });
  assert.equal(api.B.output[0].text, 'boom', '去掉尾部换行');
  assert.equal(api.B.output[0].style, 'error');
});

// ── 状态栏段 / 键位 / 断点 ──

test('状态栏段:无会话 ⇒ null;停止 ⇒ #id ⏸ t<thread>', () => {
  const { calls, api } = load({ cfg: {}, sessions: () => [] });
  assert.equal(calls.statusline.name, 'dap');
  assert.equal(api.statusline_segment(), null);
  assert.equal(calls.statusline.fn(), null, '经核心取的段函数同一行为');
});

test('状态栏段:停止时带线程', () => {
  const { api } = load({ cfg: {}, sessions: () => [stopped] });
  const seg = api.statusline_segment();
  assert.equal(seg.text, '#1 ⏸ t1');
  assert.equal(seg.style, 'error');
});

test('修饰键穿透:ctrl 组合 ignore,C-\\ 回主区,未映射键穿透', () => {
  const { api } = load({ cfg: {}, sessions: () => [] });
  assert.equal(api.key_tree({ ctrl: true, name: 'y' }), 'ignore');
  assert.equal(api.key_tree({ ctrl: true, name: '\\' }), 'handled');
  assert.equal(api.key_bottom({ ctrl: true, name: 'y' }), 'ignore');
  assert.equal(api.key_tree({ name: 'z' }), 'ignore');
});

test('断点 Picker:行号显示 +1,带徽标', () => {
  const { calls } = load({
    cfg: {},
    sessions: () => [stopped],
    breakpoints: [{ path: '/tmp/a.rs', line: 3, verified: true, condition: 'x>1' }],
  });
  calls.commands['dapui-breakpoints']();
  const rows = calls.picker.o.items();
  assert.equal(rows[0].cells[0], '/tmp/a.rs:4');
  assert.match(rows[0].cells[1], /✓/);
  assert.match(rows[0].cells[1], /cond/);
});

test('命令齐了:dapui / dapui-repl / dapui-output / dapui-breakpoints', () => {
  const { calls } = load({ cfg: {}, sessions: () => [stopped] });
  for (const name of ['dapui', 'dapui-repl', 'dapui-output', 'dapui-breakpoints']) {
    assert.equal(typeof calls.commands[name], 'function', `命令 ${name} 应注册`);
  }
  calls.commands['dapui-repl']();
  assert.equal(calls.panels[0].side, 'bottom');
});

// ── 当前执行行高亮（规格里 "D 面" 的另一半） ──

test('命中停止 ⇒ 高亮当前执行行（默认开）', async () => {
  const { calls, api } = load({
    cfg: { auto_open: false, max_depth: 0 },
    sessions: () => [stopped],
    frames: () => [{ id: 11, name: 'main', source: '/tmp/main.rs', line: 41, column: 0 }],
    scopes: () => [],
  });
  calls.handlers['dap-stopped']();
  await tick();
  await tick();
  const hl = calls.highlights || [];
  assert.equal(hl.length, 1, `应有一条高亮: ${JSON.stringify(hl)}`);
  assert.equal(hl[0].path, '/tmp/main.rs');
  assert.equal(hl[0].sr, 41, '高亮从执行行开始');
  assert.equal(hl[0].er, 42, '整行（行尾独占）');
  assert.equal(hl[0].sc, 0);
  assert.equal(hl[0].ec, 0);
  assert.ok(hl[0].style, `要有 scope: ${hl[0].style}`);
});

test('highlight_current_line = false ⇒ 不写高亮', async () => {
  const { calls } = load({
    cfg: { auto_open: false, max_depth: 0, highlight_current_line: false },
    sessions: () => [stopped],
    frames: () => [{ id: 11, name: 'main', source: '/tmp/main.rs', line: 41, column: 0 }],
    scopes: () => [],
  });
  calls.handlers['dap-stopped']();
  await tick();
  await tick();
  assert.deepEqual(calls.highlights || [], [], '不该有高亮调用');
});

test('scope 解析：主题没定义 ⇒ 从 cursorline 底色派生并注册', async () => {
  const { calls } = load({
    cfg: { auto_open: false, max_depth: 0 },
    sessions: () => [stopped],
    frames: () => [{ id: 11, name: 'main', source: '/tmp/main.rs', line: 5, column: 0 }],
    scopes: () => [],
    styles: { 'ui.cursorline.primary': { bg: '#222222' } },
  });
  calls.handlers['dap-stopped']();
  await tick();
  await tick();
  assert.deepEqual(
    calls.theme,
    [{ 'dap.current-line': { bg: '#222222' } }],
    `应派生并注册 scope: ${JSON.stringify(calls.theme)}`,
  );
  assert.equal(calls.highlights[0].style, 'dap.current-line');
});

test('换主题 ⇒ 派生 scope 缓存失效并重新解析', async () => {
  const styles = { 'ui.cursorline.primary': { bg: '#111111' } };
  const { calls } = load({
    cfg: { auto_open: false, max_depth: 0 },
    sessions: () => [stopped],
    frames: () => [{ id: 11, name: 'main', source: '/tmp/main.rs', line: 1, column: 0 }],
    scopes: () => [],
    styles,
  });
  calls.handlers['dap-stopped']();
  await tick();
  await tick();
  assert.equal(calls.theme.length, 1);
  // 模拟换主题：底色变了 + 触发 theme-change
  styles['ui.cursorline.primary'] = { bg: '#333333' };
  calls.handlers['theme-change']();
  delete calls.theme; // 清空记录再看下一次
  calls.theme = [];
  calls.handlers['dap-stopped']();
  await tick();
  await tick();
  assert.deepEqual(
    calls.theme.pop(),
    { 'dap.current-line': { bg: '#333333' } },
    '换主题后应按新底色重算并覆盖注册',
  );
});

test('继续执行 ⇒ 清装饰（含高亮），不是只清内联值', async () => {
  const { calls, api } = load({
    cfg: { auto_open: false, max_depth: 0 },
    sessions: () => [stopped],
    frames: () => [{ id: 11, name: 'main', source: '/tmp/main.rs', line: 2, column: 0 }],
    scopes: () => [],
  });
  calls.handlers['dap-stopped']();
  await tick();
  await tick();
  const before = calls.vt.length;
  api.close_bottom && api.close_bottom();
  calls.handlers['dap-continued']();
  const last = calls.vt[calls.vt.length - 1];
  assert.ok(
    calls.vt.length > before,
    `继续执行应再发一次 Clear: ${JSON.stringify(calls.vt)}`,
  );
  assert.equal(last.args.length, 1, 'Clear = 只传 path（VT + highlight 一起清）');
  assert.equal(last.args[0], '/tmp/main.rs');
  assert.equal(api.T.ctx, null, '上下文清空');
});
