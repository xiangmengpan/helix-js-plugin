// statusline.js — 状态栏美化（lazyvim 风格三段分栏）
// 依赖清单：图标已收进核心,不再需要 deps;git 分支名走 `run_async` ⇒ 起进程
helix.plugin("statusline", {
  deps: [],
  // 2.0:配置键改成 left/center/right/zones/corners(旧的 show_git_branch/show_diagnostics 已删,
  //      老配置会以"未在 define_config 中声明(已忽略)"报出来 —— 那就是迁移提示)。
  version: "2.0",
  // 清单**声明面**版本(比的是本编辑器支持的 manifest 语义,现在 = 1),不是本插件配置的版本。
  schema_version: 1,
  permissions: { process: true },
});

// 依赖：icons（统一图标映射,已进核心 `helix.icons.*`）；set_statusline replace 模式 + zones 比例。
// 布局：left / center / right 三个**段名列表**（config.toml 可配）+ 圆角风格。
//   · 每个区按列表顺序渲染,段间自动 1 空格 gap（Rust 端插）⇒ **空段必须丢**,否则双空格
//   · zones = [左,中,右] 宽度比例；它是**注册态**(Rust `statusline_zones` 只在 set_statusline
//     时写入)⇒ 改比例要 `:plugin-reload`。其余键每帧读,`:config-reload` 即时生效。

// ============================ 配置 ============================

// 默认布局 = 2.0 之前的观感（长串拆成细粒度段,顺序不变）
const DEFAULT_LEFT = ["window", "mode", "file", "git"];
const DEFAULT_CENTER = ["diagnostics"];
const DEFAULT_RIGHT = [
  "position",
  "percent",
  "modified",
  "selections",
  "readonly",
  "encoding",
  "line-ending",
  "register",
  "chars",
  "indent",
  "filetype",
  "cwd",
  "code-action",
  "workspace-diag",
  "spinner",
  "lines",
];

helix.define_config("statusline", {
  left: {
    type: "array",
    items: "string",
    default: DEFAULT_LEFT,
    doc: "左区段名列表(顺序即显示顺序);可用段见 docs 或 helix.get_config_docs",
  },
  center: { type: "array", items: "string", default: DEFAULT_CENTER, doc: "中区段名列表(居中)" },
  right: { type: "array", items: "string", default: DEFAULT_RIGHT, doc: "右区段名列表(右对齐贴最右)" },
  zones: {
    type: "array",
    items: "number",
    default: [1, 3, 2],
    doc: "左/中/右 宽度比例;**注册时读取**,改后需 :plugin-reload(其余键即时生效)",
  },
  corners: {
    type: "enum",
    default: "line",
    options: ["off", "line", "powerline"],
    doc: "圆角:off 无 | line 整条两端 ╭╮ | powerline mode 色块两侧圆角端帽(需 Nerd Font)",
  },
});

// cfg 每次渲染读取(config-reload 后即时生效)
function current_cfg() {
  return helix.get_config("statusline") || {};
}

// ============================ 状态 ============================

let gitBranch = ""; // 当前 git 分支（异步查询缓存）

// 能力清单查询(`helix.permissions()`):**不问就放行吗**。
// 没声明 `permissions` 的插件(legacy)与内置插件恒为 true;声明了但没要 `process`
// (或用户 deny)时为 false ⇒ 这里**降级为"无分支"**,而不是 spawn 一次再收失败。
// try/catch 兼作老编辑器兼容:没有这个 API 时按"可用"处理。
function hasProcess() {
  try {
    return !!(helix.permissions && helix.permissions().process);
  } catch (e) {
    return true;
  }
}

async function refreshGit(path) {
  if (!path) {
    gitBranch = "";
    return;
  }
  if (!hasProcess()) {
    gitBranch = "";
    return;
  }
  try {
    const out = await helix.run_async("git -C " + shq(dirname(path)) + " branch --show-current");
    gitBranch = (out || "").trim();
  } catch (e) {
    gitBranch = "";
  }
}

function dirname(p) {
  const i = p.lastIndexOf("/");
  return i > 0 ? p.slice(0, i) : ".";
}

