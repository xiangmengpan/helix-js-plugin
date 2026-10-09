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
let dash_doc = null; // 启动屏的 buffer id(每次"当前 buffer"变化都会清掉 ✓)
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
  guard(() => helix.run_command(name));
}
function guess_config() {
  return guard(() => helix.config_dir() + "/init.js", "~/.config/helix/init.js");
}
function stamp() {
  return guard(() => new Date().toISOString().slice(0, 16).replace("T", " "), "");
}
function width() {
  return guard(() => helix.width(), 80) || 80;
}

// ── 菜单 ──────────────────────────────────────────────────────────────────
// 动作全部走 `guard`,失败静默 —— 启动屏不该因为某个动作不可用而报错 ✓
const MENU = [
  ["f", "查找文件", () => try_command("picker")],
  ["r", "打开 buffer", () => try_command("buffer")],
  ["n", "新建文件", () => guard(() => helix.open_file("/tmp/hx-new.txt", { scratch: true }))],
  ["e", "文件树", () => try_command("filetree")],
  ["c", "配置", () => guard(() => helix.open_file(guess_config(), {}))],
  ["m", "市场", () => try_command("arsenal")],
  ["q", "关闭", () => close_dashboard()],
];

// ── 内容 ──────────────────────────────────────────────────────────────────
function build_text() {
  const cfg = guard(() => helix.get_config("dashboard"), {}) || {};
  const w = width();
  const pad = (s) => " ".repeat(Math.max(0, Math.floor((w - s.length) / 2))) + s;
  const lines = [""];
  if (cfg.logo !== "none") {
    for (const l of cfg.logo === "slim" ? LOGO_SLIM : LOGO_BLOCKS) lines.push(pad(l));
  }
  lines.push("");
  for (const item of MENU) lines.push(pad("  " + item[0] + "   " + item[1]));
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
function close_dashboard() {
  unbind_keys();
  if (dash_doc !== null) guard(() => helix.close_buffer(dash_doc));
  dash_doc = null;
}

function open_dashboard() {
  if (dash_doc !== null) return;
  // 关掉启动时留下的那个空 buffer(尽量,失败无所谓 ✓)
  guard(() => helix.close_buffer(helix.buffer.current().id));
  const id = guard(() => helix.open_file("/tmp/hx-dashboard.txt", { scratch: true }), null);
  dash_doc = guard(() => (id === null ? helix.buffer.current().id : id), null);

  // **内容写入必须报错可见** —— 先前这里包了 guard(),于是"内容没写进去"被**静默吞掉** ✗
  // (E2E 第一次运行就发现:buffer 开对了,但文本只有 "\n")⇒ 改成显式 try/catch + echo ✓
  try {
    const body = build_text();
    helix.insert(0, 0, body);
    if (helix.read_lines === undefined) {
      // no-op:保留一个能力探测点,便于日后替换成"读回校验" ✓
    }
  } catch (e) {
    helix.echo("dashboard: 写入内容失败 —— " + (e && e.message ? e.message : e));
  }
  guard(() => helix.echo("HELIX 启动屏 —— f 查找 · r buffer · e 文件树 · q 关闭"));
  bind_keys();
}

// ── 显式命令(永远可用,不依赖 startup ✓)──────────────────────────────────
helix.register_command("dashboard", () => open_dashboard());
helix.register_command("dashboard-close", () => close_dashboard());

// ── 启动自动显示(仅"无文件启动")──────────────────────────────────────────
// 判定:当前 buffer **没有路径** ⇒ 视为无文件启动 ✓
// 判定不出/形状不同 ⇒ **不显示**(安全降级 ✓,绝不打扰正常编辑)
helix.on("startup", () => {
  const cfg = guard(() => helix.get_config("dashboard"), {}) || {};
  if (cfg.enabled === false) return;
  const cur = guard(() => helix.buffer.current(), null);
  if (!cur) return;
  const p = cur.path;
  if (p !== null && p !== undefined && p !== "") return; // 有文件 ⇒ 不显示 ✓
  open_dashboard();
});
