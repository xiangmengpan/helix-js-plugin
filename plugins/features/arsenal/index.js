// arsenal.js — mason 式工具市场窗(M3 主视图;M4 动作菜单/版本输入/信息/批量进度)
// 用法:init.js 里 helix.load("features/arsenal/index.js"),:arsenal 打开市场窗。
// 容器(任务 7 改用 popup v2):主窗 open_popup({ layer:"arsenal", position:"center",
//       width:"78%", height:"75%" }),信息/动作菜单/版本输入各自独立 layer 叠于主窗上
//       (同层再开=替换;子层 Esc 关自己回主窗)。任务 9 实测:JS 侧无 popup 层移除请求,
//       关闭一律走按键路径——onKey 返回 "close" 由 term 按 layer 移除,onClose 兜底置空。
// 键位:j/k 或 Up/Down 导航(过滤非空时 j/k 归搜索,移动用 ↑↓)· 字符即搜(匹配名/语言/描述;
//       过滤非空时字母一律进搜索——x/u/i/f/t/j/k/q/r/C 不触发命令)· f 分类循环(无过滤时) ·
//       t 标记(无过滤时;Enter 批量)· Enter 动作菜单/批量(过滤中仍可用)· r 刷新(无过滤时) ·
//       x/u/i 直达卸载(停挂接)/升级/信息(无过滤时)· C(Shift+c) 清版本记忆(无过滤时) ·
//       Backspace 删过滤 · Esc 先清过滤再关 · q 无过滤时关闭(过滤中 q 为搜索字符)。
//       任务运行中(S.busy)拒绝再提交(run_action/batch_run/版本 Enter 入口 echo 提示)。
// 数据:helix.server.rows(cb) 回传行 {name,kind,languages,installed,local,version,
//       description,homepage,installable,upgradable,needs_version,bin,source};
//       upgradable → 状态列 ▲(受管且可升级);needs_version 行:install 无记忆弹输入、
//       update 恒弹输入(升级重询,记忆锁不死换版入口);C/Shift+C 清版本记忆;
//       bin/source 供信息弹窗显示命令路径与下载源。

helix.plugin("arsenal", { deps: [] });

// 行布局列宽(按渲染 ctx.width 换算,见 render):名称列 / 状态列固定,描述列伸缩。
const NAME_W = 24; // mark(2)+icon(2)+name(≈19 字符)
const ST_W = 14; // 状态最宽 "✓ 2024-09-16"/"▲ 2024-09-16"=14
const LAYERS = { main: "arsenal", menu: "arsenal-menu", input: "arsenal-input", info: "arsenal-info" };

// 分类循环序(含 kind 过滤位;installed/local 为状态过滤)
const KINDS = ["all", "lsp", "dap", "linter", "formatter", "installed", "local"];
const KIND_GLYPH = { lsp: "L", dap: "D", linter: "!", formatter: "F" };

