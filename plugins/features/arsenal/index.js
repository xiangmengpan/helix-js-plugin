// arsenal.js — mason 式工具市场窗(M3 主视图;M4 动作菜单/版本输入/信息/批量进度)
// 用法:init.js 里 helix.load("features/arsenal/index.js"),:arsenal 打开市场窗。
// UI 改版 v2(spec 2026-09-07):自绘窗框(顶框嵌标题/底框) · 分类页签条(Tab/Shift+Tab
// 切换,all/lsp/dap/linter/formatter/installed/local)· `/` 模态搜索(SEARCH 内一切
// 可打印字符进查询,Backspace 删尾,↑↓ 移动,Esc 清查询回 NORMAL)· 全英文文案 ·
// 行布局按显示列宽计算(宽字符 CJK 双宽),状态列右锚贴框,描述超长以 … 截断不越界。
// 容器(任务 7 改用 popup v2):主窗 open_popup({ layer:"arsenal", position:"center",
//       width:"78%", height:"75%" }),信息/动作菜单/版本输入各自独立 layer 叠于主窗上
//       (同层再开=替换;子层 Esc 关自己回主窗)。JS 侧无 popup 层移除请求,关闭一律走
//       按键路径——onKey 返回 "close" 由 term 按 layer 移除,onClose 兜底置空。
// 键位:NORMAL——↑↓/j k 移动 · / 进搜索 · Enter 动作菜单(有标记→批量)· i 信息 ·
//       t 标记 · x 卸/unmanage · u 升级 · r 刷新 · C(Shift+c) 清版本记忆 ·
//       Tab/Shift+Tab 切页签 · q/Esc 关窗。SEARCH——一切可打印字符进查询 · Backspace
//       删尾 · ↑↓ 移动 · Enter 动作/批量 · Tab 切页签(查询保留)· Esc 清查询回 NORMAL。
// 任务运行中(S.busy)拒绝再提交(run_action/batch_run/版本 Enter 入口 echo 提示)。
// 数据:helix.server.rows(cb) 回传行 {name,kind,languages,installed,local,version,
//       description,homepage,installable,upgradable,needs_version,bin,source};
//       upgradable → 状态列 ▲(受管且可升级);needs_version 行:install 无记忆弹输入、
//       update 恒弹输入(升级重询,记忆锁不死换版入口);C/Shift+C 清版本记忆;
//       bin/source 供信息弹窗显示命令路径与下载源。

helix.plugin("arsenal", { deps: [] });

const ST_W = 14; // 状态列宽(列数);行右锚贴框内右缘
const BUSY_MSG = "arsenal: a task is running, please wait…";
const LAYERS = { main: "arsenal", menu: "arsenal-menu", input: "arsenal-input", info: "arsenal-info" };

// 分类页签序(all 与 installed/local 为状态过滤,其余 kind 过滤)
const KINDS = ["all", "lsp", "dap", "linter", "formatter", "installed", "local"];
const KIND_GLYPH = { lsp: "L", dap: "D", linter: "!", formatter: "F" };

// ── 显示列宽助手:宽字符(CJK/全角)占 2 列;截断/补齐一律按列,绝不中腰字符 ──
const WIDE_CHAR = /[\u1100-\u115F\u2E80-\uA4CF\uAC00-\uD7A3\uF900-\uFAFF\uFE30-\uFE4F\uFF00-\uFF60\uFFE0-\uFFE6]/;
function wc_len(s) {
  let w = 0;
  for (const ch of String(s || "")) w += WIDE_CHAR.test(ch) ? 2 : 1;
  return w;
}
function wc_trunc(s, cols) {
  let out = "";
  let acc = 0;
  for (const ch of String(s || "")) {
    const cw = WIDE_CHAR.test(ch) ? 2 : 1;
    if (acc + cw > cols) break;
    out += ch;
    acc += cw;
  }
  return out;
}
// 超长时以 … 收尾(共占 cols 列);未超长原样返回
function wc_ell(s, cols) {
  if (wc_len(s) <= cols) return s;
  return wc_trunc(s, Math.max(1, cols - 1)) + "…";
}
// 补齐到 cols 列(不足补空格;不裁切——内容侧先截好)
function wc_pad(s, cols) {
  const have = wc_len(s);
  return s + " ".repeat(Math.max(0, cols - have));
}

