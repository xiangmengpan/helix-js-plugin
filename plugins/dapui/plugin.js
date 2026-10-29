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
  // process: 起 adapter 子进程 + 读 `$HOME`（历史文件路径要绝对路径）
  // fs_read/fs_write: 只用于 `~/.cache/helix/dapui-repl-history`（REPL 历史持久化）
  permissions: { process: true, fs_read: true, fs_write: true },
});

helix.define_config("dapui", {
  panel_size: { type: "number", default: 40, doc: "右侧变量树面板宽度(列)" },
  labels: {
    type: "enum",
    default: "en",
    options: ["en", "zh-CN"],
    doc: "树里的分组名语言（Watches/Locals/Globals… vs 监视/局部/全局…）",
  },
  persist_watches: {
    type: "boolean",
    default: false,
    doc: "把监视表达式按**工作区**存盘（默认关：只在本次运行内有效，重启 hx 就清空）",
  },
  just_my_code: {
    type: "boolean",
    default: false,
    doc: "单步时跳过**不在工作区**的源码（std/core/系统库）：落点在工作区外就自动 stepOut，最多 12 跳",
  },
  frame_name: {
    type: "enum",
    default: "short",
    options: ["short", "keeper", "full"],
    doc: "栈帧名：short=main/len（默认）/ keeper=保留类型限定 Point::len / full=完整路径 rsdbg::add",
  },
  align_columns: {
    type: "boolean",
    default: false,
    doc: "变量名补空格对齐（`a    = 2`）；默认不补，写成一格：`a = 2`",
  },
  columns_header: {
    type: "boolean",
    default: false,
    doc: "变量列表上方画一行列头（名字 / 值 / 类型）；多占 1 行，默认关",
  },
  locals_summary: {
    type: "boolean",
    default: false,
    doc: "树顶那行 `a=2 b=3` 摘要（默认关：展开 Locals 就有同样的信息，省一行）",
  },
  bottom_size: { type: "number", default: 12, doc: "底部 REPL/Output 面板高度(行)" },
  auto_open: { type: "boolean", default: true, doc: "首次命中停止时自动打开变量树面板" },
  focus_on_auto_open: {
    type: "boolean",
    default: false,
    doc: "自动打开面板时把键盘焦点也给它(默认 false:面板出现但焦点留在编辑器)",
  },
  auto_open_repl: {
    type: "boolean",
    default: false,
    doc: "(旧) 等价于 auto_open_bottom = \"repl\"；两者都设时以 auto_open_bottom 为准",
  },
  auto_open_bottom: {
    type: "enum",
    default: "off",
    options: ["off", "repl", "output", "breakpoints"],
    doc: "命中停止时同时打开底部面板并切到该页(off=不开)",
  },
  max_frames: { type: "number", default: 20, doc: "变量树里最多列多少个栈帧(0=不限)" },
  perspective: {
    type: "boolean",
    default: false,
    doc: "停止时记住当前布局，继续/结束时自动恢复（调试视角）",
  },
  readonly_on_stop: {
    type: "enum",
    default: "off",
    options: ["off", "doc", "all"],
    doc: "命中停止时把 buffer 设为强制只读(doc=当前文件 / all=所有打开的文件),继续/结束时自动解除",
  },
  show_all_sessions: {
    type: "boolean",
    default: true,
    doc: "多会话时把每个会话都画在树里（当前会话展开，其余折叠；选中另一个会话按 Enter 切换）",
  },
  breakpoint_marks: {
    type: "boolean",
    // 默认**关**：核心的 gutter 已经画 `●/◯`（`helix-view/src/gutter.rs`），
    // 这里再插一段虚拟文本就等于"行号前后各一个标记"。
    default: false,
    doc: "在断点行首画 ●/◯ 标记（虚拟文本只能插在列 0 ⇒ 该行整体右移 1 格）",
  },
  inline_values: { type: "boolean", default: false, doc: "(实验)把 Locals 写到行首虚拟文本；只能插在列 0，会把代码右移，默认关" },
  highlight_current_line: {
    type: "boolean",
    default: true,
    doc: "命中时高亮当前执行行(用当前行 scope)",
  },
  current_line_scope: {
    type: "string",
    default: "dap.current-line",
    doc: "当前执行行用的主题 scope;主题没定义时按 cursorline/selection 的底色派生一个",
  },
  max_depth: { type: "number", default: 2, doc: "停止时自动展开的层数(0 = 全折叠)" },
  max_nodes: { type: "number", default: 2000, doc: "变量树节点上限(超出截断,不再发请求)" },
});

function cfg() {
  return helix.get_config("dapui") || {};
}

/** 布尔配置：`cfg()` 只返回用户显式写过的键，所以缺省值要调用方给。 */
function bool_opt(key, fallback) {
  const v = cfg()[key];
  return v === undefined || v === null ? fallback : !!v;
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
  /** 最近一次停止的原因（`step` / `breakpoint` / `exception` …），just-my-code 用 */
  lastStopReason: "",
  /** 监视表达式(跨停止保留,只在本进程内) */
  watches: [],
  /** 变量树过滤串（`/` 设置；空 = 不过滤） */
  filter: "",
  /** 上次 `dap-exited` 的退出码（`terminated` 时决定要不要留着 Output） */
  exit: null,
};

/** 调试视角：停止时记住布局 JSON，继续/结束时恢复。 */
const PERSP = { saved: null };

/** 停止：借走布局（`helix.layout.get()` 的返回值原样存着，退出时回灌）。 */
function apply_perspective() {
  if (!cfg().perspective || PERSP.saved) return;
  try {
    PERSP.saved = helix.layout.get();
  } catch (e) {
    PERSP.saved = null;
  }
}

/** 继续/结束：把布局还回去（只在真存过时动手，别覆盖用户此刻的布局）。 */
function restore_perspective() {
  if (!PERSP.saved) return;
  const saved = PERSP.saved;
  PERSP.saved = null;
  try {
    helix.layout.restore(saved);
  } catch (e) {
    /* 布局恢复失败不该影响调试本身 */
  }
}

/** 面板是否由我们自动打开（终止/退出时负责关掉，别留一个空树）。 */
const AUTO = { tree: false, bottom: false };

/** 只读联动状态：`readonly_on_stop` 打开时由我们置位，继续/结束时**只撤销我们自己置的**（幂等）。 */
const RO = { applied: false };

/** 底部面板：REPL / Output / Breakpoints */
const B = {
  panel_id: null,
  /** "repl" | "output" | "breakpoints" */
  tab: "repl",
  input: "",
  history: [],
  /** REPL 续行中（括号没配平） */
  more: false,
  /** REPL 补全候选（`{ items, idx }`；空 = 没有候选） */
  comp: { items: [], idx: -1 },
  /** 历史是否已尝试加载 */
  hist_loaded: false,
  pos: -1,
  /** REPL 转录（回显 / 结果 / 报错） */
  transcript: [],
  /** DAP output 环形缓冲（上限 500） */
  output: [],
  /** 是否跟随尾部 */
  follow: true,
  /** 断点 tab 的光标（在 `bp_list()` 里的下标） */
  bp_cursor: 0,
};

const OUTPUT_CAP = 500;

/**
 * 焦点回编辑器（面板保留 —— 调试时最常见：看一眼代码再回来）。
 *
 * 两个坑：
 * ① **不能硬编码 pane id 0**（虽然本机编辑器恰好是 0，但那是布局给的，不是契约）⇒ 从
 *    `pane.list()` 里按 kind 找；
 * ② **`C-\` 在传统终端编码里就是 `0x1C`，与 `C-4` 同码** —— 实测本机的 termina 把它解析成
 *    `name:"4", ctrl:true`。所以两个名字都认（否则"回编辑器"这个键在某些终端上永远无效）。
 */
function focus_editor() {
  let id = 0;
  try {
    const list = helix.pane.list() || [];
    const ed = list.find((p) => p.kind === "EditorView");
    if (ed) id = ed.id;
  } catch (e) {
    /* 拿不到 pane 列表：退回 0（老行为） */
  }
  helix.pane.focus(id);
  note_focus(null);
  return "handled";
}

/** `C-\` / `C-4`（同码）都算"回编辑器"。 */
function is_focus_editor_key(key) {
  return !!key.ctrl && (key.name === "\\" || key.name === "4");
}

function say(msg) {
  helix.echo("dapui: " + msg);
}

function err_msg(e) {
  // 核心抛给 JS 的 API 错误是**字符串**（不是 Error 对象）⇒ 两种都要认
  return String(e && e.message ? e.message : e);
}

// ============================ 弹层：输文本 / 选一项 ============================
// 为什么不用 Picker 的 onKey:
//   核心合同——**纯字符键永不交给 picker 的插件钩子**（拾字安全阀，picker.rs:1305）。
//   所以“在 picker 里按 d 删断点”这类需求根本无法实现;而面板/弹层的 onKey 收全部键。
/** 上一次把焦点送进的是哪个面板（`:dapui-focus` 无参数时在两个之间轮换）。 */
let last_panel = "tree";

/// just-my-code：连续 auto-stepOut 的跳数（落到自己代码 / 非单步原因时归零）
const SKIP = { hops: 0, total: 0 };

/// 当前焦点在哪个 dapui 面板里（`"tree"` / `"bottom"` / `null`）。
/// 为什么要自己记:render 的 `focus` 参数是**JS 节点**的焦点（我们的面板是 draw API，
/// 没有节点焦点）⇒ 它永远是 null，不能拿来判断"面板有没有键盘焦点"。
const FOCUS = { where: null };

function note_focus(where) {
  FOCUS.where = where;
}

/** 把焦点还给 dapui 面板（弹层关闭后焦点会回编辑器）。 */
function refocus_panel() {
  if (T.panel_id !== null) {
    last_panel = "tree";
    helix.pane.focus_panel(T.panel_id);
    note_focus("tree");
    return;
  }
  if (B.panel_id !== null) {
    last_panel = "bottom";
    helix.pane.focus_panel(B.panel_id);
    note_focus("bottom");
  }
}

function ask_text(title, initial, on_submit) {
  let value = String(initial === null || initial === undefined ? "" : initial);
  helix.open_popup({
    width: 64,
    height: 6,
    // 样式与 picker-ui 的弹窗一致：圆角边框 + 标题 + 分隔线 + 底部提示。
    // （面板用核心的直角框，**浮层**统一用圆角框 —— 一眼区分"浮在我上面的"和"贴着边的"）
    render: (_focus, ctx) => {
      const w = Math.max(28, (ctx && ctx.width) || 64);
      const h = Math.max(5, (ctx && ctx.height) || 6);
      helix.draw.rect(0, 0, w, h, {
        border: "round",
        title: ` ${title} `,
        style: "ui.popup",
      });
      helix.draw.hline(1, 2, Math.max(0, w - 2), { style: "ui.virtual" });
      // 输入行：超宽时保留**尾部**（光标永远在最后，前面挂 …）
      const room = Math.max(4, w - 8);
      const shown =
        helix.text.width(value) > room
          ? "…" + helix.text.truncate(value.slice(value.length - room + 1), room - 1)
          : value;
      helix.draw.text(
        2,
        1,
        [
          { text: "> ", style: "ui.virtual" },
          { text: shown, style: "ui.text" },
          { text: "█", style: "ui.text.focus" },
        ],
        { maxWidth: Math.max(1, w - 4) },
      );
      helix.draw.text(
        2,
        h - 2,
        helix.text.truncate("Enter 确认（空串 = 清除该项） · Esc 取消", Math.max(8, w - 4)),
        { style: "ui.help" },
      );
      return helix.draw.done();
    },
    onKey: (key) => {
      if (key.ctrl || key.alt) return "handled";
      if (key.name === "Enter") {
        on_submit(value);
        refocus_panel();
        return "close";
      }
      if (key.name === "Esc") return "close";
      if (key.name === "Backspace") {
        value = value.slice(0, -1);
        return "handled";
      }
      if (typeof key.name === "string" && key.name.length === 1) {
        value += key.name;
        return "handled";
      }
      return "handled";
    },
  });
}