const S = {
  popup: null, // 主窗 open_popup id(null = 未打开)
  rows: [], // 全部配方行(server.rows 回传)
  filter: "", // 即搜过滤串
  kind: "all", // 当前分类
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
    const b = S.row_busy[ev.name] || { phase: "下载中", pct: null };
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
      S.busy.fail.push(ev.name + (ev.msg ? "(" + ev.msg + ")" : ""));
      S.busy.idx += 1;
    } else {
      // 批级 panic(name="";run_batch catch 兜底事件):worker 已不执行本批剩余 item,
      // 不会再发事件 → 剩余项逐个补 error 终态,防 busy 卡底栏(done+fail 永不到总量)
      if (ev.msg) S.busy.fail.push("批量 worker panic: " + ev.msg + "(剩余项已终止)");
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
        "arsenal: 批量完成 " + done + "/" + items.length + (all_done ? "" : ", 失败: " + fail.join(" "))
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
    helix.echo("arsenal: 任务进行中,请等待完成…");
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

// 行状态:行 busy 进度 > 可升级 ▲/✓/本机/–;任务 7 起 upgradable 字段驱动 ▲
function status_text(r) {
  const busy = S.row_busy[r.name];
  if (busy) return (busy.pct != null ? busy.pct + "% " : "") + (busy.phase || "…");
  if (r.installed && r.local) return r.version ? "本机 " + r.version : "本机";
  if (r.installed) return (r.upgradable ? "▲ " : "✓ ") + (r.version || "");
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

// 底栏进度行:▮▮▮▯▯▯▯▯▯▯ 30% (1/2) demo-bin
function busy_line() {
  const b = S.busy;
  if (!b) return null;
  const fin = b.done + b.fail.length;
  const pct = b.pct != null ? b.pct : b.items.length ? Math.floor((100 * fin) / b.items.length) : 0;
  const bar = "▮".repeat(Math.round(pct / 10)) + "▯".repeat(10 - Math.round(pct / 10));
  const cur = b.cur ? b.cur.name + " " + b.cur.op : "";
  return bar + " " + pct + "% (" + b.done + "/" + b.items.length + ") " + cur;
}

function main_render(focus, ctx) {
  try {
    const W = Math.max(40, (ctx && ctx.width) || 91);
    const rows = visible();
    const H = Math.max(1, ((ctx && ctx.height) || 28) - 2); // 标题/底栏占 2 行
    if (S.sel > rows.length - 1) S.sel = Math.max(0, rows.length - 1);
    const start = Math.max(0, Math.min(S.sel - Math.floor(H / 2), Math.max(0, rows.length - H)));
    const win = rows.slice(start, start + H).map((r, i) => row_el(r, start + i, W));
    const list = win.length
      ? win
      : [helix.el("text", "(无匹配 · Esc 清过滤 / q 关闭)", { style: "ui.virtual" })];
    const title = helix.el(
      "text",
      "arsenal" + (S.filter ? " [" + S.filter + "]" : "") + " · " + S.kind + " · " + rows.length + "/" + S.rows.length + " servers",
      { style: "ui.virtual" }
    );
    const busy = busy_line();
    const footerText = S.filter
      ? "过滤中: 字母=搜索字符 · ↑↓ 移动 · Enter 操作 · Backspace 删 · Esc 清过滤"
      : "j/k ↑↓ 导航 · 即搜 · Enter 动作 · x 卸/u 升/i 信息 · f 分类 · t 标记 · r 刷新 · C 清记忆 · Esc/q 关";
    const footer = helix.el(
      "text",
      busy || footerText,
      { style: busy ? "ui.popup" : "ui.virtual" }
    );
    return helix.el("col", [title, helix.el("scroll", list, { height: H }), footer]);
  } catch (e) {
    return helix.el("col", [
      helix.el("text", "arsenal 渲染错误: " + (e && e.message || e), { style: "ui.popup" }),
    ]);
  }
}

// ────────────────────────── 动作(M4:真实任务执行/菜单/版本输入) ──────────────────────────

// 行可用动作清单(菜单项与 x/u 直达的判定源)
function row_actions(r) {
  const acts = [];
  if (r.installed && r.local) acts.push({ op: "unmanage", label: "unmanage 停用挂接" });
  else if (r.installed && !r.local) {
    acts.push({ op: "update", label: "update 升级" });
    acts.push({ op: "remove", label: "remove 卸载" });
  } else if (r.installable) acts.push({ op: "install", label: "install 安装" });
  return acts;
}

// 打开版本输入弹窗(needs_version 的 install/update 先取版本)。
// e:update 恒弹(升级重询——记忆只在弹窗里预填,绝不跳过输入直发);
//    install 无记忆才弹(有记忆走 run_action 快捷直发);弹窗内 C(Shift+c)清该行记忆(换版本逃生)。
function open_version_input(r, op) {
  S.vinput = { op: op || (r.installed ? "update" : "install"), name: r.name, val: S.vinputs[r.name] || "" };
  helix.open_popup({
    layer: LAYERS.input,
    position: "center",
    width: 54,
    height: 7,
    render: () =>
      helix.el("col", [
        helix.el("text", S.vinput.name + " 需显式版本(下载源含 {version} 占位)", { style: "ui.virtual" }),
        helix.el("text", "> " + (S.vinput.val || "") + "▏"),
        helix.el("text", "字符输入 · Enter 提交 · C 清记忆 · Backspace 删 · Esc 取消", { style: "ui.virtual" }),
      ]),
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
          helix.echo("arsenal: 任务进行中,请等待完成…");
          return "handled";
        }
        const v = (S.vinput.val || "").trim();
        const { op, name } = S.vinput;
        S.vinput = null;
        if (!v) {
          helix.echo("arsenal: 版本为空,已取消");
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
  const status = st + (r.installed && !r.local && r.upgradable ? " (可升级)" : "");
  const lines = [
    r.name + "  [" + r.kind + "]",
    "状态: " + status,
    "语言: " + lang,
    "描述: " + (r.description || "—"),
    "主页: " + (r.homepage || "—"),
    "命令: " + (r.bin || (r.installable ? "(未安装)" : "—")),
    "下载源: " + (r.source || "—"),
    (r.needs_version ? "版本: 需显式输入(下载源含 {version})" : r.installed ? "版本: 配方固定" : "") ,
  ].filter((l) => l !== "");
  lines.push("Enter/Esc/q 关闭");
  helix.open_popup({
    layer: LAYERS.info,
    position: "center",
    width: 68,
    height: lines.length + 2,
    render: () =>
      helix.el(
        "col",
        lines.map((l, i) =>
          helix.el("text", l.slice(0, 64), i === 0 ? { style: "ui.popup" } : i === lines.length - 1 ? { style: "ui.virtual" } : {})
        )
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
    helix.echo("arsenal: 任务进行中,请等待完成…");
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
    helix.echo("arsenal: " + r.name + " 无可用动作(无下载源/本地既有/缺失)");
    return;
  }
  if (acts.length === 1) return run_action(acts[0].op, r.name); // 单项直达
  S.menu = { items: acts, sel: 0, name: r.name };
  helix.open_popup({
    layer: LAYERS.menu,
    position: "center",
    width: 44,
    height: S.menu.items.length + 3,
    render: () =>
      helix.el("col", [
        helix.el("text", S.menu.name + " 动作", { style: "ui.virtual" }),
      ].concat(
        S.menu.items.map((it, i) =>
          helix.el("text", (i === S.menu.sel ? "▸ " : "  ") + it.label, i === S.menu.sel ? { style: "ui.selection" } : {})
        ),
        [helix.el("text", "↑↓/j k 选择 · Enter 执行 · i 直达信息 · Esc/q 关闭", { style: "ui.virtual" })]
      )),
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
    helix.echo("arsenal: 任务进行中,请等待完成…");
    return;
  }
  const marked = [...S.marks];
  const items = S.rows
    .filter((r) => marked.includes(r.name) && !r.installed && r.installable)
    .map((r) => ({ op: "install", name: r.name, version: r.needs_version ? S.vinputs[r.name] : undefined }));
  if (!items.length) {
    helix.echo("arsenal: 没有可安装的选中项(标记需为未安装且有下载源)");
    return;
  }
  S.marks.clear(); // 快照即提交:清标记防二次 Enter 重复投
  begin_task(items);
}

// ────────────────────────── 开关 ──────────────────────────

function open_arsenal() {
  if (S.popup !== null) {
    helix.echo("arsenal: 已打开(Esc/q 关闭)"); // 无 JS 弹层移除请求 → 关闭走按键路径
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

// ────────────────────────── 按键(主窗 onKey;Esc 语义:先清过滤再关) ──────────────────────────

function handle_key(key) {
  if (key.ctrl || key.alt) return "handled";
  const k = key.name;
  // 过滤态例外键(不受"字母即搜"影响):Esc 清/关、Backspace 删过滤、Enter 仍执行动作
  if (k === "Esc") {
    if (S.filter) {
      S.filter = "";
      return "handled";
    }
    return "close";
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
  // I2:过滤非空时,可打印字母一律进搜索——x/u/i/f/t/j/k/q/r/C 均不触发
  // 对应命令/导航(防搜名字里的字母误卸载/误弹窗/误分类/误关窗);↑↓ 仍可移动
  if (S.filter) {
    if (k.length === 1) {
      S.filter += k;
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
    return "handled"; // 未映射键消费(模态市场窗)
  }
  // ── 以下仅无过滤时生效的快捷键 ──
  if (k === "q") return "close";
  if (k === "j" || k === "Down") {
    move(1);
    return "handled";
  }
  if (k === "k" || k === "Up") {
    move(-1);
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
  if (k === "r") {
    fetch_rows(); // 手动刷新行列表(状态列/可升级标记重新检测)
    return "handled";
  }
  if (k === "C") {
    // Shift+C:清版本记忆——选中行 needs_version 且有记忆清该行,否则清全部(换版本逃生)
    const r = selected_row();
    if (r && r.needs_version && S.vinputs[r.name] !== undefined) {
      delete S.vinputs[r.name];
      helix.echo("arsenal: 已清 " + r.name + " 的版本记忆(重装/升级需重输版本)");
    } else {
      const n = Object.keys(S.vinputs).length;
      if (n) {
        S.vinputs = {};
        helix.echo("arsenal: 已清全部版本记忆(" + n + " 行)");
      } else {
        helix.echo("arsenal: 无版本记忆可清(Shift+C 清选中行记忆,无则清全部)");
      }
    }
    return "handled";
  }
  if (k === "x") {
    const r = selected_row();
    if (r && r.installed && r.local) run_action("unmanage", r.name);
    else if (r && r.installed) run_action("remove", r.name);
    else helix.echo("arsenal: 选中行无可卸载项(按 Enter 看动作)");
    return "handled";
  }
  if (k === "u") {
    const r = selected_row();
    if (r && r.installed && !r.local) run_action("update", r.name);
    else helix.echo("arsenal: 无受管安装可升级(按 Enter 看动作)");
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
  helix.register_command("arsenal", toggle_arsenal, "打开 LSP/工具管理市场(搜索/分类/标记;Enter 操作,浮层主窗)");
}

if (typeof module !== "undefined" && module.exports) {
  module.exports = {
    S, KINDS, KIND_GLYPH, LAYERS,
    visible, status_text, move, cycle_kind, toggle_mark, handle_key, main_render,
    row_actions, run_action, batch_run, busy_line,
  };
}