const S = {
  popup: null, // 主窗 open_popup id(null = 未打开)
  rows: [], // 全部配方行(server.rows 回传)
  searching: false, // 搜索模态(SEARCH 态;由 / 进入,Esc 退出)
  filter: "", // 搜索查询串(仅 SEARCH 态随字符增长)
  kind: "all", // 当前页签
  sel: 0, // 选中行索引(visible 序)
  marks: new Set(), // 标记集合(name;Enter 批量)
  menu: null, // 动作菜单状态 { items:[{op,label}], sel, name };非空 = 菜单弹窗开
  vinput: null, // 版本输入弹窗状态 { op, name, val };非空 = 输入弹窗开
  // name → 最近一次输入的版本。install 有记忆快捷直发(批量复用);update 恒弹窗不受记忆
  // 短路(换版本靠弹窗重输);Shift+C 清选中行记忆、无则清全部(弹窗内 C 清当前行)。
  vinputs: {},
  busy: null, // 批量进度 { task_id, items, idx, done, fail, pct };非空 = 任务在跑
  row_busy: {}, // name → { phase, pct };行状态优先显示(进度覆写)
};

// 下载/任务事件 → S.row_busy / S.busy;kind 见 helix.server.task 事件约定
function on_task_event(ev) {
  if (!S.busy || ev.task_id !== S.busy.task_id) return; // 非本批次(旧任务残留)忽略
  if (ev.kind === "phase") {
    S.row_busy[ev.name] = { phase: ev.msg || "…", pct: null };
    return;
  }
  if (ev.kind === "progress") {
    const b = S.row_busy[ev.name] || { phase: "downloading", pct: null };
    b.pct = ev.total > 0 ? Math.floor((100 * ev.bytes) / ev.total) : null;
    S.row_busy[ev.name] = b;
    S.busy.pct = b.pct;
    return;
  }
  // done / error = 当前项终态:worker 串行,按序推进 cur
  delete S.row_busy[ev.name];
  if (ev.kind === "done") {
    S.busy.done += 1;
    if (S.busy.fail.length === 0 && ev.msg) helix.echo(ev.msg); // 单项成功可直报(done 事件 msg)
    S.busy.idx += 1;
  } else if (ev.kind === "error") {
    if (ev.name) {
      S.busy.fail.push(ev.name + (ev.msg ? " (" + ev.msg + ")" : ""));
      S.busy.idx += 1;
    } else {
      // 批级 panic(name="";run_batch catch 兜底事件):worker 已不执行本批剩余 item,
      // 不会再发事件 → 剩余项逐个补 error 终态,防 busy 卡底栏(done+fail 永不到总量)
      if (ev.msg) S.busy.fail.push("batch worker panic: " + ev.msg + " (remaining aborted)");
      for (let i = S.busy.idx; i < S.busy.items.length; i++) {
        S.busy.fail.push(S.busy.items[i].name);
      }
      S.busy.idx = S.busy.items.length;
    }
  }
  S.busy.cur = S.busy.items[S.busy.idx] || null;
  fetch_rows(); // done/error 后刷新行状态
  if (S.busy.done + S.busy.fail.length >= S.busy.items.length) {
    const { done, fail, items } = S.busy;
    // 单项成功:done 事件 msg 已直报,不再补汇总(防双 echo);失败或批量才汇总
    if (items.length > 1 || fail.length > 0) {
      const all_done = fail.length === 0;
      helix.echo(
        "arsenal: batch done " + done + "/" + items.length + (all_done ? "" : ", failed: " + fail.join(" "))
      );
    }
    S.busy = null;
    S.row_busy = {};
  }
}

// 提交流程任务(单/批量共用):helix.server.task 入队即回;busy 底栏 + 行进度。
// I4 重入守卫:任务进行中(S.busy 非空)拒绝新提交——run_action/batch_run/版本输入
// Enter 全部汇到这里,守卫一处覆盖三入口。
function begin_task(items) {
  if (S.busy) {
    helix.echo(BUSY_MSG);
    return;
  }
  const task_items = items.map((it) => {
    const o = { op: it.op, name: it.name };
    if (it.version) o.version = it.version;
    return o;
  });
  S.busy = {
    task_id: 0,
    items: task_items,
    idx: 0,
    done: 0,
    fail: [],
    pct: null,
    cur: task_items[0] || null,
  };
  S.busy.task_id = helix.server.task(task_items, on_task_event);
}

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

