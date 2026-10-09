// dashboard/plugin.js — LazyVim 风格的启动界面(B 方案:buffer 式)。
//
// ## 为什么是 buffer 式
// LazyVim 的启动屏本质是**一个空白无名 buffer + 局部键位**;
// 本 fork 里 `map` 是**全局**表 ✗ ⇒ 用"**开屏绑 / 关屏解绑**"实现等价的按键作用域 ✓
// (依赖 `helix.map(..., { unbind: true })` ✓)
//
// ## 依赖的薄能力(都已就位 ✓)
//   `startup` 事件 · `open_file(p, {scratch})` · `insert` · `map(..., {unbind})` · `icons` ✓
//
// ## 安全设计(重要)
// **任何一步失败都不抛到顶层** —— 启动屏是锦上添花,绝不能让它把编辑器弄坏 ✓
// 所有 API 探测都包 `guard()`;探测不到能力时**静默降级**(只保留 `:dashboard` 命令)✓
helix.plugin("dashboard", { deps: [], version: "1.0" });

// ── 可配置 ────────────────────────────────────────────────────────────────
helix.define_config("dashboard", {
  enabled: { type: "boolean", default: true, doc: "无文件启动时是否显示启动屏" },
  logo: { type: "string", default: "blocks", doc: "logo 风格:blocks | slim | none" },
});

const LOGO_BLOCKS = [
  "  ██╗  ██╗███████╗██╗     ██╗██╗  ██╗",
  "  ██║  ██║██╔════╝██║     ██║╚██╗██╔╝",
  "  ███████║█████╗  ██║     ██║ ╚███╔╝ ",
  "  ██╔══██║██╔══╝  ██║     ██║ ██╔██╗ ",
  "  ██║  ██║███████╗███████╗██║██╔╝ ██╗",
  "  ╚═╝  ╚═╝╚══════╝╚══════╝╚═╝╚═╝  ╚═╝",
];
const LOGO_SLIM = ["  H E L I X  ⌘"];

// ── 状态 ──────────────────────────────────────────────────────────────────
let dash_open = false; // 启动屏是否已开(不新开 buffer ⇒ 只需一个标记 ✓)
let dash_buffer_id = null; // 启动屏所在 buffer(用于"真的离开" ✓)
// 诊断计数器:用来分清"哪条触发路径真的跑了" ✓(纯观测,不影响行为 ✓)
let n_startup = 0; // startup 事件次数
let n_buffer_open = 0; // buffer-open 事件次数
let n_open_calls = 0; // open_dashboard 被调用次数
let n_open_drew = 0; // 其中"真的插入成功"的次数
let bound = false; // 键位是否已绑(精确解绑用 ✓)

// ── 小工具 ────────────────────────────────────────────────────────────────
function guard(fn, fallback) {
  try {
    return fn();
  } catch (e) {
    return fallback;
  }
}
function try_command(name) {
  // 失败必须**可见** —— 静默的 guard 让"命令不存在"变成了"按了没反应" ✗(用户实测过)
  try {
    helix.run_command(name);
  } catch (e) {
    helix.echo("dashboard: 命令 " + name + " 不可用 —— " + (e && e.message ? e.message : e));
  }
}

/// picker 是 **API 不是命令** ✗(`:picker` 不存在)✓ 见 docs/api/picker-theme.md
function picker_run(source) {
  try {
    if (!helix.picker || typeof helix.picker.run !== "function") {
      throw new Error("helix.picker.run 不可用");
    }
    helix.picker.run(source);
  } catch (e) {
    helix.echo("dashboard: 打开 " + source + " 失败 —— " + (e && e.message ? e.message : e));
  }
}
function guess_config() {
  return guard(() => helix.config_dir() + "/init.js", "~/.config/helix/init.js");
}
function stamp() {
  return guard(() => new Date().toISOString().slice(0, 16).replace("T", " "), "");
}
function width() {
  // 视口列数来自核心新增的 `helix.viewport() -> [width, height]` ✓
  // (此前**没有任何接口**能读到 ✗ —— `helix.width`/`rows` 都是 undefined,探针实测过 ✓)
  const v = guard(() => helix.viewport(), null);
  if (v && typeof v[0] === "number" && v[0] > 0) return v[0];
  return 0; // 读不到(如测试夹具无真实终端)⇒ 返回 0 ⇒ pad() 用固定缩进,不假装居中 ✓
}

