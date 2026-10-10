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
let popup = null;     // 弹窗 id
let all = [];         // 全部条目(字符串)
let shown = [];       // 过滤后的条目
let sel = 0;          // 当前选中下标
let query = "";       // 当前查询
let n_open = 0, n_typed = 0, n_accepted = 0;

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
  guard(() => helix.open_file(target, {}));
  return "close";
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

function open_picker() {
  if (popup !== null) return;
  n_open++;
  query = "";
  sel = 0;
  // 数据源 v1:文件树(现有 picker 的文件源也是这么取的 ✓)
  all = guard(() => helix.read_tree("."), []) || [];
  all = all
    .map((x) => (typeof x === "string" ? x : x && (x.path || x.name) ? x.path || x.name : null))
    .filter((x) => typeof x === "string" && x.length > 0);
  refilter();
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
helix.register_command("pick", () => open_picker());
helix.register_command("pick-close", () => close_picker());
/// 自诊断:把 v1 依赖的事实打出来 ✓
helix.register_command("pick-diag", () => {
  say(
    "pick-diag: open=" + (popup === null ? "closed" : popup) +
      " opens=" + n_open + " typed=" + n_typed + " accepted=" + n_accepted +
      " all=" + all.length + " shown=" + shown.length + " q=" + JSON.stringify(query)
  );
});
