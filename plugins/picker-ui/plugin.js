// picker-ui/plugin.js —— **纯 JS 自绘的 picker**(A′ 弹窗式)。
//
// ## 目的(架构方向 ✓)
// 把 picker 的**策略层**(界面/交互/源/过滤)**移到插件** ✓,而**机制层**
// (核心的 nucleo 匹配与预览)保持不动 ✓ —— 即"策略外移、机制留进程内" ✓。
//
// ## 依赖的能力(全部**已存在**,本插件零核心改动 ✓)
//   open_popup({render, onKey, width, height, position})  ✓(探针实测过 ✓)
//   read_tree(".") / run_async("rg …")                    ✓(现有 picker 源已在用 ✓)
//   open_file(path) / run_command(name)                   ✓
//   echo(...)                                             ✓
//
// ## v1 的范围(刻意最小 ✓)
//   · 数据源:文件(read_tree ✓)
//   · 过滤:**纯 JS** 子序列匹配 + 打分(零新能力 ✓;列表规模数千时足够快 ✓)
//   · 渲染:**纯字符串行**(render 契约实测支持 RenderLine[] ✓ —— 不冒险用未验证的节点 API ✓)
//   · 交互:可打印字符=追加查询 ✓ / Backspace=退格 ✓ / ↑↓=移动 ✓ / Enter=打开 ✓ / Esc=关闭 ✓
//
// ## 已知边界(诚实 ✓)
//   · 大列表(10 万+)会慢 ✗ ⇒ 那时再上 `helix.fuzzy.filter`(包进程内 nucleo ✓)
//   · 无预览、无高亮、无多列 ✗(v1 不含)
//   · 弹窗打开时会**吞掉所有按键** ✓(A′ 的既有性质 ✓)⇒ 对 picker 而言这正是我们要的 ✓
helix.plugin("picker-ui", { deps: [], version: "0.1" });

// ── 可配置 ────────────────────────────────────────────────────────────────
helix.define_config("picker-ui", {
  max_items: { type: "number", default: 200, doc: "最多显示/匹配的条目数" },
  width: { type: "string", default: "80%", doc: "弹窗宽度" },
  height: { type: "string", default: "70%", doc: "弹窗高度" },
});

function guard(fn, fallback) {
  try {
    return fn();
  } catch (e) {
    return fallback;
  }
}
function say(msg) {
  guard(() => helix.echo(msg));
}
// 显示宽度:CJK/全角占 2 列 ✓(块元素/制表符是 1 列 —— 真机渲染反证过 ✓)
function dw(s) {
  let n = 0;
  for (const ch of String(s)) {
    const c = ch.codePointAt(0);
    const wide =
      (c >= 0x1100 && c <= 0x115f) || (c >= 0x2e80 && c <= 0xa4cf) ||
      (c >= 0xac00 && c <= 0xd7a3) || (c >= 0xf900 && c <= 0xfaff) ||
      (c >= 0xfe30 && c <= 0xfe6f) || (c >= 0xff00 && c <= 0xff60) ||
      (c >= 0xffe0 && c <= 0xffe6) || (c >= 0x1f300 && c <= 0x1f64f);
    n += wide ? 2 : 1;
  }
  return n;
}
function cut(s, width) {
  let out = "";
  for (const ch of String(s)) {
    if (dw(out + ch) > width) break;
    out += ch;
  }
  return out;
}

// ── 纯 JS 模糊匹配(子序列 + 打分;零新能力 ✓)────────────────────────────
// 规则:全部查询字符按序出现即成;奖励连续、词首、以及"短目标"
// (与 nucleo 比差在细节与速度,但 v1 形态验证足够 ✓)
function fuzzy(query, target) {
  if (!query) return { score: 0, hits: [] };
  const q = query.toLowerCase();
  const t = String(target).toLowerCase();
  let qi = 0;
  let score = 0;
  let streak = 0;
  const hits = [];
  for (let ti = 0; ti < t.length && qi < q.length; ti++) {
    if (t[ti] !== q[qi]) {
      streak = 0;
      continue;
    }
    hits.push(ti);
    streak += 1;
    score += 1 + streak * 2; // 连续命中加成
    const prev = ti === 0 ? "/" : t[ti - 1];
    if ("/._- ".includes(prev)) score += 6; // 词首加成(路径/单词边界)
    qi += 1;
  }
  if (qi < q.length) return null; // 未全部按序命中
  score += Math.max(0, 40 - Math.floor(t.length / 4)); // 短目标优先
  return { score, hits };
}