/// **显示宽度**(不是码点数):中文/全角在屏上占 **2 列** ✓
/// 不加这个,菜单缩进就会参差 —— E2E 实测过 ✗(logo 那几行因为全是块元素/制表符=1列,
/// 反而看起来很整齐,只有中文行偏 ✗ ⇒ 正好印证:块元素按 1 列、**只有 CJK 按 2 列** ✓)
function disp_width(s) {
  let n = 0;
  for (const ch of s) {
    const c = ch.codePointAt(0);
    const wide =
      (c >= 0x1100 && c <= 0x115f) || // 韩文字母
      (c >= 0x2e80 && c <= 0xa4cf) || // CJK 部首 / 假名 / 表意文字
      (c >= 0xac00 && c <= 0xd7a3) || // 韩文音节
      (c >= 0xf900 && c <= 0xfaff) || // CJK 兼容表意
      (c >= 0xfe30 && c <= 0xfe6f) || // CJK 兼容形式
      (c >= 0xff00 && c <= 0xff60) || // 全角标点
      (c >= 0xffe0 && c <= 0xffe6) || // 全角符号
      (c >= 0x1f300 && c <= 0x1f64f); // 常用 emoji
    n += wide ? 2 : 1;
  }
  return n;
}

// ── 菜单 ──────────────────────────────────────────────────────────────────
// 动作全部走 `guard`,失败静默 —— 启动屏不该因为某个动作不可用而报错 ✓
const MENU = [
  ["f", "查找文件", () => picker_run("files")],
  ["r", "打开 buffer", () => picker_run("buffers")],
  ["n", "新建文件", () => guard(() => helix.open_file("/tmp/hx-new.txt", { scratch: true }))],
  ["e", "文件树", () => try_command("filetree")],
  ["c", "配置", () => guard(() => helix.open_file(guess_config(), {}))],
  ["m", "市场", () => try_command("arsenal")],
  ["q", "关闭", () => guard(() => helix.run_command("dashboard-close"))],
];

// ── 内容 ──────────────────────────────────────────────────────────────────
function build_text() {
  const cfg = guard(() => helix.get_config("dashboard"), {}) || {};
  const w = width();
  // 宽度已知 ⇒ 按**显示宽度**居中;未知(w==0)⇒ 固定缩进 4 列(不假装居中 ✓)
  const pad = (s) =>
    w > 0 ? " ".repeat(Math.max(0, Math.floor((w - disp_width(s)) / 2))) + s : "    " + s;
  const lines = [""];
  if (cfg.logo !== "none") {
    for (const l of cfg.logo === "slim" ? LOGO_SLIM : LOGO_BLOCKS) lines.push(pad(l));
  }
  lines.push("");
  // **整体居中**:先求最宽项,再统一左边距 —— 逐行居中会让键位参差 ✗(第一版就这样)
  const label = (it) => "  " + it[0] + "   " + it[1];
  const item_w = Math.max.apply(
    null,
    MENU.map((it) => disp_width(label(it)))
  );
  const left = " ".repeat(Math.max(0, Math.floor((w - item_w) / 2)));
  for (const it of MENU) lines.push(left + label(it));
  lines.push("");
  lines.push(pad("────────────────────────────────"));
  lines.push(pad("  " + (stamp() ? "启动于 " + stamp() : "欢迎") + " · 按 q 关闭"));
  lines.push("");
  return lines.join("\n");
}

// ── 绑 / 解绑 ─────────────────────────────────────────────────────────────
function unbind_keys() {
  for (const item of MENU) {
    guard(() => helix.map("normal", item[0], () => {}, { unbind: true }));
  }
  bound = false;
}

function bind_keys() {
  if (bound) return;
  let ok = 0;
  for (const item of MENU) {
    if (guard(() => (helix.map("normal", item[0], item[2]), true), false)) ok++;
  }
  if (ok !== MENU.length) {
    // 只绑上一部分 ⇒ 立刻全部解绑,保持"要么全绑、要么不绑" ✓
    unbind_keys();
    return;
  }
  bound = true;
}