/**
 * 多选弹层：`items: [{label, value}]`，`enabled` 是已选集合（Set）。
 * j/k 移动 · Space/Enter 勾选 · Esc 关闭时用最终集合调 `on_done(values[])`。
 */
function ask_multi(title, items, enabled, on_done) {
  let cursor = 0;
  const picked = new Set(enabled || []);
  helix.open_popup({
    width: 60,
    height: Math.min(18, Math.max(6, items.length + 5)),
    render: (_focus, ctx) => {
      const w = Math.max(28, (ctx && ctx.width) || 60);
      const h = Math.max(6, (ctx && ctx.height) || 8);
      helix.draw.rect(0, 0, w, h, {
        border: "round",
        title: ` ${title} `,
        style: "ui.popup",
      });
      helix.draw.hline(1, 2, Math.max(0, w - 2), { style: "ui.virtual" });
      const body = Math.max(1, h - 5);
      const top = Math.min(
        Math.max(0, cursor - Math.floor(body / 2)),
        Math.max(0, items.length - body),
      );
      for (let i = 0; i < body && top + i < items.length; i++) {
        const idx = top + i;
        const it = items[idx];
        const sel = idx === cursor;
        helix.draw.text(
          1,
          3 + i,
          [
            { text: ` ${sel ? "›" : " "} ${picked.has(it.value) ? "[x]" : "[ ]"} `, style: "ui.virtual" },
            { text: helix.text.truncate(String(it.label || ""), Math.max(4, w - 8)), style: null },
          ],
          { style: sel ? "ui.selection" : null, maxWidth: Math.max(1, w - 2) },
        );
      }
      helix.draw.text(
        2,
        h - 2,
        helix.text.truncate("Space/Enter 勾选 · j/k 移动 · Esc 保存关闭", Math.max(8, w - 4)),
        { style: "ui.help" },
      );
      return helix.draw.done();
    },
    onKey: (key) => {
      if (key.ctrl || key.alt) return "handled";
      switch (key.name) {
        case "Esc":
          on_done([...picked]);
          return "close";
        case "j":
        case "Down":
          cursor = Math.min(items.length - 1, cursor + 1);
          return "handled";
        case "k":
        case "Up":
          cursor = Math.max(0, cursor - 1);
          return "handled";
        case "Enter":
        case " ": {
          const it = items[cursor];
          if (!it) return "handled";
          if (picked.has(it.value)) picked.delete(it.value);
          else picked.add(it.value);
          return "handled";
        }
        default:
          return "handled";
      }
    },
  });
}