// ── 状态 ──────────────────────────────────────────────────────────────────
/// **源注册表**(策略层的一等公民 ✓):每个源给出"取条目"与"打开条目" ✓
/// 加新源只需在此加一项 ⇒ 这正是"策略可插拔"的落点 ✓
const SOURCES = {
  files: {
    label: "文件",
    // 先例:plugins/examples/picker.js 用的是 read_tree(".").then(...) ✓
    load: (done) => {
      let p = null;
      try { p = helix.read_tree("."); } catch (e) { say("pick: read_tree 不可用 —— " + (e && e.message ? e.message : e)); }
      if (p && typeof p.then === "function") {
        p.then((es) => done(normalize(es))).catch((e) => {
          say("pick: read_tree 失败,改用同步兜底 —— " + (e && e.message ? e.message : e));
          done(files_sync());
        });
        return;
      }
      done(files_sync());
    },
    open: (item) => guard(() => helix.open_file(item, {})),
  },
  buffers: {
    label: "缓冲",
    load: (done) => { buffer_items = []; done(buffers_list()); },
    open: (item) => buffer_open(item),
  },
};

let popup = null;     // 弹窗 id
let all = [];         // 全部条目(字符串)
let shown = [];       // 过滤后的条目
let sel = 0;          // 当前选中下标
let query = "";       // 当前查询
let n_open = 0, n_typed = 0, n_accepted = 0;
let cur_source = "files"; // 当前源 ✓

function refilter() {
  const cfg = guard(() => helix.get_config("picker-ui"), {}) || {};
  const maxn = typeof cfg.max_items === "number" && cfg.max_items > 0 ? cfg.max_items : 200;
  if (!query) {
    shown = all.slice(0, maxn);
  } else {
    const scored = [];
    for (const it of all) {
      const m = fuzzy(query, it);
      if (m) scored.push({ it, s: m.score });
    }
    scored.sort((a, b) => b.s - a.s);
    shown = scored.slice(0, maxn).map((x) => x.it);
  }
  if (sel >= shown.length) sel = Math.max(0, shown.length - 1);
}

function accept() {
  if (!shown.length) {
    say("pick: 没有可打开的条目");
    return "close";
  }
  const target = shown[sel];
  n_accepted++;
  const src = SOURCES[cur_source] || SOURCES.files;
  guard(() => src.open(target));
  return "close";
}

/// 归一化:接受 `["a","b"]` 或 `[{path|name}, …]` ✓(形状不确定 ⇒ 两种都收 ✓)
function normalize(entries) {
  const list = Array.isArray(entries) ? entries : [];
  return list
    .map((x) => (typeof x === "string" ? x : x && (x.path || x.name) ? x.path || x.name : null))
    .filter((x) => typeof x === "string" && x.length > 0);
}

/// 取文件列表:**先照先例用 read_tree 的 Promise** ✓;失败/不可用则**同步兜底** `helix.run` ✓
function load_files() {
  let p = null;
  try {
    p = helix.read_tree(".");
  } catch (e) {
    say("pick: read_tree 不可用 —— " + (e && e.message ? e.message : e));
  }
  if (p && typeof p.then === "function") {
    p.then((entries) => {
      all = normalize(entries);
      refilter();
    }).catch((e) => {
      say("pick: read_tree 失败,改用同步兜底 —— " + (e && e.message ? e.message : e));
      sync_fallback();
    });
    return;
  }
  sync_fallback();
}

/// 打开的缓冲区列表 —— 形状未验证 ✗ ⇒ 兼容三种形态并在诊断里报告 ✓
/// 缓冲区列表 —— **真名来自先例** ✓(plugins/examples/picker.js:37)
///   `helix.buffer.list()` ⇒ [{id,name,path}] ✓ · `helix.buffer.focus(id)` ✓
/// (此前我猜的是 `helix.buffers()` ✗ ⇒ 诊断里显示 `bufs=none` ✓ —— 又一次"名字在清单里但挂别处" ✓)
function buffers_list() {
  const raw = guard(() => (helix.buffer && helix.buffer.list ? helix.buffer.list() : null), null);
  if (!raw) {
    say("pick: helix.buffer.list 不可用(该版本?)");
    return [];
  }
  const arr = Array.isArray(raw) ? raw : [];
  buffer_items = arr
    .map((b) => (b && (b.name || b.path) ? { id: b.id, label: String(b.name || b.path) } : null))
    .filter((x) => x !== null);
  return buffer_items.map((x) => x.label);
}
/// 打开缓冲区:按 **id** 聚焦(匿名 buffer 无 path ⇒ 必须用 id ✓,照先例 ✓)
function buffer_open(label) {
  const hit = buffer_items.find((x) => x.label === label);
  if (!hit) return false;
  return guard(() => { helix.buffer.focus(Number(hit.id)); return true; }, false);
}
let buffer_items = [];

