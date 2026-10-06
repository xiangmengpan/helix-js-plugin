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
  const pct = Math.round((row / total) * 100) + "%" + modMark;
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
