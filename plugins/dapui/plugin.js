// dapui/plugin.js — 调试 UI（变量树 / REPL / Output / 断点 / 状态栏段）
//
// 依赖：核心的 `helix.dap.*` + 状态栏插件的外部段通道（`register_statusline_segment`）。
//
// 布局（v1.1，规格见 docs/superpowers/specs/2026-10-25-dapui-layout.md）：
//   · 右侧面板 = **变量树**（栈帧 → 作用域 → 变量，懒加载）
//   · 底侧面板 = **REPL / Output 双 tab**（tab 条是手绘的：面板 API 没有 tab 原语）
//   · 断点不占面板：`:dapui-breakpoints` 开 Picker
//   · 编辑器内：当前执行行高亮 + 行尾内联值（默认关）
//
// 面板按键：面板打开后就是布局的活动叶子 ⇒ `onKey` 直接收键（`tests/test/window_rail.rs`
// 有断言）。所以两个面板各有一套键位，`Esc` 关闭（REPL 里可打印字符都算输入，不能用 q 关）。

helix.plugin("dapui", {
  deps: [],
  version: "0.2",
  schema_version: 1,
  permissions: { process: true }, // `helix.dap.launch/attach` 要起 adapter 子进程
});

helix.define_config("dapui", {
  panel_size: { type: "number", default: 34, doc: "右侧变量树面板宽度(列)" },
  bottom_size: { type: "number", default: 12, doc: "底部 REPL/Output 面板高度(行)" },
  auto_open: { type: "boolean", default: true, doc: "首次命中停止时自动打开变量树面板" },
  auto_open_repl: {
    type: "boolean",
    default: false,
    doc: "命中停止时自动打开底部 REPL 面板",
  },
  inline_values: { type: "boolean", default: false, doc: "命中时在行尾写内联变量值" },
  max_depth: { type: "number", default: 2, doc: "停止时自动展开的层数(0 = 全折叠)" },
  max_nodes: { type: "number", default: 2000, doc: "变量树节点上限(超出截断,不再发请求)" },
});

function cfg() {
  return helix.get_config("dapui") || {};
}

function num(key, fallback) {
  const n = Math.floor(Number(cfg()[key]));
  return Number.isFinite(n) && n > 0 ? n : fallback;
}

// ============================ 状态 ============================

/** 右侧面板：变量树 */
const T = {
  panel_id: null,
  /** 根层节点（树本体；`nodes` 是它按"可见"展平后的结果） */
  roots: [],
  nodes: [],
  cache: {},
  cursor: 0,
  /** 本次停止的上下文:`{ id, thread, frame, source, line }` */
  ctx: null,
};

/** 底部面板：REPL / Output */
const B = {
  panel_id: null,
  /** "repl" | "output" */
  tab: "repl",
  input: "",
  history: [],
  pos: -1,
  /** REPL 转录（回显 / 结果 / 报错） */
  transcript: [],
  /** DAP output 环形缓冲（上限 500） */
  output: [],
  /** 是否跟随尾部 */
  follow: true,
};

const OUTPUT_CAP = 500;

function say(msg) {
  helix.echo("dapui: " + msg);
}

function err_msg(e) {
  // 核心抛给 JS 的 API 错误是**字符串**（不是 Error 对象）⇒ 两种都要认
  return String(e && e.message ? e.message : e);
}

function active_session() {
  const list = helix.dap.sessions() || [];
  return list.find((s) => s.active) || list[0] || null;
}

function short_path(p) {
  if (!p) return "?";
  const parts = String(p).split("/");
  return parts[parts.length - 1] || p;
}

function state_icon(state) {
  switch (state) {
    case "stopped":
      return "⏸";
    case "running":
      return "▶";
    case "starting":
      return "⏳";
    default:
      return "■";
  }
}

function session_badge() {
  const s = active_session();
  if (!s) return "no session";
  let text = `#${s.id} ${state_icon(s.state)}`;
  if (s.state === "stopped" && s.thread !== null && s.thread !== undefined) {
    text += ` t${s.thread}`;
  }
  return text;
}

// ============================ 变量树 ============================

function reset_tree() {
  T.roots = [];
  T.nodes = [];
  T.cache = {};
  T.cursor = 0;
}