function row_by_name(name) {
  return S.rows.find((r) => r.name === name) || null;
}

function move(d) {
  const n = visible().length;
  if (!n) return;
  S.sel = Math.max(0, Math.min(S.sel + d, n - 1));
}

// Tab 页签循环:Tab 下一个 / Shift+Tab 上一个;切页签重置选中
function tab_step(dir) {
  const i = KINDS.indexOf(S.kind);
  S.kind = KINDS[(i + dir + KINDS.length) % KINDS.length];
  S.sel = 0;
}
function tab_next() {
  tab_step(1);
}
function tab_prev() {
  tab_step(-1);
}

function toggle_mark() {
  const r = selected_row();
  if (!r) return;
  if (S.marks.has(r.name)) S.marks.delete(r.name);
  else S.marks.add(r.name);
}

// 过滤:页签 + 查询(名/语言/描述子串;大小写不敏感)
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

// 行状态:行 busy 进度 > 可升级 ▲/✓/local/–;upgradable 字段驱动 ▲
function status_text(r) {
  const busy = S.row_busy[r.name];
  if (busy) return (busy.pct != null ? busy.pct + "% " : "") + (busy.phase || "…");
  if (r.installed && r.local) return r.version ? "local " + r.version : "local";
  if (r.installed) return (r.upgradable ? "▲ " : "✓ ") + (r.version || "");
  if (r.installable) return "–";
  return "no source";
}

// ────────────────────────── 自绘窗框与行版式(按显示列宽) ──────────────────────────

// 顶框:┌ arsenal ──…── 3/52 recipes ─┐(title 左、right 右,中间 ─ 填充)
function frame_top(W, title, right) {
  const innerW = Math.max(0, W - 2);
  const tl = " " + title + " ";
  const rr = right ? " " + right + " " : "";
  let fill = innerW - wc_len(tl) - wc_len(rr);
  if (fill < 0) {
    // 极端窄:优先裁右侧计数,再裁标题
    const t2 = wc_ell(tl, innerW);
    return "┌" + wc_pad(t2, innerW) + "┐";
  }
  return "┌" + tl + "─".repeat(fill) + rr + "┐";
}
function frame_mid(W, inner) {
  return "│" + wc_pad(inner, Math.max(0, W - 2)) + "│";
}
function frame_bottom(W) {
  return "└" + "─".repeat(Math.max(0, W - 2)) + "┘";
}
// 小弹窗(菜单/信息/版本输入)整框构建:返回 [text节点...];body = [{text, style?}]
function build_box(title, W, body) {
  const nodes = [helix.el("text", frame_top(W, title, ""), { style: "ui.virtual" })];
  for (const b of body) {
    nodes.push(helix.el("text", frame_mid(W, b.text), b.style ? { style: b.style } : {}));
  }
  nodes.push(helix.el("text", frame_bottom(W), { style: "ui.virtual" }));
  return nodes;
}

// 页签条整行(逐页签节点配色;行宽恰 W):页签左起,右侧信息(search/marked)贴右
function tab_row(W) {
  const innerW = W - 2;
  let rightTxt = "";
  if (S.searching) rightTxt = "search:" + (S.filter || " ");
  else if (S.marks.size) rightTxt = S.marks.size + " marked";
  let usedCols = 0;
  for (const k of KINDS) usedCols += wc_len(" " + k + " ");
  usedCols += KINDS.length - 1; // 页签间分隔 │
  const leftCols = usedCols;
  const avail = Math.max(0, innerW - leftCols);
  rightTxt = avail > 0 ? wc_ell(rightTxt, avail) : "";
  const padN = Math.max(0, innerW - leftCols - wc_len(rightTxt));
  const nodes = [helix.el("text", "│", { style: "ui.virtual" })];
  KINDS.forEach((k, i) => {
    nodes.push(helix.el("text", " " + k + " ", { style: k === S.kind ? "ui.selection" : "ui.virtual" }));
    if (i < KINDS.length - 1) nodes.push(helix.el("text", "│", { style: "ui.virtual" }));
  });
  if (padN > 0) nodes.push(helix.el("text", " ".repeat(padN), {}));
  if (rightTxt) nodes.push(helix.el("text", rightTxt, { style: "ui.virtual" }));
  nodes.push(helix.el("text", "│", { style: "ui.virtual" }));
  return helix.el("row", nodes);
}

