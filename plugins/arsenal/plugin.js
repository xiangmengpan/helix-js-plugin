// arsenal.js — mason 式工具市场窗(M3 主视图;M4 动作菜单/版本输入/信息/批量进度)
// 用法:init.js 里 helix.load("arsenal")(裸名 → arsenal/plugin.js),:arsenal 打开市场窗。
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

// permissions: arsenal 会调 helix.server.*(下载/安装/卸载受管工具 + 改写 languages.toml)。
// 这些 API 自 2026-10 起受 `process` 能力检查(内置插件本就 grandfathered,显式声明是为了
// 让 manifest 与事实一致 —— 用户层同名覆盖时按第三方对待,声明才不会被询问)。
helix.plugin("arsenal", {
  deps: [],
  permissions: { process: true },
  // v2：行内进度条 / 取消 / 接线反馈 / 打开即查更新 / 版本列表 / 语言过滤 / 自检·日志·复制
  version: "2.0",
});

const ST_W = 14; // 状态列宽(列数);行右锚贴框内右缘
const BUSY_MSG = "arsenal: a task is running, please wait…";
const LAYERS = { main: "arsenal", menu: "arsenal-menu", input: "arsenal-input", info: "arsenal-info", confirm: "arsenal-confirm" };

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
  loaded: false, // 至少成功回传过一次(首帧显示 loading 而不是"没有服务器")
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
  lang: null, wiring: null, verlist: null, checking: false,
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
  if (ev.kind === "wiring") {
    try { S.wiring = JSON.parse(ev.msg || "{}"); } catch (e) { S.wiring = {}; }
    const w = S.wiring || {};
    helix.echo("arsenal: languages.toml " + (w.written ? "updated" : "unchanged") +
      ((w.servers || []).length ? " · wired " + w.servers.length : "") +
      ((w.conflicts || []).length ? " · " + w.conflicts.length + " 处需手动接线" : ""));
    return;
  }
  if (ev.kind === "canceled") {
    delete S.row_busy[ev.name];
    S.busy.canceled = (S.busy.canceled || 0) + 1;
    S.busy.idx += 1;
    S.busy.cur = S.busy.items[S.busy.idx] || null;
    if (S.busy.done + S.busy.fail.length + S.busy.canceled >= S.busy.items.length) {
      S.busy = null; S.row_busy = {}; fetch_rows();
    }
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
      // 失败摘要压成一行（核心的错误可能很长，含换行/提示）
      const one = (ev.msg || "").split("\n")[0];
      S.busy.fail.push(ev.name + (one ? " — " + (one.length > 70 ? one.slice(0, 69) + "…" : one) : ""));
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
    // 批量结束再刷新一次行状态(原实现每个 done/error 都全量刷新:50 项 = 50 次
    // 注册表重算 + 版本探测,而每项的进度已由 S.row_busy 就地显示)。
    fetch_rows();
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
    S.loaded = true;
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
    if (S.lang && !(r.languages || []).includes(S.lang)) return false;
    if (!f) return true;
    return (r.name + " " + (r.bin || "") + " " + (r.languages || []).join(" ") + " " + (r.description || ""))
      .toLowerCase()
      .includes(f);
  });
}

// 行状态:行 busy 进度 > 可升级 ▲/✓/local/–;upgradable 字段驱动 ▲
function status_text(r) {
  const busy = S.row_busy[r.name];
  if (busy) {
    if (busy.pct != null) {
      const n = Math.max(0, Math.min(8, Math.round(busy.pct / 12.5)));
      return "█".repeat(n) + "░".repeat(8 - n) + " " + busy.pct + "%";
    }
    return (busy.phase || "…").replace(/^\[\d+\/\d+\]\s*/, "");
  }
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
  if (S.verlist) return "j/k 选版本 · Enter 装该版本 · m 手动输入 · Esc 取消";
  const wide = (W || 200) >= 96;
  return wide
    ? "jk · / search · Enter act · v ver · u/U upd · d doctor · L log · y copy · l lang · i · t · x · Esc"
    : "↑↓/jk · / search · Enter act · i info · t mark · x rm · u upd · r ref · Esc/q";
}

