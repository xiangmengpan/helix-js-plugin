// statusline node 级测试 —— 配置面(段名/位置/圆角)的纯 JS 行为。
// 运行:node --test plugins/statusline/tests/statusline.test.js
// 原理:插件不是 CommonJS 模块(顶层直接跑),所以在 require 之前注入最小 `global.helix` 桩,
//      从 `set_statusline(fn, opts)` 捕获 render 与 zones,再拿自造 ctx 直接调它。
//      桩里 `get_style` 返回假颜色 ⇒ powerline 端帽的派生 scope 路径也能被真的走一遍。
'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');

const PLUGIN = path.join(__dirname, '..', 'plugin.js');

// 一份"每个字段都非默认"的 ctx:断言里出现某个值,必定来自它(不是恰好等于默认值)
function ctx() {
  return {
    path: '/tmp/a.rs',
    mode: 'insert',
    cursor: { row: 3, col: 7 }, // ⇒ 4:8 / 4%
    total_lines: 100,
    diagnostics_error: 2,
    diagnostics_warning: 1,
    window_mode: false,
    active_leaf_type: 'editor',
    active_leaf_path: null,
    modified: false,
    selections: 1,
    selections_primary: 0,
    read_only: false,
    encoding: 'utf-8',
    line_ending: 'LF',
    register: null,
    primary_selection_length: 1,
    indent_style: '4 spaces',
    file_type: 'rust',
    cwd: 'proj',
    code_action_hint: false,
    workspace_diagnostics_error: 0,
    workspace_diagnostics_warning: 0,
    workspace_diagnostics_info: 0,
    workspace_diagnostics_hint: 0,
    spinner: null,
  };
}

// helix 最小桩。styles 为 null 时模拟"拿不到颜色"(无 theme 源)的情形。
// segments: 外部段的桩(段名 → 函数)。未注册的段名照**核心的真实语义**抛一个**字符串**
// (不是 Error 对象) —— 插件靠 catch 区分"拼错"与"本帧没内容"。
function load(cfg, styles, segments) {
  const captured = { render: null, opts: null, theme: {}, schema: null };
  global.helix = {
    plugin() {},
    define_config(name, schema) {
      captured.schema = { name, schema };
    },
    get_config() {
      return cfg;
    },
    set_statusline(fn, opts) {
      captured.render = fn;
      captured.opts = opts;
    },
    on() {},
    permissions() {
      return { process: false };
    },
    icons: {
      mode: (m) => ({ normal: 'N', insert: 'I', select: 'S' })[m] || '?',
      file: () => 'F',
      dir: () => 'D',
      diagnostic: (k) => ({ error: 'E', warning: 'W' })[k] || '?',
    },
    get_style(scope) {
      return styles ? styles[scope] || null : null;
    },
    statusline_segment(name, ctx) {
      if (!segments || !Object.prototype.hasOwnProperty.call(segments, name)) {
        throw "helix.statusline_segment: no segment registered as " + name;
      }
      return segments[name](ctx);
    },
    set_theme(obj) {
      Object.assign(captured.theme, obj);
    },
    run_async: async () => '',
  };
  delete require.cache[require.resolve(PLUGIN)];
  require(PLUGIN);
  assert.ok(captured.render, '插件应注册状态栏 render');
  return captured;
}

const texts = (parts) => parts.map((p) => p.text);
const joined = (parts) => texts(parts).join('|');
const zone = (parts, z) => parts.filter((p) => (p.zone || 'left') === z);

test('默认布局:两端 line 圆角 + 各段归位', () => {
  const cap = load({}, null);
  const parts = cap.render(ctx());
  const line = joined(parts);
  assert.equal(parts[0].text, '╭', `left 区从 ╭ 起: ${line}`);
  assert.equal(texts(parts).at(-1), '╮', `right 区以 ╮ 收: ${line}`);
  assert.ok(line.includes('F a.rs'), `文件名段: ${line}`);
  assert.deepEqual(texts(zone(parts, 'right')), ['4:8', '4%', '1 char', '4 spaces', 'rust', 'proj', '100', '╮']);
  assert.deepEqual(texts(zone(parts, 'center')), ['E 2', 'W 1'], '诊断进 center');
  assert.equal(parts[0].style, null, '圆角继承状态栏底色');
});

test('corners = "off":一个圆角字符都不出现', () => {
  const cap = load({ corners: 'off' }, null);
  const line = joined(cap.render(ctx()));
  assert.ok(!line.includes('╭') && !line.includes('╮'), line);
  assert.ok(line.includes('a.rs'), line);
});