// 行:│ mark glyph name … desc …  status │(列宽=显示列宽;截断 … 收尾,状态右锚)
function row_el(r, i, W) {
  const sel = i === S.sel;
  const innerW = Math.max(10, W - 2);
  const stW = Math.min(ST_W, Math.max(6, innerW - 8));
  const nameW = Math.max(8, Math.min(24, Math.floor(innerW * 0.28)));
  const lead = (S.marks.has(r.name) ? "▣ " : "  ") + (KIND_GLYPH[r.kind] || "?") + " ";
  const nameArea = Math.max(1, nameW - wc_len(lead));
  const nameTxt = wc_pad(wc_ell(r.name, nameArea), nameArea);
  const stTxt = wc_trunc(status_text(r), stW);
  const descW = Math.max(1, innerW - nameW - stW - 2);
  const descTxt = wc_pad(wc_ell(r.description || "", descW), descW);
  const stSlot = wc_pad(stTxt, stW);
  const inner = lead + nameTxt + " " + descTxt + " " + stSlot;
  const line = frame_mid(W, inner);
  return helix.el("text", line, sel ? { style: "ui.selection" } : {});
}

// 底帮助行:常态帮助 / 搜索提示 / busy 进度互斥;窄窗用短版防截断丢尾部键位
function footer_text(W) {
  const b = S.busy;
  if (b) {
    const fin = b.done + b.fail.length;
    const pct = b.pct != null ? b.pct : b.items.length ? Math.floor((100 * fin) / b.items.length) : 0;
    const bar = "▮".repeat(Math.round(pct / 10)) + "▯".repeat(10 - Math.round(pct / 10));
    const cur = b.cur ? b.cur.op + " " + b.cur.name : "";
    return bar + " " + pct + "% (" + b.done + "/" + b.items.length + ") " + cur;
  }
  if (S.searching) return "filtering: type to filter · ↑↓ move · Enter act · Esc exit";
  const wide = (W || 200) >= 96;
  return wide
    ? "↑↓/jk move · / search · Enter act · i info · t mark · x rm · u upd · r ref · C ver · Esc/q close"
    : "↑↓/jk · / search · Enter act · i info · t mark · x rm · u upd · r ref · Esc/q";
}

function main_render(focus, ctx) {
  try {
    const W = Math.max(30, (ctx && ctx.width) || 90);
    const H = Math.max(6, (ctx && ctx.height) || 30);
    const listH = H - 4;
    const rows = visible();
    if (S.sel > rows.length - 1) S.sel = Math.max(0, rows.length - 1);
    const start = Math.max(0, Math.min(S.sel - Math.floor(listH / 2), Math.max(0, rows.length - listH)));
    const win = rows.slice(start, start + listH);
    const lines = win.map((r, i) => row_el(r, start + i, W));
    if (!win.length) {
      lines.push(
        helix.el(
          "text",
          frame_mid(W, S.filter && S.searching ? "no matches for '" + wc_trunc(S.filter, Math.max(1, W - 8)) + "'" : "no servers yet — press r to refresh"),
          { style: "ui.virtual" }
        )
      );
    }
    // 行不足时以空白填充到 listH:scroll 只保留末尾 listH 行,框底须贴区域底
    while (lines.length < listH) lines.push(helix.el("text", frame_mid(W, ""), {}));
    const list = lines;
    const topTxt = frame_top(W, "arsenal", rows.length + "/" + S.rows.length + " recipes");
    const footer = frame_mid(W, wc_ell(footer_text(W), W - 2));
    return helix.el("col", [
      helix.el("text", topTxt, { style: "ui.virtual" }),
      tab_row(W),
      helix.el("scroll", list, { height: listH }),
      helix.el("text", footer, { style: S.busy ? "ui.popup" : "ui.virtual" }),
      helix.el("text", frame_bottom(W), { style: "ui.virtual" }),
    ]);
  } catch (e) {
    return helix.el("col", [
      helix.el("text", "arsenal render error: " + (e && e.message || e), { style: "ui.popup" }),
    ]);
  }
}