function load_roots() {
  const session = active_session();
  reset_tree();
  if (!session || session.thread === null || session.thread === undefined) {
    T.ctx = null;
    return;
  }
  if (session.state !== "stopped") {
    T.ctx = null;
    return;
  }
  const thread = session.thread;
  T.ctx = { id: session.id, thread, frame: session.frame ? session.frame.id : null };
  T.roots = [{ id: "loading", depth: 0, label: "…", value: "loading" }];
  flatten();

  helix.dap
    .stack_frames(thread)
    .then((frames) => {
      const list = frames || [];
      if (list.length === 0) {
        T.roots = [{ id: "noframes", depth: 0, label: "(没有栈帧)", value: "" }];
        flatten();
        return;
      }
      const currentId = T.ctx && T.ctx.frame;
      const ordered = list
        .slice()
        .sort((a, b) => (a.id === currentId ? -1 : b.id === currentId ? 1 : 0));
      T.ctx.frame = ordered[0].id;
      // 内联值要"当前帧的文件 + 行"：会话快照的 frame 没有 source 字段，从栈帧里记下来
      T.ctx.source = ordered[0].source || null;
      T.ctx.line = ordered[0].line || 0;
      const roots = ordered.slice(0, 20).map((f) => ({
        id: "f" + f.id,
        depth: 0,
        label: `${f.name}  ${short_path(f.source)}:${f.line + 1}`,
        value: "",
        frame: f.id,
        expandable: true,
        expanded: false,
      }));
      T.roots = roots;
      flatten();
      helix.dap
        .scopes(T.ctx.frame)
        .then((scopes) => {
          roots[0].children = (scopes || []).map((sc) => ({
            id: "s" + sc.variablesReference,
            depth: 1,
            label: sc.name,
            value: sc.expensive ? "(expensive)" : "",
            ref: sc.variablesReference,
            expandable: true,
            expanded: false,
          }));
          roots[0].expanded = true;
          flatten();
          const depth = Math.max(0, Math.floor(Number(cfg().max_depth) || 0));
          if (depth >= 2) {
            const first = roots[0].children[0];
            if (first) expand_node(first, depth - 1);
          }
        })
        .catch((e) => {
          roots[0].children = [{ id: "serr", depth: 1, label: "!! " + err_msg(e), value: "", error: true }];
          flatten();
        });
    })
    .catch((e) => {
      T.roots = [{ id: "ferr", depth: 0, label: "!! " + err_msg(e), value: "", error: true }];
      flatten();
    });
}

/** 把树按"可见"展平成 `T.nodes`（折叠的子节点不进列表）。 */
function flatten() {
  const out = [];
  const budget = num("max_nodes", 2000);
  let truncated = false;
  const walk = (list) => {
    for (const node of list || []) {
      if (out.length >= budget) {
        truncated = true;
        return;
      }
      out.push(node);
      if (node.expanded && node.children) walk(node.children);
    }
  };
  walk(T.roots);
  if (truncated) {
    out.push({
      id: "trunc",
      depth: 0,
      label: `… (truncated, max_nodes=${budget})`,
      value: "",
    });
  }
  T.nodes = out;
  if (T.cursor >= T.nodes.length) T.cursor = Math.max(0, T.nodes.length - 1);
}

function expand_node(node, auto_depth) {
  if (!node || !node.expandable) return;
  if (node.loaded) {
    node.expanded = true;
    flatten();
    return;
  }
  const cached = T.cache[node.id];
  if (cached) {
    node.children = cached;
    node.loaded = true;
    node.expanded = true;
    flatten();
    return;
  }
  node.loading = true;
  flatten();
  helix.dap
    .variables(node.ref)
    .then((vars) => {
      node.children = (vars || []).map((v) => ({
        id: node.id + "/" + v.name,
        depth: node.depth + 1,
        label: v.name,
        value: format_value(v),
        ref: v.variablesReference,
        expandable: (v.variablesReference || 0) > 0,
        expanded: false,
      }));
      T.cache[node.id] = node.children;
      node.loaded = true;
      node.expanded = true;
      node.loading = false;
      flatten();
      if (auto_depth > 0) {
        const first = node.children.find((c) => c.expandable);
        if (first) expand_node(first, auto_depth - 1);
      }
    })
    .catch((e) => {
      node.loading = false;
      node.children = [
        { id: node.id + "/err", depth: node.depth + 1, label: "!! " + err_msg(e), value: "", error: true },
      ];
      node.loaded = true;
      node.expanded = true;
      flatten();
    });
}

function format_value(v) {
  let out = v.value === null || v.value === undefined ? "" : String(v.value);
  if (v.type) out += " :" + v.type;
  if (v.namedVariables) out += ` (${v.namedVariables})`;
  else if (v.indexedVariables) out += ` [${v.indexedVariables}]`;
  return out;
}

