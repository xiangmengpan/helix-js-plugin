// filetree.js — 类 lazyvim/neo-tree 的侧边文件树面板
// 依赖清单：icons（共享图标表，helix.load 自动先加载）
// 图标已收进核心，不再需要 deps；列目录 ⇒ fs_read，文件操作走 shell(`run_async`) ⇒ process。
// 根目录用 `helix.cwd()`（现取；旧写法 `helix.run("pwd")` 只能取一次且会陈旧）
helix.plugin("filetree", {
  deps: [],
  schema_version: 1,
  permissions: { process: true, fs_read: true },
});

// 插件配置(方案 C):config.toml [plugins.filetree]
helix.define_config("filetree", {
  show_hidden: { type: "boolean", default: false, doc: "默认显示隐藏文件" },
});
const CFG = helix.get_config("filetree") || {};
// 用法:init.js 里 helix.load("filetree")(裸名 → filetree/plugin.js),:filetree 开关面板。
// 键位：Enter/o 打开或展开，h/l 折叠/进入，Up/Down 导航，H 隐藏文件，
//       R 刷新，a/A 新建文件/目录，m 重命名，d 删除，P 上级目录，F 跟随当前文件，q/Esc 关闭。
//       **C-\ 保留面板、焦点回编辑器**；其余 C-*/M-* **穿透给编辑器**（面板非模态，
//       用户在 `[keys]` / `helix.map` 里绑的修饰键组合必须照常生效）。
// 说明：面板无节点点击命中（helix 限制），选中行由 JS 维护行号；纯键盘操作。

// ============================ 纯逻辑（不依赖 helix，node 可测） ============================

// 目录优先 + 名称升序（read_dir 已按名排序，这里只调整目录在前）
function sort_entries(entries) {
  return entries.slice().sort((a, b) => {
    if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
    return a.name < b.name ? -1 : a.name > b.name ? 1 : 0;
  });
}

// 可见节点扁平化（深度优先；折叠目录的子节点不展开；隐藏文件按 show_hidden 过滤）
// 返回 [{ node, depth }]，root 的 children 在 depth 0
function visible_rows(node, show_hidden, depth, out, seen) {
  // seen:按 path 去重,防符号链接循环目录(指向祖先的链接)导致无限递归栈溢出
  seen = seen || new Set([node.path]);
  for (const c of node.children) {
    if (!show_hidden && c.name.startsWith(".")) continue;
    out.push({ node: c, depth });
    if (c.is_dir && c.expanded && !seen.has(c.path)) {
      seen.add(c.path);
      visible_rows(c, show_hidden, depth + 1, out, seen);
    }
  }
  return out;
}

// 从根构建一个目录节点（root 本身不渲染，children 即其内容）
function make_tree(root_path, entries) {
  return {
    name: root_path.split("/").filter(Boolean).pop() || root_path,
    path: root_path,
    is_dir: true,
    expanded: true,
    children: entries.map((e) => ({
      name: e.name,
      path: e.path,
      is_dir: e.is_dir,
      expanded: false,
      children: null, // 懒加载：首次展开时才 read_dir
    })),
  };
}

// file_path 相对 root 的路径组件链（"a/b/c" → ["a","b","c"]）；不在 root 下返回 null
function rel_parts(root, file_path) {
  if (file_path === root) return [];
  const prefix = root.endsWith("/") ? root : root + "/";
  if (!file_path.startsWith(prefix)) return null;
  return file_path.slice(prefix.length).split("/").filter(Boolean);
}

// 沿链查找节点（每个中间段必须是目录；找不到返回 null）
function find_node(node, parts) {
  let cur = node;
  for (const p of parts) {
    if (!cur.children) return null;
    const hit = cur.children.find((c) => c.name === p);
    if (!hit) return null;
    cur = hit;
  }
  return cur;
}

// 查找节点的父节点（同路径唯一；root 的父是 null）
function find_parent(node, target_path) {
  if (node.path === target_path) return null;
  for (const c of node.children || []) {
    if (c.path === target_path) return node;
    if (c.is_dir && c.children) {
      const p = find_parent(c, target_path);
      if (p) return p;
    }
  }
  return null;
}