// ────────────────────────── 动作(M4:真实任务执行/菜单/版本输入) ──────────────────────────

// 行可用动作清单(菜单项与 x/u 直达的判定源;label = op 英文短词)
function row_actions(r) {
  const acts = [];
  if (r.installed && r.local) acts.push({ op: "unmanage", label: "unmanage" });
  else if (r.installed && !r.local) {
    acts.push({ op: "update", label: "update" });
    acts.push({ op: "remove", label: "remove" });
  } else if (r.installable) acts.push({ op: "install", label: "install" });
  return acts;
}

// 打开版本输入弹窗(needs_version 的 install/update 先取版本)。
// e:update 恒弹(升级重询——记忆只在弹窗里预填,绝不跳过输入直发);
//    install 无记忆才弹(有记忆走 run_action 快捷直发);弹窗内 C(Shift+c)清该行记忆(换版本逃生)。
function open_version_input(r, op) {
  S.vinput = { op: op || (r.installed ? "update" : "install"), name: r.name, val: S.vinputs[r.name] || "" };
  const W = 58;
  helix.open_popup({
    layer: LAYERS.input,
    position: "center",
    width: W,
    height: 5,
    render: () =>
      helix.el(
        "col",
        build_box(S.vinput.name + " · explicit version", W, [
          { text: "source URL contains {version} placeholder — type a tag" },
          { text: "> " + S.vinput.val + "▏", style: "ui.selection" },
          { text: "type chars · Enter submit · C clear memory · Backspace delete · Esc cancel" },
        ])
      ),
    onClose: () => {
      S.vinput = null; // 引擎非按键关闭时自愈(任务 9 防抖)
    },
    onKey: (key) => {
      if (key.ctrl || key.alt) return "handled";
      const k = key.name;
      if (k === "Esc") {
        S.vinput = null;
        return "close";
      }
      if (k === "Enter") {
        if (S.busy) {
          // I4:任务进行中拒新提交——保留弹窗与已输内容(不吞输入)
          helix.echo(BUSY_MSG);
          return "handled";
        }
        const v = (S.vinput.val || "").trim();
        const { op, name } = S.vinput;
        S.vinput = null;
        if (!v) {
          helix.echo("arsenal: version empty, cancelled");
          return "close";
        }
        S.vinputs[name] = v;
        begin_task([{ op, name, version: v }]);
        return "close";
      }
      if (k === "C") {
        // C(Shift+c):清该行版本记忆并清空预填——否则该记忆会锁死后续 install 的快捷直发
        delete S.vinputs[S.vinput.name];
        S.vinput.val = "";
        return "handled";
      }
      if (k === "Backspace") {
        S.vinput.val = S.vinput.val.slice(0, -1);
        return "handled";
      }
      if (k.length === 1) {
        S.vinput.val += k;
        return "handled";
      }
      return "handled";
    },
  });
}

// 信息弹窗(M4:补命令路径 bin / 下载源 source / 版本需求)
function open_info(row) {
  const r = row || selected_row();
  if (!r) return;
  const lang = (r.languages || []).join(", ") || "—";
  // 状态列与信息弹窗不双份重复:版本只经 status_text 出现一次(可升级以 ▲ 字样 + 括注提示)
  const st = status_text(r);
  const status = st + (r.installed && !r.local && r.upgradable ? " (upgradable)" : "");
  const lines = [
    r.name + "  [" + r.kind + "]",
    "status: " + status,
    "languages: " + lang,
    "description: " + (r.description || "—"),
    "homepage: " + (r.homepage || "—"),
    "command: " + (r.bin || (r.installable ? "(not installed)" : "—")),
    "source: " + (r.source || "—"),
    r.needs_version ? "version: explicit tag required ({version} in source)" : r.installed ? "version: pinned by recipe" : "",
  ].filter((l) => l !== "");
  lines.push("Enter/Esc/q close");
  const W = 70;
  helix.open_popup({
    layer: LAYERS.info,
    position: "center",
    width: W,
    height: lines.length + 2,
    render: () =>
      helix.el(
        "col",
        build_box(r.name + " · info", W, lines.map((l) => ({ text: wc_trunc(l, W - 2) })))
      ),
    onKey: (key) =>
      key.name === "Esc" || key.name === "q" || key.name === "Enter" ? "close" : "handled",
  });
}