function collapse_node(node) {
  if (!node) return;
  if (node.expanded) {
    node.expanded = false;
    flatten();
    return;
  }
  for (let i = T.cursor - 1; i >= 0; i--) {
    if (T.nodes[i].depth < node.depth) {
      T.cursor = i;
      return;
    }
  }
}

// ============================ 右侧面板：渲染 + 键位 ============================

function panel_box(focus, ctx, fallback_w, fallback_h) {
  const w = Math.max(12, (ctx && ctx.width) || fallback_w);
  const h = Math.max(3, (ctx && ctx.height) || fallback_h);
  return { w, h };
}

function render_tree(focus, ctx) {
  const { w, h } = panel_box(focus, ctx, num("panel_size", 34), 20);
  const session = active_session();
  helix.draw.text(
    0,
    0,
    helix.text.truncate(` ${session ? session_badge() : "no session"}`, w),
    { style: "ui.text.focus" },
  );
  const tree_h = Math.max(1, h - 1);
  const total = T.nodes.length;
  let top = 0;
  if (total > tree_h) {
    top = Math.min(Math.max(0, T.cursor - Math.floor(tree_h / 2)), Math.max(0, total - tree_h));
  }
  const sel = T.nodes[T.cursor];
  for (let i = 0; i < tree_h && top + i < total; i++) {
    const node = T.nodes[top + i];
    const pad = "  ".repeat(node.depth);
    const mark = node.expandable ? (node.expanded ? "▾ " : "▸ ") : "  ";
    const loading = node.loading ? " …" : "";
    const line = `${pad}${mark}${node.label}${node.value ? " = " + node.value : ""}${loading}`;
    let style = null;
    if (node.error) style = "error";
    else if (sel && node.id === sel.id) style = "ui.selection";
    helix.draw.text(0, 1 + i, helix.text.truncate(line, w), { style });
  }
  return helix.draw.done();
}

function key_tree(key) {
  if (key.ctrl || key.alt) {
    if (key.ctrl && key.name === "\\") {
      helix.pane.focus(0);
      return "handled";
    }
    return "ignore";
  }
  switch (key.name) {
    case "Esc":
    case "q":
      close_tree();
      return "handled";
    case "Up":
    case "k":
      T.cursor = Math.max(0, T.cursor - 1);
      return "handled";
    case "Down":
    case "j":
      T.cursor = Math.min(Math.max(0, T.nodes.length - 1), T.cursor + 1);
      return "handled";
    case "Right":
    case "l":
    case "Enter":
      expand_node(T.nodes[T.cursor], 0);
      return "handled";
    case "Left":
    case "h":
      collapse_node(T.nodes[T.cursor]);
      return "handled";
    case "R":
      T.cache = {};
      load_roots();
      return "handled";
    case "g":
      T.cursor = 0;
      return "handled";
    case "G":
      T.cursor = Math.max(0, T.nodes.length - 1);
      return "handled";
    case "?":
      show_help();
      return "handled";
    default:
      return "ignore";
  }
}

function show_help() {
  say(
    "变量树:↑↓/jk 移动 · l/Enter 展开 · h 折叠 · R 重取 · q/Esc 关\n" +
      "底部:Tab 切 REPL/Output · Esc 关 · 命令 :dapui-repl / :dapui-output / :debug-eval <expr>",
  );
}

function open_tree() {
  if (T.panel_id !== null) return;
  T.panel_id = helix.open_panel({
    side: "right",
    size: num("panel_size", 34),
    // 注意：**不要**写 `rail: true`。rail 是"贴边全高轨道"，全局**只能有一条** ——
    // 注册新的会把旧的取掉（filetree 也是侧栏，两个一起用会互相驱逐）。
    // 面板打开后本身就是活动叶子 ⇒ 按键直达 `onKey`（`tests/test/window_rail.rs` 有断言）。
    render: render_tree,
    onKey: key_tree,
    focusable: true,
    onClose: () => {
      T.panel_id = null;
    },
  });
  load_roots();
}

function close_tree() {
  if (T.panel_id !== null) {
    helix.close_panel(T.panel_id);
    T.panel_id = null;
  }
}

function toggle_tree() {
  if (T.panel_id !== null) close_tree();
  else open_tree();
}

// ============================ 底部面板：REPL / Output ============================

function repl_push(text, style) {
  B.transcript.push({ text, style: style || null });
  if (B.transcript.length > OUTPUT_CAP) B.transcript.shift();
}

function output_push(text, style) {
  B.output.push({ text, style: style || null });
  if (B.output.length > OUTPUT_CAP) B.output.shift();
  if (B.follow) B.scroll = 0;
}

