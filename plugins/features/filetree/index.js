// filetree.js — 类 lazyvim/neo-tree 的侧边文件树面板
// 依赖清单：icons（共享图标表，helix.load 自动先加载）
helix.plugin("filetree", { deps: ["lib/icons.js"] });

// 插件配置(方案 C):config.toml [plugins.filetree]
helix.define_config("filetree", {
  show_hidden: { type: "boolean", default: false, doc: "默认显示隐藏文件" },
  refresh_ms: { type: "number", default: 1000, doc: "自动刷新间隔(ms;0 = 关闭)" },
});
const CFG = helix.get_config("filetree") || {};
// 用法：init.js 里 helix.load("features/filetree/index.js")，:filetree 开关面板。
// 键位：Enter/o 打开或展开，h/l 折叠/进入，Up/Down 导航，H 隐藏文件，
//       R 刷新，a/A 新建文件/目录，m 重命名，d 删除，P 上级目录，F 跟随当前文件，q/Esc 关闭。
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

// 统一图标源（icons.js 的 exports；load 后填充，node 测试环境为 null 走字符回退）
let ICONS = null;

const S = {
  root: null,          // 当前树根目录
  tree: null,          // 根 Node（虚拟容器）
  cursor: 0,           // 选中行索引（visible_rows 序）
  show_hidden: false,  // H 切换
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
    helix.focus(0);
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

// ============================ 渲染 ============================

// 一行：缩进 + 箭头 + 名字；样式：选中 > 当前文件 > 目录
function row_el(row, selected, is_current) {
  const { node, depth } = row;
  const indent = "  ".repeat(depth);
  // 图标：目录（展开/折叠）或文件类型图标；无 icons.js 时回退 ▸/▾/空格
  let glyph;
  if (ICONS) {
    glyph = node.is_dir
      ? ICONS.getDirIcon(node.expanded) + " "
      : ICONS.getFileIcon(node.name) + " ";
  } else {
    glyph = node.is_dir ? (node.expanded ? "▾ " : "▸ ") : "  ";
  }
  const style = selected ? "ui.selection" : is_current ? "ui.popup" : node.is_dir ? "ui.virtual" : null;
  return helix.el("text", indent + glyph + node.name, { style });
}

function render(focus, ctx) {
  try {
    const rows = visible_rows(S.tree, S.show_hidden, 0, []);
    const h = Math.max(1, (ctx ? ctx.height : 30) - 1);
    // 手动窗口化：以 cursor 为中心截取 h 行（scroll 组件只会保留末行，需自行裁剪）
    const start = Math.max(0, Math.min(S.cursor - Math.floor(h / 2), rows.length - h));
    const window = rows.slice(start, start + h);
    const lines = window.map((r, i) => row_el(r, start + i === S.cursor, r.node.path === S.current));
    if (rows.length > 20000) {
      lines.push(helix.el("text", "... " + (rows.length - start - h) + " 行未显示（内容过多）", { style: "ui.virtual" }));
    }
    return helix.el("scroll", lines, { height: h });
  } catch (e) {
    return helix.el("col", [helix.el("text", "filetree 渲染错误: " + (e && e.message || e), { style: "ui.popup" })]);
  }
}

// ============================ 面板 ============================

function open_panel() {
  if (S.root === null) {
    // 首次打开：以启动目录为根（同步 run 仅此一次）
    try {
      set_root(helix.run("pwd").trim() || "/");
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
    helix.focus(0);
    return "handled";
  }
  // 修饰键组合（C-x / M-x）不处理，穿透
  if (key.ctrl || key.alt) return "handled";
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
  // 统一图标映射表（bufferline/树/状态栏/诊断共用）
  try {
    ICONS = helix.load("lib/icons.js") || null;
  } catch (e) {
    ICONS = null; // icons.js 缺失时退回 ▸/▾ 字符
  }
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
  helix.export({ toggle: toggle_panel, open: open_panel, reveal, set_root });
}

// ============================ node 自检导出 ============================

if (typeof module !== "undefined" && module.exports) {
  module.exports = { sort_entries, visible_rows, make_tree, rel_parts, find_node, find_parent, collect_expanded, shq, S, render, reveal, open_panel };
// render/open_panel 依赖 helix，node 测试需先 stub global.helix
}
