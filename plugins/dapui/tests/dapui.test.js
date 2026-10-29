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
    frames: [],
    scopes: [],
    opened: [],
    popup: null,
    setbp: [],
    rmbp: [],
    switched: [],
    excfilters: [],
    steps: [],
    ran: [],
    reqs: [],
    regs: [],
    layout: [],
    reads: [],
    writes: [],
    ran_async: [],
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
    // just-my-code 判"源码在不在工作区"要用；默认给个假工作区
    cwd: () => o.cwd || '/tmp/proj',
    text: {
      // 测试桩：显示宽度按字符数算（CJK 双宽的行为在真机上验，这里只测结构）
      width: (s) => String(s).length,
      truncate: (s, w) => String(s).slice(0, w),
      pad: (s, w) => String(s).padEnd(w),
    },
    draw: {
      text: (x, y, t, o2) => calls.draws.push({ x, y, t, style: o2 && o2.style }),
      rect: (x, y, w, h, o2) => calls.draws.push({ rect: [x, y, w, h], title: o2 && o2.title }),
      hline: () => {},
      vline: () => {},
      fill: () => {},
      push_clip: () => {},
      push_scroll: () => {},
      pop: () => {},
      done: () => 'DONE',
    },
    picker: {
      define: (n, o2) => (calls.picker = { n, o: o2 }),
      run: () => {},
    },
    command_args: () => o.args || [],
    layout: {
      get: () => {
        calls.layout.push('get');
        return o.layout_get ? o.layout_get() : '{}';
      },
      restore: (json) => {
        calls.layout.push('restore:' + json);
      },
    },
    run_async: (cmd) => {
      calls.ran_async.push(cmd);
      return Promise.resolve(o.home || '/home/test');
    },
    run: (cmd) => {
      calls.ran_async.push(cmd);
      return o.home || '/home/test';
    },
    read_file: (path) => {
      calls.reads.push(path);
      if (o.history === undefined) throw new Error('ENOENT');
      return o.history;
    },
    write_file: (path, text) => {
      calls.writes.push([path, text]);
    },
    buffer: {
      list: () => (o.current_doc ? [{ id: 7, path: o.current_doc.path }] : []),
      current: () => (o.current_doc ? 7 : null),
    },
    set_register: (text, reg) => {
      calls.regs.push(text);
    },
    set_readonly: (on, all) => {
      calls.ran.push(`readonly:${on}:${all ? 'all' : 'doc'}`);
    },
    pane: {
      list: () => (o.panes ? o.panes() : []),
      focus_dir: (id, dir) => {
        if (o.focus) o.focus(id + ":" + dir);
      },
      focus_panel: (id) => {
        if (o.focus) o.focus("panel:" + id);
      },
      focus: (id) => {
        if (o.focus) o.focus(id);
      },
    },
    open_popup: (opts) => {
      calls.popup = opts;
      return 1;
    },
    // 弹层里用的是组件树;测试只需要“能被调”
    el: (kind, children, opts) => ({ kind, children, opts }),
    open_file: (...args) => calls.opened.push(args),
    set_virtual_text: (...args) => calls.vt.push({ args }),
    set_highlight: (path, sr, sc, er, ec, style) =>
      calls.highlights.push({ path, sr, sc, er, ec, style }),
    set_theme: (obj) => calls.theme.push(obj),
    get_style: (scope) => (o.styles && o.styles[scope]) || null,
    dap: {
      sessions: () => (o.sessions ? o.sessions() : []),
      stack_frames: (thread, id) => {
        calls.frames.push([thread, id]);
        return Promise.resolve(o.frames ? o.frames(thread) : []);
      },
      scopes: (frame) => {
        calls.scopes.push(frame);
        return Promise.resolve(o.scopes ? o.scopes(frame) : []);
      },
      variables: (ref, id, start, count) => {
        calls.variables.push([ref, id, start, count]);
        const v = o.variables ? o.variables(ref, id, start, count) : [];
        return v instanceof Error ? Promise.reject(v) : Promise.resolve(v);
      },
      eval: (expr) => {
        calls.eval.push(expr);
        const v = o.eval ? o.eval(expr) : { result: '1', type: 'i32' };
        return v instanceof Error ? Promise.reject(v) : Promise.resolve(v);
      },
      request: (method, params) => {
        calls.reqs.push([method, params]);
        const v = o.request ? o.request(method, params) : { value: params && params.value };
        return v instanceof Error ? Promise.reject(v) : Promise.resolve(v);
      },
      threads: (id) => Promise.resolve(o.threads ? o.threads(id) : []),
      next: () => {
        calls.steps.push('next');
        return Promise.resolve();
      },
      step_in: () => {
        calls.steps.push('step_in');
        return Promise.resolve();
      },
      step_out: () => {
        calls.steps.push('step_out');
        return Promise.resolve();
      },
      continue: () => {
        calls.steps.push('continue');
        return Promise.resolve();
      },
      pause: () => {
        calls.steps.push('pause');
        return Promise.resolve();
      },
      switch_session: (id) => {
        calls.switched.push(id);
        return Promise.resolve();
      },
      set_exception_breakpoints: (filters) => {
        calls.excfilters.push(filters);
        return Promise.resolve();
      },
      set_breakpoint: (...args) => {
        calls.setbp.push(args);
        return Promise.resolve();
      },
      remove_breakpoint: (...args) => {
        calls.rmbp.push(args);
        return Promise.resolve();
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

test('auto_open 不抢焦点(默认),focus_on_auto_open = true 才抢', async () => {
  const focused = [];
  const mk = (cfg) =>
    load({
      cfg,
      sessions: () => [stopped],
      frames: () => [{ id: 11, name: 'main', source: '/tmp/m.rs', line: 1, column: 0 }],
      scopes: () => [],
      panes: () => [{ id: 0, kind: 'EditorView', focused: true }],
      focus: (id) => focused.push(id),
    });
  const a = mk({ auto_open: true });
  a.calls.handlers['dap-stopped']();
  await tick();
  await tick();
  // 现在开面板前会先 focus_editor（锚点：面板要裂在**编辑区**上，而不是当前焦点所在的那半块），
  // 开完再 focus_editor 一次（不抢焦点）⇒ 允许重复，但**绝不能**焦点跑进面板 id
  assert.ok(focused.length >= 1, '面板出现时至少要把焦点放回编辑器一次');
  assert.ok(
    focused.every((id) => id === 0),
    `焦点必须留在编辑器（不能被面板抢走）: ${JSON.stringify(focused)}`,
  );
  assert.equal(a.calls.panels.length, 1, '面板照样打开');

  focused.length = 0;
  const b = mk({ auto_open: true, focus_on_auto_open: true });
  b.calls.handlers['dap-stopped']();
  await tick();
  await tick();
  // 抢焦点模式下也不再"开完把焦点送回编辑器"；开之前的锚点调用（id 0）允许存在
  assert.ok(
    !focused.includes(b.api.T.panel_id),
    `抢焦点时不应该把焦点送进编辑器: ${JSON.stringify(focused)}`,
  );
});

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
  assert.equal(calls.variables.length, 1, '第一次展开拉一次');
  assert.equal(calls.variables[0][0], 100, '请求的 reference');
  api.key_tree({ name: 'h' });
  const scope2 = api.T.nodes.find((n) => n.label === 'Local');
  api.T.cursor = api.T.nodes.indexOf(scope2);
  api.key_tree({ name: 'l' });
  await tick();
  assert.equal(calls.variables.length, 1, '第二次展开命中缓存');
});