// ── 开屏 / 关屏 ───────────────────────────────────────────────────────────
function close_dashboard(ctx) {
  unbind_keys();
  dash_open = false;
  // ★ 探针实测:**没有** `helix.close` / `buffer_close` / `close_buffer` ✗
  // ⇒ 用**已证实**的 `ctx.doc.delete` 把启动屏**清空**(留下的就是启动时那个空 buffer ✓ = 离开 ✓)
  if (ctx && ctx.doc && typeof ctx.doc.delete === "function") {
    try {
      const rows = String(ctx.doc.text || "").split("\n").length;
      ctx.doc.delete(0, 0, rows, 0);
      guard(() => helix.echo("dashboard: 已关闭启动屏"));
      return;
    } catch (e) {
      helix.echo("dashboard: 清屏失败 —— " + (e && e.message ? e.message : e));
    }
  }
  guard(() => helix.echo("dashboard: 键位已解除(未拿到 ctx.doc,屏内容保留)"));
  // **真的离开**:尽量关掉这张 buffer(dashboard 就是启动时那个空 buffer ✓)
  // 关不掉也**不静默** —— 至少把状态说清 ✓(上次"按 q 没反应"就是因为它什么都不做 ✗)
  // 真名是 `helix.close` ✓(注册清单里有 `close` 与 `close_panel`;后者是面板 ✗)
  let closed = false;
  for (const fn of ["close", "buffer_close", "close_buffer"]) {
    if (typeof helix[fn] !== "function") continue;
    try {
      if (fn === "close") {
        try { helix.close(dash_buffer_id); } catch (e) { helix.close(); }
      } else {
        helix[fn](dash_buffer_id);
      }
      closed = true;
      break;
    } catch (e) {
      helix.echo("dashboard: " + fn + " 失败 —— " + (e && e.message ? e.message : e));
    }
  }
  guard(() => helix.echo(closed ? "dashboard: 已关闭启动屏" : "dashboard: 已解除键位,但没找到可用的关闭接口(请报告)"));
  dash_buffer_id = null;
}

/// 任何 buffer 活动都视为"用户离开了启动屏" ⇒ 立刻解绑
/// **这一条是"按 e 卡死"的正解** ✓:重入的前提是那 7 个键还在 ✗
function dismiss_on_others() {
  if (!dash_open) return;
  unbind_keys();
  dash_open = false;
  // **可见**(便于验证与排错):证明"离开即解绑"真的跑了 ✓
  guard(() => helix.echo("dashboard: 已离开启动屏,键位已解除"));
}

