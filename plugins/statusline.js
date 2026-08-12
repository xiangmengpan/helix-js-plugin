// statusline.js — 状态栏美化插件（lazyvim 风格：mode 色块 + 图标 + 诊断 + git + 位置）
// 依赖：icons.js（统一图标映射）；helix.set_statusline 分段样式（Rust 增强）。
// 说明：追加到状态栏右侧（右对齐）；左侧保持 helix 默认（mode 色块 + 文件名），
//       可在 config.toml 的 [statusline] 调整左侧元素。

// ============================ 状态 ============================

let ICONS = null;
let gitBranch = "";      // 当前 git 分支（异步查询缓存）
let gitDirty = false;    // 需要重新查询

// ============================ git 分支（异步 + 缓存） ============================

function refreshGit(path) {
  if (!path) { gitBranch = ""; return; }
  gitDirty = true;
  // 打开/切换文件时查一次；结果缓存，状态栏渲染零开销
  helix.run_async("git -C " + shq(dirname(path)) + " branch --show-current", (err, out) => {
    if (!err) gitBranch = (out || "").trim();
    else gitBranch = "";
    gitDirty = false;
  });
}

function dirname(p) {
  const i = p.lastIndexOf("/");
  return i > 0 ? p.slice(0, i) : ".";
}

function shq(s) { return "'" + String(s).replace(/'/g, "'\\''") + "'"; }

// ============================ 状态栏渲染 ============================

// 分段：每段 { text, style }，style 为主题 scope（mode 色块用 ui.statusline.*，
// 诊断用 error/warning，弱化用 ui.virtual，分隔用默认）
function render(ctx) {
  if (!ICONS) ICONS = helix.load("icons.js") || null;
  const mode = ctx.mode || "normal";
  const parts = [];

  // 1. mode 色块（跟随主题 ui.statusline.<mode>，与左侧默认 mode 色块同色系）
  const modeStyle = "ui.statusline." + mode;
  parts.push({ text: " " + (ICONS ? ICONS.getModeIcon(mode) : mode.charAt(0).toUpperCase()) + " ", style: modeStyle });

  // 2. 文件名（带文件类型图标）
  const name = ctx.path ? ctx.path.split("/").pop() : "[scratch]";
  const fileIcon = ICONS ? ICONS.getFileIcon(ctx.path || "") : "";
  parts.push({ text: " " + fileIcon + name + " ", style: null });

  // 3. git 分支（异步缓存；无仓库时不显示； = git-branch 图标）
  if (gitBranch) {
    parts.push({ text: " \uf1d3 " + gitBranch + " ", style: "ui.virtual" });
  }

  // 4. 诊断计数（error 红 / warning 黄；0 不显示）
  if (ctx.diagnostics_error > 0) {
    parts.push({ text: " " + (ICONS ? ICONS.getDiagnosticIcon("error") : "✗") + " " + ctx.diagnostics_error + " ", style: "error" });
  }
  if (ctx.diagnostics_warning > 0) {
    parts.push({ text: " " + (ICONS ? ICONS.getDiagnosticIcon("warning") : "!") + " " + ctx.diagnostics_warning + " ", style: "warning" });
  }

  // 5. 分隔符
  parts.push({ text: "│", style: "ui.virtual" });

  // 6. 位置：行:列（1-based）、百分比、总行数
  const row = (ctx.cursor.row || 0) + 1;
  const col = (ctx.cursor.col || 0) + 1;
  const total = ctx.total_lines || 1;
  const pct = Math.round((row / total) * 100) + "%";
  parts.push({ text: " " + row + ":" + col + " ", style: null });
  parts.push({ text: pct + " ", style: "ui.virtual" });
  parts.push({ text: String(total) + " ", style: "ui.virtual" });

  return parts;
}

// ============================ 注册 ============================

if (typeof helix !== "undefined") {
  helix.set_statusline(render);
  helix.on("buffer-open", (doc) => { refreshGit(doc && doc.path); });
}

