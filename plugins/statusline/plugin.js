// statusline.js — 状态栏美化（lazyvim 风格三段分栏）
// 依赖清单：icons（统一图标映射，自动先加载）
helix.plugin("statusline", { deps: [] }); // 图标已收进核心,不再需要 deps

// 插件配置(方案 C):config.toml [plugins.statusline]
helix.define_config("statusline", {
  show_git_branch: { type: "boolean", default: true, doc: "显示 git 分支" },
  show_diagnostics: { type: "boolean", default: true, doc: "显示诊断计数" },
});
// cfg 每次渲染读取(config-reload 后即时生效)
function current_cfg() {
  return helix.get_config("statusline") || {};
}
// 依赖：icons.js（统一图标映射）；set_statusline replace 模式 + zones 比例（Rust 增强）。
// 布局：左区（mode 色块 + 文件名 + git）· 中区（诊断，居中）· 右区（位置/百分比/行数，右对齐贴最右）。
// 比例在注册处 zones: [左, 中, 右]；段间自动 1 空格 gap。

// ============================ 状态 ============================

// 图标已收进核心:直接用 `helix.icons.*`(原先的懒加载 + 逐处 `ICONS ? … : 回退` 已删)
let gitBranch = "";      // 当前 git 分支（异步查询缓存）

async function refreshGit(path) {
  if (!path) { gitBranch = ""; return; }
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

function shq(s) { return "'" + String(s).replace(/'/g, "'\\''") + "'"; }

// ============================ 状态栏渲染 ============================

// 三段分栏：zone 默认 left；诊断走 center；位置/百分比/行数走 right。
function render(ctx) {
  ctx = ctx || {};
  const mode = ctx.mode || "normal";
  const parts = [];

  // ── 窗口模式指示：当前窗口图标 ──
  if (ctx.window_mode) {
    let winIcon = helix.icons.file(ctx.active_leaf_path || "");
    if (ctx.active_leaf_type === "terminal") winIcon = helix.icons.file("term.sh");
    else if (ctx.active_leaf_type === "panel") winIcon = helix.icons.dir(false);
    parts.push({ text: " " + winIcon + " ", style: "ui.statusline.insert" });
  }

  // ── 左区：mode 色块 + 文件名（类型图标）+ git 分支 ──
  // 关掉图标时**不能**变空:用模式首字母回退(保留信息,与旧行为一致)
  parts.push({ text: " " + (helix.icons.mode(mode) || mode.charAt(0).toUpperCase()) + " ", style: "ui.statusline." + mode });
  const name = ctx.path ? ctx.path.split("/").pop() : "[scratch]";
  const fileIcon = helix.icons.file(ctx.path || "");
  parts.push({ text: fileIcon + " " + name, style: null });
  const cfg = current_cfg();
  if (cfg.show_git_branch && gitBranch) {
    parts.push({ text: "\uf1d3 " + gitBranch, style: "ui.virtual" });
  }

  // ── 中区：诊断计数（error 红 / warning 黄；0 不显示）──
  if (cfg.show_diagnostics && (ctx.diagnostics_error || 0) > 0) {
    parts.push({ text: (helix.icons.diagnostic("error") || "✗") + " " + ctx.diagnostics_error, style: "error", zone: "center" });
  }
  if (cfg.show_diagnostics && (ctx.diagnostics_warning || 0) > 0) {
    parts.push({ text: (helix.icons.diagnostic("warning") || "!") + " " + ctx.diagnostics_warning, style: "warning", zone: "center" });
  }

  // ── 右区：位置 + 百分比 + 总行数（右对齐贴最右）──
  const cur = ctx.cursor || {};
  const row = (cur.row || 0) + 1;
  const col = (cur.col || 0) + 1;
  const total = ctx.total_lines || 1;
  // 修改标记:语义严格对齐核心的 `FileModificationIndicator`(`[+]` / 未改时无),
  // 由新增的 ctx 字段 `modified`(statusline.rs:79 填入 `doc.is_modified()`)驱动。
  // 放置位置是 JS 状态栏自己的布局选择(挂在右区位置段上)。
  const modMark = ctx.modified ? " [+]" : "";
  // 选区数:语义**对齐核心** `render_selections`(1 个时不显示;多个显示 `主序/总数 sels`)
  const selMark = ctx.selections > 1 ? " " + (ctx.selections_primary + 1) + "/" + ctx.selections + " sels" : "";
  // 只读 / 编码:语义对齐核心(`" [readonly] "` · 非 UTF-8 时才显示编码名)
  const roMark = ctx.read_only ? " [readonly]" : "";
  const encMark = ctx.encoding && ctx.encoding !== "utf-8" ? " " + ctx.encoding : "";
  // 行尾符:核心**总是**显示(`" LF "`);JS 布局里 LF 是常态,故**仅非 LF 时显示** ——
  // 这是一处**刻意的布局取舍**(同"放置位置"由 JS 布局决定),不是语义偏差:字符串本身与核心一致。
  const leMark = ctx.line_ending && ctx.line_ending !== "LF" ? " " + ctx.line_ending : "";
  // 寄存器:对齐核心 `" reg=X "`(未选中寄存器时不显示)
  const regMark = ctx.register ? " reg=" + ctx.register : "";
  // 主选区长度:对齐核心 `" N char(s) "`
  const n = ctx.primary_selection_length;
  const charMark = " " + n + " char" + (n === 1 ? "" : "s");
  // 缩进风格:直接用核心的原文案(`tabs` / `4 spaces`)
  const indMark = ctx.indent_style ? " " + ctx.indent_style : "";
  // 语言名:对齐核心的 `unwrap_or(DEFAULT_LANGUAGE_NAME)`(该常量的值实测为 `"text"`)
  const ftMark = " " + (ctx.file_type || "text");
  // 工作目录末段名(核心 `render_cwd` 就是 `current_working_dir().file_name()`)
  const cwdMark = ctx.cwd ? " " + ctx.cwd : "";
  // 代码动作提示:沿用核心的符号 `⋮`(`⋮` = U+22EE),仅在 focused 且有提示时出现
  const caMark = ctx.code_action_hint ? " ⋮" : "";
  // 工作区诊断:核心把它折叠成 4 个计数;这里**只列非零项**,字母 E/W/I/H。
  // (核心的具体排版我没读全 → 这是 JS 侧自己的排版选择,计数语义与核心一致)
  const ws = [["E", ctx.workspace_diagnostics_error], ["W", ctx.workspace_diagnostics_warning],
              ["I", ctx.workspace_diagnostics_info], ["H", ctx.workspace_diagnostics_hint]]
    .filter(([, n]) => n > 0).map(([k, n]) => k + n).join(" ");
  const wsMark = ws ? " " + ws : "";
  // LSP 进度帧:核心在无进度时也占位;JS 里仅在有时显示(布局取舍,已注明)
  const spinMark = ctx.spinner ? " " + ctx.spinner : "";
  const pct = Math.round((row / total) * 100) + "%" + modMark + selMark + roMark + encMark + leMark + regMark + charMark + indMark + ftMark + cwdMark + caMark + wsMark + spinMark;
  parts.push({ text: row + ":" + col, style: null, zone: "right" });
  parts.push({ text: pct, style: "ui.virtual", zone: "right" });
  parts.push({ text: String(total), style: "ui.virtual", zone: "right" });

  return parts;
}

// ============================ 注册 ============================

if (typeof helix !== "undefined") {
  helix.set_statusline(render, { replace: true, zones: [1, 3, 2] });
  helix.on("buffer-open", (doc) => { refreshGit(doc && doc.path); });
}