function repl_eval() {
  const expr = B.input.trim();
  B.input = "";
  if (!expr) return;
  B.history.push(expr);
  B.pos = -1;
  repl_push("(f) " + expr, "ui.virtual");
  const frame = T.ctx ? T.ctx.frame : undefined;
  helix.dap
    .eval(expr, frame)
    .then((r) => {
      repl_push(
        `${expr} => ${r && r.result !== null && r.result !== undefined ? r.result : ""}` +
          `${r && r.type ? " (" + r.type + ")" : ""}`,
      );
    })
    .catch((e) => {
      // 失败落在 REPL 里（不 echo：状态栏刷屏更烦）
      repl_push(`${expr} !! ${err_msg(e)}`, "error");
    });
}

function render_bottom(focus, ctx) {
  const { w, h } = panel_box(focus, ctx, 80, num("bottom_size", 12));
  // ① tab 条：手绘（面板没有 tab 原语）
  const repl_label = (B.tab === "repl" ? "▸" : " ") + "REPL";
  const output_label = (B.tab === "output" ? "▸" : " ") + "Output";
  helix.draw.text(0, 0, `${repl_label}  ${output_label}`, { style: "ui.text" });
  const badge = session_badge();
  if (badge.length + 8 < w) {
    helix.draw.text(w - badge.length - 1, 0, badge, { style: "ui.virtual" });
  }
  helix.draw.hline(0, 1, w, { style: "ui.virtual" });

  const body_h = Math.max(1, h - 2);
  if (B.tab === "repl") {
    const lines = B.transcript.slice(-(body_h - 1));
    for (let i = 0; i < lines.length; i++) {
      helix.draw.text(0, 2 + i, helix.text.truncate(lines[i].text, w), {
        style: lines[i].style,
      });
    }
    const prompt = "> " + B.input + "█";
    helix.draw.text(0, h - 1, helix.text.truncate(prompt, w), { style: "ui.text.focus" });
  } else {
    const start = Math.max(0, B.output.length - body_h - B.scroll);
    const lines = B.output.slice(start, start + body_h);
    for (let i = 0; i < lines.length; i++) {
      helix.draw.text(0, 2 + i, helix.text.truncate(lines[i].text, w), {
        style: lines[i].style,
      });
    }
    if (B.output.length === 0) {
      helix.draw.text(0, 2, "(no output yet)", { style: "ui.virtual" });
    }
  }
  return helix.draw.done();
}

function key_bottom(key) {
  if (key.ctrl || key.alt) {
    if (key.ctrl && key.name === "\\") {
      helix.pane.focus(0);
      return "handled";
    }
    return "ignore";
  }
  // 与 tab 无关的键：切换 / 关闭
  switch (key.name) {
    case "Tab":
      B.tab = B.tab === "repl" ? "output" : "repl";
      return "handled";
    case "1":
      if (B.tab !== "repl" || B.input.length === 0) {
        B.tab = "repl";
        return "handled";
      }
      break;
    case "2":
      if (B.tab !== "output" || B.input.length === 0) {
        B.tab = "output";
        return "handled";
      }
      break;
    case "Esc":
      close_bottom();
      return "handled";
  }

  if (B.tab === "repl") {
    switch (key.name) {
      case "Enter":
        repl_eval();
        return "handled";
      case "Backspace":
        B.input = B.input.slice(0, -1);
        return "handled";
      case "Up":
      case "Down": {
        if (B.history.length === 0) return "handled";
        if (key.name === "Up") B.pos = Math.min(B.history.length - 1, B.pos + 1);
        else B.pos = Math.max(-1, B.pos - 1);
        B.input = B.pos >= 0 ? B.history[B.history.length - 1 - B.pos] : "";
        return "handled";
      }
      default:
        if (typeof key.name === "string" && key.name.length === 1) {
          B.input += key.name;
          return "handled";
        }
        return "ignore";
    }
  }

  // Output tab：滚动
  switch (key.name) {
    case "Up":
    case "k":
      B.scroll = Math.min(Math.max(0, B.output.length - 1), B.scroll + 1);
      B.follow = false;
      return "handled";
    case "Down":
    case "j":
      B.scroll = Math.max(0, B.scroll - 1);
      if (B.scroll === 0) B.follow = true;
      return "handled";
    case "g":
      B.scroll = Math.max(0, B.output.length - 1);
      B.follow = false;
      return "handled";
    case "G":
      B.scroll = 0;
      B.follow = true;
      return "handled";
    default:
      return "ignore";
  }
}