// 执行单个动作(op∈unmanage/update/install/remove/info):needs_version 的安装/升级先弹版本输入
function run_action(op, name) {
  const r = row_by_name(name) || selected_row();
  if (!r) return;
  if (op === "info") return open_info(r);
  if (S.busy) {
    // I4:任务进行中拒新提交(信息查看不受限;写动作全拦)
    helix.echo(BUSY_MSG);
    return;
  }
  if (op === "install" || op === "update") {
    // e:needs_version 的 update 恒弹输入(升级重询——防记忆把换版入口锁死);install 有记忆才直发(快捷)
    if (r.needs_version && (op === "update" || !S.vinputs[r.name])) return open_version_input(r, op);
    const version = r.needs_version ? S.vinputs[r.name] : undefined;
    begin_task([{ op, name: r.name, version }]);
    return;
  }
  begin_task([{ op, name: r.name }]);
}

// 动作菜单弹窗(layer arsenal-menu 叠于主窗上;Esc/q 关自己回主窗)
function open_action_menu() {
  const r = selected_row();
  if (!r) return;
  const acts = row_actions(r);
  if (!acts.length) {
    // 占位配方(无下载源且未装)无动作:提示原因并回退打开信息弹窗
    // (info 含 source/description/homepage,解释为何不可直装/如何装)
    helix.echo("arsenal: " + r.name + " has no download source (placeholder recipe) — showing info; press i anytime");
    open_info(r);
    return;
  }
  if (acts.length === 1) return run_action(acts[0].op, r.name); // 单项直达
  S.menu = { items: acts, sel: 0, name: r.name };
  const W = 46;
  helix.open_popup({
    layer: LAYERS.menu,
    position: "center",
    width: W,
    height: S.menu.items.length + 3,
    render: () => {
      const body = S.menu.items.map((it, idx) => ({
        text: (idx === S.menu.sel ? "▸ " : "  ") + it.label + "  " + S.menu.name,
        style: idx === S.menu.sel ? "ui.selection" : undefined,
      }));
      body.push({ text: "↑↓/jk select · Enter run · i info · Esc/q close", style: "ui.virtual" });
      return helix.el("col", build_box(S.menu.name + " · actions", W, body));
    },
    onClose: () => {
      S.menu = null; // 引擎非按键关闭时自愈
    },
    onKey: (key) => {
      if (key.ctrl || key.alt) return "handled";
      const k = key.name;
      if (k === "Esc" || k === "q") {
        S.menu = null;
        return "close";
      }
      if (k === "i") {
        run_action("info", S.menu.name); // 信息叠于菜单层上(i 直达,菜单保留)
        return "handled";
      }
      if (k === "j" || k === "Down") {
        S.menu.sel = (S.menu.sel + 1) % S.menu.items.length;
        return "handled";
      }
      if (k === "k" || k === "Up") {
        S.menu.sel = (S.menu.sel - 1 + S.menu.items.length) % S.menu.items.length;
        return "handled";
      }
      if (k === "Enter") {
        const it = S.menu.items[S.menu.sel];
        const name = S.menu.name;
        S.menu = null;
        run_action(it.op, name); // 可能在 run_action 里打开版本输入弹窗(叠于其上)
        return "close";
      }
      return "handled";
    },
  });
}

// 批量:marks 集合快照 → 仅可安装项(未装+installable)install;版本用 vinputs 记忆
function batch_run() {
  if (S.busy) {
    // I4:守卫在清 marks 之前——拒绝时保留标记,可稍后重试
    helix.echo(BUSY_MSG);
    return;
  }
  const marked = [...S.marks];
  const items = S.rows
    .filter((r) => marked.includes(r.name) && !r.installed && r.installable)
    .map((r) => ({ op: "install", name: r.name, version: r.needs_version ? S.vinputs[r.name] : undefined }));
  if (!items.length) {
    helix.echo("arsenal: nothing installable among marked rows (mark uninstalled rows with a source)");
    return;
  }
  S.marks.clear(); // 快照即提交:清标记防二次 Enter 重复投
  begin_task(items);
}

// ────────────────────────── 开关 ──────────────────────────