test('树排版：名字/值/类型三段分开（值里不再带 :类型）', async () => {
  const { calls, api } = load({
    cfg: { auto_open: false, max_depth: 2, panel_size: 40 },
    sessions: () => [stopped],
    frames: () => [{ id: 11, name: 'main', source: '/tmp/m.c', line: 3, column: 0 }],
    scopes: () => [{ name: 'Locals', variablesReference: 7 }],
    variables: () => [
      { name: 'buf', value: 'hello world and more', type: 'char[64]', variablesReference: 0 },
      { name: 'n', value: '42', type: 'int', variablesReference: 0 },
    ],
  });
  calls.handlers['dap-stopped']();
  await tick();
  await tick();
  await tick();
  const buf = api.T.nodes.find((n) => n.label === 'buf');
  assert.ok(buf, '应有 buf 节点');
  assert.equal(buf.value, 'hello world and more', 'value 只存值，不带 :类型');
  assert.equal(buf.type, 'char[64]', 'type 独立字段');
  // 渲染出来的两行里，类型都在（截断后也留可见片段）
  calls.draws.length = 0;
  api.render_tree(null, { width: 60, height: 14 });
  const text = calls.draws.map((d) => (typeof d.t === 'string' ? d.t : JSON.stringify(d.t))).join('\n');
  assert.ok(/char\[64\]/.test(text) || /char/.test(text), `类型应出现在渲染结果: ${text.slice(0, 200)}`);
});

test('树排版：长值截断并标出还剩多少字符', () => {
  const { api } = load({ cfg: {} });
  const long = 'x'.repeat(80);
  const got = api.fit_value(long, 20);
  assert.ok(got.length < long.length, '应截断');
  assert.ok(/\(\+\d+\)$/.test(got), `应带 (+N) 提示: ${got}`);
});

test('监视失败：值列只显示 `?`，原因放右侧提示（不撑爆一行）', async () => {
  const { calls, api } = load({
    cfg: { auto_open: false, labels: 'en' },
    sessions: () => [stopped],
    frames: () => [{ id: 11, name: 'main', source: '/tmp/m.c', line: 3, column: 0 }],
    scopes: () => [{ name: 'Locals', variablesReference: 7 }],
    variables: () => [],
    // 求值失败：模拟"换了个程序，老监视表达式不存在了"
    // 桩约定：返回 `Error` 实例 ⇒ 变成 rejected promise（跟真机一致）
    eval: () => new Error("Error: <user expression 1>:1:1: use of undeclared identifier 'v'"),
  });
  api.T.watches = ['v'];
  calls.handlers['dap-stopped']();
  await tick();
  await tick();
  await tick();
  const w = api.T.nodes.find((n) => n.kind === 'var' && n.label === 'v');
  assert.ok(w, '应有监视节点');
  assert.equal(w.value, '?', '值位置只放 ?');
  // hint 会被 `short_err` 压到 40 字以内，测关键词就够
  assert.ok(/undeclared/.test(String(w.hint)), `原因应在 hint: ${w.hint}`);
  assert.equal(w.error, true);
});

test('作用域展开后为空 ⇒ 标 (empty)，不让人以为还在加载', async () => {
  const { calls, api } = load({
    cfg: { auto_open: false, max_depth: 1 },
    sessions: () => [stopped],
    frames: () => [{ id: 11, name: 'main', source: '/tmp/m.c', line: 3, column: 0 }],
    scopes: () => [{ name: 'Globals', variablesReference: 9 }],
    variables: () => [],
  });
  calls.handlers['dap-stopped']();
  await tick();
  await tick();
  const globals = api.T.nodes.find((n) => n.label === 'Globals');
  assert.ok(globals, '应有 Globals 节点');
  api.T.cursor = api.T.nodes.indexOf(globals);
  api.key_tree({ name: 'l' });
  await tick();
  await tick();
  const g2 = api.T.nodes.find((n) => n.label === 'Globals');
  assert.equal(g2.hint, '(empty)', `空作用域应有 (empty) 标记: ${g2.hint}`);
});

test('just_my_code：单步落到工作区外 ⇒ 自动 stepOut；断点停库里不动', async () => {
  const outside = [
    { id: 1, name: 'core::iter::sum', source: '/root/.rustup/toolchains/x/library/core/src/iter.rs', line: 5, column: 0 },
  ];
  // ① 单步（reason=step）⇒ 发 stepOut，且不渲染这一帧
  let ctx = load({
    cfg: { auto_open: false, just_my_code: true },
    cwd: '/tmp/proj',
    sessions: () => [stopped],
    frames: () => outside,
    scopes: () => [],
  });
  ctx.calls.handlers['dap-stopped']({ id: 1, reason: 'step', thread_id: 1 });
  await tick();
  await tick();
  assert.ok(
    ctx.calls.reqs.some(([m]) => m === 'stepOut'),
    `应自动 stepOut: ${JSON.stringify(ctx.calls.reqs)}`,
  );

  // ② 断点停在库里 ⇒ 原地停，不许自动跳走
  ctx = load({
    cfg: { auto_open: false, just_my_code: true },
    cwd: '/tmp/proj',
    sessions: () => [stopped],
    frames: () => outside,
    scopes: () => [],
  });
  ctx.calls.handlers['dap-stopped']({ id: 1, reason: 'breakpoint', thread_id: 1 });
  await tick();
  await tick();
  assert.ok(
    !ctx.calls.reqs.some(([m]) => m === 'stepOut'),
    '断点停在库代码里不应自动跳走',
  );
});

test('树搜索：命中递归保留祖先、光标跳到命中项、n 循环', async () => {
  const vars = [
    { name: 'alpha', value: '1', type: 'int', variablesReference: 0 },
    { name: 'beta', value: '2', type: 'int', variablesReference: 0 },
  ];
  const { calls, api } = load({
    cfg: { auto_open: false, max_depth: 2 },
    sessions: () => [stopped],
    frames: () => [{ id: 11, name: 'main', source: '/tmp/proj/m.c', line: 3, column: 0 }],
    scopes: () => [{ name: 'Locals', variablesReference: 7 }],
    variables: () => vars,
  });
  calls.handlers['dap-stopped']();
  await tick();
  await tick();
  await tick();
  api.T.filter = 'beta';
  api.flatten();
  const labels = api.T.nodes.map((n) => n.label);
  assert.ok(labels.includes('beta'), `应保留命中项: ${JSON.stringify(labels)}`);
  assert.ok(!labels.includes('alpha'), '非命中项应被过滤');
  assert.ok(labels.includes('Locals'), '祖先要保留（否则命中项看不到）');
  // 光标跳转 + 循环（只有一处命中 ⇒ n 原地）
  const idx = api.T.nodes.findIndex((n) => n.label === 'beta');
  api.T.cursor = 0;
  assert.equal(api.jump_match(1), 'handled');
  assert.equal(api.T.cursor, idx, 'n 应跳到命中项');
});