/** items: `[{label, payload}]`;`on_pick(payload)` 只在 Enter 时调。 */
function ask_choice(title, items, on_pick) {
  let cursor = 0;
  helix.open_popup({
    width: 60,
    height: Math.min(16, Math.max(3, items.length + 2)),
    render: () => {
      const rows = items.map((it, i) =>
        helix.el("text", (i === cursor ? "▸ " : "  ") + it.label, {
          style: i === cursor ? "ui.selection" : "ui.text",
        }),
      );
      return helix.el("col", [helix.el("text", title, { style: "ui.virtual" }), ...rows], {
        gap: 1,
      });
    },
    onKey: (key) => {
      if (key.ctrl || key.alt) return "handled";
      switch (key.name) {
        case "Esc":
          return "close";
        case "j":
        case "Down":
          cursor = Math.min(items.length - 1, cursor + 1);
          return "handled";
        case "k":
        case "Up":
          cursor = Math.max(0, cursor - 1);
          return "handled";
        case "Enter": {
          const it = items[cursor];
          if (it) on_pick(it.payload);
          refocus_panel();
          return "close";
        }
        default:
          return "handled";
      }
    },
  });
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

/// nerd-font 图标（与主题/其它段一致的字体族；别用 `⏸ ▶ ⏳ ■` 这种会跟图标字体不搭）。
function state_icon(state) {
  switch (state) {
    case "stopped":
      return "\uf04c"; // pause
    case "running":
      return "\uf04b"; // play
    case "starting":
      return "\uf017"; // clock
    default:
      return "\uf04d"; // stop
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

/** 分组/标记文案（`[plugins.dapui] labels = "en" | "zh-CN"`）。 */
const LABELS = {
  en: {
    watches: "Watches",
    systemFrames: (n) => `… +${n} system frames`,
    noSource: "(no source)",
    expensive: "expensive",
    noSession: "no session",
    sessions: (n) => ` (${n} sessions)`,
    switchHint: "Enter: switch to this session / expand for frames",
    colName: "name",
    colValue: "value",
    colType: "type",
    empty: "(empty)",
    noMatch: "no match",
  },
  "zh-CN": {
    watches: "监视",
    systemFrames: (n) => `… 还有 ${n} 个系统帧`,
    noSource: "(无源码)",
    expensive: "expensive",
    noSession: "没有会话",
    sessions: (n) => ` (${n} 个会话)`,
    switchHint: "按 Enter 切到该会话 / 展开看栈帧",
    colName: "名字",
    colValue: "值",
    colType: "类型",
    empty: "(空)",
    noMatch: "没有匹配",
  },
};

/** adapter 给的作用域名 → 当前语言的分组名（认不出的原样保留，比如 V8 的 "Closure"）。 */
const SCOPE_NAMES = {
  locals: { en: "Locals", "zh-CN": "局部" },
  globals: { en: "Globals", "zh-CN": "全局" },
  registers: { en: "Registers", "zh-CN": "寄存器" },
  arguments: { en: "Arguments", "zh-CN": "参数" },
  meta: { en: "Meta", "zh-CN": "元信息" },
  watches: { en: "Watches", "zh-CN": "监视" },
};

function L() {
  const want = String(cfg().labels || "en");
  return LABELS[want] || LABELS.en;
}

function scope_label(name) {
  const key = String(name || "").toLowerCase();
  const row = SCOPE_NAMES[key];
  if (!row) return String(name || "");
  return row[String(cfg().labels || "en")] || row.en;
}

function reset_tree() {
  T.roots = [];
  T.nodes = [];
  T.cache = {};
  T.cursor = 0;
}

/**
 * 拉树。`opts.keep = {thread?, frame?}`：面板里手动切过线程/帧时要尊重它，
 * 否则会被会话快照里的“引擎当前线程/帧”盖回去（`dap-stopped` 不带 keep ⇒ 回到引擎那个）。
 */
function load_roots(opts) {
  watches_load(); // 首次建树时把持久化的监视表达式读进来
  const session = active_session();
  const keep = (opts && opts.keep) || {};
  reset_tree();
  if (!session || session.thread === null || session.thread === undefined) {
    T.ctx = null;
    return;
  }
  if (session.state !== "stopped") {
    T.ctx = null;
    return;
  }
  const thread = keep.thread !== undefined && keep.thread !== null ? keep.thread : session.thread;
  const frame = keep.frame !== undefined && keep.frame !== null ? keep.frame : session.frame ? session.frame.id : null;
  T.ctx = { id: session.id, thread, frame };
  T.roots = [{ id: "loading", depth: 0, label: "…", value: "loading" }];
  flatten();

  helix.dap
    .stack_frames(thread)
    .then((frames) => {
      const list = frames || [];
      // just-my-code：**单步**落到工作区外的源码（std/core/系统库）就自动 stepOut。
      // 只对 `reason === "step"` 生效 —— 断点/异常停在库里时必须原地停住给你看。
      if (list.length > 0 && bool_opt("just_my_code", false) && T.lastStopReason === "step") {
        const top = list[0];
        if (top && top.source && !in_workspace(top.source) && SKIP.hops < 12) {
          SKIP.hops += 1;
          SKIP.total += 1;
          helix.dap
            .request("stepOut", { threadId: thread })
            .catch(() => {
              /* adapter 不支持就放弃，别卡住 */
              SKIP.hops = 12;
            });
          return; // 这次中间帧不画（下一拍会再来一次 dap-stopped）
        }
        if (top && (!top.source || in_workspace(top.source))) {
          if (SKIP.total > 0) {
            say(`已跳过 ${SKIP.total} 层库代码（just_my_code）`);
          }
          SKIP.hops = 0;
          SKIP.total = 0;
        }
      }
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
      // 装饰（当前行高亮/内联值）依赖上面这两个字段 —— 它们是**异步**才有的，
      // 所以在栈帧回来之后才重写（handler 里同步调会拿到 undefined ⇒ 什么都不画）。
      refresh_decorations();

      // 栈帧分组：**有源码的**（你自己的代码）平铺；**没有源码的**（libc/加载器里的
      // `__libc_start_main@@GLIBC_2.34`、`_start` …）折叠成一行 —— 否则树里一半是噪音。
      // 当前帧永远在第一个（即使它没有源码），因为 scopes 用的是它。
      const maxFrames = Math.max(0, Math.floor(Number(cfg().max_frames) || 0));
      const current = ordered[0];
      // 系统帧：没源码 / 仍是 mangled / **源码不在当前工作区**（std/core 的库源码）。
      // 最后那条是关键：核心已经 demangle 过，`call_once`/`lang_start` 看着像人话，
      // 但它们是 std 的实现，不该和你的 `main` 挤在顶层。
      const isSystem = (f) => !f.source || looks_mangled(f.name) || !in_workspace(f.source);
      const withoutSource = ordered.filter((f) => isSystem(f) && f.id !== current.id);
      const withSource = ordered.filter((f) => !isSystem(f) && f.id !== current.id);
      // 缩短后的名字：同屏重名时补 `(模块)` 消歧（只给重名的那几个加）
      const mode = String(cfg().frame_name || "short");
      // 只统计**会显示在顶层**的帧（`current` + 有源码的）——
      // 折叠进系统组的那些同名帧（lldb 会给 `main` 额外一帧无源码的）不该触发消歧
      const shown = [current, ...withSource].map((f) => frame_display(f.name, mode));
      const counts = {};
      for (const n of shown) counts[n] = (counts[n] || 0) + 1;
      const labelOf = (f) => {
        let n = frame_display(f.name, mode);
        // 重名才补 `(模块)`；同一个函数出现在多帧（递归/回调）不算重名
        if (mode !== "full" && counts[n] > 1) {
          const segs = strip_generics(String(f.name || "")).split("::").filter(Boolean);
          const plain = segs.length > 1 ? segs[segs.length - 2] : "";
          // 只有**真的是另一个函数**才算重名：同一个函数的多帧
          // （`rsdbg::main` 与 `rsdbg::main::{closure#0}`）不算。
          const mine = strip_generics(String(f.name || ""));
          const other = [current, ...withSource].some((g) => {
            if (g === f || frame_display(g.name, mode) !== n) return false;
            const them = strip_generics(String(g.name || ""));
            return !them.startsWith(mine) && !mine.startsWith(them);
          });
          if (other && plain && plain !== n) n = `${n} (${plain})`;
        }
        return n.endsWith(")") ? n : `${n}()`;
      };
      const frameNode = (f) => ({
        id: "f" + f.id,
        depth: 0,
        kind: "frame",
        // 名字后面加 `()`：一眼区分"函数"和"变量"
        label: labelOf(f),
        value: "",
        hint: f.source
          ? `${short_path(f.source)}:${f.line + 1}`
          : L().noSource,
        frame: f.id,
        expandable: true,
        expanded: false,
      });
      const limit = maxFrames > 0 ? maxFrames : withSource.length;
      let roots = [frameNode(current)]
        .concat(withSource.slice(0, limit).map(frameNode));
      if (withoutSource.length > 0) {
        roots.push({
          id: "sysframes",
          depth: 0,
          kind: "note",
          label: L().systemFrames(withoutSource.length),
          value: "",
          expandable: true,
          expanded: false,
          children: withoutSource.map((f) => ({ ...frameNode(f), depth: 1 })),
        });
      }
      // ── 多会话：每个会话一个节点（当前展开，其余折叠）──
      // 单会话时保持原来的"帧直接铺在顶层"（零回归）。
      const allSessions = helix.dap.sessions() || [];
      if (cfg().show_all_sessions !== false && allSessions.length > 1) {
        const head = (sess) =>
          `#${sess.id} ${state_icon(sess.state)}` +
          `${sess.thread !== null && sess.thread !== undefined ? " t" + sess.thread : ""}` +
          `${sess.active ? " (当前)" : ""}`;
        const others = allSessions
          .filter((sess) => sess.id !== session.id)
          .map((sess) => ({
            id: "s" + sess.id,
            depth: 0,
            label: head(sess),
            value: sess.name || "",
            session: sess.id,
            thread: sess.thread,
            expandable: sess.thread !== null && sess.thread !== undefined,
            expanded: false,
            children: [
              {
                id: "s" + sess.id + "/ph",
                depth: 1,
                kind: "note",
                label: L().switchHint,
                value: "",
              },
            ],
          }));
        roots = [
          {
            id: "s" + session.id,
            depth: 0,
            label: head(session),
            value: "",
            session: session.id,
            expandable: true,
            expanded: true,
            children: roots.map((r) => ({ ...r, depth: 1 })),
          },
          ...others,
        ];
      }

      // 监视组永远在树顶（有监视时才出现）
      if (T.watches.length > 0) {
        const watchGroup = {
          id: "watches",
          depth: 0,
          kind: "group",
          label: `${L().watches} (${T.watches.length})`,
          value: "",
          expandable: true,
          expanded: true,
          children: [],
        };
        roots.unshift(watchGroup);
        T.roots = roots;
        refresh_watches(watchGroup);
      } else {
        T.roots = roots;
      }
      flatten();
      const currentRoot = roots.find((r) => r.frame === T.ctx.frame);
      helix.dap
        .scopes(T.ctx.frame)
        .then((scopes) => {
          if (!currentRoot) return;
          currentRoot.children = map_scopes(scopes, currentRoot);
          currentRoot.loaded = true;
          T.cache[currentRoot.id] = currentRoot.children;
          currentRoot.expanded = true;
          flatten();
          const depth = Math.max(0, Math.floor(Number(cfg().max_depth) || 0));
          if (depth >= 2) {
            const first = currentRoot.children[0];
            if (first) expand_node(first, depth - 1);
          }
          // 自动展开了 Locals ⇒ 变量进了缓存 ⇒ 立刻把内联值画上（别等下一次刷新）
          apply_inline_values();
        })
        .catch((e) => {
          if (!currentRoot) return;
          currentRoot.children = [
            { id: "serr", depth: 1, label: "!! " + err_msg(e), value: "", error: true },
          ];
          currentRoot.expanded = true;
          flatten();
        });
    })
    .catch((e) => {
      T.roots = [{ id: "ferr", depth: 0, label: "!! " + err_msg(e), value: "", error: true }];
      flatten();
    });
}

/** scopes → 树子节点（帧节点的孩子；`load_roots` 与 `expand_node` 共用）。 */
function map_scopes(scopes, parent) {
  return (scopes || [])
    // 明确报"0 个"的作用域不占地方（Registers/Globals 常常是空的）
    .filter((sc) => {
      const named = sc.namedVariables;
      const indexed = sc.indexedVariables;
      return !(named === 0 && (indexed === undefined || indexed === null || indexed === 0));
    })
    .map((sc) => ({
      id: parent.id + "/s" + sc.variablesReference,
      depth: parent.depth + 1,
      kind: "scope",
      // 原始名留着：`locals_summary`/内联值要按 `Locals` 找这个作用域（标签可能被翻译）
      raw_name: sc.name,
      label: scope_label(sc.name),
      value: "",
      hint: sc.expensive ? L().expensive : "",
      ref: sc.variablesReference,
      expandable: true,
      expanded: false,
    }));
}

/** 变量 → 树子节点。 */
/** 变量一页取多少（`supportsVariablePaging` 打开后按 start/count 请求）。 */
const VAR_PAGE = 200;
/** 最多翻多少页（200×20 = 4000 项；再多的部分靠 `/` 过滤或调大 `max_nodes`）。 */
const VAR_MAX_PAGES = 20;

/** 「… 更多」节点：展开它 = 拉下一页并追加到父节点。 */
function more_node(parent, ref, start) {
  return {
    id: parent.id + "/more@" + start,
    depth: parent.depth + 1,
    label: `… 更多（start=${start}，按 l 加载）`,
    value: "",
    more: { ref, start },
    parent_node: parent,
    expandable: true, // 按 l/Enter 触发"拉下一页"（expand_node 里的 more 分支）
  };
}

function map_variables(vars, parent) {
  return (vars || []).map((v) => ({
    id: parent.id + "/" + v.name,
    depth: parent.depth + 1,
    kind: "var",
    label: v.name,
    // 值、类型、子项数**分开存**：渲染时才按列拼（早先拼进一个字符串 ⇒ 没法对齐）
    value: v.value === null || v.value === undefined ? "" : String(v.value),
    // 原始值（`e` 设值时的初值：不带 ` :类型` 后缀）
    raw: v.value,
    type: v.type || "",
    hint: count_hint(v),
    ref: v.variablesReference,
    // 父容器的 variablesReference：`setVariable` 要用它定位
    parent_ref: parent.ref,
    expandable: (v.variablesReference || 0) > 0,
    expanded: false,
  }));
}

/** `(3)` = 有 3 个具名子项；`[64]` = 有 64 个索引子项。 */
function count_hint(v) {
  if (v.namedVariables) return `(${v.namedVariables})`;
  if (v.indexedVariables) return `[${v.indexedVariables}]`;
  return "";
}

/** 把树按"可见"展平成 `T.nodes`（折叠的子节点不进列表）。 */
function flatten() {
  const out = [];
  const budget = num("max_nodes", 2000);
  let truncated = false;
  const f = String(T.filter || "").trim().toLowerCase();
  /** 过滤：自身命中，或某个后代命中（否则整枝都不显示） */
  const keep = (node) => {
    if (!f) return true;
    const hay = `${node.label || ""} ${node.value || ""}`.toLowerCase();
    if (hay.includes(f)) return true;
    return (node.children || []).some(keep);
  };
  const walk = (list) => {
    for (const node of list || []) {
      if (out.length >= budget) {
        truncated = true;
        return;
      }
      if (!keep(node)) continue;
      out.push(node);
      // 过滤时自动展开命中的分支（否则命中项藏在折叠的祖先里看不到）
      if ((f ? true : node.expanded) && node.children) walk(node.children);
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
  if (out.length === 0 && f) {
    out.push({
      id: "__no_match",
      depth: 0,
      kind: "note",
      label: `${L().noMatch}: ${T.filter}`,
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
  // 帧节点（`frame` 有值、`ref` 没有）展开的是**作用域**，不是 variables ——
  // 以前这里直接 `variables(node.ref)` 会抛 `'reference' is required`（面板上表现为 onKey 出错）。
  let request;
  // 「更多…」节点：拉**下一页**变量并追加（`start` = 已有条数）
  if (node.more) {
    const { ref, start } = node.more;
    helix.dap
      .variables(ref, undefined, start, VAR_PAGE)
      .then((vars) => {
        const parent = node.parent_node;
        if (!parent) return;
        // **去重**：adapter 若忽略 `start`（老版本/不支持的实现）会返回同一页 ⇒
        // 不能把它当"还有更多"一路追加（否则树里堆重复项）。
        const seen = new Set((parent.children || []).map((c) => c.name || c.label));
        const list = map_variables(vars || [], node).filter((c) => {
          const key = c.name || c.label;
          if (seen.has(key)) return false;
          seen.add(key);
          return true;
        });
        const pages = (parent.pages || 1) + 1;
        parent.pages = pages;
        parent.children = (parent.children || []).filter((c) => c.id !== node.id).concat(list);
        const full = (vars || []).length === VAR_PAGE;
        if (list.length === 0) {
          // 一页里全是重复 ⇒ 这个 adapter 不认分页，别再加"更多"
          parent.children.push({
            id: parent.id + "/more-end",
            depth: parent.depth + 1,
            label: "… （adapter 不支持分页，已到能取的最后）",
            value: "",
          });
        } else if (full && pages < VAR_MAX_PAGES) {
          parent.children.push(more_node(parent, ref, start + VAR_PAGE));
        }
        parent.loaded = true;
        T.cache[parent.id] = parent.children;
        flatten();
        apply_inline_values();
      })
      .catch((e) => {
        node.label = "!! " + err_msg(e);
        node.error = true;
        flatten();
      });
    return;
  }
  // 会话节点（非当前会话）：拉它的栈帧列表（不带 scopes —— 切过去才需要变量）
  if (node.session && node.session !== (T.ctx && T.ctx.id)) {
    const tid = node.thread;
    if (tid === null || tid === undefined) {
      node.expandable = false;
      flatten();
      return;
    }
    helix.dap
      .stack_frames(tid, node.session)
      .then((frames) => {
        node.children = (frames || []).map((f) => ({
          id: `s${node.session}/f${f.id}`,
          depth: node.depth + 1,
          label: `${f.name}  ${short_path(f.source)}:${f.line + 1}`,
          value: "",
          session: node.session,
          expandable: false,
        }));
        node.loaded = true;
        node.expanded = true;
        flatten();
      })
      .catch((e) => {
        node.children = [
          { id: node.id + "/err", depth: node.depth + 1, label: "!! " + err_msg(e), value: "", error: true },
        ];
        node.loaded = true;
        node.expanded = true;
        flatten();
      });
    return;
  }
  if (node.ref !== undefined && node.ref !== null) {
    request = helix.dap.variables(node.ref, undefined, 0, VAR_PAGE).then((vars) => {
      const list = map_variables(vars || [], node);
      node.pages = 1;
      // 展开后**真的是空的** ⇒ 标一下（Globals/Registers 经常空；别让人以为还在加载）
      node.hint = (vars || []).length === 0 ? L().empty : "";
      // 分页：拿满一页 ⇒ 可能还有（adapter 只在支持分页时按 start/count 回）
      if ((vars || []).length === VAR_PAGE && node.pages < VAR_MAX_PAGES) {
        list.push(more_node(node, node.ref, VAR_PAGE));
      }
      return list;
    });
  } else if (node.frame !== undefined && node.frame !== null) {
    request = helix.dap.scopes(node.frame).then((scopes) => map_scopes(scopes, node));
  } else {
    node.expandable = false;
    node.loading = false;
    flatten();
    return;
  }
  node.loading = true;
  flatten();
  request
    .then((children) => {
      node.children = children;
      T.cache[node.id] = node.children;
      node.loaded = true;
      node.expanded = true;
      node.loading = false;
      flatten();
      // 变量进缓存 ⇒ 行尾内联值可能变了（`a=2 b=3`），立刻重画
      apply_inline_values();
      if (auto_depth > 0) {
        // 自动展开只跟"真变量/作用域"，**不要**顺手把「… 更多」也点了（会把整棵树拉满）
        const first = node.children.find((c) => c.expandable && !c.more);
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
  const { w, h } = panel_box(focus, ctx, num("panel_size", 40), 20);
  // 面板没焦点时不要把选中行画成亮底（否则整栏像一条彩色带，跟别处分不清）；
  // 焦点在面板里才用 `ui.selection`，同时**永远**加 `›` 记号 —— 单色终端也看得出选中。
  const focused = FOCUS.where === "tree";
  const session = active_session();
  const sessCount = (helix.dap.sessions() || []).length;
  const head = session_badge() + (sessCount > 1 ? L().sessions(sessCount) : "");
  const program = (session && session.program) || "";
  // 顶部摘要默认不显示（展开 Locals 就有同样信息）；要的话配 locals_summary = true
  const locals = bool_opt("locals_summary", false) ? locals_summary(6) : "";
  helix.draw.text(
    0,
    0,
    [
      // 徽标也用 `ui.text`：`ui.text.focus` 在很多主题里带 bg（整行一条色带）
      { text: ` ${session ? head : L().noSession}`, style: session ? "ui.text" : "ui.virtual" },
      // 被调试程序名（多会话时最有用）：暗色，跟在会话徽标后面
      ...(program ? [{ text: " · " + program, style: "ui.virtual" }] : []),
      ...(locals ? [{ text: "  │ " + locals, style: "ui.virtual" }] : []),
    ],
    { style: null },
  );
  // 可选列头（默认关：多占一行，小屏不值）。用与正文**同一套列宽**才对得上。
  const showHead = bool_opt("columns_header", false);
  if (showHead) {
    const hc = compute_columns(T.nodes, w);
    const t = L();
    helix.draw.text(
      0,
      1,
      [
        // 与变量行**逐列对齐**：缩进 + 名字列 + ` = ` + 值 + 两格 + 类型
        { text: " ".repeat(Math.max(2, hc.indent)), style: "ui.virtual" },
        { text: helix.text.pad(t.colName, hc.name), style: "ui.virtual" },
        { text: " = " + t.colValue, style: "ui.virtual" },
        ...(hc.type ? [{ text: "  " + t.colType, style: "ui.virtual" }] : []),
      ],
      { style: null },
    );
  }
  // 有焦点才画一行键位提示（没焦点时省下这一行给内容）
  const showFoot = focused && h > 6;
  const tree_h = Math.max(1, h - 1 - (showHead ? 1 : 0) - (showFoot ? 1 : 0));
  const tree_y0 = showHead ? 2 : 1;
  const total = T.nodes.length;
  let top = 0;
  if (total > tree_h) {
    top = Math.min(Math.max(0, T.cursor - Math.floor(tree_h / 2)), Math.max(0, total - tree_h));
  }
  const sel = T.nodes[T.cursor];
  const visible = T.nodes.slice(top, top + tree_h);
  const cols = compute_columns(visible, w);
  for (let i = 0; i < visible.length; i++) {
    const node = visible[i];
    const spans = tree_spans(node, cols, node === sel, focused);
    helix.draw.text(0, tree_y0 + i, spans, { style: null });
  }
  if (showFoot) {
    helix.draw.text(
      0,
      h - 1,
      helix.text.truncate(" j/k 移动 · l/Enter 展开 · w 监视 · e 设值 · y 复制 · / 过滤 · q 关", w),
      { style: "ui.help" },
    );
  }
  return helix.draw.done();
}

/** 名字列宽：按可见节点的名字自适应（上限 45%），至少 8 列。 */
function compute_columns(nodes, w) {
  // 缩进预算按**可见节点的最大深度**算（各行列必须共用同一套列宽）
  // 1(记号列) + 缩进 + 2(折叠符)
  const indentOf = (n) => 1 + Math.min(n.depth || 0, 4) + 2;
  let indent = 2;
  for (const n of nodes) indent = Math.max(indent, indentOf(n));
  let nameMax = 0;
  let typeMax = 0;
  for (const n of nodes) {
    // 只有变量进名字列；帧/分组用的是"名字 + 右对齐位置"，不进这一列
    if (n.kind === "var" && !n.is_watch) {
      nameMax = Math.max(nameMax, helix.text.width(String(n.label || "")));
    }
    if (n.type) typeMax = Math.max(typeMax, helix.text.width(String(n.type)));
    if (n.hint) typeMax = Math.max(typeMax, helix.text.width(String(n.hint)));
  }
  // 名字列上限 20：标识符再长也长不过这（监视表达式另算，见 `is_watch`）
  const maxName = Math.max(6, Math.min(14, Math.floor(w * 0.4)));
  const name = Math.max(4, Math.min(nameMax, maxName));
  // 值优先：值至少 12 列；类型列拿剩下的（放不下就整段省略，别把值挤成 `0…`）
  const base = Math.max(8, w - (indent + name + 3));
  const type = Math.max(0, Math.min(typeMax, base - 12 - 2));
  const value = Math.max(6, base - (type ? type + 2 : 0));
  // 帧/说明的右列（文件:行）：最多 40%
  const right = Math.max(0, Math.min(typeMax, Math.floor(w * 0.4)));
  return { name, type, right, value, w, indent };
}

/** 一行 = 若干段样式（缩进/名字/`= 值`/右列），这样才能按列对齐。 */
function tree_spans(node, cols, selected, focused) {
  // 注意：**不用 `ui.text.focus`** —— 很多主题给这个 scope 配了 `bg`
  // （实测用户主题是 323843，于是"帧/作用域"每行背后都有一条色带，跟选中行撞色）。
  // 层级只靠 `▾/▸` 记号 + 暗色右列表达。
  const style = selected
    ? focused
      ? "ui.selection"
      : "ui.text"
    : node.error
      ? "error"
      : node.kind === "note"
        ? "ui.virtual"
        : "ui.text";
  const indent = " ".repeat(Math.min(node.depth || 0, 4));
  const deep = (node.depth || 0) > 3 ? "┆" : "";
  const mark = node.expandable ? (node.expanded ? "▾ " : "▸ ") : "  ";
  // 选中记号 `›` 占折叠符左边的**一列**（所有行都留出来，避免整体右移）
  const caret = selected ? "›" : " ";
  // 记号在最左（固定列），后面才是缩进 —— 选中行一眼能看到，且不挤名字
  const spans = [{ text: `${caret}${indent}${deep}${mark}`, style: "ui.virtual" }];

  const label = String(node.label || "");
  const isVar = node.kind === "var";
  if (node.is_watch) {
    // 监视：`表达式 = 值`，表达式按剩余宽度截断（不进名字列，免得整栏被一条长表达式撑开）
    const used = spans.reduce((n, sp) => n + helix.text.width(sp.text), 0);
    const exprRoom = Math.max(10, Math.min(helix.text.width(label), Math.floor(cols.w * 0.45)));
    spans.push({ text: helix.text.truncate(label, exprRoom), style });
    spans.push({ text: " = ", style: "ui.virtual" });
    spans.push({ text: fit_value(String(node.value || "?"), Math.max(6, cols.value)), style });
    if (node.hint) spans.push({ text: "  " + helix.text.truncate(node.hint, cols.right || 24), style: "ui.virtual" });
    if (node.loading) spans.push({ text: " …", style: "ui.virtual" });
    return spans;
  }
  if (node.value) {
    // 变量：默认**名字后一空格**（`a = 2`）；要按列对齐再开 `align_columns`
    const nameW = bool_opt("align_columns", false) ? cols.name : 0;
    spans.push({
      text: nameW ? helix.text.pad(helix.text.truncate(label, nameW), nameW) : helix.text.truncate(label, Math.max(4, cols.name + 8)),
      style,
    });
    spans.push({ text: " = ", style: "ui.virtual" });
    spans.push({ text: fit_value(trim_value(node.value, node.type), cols.value), style });
  } else {
    // 帧/分组/说明：名字后面留两格，右列再补
    spans.push({ text: label + (node.hint ? "  " : ""), style });
  }
  if (isVar) {
    // 变量：类型**紧跟值**（留两格）—— 右对齐会在宽栏里拉出很长的空白。
    // 类型按剩余宽度截断（否则会被面板右边裁掉，看起来像"类型只有一半"）
    if (node.type) {
      const ty = String(node.type);
      // 放得下就整段显示；放不下就**整段省略**（截一半的类型比没有更误导）
      if (cols.type && helix.text.width(ty) <= cols.type) {
        spans.push({ text: "  " + ty, style: "ui.virtual" });
      }
    }
  } else {
    // 帧/说明：位置/提示**右对齐**到面板右边（看起来像一列）
    const rightText = helix.text.truncate(String(node.hint || ""), Math.max(4, cols.right || 12));
    if (rightText) {
      const used = spans.reduce((n, sp) => n + helix.text.width(sp.text), 0);
      const gap = Math.max(1, cols.w - used - helix.text.width(rightText));
      spans.push({ text: " ".repeat(gap), style: "ui.virtual" });
      spans.push({ text: rightText, style: "ui.virtual" });
    }
  }
  if (node.loading) spans.push({ text: " …", style: "ui.virtual" });
  return spans;
}

/** 值里重复了类型名就去掉（lldb 对 struct/Vec 常给 `Type @ 0xaddr` 或干脆只有类型名）。
 *
 * 例：`rsdbg::Point @ 0x7fff…` + 类型 `rsdbg::Point` ⇒ `…`（展开就能看到字段）
 *     `alloc::vec::Vec<int, Global>` + 同类型 ⇒ `…`
 *     其余原样返回（不要瞎改用户数据）。
 */
function trim_value(value, type) {
  const v = String(value === null || value === undefined ? "" : value);
  const t = String(type || "").trim();
  if (!v || !t) return v;
  const norm = (s) => s.replace(/\s+/g, " ").trim();
  const nv = norm(v);
  const nt = norm(t);
  if (nv === nt) return "…";
  if (!nv.startsWith(nt)) return v;
  let rest = nv.slice(nt.length).trim();
  // `Type @ 0x1234` 里的纯地址没有信息量（要地址就展开看）
  rest = rest.replace(/^@\s*0x[0-9a-fA-F]+$/, "").trim();
  return rest || "…";
}

/** 值放不下时截断，并标出"还有多少字符"（`"hello …(+42)`）。 */
function fit_value(text, width) {
  const w = Math.max(4, width);
  const total = helix.text.width(text);
  if (total <= w) return text;
  // 留出 `…(+N)` 的位置
  const suffix = `…(+${total - w + 3})`;
  const room = Math.max(1, w - helix.text.width(suffix));
  return helix.text.truncate(text, room) + suffix;
}

/** 选中文本（单行选择时直接拿来做监视表达式的初值）。 */
function selection_text(doc) {
  if (!doc || !doc.selection || !doc.text) return "";
  const a = doc.selection.anchor;
  const h = doc.selection.head;
  if (!a || !h || a.row !== h.row) return "";
  const [from, to] = [Math.min(a.col, h.col), Math.max(a.col, h.col)];
  const line = String(doc.text).split("\n")[a.row] || "";
  return line.slice(from, to).trim();
}

function add_watch(doc) {
  ask_text("监视表达式（每命中一次重算）", selection_text(doc), (expr) => {
    const name = String(expr || "").trim();
    if (!name) return;
    if (!T.watches.includes(name)) T.watches.push(name);
    watches_save();
    load_roots();
  });
}

/** 改变量值（`setVariable`；lldb-dap `supportsSetVariable ✓`）。走 `request` 逃生口，不加 API。 */
function set_variable(node, value) {
  helix.dap
    .request("setVariable", {
      variablesReference: node.parent_ref,
      name: node.label,
      value,
    })
    .then((r) => {
      say(`${node.label} = ${(r && r.value) || value}`);
      node.value = (r && r.value) || value;
      flatten();
    })
    .catch((e) => say("改变量失败: " + err_msg(e)));
}

/** 监视值：每次停止/切帧都重算（`eval(expr, frame)`），失败显示成红色节点。 */
function refresh_watches(parent) {
  if (T.watches.length === 0) return;
  const nodes = T.watches.map((expr, i) => ({
    id: "w" + i,
    depth: 1,
    kind: "var",
    is_watch: true, // 表达式不是标识符：不参与名字列宽、也不按列 padding（否则一行能撑爆整栏）
    label: expr,
    value: "…",
    type: "",
    error: false,
  }));
  parent.children = nodes;
  parent.expanded = true;
  flatten();
  if (!T.ctx) {
    for (const n of nodes) n.value = "(没有停止的会话)";
    flatten();
    return;
  }
  T.watches.forEach((expr, i) => {
    helix.dap
      .eval(expr, T.ctx ? T.ctx.frame : undefined)
      .then((r) => {
        nodes[i].value =
          r && r.result !== undefined && r.result !== null ? String(r.result) : "";
        nodes[i].type = (r && r.type) || "";
        nodes[i].hint = "";
      })
      .catch((e) => {
        // 值位置只放一个 `?`：完整错误太长，塞在值列会把整行挤爆。
        // 原因放右列（暗色）—— 一眼能看出"没这个变量"还是"表达式写错"。
        nodes[i].value = "?";
        nodes[i].hint = short_err(e);
        nodes[i].error = true;
      })
      .then(() => flatten());
  });
}

function key_tree(key, doc) {
  if (key.ctrl || key.alt) {
    if (is_focus_editor_key(key)) return focus_editor();
    return "ignore";
  }
  switch (key.name) {
    // Esc = 焦点回编辑器（**面板保留**）：调试时"看一眼代码"最常用的动作。
    case "Esc":
      return focus_editor();
    case "q":
      close_tree();
      return "handled";
    // ── 树内导航（只跟"选中项"有关，编辑器里没有对应语义）──
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
    case "Enter": {
      const node = T.nodes[T.cursor];
      // 选中**别的会话**：Enter = 切过去（比记住 S 弹层快）
      if (node && node.session && T.ctx && node.session !== T.ctx.id) {
        helix.dap
          .switch_session(node.session)
          .then(() => load_roots())
          .catch((e) => say("切会话失败: " + err_msg(e)));
        return "handled";
      }
      expand_node(node, 0);
      return "handled";
    }
    case "Left":
    case "h":
      collapse_node(T.nodes[T.cursor]);
      return "handled";
    case "g":
      T.cursor = 0;
      return "handled";
    case "G":
      T.cursor = Math.max(0, T.nodes.length - 1);
      return "handled";
    // ── 与选中项有关、编辑器里没有的动作 ──
    case "w":
      add_watch(doc);
      return "handled";
    case "W":
      T.watches = [];
      watches_save();
      load_roots();
      say("已清空监视");
      return "handled";
    case "/": {
      ask_text("搜索变量（名字或值；空 = 清除）", T.filter || "", (v) => {
        T.filter = String(v || "").trim();
        flatten();
        if (!T.filter) {
          say("搜索已清除");
          return;
        }
        // **跳转**：过滤后把光标放到第一个"自己命中"的节点（而不是祖先）
        const idx = T.nodes.findIndex((n) => own_match(n));
        if (idx >= 0) T.cursor = idx;
        const hits = T.nodes.filter((n) => own_match(n)).length;
        say(
          hits > 0
            ? `搜索: ${T.filter}（${hits} 处；n/N 跳下一个/上一个）`
            : `搜索: ${T.filter}（没有匹配）`,
        );
      });
      return "handled";
    }
    case "n": {
      if (!T.filter) return "ignore"; // 没在搜索 ⇒ 交回全局（n = 单步跳过）
      return jump_match(1);
    }
    case "N": {
      if (!T.filter) return "ignore";
      return jump_match(-1);
    }
    case "y": {
      const node = T.nodes[T.cursor];
      // 帧节点：复制 `file:line`（"这个帧在哪"，贴到 issue/聊天里常用）
      if (node && node.frame && T.ctx && T.ctx.source) {
        helix.set_register(`${T.ctx.source}:${(T.ctx.line || 0) + 1}`);
        say(`已复制 ${short_path(T.ctx.source)}:${(T.ctx.line || 0) + 1}`);
        return "handled";
      }
      if (!node || (!node.frame && !node.parent_ref)) return "ignore";
      const text = String(node.raw ?? node.value ?? "");
      if (!text) return "ignore";
      helix.set_register(text);
      say(`已复制 ${node.label} = ${text}`);
      return "handled";
    }
    case "d": {
      const node = T.nodes[T.cursor];
      const idx = node && /^w\d+$/.test(node.id) ? Number(node.id.slice(1)) : -1;
      if (idx >= 0 && idx < T.watches.length) {
        const removed = T.watches.splice(idx, 1)[0];
        watches_save();
        load_roots();
        say(`已删监视 ${removed}`);
        return "handled";
      }
      return "ignore";
    }
    case "e": {
      const node = T.nodes[T.cursor];
      if (node && node.parent_ref) {
        ask_text(`设值 ${node.label} =`, String(node.raw ?? node.value ?? ""), (v) =>
          set_variable(node, String(v)),
        );
        return "handled";
      }
      return "ignore";
    }
    // ── 会话动作（n/s/o/c/p/b/t/f/S/v/R/K/T/r/l）**一律不在这里处理** ──
    // 它们冒泡给编辑器：暂停时 `dap_keymap()` 的接管让这些键在**任何焦点**下同义。
    // 这是"一套键、无模式"的关键：面板永远不是第二套键位表。
    default:
      return "ignore";
  }
}

function open_tree(auto) {
  if (T.panel_id !== null) {
    helix.pane.focus_panel(T.panel_id);
    note_focus("tree");
    return;
  }
  if (auto) AUTO.tree = true;
  // **先把焦点交给编辑器再开**：`open_panel` 是在**当前活动叶子**上分裂的，
  // 焦点若在底部面板/别处，右侧栏就会在"那半块区域"里开出来（只有半屏高、位置也不对）。
  // 让编辑器做锚点 ⇒ 右栏永远是"整块编辑区右边的全高列"。
  focus_editor();
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

/// REPL 历史持久化（`~/.cache/helix/dapui-repl-history`，最多 200 条）。
const REPL_FS = { path: null };

/**
 * 懒加载历史（第一次打开 REPL 页时；**同步**读——`run_async` 的 promise 在插件
 * 初始化/页面切换这些时机不一定泵得到，实测 then/catch 都不进，会静默丢历史）。
 * `printf %s "$HOME"` 阻塞 ~1ms，换来确定性。
 */
function repl_history_load() {
  if (B.hist_loaded) return;
  B.hist_loaded = true;
  try {
    const home = String(helix.run('printf %s "$HOME"') || "").trim();
    if (!home) return;
    REPL_FS.path = `${home}/.cache/helix/dapui-repl-history`;
    const text = helix.read_file(REPL_FS.path);
    const lines = String(text || "")
      .split("\n")
      .map((l) => l.trimEnd().split("\\n").join("\n")) // 还原多行转义
      .filter(Boolean);
    const seen = new Set();
    const uniq = [];
    for (let i = lines.length - 1; i >= 0; i--) {
      if (seen.has(lines[i])) continue;
      seen.add(lines[i]);
      uniq.unshift(lines[i]);
    }
    B.history = uniq.slice(-200);
  } catch (e) {
    /* 首次运行没有这个文件：正常（也就没有历史） */
  }
}

/// 监视表达式的持久化：**按工作区**（`~/.cache/helix/dapui-watches-<cwd 的 slug>`）。
///
/// 为什么按工作区：早先是全局一份 ⇒ 打开另一个项目时，上一个项目的 `a` 还在列表里，
/// 而且求值失败（那个变量在新程序里不存在）。分区之后就互不干扰了。
const WATCH_FS = { path: null, loaded: false };

/** 工作区 slug：`/home/u/proj` → `home-u-proj`。 */
function workspace_slug() {
  let cwd = "";
  try {
    cwd = String(helix.cwd() || "");
  } catch (e) {
    cwd = "";
  }
  return cwd.replace(/^\/+|\/+$/g, "").replace(/[^A-Za-z0-9._-]+/g, "-") || "default";
}

/** 懒加载监视表（第一次建树时；同步读，理由同 REPL 历史）。 */
function watches_load() {
  if (WATCH_FS.loaded) return;
  WATCH_FS.loaded = true;
  if (bool_opt("persist_watches", true) === false) return;
  try {
    const home = String(helix.run('printf %s "$HOME"') || "").trim();
    if (!home) return;
    WATCH_FS.path = `${home}/.cache/helix/dapui-watches-${workspace_slug()}`;
    const text = helix.read_file(WATCH_FS.path);
    const list = String(text || "")
      .split("\n")
      .map((l) => l.trim())
      .filter(Boolean);
    for (const expr of list) {
      if (!T.watches.includes(expr)) T.watches.push(expr);
    }
  } catch (e) {
    /* 首次运行没有这个文件：正常 */
  }
}

/** 落盘（写失败不影响调试）。 */
function watches_save() {
  if (bool_opt("persist_watches", true) === false) return;
  if (!WATCH_FS.path) {
    // 还没解析过 HOME（例如刚启动就加监视）：先解析一次
    try {
      const home = String(helix.run('printf %s "$HOME"') || "").trim();
      if (home) WATCH_FS.path = `${home}/.cache/helix/dapui-watches-${workspace_slug()}`;
    } catch (e) {
      return;
    }
  }
  if (!WATCH_FS.path) return;
  try {
    helix.write_file(WATCH_FS.path, T.watches.join("\n") + (T.watches.length ? "\n" : ""));
  } catch (e) {
    /* ignore */
  }
}

/** 落盘（最多 200 条）。写失败不影响调试。 */
function repl_history_save() {
  if (!REPL_FS.path) return;
  try {
    const text = B.history
      .slice(-200)
      .map((e) => String(e).split("\n").join("\\n")) // 多行表达式压成一行（`\n` 转义）
      .join("\n");
    helix.write_file(REPL_FS.path, text + "\n");
  } catch (e) {
    /* ignore */
  }
}

/** 括号是否还没配平（决定 Enter 是"续行"还是"求值"）。忽略字符串内容。 */
function unclosed_brackets(text) {
  let depth = 0;
  let quote = null;
  for (const ch of text) {
    if (quote) {
      if (ch === quote) quote = null;
      continue;
    }
    if (ch === '"' || ch === "'") quote = ch;
    else if (ch === "(" || ch === "[" || ch === "{") depth++;
    else if (ch === ")" || ch === "]" || ch === "}") depth--;
  }
  return depth > 0 || text.endsWith("\\");
}

function repl_push(text, style) {
  B.transcript.push({ text, style: style || null });
  if (B.transcript.length > OUTPUT_CAP) B.transcript.shift();
}

/// 每个 channel 的**未完成行**（adapter 的输出块可能不含结尾换行）。
const OUT_PARTIAL = new Map();

/** 追加一段原始输出：按 `\n` 拆行；最后一段（可能是半行）留着等下一块。 */
function output_feed(chunk, category, style) {
  // **CRLF 归一**：adapter 常给 `line0\r\nline1\r\n`；若把 `\r` 留在行里，
  // 面板 draw.text 画出来时光标会被拉回行首 ⇒ 后画的字覆盖前面的（看着像"没换行/乱码"）。
  const text = String(chunk || "").replace(/\r\n/g, "\n").replace(/\r/g, "");
  if (!text) return;
  const key = category || "stdout";
  const buf = (OUT_PARTIAL.get(key) || "") + text;
  const parts = buf.split("\n");
  const tail = parts.pop(); // 末尾可能是没写完的一行
  for (const line of parts) output_push(line, style);
  OUT_PARTIAL.set(key, tail || "");
}

/** 会话结束/切页时把缓存里的半行刷出来（否则最后一行会丢）。 */
function output_flush() {
  for (const [key, tail] of OUT_PARTIAL) {
    if (tail) output_push(tail, key === "stderr" ? "error" : null);
  }
  OUT_PARTIAL.clear();
}

function output_push(text, style) {
  B.output.push({ text, style: style || null });
  if (B.output.length > OUTPUT_CAP) B.output.shift();
  if (B.follow) B.scroll = 0;
}

/**
 * REPL 补全（`completions` 请求；lldb-dap 的 `supportsCompletionsRequest` 为 true）。
 *
 * 交互与 shell 一致：第一次 `Tab` 拉候选并补第一个，之后 `Tab`/`Shift-Tab` 在候选间循环；
 * 任何一个字符/退格/回车都会清掉候选。候选条画在提示行**上一行**。
 */
function repl_complete(backward) {
  const items = B.comp.items || [];
  if (items.length > 0) {
    const n = items.length;
    const idx = B.comp.idx < 0 ? 0 : (((B.comp.idx + (backward ? -1 : 1)) % n) + n) % n;
    B.comp.idx = idx;
    apply_completion(items[idx]);
    return;
  }
  const text = B.input;
  if (!text.trim()) return;
  const params = { text, column: text.length + 1 };
  if (T.ctx && T.ctx.frame !== null && T.ctx.frame !== undefined) params.frameId = T.ctx.frame;
  helix.dap
    .request("completions", params)
    .then((r) => {
      const targets = (r && r.targets) || [];
      if (targets.length === 0) {
        say("没有补全候选");
        return;
      }
      // lldb 会塞一个空候选（label/text 都空）⇒ 过滤掉，否则第一次 Tab 等于没反应
      const items = targets.filter((t) => String(t.label || t.text || "").trim().length > 0);
      if (items.length === 0) {
        say("没有可用的补全候选");
        return;
      }
      B.comp = { items, idx: -1 };
      repl_complete(backward);
      flatten();
    })
    .catch((e) => say("补全失败: " + err_msg(e)));
}

/** 把候选应用到输入框（`text` 优先：DAP 的 text 是"替换后的整行"）。 */
function apply_completion(item) {
  if (!item) return;
  // lldb 常把文档字符串塞进 `label`（`  s -- int &`），所以：`text` 优先、都空就别动输入框
  const text = item.text && String(item.text).trim() ? String(item.text) : "";
  // lldb 的 label 形如 `s -- int &`；插入时只取第一个词
  const word = String(item.label || "").trim().split(/\s+/)[0] || "";
  const cur = B.input.replace(/\n/g, "");
  // 谁"以当前输入为前缀"就用谁（lldb 的 `text` 有时给的是整行原文 ⇒ 不能盲用它）
  let rep = "";
  if (word && word.startsWith(cur)) rep = word;
  else if (text && text.startsWith(cur)) rep = text;
  else rep = word || text;
  if (!rep) return;
  B.input = rep;
}

function repl_eval() {
  const expr = B.input.trim();
  B.input = "";
  B.more = false;
  if (!expr) return;
  B.history.push(expr);
  repl_history_save();
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

// ============================ 断点列表（底部面板的第三个 tab） ============================
// 为什么不是 Picker:核心合同规定**纯字符键永不交给 picker 的 onKey**(拾字安全阀),
// 所以 d/c/l 在 picker 里根本收不到。底部面板的 onKey 收全部键 ⇒ 放这里。

function bp_list() {
  const list = helix.dap.list_breakpoints() || [];
  return list.slice().sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : a.line - b.line));
}

/** 断点行 = 若干段样式（图标 / 位置 / 条件 / 日志），由 `render_bp_row` 按列拼。 */
function bp_parts(b) {
  return {
    mark: b.verified ? "●" : "◯",
    // 路径取**末两段**（`src/a.c`）：既紧凑又能区分同名文件
    where: `${path_tail(b.path, 2)}:${b.line + 1}`,
    cond: b.condition ? b.condition : "",
    log: b.log ? b.log : "",
    note: !b.verified && b.message ? b.message : "",
  };
}

/** 一行的富文本（各列宽度由整页的最长内容决定 → 逐行对齐）。 */
function bp_spans(b, cols, selected) {
  const p = bp_parts(b);
  const base = selected ? "ui.selection" : null;
  const dim = selected ? "ui.selection" : "ui.virtual";
  const spans = [
    { text: " " + p.mark + " ", style: selected ? "ui.selection" : b.verified ? "ui.text" : "error" },
    { text: helix.text.pad(p.where, cols.where), style: base },
  ];
  if (p.cond) spans.push({ text: "  条件: " + helix.text.truncate(p.cond, cols.cond), style: dim });
  else if (p.log) spans.push({ text: "  日志: " + helix.text.truncate(p.log, cols.log), style: dim });
  if (p.note) spans.push({ text: "  !" + p.note, style: selected ? "ui.selection" : "error" });
  return spans;
}

/** 路径末 n 段（`/a/b/c.c` → `b/c.c`）。 */
/** 错误压成一句短话（去掉 `Error:` 前缀、取第一行、超长**从中间截**）。
 *
 * 为什么从中间截：lldb 的错误是 `<user expression 1>:1:1: use of undeclared identifier 'v'`
 * —— 开头那段（表达式序号）没用，真正有用的在**尾巴**上。只截尾巴会把关键信息丢掉。
 */
function short_err(e) {
  let m = e && e.message ? String(e.message) : String(e || "");
  m = m.replace(/^Error:\s*/i, "").split("\n")[0].trim();
  const LIMIT = 40;
  if (m.length <= LIMIT) return m;
  const head = 10;
  const tail = LIMIT - head - 1;
  return m.slice(0, head) + "…" + m.slice(-tail);
}

/** 栈帧名缩短（纯字符串；demangle 已经在核心做过）。
 *
 * `frame_name` 三档：
 *   short  = `add` / `len`（默认；重名时自动补 `(rsdbg)` 消歧）
 *   keeper = 保留类型限定 `Point::len`
 *   full   = `rsdbg::add` / `core::ptr::drop_in_place`
 */
function frame_display(raw, mode) {
  let name = String(raw || "").trim();
  if (mode === "full") return name;
  // 去 lldb 残留修饰
  name = name.replace(/^\[inlined\]\s*/, "");
  const atIdx = name.indexOf(" at ");
  if (atIdx > 0) name = name.slice(0, atIdx);
  // 去泛型参数（尖括号配平；`operator<<` 这种 << 不是模板，靠"能配平到末尾"判断）
  name = strip_generics(name);
  // 去 hash（legacy `::h1234…` / v0 残留）
  name = name.replace(/::h[0-9a-f]{16}$/i, "");
  // 去闭包后缀
  name = name.replace(/::\{closure#\d+\}$/, "").replace(/\{\{closure\}\}/g, "");
  name = name.replace(/\{closure#\d+\}/g, "closure");
  if (mode === "keeper") {
    // 保留 `Type::method`：去掉前面的**模块**路径（保留最后两段）
    const segs = name.split("::").filter(Boolean);
    return segs.slice(-2).join("::");
  }
  const segs = name.split("::").filter(Boolean);
  return segs.length ? segs[segs.length - 1] : name;
}

/** 删掉 `<...>`（配平；括号不配平就原样返回，别把 `operator<<` 吃掉）。 */
function strip_generics(s) {
  let out = "";
  let depth = 0;
  for (let i = 0; i < s.length; i++) {
    const ch = s[i];
    if (ch === "<") {
      depth++;
      continue;
    }
    if (ch === ">") {
      if (depth > 0) {
        depth--;
        continue;
      }
      out += ch;
      continue;
    }
    if (depth === 0) out += ch;
  }
  if (depth !== 0) return s; // 不配平 ⇒ 不动
  return out;
}

/// 当前工作区（缓存；`:cd` 后下次 render 会重取）。
let WS_CACHE = { cwd: null, at: 0 };

function workspace_root() {
  const now = Date.now();
  if (WS_CACHE.cwd !== null && now - WS_CACHE.at < 2000) return WS_CACHE.cwd;
  let cwd = "";
  try {
    cwd = String(helix.cwd() || "");
  } catch (e) {
    cwd = "";
  }
  WS_CACHE = { cwd, at: now };
  return cwd;
}

/** 源码是否在**当前工作区**里（std/core 的库源码在 rustup 目录 ⇒ 不算）。 */
function in_workspace(path) {
  const root = workspace_root();
  if (!root || !path) return true; // 判不了就别乱折叠
  const norm = String(path).replace(/\\/g, "/");
  return norm === root || norm.startsWith(root.endsWith("/") ? root : root + "/");
}

/** 全是 mangled 符号（`^_R…` / `^_ZN…`）⇒ 当系统帧折叠，别铺在顶层。 */
function looks_mangled(name) {
  return /^_R[A-Za-z0-9_]+$/.test(String(name || "")) || /^_ZN[0-9]/.test(String(name || ""));
}

function path_tail(p, n) {
  const parts = String(p || "").split("/").filter(Boolean);
  return parts.slice(-(n || 2)).join("/") || String(p || "");
}

function bp_columns(list, w) {
  let where = 0;
  let cond = 0;
  let log = 0;
  for (const b of list) {
    const p = bp_parts(b);
    where = Math.max(where, helix.text.width(p.where));
    if (p.cond) cond = Math.max(cond, helix.text.width(p.cond));
    if (p.log) log = Math.max(log, helix.text.width(p.log));
  }
  const maxWhere = Math.max(12, Math.floor(w * 0.45));
  const room = Math.max(8, w - Math.min(where, maxWhere) - 12);
  return {
    where: Math.min(where, maxWhere),
    cond: Math.min(cond, room),
    log: Math.min(log, room),
  };
}

/** 改单项时把另几项原样带上:核心的 set_breakpoint 是**整组覆盖**(缺省 = 清空)。 */
function set_bp_field(b, field, value) {
  const opts = {
    condition: field === "condition" ? value || null : b.condition || null,
    log: field === "log" ? value || null : b.log || null,
    hit: b.hit || null,
  };
  helix.dap
    .set_breakpoint(b.path, b.line, opts)
    .then(() => say(`${short_path(b.path)}:${b.line + 1} 已更新`))
    .catch((e) => say("改断点失败: " + err_msg(e)));
}

function render_bottom(focus, ctx) {
  const { w, h } = panel_box(focus, ctx, 80, num("bottom_size", 12));
  // ① tab 条：手绘（面板没有 tab 原语）
  const tab_bar = ["repl", "output", "breakpoints"]
    .map((t) => (B.tab === t ? "▸" : " ") + (t === "repl" ? "REPL" : t === "output" ? "Output" : "Breakpoints"))
    .join("  ");
  helix.draw.text(0, 0, tab_bar, { style: "ui.text" });
  const badge = session_badge();
  if (badge.length + 8 < w) {
    helix.draw.text(w - badge.length - 1, 0, badge, { style: "ui.virtual" });
  }
  helix.draw.hline(0, 1, w, { style: "ui.virtual" });

  const body_h = Math.max(1, h - 2);
  if (B.tab === "repl") {
    // 多行草稿：占最后 showN 行（最多 4 行 / 一半面板），转录区相应收缩。
    const draft = B.input.split("\n");
    const maxDraft = Math.max(1, Math.min(4, Math.floor(body_h / 2)));
    const showN = Math.min(draft.length, maxDraft);
    const from = draft.length - showN;
    const listH = Math.max(1, body_h - showN);
    const lines = B.transcript.slice(-(listH - 1));
    for (let i = 0; i < lines.length; i++) {
      helix.draw.text(0, 2 + i, helix.text.truncate(lines[i].text, w), {
        style: lines[i].style,
      });
    }
    // 候选条：紧贴草稿上方
    const items = (B.comp && B.comp.items) || [];
    if (items.length > 0) {
      const cur = B.comp.idx >= 0 ? B.comp.idx + 1 : 1;
      const bar =
        `[${cur}/${items.length}] ` +
        items.map((i) => String(i.label || i.text || "").trim()).join("  ");
      helix.draw.text(0, h - 1 - showN, helix.text.truncate(bar, w), { style: "ui.virtual" });
    }
    // 草稿逐行画：首行带提示符（续行 `... `，被裁掉过则 `… `），末行带光标块
    for (let i = 0; i < showN; i++) {
      const prefix = i === 0 ? (from > 0 ? "… " : B.more ? "... " : "> ") : "  ";
      const tail = i === showN - 1 ? "█" : "";
      const text = prefix + draft[from + i] + tail;
      helix.draw.text(0, h - showN + i, helix.text.truncate(text, w), {
        style: "ui.text.focus",
      });
    }
  } else if (B.tab === "breakpoints") {
    const list = bp_list();
    if (list.length === 0) {
      helix.draw.text(0, 2, "(没有断点：在编辑器里按 F9 / <space>G b 打一个)", { style: "ui.virtual" });
    } else {
      if (B.bp_cursor >= list.length) B.bp_cursor = Math.max(0, list.length - 1);
      const top = Math.min(
        Math.max(0, B.bp_cursor - Math.floor((body_h - 1) / 2)),
        Math.max(0, list.length - body_h),
      );
      const cols = bp_columns(list, w);
      for (let i = 0; i < body_h - 1 && top + i < list.length; i++) {
        const idx = top + i;
        helix.draw.text(0, 2 + i, bp_spans(list[idx], cols, idx === B.bp_cursor), {
          style: null,
        });
      }
    }
    helix.draw.text(0, h - 1, helix.text.truncate("d 删 · c 条件 · l 日志 · e 异常 · Enter 跳转 · Esc 回编辑器", w), {
      style: "ui.help",
    });
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

/** 断点 tab 的键位：j/k/g/G 移动 · Enter 跳转 · d 删 · c 条件 · l 日志。 */
function key_breakpoints(key) {
  const list = bp_list();
  if (B.bp_cursor >= list.length) B.bp_cursor = Math.max(0, list.length - 1);
  const current = () => list[B.bp_cursor];
  switch (key.name) {
    case "k":
    case "Up":
      B.bp_cursor = Math.max(0, B.bp_cursor - 1);
      return "handled";
    case "j":
    case "Down":
      B.bp_cursor = Math.min(Math.max(0, list.length - 1), B.bp_cursor + 1);
      return "handled";
    case "g":
      B.bp_cursor = 0;
      return "handled";
    case "G":
      B.bp_cursor = Math.max(0, list.length - 1);
      return "handled";
    case "R":
      say(`断点 ${list.length} 个`);
      return "handled";
    case "e": {
      const session = active_session();
      const filters = (session && session.filters) || [];
      if (filters.length === 0) return say("这个 adapter 没有异常断点过滤器");
      const enabled = new Set(filters.filter((f) => f.default).map((f) => f.filter));
      ask_multi(
        `异常断点（会话 #${session.id}）`,
        filters.map((f) => ({ label: f.label || f.filter, value: f.filter })),
        enabled,
        (picked) => {
          helix.dap
            .set_exception_breakpoints(picked)
            .then(() => say(`异常断点: ${picked.length ? picked.join(", ") : "全部关闭"}`))
            .catch((e) => say("设异常断点失败: " + err_msg(e)));
        },
      );
      return "handled";
    }
    case "Enter": {
      const b = current();
      if (!b) return say("没有断点");
      helix.open_file(b.path, { row: b.line });
      return "handled";
    }
    case "d": {
      const b = current();
      if (!b) return say("没有断点");
      helix.dap
        .remove_breakpoint(b.path, b.line)
        .then(() => {
          const left = bp_list().length;
          if (B.bp_cursor >= left) B.bp_cursor = Math.max(0, left - 1);
          say(`已删 ${short_path(b.path)}:${b.line + 1}`);
        })
        .catch((e) => say("删断点失败: " + err_msg(e)));
      return "handled";
    }
    case "c": {
      const b = current();
      if (!b) return say("没有断点");
      ask_text(`条件 ${short_path(b.path)}:${b.line + 1}`, b.condition || "", (v) =>
        set_bp_field(b, "condition", v),
      );
      return "handled";
    }
    case "l": {
      const b = current();
      if (!b) return say("没有断点");
      ask_text(`日志消息 ${short_path(b.path)}:${b.line + 1}`, b.log || "", (v) =>
        set_bp_field(b, "log", v),
      );
      return "handled";
    }
    default:
      return "ignore";
  }
}

function key_bottom(key) {
  // `C-l`：清 REPL 转录（gdb 里同款），其它 ctrl 键照旧放行
  if (key.ctrl && key.name === "l" && B.tab === "repl") {
    B.transcript.length = 0;
    B.more = false;
    return "handled";
  }
  if (key.ctrl || key.alt) {
    if (is_focus_editor_key(key)) return focus_editor();
    return "ignore";
  }
  // 与 tab 无关的键：切换 / 关闭
  switch (key.name) {
    case "Tab":
      // REPL 里**有输入时 Tab 是补全**（见下方 repl 分支），否则才是切页
      if (B.tab === "repl" && B.input.length > 0) break;
      B.tab = B.tab === "repl" ? "output" : B.tab === "output" ? "breakpoints" : "repl";
      return "handled";
    case "1":
      // REPL 页里数字是**输入**（要能写 `1+1`）；切页用 Tab 或 :dapui-*
      if (B.tab !== "repl") {
        B.tab = "repl";
        return "handled";
      }
      break;
    case "2":
      if (B.tab !== "repl") {
        B.tab = "output";
        return "handled";
      }
      break;
    case "3":
      if (B.tab !== "repl") {
        B.tab = "breakpoints";
        return "handled";
      }
      break;
    case "Esc":
      // REPL：**有输入先清输入**（gdb 里 C-c 的手感），清空了再按才回编辑器
      if (B.tab === "repl" && B.input.length > 0) {
        B.input = "";
        B.more = false;
        return "handled";
      }
      // 与树一致：先回编辑器，面板保留；关面板用 `:dapui-close`
      return focus_editor();
    case "q":
      // REPL 页 `q` 是可打印字符（要能输入），所以只在非 REPL 页接管
      if (B.tab !== "repl" && B.input.length === 0) {
        close_bottom();
        return "handled";
      }
      break;
  }

  if (B.tab === "breakpoints") return key_breakpoints(key);

  if (B.tab === "repl") {
    switch (key.name) {
      case "Tab":
        repl_complete(key.shift);
        return "handled";
      case "Enter":
        // 括号没配平（`foo({` / 多行表达式）⇒ 续行，不立刻求值
        if (unclosed_brackets(B.input)) {
          B.input += "\n";
          B.more = true;
          return "handled";
        }
        repl_eval();
        return "handled";
      case "Backspace":
        B.input = B.input.slice(0, -1);
        B.comp = { items: [], idx: -1 };
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
          B.comp = { items: [], idx: -1 }; // 继续打字 ⇒ 候选作废
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

function open_bottom(tab, auto) {
  if (tab) B.tab = tab;
  // REPL 页第一次打开时加载历史（这里在命令/按键上下文，promise 才泵得动）
  if (B.tab === "repl" && !B.hist_loaded) repl_history_load();
  if (B.panel_id !== null) {
    // 已经开着：只切 tab 并把焦点送进去（`open_panel` 的 id ⇒ focus_panel）
    helix.pane.focus_panel(B.panel_id);
    note_focus("bottom");
    return;
  }
  if (auto) AUTO.bottom = true;
  // 底部面板要**横跨整宽**（而"在当前叶上分裂"会缩进右栏那一列里）⇒ 用 `rail: true`
  // 让核心把它**停靠到底部**（`register_dock_bottom`，与左右 rail 对称）。
  B.panel_id = helix.open_panel({
    side: "bottom",
    rail: true,
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

// ============================ 状态栏段 ============================

function statusline_segment() {
  // 层徽标由 `statusline` 插件的 `layers` 段统一显示（`\uf188 DEBUG`）——
  // 这里**只报会话**，否则 `[DEBUG]` 会出现两次。
  const session = active_session();
  if (!session) return null;
  let text = `#${session.id} ${state_icon(session.state)}`;
  if (session.state === "stopped" && session.thread !== null && session.thread !== undefined) {
    text += ` t${session.thread}`;
  }
  return { text, style: session.state === "stopped" ? "error" : "ui.virtual" };
}

// ============================ 内联值（默认关） ============================

/// 当前执行行用的 scope。主题定义了就用；否则从 `ui.cursorline.primary` / `ui.selection`
/// 的底色派生一个注册（`set_theme` 是合并语义，不会抹掉别人的 scope）。
let current_line_scope_cache;
function current_line_scope() {
  if (current_line_scope_cache !== undefined) return current_line_scope_cache;
  const want = cfg().current_line_scope || "dap.current-line";
  let scope = want;
  try {
    if (!helix.get_style(want)) {
      const base = helix.get_style("ui.cursorline.primary") || helix.get_style("ui.selection");
      const bg = base && base.bg;
      if (bg) {
        helix.set_theme({ [want]: { bg: bg } });
      } else {
        // 主题里连 cursorline/selection 都没有 ⇒ 直接借用（渲染成主题默认，不报错）
        scope = "ui.cursorline.primary";
      }
    }
  } catch (e) {
    scope = "ui.cursorline.primary";
  }
  current_line_scope_cache = scope;
  return scope;
}

/// 重写当前帧的所有装饰。
///
/// **必须一次写齐**：Rust 侧按文档处理这一批装饰请求时是**整组替换**
/// （`virtual_text` 与 `highlights` 一起换），所以"先清空再只写一类"会让另一类消失。
/// 这里起手 `set_virtual_text(path)` = Clear（VT + highlight 一起清），再按开关写回。
function refresh_decorations() {
  const path = T.ctx && T.ctx.source;
  if (!path) return;
  try {
    helix.set_virtual_text(path); // Clear：VT + highlight 一起清
  } catch (e) {
    return;
  }
  const row = T.ctx.line || 0;
  // ① 当前执行行高亮（默认开）
  if (cfg().highlight_current_line !== false) {
    try {
      helix.set_highlight(path, row, 0, row + 1, 0, current_line_scope());
    } catch (e) {
      /* 行号越界等，忽略 */
    }
  }
  // ② 行尾内联值（默认开）
  apply_inline_values();
  // ③ 断点行标记（**最后画**：`apply_inline_values` 不再 Clear，但顺序上更安全）
  draw_breakpoint_marks(path);
}

/** 当前帧 Locals 的一行摘要（`a=2 b=3`）：树头部显示 + 内联值共用。 */
function locals_summary(limit) {
  if (!T.ctx) return "";
  const frameRoot = T.roots.find((r) => r.frame && r.frame === T.ctx.frame);
  const scopes = (frameRoot && (T.cache[frameRoot.id] || frameRoot.children)) || [];
  const locals = find_scope_by_name(scopes, "locals");
  const vars = (locals && (T.cache[locals.id] || locals.children)) || [];
  return vars
    .slice(0, limit || 6)
    .filter((n) => n.raw || n.value)
    .map((n) => `${n.label}=${n.raw ?? n.value}`)
    .join(" ");
}

/** 这个节点**自身**命中当前搜索吗（祖先节点只是因为子节点命中才留下）。 */
function own_match(node) {
  const f = String(T.filter || "").trim().toLowerCase();
  if (!f || !node) return false;
  const hay = `${node.label || ""} ${node.value || ""}`.toLowerCase();
  return hay.includes(f);
}

/** 在 `T.nodes` 里找下一个/上一个"自身命中"的节点并跳过去（循环）。 */
function jump_match(dir) {
  const n = T.nodes.length;
  if (n === 0) return "handled";
  for (let i = 1; i <= n; i++) {
    const idx = (T.cursor + dir * i + n * 2) % n;
    if (own_match(T.nodes[idx])) {
      T.cursor = idx;
      return "handled";
    }
  }
  say("没有更多匹配");
  return "handled";
}

/** 按**原始作用域名**找（标签可能是中文，不能拿 label 比）。 */
function find_scope_by_name(scopes, want) {
  return (scopes || []).find(
    (sc) => String(sc.raw_name || "").toLowerCase() === want || /^locals$/i.test(String(sc.label)),
  );
}

/** 只重画行尾内联值（scopes/variables 到位后调用，避免重复写高亮）。 */
function apply_inline_values() {
  const path = T.ctx && T.ctx.source;
  if (!path || !cfg().inline_values) return;
  const row = T.ctx.line || 0;
  const frameRoot = T.roots.find((r) => r.frame && r.frame === T.ctx.frame);
  const scopes = (frameRoot && (T.cache[frameRoot.id] || frameRoot.children)) || [];
  const locals = find_scope_by_name(scopes, "locals");
  const vars = (locals && (T.cache[locals.id] || locals.children)) || [];
  const text = vars
    .slice(0, 8)
    .filter((n) => n.raw || n.value)
    .map((n) => `${n.label}=${n.raw ?? n.value}`)
    .join(" ");
  if (!text) return;
  try {
    helix.set_virtual_text(path, row, 0, "  " + text, "ui.virtual");
  } catch (e) {
    /* 忽略 */
  }
}

/// 清掉装饰（继续执行/会话结束）：`set_virtual_text(path)` = Clear。
function clear_decorations() {
  const path = T.ctx && T.ctx.source;
  if (!path) return;
  try {
    helix.set_virtual_text(path);
  } catch (e) {
    /* ignore */
  }
}

/**
 * 在**当前文档**的断点行首画标记（`●` 已验证 / `◯` 未验证）。
 *
 * 为什么用虚拟文本:`set_virtual_text` 是唯一的"往缓冲区里写东西"的插件 API，
 * 且只能插在**列 0** ⇒ 断点行会整体右移 1 格（有 `breakpoint_marks=false` 可关）。
 * 调用点：断点变更（b/d、Breakpoints 页）、命中/继续、打开文档。
 */
function refresh_breakpoint_marks() {
  if (!cfg().breakpoint_marks) return;
  // 有会话上下文 ⇒ 交给 `refresh_decorations()`（它会先 Clear 再把高亮/内联/标记一起画）
  if (T.ctx) {
    refresh_decorations();
    return;
  }
  const doc = active_doc_path();
  if (!doc) return;
  try {
    helix.set_virtual_text(doc.path); // 无会话：自己清一次
  } catch (e) {
    return;
  }
  draw_breakpoint_marks(doc.path);
}

/** 只画标记（**不清屏**：Clear 会抹掉同文件的高亮/内联值）。 */
function draw_breakpoint_marks(path) {
  if (!cfg().breakpoint_marks) return;
  for (const bp of helix.dap.list_breakpoints(path) || []) {
    if (bp.path !== path) continue;
    try {
      helix.set_virtual_text(path, bp.line, 0, bp.verified ? "●" : "◯", "error");
    } catch (e) {
      /* 行号越界等，忽略 */
    }
  }
}

/** 当前编辑器文档（`{path}`）；拿不到 shell 信息时返回 null。 */
function active_doc_path() {
  try {
    const list = helix.buffer && helix.buffer.list ? helix.buffer.list() : null;
    const id = helix.buffer && helix.buffer.current ? helix.buffer.current() : null;
    if (!list || id === null || id === undefined) return null;
    const doc = list.find((b) => b.id === id);
    return doc && doc.path ? { path: doc.path } : null;
  } catch (e) {
    return null;
  }
}

/** `readonly_on_stop`：停止时上锁（走核心的 `:read-only [all]`，不新增 API）。 */
function apply_readonly() {
  const mode = cfg().readonly_on_stop || "off";
  if (mode === "off" || RO.applied) return;
  try {
    helix.set_readonly(true, mode === "all");
    RO.applied = true;
    say(`只读已开（${mode}）：编辑被拦，Esc 退出调试态后可改，:read-only 解除`);
  } catch (e) {
    say("设置只读失败: " + err_msg(e));
  }
}

/** 继续/结束时撤销（只撤销我们置的；用户手动开的只读不动）。 */
function clear_readonly() {
  if (!RO.applied) return;
  RO.applied = false;
  try {
    helix.set_readonly(false, (cfg().readonly_on_stop || "off") === "all");
  } catch (e) {
    /* ignore */
  }
}

// ============================ 事件 ============================

// 新会话建成 ⇒ Output 清空：不清的话每次 `:dapui-output` 看到的都是**上一条会话**的残留。
// 只认 `kind === "created"`（`running` 那种在 continue 之后也会发，清空会误伤）。
helix.on("dap-session", (payload) => {
  if (!payload || payload.kind !== "created") return;
  B.output.length = 0;
  OUT_PARTIAL.clear();
});

helix.on("dap-stopped", (payload) => {
  if (!is_active_event(payload)) return;
  T.lastStopReason = payload && payload.reason ? String(payload.reason) : "";
  if (T.lastStopReason !== "step") {
    // 断点/异常/attch 停在库里也要停给人看 ⇒ 重新计数
    SKIP.hops = 0;
    SKIP.total = 0;
  }
  apply_readonly();
  apply_perspective();
  refresh_breakpoint_marks();
  // 自动打开**不抢焦点**：命中那一刻你要的是继续看代码/按 step，不是先按 Esc 逃出来。
  // （面板照样开、照样跟着刷新；要进去操作：`Esc` 反向不成立 —— 用 `C-\`/`C-4` 进面板，
  //   或 `:dapui` 明确打开。见 §17.2。）
  if (cfg().auto_open !== false) {
    const steal = cfg().focus_on_auto_open === true; // 默认 false
    if (T.panel_id === null) open_tree(true);
    if (!steal) focus_editor();
  }
  load_roots();
  const bottom = cfg().auto_open_bottom || (cfg().auto_open_repl ? "repl" : "off");
  if (bottom && bottom !== "off") open_bottom(bottom, true);
});

/** 事件属于"当前 active 会话"吗？（多会话 / 重启时旧会话的收尾事件不能动新会话的装饰） */
function is_active_event(payload) {
  if (!payload || payload.id === undefined || payload.id === null) return true;
  const s = active_session();
  return !s || s.id === payload.id;
}

helix.on("dap-continued", (payload) => {
  if (!is_active_event(payload)) return;
  clear_readonly();
  restore_perspective();
  refresh_breakpoint_marks();
  clear_decorations();
  T.ctx = null;
  reset_tree();
});

helix.on("dap-terminated", (payload) => {
  if (!is_active_event(payload)) return;
  clear_readonly();
  restore_perspective();
  refresh_breakpoint_marks();
  clear_decorations();
  T.ctx = null;
  reset_tree();
  // 会话没了 ⇒ 树没有栈帧可看，收掉；**异常退出**才把底部（Output）留着给你看输出
  output_flush();
  const crashed = T.exit !== null && T.exit !== 0;
  close_auto_opened(crashed);
  T.exit = null;
});

/**
 * 关掉"我们自动打开"的面板；用户手动开的不动。
 *
 * `keep_bottom`：异常退出时留着底部（切到 Output，让你看得见崩溃输出）。
 */
function close_auto_opened(keep_bottom) {
  if (AUTO.bottom) {
    AUTO.bottom = false;
    if (keep_bottom) {
      B.tab = "output"; // 留着给你看崩溃输出
    } else {
      close_bottom();
    }
  }
  if (AUTO.tree) {
    AUTO.tree = false;
    close_tree();
  }
}

helix.on("dap-exited", (payload) => {
  if (!is_active_event(payload)) return;
  clear_decorations();
  const code = payload && payload.exit_code;
  T.exit = code === undefined || code === null ? null : code;
  if (code && code !== 0) {
    // 异常退出：把底部切到 Output（崩溃信息在那儿），状态栏报退出码
    if (B.panel_id === null) open_bottom("output", true);
    B.tab = "output";
    say(`进程退出码 ${code}（Output 里有输出）`);
  } else if (code === 0) {
    say("进程正常退出（0）");
  }
});

// 换主题 ⇒ 派生 scope 的底色失效，下次重新解析
helix.on("theme-change", () => {
  current_line_scope_cache = undefined;
});

// 断点表**本地**变了（切花/删除/条件/日志/编辑重映射）⇒ 重画行首 `●/◯`。
// 这条不能靠 `dap-breakpoint`：那个只在 adapter 回 `setBreakpoints` 时才有，
// 而"还没启动会话就打断点"根本没有回包。
helix.on("dap-breakpoints", () => {
  refresh_breakpoint_marks();
});

helix.on("dap-output", (payload) => {
  if (!payload) return;
  const style = payload.category === "stderr" ? "error" : null;
  // 逐行喂（adapter 的输出块里可能有多行，也可能只有半行）
  output_feed(payload.output, payload.category, style);
  // 构建输出是**流式**来的：第一行到达就把底部切到 Output（否则用户看不到进度，
  // 会以为 `:debug-start` 卡死）。只在还没开面板时动，不抢焦点。
  if (payload.category === "build" || payload.category === "stderr") {
    try {
      if (B.panel_id === null) open_bottom("output", true);
      else if (B.tab !== "output") B.tab = "output";
    } catch (e) {
      /* 面板打不开不影响输出本身（都在 B.output 里，手动 :dapui-output 也能看） */
    }
  }
});

// ============================ 命令 ============================

helix.register_command("dapui", () => toggle_tree(), "开/关右侧变量树");
helix.register_command("dapui-repl", () => open_bottom("repl"), "底部切到 REPL 页（求值输入框）");
helix.register_command("dapui-output", () => open_bottom("output"), "底部切到 Output 页（构建/程序输出）");
helix.register_command("dapui-breakpoints", () => open_bottom("breakpoints"), "底部切到断点页（d 删 · c 条件 · l 日志 · Enter 跳转）");
/**
 * 把焦点送进 dapui 面板（auto_open 不再抢焦点之后，这是"进面板"的显式入口）。
 *
 * 实现用**方向聚焦**（`pane.focus_dir(编辑器id, dir)`）：面板是布局叶子，按方向移动是
 * 核心既有的能力，不必再给核心加"按面板 id 聚焦"的新 API。默认往右（变量树那一列）。
 */
/**
 * `:dapui-focus [tree|bottom]`：把焦点送进 dapui 面板（auto_open 不抢焦点之后的"回去"入口）。
 *
 * 不带参数时**在两个面板之间轮换**（只开了一个就聚焦它；都没开就开右侧树）——
 * 这样从编辑器回来只要重复同一条命令/同一个键，就能在"右树 ↔ 底面板"之间跳。
 * 带参数则直达：`:dapui-focus bottom`（不需要先知道上次停在哪个）。
 */
helix.register_command("dapui-focus", () => {
  let want = null;
  try {
    want = (helix.command_args() || [])[0] || null;
  } catch (e) {
    /* 拿不到参数就当没给 */
  }
  const has_tree = T.panel_id !== null;
  const has_bottom = B.panel_id !== null;
  const focus_tree = () => {
    last_panel = "tree";
    helix.pane.focus_panel(T.panel_id);
    note_focus("tree");
  };
  const focus_bottom = () => {
    last_panel = "bottom";
    helix.pane.focus_panel(B.panel_id);
    note_focus("bottom");
  };

  if (want === "tree" || want === "bottom") {
    if (want === "tree") {
      has_tree ? focus_tree() : open_tree();
    } else {
      has_bottom ? focus_bottom() : open_bottom(B.tab || "output");
    }
    return;
  }

  if (!has_tree && !has_bottom) {
    open_tree();
    return;
  }
  if (has_tree && !has_bottom) return focus_tree();
  if (!has_tree && has_bottom) return focus_bottom();
  // 两个都开着：轮换（上一次去的是树 ⇒ 这次去底部）
  last_panel = last_panel === "tree" ? "bottom" : "tree";
  return last_panel === "tree" ? focus_tree() : focus_bottom();
}, "焦点送进 dapui 面板（无参数在两个面板间轮换；tree|bottom 指定）");

helix.register_command(
  "dapui-close",
  () => {
    close_tree();
    close_bottom();
  },
  "关掉 dapui 的所有面板",
);

helix.register_statusline_segment("dap", statusline_segment);

// REPL 历史在**第一次打开 REPL 页**时懒加载（见 `open_bottom`）——load 期调用
// 那把 promise 不会兑现（实测：then/catch 都不进），所以挪到命令/按键上下文里。

helix.export({
  open_tree,
  close_tree,
  toggle_tree,
  open_bottom,
  close_bottom,
  refresh: load_roots,
  render_tree,
  flatten,
  render_bottom,
  bottom_rows,
  key_tree,
  key_bottom,
  key_breakpoints,
  focus_editor,
  is_focus_editor_key,
  is_active_event,
  apply_readonly,
  clear_readonly,
  apply_perspective,
  restore_perspective,
  watches_load,
  watches_save,
  refresh_breakpoint_marks,
  draw_breakpoint_marks,
  active_doc_path,
  locals_summary,
  frame_display,
  trim_value,
  own_match,
  jump_match,
  strip_generics,
  looks_mangled,
  fit_value,
  compute_columns,
  repl_eval,
  repl_complete,
  apply_completion,
  repl_history_load,
  repl_history_save,
  unclosed_brackets,
  refocus_panel,
  selection_text,
  add_watch,
  refresh_watches,
  set_variable,
  load_roots,
  bp_parts,
  bp_spans,
  path_tail,
  expand_node,
  map_scopes,
  map_variables,
  ask_text,
  ask_choice,
  ask_multi,
  bp_list,
  set_bp_field,
  statusline_segment,
  T,
  B,
});