function open_dashboard(ctx) {
  n_open_calls++;
  if (dash_open) return;

  // **安全闸(必需)**:若当前 buffer **有路径**(真实文件),绝不把启动屏写进去 ✗
  // E2E 实测过这个后果:在打开的 a.txt 上跑 `:dashboard`,内容被混进那个文件 ✗
  // 判定改用**已证实**的 `ctx.doc.text`(探针实测:`buffer.current().path` 是 undefined ✗ 不可用 ✓)
  // 规则:**只在空白 buffer 上绘制** —— 等价于 LazyVim("空白无名 buffer 才显示")且更安全 ✓
  const text = ctx && ctx.doc ? String(ctx.doc.text || "") : null;
  if (text === null) {
    guard(() => helix.echo("dashboard: 拿不到 ctx.doc(需经命令路径调用)"));
    return;
  }
  if (text.trim() !== "") {
    guard(() => helix.echo("dashboard: 当前 buffer 非空,不在此绘制启动屏"));
    return;
  }
  // **不新开 buffer** —— 启动屏就是"启动时那个空白无名 buffer 本身" ✓(LazyVim 也是这个模型)
  // 先前新开一张的写法有个隐蔽 bug:`ctx.doc` 是在处理器运行**之前**构造的 ✗
  // ⇒ 它指向**开屏前**的 buffer ⇒ 内容写进了旧的,新 buffer 仍是空的 ✗(E2E 实得 LEN=1)
  dash_open = true;

  // **内容写入必须报错可见** —— 先前这里包了 guard(),于是"内容没写进去"被**静默吞掉** ✗
  // (E2E 第一次运行就发现:buffer 开对了,但文本只有 "\n")⇒ 改成显式 try/catch + echo ✓
  // ★★ 关键顺序修正(真机四连测得出 ✓):
  // 插入本身没问题(连 D-REAL 都显示 ✓);出错的是**插入之后**发的请求
  // (set_cursor 与 7 次 helix.map)⇒ 它们似乎会冲掉刚排队的编辑 ✗
  // ⇒ 先把键绑好、把光标定位好,**最后**再插入 ✓(插入后不再发任何请求 ✓)
  bind_keys();
  guard(() => helix.set_cursor(0, 0)); // 签名已读实现确认:set_cursor(row, col) ✓

  try {
    const body = build_text();
    // **编辑方法在 doc 对象上**(不在 helix 全局)✗ —— 诊断实得 "not a callable function" ✓
    // 优先用当前 doc 的 insert;再退回全局(两级都试,任一成功即可)✓
    // **编辑方法在 `ctx.doc` 上**(不在 helix 全局)✓ —— 见 docs/api/editing.md
    const doc = ctx && ctx.doc ? ctx.doc : null;
    if (doc && typeof doc.insert === "function") {
      // ★★ 关键:编辑是**排队**的 —— `end_edit()` 才让积压编辑被 take_edits 取走并应用 ✓
      // (实现注释:"声明批量编辑事务结束:积压编辑可被 take_edits 取走" ✓)
      // 我的插件此前**从未调用** begin_edit/end_edit ✗ ⇒ 插入很可能一直躺在队列里没生效 ✓
      // —— 这正好解释"回显成功 ✓ 而 buffer 里只有 1 字符 ✗"
      const has_batch = typeof helix.begin_edit === "function" && typeof helix.end_edit === "function";
      if (has_batch) helix.begin_edit();
      try {
        doc.insert(0, 0, body);
      } finally {
        if (has_batch) helix.end_edit();
      }
      // 【已移走】set_cursor 挪到插入之前 —— 见下方说明 ✓
      // 真机四连测已证:插入本身完全正常(连 D-REAL 都显示 ✓)
      // ⇒ 罪魁是**插入之后**发出的请求(set_cursor / 7 次 helix.map)✗
      // ⇒ 顺序改为:**先绑键、先定位,最后插入** ✓(插入后不再有任何请求扰动它 ✓)
      // —— 那三个接口的形状**我从未验证** ✗,而真机上"主区域不显示内容"的症状
      //    出现在加了它之后 ⇒ 按纪律**回退未经证实的猜测**,只保留已验证的 insert ✓
      // (真实编辑器里的视图落点仍待用探针确认 —— 见 :dashboard-diag ✓)
    } else {
      throw new Error(
        "拿不到带编辑方法的 ctx.doc(命令/事件处理器必须接收并使用其 ctx 参数)"
      );
    }
    if (helix.read_lines === undefined) {
      // no-op:保留一个能力探测点,便于日后替换成"读回校验" ✓
    }
  } catch (e) {
    helix.echo("dashboard: 写入内容失败 —— " + (e && e.message ? e.message : e));
  }
  // 记下自己所在的 buffer(退出时要用 ✓)
  dash_buffer_id = guard(() => helix.buffer.current().id, null);
  guard(() => helix.echo("HELIX 启动屏 —— f 查找 · r buffer · e 文件树 · q 关闭"));
}

// ── 显式命令(永远可用,不依赖 startup ✓)──────────────────────────────────
helix.register_command("dashboard", (ctx) => open_dashboard(ctx));

// **离开即解绑**(最高优先的修复 ✓):切 buffer / 开新 buffer ⇒ 那 7 个全局键立即失效 ✓
// ⇒ 从根上消掉"在 filetree 里按键又触发 dashboard ⇒ 重入卡死" ✗
helix.on("buffer-open", () => {
  n_buffer_open++;
  if (dash_open) {
    // 已经在屏上 ⇒ 这是"用户切走了" ⇒ 解绑 ✓(这条消掉了"按 e 卡死" ✓)
    dismiss_on_others();
    return;
  }
  // ★ 真机实测:`startup` 期插入的内容会被**后续换 buffer 冲掉** ✗
  // (状态栏留着回显 ✓ 而主区域空 ✗;之后查 doc 只有 1 字符 ✓)
  // ⇒ 在 `buffer-open`(启动定型之后)再补画一次;闸门会保证只画空白 buffer ✓
  guard(() => helix.run_command("dashboard"));
});
helix.on("buffer-close", () => dismiss_on_others());
helix.register_command("dashboard-close", (ctx) => close_dashboard(ctx));

