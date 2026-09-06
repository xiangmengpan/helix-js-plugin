// arsenal.js — mason 式工具市场窗(M3 主视图;M4 接动作执行/信息详情/批量进度)
// 用法:init.js 里 helix.load("features/arsenal/index.js"),:arsenal 开关市场窗。
// 键位:j/k 或 Up/Down 导航 · 可打印字符即搜(匹配名/语言/描述) · f 分类循环 ·
//       t 标记(批量) · x/u 直达卸载/升级占位 · i 信息占位弹窗 · Enter 动作占位 ·
//       Backspace 删过滤 · q/Esc 关闭。
// 数据:helix.server.rows(cb) 回传行 {name,kind,languages,installed,local,version,
//       description,homepage,installable};可升级 ▲/busy 进度需行字段扩展(M4/任务7)。
// 容器:规格想居中浮动大窗,但 open_popup 实测无居中/无百分比、content 自适 + 上限、
//       且全插件共享单弹窗层(重复打开替换前一个 → M4 信息/菜单/版本输入无法叠在市场之上);
//       M4 弹窗叠层需求下唯一可行容器是 open_panel(side:left),此为主参照 filetree 同款。
//       详见任务 6 report。

helix.plugin("arsenal", { deps: [] });

// 行布局列宽(按面板实际宽度换算,见 render):名称列 / 状态列固定,描述列伸缩。
const NAME_W = 24; // mark(2)+icon(2)+name(≈19 字符)
const ST_W = 14; // 状态最宽 "✓ 2024-09-16"=14
const DEF_W = 64; // ctx 缺失时的面板宽度回退

// 分类循环序(含 kind 过滤位;规格的 "可升级" 需行字段,本任务无 → 略)
const KINDS = ["all", "lsp", "dap", "linter", "formatter", "installed", "local"];
const KIND_GLYPH = { lsp: "L", dap: "D", linter: "!", formatter: "F" };

const S = {
  panel: null, // open_panel 返回 id(null = 未打开)
  rows: [], // 全部配方行(server.rows 回传)
  filter: "", // 即搜过滤串
  kind: "all", // 当前分类
  sel: 0, // 选中行索引(visible 序)
  marks: new Set(), // 标记集合(name;Enter 批量,M4 接执行)
  // M4:busy 批量进度状态在此扩展(底栏渲染)
};

function fetch_rows() {
  helix.server.rows((rows) => {
    S.rows = rows || [];
    if (S.sel >= S.rows.length) S.sel = Math.max(0, S.rows.length - 1);
  });
}

function selected_row() {
  const rows = visible();
  return rows.length ? rows[Math.min(S.sel, rows.length - 1)] : null;
}

function move(d) {
  const n = visible().length;
  if (!n) return;
  S.sel = Math.max(0, Math.min(S.sel + d, n - 1));
}

function cycle_kind() {
  const i = KINDS.indexOf(S.kind);
  S.kind = KINDS[(i + 1) % KINDS.length];
  S.sel = 0;
}

function toggle_mark() {
  const r = selected_row();
  if (!r) return;
  if (S.marks.has(r.name)) S.marks.delete(r.name);
  else S.marks.add(r.name);
}

// 过滤:分类 + 即搜(名/语言/描述子串;大小写不敏感)
function visible() {
  const f = S.filter.toLowerCase();
  return S.rows.filter((r) => {
    if (S.kind === "installed" && !r.installed) return false;
    if (S.kind === "local" && !r.local) return false;
    if (S.kind !== "all" && S.kind !== "installed" && S.kind !== "local" && r.kind !== S.kind)
      return false;
    if (!f) return true;
    return (r.name + " " + (r.languages || []).join(" ") + " " + (r.description || "")).toLowerCase().includes(f);
  });
}

// 行状态(M4:行 busy 进度覆写插在最前;可升级 ▲ 需 is_upgradable 行字段,任务 7 后加)
function status_text(r) {
  if (r.installed && r.local) return r.version ? "本机 " + r.version : "本机";
  if (r.installed) return "✓ " + (r.version || "");
  if (r.installable) return "–";
  return "no source";
}

function row_el(r, i, W) {
  const sel = i === S.sel;
  const mark = S.marks.has(r.name) ? "▣ " : "  ";
  const name = mark + (KIND_GLYPH[r.kind] || "?") + " " + r.name;
  const head = name.length <= NAME_W ? name + " ".repeat(NAME_W - name.length) : name;
  const st = status_text(r).slice(0, ST_W);
  const dw = Math.max(1, W - NAME_W - ST_W - 1); // 描述列宽
  const desc = (r.description || "").slice(0, dw);
  const line = head + desc + " ".repeat(dw - desc.length) + " " + st;
  return helix.el("text", line, sel ? { style: "ui.selection" } : {});
}

function render(focus, ctx) {
  try {
    const W = Math.max(40, (ctx && ctx.width) || DEF_W);
    const rows = visible();
    const H = Math.max(4, ((ctx && ctx.height) || 30) - 4); // 标题/底栏占 2 行 + 余量
    if (S.sel > rows.length - 1) S.sel = Math.max(0, rows.length - 1);
    const start = Math.max(0, Math.min(S.sel - Math.floor(H / 2), Math.max(0, rows.length - H)));
    const win = rows.slice(start, start + H).map((r, i) => row_el(r, start + i, W));
    const list = win.length
      ? win
      : [helix.el("text", "(无匹配 · q/Esc 关闭)", { style: "ui.virtual" })];
    const title = helix.el(
      "text",
      "arsenal" + (S.filter ? " [" + S.filter + "]" : "") + " · " + S.kind + " · " + rows.length + "/" + S.rows.length + " servers",
      { style: "ui.virtual" }
    );
    const footer = helix.el(
      "text",
      "j/k ↑↓ · 字符即搜 · Enter 操作 · i 详情 · f 分类 · t 标记 · x 卸载 · u 升级 · q/Esc 关闭",
      { style: "ui.virtual" }
    );
    return helix.el("col", [title, helix.el("scroll", list, { height: H }), footer]);
  } catch (e) {
    return helix.el("col", [
      helix.el("text", "arsenal 渲染错误: " + (e && e.message || e), { style: "ui.popup" }),
    ]);
  }
}