test('值里重复类型名 ⇒ 简化（`Point @ 0x…` → `…`）', () => {
  const { api } = load({ cfg: {} });
  // lldb 常见形态：值 = 类型名 + 地址
  assert.equal(api.trim_value('rsdbg::Point @ 0x7fffffffea88', 'rsdbg::Point'), '…');
  // 值就是类型名（Vec 的 summary）
  assert.equal(api.trim_value('alloc::vec::Vec<int, Global>', 'alloc::vec::Vec<int, Global>'), '…');
  // 有真实内容 ⇒ 保留（去掉重复的类型前缀）
  assert.equal(api.trim_value('rsdbg::Point { x: 1, y: 2 }', 'rsdbg::Point'), '{ x: 1, y: 2 }');
  // 跟类型无关的值 ⇒ 原样
  assert.equal(api.trim_value('42', 'int'), '42');
  assert.equal(api.trim_value('hello', ''), 'hello');
});

test('栈帧名：demangle 后缩短成 main/add；重名加模块；mangled 折叠进系统帧', async () => {
  const { api } = load({ cfg: {} });
  // 缩短规则
  assert.equal(api.frame_display('rsdbg::main', 'short'), 'main');
  assert.equal(api.frame_display('rsdbg::Point::len', 'short'), 'len');
  assert.equal(api.frame_display('rsdbg::Point::len', 'keeper'), 'Point::len');
  assert.equal(api.frame_display('rsdbg::add', 'full'), 'rsdbg::add');
  // 泛型 / hash / 闭包
  assert.equal(api.frame_display('core::ptr::drop_in_place<alloc::vec::Vec<u8>>', 'short'), 'drop_in_place');
  assert.equal(api.frame_display('rsdbg::add::h1234567890abcdef', 'short'), 'add');
  assert.equal(api.frame_display('rsdbg::main::{closure#0}', 'short'), 'main');
  // C++ 的 operator<< 不能被当成泛型剥掉
  assert.equal(api.frame_display('std::ostream::operator<<', 'short'), 'operator<<');
  // mangled ⇒ 当系统帧
  assert.equal(api.looks_mangled('_RNvCs1TQt3UJmI7b_5rsdbg4main'), true);
  assert.equal(api.looks_mangled('_ZN5rsdbg3addE'), true);
  assert.equal(api.looks_mangled('rsdbg::add'), false);
});

test('栈帧：mangled 的帧即使有源码也折叠进系统帧', async () => {
  const frames = [
    { id: 1, name: 'rsdbg::main', source: '/tmp/rsdbg/src/main.rs', line: 8, column: 0 },
    { id: 2, name: '_RNvCs1TQt3UJmI7b_5rsdbg4main', source: '/tmp/x/function.rs', line: 250, column: 0 },
  ];
  const { calls, api } = load({
    cfg: { auto_open: false },
    sessions: () => [stopped],
    frames: () => frames,
    scopes: () => [],
  });
  calls.handlers['dap-stopped']();
  await tick();
  await tick();
  const labels = api.T.nodes.map((n) => n.label);
  assert.ok(labels.some((l) => l.startsWith('main')), `应有 main: ${JSON.stringify(labels)}`);
  assert.ok(
    labels.some((l) => /system frames/.test(l)),
    `mangled 帧应折叠: ${JSON.stringify(labels)}`,
  );
  assert.ok(!labels.some((l) => l.startsWith('_R')), '不应出现原始 mangled 名');
});