/// 空列表文案(纯函数,便于测):**加载完**才说"没有服务器",否则说 loading ——
/// 原实现无条件说 "no servers yet",而 `server.rows` 是异步的,打开瞬间必闪一句误导。
function empty_list_text() {
  if (S.filter && S.searching) return "no matches for '" + wc_trunc(S.filter, 40) + "'";
  return S.loaded ? "no servers yet — press r to refresh" : "loading servers…";
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
          frame_mid(W, wc_ell(empty_list_text(), Math.max(1, W - 8))),
          { style: "ui.virtual" }
        )
      );
    }
    // 行不足时以空白填充到 listH:scroll 只保留末尾 listH 行,框底须贴区域底
    while (lines.length < listH) lines.push(helix.el("text", frame_mid(W, ""), {}));
    const list = lines;
    const upd = S.rows.filter((r) => r.upgradable).length;
    const topTxt = frame_top(W, "arsenal",
      (upd ? "▲" + upd + " 更新 · " : S.checking ? "检查中… · " : "") +
      rows.length + "/" + S.rows.length + " recipes" + (S.lang ? " · " + S.lang : ""));
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

/// `x` 的确认弹层。
///
/// 为什么要确认(2026-10):原实现单键即 `run_action("remove")`,手滑就卸载掉受管安装
/// (并改写 languages.toml),没有撤销。弹层里 Enter/y 确认、Esc/n 取消。
function open_remove_confirm(r) {
  const op = r.local ? "unmanage" : "remove";
  const W = 62;
  const line1 =
    op === "remove"
      ? "remove managed install? (languages.toml entry is rewritten)"
      : "stop auto-wiring this local tool? (it stays on PATH)";
  helix.open_popup({
    layer: LAYERS.confirm,
    position: "center",
    width: W,
    height: 4,
    render: () =>
      helix.el(
        "col",
        build_box(r.name + " · confirm", W, [
          { text: line1 },
          { text: "Enter/y confirm · Esc/n cancel", style: "ui.virtual" },
        ])
      ),
    onKey: (key) => {
      if (key.ctrl || key.alt) return "handled";
      const k = key.name;
      if (k === "Esc" || k === "n" || k === "q") return "close";
      if (k === "Enter" || k === "y") {
        run_action(op, r.name);
        return "close";
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
/// 批量提交的**纯**规划(不碰 helix / 不改 S):把标记行按「可见性 + 状态」分流。
///
/// 三条规则都是实测 bug 换来的(2026-10-17):
///  1. **只处理当前可见的行** —— 标记跨页签/搜索保留,原实现用全量 `S.rows` 过滤,
///     会把看不见的页签里的行也提交(标 3 行切到 lsp,Enter 装了 dap 的);
///  2. **已装(受管)行 → update**(原实现只装未装,批量升级根本不存在);
///  3. **needs_version 且无版本记忆 → 跳过并报告** —— 原实现照发,
///     让 server 端逐项报「配方未给固定 version」,等于 N 条错误。
///
/// 返回 `{ items, skipped: { hidden, local, noSource, needsVersion } }`;
/// `items` 直接就是 `helix.server.task` 的载荷(去掉 undefined 字段)。
function plan_batch(rows, marks, visibleNames, vinputs) {
  const items = [];
  const skipped = { hidden: [], local: [], noSource: [], needsVersion: [] };
  const seen = new Set();
  for (const name of marks) {
    if (seen.has(name)) continue;
    seen.add(name);
    const r = rows.find((x) => x.name === name);
    if (!r) continue;
    if (visibleNames && !visibleNames.has(name)) {
      skipped.hidden.push(name);
      continue;
    }
    if (r.installed && r.local) {
      skipped.local.push(name); // 本机已有:既不能装也不能卸(batch 不做 unmanage)
      continue;
    }
    if (r.installed) {
      // 受管已装 → update。needs_version 的 update **恒需显式版本**(不读记忆,防锁死换版入口)
      if (r.needs_version) {
        skipped.needsVersion.push(name);
        continue;
      }
      items.push({ op: "update", name });
      continue;
    }
    if (!r.installable) {
      skipped.noSource.push(name);
      continue;
    }
    const version = r.needs_version ? (vinputs || {})[name] : undefined;
    if (r.needs_version && !version) {
      skipped.needsVersion.push(name); // 没有记忆就不猜版本
      continue;
    }
    items.push(version ? { op: "install", name, version } : { op: "install", name });
  }
  return { items, skipped };
}

function batch_run() {
  if (S.busy) {
    // I4:守卫在清 marks 之前——拒绝时保留标记,可稍后重试
    helix.echo(BUSY_MSG);
    return;
  }
  if (!S.marks.size) {
    helix.echo("arsenal: no rows marked — press t to mark, Enter to submit");
    return;
  }
  const visibleNames = new Set(visible().map((r) => r.name));
  const { items, skipped } = plan_batch(S.rows, S.marks, visibleNames, S.vinputs);
  const notes = [];
  if (skipped.hidden.length) notes.push(skipped.hidden.length + " hidden by tab/search");
  if (skipped.local.length) notes.push(skipped.local.length + " local (not managed)");
  if (skipped.noSource.length) notes.push(skipped.noSource.length + " without download source");
  if (skipped.needsVersion.length) notes.push(skipped.needsVersion.length + " need an explicit version (run individually)");
  if (!items.length) {
    helix.echo("arsenal: nothing batchable among marked rows" + (notes.length ? " — " + notes.join("; ") : ""));
    return;
  }
  // 快照即提交:只清**已提交**行的标记;跳过的保留(便于逐行处理/重试)
  for (const it of items) S.marks.delete(it.name);
  begin_task(items);
  if (notes.length) helix.echo("arsenal: queued " + items.length + "; skipped " + notes.join("; "));
}

// ── v2 动作 ──────────────────────────────────────────────
function lang_filter_cycle() {
  const langs = [];
  for (const r of S.rows) for (const l of r.languages || []) if (!langs.includes(l)) langs.push(l);
  langs.sort();
  const i = S.lang === null ? -1 : langs.indexOf(S.lang);
  S.lang = i + 1 >= langs.length ? null : langs[i + 1];
  S.sel = 0;
  helix.echo(S.lang ? "arsenal: language = " + S.lang : "arsenal: language filter cleared");
}
function open_doctor() { const r = selected_row(); if (r) helix.run_command("server", ["doctor", r.name]); }
function open_log() { const r = selected_row(); if (r) helix.run_command("server", ["log", r.name, "300"]); }
function copy_bin() {
  const r = selected_row(); if (!r) return;
  try { helix.set_register(r.bin || r.name); helix.echo("arsenal: copied " + (r.bin || r.name)); }
  catch (e) { helix.echo("arsenal: copy failed: " + e); }
}
function update_all() {
  const items = S.rows.filter((r) => r.upgradable && r.installed && !r.local)
    .map((r) => ({ op: "update", name: r.name, version: S.vinputs[r.name] }));
  if (!items.length) { helix.echo("arsenal: nothing to update"); return; }
  begin_task(items);
}
function open_version_picker(op) {
  const r = selected_row(); if (!r) return;
  let list = [];
  try { list = helix.server.versions(r.name) || []; } catch (e) { list = []; }
  if (!list.length) { helix.echo("arsenal: no version list — manual input"); open_version_input(r, op || "install"); return; }
  S.verlist = { name: r.name, op: op || (r.installed ? "update" : "install"),
    items: list.slice(0, 20).map((v) => ({ tag: v.tag, date: v.date || "" })), sel: 0 };
  shell_version_picker();
}
function shell_version_picker() {
  const st = S.verlist; if (!st) return;
  const W = 56;
  const body = st.items.map((it, i) => ({
    text: (i === st.sel ? "› " : "  ") + wc_pad(wc_ell(it.tag, 22), 22) + " " + String(it.date || "").slice(0, 10),
    style: i === st.sel ? "ui.selection" : undefined,
  }));
  body.push({ text: "  manual input…", style: "ui.virtual" });
  helix.open_popup({
    layer: LAYERS.input, position: "center", width: W, height: body.length + 3,
    render: () => helix.el("col", build_box("版本 · " + st.name, W, body), { gap: 0 }),
    onKey: (key) => {
      if (!S.verlist) return "close";
      const k = key.name;
      if (k === "Esc") { S.verlist = null; return "close"; }
      if (k === "j" || k === "Down") { S.verlist.sel = Math.min(S.verlist.items.length, S.verlist.sel + 1); shell_version_picker(); return "handled"; }
      if (k === "k" || k === "Up") { S.verlist.sel = Math.max(0, S.verlist.sel - 1); shell_version_picker(); return "handled"; }
      if (k === "m") { const s2 = S.verlist; S.verlist = null; const r2 = row_by_name(s2.name); if (r2) open_version_input(r2, s2.op); return "close"; }
      if (k === "Enter") {
        const s2 = S.verlist; const it = s2.items[s2.sel]; S.verlist = null;
        const r2 = row_by_name(s2.name);
        if (!it) { if (r2) open_version_input(r2, s2.op); return "close"; }
        S.vinputs[s2.name] = it.tag; run_action(s2.op, s2.name); return "close";
      }
      return "handled";
    },
  });
}
function silent_update_check() {
  const items = S.rows.filter((r) => r.installed && !r.local).slice(0, 12).map((r) => ({ op: "check", name: r.name }));
  if (!items.length) return;
  S.checking = true;
  try { helix.server.task(items, (ev) => { if (ev.kind === "done" || ev.kind === "error") { S.checking = false; fetch_rows(); } }); }
  catch (e) { S.checking = false; }
}

// ────────────────────────── 开关 ──────────────────────────

function open_arsenal() {
  // 自愈:层可能被 onKey 的 "close" / reload / 别的插件移除,而 JS 侧没有主动关闭入口
  // (`helix.close_popup` 未注册)⇒ 本地句柄会永久失效(原症状:`:arsenal` 一直回 already open)。
  // `helix.popup_exists` 让我们能自查;老版本没有这个 API 时退化为"信任本地句柄"。
  if (S.popup !== null && typeof helix.popup_exists === "function" && !helix.popup_exists(S.popup)) {
    S.popup = null;
  }
  if (S.popup !== null) {
    helix.echo("arsenal: already open (Esc/q to close)");
    return;
  }
  fetch_rows();
  silent_update_check();
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
  // 打开失败必须可见：早先异常被吞掉 ⇒ `:arsenal` "什么都不发生"，无法诊断。
  try {
    open_arsenal();
  } catch (e) {
    helix.echo("arsenal: open failed: " + (e && e.message ? e.message : e));
    throw e;
  }
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
  if (k === "i") { open_info(); return "handled"; }
  if (k === "l") { lang_filter_cycle(); return "handled"; }
  if (k === "d") { open_doctor(); return "handled"; }
  if (k === "L") { open_log(); return "handled"; }
  if (k === "y") { copy_bin(); return "handled"; }
  if (k === "U") { update_all(); return "handled"; }
  if (k === "v") { const r = selected_row(); open_version_picker(r && r.installed ? "update" : "install"); return "handled"; }
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
    if (r && r.installed) open_remove_confirm(r);
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
    handle_key, main_render, row_actions, run_action, batch_run, plan_batch,
    open_remove_confirm, empty_list_text, open_arsenal, footer_text,
    lang_filter_cycle, update_all, silent_update_check,
  };
}