function shq(s) {
  return "'" + String(s).replace(/'/g, "'\\''") + "'";
}

// ============================ 段注册表 ============================
// 每个段：`(ctx) -> {text, style} | [parts] | null`。返回 null / 空 text ⇒ 本帧不显示。
// 带 `cap` 字段的段在 corners = "powerline" 时两侧加圆角端帽（目前只有 mode）。

const CAP = "\ue0b6"; // 左圆角端帽(U+E0B6)
const CAP_END = "\ue0b4"; // 右圆角端帽(U+E0B4)

const SEGMENTS = {
  // 窗口模式指示(平级窗口/面板态)：图标跟当前窗口类型
  window: (ctx) => {
    if (!ctx.window_mode) return null;
    let winIcon = helix.icons.file(ctx.active_leaf_path || "");
    if (ctx.active_leaf_type === "terminal") winIcon = helix.icons.file("term.sh");
    else if (ctx.active_leaf_type === "panel") winIcon = helix.icons.dir(false);
    return { text: " " + winIcon + " ", style: "ui.statusline.insert" };
  },
  // mode 色块：关掉图标时**不能**变空,用模式首字母回退(保留信息)
  mode: (ctx) => {
    const mode = ctx.mode || "normal";
    const icon = helix.icons.mode(mode) || mode.charAt(0).toUpperCase();
    return { text: " " + icon + " ", style: "ui.statusline." + mode, cap: mode };
  },
  file: (ctx) => {
    const name = ctx.path ? ctx.path.split("/").pop() : "[scratch]";
    return { text: helix.icons.file(ctx.path || "") + " " + name, style: null };
  },
  git: () => (gitBranch ? { text: "\uf1d3 " + gitBranch, style: "ui.virtual" } : null),
  // 诊断计数(error 红 / warning 黄;0 不显示):一个段可以吐多块
  diagnostics: (ctx) => {
    const out = [];
    if ((ctx.diagnostics_error || 0) > 0) {
      out.push({ text: (helix.icons.diagnostic("error") || "✗") + " " + ctx.diagnostics_error, style: "error" });
    }
    if ((ctx.diagnostics_warning || 0) > 0) {
      out.push({ text: (helix.icons.diagnostic("warning") || "!") + " " + ctx.diagnostics_warning, style: "warning" });
    }
    return out;
  },

  position: (ctx) => {
    const cur = ctx.cursor || {};
    const row = (cur.row || 0) + 1;
    const col = (cur.col || 0) + 1;
    return { text: row + ":" + col, style: null };
  },
  percent: (ctx) => {
    const row = ((ctx.cursor || {}).row || 0) + 1;
    const total = ctx.total_lines || 1;
    return { text: Math.round((row / total) * 100) + "%", style: "ui.virtual" };
  },
  // 修改标记:语义对齐核心 `FileModificationIndicator`(`[+]` / 未改时无)
  modified: (ctx) => (ctx.modified ? { text: "[+]", style: "ui.virtual" } : null),
  // 选区数:对齐核心 `render_selections`(1 个时不显示;多个显示 `主序/总数 sels`)
  selections: (ctx) =>
    ctx.selections > 1
      ? { text: ctx.selections_primary + 1 + "/" + ctx.selections + " sels", style: "ui.virtual" }
      : null,
  readonly: (ctx) => (ctx.read_only ? { text: "[readonly]", style: "ui.virtual" } : null),
  encoding: (ctx) =>
    ctx.encoding && ctx.encoding !== "utf-8" ? { text: ctx.encoding, style: "ui.virtual" } : null,
  // 行尾符:核心**总是**显示 `LF`;JS 布局里 LF 是常态 ⇒ 仅非 LF 时显示(刻意的布局取舍)
  "line-ending": (ctx) =>
    ctx.line_ending && ctx.line_ending !== "LF" ? { text: ctx.line_ending, style: "ui.virtual" } : null,
  register: (ctx) => (ctx.register ? { text: "reg=" + ctx.register, style: "ui.virtual" } : null),
  // 主选区长度:对齐核心 `N char(s)`(总是显示)
  chars: (ctx) => {
    const n = ctx.primary_selection_length || 0;
    return { text: n + " char" + (n === 1 ? "" : "s"), style: "ui.virtual" };
  },
  indent: (ctx) => (ctx.indent_style ? { text: ctx.indent_style, style: "ui.virtual" } : null),
  filetype: (ctx) => ({ text: ctx.file_type || "text", style: "ui.virtual" }),
  cwd: (ctx) => (ctx.cwd ? { text: ctx.cwd, style: "ui.virtual" } : null),
  "code-action": (ctx) => (ctx.code_action_hint ? { text: "⋮", style: "ui.virtual" } : null),
  // 工作区诊断:核心折叠成 4 个计数;这里**只列非零项**,字母 E/W/I/H
  "workspace-diag": (ctx) => {
    const ws = [
      ["E", ctx.workspace_diagnostics_error],
      ["W", ctx.workspace_diagnostics_warning],
      ["I", ctx.workspace_diagnostics_info],
      ["H", ctx.workspace_diagnostics_hint],
    ]
      .filter(([, n]) => n > 0)
      .map(([k, n]) => k + n)
      .join(" ");
    return ws ? { text: ws, style: "ui.virtual" } : null;
  },
  spinner: (ctx) => (ctx.spinner ? { text: ctx.spinner, style: "ui.virtual" } : null),
  lines: (ctx) => ({ text: String(ctx.total_lines || 1), style: "ui.virtual" }),
};

// ============================ 圆角 ============================
// powerline 端帽要 fg = 该段的**底色**、bg = 状态栏底色 —— 而 `StatuslinePart.style` 只
// 接受**主题 scope 名**(不能内联颜色) ⇒ 派生一个 scope 交给 `set_theme` 注册。
// 拿不到颜色(主题没给 bg / 无 theme 源)就**退化为无帽**,不报错。
const capScopes = {}; // mode -> scope 名 | null(本主题下没有可用的派生 scope)

function bgOf(scope) {
  try {
    const st = helix.get_style(scope);
    return st && typeof st === "object" && st.bg ? st.bg : null;
  } catch (e) {
    return null;
  }
}

function capScope(mode) {
  if (Object.prototype.hasOwnProperty.call(capScopes, mode)) return capScopes[mode];
  let scope = null;
  const fg = bgOf("ui.statusline." + mode);
  if (fg) {
    const name = "statusline.cap." + mode;
    try {
      helix.set_theme({ [name]: { fg: fg, bg: bgOf("ui.statusline") } });
      scope = name;
    } catch (e) {
      scope = null;
    }
  }
  capScopes[mode] = scope;
  return scope;
}

function pushCapped(out, part) {
  const scope = part.cap ? capScope(part.cap) : null;
  if (scope) out.push({ text: CAP, style: scope });
  out.push({ text: part.text, style: part.style || null });
  if (scope) out.push({ text: CAP_END, style: scope });
}

// ============================ 状态栏渲染 ============================

function pushSegment(out, name, ctx, corners) {
  // 内置段**优先**（外部插件注册同名段无效 —— 核心不做名字空间隔离，这条是渲染方的决定）。
  // 非内置段走 `helix.statusline_segment`：
  //   · 名字**未注册** ⇒ 它抛错（这里接住 ⇒ 红字 `!名字`，让拼错可见）
  //   · 段函数**自己抛错** ⇒ 它返回 null（只坏这一格，不拖掉整条）
  let seg = SEGMENTS[name];
  let external = false;
  if (!seg) {
    seg = (c) => helix.statusline_segment(name, c);
    external = true;
  }
  let got;
  try {
    got = seg(ctx);
  } catch (e) {
    out.push({ text: "!" + name, style: "error" });
    return;
  }
  // 外部段用字符串返回也说得通（与钩子同一套：字符串 = 单段）
  if (external && typeof got === "string") got = { text: got, style: null };
  if (!got) return;
  for (const part of Array.isArray(got) ? got : [got]) {
    // 空段必须丢: Rust 端会为**每一段**前插 1 空格 gap,空段会吐双空格
    if (!part || !part.text) continue;
    if (corners === "powerline" && part.cap) pushCapped(out, part);
    else out.push({ text: part.text, style: part.style || null });
  }
}

function pushZone(out, names, fallback, ctx, corners) {
  const list = Array.isArray(names) ? names : fallback;
  for (const raw of list) pushSegment(out, String(raw), ctx, corners);
}

function render(ctx) {
  ctx = ctx || {};
  const cfg = current_cfg();
  const corners = cfg.corners || "line";

  // ── 三个区各自成串(zone 字段由渲染顺序决定) ──
  const left = [];
  const center = [];
  const right = [];
  pushZone(left, cfg.left, DEFAULT_LEFT, ctx, corners);
  pushZone(center, cfg.center, DEFAULT_CENTER, ctx, corners);
  pushZone(right, cfg.right, DEFAULT_RIGHT, ctx, corners);

  // ── 圆角 `line`:整条两端 ╭ / ╮ ──
  // 属于"条"不属于"段":即使某个区的列表被清空,两端仍画。
  // 位置是**算得准的**:left 区从第 0 列起,right 区右对齐贴最右 ⇒ 不需要视口宽度。
  if (corners === "line") {
    left.unshift({ text: "╭", style: null });
    right.push({ text: "╮", style: null });
  }

  const parts = [];
  for (const p of left) parts.push({ text: p.text, style: p.style || null });
  for (const p of center) parts.push({ text: p.text, style: p.style || null, zone: "center" });
  for (const p of right) parts.push({ text: p.text, style: p.style || null, zone: "right" });
  return parts;
}

// ============================ 注册 ============================

// zones 是注册态 ⇒ 这里读一次(config 缺失/非法就回默认 [1,3,2])
function zoneWeights() {
  const z = current_cfg().zones;
  if (Array.isArray(z) && z.length === 3 && z.every((n) => Number(n) > 0)) return z.map(Number);
  return [1, 3, 2];
}

if (typeof helix !== "undefined") {
  helix.set_statusline(render, { replace: true, zones: zoneWeights() });
  helix.on("buffer-open", (doc) => {
    refreshGit(doc && doc.path);
  });
  // 换主题 ⇒ 派生端帽 scope 的底色失效,丢掉缓存(下次渲染按新主题重算并覆盖)
  helix.on("theme-change", () => {
    for (const k in capScopes) delete capScopes[k];
  });
}