/// **最小探针**:真机上"JS 发起的编辑到底会不会被应用" ✓
/// 只在**空白 buffer** 上试(绝不碰你的文件 ✓);插入固定串后立刻回报长度 ✓
/// 用法:`:dashboard-probe` ⇒ 看状态栏 + 看主区域是否出现 `PROBE-OK` ✓
helix.register_command("dashboard-probe", (ctx) => {
  const doc = ctx && ctx.doc ? ctx.doc : null;
  if (!doc || typeof doc.insert !== "function") {
    guard(() => helix.echo("dashboard-probe: 没有 ctx.doc.insert ⇒ 命令路径本身有问题 ✗"));
    return;
  }
  const before = String(doc.text || "");
  if (before.trim() !== "") {
    guard(() => helix.echo("dashboard-probe: 当前 buffer 非空,请在**空 buffer** 上试(避免改到文件)"));
    return;
  }
  // **三连测**:分别插入三种形态,各带独立标记 ✓(都在同一个空 buffer 上 ✓)
  // 目的:一次问出"到底是哪一种属性让 dashboard 的插入失效" ✗
  const cases = [
    ["A-SHORT", "A-SHORT"],                                  // ① 短串(已知会成功 ✓)
    ["B-MULTI", "B-MULTI\nB-LINE2"],                         // ② 多行
    ["C-LONG", "C-LONG\n" + "x".repeat(700)],                 // ③ 长串(≈dashboard 的量级)
    ["D-REAL", build_text()],                                 // ④ dashboard 真正要插的那段
  ];
  try {
    if (typeof helix.begin_edit === "function") helix.begin_edit();
    for (const [, s] of cases) doc.insert(0, 0, s + "\n");
    if (typeof helix.end_edit === "function") helix.end_edit();
    n_open_drew++; // 真的插入成功(诊断计数 ✓)
    guard(() => helix.echo("dashboard-probe: 已依次插入 A-SHORT / B-MULTI / C-LONG / D-REAL ⇒ 看主区域出现了哪几个"));
  } catch (e) {
    guard(() => helix.echo("dashboard-probe: insert 抛错 —— " + (e && e.message ? e.message : e)));
  }
});

/// 自诊断:把"居中/定位"依赖的两个事实打出来 ✓
/// 用法:`:dashboard-diag` —— 若 `vw=0` 说明核心没把视口尺寸送到 JS ✗;
/// 若 `vbuf=...` 的行号不是 0,说明光标没回到行首 ⇒ 视图会停在末尾 ✓
helix.register_command("dashboard-diag", (ctx) => {
  const v = guard(() => helix.viewport(), null);
  const doc = ctx && ctx.doc ? ctx.doc : null;
  const cur = doc ? String(doc.cursor || JSON.stringify(doc.cursor)) : "no-ctx";
  const len = doc ? String(doc.text || "").length : -1;
  // ★ 关键新信息:`_target`(ctx.doc 指向的 buffer/文件)与当前 buffer 是否同一个 ✓
  // 若两者不同 ⇒ "内容插进了另一个 buffer" ⇒ 主区域当然不显示 ✗
  const tgt = doc ? String(doc._target === undefined ? "undefined" : doc._target) : "no-ctx";
  const curBuf = guard(() => {
    const b = helix.buffer.current();
    return b ? JSON.stringify({ id: b.id, path: b.path, name: b.name }) : "null";
  }, "err");
  guard(() =>
    helix.echo(
      "startup=" + n_startup + " bufOpen=" + n_buffer_open +
      " openCalls=" + n_open_calls + " drew=" + n_open_drew +
      " dashOpen=" + (dash_open ? 1 : 0) +
      " | vw=" + (v ? v[0] : "n/a") + " vh=" + (v ? v[1] : "n/a") +
      " len=" + len + " target=" + tgt + " currentBuffer=" + curBuf
    )
  );
});

// ── 启动自动显示(仅"无文件启动")──────────────────────────────────────────
// 判定:当前 buffer **没有路径** ⇒ 视为无文件启动 ✓
// 判定不出/形状不同 ⇒ **不显示**(安全降级 ✓,绝不打扰正常编辑)
helix.on("startup", (arg) => {
  n_startup++;
  // 事件处理器的参数形状与事件相关(如 layout-change 带 kind)✗ ⇒ 兼容多种形状取 ctx ✓
  const ctx = arg && arg.doc ? arg : arg && arg.ctx ? arg.ctx : null;
  const cfg = guard(() => helix.get_config("dashboard"), {}) || {};
  if (cfg.enabled === false) return;
  // ★ 关键:事件处理器**拿不到可编辑的 ctx** ✗ ⇒ 改为**触发自己的命令**(命令路径能拿到 ctx ✓)
  // (这条正是"手动输入命令就能用"所证明的 ✓)
  guard(() => helix.run_command("dashboard"));
});