/// 同步兜底:rg --files(gitignore 友好 ✓)→ git ls-files → find ✓;全部失败则空表 ✓
function sync_fallback() {
  for (const cmd of ["rg --files", "git ls-files", "find . -type f -not -path './.git/*'"]) {
    const out = guard(() => helix.run(cmd), null);
    if (typeof out === "string" && out.length > 0) {
      all = out.split("\n").map((s) => s.trim()).filter((s) => s.length > 0);
      refilter();
      say("pick: 数据源 = " + JSON.stringify(cmd) + "(" + all.length + " 条)");
      return;
    }
  }
  say("pick: 没有取到文件列表(read_tree/兜底命令都不可用)");
  refilter();
}

function render(_focus, ctx) {
  const w = ctx && typeof ctx.width === "number" ? ctx.width : 80;
  const h = ctx && typeof ctx.height === "number" ? ctx.height : 20;
  const rows = [];
  rows.push(cut("> " + query + "_", w));
  rows.push("─".repeat(Math.max(0, Math.min(w, 60))));
  const list_h = Math.max(1, h - 3);
  // 滚动窗口:让选中项可见 ✓
  let top = 0;
  if (sel >= list_h) top = sel - list_h + 1;
  const cfg = guard(() => helix.get_config("picker-ui"), {}) || {};
  const tag = "[" + shown.length + "/" + all.length + "]";
  for (let i = 0; i < list_h; i++) {
    const idx = top + i;
    if (idx >= shown.length) break;
    const mark = idx === sel ? "▸ " : "  ";
    const item = cut(String(shown[idx]), Math.max(1, w - 4));
    rows.push(mark + item);
  }
  rows.push("");
  rows.push(cut("  " + tag + "  ↑↓ 移动 · Enter 打开 · Esc 关闭", w));
  return rows;
}

function open_picker(source_name) {
  if (popup !== null) return;
  const src = SOURCES[source_name] ? source_name : "files";
  cur_source = src;
  n_open++;
  query = "";
  sel = 0;
  // ★ 修正(由先例得出):`helix.read_tree(".")` 返回的是 **Promise** ✗ 不是数组 ✓
  //   (plugins/examples/picker.js:12 `helix.read_tree(".").then(entries => …)` ✓)
  //   而我先前当数组用 ⇒ `all.map` 不是函数 ⇒ 命令直接失败 ✓
  // ⇒ 改成:先给空列表 ⇒ 数据到了再重过滤(render 每帧都调 ⇒ 自然刷新 ✓)
  all = [];
  shown = [];
  guard(() => SOURCES[cur_source].load((items) => { all = items; refilter(); }));
  const cfg = guard(() => helix.get_config("picker-ui"), {}) || {};
  const id = guard(
    () =>
      helix.open_popup({
        width: cfg.width || "80%",
        height: cfg.height || "70%",
        position: "center",
        render,
        onKey: (key) => {
          const nm = key && key.name !== undefined ? String(key.name) : String(key);
          if (nm === "Esc" || nm === "C-c") return "close";
          if (nm === "Enter" || nm === "Ret") return accept();
          if (nm === "Backspace") {
            query = query.slice(0, -1);
            refilter();
            return "handled";
          }
          if (nm === "Up" || nm === "C-p") {
            sel = Math.max(0, sel - 1);
            return "handled";
          }
          if (nm === "Down" || nm === "C-n") {
            sel = Math.min(Math.max(0, shown.length - 1), sel + 1);
            return "handled";
          }
          // 可打印单字符 ⇒ 追加查询 ✓(多字符/特殊名一律忽略,避免误吞 ✓)
          if (nm.length === 1 && nm >= " " && nm !== "\u007f") {
            n_typed++;
            query += nm;
            sel = 0;
            refilter();
            return "handled";
          }
          return "handled";
        },
        onClose: () => {
          popup = null;
        },
      }),
    null
  );
  if (id === null || id === undefined) {
    say("pick: open_popup 失败(该版本可能不支持)");
    return;
  }
  popup = id;
}

function close_picker() {
  const id = popup;
  popup = null;
  if (id === null) return;
  for (const fn of ["close_panel", "close_popup"]) {
    if (typeof helix[fn] !== "function") continue;
    try {
      helix[fn](id);
      return;
    } catch (e) {
      say("pick: " + fn + " 失败 —— " + (e && e.message ? e.message : e));
    }
  }
}