test('分组名语言：labels = zh-CN 时针局部/全局是中文', async () => {
  const { calls, api } = load({
    cfg: { auto_open: false, max_depth: 1, labels: 'zh-CN' },
    sessions: () => [stopped],
    frames: () => [{ id: 11, name: 'main', source: '/tmp/m.c', line: 3, column: 0 }],
    scopes: () => [
      { name: 'Locals', variablesReference: 7 },
      { name: 'Globals', variablesReference: 8 },
    ],
  });
  calls.handlers['dap-stopped']();
  await tick();
  await tick();
  const labels = api.T.nodes.map((n) => n.label);
  assert.ok(labels.includes('局部'), `应有「局部」: ${JSON.stringify(labels)}`);
  assert.ok(labels.includes('全局'), `应有「全局」: ${JSON.stringify(labels)}`);
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

// ── t / f：切线程与栈帧（只影响本面板的查看/求值） ──

test('展开非当前帧节点 ⇒ 用 scopes(frame) 取作用域(不是 variables)', async () => {
  const frames = [
    // 路径要在桩工作区 `/tmp/proj` 里（否则按"工作区外 = 系统帧"被折叠）
    { id: 11, name: 'main', source: '/tmp/proj/m.rs', line: 10, column: 0 },
    { id: 12, name: 'inner', source: '/tmp/proj/m.rs', line: 20, column: 0 },
  ];
  const { calls, api } = load({
    cfg: { auto_open: false, max_depth: 0 },
    sessions: () => [stopped],
    frames: () => frames,
    scopes: (frame) => (frame === 12 ? [{ name: 'Args', variablesReference: 200 }] : []),
  });
  calls.handlers['dap-stopped']();
  await tick();
  await tick();
  const node = api.T.nodes.find((n) => n.frame === 12);
  assert.ok(node, `应有内层帧节点: ${JSON.stringify(api.T.nodes.map((n) => n.label))}`);
  api.T.cursor = api.T.nodes.indexOf(node);
  calls.scopes.length = 0;
  api.key_tree({ name: 'l' });
  await tick();
  assert.deepEqual(calls.scopes, [12], '按帧 id 拉 scopes');
  assert.deepEqual(calls.variables, [], '不该去调 variables(帧没有 variablesReference)');
  assert.ok(api.T.nodes.some((n) => n.label === 'Args'), '作用域进了树');
});

test('既无 ref 也无 frame 的节点展开 ⇒ 变不可展开,不抛', () => {
  const { api } = load({ cfg: {}, sessions: () => [] });
  const node = { id: 'x', depth: 0, label: 'x', value: '', expandable: true, expanded: false };
  api.T.nodes = [node];
  api.expand_node(node, 0);
  assert.equal(node.expandable, false, '没有可展开内容 ⇒ 摘掉 expandable');
  assert.equal(node.loading, false, '不该停在 loading 态');
});

// ── 底部面板：REPL / Output ──

test('底部面板:三个 tab 手绘 + Tab 循环 + 1/2/3 直达', () => {
  const { calls, api } = load({ cfg: {}, sessions: () => [stopped] });
  api.open_bottom('repl');
  assert.equal(calls.panels[0].side, 'bottom', 'REPL/Output/Breakpoints 在底部');
  calls.draws.length = 0;
  api.render_bottom(null, { width: 80, height: 12 });
  let texts = calls.draws.map((d) => d.t).join('|');
  assert.ok(texts.includes('▸REPL'), `tab 条应标出当前 tab: ${texts}`);
  assert.ok(texts.includes('Output'), `tab 条应有 Output: ${texts}`);
  assert.ok(texts.includes('Breakpoints'), `tab 条应有 Breakpoints: ${texts}`);
  // Tab 循环 REPL → Output → Breakpoints → REPL
  api.key_bottom({ name: 'Tab' });
  assert.equal(api.B.tab, 'output');
  api.key_bottom({ name: 'Tab' });
  assert.equal(api.B.tab, 'breakpoints');
  api.key_bottom({ name: 'Tab' });
  assert.equal(api.B.tab, 'repl');
  // REPL 页里数字是**输入**（要能写 `1+1`）
  api.B.input = '';
  api.key_bottom({ name: '3' });
  assert.equal(api.B.tab, 'repl', 'REPL 页里 3 不应切页');
  // 其它页里 3 仍是"直达断点页"的快捷键
  api.B.input = ""; // 清空输入 ⇒ Tab 才是切页（有输入时 Tab 是补全）
  api.key_bottom({ name: 'Tab' }); // → output
  api.key_bottom({ name: '3' });
  assert.equal(api.B.tab, 'breakpoints');
  // Esc = 回编辑器(面板保留)：不关面板
  api.key_bottom({ name: 'Esc' });
  assert.deepEqual(calls.closed, [], 'Esc 不该关面板');
  // q(非 REPL 页) = 关面板
  api.key_bottom({ name: 'q' });
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

test('Output: 多行块按行拆开、半行拼接、环形上限', () => {
  const { calls, api } = load({ cfg: {}, sessions: () => [stopped] });
  api.open_bottom('output');
  // 一个块里有多行 ⇒ 拆成多条（以前会粘成一行，面板里就不换行了）
  calls.handlers['dap-output']({ id: 1, category: 'build', output: 'a\nb\n' });
  assert.deepEqual(
    api.B.output.map((l) => l.text),
    ['a', 'b'],
  );
  // 半行：下一块接上
  calls.handlers['dap-output']({ id: 1, category: 'build', output: 'cc ' });
  calls.handlers['dap-output']({ id: 1, category: 'build', output: 'hello.c\n' });
  assert.equal(api.B.output.at(-1).text, 'cc hello.c');
  // 环形上限 + 跟随
  for (let i = 0; i < 520; i++) calls.handlers['dap-output']({ id: 1, output: 'line' + i + '\n' });
  assert.equal(api.B.output.length, 500, '环形上限');
  assert.equal(api.B.follow, true, '默认跟随尾部');
  api.key_bottom({ name: 'k' });
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
  assert.equal(seg.text, '#1 \uf04c t1', '用 nerd-font 的 pause 图标（⏸ 与图标字体不搭）');
  assert.equal(seg.style, 'error');
});

test('修饰键穿透:ctrl 组合 ignore,C-\\ 回主区,未映射键穿透', () => {
  const { api } = load({ cfg: {}, sessions: () => [] });
  assert.equal(api.key_tree({ ctrl: true, name: 'y' }), 'ignore');
  assert.equal(api.key_tree({ ctrl: true, name: '\\' }), 'handled');
  assert.equal(api.key_tree({ ctrl: true, name: '4' }), 'handled', 'C-4 与 C-\\ 同码');
  assert.equal(api.key_bottom({ ctrl: true, name: 'y' }), 'ignore');
  assert.equal(api.key_tree({ name: 'z' }), 'ignore');
});

test('断点页:行号 +1、带徽标,空态不空白', () => {
  const { calls, api } = load({
    cfg: {},
    sessions: () => [stopped],
    breakpoints: [{ path: '/tmp/a.rs', line: 3, verified: true, condition: 'x>1' }],
  });
  api.open_bottom('breakpoints');
  assert.equal(api.B.tab, 'breakpoints');
  calls.draws.length = 0;
  api.render_bottom(null, { width: 120, height: 12 });
  // 富文本段数组 ⇒ 拼接后再断言
  const texts = calls.draws
    .map((d) => (typeof d.t === 'string' ? d.t : d.t.map((sp) => sp.text).join('')))
    .join('|');
  assert.ok(/a\.rs:4/.test(texts), `行号应 +1（路径取末两段）: ${texts}`);
  assert.ok(texts.includes('●'), `verified 徽标: ${texts}`);
  assert.ok(texts.includes('条件: x>1'), `条件列: ${texts}`);
  assert.ok(texts.includes('d 删'), `键位提示: ${texts}`);
});

test('断点页:d 删除 / Enter 跳转', async () => {
  const { calls, api } = load({
    cfg: {},
    sessions: () => [stopped],
    breakpoints: [{ path: '/tmp/a.rs', line: 3, verified: true }],
  });
  api.open_bottom('breakpoints');
  api.key_breakpoints({ name: 'Enter' });
  assert.deepEqual(calls.opened, [['/tmp/a.rs', { row: 3 }]], 'Enter 跳到断点行(0 起始)');
  api.key_breakpoints({ name: 'd' });
  await tick();
  assert.deepEqual(calls.rmbp, [['/tmp/a.rs', 3]], 'd 调 remove_breakpoint(0 起始行)');
});

test('断点页:c 改条件时把 log/hit 原样带上(核心是整组覆盖)', async () => {
  const { calls, api } = load({
    cfg: {},
    sessions: () => [stopped],
    breakpoints: [{ path: '/tmp/a.rs', line: 3, verified: true, log: 'x is {x}', hit: '5' }],
  });
  api.open_bottom('breakpoints');
  api.key_breakpoints({ name: 'c' });
  assert.ok(calls.popup, 'c 应弹输入层（picker 收不到纯字符键）');
  for (const ch of 'x>1') calls.popup.onKey({ name: ch });
  calls.popup.onKey({ name: 'Enter' });
  await tick();
  assert.deepEqual(calls.setbp, [
    ['/tmp/a.rs', 3, { condition: 'x>1', log: 'x is {x}', hit: '5' }],
  ]);
});

test('断点页:l 改日志消息(空串 = 清除该项)', async () => {
  const { calls, api } = load({
    cfg: {},
    sessions: () => [stopped],
    breakpoints: [{ path: '/tmp/a.rs', line: 0, verified: false, condition: 'n>2', log: 'old' }],
  });
  api.open_bottom('breakpoints');
  api.key_breakpoints({ name: 'l' });
  calls.popup.onKey({ name: 'Backspace' });
  calls.popup.onKey({ name: 'Backspace' });
  calls.popup.onKey({ name: 'Backspace' });
  calls.popup.onKey({ name: 'Enter' });
  await tick();
  assert.deepEqual(calls.setbp, [['/tmp/a.rs', 0, { condition: 'n>2', log: null, hit: null }]]);
});

test(':dapui-focus 无参数:两个面板都开 ⇒ 轮换(树 ↔ 底)', () => {
  const focused = [];
  const { calls, api } = load({
    cfg: {},
    sessions: () => [stopped],
    panes: () => [{ id: 0, kind: 'EditorView', focused: true }],
    focus: (x) => focused.push(x),
  });
  api.open_tree();            // 树 id 1
  api.open_bottom('output');  // 底 id 2
  calls.commands['dapui-focus']();
  calls.commands['dapui-focus']();
  calls.commands['dapui-focus']();
  assert.deepEqual(
    focused.filter((x) => typeof x === 'string'),
    ['panel:2', 'panel:1', 'panel:2'],
    '第一次从树轮换到底,再回树',
  );
});

test(':dapui-focus bottom|tree:直达指定面板(没开就开)', () => {
  // bottom:没开底部 ⇒ 打开并聚焦
  const b = load({
    cfg: {},
    sessions: () => [stopped],
    args: ['bottom'],
    panes: () => [{ id: 0, kind: 'EditorView' }],
  });
  b.api.open_tree();
  b.calls.commands['dapui-focus']();
  assert.equal(b.api.B.tab, 'repl', '没开底部 ⇒ 用上次记住的页(默认 repl)打开');
  assert.ok(b.calls.panels.some((p) => p.side === 'bottom'));

  // tree:已开 ⇒ 只聚焦
  const focused = [];
  const t = load({
    cfg: {},
    sessions: () => [stopped],
    args: ['tree'],
    panes: () => [{ id: 0, kind: 'EditorView' }],
    focus: (x) => focused.push(x),
  });
  t.api.open_tree();
  focused.length = 0;
  t.calls.commands['dapui-focus']();
  assert.deepEqual(focused, ['panel:1']);
});

test(':dapui-focus:面板已开 ⇒ focus_panel(实例 id);没开 ⇒ 先开', () => {
  const focused = [];
  const a = load({
    cfg: {},
    sessions: () => [stopped],
    panes: () => [{ id: 0, kind: 'EditorView', focused: true }],
    focus: (x) => focused.push(x),
  });
  a.api.open_tree();
  a.calls.commands['dapui-focus']();
  assert.deepEqual(
    focused.filter((x) => typeof x === 'string'),
    ['panel:1'],
    '按面板实例 id 聚焦（open_panel 返回 1）',
  );

  focused.length = 0;
  const b = load({ cfg: {}, sessions: () => [stopped], panes: () => [{ id: 0, kind: 'EditorView' }] });
  b.calls.commands['dapui-focus']();
  assert.equal(b.calls.panels.length, 1, '没开就先开');
});

test('面板不接管会话键:n/s/o/c/p/b/t/f/S/v/T/r/l 全部 ignore（冒泡给编辑器同一张表）', () => {
  const { api } = load({ cfg: {}, sessions: () => [stopped] });
  for (const k of ['n', 's', 'o', 'c', 'p', 'b', 't', 'f', 'S', 'v', 'T', 'r', 'R', 'K']) {
    assert.equal(api.key_tree({ name: k }), 'ignore', `${k} 必须冒泡（面板不是第二套键位表）`);
  }
  // 例外：`l` 在树里是"展开节点"（j/k/h/l 导航），这是面板**唯一**自留的同形键
  assert.equal(api.key_tree({ name: 'l' }), 'handled');
});

test('w/W:监视表达式（add → 顶部 Watches 组 + eval；W 清空）', async () => {
  const { calls, api } = load({
    cfg: { auto_open: false, max_depth: 0 },
    sessions: () => [stopped],
    frames: () => [{ id: 11, name: 'main', source: '/tmp/m.c', line: 3, column: 0 }],
    scopes: () => [],
    eval: (expr) => ({ result: expr === 'x' ? '42' : '?', type: 'int' }),
  });
  calls.handlers['dap-stopped']();
  await tick();
  await tick();
  // 加监视：ask_text 弹层 → 输入 x → Enter
  api.key_tree({ name: 'w' }, { path: '/tmp/m.c', text: 'x', cursor: { row: 0, col: 0 }, selection: null });
  assert.ok(calls.popup, 'w 应弹输入层');
  calls.popup.onKey({ name: 'x' });
  calls.popup.onKey({ name: 'Enter' });
  await tick();
  await tick();
  await tick();
  assert.deepEqual(calls.eval, ['x'], '监视表达式被求值');
  assert.equal(api.T.watches.length, 1);
  const group = api.T.nodes.find((n) => n.id === 'watches');
  const watch = api.T.nodes.find((n) => n.id === 'w0');
  assert.ok(group, `应有 Watches 组: ${JSON.stringify(api.T.nodes.map((n) => n.label))}`);
  assert.match(watch.value, /42/, `监视值: ${watch.value}`);
  // 清空
  api.key_tree({ name: 'W' });
  assert.equal(api.T.watches.length, 0);
});

test('栈帧分组:无源码的折叠成"系统帧"一行,当前帧永远第一个', async () => {
  const frames = [
    { id: 11, name: 'add', source: '/tmp/hello.c', line: 3, column: 0 },
    { id: 12, name: 'main', source: '/tmp/hello.c', line: 7, column: 0 },
    { id: 13, name: '__libc_start_call_main', source: null, line: 0, column: 0 },
    { id: 14, name: '_start', source: null, line: 0, column: 0 },
  ];
  const { calls, api } = load({
    cfg: { auto_open: false, max_depth: 0 },
    sessions: () => [stopped],
    frames: () => frames,
    scopes: () => [],
  });
  calls.handlers['dap-stopped']();
  await tick();
  await tick();
  const labels = api.T.roots.map((n) => n.label);
  assert.match(labels[0], /add/, '当前帧第一');
  assert.ok(
    labels.some((l) => /system frames/.test(l)),
    `默认英文标签下应有系统帧分组: ${JSON.stringify(labels)}`,
  );
  assert.ok(!labels.some((l) => l.includes('__libc_start')), '系统帧不直接铺在顶层');
});

test('auto_open_bottom:停止时按配置打开底部对应页', async () => {
  for (const [cfgv, tab] of [
    [{ auto_open_bottom: 'output' }, 'output'],
    [{ auto_open_bottom: 'breakpoints' }, 'breakpoints'],
    [{ auto_open_repl: true }, 'repl'],
    [{ auto_open_bottom: 'off' }, null],
  ]) {
    const { calls, api } = load({ cfg: { auto_open: false, max_depth: 0, ...cfgv }, sessions: () => [stopped] });
    calls.handlers['dap-stopped']();
    await tick();
    await tick();
    if (tab === null) {
      assert.equal(calls.panels.length, 0, 'off ⇒ 不开底部');
    } else {
      assert.equal(calls.panels[0].side, 'bottom', JSON.stringify(cfgv));
      assert.equal(api.B.tab, tab, JSON.stringify(cfgv));
    }
  }
});

test('REPL:括号没配平 ⇒ Enter 续行，不立刻求值', async () => {
  const { calls, api } = load({ cfg: {}, sessions: () => [stopped] });
  assert.equal(api.unclosed_brackets('foo('), true);
  assert.equal(api.unclosed_brackets('foo({"a": 1})'), false);
  assert.equal(api.unclosed_brackets('"("'), false, '字符串里的括号不算');
  api.B.tab = 'repl';
  api.B.input = 'foo({';
  api.key_bottom({ name: 'Enter' });
  assert.equal(api.B.input, 'foo({\n', '应续行');
  assert.equal(api.B.more, true);
  assert.deepEqual(calls.eval, [], '还没求值');
});

test('REPL:C-l 清转录', async () => {
  const { api } = load({ cfg: {}, sessions: () => [stopped] });
  api.B.tab = 'repl';
  api.B.transcript.push({ text: 'a => 1', style: null });
  assert.equal(api.key_bottom({ name: 'l', ctrl: true }), 'handled');
  assert.equal(api.B.transcript.length, 0);
});

test('REPL:历史持久化（读 $HOME 下的缓存文件；求值后落盘）', async () => {
  const { calls, api } = load({
    cfg: {},
    sessions: () => [stopped],
    home: '/home/test',
    history: 'x\ny\nx\n',
  });
  api.repl_history_load();
  assert.equal(calls.reads[0], '/home/test/.cache/helix/dapui-repl-history');
  assert.deepEqual(api.B.history, ['y', 'x'], '去重保序');
  api.B.tab = 'repl';
  api.B.input = 'z + 1';
  api.key_bottom({ name: 'Enter' });
  await tick();
  assert.equal(calls.writes.length, 1);
  assert.deepEqual(calls.writes[0][0], '/home/test/.cache/helix/dapui-repl-history');
  assert.match(calls.writes[0][1], /z \+ 1/);
});


test('变量分页：拿满一页挂「更多」，展开它拉下一页并追加', async () => {
  const page = (start, n) =>
    Array.from({ length: n }, (_, i) => ({
      name: `v${start + i}`,
      value: String(start + i),
      type: 'int',
      variablesReference: 0,
    }));
  const { calls, api } = load({
    cfg: { auto_open: false, max_depth: 2 },
    sessions: () => [stopped],
    frames: () => [{ id: 11, name: 'main', source: '/tmp/m.c', line: 3, column: 0 }],
    scopes: () => [{ name: 'Locals', variablesReference: 77 }],
    variables: (ref, id, start) => {
      start = start || 0;
      // 第 0 页给满 200（模拟"还有更多"），第 200 页给 3 个
      return page(start, start === 0 ? 200 : 3);
    },
  });
  calls.handlers['dap-stopped']();
  await tick();
  await tick();
  await tick();
  assert.ok(
    calls.variables.some((v) => v && v[2] === 0 && v[3] === 200),
    `请求应带 start=0/count=200: ${JSON.stringify(calls.variables.slice(0, 3))}`,
  );
  const more = api.T.nodes.find((n) => String(n.label).startsWith('… 更多'));
  assert.ok(more, `应有「更多」节点: ${api.T.nodes.length}`);
  api.T.cursor = api.T.nodes.findIndex((n) => n.id === more.id);
  api.key_tree({ name: 'l' });
  await tick();
  await tick();
  const labels = api.T.nodes.map((n) => n.label);
  assert.ok(labels.includes('v200'), `追加下一页后应有 v200: ${labels.slice(-3)}`);
});

test('监视持久化：加/删落盘，下次启动读回（须显式 persist_watches = true）', async () => {
  const { calls, api } = load({
    cfg: { persist_watches: true },
    sessions: () => [stopped],
    home: '/home/test',
    history: 'x\ny\n',
  });
  // 读回（与 REPL 历史同一个 $HOME 解析器；这里读的是 dapui-watches）
  api.watches_load();
  // 现在按**工作区**分文件（`dapui-watches-<slug>`）
  assert.ok(
    calls.reads.some((p) => String(p).includes('dapui-watches-')),
    `应读按工作区分区的持久化文件: ${JSON.stringify(calls.reads)}`,
  );
  assert.deepEqual(api.T.watches, ['x', 'y']);
  // 落盘
  api.watches_save();
  const w = calls.writes.find(([p]) => String(p).includes('dapui-watches-'));
  assert.ok(w, '应写入持久化文件');
  assert.equal(w[1], 'x\ny\n');
});

test('变量分页：adapter 忽略 start ⇒ 不追加重复项、也不再挂「更多」', async () => {
  const page = (n) =>
    Array.from({ length: n }, (_, i) => ({
      name: `v${i}`,
      value: String(i),
      type: 'int',
      variablesReference: 0,
    }));
  const { calls, api } = load({
    cfg: { auto_open: false, max_depth: 2 },
    sessions: () => [stopped],
    frames: () => [{ id: 11, name: 'main', source: '/tmp/m.c', line: 3, column: 0 }],
    scopes: () => [{ name: 'Locals', variablesReference: 77 }],
    variables: () => page(200), // 无论 start 都同一批 = 不支持分页的 adapter
  });
  calls.handlers['dap-stopped']();
  await tick();
  await tick();
  await tick();
  const more = api.T.nodes.find((n) => String(n.label).startsWith('… 更多'));
  assert.ok(more, '首屏应挂「更多」');
  api.T.cursor = api.T.nodes.findIndex((n) => n.id === more.id);
  api.key_tree({ name: 'l' });
  await tick();
  await tick();
  const dups = api.T.nodes.filter((n) => n.label === 'v0').length;
  assert.equal(dups, 1, '重复项不应入树');
  assert.ok(
    api.T.nodes.some((n) => String(n.label).includes('不支持分页')),
    '应给出"adapter 不支持分页"的说明',
  );
});

test('多会话：每个会话一个节点，当前展开、其余折叠；Enter 切会话', async () => {
  const s1 = { ...stopped, id: 1, active: true, thread: 1 };
  const s2 = { ...stopped, id: 2, active: false, thread: 5 };
  const { calls, api } = load({
    cfg: { auto_open: false, max_depth: 0, show_all_sessions: true },
    sessions: () => [s1, s2],
    frames: (thread) =>
      thread === 1
        ? [{ id: 11, name: 'add', source: '/tmp/m.c', line: 3, column: 0 }]
        : [{ id: 21, name: 'loop', source: '/tmp/n.c', line: 7, column: 0 }],
    scopes: () => [],
  });
  calls.handlers['dap-stopped']();
  await tick();
  await tick();
  const labels = api.T.nodes.map((n) => n.label);
  assert.ok(labels.some((l) => l.startsWith('#1')), `应有 #1: ${JSON.stringify(labels)}`);
  assert.ok(labels.some((l) => l.startsWith('#2')), `应有 #2: ${JSON.stringify(labels)}`);
  // 当前会话展开（能看到 add 帧），另一个折叠（只有提示行）
  assert.ok(labels.some((l) => l.includes('add')), '当前会话应展开');
  assert.ok(!labels.some((l) => l.includes('loop')), '另一个会话默认折叠');
  // 选中 #2 按 Enter ⇒ 切会话
  const idx = api.T.nodes.findIndex((n) => String(n.label).startsWith('#2'));
  api.T.cursor = idx;
  api.key_tree({ name: 'Enter' });
  await tick();
  await tick();
  assert.deepEqual(calls.switched, [2], '应切到会话 2');
});

test('单会话时不加会话节点（零回归）', async () => {
  const { calls, api } = load({
    cfg: { auto_open: false, max_depth: 0 },
    sessions: () => [stopped],
    frames: () => [{ id: 11, name: 'add', source: '/tmp/m.c', line: 3, column: 0 }],
    scopes: () => [],
  });
  calls.handlers['dap-stopped']();
  await tick();
  await tick();
  assert.ok(!api.T.nodes.some((n) => String(n.label).startsWith('#1')), '单会话不包一层');
  assert.ok(api.T.nodes.some((n) => n.label.includes('add')));
});

test('REPL 补全：Tab 拉候选并补第一个，再 Tab 循环，打字清掉', async () => {
  const { calls, api } = load({
    cfg: {},
    sessions: () => [stopped],
    request: (method, params) => {
      assert.equal(method, 'completions');
      return { targets: [{ label: 'alpha' }, { label: 'beta' }] };
    },
  });
  api.B.tab = 'repl';
  api.B.input = 'al';
  assert.equal(api.key_bottom({ name: 'Tab' }), 'handled');
  await tick();
  await tick();
  assert.equal(calls.reqs.length, 1, '应发一次 completions');
  assert.equal(calls.reqs[0][1].column, 3, 'column 是 1-based 的行尾');
  assert.equal(api.B.input, 'alpha', '首次 Tab 补第一个');
  api.key_bottom({ name: 'Tab' });
  assert.equal(api.B.input, 'beta', '再 Tab 循环到第二个');
  api.key_bottom({ name: 'Tab', shift: true });
  assert.equal(api.B.input, 'alpha', 'Shift-Tab 往回');
  api.key_bottom({ name: 'x' });
  assert.equal(api.B.comp.items.length, 0, '继续打字清候选');
});

test('断点行标记：●/◯ 画在当前文档的断点行首', async () => {
  const { calls, api } = load({
    cfg: { auto_open: false, breakpoint_marks: true },
    breakpoints: [
      { path: '/tmp/a.c', line: 1, verified: true },
      { path: '/tmp/a.c', line: 3, verified: false },
    ],
    current_doc: { path: '/tmp/a.c' },
  });
  api.refresh_breakpoint_marks();
  const marks = calls.vt
    .filter((c) => (c.args || []).length >= 4)
    .map((c) => [c.args[1], c.args[3]]);
  assert.deepEqual(marks, [[1, '●'], [3, '◯']]);
});

test('REPL：Esc 有输入先清空，空输入才回编辑器', async () => {
  const { api } = load({ cfg: {}, sessions: () => [stopped] });
  api.B.tab = 'repl';
  api.B.input = 'x + 1';
  assert.equal(api.key_bottom({ name: 'Esc' }), 'handled');
  assert.equal(api.B.input, '', '应先清输入');
});

test('构建输出流式：第一行到达就开底部并切到 Output', async () => {
  const { calls, api } = load({ cfg: { auto_open: false }, sessions: () => [] });
  calls.handlers['dap-output']({ id: 0, category: 'build', output: '[build] Compiling foo\n' });
  assert.ok(calls.panels.some((p) => p.side === 'bottom'), '应自动开底部');
  assert.equal(api.B.tab, 'output');
  assert.ok(api.B.output.some((l) => String(l.text).includes('Compiling foo')));
  // stderr 也走同一条路
  calls.handlers['dap-output']({ id: 0, category: 'stderr', output: 'error: boom\n' });
  assert.ok(api.B.output.some((l) => String(l.text).includes('boom')));
});

test('locals_summary:当前帧 Locals 的一行摘要（树头部显示）', async () => {
  const { calls, api } = load({
    cfg: { auto_open: false, max_depth: 2 },
    sessions: () => [stopped],
    frames: () => [{ id: 11, name: 'add', source: '/tmp/m.c', line: 3, column: 0 }],
    scopes: () => [
      { name: 'Locals', variablesReference: 77 },
      { name: 'Globals', variablesReference: 78 },
    ],
    variables: (ref) =>
      ref === 77
        ? [
            { name: 'a', value: '2', type: 'int', variablesReference: 0 },
            { name: 'b', value: '3', type: 'int', variablesReference: 0 },
          ]
        : [],
  });
  calls.handlers['dap-stopped']();
  await tick();
  await tick();
  await tick();
  assert.equal(api.locals_summary(6), 'a=2 b=3');
});

test('/ 过滤：只留命中项与其祖先，且自动展开', async () => {
  const { calls, api } = load({
    cfg: { auto_open: false, max_depth: 0 },
    sessions: () => [stopped],
    frames: () => [{ id: 11, name: 'main', source: '/tmp/m.c', line: 3, column: 0 }],
    scopes: () => [{ name: 'Locals', variablesReference: 77 }],
    variables: () => [
      { name: 'alpha', value: '1', type: 'int', variablesReference: 0 },
      { name: 'beta', value: '2', type: 'int', variablesReference: 0 },
    ],
  });
  calls.handlers['dap-stopped']();
  await tick();
  await tick();
  await tick();
  // 展开 Locals 让变量进 nodes
  const idx = api.T.nodes.findIndex((n) => n.label === 'Locals');
  api.T.cursor = idx;
  api.key_tree({ name: 'l' });
  await tick();
  await tick();
  assert.ok(api.T.nodes.some((n) => n.label === 'alpha'));
  // 过滤 beta
  api.key_tree({ name: '/' });
  assert.ok(calls.popup, '/ 应弹输入层');
  calls.popup.onKey({ name: 'b' });
  calls.popup.onKey({ name: 'Enter' });
  const labels = api.T.nodes.map((n) => n.label);
  assert.ok(labels.includes('beta'), `应留 beta: ${JSON.stringify(labels)}`);
  assert.ok(!labels.includes('alpha'), `应过滤掉 alpha: ${JSON.stringify(labels)}`);
});

test('y:复制变量值到寄存器', async () => {
  const { calls, api } = load({
    cfg: { auto_open: false, max_depth: 2 },
    sessions: () => [stopped],
    frames: () => [{ id: 11, name: 'main', source: '/tmp/m.c', line: 3, column: 0 }],
    scopes: () => [{ name: 'Locals', variablesReference: 77 }],
    variables: () => [{ name: 'x', value: '42', type: 'int', variablesReference: 0 }],
  });
  calls.handlers['dap-stopped']();
  await tick();
  await tick();
  await tick();
  const idx = api.T.nodes.findIndex((n) => n.label === 'x');
  api.T.cursor = idx;
  assert.equal(api.key_tree({ name: 'y' }), 'handled');
  assert.deepEqual(calls.regs, ['42'], 'y 应把原始值写进寄存器');
});

test('d:在监视项上删掉那一条（其它节点不接管）', async () => {
  const { calls, api } = load({
    cfg: { auto_open: false, max_depth: 0 },
    sessions: () => [stopped],
    frames: () => [{ id: 11, name: 'main', source: '/tmp/m.c', line: 3, column: 0 }],
    scopes: () => [],
    eval: () => ({ result: '7', type: 'int' }),
  });
  calls.handlers['dap-stopped']();
  await tick();
  await tick();
  api.T.watches.push('a', 'b');
  api.load_roots();
  await tick();
  await tick();
  const idx = api.T.nodes.findIndex((n) => n.id === 'w1');
  assert.ok(idx >= 0, '应有 w1');
  api.T.cursor = idx;
  assert.equal(api.key_tree({ name: 'd' }), 'handled');
  assert.deepEqual(api.T.watches, ['a'], '删掉第二条');
  // 非监视节点 ⇒ 不接管
  api.T.cursor = 0;
  assert.equal(api.key_tree({ name: 'd' }), 'ignore');
});

test('e:改变量值 ⇒ setVariable(父 ref, 名字, 新值)', async () => {
  const { calls, api } = load({
    cfg: { auto_open: false, max_depth: 2 },
    sessions: () => [stopped],
    frames: () => [{ id: 11, name: 'main', source: '/tmp/m.c', line: 3, column: 0 }],
    scopes: () => [{ name: 'Locals', variablesReference: 77 }],
    variables: () => [{ name: 'x', value: '1', type: 'int', variablesReference: 0 }],
  });
  calls.handlers['dap-stopped']();
  await tick();
  await tick();
  await tick();
  const idx = api.T.nodes.findIndex((n) => n.label === 'x');
  assert.ok(idx >= 0, `应有变量 x: ${JSON.stringify(api.T.nodes.map((n) => n.label))}`);
  api.T.cursor = idx;
  assert.equal(api.key_tree({ name: 'e' }), 'handled');
  assert.ok(calls.popup, '应弹输入层');
  calls.popup.onKey({ name: 'Backspace' }); // 初值是原始值 "1"，删掉再输
  calls.popup.onKey({ name: '4' });
  calls.popup.onKey({ name: '2' });
  calls.popup.onKey({ name: 'Enter' });
  await tick();
  assert.equal(calls.reqs.length, 1);
  const [method, params] = calls.reqs[0];
  assert.equal(method, 'setVariable');
  assert.equal(params.variablesReference, 77, '父作用域的 variablesReference');
  assert.equal(params.name, 'x');
  assert.equal(params.value, '42');
});

test('断点页:未验证时把 adapter 的 message 也显示出来', () => {
  const { api } = load({
    cfg: {},
    sessions: () => [stopped],
    breakpoints: [{ path: '/tmp/a.c', line: 0, verified: false, message: 'no executable' }],
  });
  const parts = api.bp_parts({
    path: '/tmp/a.c',
    line: 0,
    verified: false,
    message: 'no executable',
  });
  assert.equal(parts.mark, '◯', '未验证 = ◯');
  assert.equal(parts.note, 'no executable', '未验证的原因要带上');
  assert.equal(parts.where, 'tmp/a.c:1', '路径取末两段 + 行号 +1');
});

test('perspective:停止时存布局,继续时恢复(只恢复一次)', async () => {
  const { calls } = load({
    cfg: { auto_open: false, perspective: true },
    sessions: () => [stopped],
    layout_get: () => '{"tree":"saved"}',
  });
  calls.handlers['dap-stopped']();
  await tick();
  assert.deepEqual(calls.layout, ['get'], '停止 ⇒ 存布局');
  calls.handlers['dap-stopped']();
  await tick();
  assert.deepEqual(calls.layout, ['get'], '重复停止不重复存');
  calls.handlers['dap-continued']();
  await tick();
  assert.deepEqual(calls.layout, ['get', 'restore:{"tree":"saved"}'], '继续 ⇒ 恢复');
  calls.handlers['dap-continued']();
  await tick();
  assert.equal(calls.layout.length, 2, '已经恢复过就不再动');
});

test('perspective = false ⇒ 完全不碰布局', async () => {
  const { calls } = load({ cfg: { auto_open: false, perspective: false }, sessions: () => [stopped] });
  calls.handlers['dap-stopped']();
  await tick();
  calls.handlers['dap-continued']();
  await tick();
  assert.deepEqual(calls.layout, []);
});

test('readonly_on_stop:停止上锁 / 继续解锁(走 :read-only,不重复置位)', async () => {
  const { calls, api } = load({
    cfg: { auto_open: false, max_depth: 0, readonly_on_stop: 'all' },
    sessions: () => [stopped],
  });
  calls.handlers['dap-stopped']();
  await tick();
  assert.deepEqual(calls.ran, ['readonly:true:all'], '停止 ⇒ 上锁');
  calls.handlers['dap-stopped']();
  await tick();
  assert.deepEqual(calls.ran, ['readonly:true:all'], '重复停止不重复上锁');
  calls.handlers['dap-continued']();
  await tick();
  assert.deepEqual(calls.ran, ['readonly:true:all', 'readonly:false:all'], '继续 ⇒ 解锁');
  calls.handlers['dap-continued']();
  await tick();
  assert.equal(calls.ran.length, 2, '已经解锁后不再动');
});

test('readonly_on_stop = off ⇒ 完全不动只读状态', async () => {
  const { calls } = load({ cfg: { auto_open: false, readonly_on_stop: 'off' }, sessions: () => [stopped] });
  calls.handlers['dap-stopped']();
  await tick();
  assert.deepEqual(calls.ran, []);
});

test('e:异常断点过滤器 ⇒ 多选弹层,Space 勾选,Esc 保存', async () => {
  const withFilters = {
    ...stopped,
    filters: [
      { filter: 'cpp_catch', label: 'C++ Catch', default: false },
      { filter: 'cpp_throw', label: 'C++ Throw', default: true },
    ],
  };
  const { calls, api } = load({
    cfg: {},
    sessions: () => [withFilters],
    breakpoints: [{ path: '/tmp/a.rs', line: 0, verified: true }],
  });
  api.open_bottom('breakpoints');
  api.key_breakpoints({ name: 'e' });
  assert.ok(calls.popup, 'e 应弹多选层');
  calls.popup.render();
  calls.popup.onKey({ name: ' ' });      // 勾上第一项
  calls.popup.onKey({ name: 'Esc' });    // 保存
  await tick();
  assert.deepEqual(calls.excfilters, [['cpp_throw', 'cpp_catch']], '默认值 + 新勾选');
});

test('e:adapter 没有过滤器 ⇒ 只提示,不弹层', () => {
  const { calls, api } = load({
    cfg: {},
    sessions: () => [stopped],
    breakpoints: [{ path: '/tmp/a.rs', line: 0, verified: true }],
  });
  api.open_bottom('breakpoints');
  api.key_breakpoints({ name: 'e' });
  assert.equal(calls.popup, null);
});

test('Esc / C-4 回编辑器:按 pane.list() 找 EditorView 的 id(不硬编码 0)', () => {
  const focused = [];
  const { api } = load({
    cfg: {},
    sessions: () => [stopped],
    panes: () => [
      { id: 7, kind: 'EditorView', focused: false },
      { id: 1, kind: 'PluginPanel', focused: true },
    ],
    focus: (id) => focused.push(id),
  });
  assert.equal(api.key_tree({ name: 'Esc' }), 'handled');
  assert.equal(api.key_bottom({ name: 'Esc' }), 'handled', '底部面板 Esc 同样回编辑器');
  assert.equal(api.key_tree({ ctrl: true, name: '4' }), 'handled');
  assert.deepEqual(focused, [7, 7, 7], '焦点给 EditorView 的真实 id');
});

test('没有 pane.list() 时回退到 0(老行为)', () => {
  const focused = [];
  const { api } = load({ cfg: {}, sessions: () => [stopped], focus: (id) => focused.push(id) });
  api.key_tree({ name: 'Esc' });
  assert.deepEqual(focused, [0]);
});

test('命令齐了:dapui / dapui-repl / dapui-output / dapui-breakpoints / dapui-close', () => {
  const { calls, api } = load({ cfg: {}, sessions: () => [stopped] });
  for (const name of [
    'dapui',
    'dapui-repl',
    'dapui-output',
    'dapui-breakpoints',
    'dapui-close',
    'dapui-focus',
  ]) {
    assert.equal(typeof calls.commands[name], 'function', `命令 ${name} 应注册`);
  }
  calls.commands['dapui-repl']();
  assert.equal(calls.panels[0].side, 'bottom');
  calls.commands['dapui-breakpoints']();
  assert.equal(api.B.tab, 'breakpoints', ':dapui-breakpoints 开底部断点页');
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
