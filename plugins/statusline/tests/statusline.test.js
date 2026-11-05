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
function load(cfg, styles, segments, opts) {
  const o = opts || {};
  const captured = { render: null, opts: null, theme: {}, schema: null, logs: [] };
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
    log(msg) {
      captured.logs.push(msg);
    },
    input_badges() {
      return o.badges || '';
    },
    // 跨平台核心 API（`helix.proc_stats()`）。不提供 ⇒ 插件回退读 /proc（旧二进制路径）
    ...(o.procStats === undefined
      ? {}
      : {
          proc_stats() {
            return o.procStats();
          },
        }),
    permissions() {
      return o.permissions || { process: false };
    },
    // 设备相关的段(cpu/mem)会 `helix.read_file('/proc/...')`。桩里**默认抛错**
    //（= 没有 /proc 的平台），要看那两条段的用例自己传 `opts.readFile`。
    read_file(path) {
      if (o.readFile) return o.readFile(path);
      throw 'read_file: no such file ' + path;
    },
    icons: {
      ...(o.sysIcons === undefined ? {} : { sys: (k) => o.sysIcons[k] || '' }),
      mode: (m) => ({ normal: 'N', insert: 'I', select: 'S' })[m] || '?',
      file: () => 'F',
      dir: () => 'D',
      diagnostic: (k) => ({ error: 'E', warning: 'W' })[k] || '?',
    },
    // ⚠ 桩要跟引擎**同语义**：插件 `set_theme` 写进去的 scope，`get_style` 要能读回来
    //（真实引擎里 theme 快照就包含插件覆盖）。否则 capScope 的"主题里已有正确值"判断永远为假
    // ⇒ 端帽拿不到派生 scope（曾经就是这么假绿的）。
    get_style(scope) {
      if (Object.prototype.hasOwnProperty.call(captured.theme, scope)) {
        return captured.theme[scope];
      }
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

// 假时钟：插件只把 `Date.now()` 当作【节流】与【差分基准】，换掉它就够了（不需要假 timer）。
function fakeClock(start) {
  const real = Date.now;
  let t = start || 1000;
  Date.now = () => t;
  return {
    advance(ms) {
      t += ms;
    },
    restore() {
      Date.now = real;
    },
  };
}

// /proc 桩：按路径返回预置内容；未预置的路径抛错（= 真实 `read_file` 的语义）。
function procFiles(proc) {
  return (p) => {
    if (p === '/proc/self/schedstat') return `${proc.cpuNs} 5000 3\n`;
    if (p === '/proc/self/status') return `Name:\thelix\nVmRSS:\t${proc.rssKb} kB\n`;
    throw 'read_file: no such file ' + p;
  };
}

test('默认布局:两端 line 圆角 + 各段归位', () => {
  const cap = load({}, null);
  const parts = cap.render(ctx());
  const line = joined(parts);
  assert.equal(parts[0].text, '╭', `left 区从 ╭ 起: ${line}`);
  assert.equal(texts(parts).at(-1), '╮', `right 区以 ╮ 收: ${line}`);
  assert.ok(line.includes('F a.rs'), `文件名段: ${line}`);
  assert.deepEqual(texts(zone(parts, 'right')), ['1 char', 'rust', '4:8', '4%', '╮']);
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

test('powerline 多模式端帽共存:切模式不抹掉前一个模式的派生 scope', () => {
  // 这条是回归测试：`set_theme` 曾经是**整体替换**语义，于是 normal 注册完再切 insert，
  // 后一次调用会把 `statusline.cap.normal` 抹掉（端帽掉色）。改成合并语义后两者共存。
  // 桩的 `set_theme` 也按合并实现（Object.assign），与真实 API 一致。
  const styles = {
    'ui.statusline': { bg: '#101010' },
    'ui.statusline.normal': { fg: '#fff', bg: '#0000ff' },
    'ui.statusline.insert': { fg: '#000', bg: '#00ff00' },
  };
  const cap = load({ corners: 'powerline' }, styles);

  // 第一次渲染：normal 模式 → 注册 cap.normal
  const normal = ctx();
  normal.mode = 'normal';
  cap.render(normal);
  assert.deepEqual(cap.theme['statusline.cap.normal'], { fg: '#0000ff', bg: '#101010' });

  // 切到 insert：注册 cap.insert，**不能**把 cap.normal 抹掉
  const insert = ctx();
  insert.mode = 'insert';
  const parts = cap.render(insert);
  assert.deepEqual(cap.theme['statusline.cap.insert'], { fg: '#00ff00', bg: '#101010' });
  assert.deepEqual(
    cap.theme['statusline.cap.normal'],
    { fg: '#0000ff', bg: '#101010' },
    `切模式后前一个模式的 cap scope 必须还在: ${JSON.stringify(cap.theme)}`,
  );

  // 再切回 normal：用的还是自己的 scope（不是 insert 的）
  const back = cap.render(normal);
  const capPart = back.find((p) => p.text === '\ue0b6');
  assert.equal(capPart.style, 'statusline.cap.normal');

  // insert 那次渲染的端帽也用自己那条
  const insertCap = parts.find((p) => p.text === '\ue0b6');
  assert.equal(insertCap.style, 'statusline.cap.insert');
});

// ──────────────────── window 并进 layers（不再各占一格） ────────────────────
// 旧 `window` 段只在 `window_mode` 下显示一个**叶子类型图标**（跟 `file` 段重复），
// 而它读的 `ctx.window_mode` 就是 `pane_mode != Normal` 的镜像 —— pane 模式早就有
// `PANE`/`LOCKED`/… 徽标 ⇒ 两格报的是同一件事。所以 `window` 整段消失，`layers` 只留徽标串。
test('layers 只输出徽标串(不再有 WIN 色块/虫子图标)', () => {
  const cap = load({ corners: 'off', left: ['layers', 'file'] }, null, null, {
    badges: 'DEBUG│PANE',
  });
  const on = ctx();
  on.window_mode = true; // = 正在 pane 模式里，徽标已经说过这件事了
  const parts = zone(cap.render(on), 'left');
  assert.deepEqual(texts(parts), ['DEBUG│PANE', 'F a.rs']);
  assert.equal(parts[0].style, 'ui.virtual');
  assert.ok(!joined(parts).includes('WIN'), `不该再有 WIN 色块: ${joined(parts)}`);
});

test('layers:没有挂着的层 ⇒ 整段隐藏(不留孤立图标/空格)', () => {
  const cap = load({ corners: 'off', left: ['layers', 'file'] }, null, null, { badges: '' });
  const on = ctx();
  on.window_mode = true; // window_mode 单独为真也不该凭空长出一格
  assert.deepEqual(texts(zone(cap.render(on), 'left')), ['F a.rs']);
  assert.deepEqual(texts(zone(cap.render(ctx()), 'left')), ['F a.rs']);
});

test('window 是旧段名:老配置写了不渲染、不报红字，只提醒一次', () => {
  const cap = load({ corners: 'off', left: ['window', 'layers', 'file'] }, null, null, {
    badges: 'PANE',
  });
  const parts = zone(cap.render(ctx()), 'left');
  assert.deepEqual(texts(parts), ['PANE', 'F a.rs']);
  assert.ok(!joined(parts).includes('!window'), `合并不是拼错，不该报红字: ${joined(parts)}`);
  // 每帧渲染，但提醒只该打一次（否则日志刷屏）
  cap.render(ctx());
  cap.render(ctx());
  assert.equal(cap.logs.length, 1, `该只提醒一次: ${JSON.stringify(cap.logs)}`);
  assert.ok(cap.logs[0].includes('window') && cap.logs[0].includes('layers'), cap.logs[0]);
});

// ────────────────────── cpu / mem（本进程自身开销） ──────────────────────
// 只看这两条时把其它段都关掉（左右区各自成串，断言就不依赖默认布局）
const ONLY_METRICS = { corners: 'off', left: ['cpu', 'mem'], center: [], right: [] };
const fsOk = { process: true, fs_read: true };

test('cpu/mem:首帧无 CPU 基线 ⇒ 只显示 mem;第二帧起 cpu = 差分比', () => {
  const clock = fakeClock(1000);
  try {
    const proc = { cpuNs: 1e9, rssKb: 153600 }; // 150M
    const cap = load(ONLY_METRICS, null, null, { permissions: fsOk, readFile: procFiles(proc) });

    assert.deepEqual(texts(zone(cap.render(ctx()), 'left')), ['mem 150M']);

    // 墙钟走 1000ms、CPU 只走 500ms ⇒ 50%
    proc.cpuNs += 0.5e9;
    clock.advance(1000);
    assert.deepEqual(texts(zone(cap.render(ctx()), 'left')), ['cpu 50%', 'mem 150M']);

    // 跨过 1000MB 的档位走 GB 记法
    proc.cpuNs += 1e9;
    proc.rssKb = 2097152; // 2G
    clock.advance(1000);
    assert.deepEqual(texts(zone(cap.render(ctx()), 'left')), ['cpu 100%', 'mem 2.0G']);
  } finally {
    clock.restore();
  }
});

test('cpu/mem:1s 内重复渲染复用同一次 /proc 读数(状态栏是每帧钩子)', () => {
  const clock = fakeClock(1000);
  try {
    let reads = 0;
    const proc = { cpuNs: 1e9, rssKb: 1024 };
    const reader = procFiles(proc);
    const cap = load({ corners: 'off' }, null, null, {
      permissions: fsOk,
      readFile: (p) => {
        reads++;
        return reader(p);
      },
    });
    cap.render(ctx());
    cap.render(ctx());
    cap.render(ctx());
    assert.equal(reads, 2, `三次渲染只该读一次 schedstat + 一次 status: ${reads}`);
    clock.advance(1000);
    cap.render(ctx());
    assert.equal(reads, 4, '过了采样间隔才再读一次');
  } finally {
    clock.restore();
  }
});

test('cpu/mem:没有 /proc(非 Linux)⇒ 两段静默隐藏,不报错', () => {
  const cap = load(ONLY_METRICS, null, null, {
    permissions: fsOk,
    readFile: () => {
      throw 'ENOENT';
    },
  });
  assert.deepEqual(texts(zone(cap.render(ctx()), 'left')), []);
});

test('cpu/mem:没批准 fs_read ⇒ 段隐藏,且不去读文件', () => {
  let reads = 0;
  const cap = load(ONLY_METRICS, null, null, {
    permissions: { process: true },
    readFile: () => {
      reads++;
      return '';
    },
  });
  assert.deepEqual(texts(zone(cap.render(ctx()), 'left')), []);
  assert.equal(reads, 0, '没权限就不该先读一次再收拒绝');
});

test('默认布局:cpu/mem 在右区最前,position/percent 收在最右', () => {
  const clock = fakeClock(1000);
  try {
    const proc = { cpuNs: 2e9, rssKb: 153600 };
    const cap = load({ corners: 'off' }, null, null, { permissions: fsOk, readFile: procFiles(proc) });
    cap.render(ctx()); // 建立 CPU 基线
    proc.cpuNs += 0.25e9;
    clock.advance(1000);
    const parts = cap.render(ctx());
    assert.deepEqual(texts(zone(parts, 'left')), [' I ', 'F a.rs']);
    assert.deepEqual(texts(zone(parts, 'right')), [
      'cpu 25%', 'mem 150M',
      '1 char', 'rust',
      '4:8', '4%',
    ]);
  } finally {
    clock.restore();
  }
});

// ────────────────────── cpu/mem 的图标（核心图标表 helix.icons.sys） ──────────────────────
test('cpu/mem:有图标就只显示图标+值,没有图标才带名字', () => {
  const clock = fakeClock(1000);
  try {
    const mk = (opts) => {
      const proc = { cpuNs: 1e9, rssKb: 153600 };
      const cap = load(ONLY_METRICS, null, null, {
        permissions: fsOk,
        readFile: procFiles(proc),
        ...opts,
      });
      cap.render(ctx());
      proc.cpuNs += 0.5e9;
      clock.advance(1000);
      return texts(zone(cap.render(ctx()), 'left'));
    };
    // 图标表关掉（nerd font = false）⇒ 纯文字，不留空格占位
    assert.deepEqual(mk({}), ['cpu 50%', 'mem 150M']);
    assert.deepEqual(mk({ sysIcons: {} }), ['cpu 50%', 'mem 150M']);
    // 有图标 ⇒ **只要图标**（名字不再重复），值与单位不变
    assert.deepEqual(mk({ sysIcons: { cpu: 'C', mem: 'M' } }), ['C 50%', 'M 150M']);
  } finally {
    clock.restore();
  }
});

// ──────────────── 跨平台核心 API：helix.proc_stats() ────────────────
test('cpu/mem:优先用 helix.proc_stats()(跨平台),完全不读 /proc', () => {
  const clock = fakeClock(1000);
  try {
    let reads = 0;
    const proc = { cpuMs: 1000, rssKb: 204800 }; // 1s CPU / 200M
    const cap = load(ONLY_METRICS, null, null, {
      permissions: fsOk,
      procStats: () => ({ cpu_ms: proc.cpuMs, rss_kb: proc.rssKb }),
      readFile: () => {
        reads++;
        return '';
      },
    });
    cap.render(ctx()); // 建基线
    proc.cpuMs += 250; // 墙钟 1000ms 里用了 250ms ⇒ 25%
    clock.advance(1000);
    assert.deepEqual(texts(zone(cap.render(ctx()), 'left')), ['cpu 25%', 'mem 200M']);
    assert.equal(reads, 0, '有 proc_stats 就不该去读 /proc(那是 Linux-only 的回退)');
  } finally {
    clock.restore();
  }
});

test('cpu/mem:proc_stats 返回 null(平台不支持)⇒ 两段隐藏,不报错', () => {
  const cap = load(ONLY_METRICS, null, null, {
    permissions: fsOk,
    procStats: () => null,
    readFile: () => {
      throw 'ENOENT';
    },
  });
  assert.deepEqual(texts(zone(cap.render(ctx()), 'left')), []);
});

test('cpu/mem:rss_kb = 0 表示"这项不可用" ⇒ 只有 cpu 段', () => {
  const clock = fakeClock(1000);
  try {
    let cpu = 100;
    const cap = load(ONLY_METRICS, null, null, {
      permissions: fsOk,
      procStats: () => ({ cpu_ms: cpu, rss_kb: 0 }),
    });
    cap.render(ctx());
    cpu = 600;
    clock.advance(1000);
    assert.deepEqual(texts(zone(cap.render(ctx()), 'left')), ['cpu 50%']);
  } finally {
    clock.restore();
  }
});