function open_arsenal() {
  if (S.popup !== null) {
    helix.echo("arsenal: already open (Esc/q to close)"); // 无 JS 弹层移除请求 → 关闭走按键路径
    return;
  }
  fetch_rows();
  S.popup = helix.open_popup({
    layer: LAYERS.main,
    position: "center",
    width: "78%",
    height: "75%",
    render: main_render,
    onKey: handle_key,
    onClose: () => {
      S.popup = null;
    },
  });
}

function toggle_arsenal() {
  open_arsenal();
}

// ────────────────────────── 按键(主窗 onKey;模态搜索) ──────────────────────────

function handle_key(key) {
  if (key.ctrl || key.alt) return "handled";
  const k = key.name;
  if (S.searching) {
    // ── SEARCH 态:可打印字符进查询;j/k/q/f/x/u/t/r/i/C 均不触发命令 ──
    if (k === "Esc") {
      S.searching = false;
      S.filter = "";
      S.sel = 0;
      return "handled";
    }
    if (k === "Backspace") {
      S.filter = S.filter.slice(0, -1);
      return "handled";
    }
    if (k === "Enter") {
      if (S.marks.size) batch_run();
      else open_action_menu();
      return "handled";
    }
    if (k === "Tab") {
      tab_step(key.shift ? -1 : 1);
      return "handled";
    }
    if (k === "Down") {
      move(1);
      return "handled";
    }
    if (k === "Up") {
      move(-1);
      return "handled";
    }
    if (k === "/") return "handled"; // SEARCH 内 / 忽略,不输入字面
    if (k.length === 1) {
      S.filter += k;
      S.sel = 0;
      return "handled";
    }
    return "handled"; // 未映射键消费(模态市场窗)
  }
  // ── NORMAL 态:可打印字符不搜索(仅 / 进入搜索),字母为命令 ──
  if (k === "/") {
    S.searching = true;
    S.filter = "";
    S.sel = 0;
    return "handled";
  }
  if (k === "Esc") return "close";
  if (k === "q") return "close";
  if (k === "Tab") {
    tab_step(key.shift ? -1 : 1);
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
  if (k === "Enter") {
    if (S.marks.size) batch_run();
    else open_action_menu();
    return "handled";
  }
  if (k === "i") {
    open_info();
    return "handled";
  }
  if (k === "t") {
    toggle_mark();
    return "handled";
  }
  if (k === "r") {
    fetch_rows(); // 手动刷新行列表(状态列/可升级标记重新检测)
    return "handled";
  }
  if (k === "C") {
    // Shift+C:清版本记忆——选中行 needs_version 且有记忆清该行,否则清全部(换版本逃生)
    const r = selected_row();
    if (r && r.needs_version && S.vinputs[r.name] !== undefined) {
      delete S.vinputs[r.name];
      helix.echo("arsenal: cleared version memory for " + r.name + " (reinstall/update asks version again)");
    } else {
      const n = Object.keys(S.vinputs).length;
      if (n) {
        S.vinputs = {};
        helix.echo("arsenal: cleared all version memory (" + n + " rows)");
      } else {
        helix.echo("arsenal: no version memory to clear (Shift+C clears selected row, else all)");
      }
    }
    return "handled";
  }
  if (k === "x") {
    const r = selected_row();
    if (r && r.installed && r.local) run_action("unmanage", r.name);
    else if (r && r.installed) run_action("remove", r.name);
    else helix.echo("arsenal: selected row not removable (press Enter to see actions)");
    return "handled";
  }
  if (k === "u") {
    const r = selected_row();
    if (r && r.installed && !r.local) run_action("update", r.name);
    else helix.echo("arsenal: no managed install to update (press Enter to see actions)");
    return "handled";
  }
  return "handled"; // NORMAL 态未映射键消费(不再即搜)
}

// ────────────────────────── 注册(node 环境跳过) ──────────────────────────

if (typeof helix !== "undefined") {
  helix.register_command("arsenal", toggle_arsenal, "Open the LSP/tool manager market (tabs, /-search, Enter actions)");
}

if (typeof module !== "undefined" && module.exports) {
  module.exports = {
    S, KINDS, KIND_GLYPH, LAYERS,
    wc_len, wc_trunc, wc_ell, wc_pad,
    visible, status_text, move, tab_step, tab_next, tab_prev, toggle_mark,
    handle_key, main_render, row_actions, run_action, batch_run, footer_text,
  };
}