// 收集全部已展开目录的路径（reload 后恢复用）
function collect_expanded(node, out) {
  for (const c of node.children || []) {
    if (c.is_dir && c.expanded) {
      out.push(c.path);
      collect_expanded(c, out);
    }
  }
  return out;
}

// shell 单引号转义
function shq(s) {
  return "'" + String(s).replace(/'/g, "'\\''") + "'";
}

// ============================ 状态 ============================

// 统一图标源已收进核心(helix-js/src/icons.rs):直接用 `helix.icons.*`,零依赖

const S = {
  root: null,          // 当前树根目录
  tree: null,          // 根 Node（虚拟容器）
  cursor: 0,           // 选中行索引（visible_rows 序）
  show_hidden: CFG.show_hidden === true,  // 初值取 [plugins.filetree] show_hidden;H 切换
  follow: true,        // F 切换：buffer-open 时自动定位当前文件
  panel_id: null,      // open_panel 返回的 id（null = 未打开）
  current: null,       // 当前编辑文件路径（buffer-open 更新）
  prompt_value: "",    // 输入弹窗的文本框值（JS 状态渲染）
};

// ============================ 目录读取 ============================

function read_dir_sync(dir) {
  try {
    return helix.read_dir(dir);
  } catch (e) {
    helix.echo("filetree: 读取目录失败 " + dir + ": " + e.message);
    return [];
  }
}

function basename(p) {
  const parts = p.replace(/\/+$/, "").split("/");
  return parts[parts.length - 1] || p;
}

// 目录节点的子节点（懒加载 + 缓存）
function ensure_children(node) {
  if (node.children === null) {
    node.children = sort_entries(read_dir_sync(node.path)).map((e) => ({
      name: e.name,
      path: e.path,
      is_dir: e.is_dir,
      expanded: false,
      children: null,
    }));
  }
  return node.children;
}

// ============================ 树操作 ============================

function node_at(idx) {
  return visible_rows(S.tree, S.show_hidden, 0, [])[idx] || null;
}

function count_visible() {
  let n = 0;
  const walk = (node) => {
    for (const c of node.children) {
      if (!S.show_hidden && c.name.startsWith(".")) continue;
      n++;
      if (c.is_dir && c.expanded) walk(c);
    }
  };
  walk(S.tree);
  return n;
}

function move(delta) {
  S.cursor = Math.max(0, Math.min(S.cursor + delta, count_visible() - 1));
}

function toggle(node) {
  if (!node.is_dir) return;
  if (node.expanded) node.expanded = false;
  else {
    ensure_children(node);
    node.expanded = true;
  }
}

// 打开/展开当前选中项
function open_selected() {
  const hit = node_at(S.cursor);
  if (!hit) return;
  if (hit.node.is_dir) {
    if (hit.node.expanded) {
      hit.node.expanded = false;
    } else {
      ensure_children(hit.node);
      hit.node.expanded = true;
    }
  } else {
    helix.open_file(hit.node.path);
    // 打开后聚焦编辑器叶子(该文件所在),面板保留
    helix.pane.focus(0);
  }
}

// Right/l：目录 → 展开并选中第一个子项；文件 → 无操作
function enter_dir() {
  const hit = node_at(S.cursor);
  if (!hit || !hit.node.is_dir) return;
  if (!hit.node.expanded) {
    ensure_children(hit.node);
    hit.node.expanded = true;
  }
  const rows = visible_rows(S.tree, S.show_hidden, 0, []);
  const idx = rows.findIndex((r) => r.node.path === hit.node.path);
  if (idx >= 0 && idx + 1 < rows.length && rows[idx + 1].depth > hit.depth) {
    S.cursor = idx + 1;
  }
}

// Left/h：目录展开 → 折叠；否则 → 选中父节点
function leave_dir() {
  const hit = node_at(S.cursor);
  if (!hit) return;
  if (hit.node.is_dir && hit.node.expanded) {
    hit.node.expanded = false;
    return;
  }
  // 文件或已折叠目录：定位到父节点
  const rows = visible_rows(S.tree, S.show_hidden, 0, []);
  for (let i = S.cursor - 1; i >= 0; i--) {
    if (rows[i].depth < hit.depth) {
      S.cursor = i;
      return;
    }
  }
}

// 重建以 dir 为根的树
function set_root(dir) {
  S.root = dir;
  S.tree = make_tree(dir, sort_entries(read_dir_sync(dir)));
  S.cursor = 0;
}

// 当前选中节点的"目录上下文"：目录 → 自身；文件 → 父目录
function context_dir() {
  const hit = node_at(S.cursor);
  if (!hit) return S.root;
  if (hit.node.is_dir) return hit.node.path;
  const p = find_parent(S.tree, hit.node.path);
  return p ? p.path : S.root;
}

// 刷新：重读所有已展开目录，尽量恢复光标位置
function reload_tree() {
  if (S.root === null) return; // 还没开过 / cwd 变了但面板关着 ⇒ 没有树可刷（下次 open_panel 重建）
  const expanded = collect_expanded(S.tree, []);
  const prev = (node_at(S.cursor) || {}).node && node_at(S.cursor).node.path;
  S.tree = make_tree(S.root, sort_entries(read_dir_sync(S.root)));
  for (const p of expanded) {
    const n = find_node(S.tree, rel_parts(S.root, p) || []);
    if (n && n.is_dir) {
      ensure_children(n);
      n.expanded = true;
    }
  }
  if (prev) {
    const rows = visible_rows(S.tree, S.show_hidden, 0, []);
    const idx = rows.findIndex((r) => r.node.path === prev);
    S.cursor = idx >= 0 ? idx : 0;
  } else {
    S.cursor = 0;
  }
}

// 定位当前文件：展开路径链并选中它；文件不在 root 下时重设 root
function reveal(file_path) {
  if (!file_path) return;
  if (S.root === null) {
    set_root(file_path.split("/").slice(0, -1).join("/") || "/");
    return;
  }
  const parts = rel_parts(S.root, file_path);
  if (parts === null) {
    const dir = file_path.split("/").slice(0, -1).join("/") || "/";
    set_root(dir);
  }
  const chain = rel_parts(S.root, file_path);
  if (chain === null) return;
  const parent_parts = chain.slice(0, -1);
  if (parent_parts.length) {
    const parent = find_node(S.tree, parent_parts);
    if (parent && parent.is_dir) {
      ensure_children(parent);
      parent.expanded = true;
      // 逐级展开中间目录
      for (let i = 1; i < parent_parts.length; i++) {
        const n = find_node(S.tree, parent_parts.slice(0, i + 1));
        if (n && n.is_dir && n.children) {
          n.expanded = true;
          ensure_children(n);
        }
      }
    }
  }
  const rows = visible_rows(S.tree, S.show_hidden, 0, []);
  const idx = rows.findIndex((r) => r.node.path === file_path);
  if (idx >= 0) S.cursor = idx;
}

// ============================ 渲染(helix.draw 样板)============================
//
// 面板骨架:头(根名 + 状态标记)→ 分隔线 → **滚动树**。
// 核心已经画了面板边框 ⇒ 这里的 `(0,0)` 就是边框内区的左上角。
//
// 树的写法是关键:用 `push_scroll` 的**内容坐标系** —— 内容第 i 行就写在 y = list_top + i,
// 滚动时只改 `dy`,屏幕 y 由光栅化统一算。旧实现是每帧在 JS 里 `idx - start` 手动折算,
// 折错一次就是整棵树错位。

/// 纯逻辑:可见窗口。返回 `[{ row, idx }]`,`idx` = **内容行号**(与屏幕无关)
function window_rows(rows, top, list_h) {
  const out = [];
  const end = Math.min(rows.length, top + list_h);
  for (let idx = Math.max(0, top); idx < end; idx++) out.push({ row: rows[idx], idx });
  return out;
}

/// 纯逻辑:让光标可见的滚动位置(= 内容第 top 行顶到窗口上沿)。
/// 以光标为中心,再夹到 `[0, total - list_h]`;列表装得下时恒为 0。
function scroll_top(cursor, total, list_h) {
  if (list_h <= 0 || total <= list_h) return 0;
  const centered = cursor - Math.floor(list_h / 2);
  return Math.max(0, Math.min(centered, total - list_h));
}

const HEADER_H = 2; // 头 + 分隔线

function render(_focus, ctx) {
  const w = Math.max(1, (ctx && ctx.width) || 32);
  const h = Math.max(1, (ctx && ctx.height) || 20);
  try {
    const rows = visible_rows(S.tree, S.show_hidden, 0, []);
    const list_top = HEADER_H;
    const list_h = Math.max(1, h - list_top);
    const top = scroll_top(S.cursor, rows.length, list_h);

    // ── 头:根目录名 + 开关状态(H 隐藏 / F 跟随)──
    helix.draw.text(1, 0, [
      { text: basename(S.root || "/"), style: "ui.text" },
      { text: (S.show_hidden ? "  H" : "") + (S.follow ? "  F" : ""), style: "ui.virtual" },
      // 头从 x=1 起 ⇒ 可用宽度是 w-1(用 w-2 会把尾巴上的状态标记截掉,实测踩过)
    ], { maxWidth: Math.max(1, w - 1) });
    helix.draw.hline(0, 1, w, { style: "ui.virtual" });

    // ── 树:内容坐标 + 滚动偏移 ──
    helix.draw.push_scroll(0, list_top, w, list_h, { dy: top });
      for (const { row, idx } of window_rows(rows, top, list_h)) {
        const y = list_top + idx;             // 内容坐标(与屏幕无关)
        const { node, depth } = row;
        const is_sel = idx === S.cursor;
        const is_cur = node.path === S.current;
        if (is_sel) helix.draw.fill(0, y, w, 1, { style: "ui.selection" });
        // 图标:直接用核心 API(单一来源);关图标时目录回退 ▸/▾、文件留空格(保持两格对齐)
        const glyph = node.is_dir
          ? helix.icons.dir(node.expanded) + " "
          : (helix.icons.file(node.name) || " ") + " ";
        const style = is_sel ? "ui.selection" : is_cur ? "ui.popup" : node.is_dir ? "ui.virtual" : "ui.text";
        helix.draw.text(1, y, [
          { text: "  ".repeat(depth) + glyph, style },
          { text: node.name, style },
        ], { maxWidth: Math.max(1, w - 2) });
        // 当前文件:右缘圆点(选中底色之上仍可见)
        if (is_cur && w > 3) helix.draw.text(w - 2, y, "●", { style: "ui.text.focus" });
      }
    helix.draw.pop();

    // ── 超大目录:末尾提示(与旧行为一致)──
    if (rows.length > 20000) {
      helix.draw.text(1, h - 1, "... " + (rows.length - top - list_h) + " 行未显示", {
        style: "ui.virtual",
        maxWidth: Math.max(1, w - 2),
      });
    }
    return helix.draw.done();
  } catch (e) {
    // 绘制出错也要给出**可见**内容(否则面板一片空白,连报错都看不到)
    helix.draw.text(0, 0, "filetree 渲染错误: " + (e && e.message ? e.message : e), {
      style: "ui.popup",
      maxWidth: w,
    });
    return helix.draw.done();
  }
}

// ============================ 面板 ============================

function open_panel() {
  if (S.root === null) {
    // 首次打开：取**当前**工作目录（现取；`:cd` 后由 cwd-change 处理器重设）
    try {
      set_root(helix.cwd() || "/");
    } catch (e) {
      set_root("/");
    }
  }
  S.panel_id = helix.open_panel({
    side: "left",
    size: 32,
    render,
    onKey: handle_key,
  });
}

function close_panel() {
  if (S.panel_id !== null) {
    helix.close_panel(S.panel_id);
    S.panel_id = null;
  }
}

function toggle_panel() {
  if (S.panel_id !== null) close_panel();
  else open_panel();
}

// ============================ 输入/确认弹窗 ============================

// 文本输入弹窗：字符键直接拼输入（无焦点依赖——input 组件聚焦时 Enter 会被吞）
// 按键：普通字符/Backspace 编辑 → Enter 提交 → Esc 取消
function prompt(title, initial, on_submit) {
  S.prompt_value = initial;
  helix.open_popup({
    width: 50,
    height: 5,
    render: () =>
      helix.el("col", [
        helix.el("text", title, { style: "ui.virtual" }),
        helix.el("text", "> " + S.prompt_value + (S.prompt_value.length < 40 ? "_" : ""), { style: "ui.popup" }),
        helix.el("text", "Enter 确认 · Esc 取消", { style: "ui.help" }),
      ], { gap: 1 }),
    onKey: (key) => {
      if (key.ctrl || key.alt) return "handled";
      if (key.name === "Enter") {
        const v = S.prompt_value;
        S.prompt_value = "";
        on_submit(v);
        return "close";
      }
      if (key.name === "Esc") return "close";
      if (key.name === "Backspace") {
        S.prompt_value = S.prompt_value.slice(0, -1);
        return "handled";
      }
      if (key.name.length === 1) {
        S.prompt_value += key.name;
        return "handled";
      }
      return "handled";
    },
  });
}

// 确认弹窗（无输入）：Enter 确认 / Esc 取消
function confirm(title, on_ok) {
  helix.open_popup({
    width: 60,
    height: 4,
    render: () =>
      helix.el("col", [
        helix.el("text", title, { style: "ui.popup" }),
        helix.el("text", "Enter 确认 · Esc 取消", { style: "ui.help" }),
      ], { gap: 1 }),
    onKey: (key) => {
      if (key.name === "Enter") {
        on_ok();
        return "close";
      }
      if (key.name === "Esc") return "close";
      return "handled";
    },
  });
}

// ============================ 文件操作 ============================

async function run_fs(cmd, ok_msg) {
  try {
    await helix.run_async(cmd);
    if (ok_msg) helix.echo(ok_msg);
    reload_tree();
  } catch (err) {
    helix.echo("filetree: " + err.message);
  }
}

function new_file() {
  const dir = context_dir();
  prompt("新建文件（" + dir + "）", "", (name) => {
    const n = name.trim();
    if (!n) return;
    run_fs("touch " + shq(dir + "/" + n), "已创建 " + n);
  });
}

function new_dir() {
  const dir = context_dir();
  prompt("新建目录（" + dir + "）", "", (name) => {
    const n = name.trim();
    if (!n) return;
    run_fs("mkdir -p " + shq(dir + "/" + n), "已创建目录 " + n);
  });
}

function rename() {
  const hit = node_at(S.cursor);
  if (!hit) return;
  prompt("重命名 " + hit.node.name, hit.node.name, (name) => {
    const n = name.trim();
    if (!n || n === hit.node.name) return;
    run_fs("mv " + shq(hit.node.path) + " " + shq(hit.node.path.replace(/[^/]*$/, "") + n), "已重命名");
  });
}

function remove() {
  const hit = node_at(S.cursor);
  if (!hit) return;
  const cmd = hit.node.is_dir ? "rm -rf" : "rm -f";
  confirm("删除 " + (hit.node.is_dir ? "目录 " : "文件 ") + hit.node.path + " ?", () => {
    run_fs(cmd + " " + shq(hit.node.path), "已删除 " + hit.node.name);
  });
}

// ============================ 按键 ============================

function handle_key(key) {
  // C-\ 回编辑器（保留面板；与终端 C-\ 语义统一）
  if (key.ctrl && key.name === "\\") {
    helix.pane.focus(0);
    return "handled";
  }
  // 修饰键组合（C-x / M-x）**穿透给编辑器** —— 面板是非模态的（`open_panel`，不是 popup），
  // 用户在 `[keys]` / `helix.map` 里绑的 C-* 必须照常生效；被吞掉会变成"静默失效"。
  // 上面 C-\ 已单独处理（保留面板、焦点回编辑器），所以这里不需要再留特例。
  // 对照：`arsenal` / `dashboard` 是 popup（模态），它们在 onKey 里 `return "handled"` 是对的。
  if (key.ctrl || key.alt) return "ignore";
  switch (key.name) {
    case "Esc":
    case "q": close_panel(); return "handled";
    case "Up":
    case "k": move(-1); return "handled";
    case "Down":
    case "j": move(1); return "handled";
    case "PageUp": move(-10); return "handled";
    case "PageDown": move(10); return "handled";
    case "Home": S.cursor = 0; return "handled";
    case "End": S.cursor = Math.max(0, count_visible() - 1); return "handled";
    case "Enter":
    case "o": open_selected(); return "handled";
    case "Right":
    case "l": enter_dir(); return "handled";
    case "Left":
    case "h": leave_dir(); return "handled";
    case "Backspace":
    case "P": {
      const hit = node_at(S.cursor);
      const dir = hit ? (hit.node.is_dir ? hit.node.path : (find_parent(S.tree, hit.node.path) || {}).path) : null;
      if (dir && dir !== S.root) set_root(dir);
      return "handled";
    }
    case "H": S.show_hidden = !S.show_hidden; return "handled";
    case "R": reload_tree(); return "handled";
    case "F": S.follow = !S.follow; helix.echo(S.follow ? "跟随当前文件: 开" : "跟随当前文件: 关"); return "handled";
    case "a": new_file(); return "handled";
    case "A": new_dir(); return "handled";
    case "m": rename(); return "handled";
    case "d": remove(); return "handled";
    case "?": show_help(); return "handled";
    // 未映射键穿透（面板非模态）：: 命令、i 等仍可用
    default: return "ignore";
  }
}

function show_help() {
  helix.echo(
    "filetree: Enter/o 打开 · h/l 折叠/进入 · ↑↓ 移动 · H 隐藏文件 · R 刷新 · " +
    "a 新建文件 · A 新建目录 · m 重命名 · d 删除 · P 上级 · F 跟随 · q/Esc 关闭"
  );
}

// ============================ 事件 ============================

function on_buffer_open(doc) {
  if (!doc || !doc.path) return;
  S.current = doc.path;
  if (S.follow && S.panel_id !== null) {
    reveal(doc.path);
  }
}

// ============================ 注册（node 环境跳过） ============================

if (typeof helix !== "undefined") {
  helix.register_command("filetree", toggle_panel, "切换文件树面板");
  helix.register_command("filetree-reveal", (ctx) => {
    // 命令 ctx 形状是 { doc: { path, ... }, cursor, selection }（兼容旧顶层 path）
    const p = ctx && (ctx.doc ? ctx.doc.path : ctx.path);
    if (p) {
      if (S.panel_id === null) open_panel();
      reveal(p);
    }
  }, "文件树定位当前文件");
  helix.on("buffer-open", on_buffer_open);
  // `:cd` / `:lcd` 之后跟着换根（事件发在真 chdir 之后 ⇒ `helix.cwd()` 已是新目录）。
  // 面板没开时只丢根、不动状态：下次 open_panel 会自己取新目录。
  helix.on("cwd-change", (doc, ev) => {
    if (S.panel_id === null) {
      S.root = null;
      return;
    }
    try {
      set_root((ev && ev.cwd) || helix.cwd() || "/");
    } catch (e) {
      helix.echo("filetree: 切换目录失败 " + (e && e.message ? e.message : e));
    }
  });
  helix.export({ toggle: toggle_panel, open: open_panel, reveal, set_root });
}

// ============================ node 自检导出 ============================

if (typeof module !== "undefined" && module.exports) {
  module.exports = {
    sort_entries, visible_rows, make_tree, rel_parts, find_node, find_parent,
    collect_expanded, shq, S, render, reveal, open_panel, window_rows, scroll_top,
  };
// render/open_panel 依赖 helix，node 测试需先 stub global.helix
}