test('段列表决定内容与所在区;不适用的段自动隐藏', () => {
  const cap = load(
    { corners: 'off', left: ['file'], center: [], right: ['modified', 'position'] },
    null,
  );
  const parts = cap.render(ctx());
  assert.deepEqual(texts(zone(parts, 'left')), ['F a.rs']);
  assert.deepEqual(zone(parts, 'center'), []);
  // modified=false ⇒ 空段被丢(不留双空格),只剩 position
  assert.deepEqual(texts(zone(parts, 'right')), ['4:8']);
});

test('拼错的段名变成红字 !名字(不静默)', () => {
  const cap = load({ corners: 'off', left: ['file', 'nope'] }, null);
  const parts = cap.render(ctx());
  const bad = parts.find((p) => p.text === '!nope');
  assert.ok(bad, joined(parts));
  assert.equal(bad.style, 'error');
});

test('zones 从 config 读(注册态):合法值生效、非法回默认', () => {
  assert.deepEqual(load({}, null).opts.zones, [1, 3, 2]);
  assert.deepEqual(load({ zones: [1, 1, 1] }, null).opts.zones, [1, 1, 1]);
  assert.deepEqual(load({ zones: [0, 1, 1] }, null).opts.zones, [1, 3, 2], '0 非法 ⇒ 回默认');
  assert.deepEqual(load({ zones: 'nope' }, null).opts.zones, [1, 3, 2], '类型错 ⇒ 回默认');
  assert.equal(load({}, null).opts.replace, true);
});

test('corners = "powerline":mode 色块两侧有端帽,颜色走派生 scope', () => {
  const styles = { 'ui.statusline': { bg: '#101010' }, 'ui.statusline.insert': { fg: '#000', bg: '#00ff00' } };
  const cap = load({ corners: 'powerline' }, styles);
  const parts = cap.render(ctx());
  const line = texts(parts).join('');
  assert.ok(line.includes('\ue0b6') && line.includes('\ue0b4'), line);
  // 派生 scope:fg = mode 段底色,bg = 状态栏底色
  assert.deepEqual(cap.theme['statusline.cap.insert'], { fg: '#00ff00', bg: '#101010' });
  const capPart = parts.find((p) => p.text === '\ue0b6');
  assert.equal(capPart.style, 'statusline.cap.insert');
  // mode 色块本体仍在两帽之间
  const i = texts(parts).indexOf(' I ');
  assert.equal(texts(parts)[i - 1], '\ue0b6');
  assert.equal(texts(parts)[i + 1], '\ue0b4');
});

test('corners = "powerline" 但拿不到颜色 ⇒ 退化为无帽,不抛错', () => {
  const cap = load({ corners: 'powerline' }, null);
  const line = texts(cap.render(ctx())).join('');
  assert.ok(!line.includes('\ue0b6') && !line.includes('\ue0b4'), line);
  assert.ok(line.includes('a.rs'), '其余内容照常');
});

test('外部段:别的插件注册的段能被渲染,且受 corners/空段规则约束', () => {
  // dapui 这类插件:注册 "dap",由 statusline 按段名取
  const segments = { dap: () => ({ text: 'DAP', style: 'error' }) };
  const cap = load({ corners: 'off', left: ['file', 'dap'] }, null, segments);
  const parts = cap.render(ctx());
  const seg = parts.find((p) => p.text === 'DAP');
  assert.ok(seg, `外部段应出现: ${joined(parts)}`);
  assert.equal(seg.style, 'error', 'style 原样透传');
  assert.ok(!parts.some((p) => p.text === '!dap'), '已注册就不该出现 !dap');
});

test('外部段:未注册(拼错)⇒ !段名 红字;段自己返回 null ⇒ 静默跳过', () => {
  // 未注册:桩照核心语义抛字符串 ⇒ 渲染成红字 !名字(拼错可见)
  const typo = load({ corners: 'off', left: ['file', 'nope'] }, null, {});
  const p1 = typo.render(ctx());
  const bad = p1.find((p) => p.text === '!nope');
  assert.ok(bad, `拼错应可见: ${joined(p1)}`);
  assert.equal(bad.style, 'error');

  // 已注册但本帧没内容:返回 null ⇒ 跳过(不显示 !,也不留空段)
  const silent = load({ corners: 'off', center: ['dap'], left: ['file'] }, null, { dap: () => null });
  const p2 = silent.render(ctx());
  assert.deepEqual(zone(p2, 'center'), [], '空的外部段应被丢弃');
});

test('外部段:段函数抛错不得拖掉整条状态栏', () => {
  // 核心侧段函数抛错会返回 null;但即使渲染方拿到的是异常,也只坏这一格
  const segments = { boom: () => { throw 'kaboom'; } };
  const cap = load({ corners: 'off', left: ['file', 'boom'] }, null, segments);
  const parts = cap.render(ctx());
  const line = joined(parts);
  assert.ok(line.includes('a.rs'), `其它段必须还在: ${line}`);
  assert.ok(line.includes('!boom'), `坏掉的那格标出来: ${line}`);
});
