// statusline.js — 状态栏美化（lazyvim 风格三段分栏）
// 依赖清单：icons（统一图标映射，自动先加载）
helix.plugin("statusline", { deps: ["lib/icons.js"] });
// 依赖：icons.js（统一图标映射）；set_statusline replace 模式 + zones 比例（Rust 增强）。
// 布局：左区（mode 色块 + 文件名 + git）· 中区（诊断，居中）· 右区（位置/百分比/行数，右对齐贴最右）。
// 比例在注册处 zones: [左, 中, 右]；段间自动 1 空格 gap。

// ============================ 状态 ============================

let ICONS = null;
let gitBranch = "";      // 当前 git 分支（异步查询缓存）

function refreshGit(path) {
  if (!path) { gitBranch = ""; return; }
  helix.run_async("git -C " + shq(dirname(path)) + " branch --show-current", (err, out) => {
    gitBranch = err ? "" : (out || "").trim();
  });
}

function dirname(p) {
  const i = p.lastIndexOf("/");
  return i > 0 ? p.slice(0, i) : ".";
}

function shq(s) { return "'" + String(s).replace(/'/g, "'\\''") + "'"; }

// ============================ 状态栏渲染 ============================

// 三段分栏：zone 默认 left；诊断走 center；位置/百分比/行数走 right。
function render(ctx) {
  if (!ICONS) ICONS = helix.load("lib/icons.js") || null;
  const mode = ctx.mode || "normal";
  const parts = [];

  // ── 左区：mode 色块 + 文件名（类型图标）+ git 分支 ──
  parts.push({ text: " " + (ICONS ? ICONS.getModeIcon(mode) : mode.charAt(0).toUpperCase()) + " ", style: "ui.statusline." + mode });
  const name = ctx.path ? ctx.path.split("/").pop() : "[scratch]";
  const fileIcon = ICONS ? ICONS.getFileIcon(ctx.path || "") : "";
  parts.push({ text: fileIcon + " " + name, style: null });
  if (gitBranch) {
    parts.push({ text: "\uf1d3 " + gitBranch, style: "ui.virtual" });
  }

  // ── 中区：诊断计数（error 红 / warning 黄；0 不显示）──
  if (ctx.diagnostics_error > 0) {
    parts.push({ text: (ICONS ? ICONS.getDiagnosticIcon("error") : "✗") + " " + ctx.diagnostics_error, style: "error", zone: "center" });
  }
  if (ctx.diagnostics_warning > 0) {
    parts.push({ text: (ICONS ? ICONS.getDiagnosticIcon("warning") : "!") + " " + ctx.diagnostics_warning, style: "warning", zone: "center" });
  }

  // ── 右区：位置 + 百分比 + 总行数（右对齐贴最右）──
  const row = (ctx.cursor.row || 0) + 1;
  const col = (ctx.cursor.col || 0) + 1;
  const total = ctx.total_lines || 1;
  const pct = Math.round((row / total) * 100) + "%";
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