// ── 命令(不依赖 startup ⇒ 随时可用 ✓)────────────────────────────────────
helix.register_command("pick", (ctx) => {
  let arg = "";
  guard(() => {
    const a = ctx && ctx.args !== undefined ? ctx.args : ctx && ctx.text;
    if (typeof a === "string") arg = a.trim();
    else if (Array.isArray(a)) arg = a.join(" ").trim();
  });
  open_picker(arg);
});
helix.register_command("pick-close", () => close_picker());
/// 自诊断:把 v1 依赖的事实打出来 ✓
/// **给核心 picker 定义一个源并打开它** —— 用来验证"核心状态是否喂到 JS" ✓
/// 结构**照抄** plugins/examples/picker.js ✓(columns/items/preview/action ✓)
helix.register_command("pick-core", () => {
  guard(() =>
    helix.picker.define("picker-ui-core-demo", {
      columns: ["name", "path"],
      items: () =>
        helix.read_tree(".").then((entries) =>
          (entries || [])
            .filter((e) => e && !e.is_dir)
            .map((e) => [e.name, String(e.path || "").replace(/^\.\//, "")])
        ),
      preview: (row) => ({ path: row[1], line: 0 }),
      action: (row) => helix.open_file(row[1]),
    })
  );
  const ok = guard(() => { helix.picker.run("picker-ui-core-demo"); return true; }, false);
  if (!ok) say("pick-core: helix.picker.run 失败");
  else say("pick-core: 已打开**核心** picker ⇒ 按 Esc 关闭后跑 :pick-diag 看 core= ✓");
});

/// **第一个真正使用"渲染委托"的插件渲染器** ✓
/// 用法(顺序重要 ✓):
///   1) `:pick-core`(打开**核心** picker ⇒ 它会渲染一次,从而把状态+js_id 推给 JS ✓)
///   2) `:pick-core-draw`(本命令:读状态里的 js_id ⇒ 注册渲染函数 ✓)
///      ⇒ 此后核心 picker 的**列表内容由 JS 决定** ✓(核心仍负责匹配/窗口/预览 ✓)
/// 说明:核心的 render 回调只收到 `(focus,{width,height})` ✗ ⇒ 数据一律经 `helix.picker_state()` 取 ✓
helix.register_command("pick-core-draw", () => {
  const st = guard(() => helix.picker_state(), "");
  const idm = /js_id=(\d+)/.exec(st || "");
  if (!idm) {
    say("pick-core-draw: 状态里没有 js_id ⇒ 请先 :pick-core 让核心 picker 渲染一次");
    return;
  }
  const id = Number(idm[1]);
  const ok = guard(() => {
    helix.set_component_render(id, (_focus, ctx) => {
      const s = guard(() => helix.picker_state(), "") || "";
      const lines = s.split("\n");
      const head = lines.shift() || "";
      const rows = lines;
      const w = ctx && ctx.width ? ctx.width : 60;
      const out = [];
      // 首行:摘要(JS 自己排版 ⇒ UI 归 JS ✓)
      out.push({ type: "text", text: "JS-DRAWN  " + head.slice(0, Math.max(1, w - 12)) });
      // 其后:可见窗口的行(**核心已按 \t 分好单元格** ✓)—— 我在这里加自己的前缀/高亮 ✓
      for (const r of rows) {
        const cells = r.split("\t").join("  ");
        out.push({ type: "text", text: cells.slice(0, Math.max(1, w)) });
      }
      if (rows.length === 0) out.push({ type: "text", text: "(无可见项)" });
      return out;
    });
    return true;
  }, false);
  say(ok ? ("pick-core-draw: 已注册 JS 渲染器(id=" + id + ")⇒ 列表现在由 JS 画 ✓")
         : "pick-core-draw: set_component_render 失败(该版本?)");
});

helix.register_command("pick-diag", () => {
  say(
    "pick-diag: open=" + (popup === null ? "closed" : popup) +
      " opens=" + n_open + " typed=" + n_typed + " accepted=" + n_accepted +
      " all=" + all.length + " shown=" + shown.length + " q=" + JSON.stringify(query) +
      " src=" + cur_source +
      // ★ 修正:此前这里查的是 `helix.buffers` ✗ —— 但**真名是 `helix.buffer.list()`** ✓
      //   ⇒ 仪表在说谎(源已修、字段没修)⇒ 按"核查你自己的工具"这条纪律修 ✓
      " bufs=" + JSON.stringify(guard(() => (helix.buffer && helix.buffer.list ? helix.buffer.list().length : -1), -2)) +
      " bufsApi=" + (guard(() => typeof (helix.buffer && helix.buffer.list), "none")) +
      // ★ 核心 picker 的**状态馈送**(每帧由 render_picker 推送 ✓):
      //   打开核心 picker(如 helix.picker.run("files"))后即可读到它;
      //   关闭后仍保留**最后一次**快照(本实现只写不清 ✓)—— 故可关屏后再查 ✓
      " core=" + JSON.stringify(guard(() => helix.picker_state(), "n/a"))
  );
});