/// 底部面板高度（行）：核心按"size / 当前视口行数"算比例，小终端上 12 行会吃掉半个屏，
/// 所以按视口行数收一收（最多 40%）。
function bottom_rows() {
  const want = num("bottom_size", 12);
  let rows = 40;
  try {
    const vp = helix.viewport();
    if (vp && vp[1] > 8) rows = vp[1];
  } catch (e) {
    /* 无 viewport：用默认行数 */
  }
  return Math.max(5, Math.min(want, Math.floor(rows * 0.4)));
}

function open_bottom(tab) {
  if (tab) B.tab = tab;
  if (B.panel_id !== null) return;
  B.panel_id = helix.open_panel({
    side: "bottom",
    size: bottom_rows(),
    render: render_bottom,
    onKey: key_bottom,
    focusable: true,
    onClose: () => {
      B.panel_id = null;
    },
  });
}

function close_bottom() {
  if (B.panel_id !== null) {
    helix.close_panel(B.panel_id);
    B.panel_id = null;
  }
}

// ============================ 断点 Picker ============================

function breakpoints_picker() {
  helix.picker.define("dapui-breakpoints", {
    columns: ["where", "flags"],
    items: () => {
      const list = helix.dap.list_breakpoints() || [];
      return list.map((b) => ({
        cells: [
          `${b.path}:${b.line + 1}`,
          [b.verified ? "✓" : "◯", b.condition ? "cond" : "", b.log ? "log" : ""]
            .filter(Boolean)
            .join(" "),
        ],
        payload: [b.path, b.line],
      }));
    },
    preview: (row) => ({ path: row[0], line: row[1] }),
    action: (row) => {
      helix.open_file(row[0], { row: row[1] });
    },
    onKey: (e) => {
      if (e.key !== "d") return undefined;
      say("删除请在编辑器里按 <space>G b（当前行 toggle）");
      return "handled";
    },
  });
  helix.picker.run("dapui-breakpoints");
}

// ============================ 状态栏段 ============================

function statusline_segment() {
  const session = active_session();
  if (!session) return null;
  let text = `#${session.id} ${state_icon(session.state)}`;
  if (session.state === "stopped" && session.thread !== null && session.thread !== undefined) {
    text += ` t${session.thread}`;
  }
  return { text, style: session.state === "stopped" ? "error" : "ui.virtual" };
}

// ============================ 内联值（默认关） ============================

function apply_inline_values() {
  if (!cfg().inline_values) return;
  if (!T.ctx || !T.ctx.source) return;
  const path = T.ctx.source;
  const row = T.ctx.line || 0;
  try {
    helix.set_virtual_text(path); // 先清旧值，避免残留
  } catch (e) {
    return;
  }
  const locals = (T.cache[(T.roots[0] && T.roots[0].id) || ""] || []).slice(0, 8);
  const text = locals
    .filter((n) => n.value)
    .map((n) => `${n.label}=${n.value}`)
    .join(" ");
  if (!text) return;
  try {
    helix.set_virtual_text(path, row, 0, "  " + text, "ui.virtual");
  } catch (e) {
    /* 行号越界等，忽略 */
  }
}

function clear_inline_values() {
  const path = T.ctx && T.ctx.source;
  if (!path) return;
  try {
    helix.set_virtual_text(path);
  } catch (e) {
    /* ignore */
  }
}

// ============================ 事件 ============================

helix.on("dap-stopped", () => {
  if (cfg().auto_open !== false) open_tree();
  load_roots();
  if (cfg().auto_open_repl) open_bottom("repl");
  apply_inline_values();
});

helix.on("dap-continued", () => {
  clear_inline_values();
  T.ctx = null;
  reset_tree();
});

helix.on("dap-terminated", () => {
  clear_inline_values();
  T.ctx = null;
  reset_tree();
});

helix.on("dap-exited", () => {
  clear_inline_values();
});

helix.on("dap-output", (payload) => {
  if (!payload) return;
  const style = payload.category === "stderr" ? "error" : null;
  output_push(String(payload.output || "").replace(/\n+$/, ""), style);
});

// ============================ 命令 ============================

helix.register_command("dapui", () => toggle_tree());
helix.register_command("dapui-repl", () => open_bottom("repl"));
helix.register_command("dapui-output", () => open_bottom("output"));
helix.register_command("dapui-breakpoints", () => breakpoints_picker());

helix.register_statusline_segment("dap", statusline_segment);

helix.export({
  open_tree,
  close_tree,
  toggle_tree,
  open_bottom,
  close_bottom,
  refresh: load_roots,
  render_tree,
  render_bottom,
  bottom_rows,
  key_tree,
  key_bottom,
  statusline_segment,
  T,
  B,
});