// ────────────────────────── 动作占位(M4 替换为真实执行/菜单) ──────────────────────────

// 行上下文动作清单(菜单项来源,M4 弹菜单/执行用;本任务仅 echo 占位)
function row_actions(r) {
  const acts = [];
  if (r.local) acts.push("unmanage");
  else if (r.installed) acts.push("update");
  else if (r.installable) acts.push("install");
  if (r.installed && !r.local) acts.push("remove");
  return acts;
}

function act(op) {
  const r = selected_row();
  if (!r) return;
  // TODO(M4):helix.server.task([{op,name}], cb) 后台执行 + 行 busy/进度;本任务占位 echo
  helix.echo("arsenal: " + op + " " + r.name + " → M4 后台任务执行");
}

function open_action_menu() {
  const r = selected_row();
  if (!r) return;
  const acts = row_actions(r);
  if (!acts.length) {
    helix.echo("arsenal: " + r.name + " 无可用动作(无下载源/本地既有)");
    return;
  }
  // TODO(M4):小弹窗菜单(↑↓/Enter 选、Esc 关);本任务占位 echo 可选动作
  helix.echo("arsenal: " + r.name + " 动作: " + acts.join("/") + " (M4 动作菜单)");
}

// ────────────────────────── 信息占位弹窗(M4 完善行字段后补命令/来源) ──────────────────────────

function open_info() {
  const r = selected_row();
  if (!r) return;
  const lang = (r.languages || []).join(", ") || "—";
  const st = status_text(r) + (r.version ? " (" + r.version + ")" : "");
  const lines = [
    helix.el("text", r.name + "  [" + r.kind + "]", { style: "ui.popup" }),
    helix.el("text", "状态: " + st),
    helix.el("text", "语言: " + lang),
    helix.el("text", "描述: " + (r.description || "—")),
    helix.el("text", "主页: " + (r.homepage || "—")),
    helix.el("text", "命令路径/下载源: 行字段扩展后显示(M4)", { style: "ui.virtual" }),
    helix.el("text", "Enter/Esc/q 关闭", { style: "ui.virtual" }),
  ];
  helix.open_popup({
    width: 64,
    height: 12,
    render: () => helix.el("col", lines, { gap: 1 }),
    onKey: (key) => (key.name === "Esc" || key.name === "q" || key.name === "Enter" ? "close" : "handled"),
  });
}

// ────────────────────────── 开关 ──────────────────────────

function open_arsenal() {
  if (S.panel !== null) return;
  fetch_rows();
  S.panel = helix.open_panel({
    side: "left",
    size: 64, // 规格想 78% 大窗;open_panel size = 字符列宽(0.1~0.9 屏占比内按字符)
    render,
    onKey: handle_key,
    onClose: () => {
      S.panel = null;
    },
  });
}

function close_arsenal() {
  if (S.panel !== null) {
    helix.close_panel(S.panel);
    S.panel = null; // onClose 也会置 null;幂等
  }
}

function toggle_arsenal() {
  if (S.panel !== null) close_arsenal();
  else open_arsenal();
}

// ────────────────────────── 按键 ──────────────────────────

function handle_key(key) {
  // 修饰键组合(C-x / M-x / C-\ 等)不处理,穿透(与 filetree 一致)
  if (key.ctrl || key.alt) return "handled";
  const k = key.name;
  if (k === "q" || k === "Esc") {
    close_arsenal();
    return "handled";
  }
  if (k === "j" || k === "Down") {
    move(1);
    return "handled";
  }
  if (k === "k" || k === "Up") {
    move(-1);
    return "handled";
  }
  if (k === "Backspace") {
    S.filter = S.filter.slice(0, -1);
    return "handled";
  }
  if (k === "Enter") {
    if (S.marks.size) {
      const n = S.rows.filter((r) => S.marks.has(r.name)).length;
      // TODO(M4):批量 queue(helix.server.task 串行)+ 底栏进度;本任务占位 echo
      helix.echo("arsenal: batch " + n + " selected → M4 执行");
    } else {
      open_action_menu();
    }
    return "handled";
  }
  if (k === "i") {
    open_info();
    return "handled";
  }
  if (k === "f") {
    cycle_kind();
    return "handled";
  }
  if (k === "t") {
    toggle_mark();
    return "handled";
  }
  if (k === "x") {
    act("remove");
    return "handled";
  }
  if (k === "u") {
    act("update");
    return "handled";
  }
  if (k.length === 1) {
    S.filter += k; // 字符即搜(含空格/标点)
    return "handled";
  }
  return "handled"; // 未映射键消费(模态市场窗;q/Esc 退出)
}

// ────────────────────────── 注册(node 环境跳过) ──────────────────────────

if (typeof helix !== "undefined") {
  helix.register_command("arsenal", toggle_arsenal, "打开 LSP/工具管理市场(搜索/分类/标记;Enter 操作)");
}

if (typeof module !== "undefined" && module.exports) {
  module.exports = {
    S, KINDS, KIND_GLYPH,
    visible, status_text, move, cycle_kind, toggle_mark, handle_key, render,
  };
}
