// filetree.js — 类 lazyvim/neo-tree 的侧边文件树面板
// 依赖清单：icons（共享图标表，helix.load 自动先加载）
// 图标已收进核心，不再需要 deps；列目录 ⇒ fs_read，文件操作走 shell(`run_async`) ⇒ process。
// **路径数学一律走 `helix.path_*`**：Windows 的分隔符是 `\`、`C:\Users\x` 没有 `/` 可切，
// 自己拼 `/` 与 `split("/")` 会让 basename 变成整条路径、reveal 永远不成立（实测过）。
// 根目录用 `helix.cwd()`（现取；旧写法 `helix.run("pwd")` 只能取一次且会陈旧）
helix.plugin("filetree", {
  deps: [],
  schema_version: 1,
  // fs_write：建文件/建目录走核心的 write_file / mkdir_async（比走 shell 更窄、更跨平台）；
  // process 仍要：重命名/删除需要平台 shell（mv/rm vs move/del/rmdir），全库列表用 `rg --files`
  permissions: { process: true, fs_read: true, fs_write: true },
  // 卸载/重载时释放 nucleo 会话（内核不做猜测性清理）
  onUnload: () => {
    // 内核不做猜测性清理：写盘 + 释放每个实例的 nucleo 会话都自己来
    if (typeof save_state === "function") save_state();
    if (typeof G !== "undefined" && G.panels) {
      for (const panel of G.panels.values()) {
        if (typeof drop_flat === "function") drop_flat(panel);
      }
    }
  },
});

// 插件配置(方案 C):config.toml [plugins.filetree]
helix.define_config("filetree", {
  show_hidden: { type: "boolean", default: false, doc: "默认显示隐藏文件" },
  filter: { type: "string", default: "", doc: "打开面板即生效的过滤表达式（与 `>` 里输入的同语法；空 = 不过滤）" },
  git_status: {
    type: "boolean",
    default: false,
    doc: "显示 git 状态列（M/A/D/?），并启用 `#M`/`#A`/`#D`/`#?` 谓词；打开面板与 R 时各读一次 git status",
  },
  close_on_esc: {
    type: "boolean",
    default: true,
    doc: "Esc 是否关闭面板：true（默认）关闭；false = 只把焦点还给编辑器、面板保留（关面板改用 q）",
  },
  max_panels: {
    type: "number",
    default: 2,
    doc: "最多同时挂几个文件树面板（1 = 退回单面板；rail 全局只能有一条，屏幕也有限）",
  },
  persist: {
    type: "boolean",
    default: false,
    doc: "记住上次的过滤表达式与展开的目录（写到 <用户插件根>/filetree-state.json）；默认关 = 不写盘",
  },
});
const CFG = helix.get_config("filetree") || {};

// ============================ 面板键位（作用域声明）============================
// 键位是**数据**，不是散在 handle_key 里的 switch 键名字面量：
//   · 用户可写 `[plugins.filetree.keys]` 改键（值 = `":命令"`），不用改插件源码
//   · `?` 帮助、`:keys` 末尾的「插件作用域」段、将来看 which-key 都是**同一份**数据
//   · 覆盖内置键时状态栏会有提示（"插件 x 覆盖了 …"），不再静默
// 设计：docs/superpowers/plans/2026-11-04-panel-keybinding-scopes.md
const SCOPE = "filetree";
/// 帮助弹层单独一个作用域：它是**模态**层，Esc 语义是"关闭"而不是 blur
/// （模态层声明 `esc:"blur"` 等于空动作 —— 按键仍先到它，见 compositor 的注释）。
const HELP_SCOPE = "filetree-help";
const TREE_KEYS = {
  q: { action: "close", desc: "关闭面板" },
  "<esc>": { action: "esc", desc: "关闭（close_on_esc=false ⇒ 只回编辑器）" },
  Up: { action: "up", desc: "上移" },
  k: { action: "up", desc: "上移" },
  Down: { action: "down", desc: "下移" },
  j: { action: "down", desc: "下移" },
  PageUp: { action: "page-up", desc: "上翻 10 行" },
  PageDown: { action: "page-down", desc: "下翻 10 行" },
  Home: { action: "top", desc: "到顶" },
  End: { action: "bottom", desc: "到底" },
  Enter: { action: "open", desc: "打开 / 展开" },
  o: { action: "open", desc: "打开 / 展开" },
  Right: { action: "enter-dir", desc: "进入目录" },
  l: { action: "enter-dir", desc: "进入目录" },
  Left: { action: "leave-dir", desc: "折叠 / 上级" },
  h: { action: "leave-dir", desc: "折叠 / 上级" },
  Backspace: { action: "up-root", desc: "把根切到上级目录" },
  P: { action: "up-root", desc: "把根切到上级目录" },
  H: { action: "toggle-hidden", desc: "显示/隐藏点文件" },
  R: { action: "reload", desc: "刷新树 + git 状态" },
  F: { action: "toggle-follow", desc: "跟随当前文件" },
  a: { action: "new-file", desc: "新建文件" },
  A: { action: "new-dir", desc: "新建目录" },
  m: { action: "rename", desc: "重命名" },
  d: { action: "remove", desc: "删除" },
  "?": { action: "help", desc: "显示键位" },
  "/": { action: "find", desc: "按名字搜索（Tab 切全库）" },
  G: { action: "grep", desc: "全库内容搜索" },
  ">": { action: "filter", desc: "过滤表达式" },
  "C-\\": { action: "leave", desc: "回编辑器（保留面板）" },
};
if (helix.keymap && helix.keymap.scope) {
  helix.keymap.scope(SCOPE, {
    keys: TREE_KEYS,
    // B 阶段：这些**全局前缀**永远交回编辑器。面板本来就是非模态（未映射键穿透），
    // 这里主要给**弹层**用（见 show_help 的 `scope`）：不然 `?` 帮助开着时按 `:`/`C-w`
    // 会被弹层吃掉，用户视角就是"莫名其妙没反应"。
    // 注意**不要**把 `:` 加给文本输入弹层（prompt）——那里 `:` 是要输入的字符。
    passthrough: [":", "C-w", "C-p"],
    // 面板是**叶子式**非模态层：插件自己没消费 Esc 时，让核心把焦点挪回编辑器
    // （面板保留）。今天 filetree 自己处理了 Esc（close_on_esc 两态），所以这是
    // **声明意图 + 保险**：将来某条分支漏了 Esc，也不会漏成"Esc 落给编辑器改选区"。
    esc: "blur",
  });
  // 帮助弹层是**模态**层：Esc 的语义是"关掉它"，不能声明 blur（模态层 blur 等于空动作）。
  helix.keymap.scope(HELP_SCOPE, {
    keys: { "<esc>": { action: "close", desc: "关闭帮助" }, q: { action: "close", desc: "关闭帮助" } },
    passthrough: [":", "C-w", "C-p"],
  });
}

/// 查键 → 动作名（`null` = 没绑定 → 穿透）。旧编辑器没有 `helix.keymap` 时用内置表兜底，
/// 保证**行为不变**（只是不可覆盖、不进 `?`/`:keys`）。
function tree_action(key) {
  if (helix.keymap && helix.keymap.resolve) {
    const hit = helix.keymap.resolve(SCOPE, key);
    if (!hit) return null;
    if (hit.command) {
      // 用户把键覆盖成了命令 ⇒ 直接执行（`[plugins.filetree.keys]` 的语义）
      helix.run_command(hit.command);
      return "done";
    }
    return hit.action || null;
  }
  const nm = key && key.name !== undefined ? String(key.name) : String(key);
  if (key && key.ctrl && nm === "\\") return "leave";
  if (key && (key.ctrl || key.alt)) return null;
  const byName = {
    q: "close", Esc: "esc",
    Up: "up", k: "up", Down: "down", j: "down",
    PageUp: "page-up", PageDown: "page-down", Home: "top", End: "bottom",
    Enter: "open", o: "open", Right: "enter-dir", l: "enter-dir",
    Left: "leave-dir", h: "leave-dir", Backspace: "up-root", P: "up-root",
    H: "toggle-hidden", R: "reload", F: "toggle-follow",
    a: "new-file", A: "new-dir", m: "rename", d: "remove",
    "?": "help", "/": "find", G: "grep", ">": "filter",
  };
  return byName[nm] || null;
}
// 用法:init.js 里 helix.load("filetree")(裸名 → filetree/plugin.js),:filetree 开关面板。
// 键位：Enter/o 打开或展开，h/l 折叠/进入，Up/Down 导航，H 隐藏文件，
//       R 刷新，a/A 新建文件/目录，m 重命名，d 删除，P 上级目录，F 跟随当前文件，q/Esc 关闭。
//       **`/` 搜索**（按名字/路径过滤树；Enter 打开、目录上 Enter 钻进该目录、
//       Backspace 删字符、**Tab 切“全库”**（rg --files + nucleo）、Esc 逐级退出并回到原位置）。
//       **`:filetree-find`** 全库找文件（原生 picker；面板开着时选中后同步定位）。
//       **`G` / `:filetree-search <词>` 内容搜索**（rg 结果树：文件行为父、命中行为子，
//       Enter 跳到命中行、←/→ 折叠文件组）。
//       **`>` 过滤表达式**（glob / `#M`·`#?` 等 git 谓词 / 子串 / `!` 排除；空格分隔 = AND；
//       Enter 应用、Esc 放弃、删空 = 清除）。
// 行尾标记:● 当前文件 · · 已打开但非当前 · 诊断列（E/W/i/h）· git 列（M/A/D/R/?，`git_status` 开时）。
// 布局：面板在**根节点**上切（`open_panel({at:"root"})`）⇒ 它拿左边缘的**整列全高**，
//       之后的切分（编辑器 / 终端）都发生在右边 ⇒ `filetree | editor / term`。
// 状态分层：`G` 是编辑器事实/全局偏好（含 `panels` 注册表 + `active`），每面板一份的视图状态
// 在 `new_panel()` 的实例里；`:filetree-open` 开第二个（上限 `[plugins.filetree] max_panels`）。
// 设计：docs/superpowers/specs/2026-11-03-filetree-multi-instance-design.md
// 配置（config.toml `[plugins.filetree]`）：show_hidden · filter（初始表达式）· close_on_esc（false = Esc 只回焦点、面板保留）·
//       git_status（行尾 M/A/D/R/? 列 + `#` 谓词）· persist（记住表达式与展开目录，
//       **默认关**；写到 <用户插件根>/filetree-state.json —— 插件没有写 config.toml 的 API）。
//       **C-\ 保留面板、焦点回编辑器**；其余 C-*/M-* **穿透给编辑器**（面板非模态，
//       用户在 `[keys]` / `helix.map` 里绑的修饰键组合必须照常生效）。
// 搜索的已知天花板：① 内容搜索是**字面量**（`rg -F`，不解释正则）；过滤 DSL 有 git 谓词但**没有诊断谓词**
//       （`helix.diagnostics()` 只给聚合计数，拿不到逐文件映射）；
//       ② 诊断标记/`#E` 谓词**当前只对"当前文档"生效**（核心 `emit_lsp_diagnostics` 还没有调用点，
//       见设计文档 §14）；树范围只能看到“已加载 + 补加载预算内”的节点（EAGER_DIRS/EAGER_ENTRIES）——
//       跨预算/大仓库用 Tab 切**全库**（`rg --files`，尊重 .gitignore；无 rg 时降级 read_tree）；
//       ③ 手写输入**不支持粘贴/IME**（与 `prompt()` 同一套按键拼接）。方案见
//       docs/superpowers/specs/2026-11-02-filetree-search-design.md。
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
    name: helix.path_name(root_path) || root_path,
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
  // 前缀也走 join：Windows 上直接 `root + "/"` 拼出来的前缀永远比真实路径多一个 `/`
  const prefix = helix.path_join(root, "");
  if (!file_path.startsWith(prefix)) return null;
  // 相对部分按**平台分隔符**切（`\` 或 `/`）
  return file_path.slice(prefix.length).split(/[\\/]/).filter(Boolean);
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

// ============================ 纯逻辑：搜索/过滤（不依赖 helix 状态，node 可测）============================

// 大小写不敏感的子序列匹配。命中返回 name 里的字符下标（供高亮），不匹配返回 null。
// 空 query 返回 null —— 调用方在 query 为空时根本不进过滤路径（等价 tree 视图）。
function match_name(name, query) {
  if (!query) return null;
  const q = String(query).toLowerCase();
  const low = String(name).toLowerCase();
  const hits = [];
  let qi = 0;
  for (let i = 0; i < low.length && qi < q.length; i++) {
    if (low[i] === q[qi]) {
      hits.push(i);
      qi++;
    }
  }
  return qi === q.length ? hits : null;
}

// 相对根的显示路径（统一用 `/` 显示；只用于匹配与展示，不参与文件系统操作）
function rel_display(root_path, path) {
  const parts = rel_parts(root_path, path);
  return parts ? parts.join("/") : String(path);
}

// 单个节点是否命中：query 含 `/` ⇒ 按相对路径做**子串**匹配（子序列在路径上假阳性太多）；
// 否则按名字做子序列匹配。返回命中下标（`[]` = 命中但不高亮），不命中返回 null。
function node_match(node, rel, query) {
  if (query.indexOf("/") >= 0) {
    return rel.toLowerCase().indexOf(query.toLowerCase()) >= 0 ? [] : null;
  }
  return match_name(node.name, query);
}

// 过滤视图：保留命中节点的**全部祖先**，`depth` 保持真实层级（缩进不塌，用户知道自己在哪）。
// 返回 `[{ node, depth, hit }]`：hit = 名字内的命中下标，`null` = 仅作为祖先保留（不高亮）。
// 树序 = `visible_rows` 序（不按 score 排）——`reload_tree`/`reveal` 都假设"行序 = 树序"。
// 只看**已加载**节点（`children === null` 的懒加载目录不看内部；补加载见 eager_load_once）。
function filter_rows(root, show_hidden, query) {
  if (!root || !query) return [];
  return match_tree(root, show_hidden, (node, rel) => node_match(node, rel, query));
}

// 诊断级别（纯逻辑）：`severity` 是核心给的**字符串**（"error"/"warning"/"info"/"hint"），
// 数组里取**最严重**的那一个。空数组 / 全是未知级别 → null。
const DIAG_RANK = { error: 1, warning: 2, info: 3, hint: 4 };

function worst_severity(diags) {
  let best = null;
  let bestRank = 99;
  for (const d of diags || []) {
    const sev = d && typeof d.severity === "string" ? d.severity.toLowerCase() : "";
    const r = DIAG_RANK[sev];
    if (r === undefined) continue;
    if (r < bestRank) {
      bestRank = r;
      best = sev;
    }
  }
  return best;
}

// `#E`/`#W`/`#I`/`#H` 谓词（纯逻辑）：`W` = warning **或更严重**，`H` = 任意级别
function diag_hit(entry, code) {
  if (!entry) return false;
  const want = { E: 1, W: 2, I: 3, H: 4 }[String(code || "").toUpperCase()];
  const have = DIAG_RANK[entry.severity];
  return want !== undefined && have !== undefined && have <= want;
}

// 文件行右缘标记（纯逻辑）：`●` 当前文件 · `·` 已打开但非当前 · null 未打开。
// 为什么要区分：`●` 之前只标"当前"，切走之后面板就再没有任何线索告诉你"哪些文件还开着"。
function opened_marker(path, current, opened) {
  if (!path) return null;
  // `opened` 还没播种时退化成老行为（只看 current），别让面板开局一个标记都没有
  const has = opened && typeof opened.has === "function";
  const is_open = !has || opened.has(path);
  if (path === current && is_open) return { ch: "●", style: "ui.text.focus" };
  // `current` 只在 buffer-open 更新 ⇒ 关掉当前 buffer 后它会陈旧；用 opened 兜一层，
  // 免得给已经不在缓冲区里的文件画 `●`（真·当前文档要等下一次 buffer-open 才纠正）
  if (has && opened.has(path)) return { ch: "·", style: "ui.virtual" };
  return null;
}

// 把命中下标切成连续段（纯逻辑，渲染层再决定样式）：`[{ text, hit }]`
function highlight_segments(name, hits) {
  if (!hits || hits.length === 0) return [{ text: name, hit: false }];
  const set = new Set(hits);
  const out = [];
  for (let i = 0; i < name.length; i++) {
    const hit = set.has(i);
    const last = out[out.length - 1];
    if (last && last.hit === hit) last.text += name[i];
    else out.push({ text: name[i], hit });
  }
  return out;
}

// 内容搜索的高亮：query 的**每一次字面出现**都标上（大小写不敏感，重叠不走回头路）
const LITERAL_HITS_MAX = 400; // 一行里的高亮下标上限（防止超长行/超短 query 把数组撑爆）
function literal_hits(text, query) {
  if (!text || !query) return null;
  const low = String(text).toLowerCase();
  const q = String(query).toLowerCase();
  if (!q.length) return null;
  const hits = [];
  let i = low.indexOf(q);
  while (i >= 0 && hits.length < LITERAL_HITS_MAX) {
    for (let k = 0; k < q.length && hits.length < LITERAL_HITS_MAX; k++) hits.push(i + k);
    i = low.indexOf(q, i + q.length);
  }
  return hits.length ? hits : null;
}

// ============================ 纯逻辑：过滤表达式（DSL）/ 通用过滤遍历 / git 状态 ============================

// `run_async` 失败消息的形态是 `exit <code>: <stderr>`。
// **只有 exit 1** 才是"rg 没找到匹配"（常规退出，不是错误）；写成 `/^exit 1/` 会前缀命中
// `exit 1xx` —— 例如没装 rg 时退出码 127 ⇒ 被当成"没有匹配"，于是**静默返回空列表、降级分支也
// 不走**（2026-11-04 CI 实测：GitHub runner 上没有 rg，全库列表恒为 0 项）。
function is_no_match(err) {
  const msg = String((err && err.message) || err);
  return /^exit 1(:|\s|$)/.test(msg);
}

// 小 glob → 正则：`**` 跨目录（含零层）、`*` 不跨 `/`、`?` 单字符、`[...]` 透传、其余字面量
function glob_to_regex(pat) {
  let re = "";
  for (let i = 0; i < pat.length; i++) {
    const c = pat[i];
    if (c === "*") {
      if (pat[i + 1] === "*") {
        i++;
        if (pat[i + 1] === "/") {
          i++;
          re += "(?:.*/)?"; // `src/**/a.rs` 也要能命中 `src/a.rs`
        } else {
          re += ".*";
        }
      } else {
        re += "[^/]*";
      }
    } else if (c === "?") {
      re += "[^/]";
    } else if (c === "[") {
      const j = pat.indexOf("]", i + 1);
      if (j > i) {
        re += pat.slice(i, j + 1);
        i = j;
      } else {
        re += "\\[";
      }
    } else {
      re += c.replace(/[.+^${}()|\\]/g, "\\$&");
    }
  }
  return new RegExp("^" + re + "$", "i");
}

// 过滤表达式 → 编译结果（`null` = 空表达式 = 不过滤）。词以空格分隔，**全部**命中才算命中；
// 单个词前缀 `!` = 排除。三种词形态：
//   `*.rs` / `src/**` / `test_?.js`  glob（不含 `/` → 比 basename；含 `/` → 比相对路径）
//   `#M` `#A` `#D` `#?`              git 状态谓词（需 `[plugins.filetree] git_status = true`）
//   其它                             相对路径上的子串（大小写不敏感）
function compile_filter(expr) {
  const text = String(expr || "").trim();
  if (!text) return null;
  const items = [];
  for (const raw of text.split(/\s+/)) {
    if (!raw) continue;
    const negate = raw[0] === "!";
    const body = negate ? raw.slice(1) : raw;
    if (!body) continue;
    if (body[0] === "#") {
      const code = body.slice(1).toUpperCase();
      // `#E/#W/#I/#H` 是诊断谓词，其余（M/A/D/R/?）是 git 状态谓词
      const kind = /^[EWIH]$/.test(code) ? "diag" : "git";
      items.push({ negate, kind, code });
    } else if (/[*?[]/.test(body)) {
      items.push({ negate, kind: "glob", re: glob_to_regex(body), path_mode: body.indexOf("/") >= 0 });
    } else {
      items.push({ negate, kind: "substr", text: body.toLowerCase() });
    }
  }
  return items.length ? { items, expr: text } : null;
}

// 单个节点是否过得了这个表达式（纯逻辑；git 是 Map<绝对路径, 单字母>，没有就传 null）
function dsl_hit(flt, node, rel, git, diag) {
  if (!flt) return true;
  for (const it of flt.items) {
    let m;
    if (it.kind === "git") {
      const code = git ? git.get(node.path) : undefined;
      m = !!code && (it.code === "?" ? code === "?" : code === it.code);
    } else if (it.kind === "diag") {
      m = diag_hit(diag ? diag.get(node.path) : null, it.code);
    } else if (it.kind === "glob") {
      m = it.re.test(it.path_mode ? rel : basename_of(rel));
    } else {
      m = rel.toLowerCase().indexOf(it.text) >= 0;
    }
    if (it.negate ? m : !m) return false;
  }
  return true;
}

// 相对路径的最后一段（只用于 DSL 的 basename 匹配；不做路径运算）
function basename_of(rel) {
  const parts = String(rel).split("/");
  return parts[parts.length - 1] || "";
}

// 纯逻辑：过滤视图的通用实现。`pred(node, rel)` 命中返回 **命中下标数组**（`[]` = 命中不高亮），
// 不命中返回 `null`。命中节点的**全部祖先**一并保留，`depth` 保持真实层级（缩进不塌）。
// 只看**已加载**的节点（懒加载目录的内部由 eager_load_once 补）。
function match_tree(root, show_hidden, pred) {
  const out = [];
  if (!root) return out;
  const walk = (node, depth) => {
    let any = false;
    for (const c of node.children || []) {
      if (!show_hidden && c.name.startsWith(".")) continue;
      const hit = pred(c, rel_display(root.path, c.path));
      const at = out.length;
      if (hit) {
        out.push({ node: c, depth, hit });
        any = true;
      }
      const sub = walk(c, depth + 1);
      if (sub && !hit) out.splice(at, 0, { node: c, depth, hit: null });
      if (sub) any = true;
    }
    return any;
  };
  walk(root, 0);
  return out;
}

// 纯逻辑：`git status --porcelain -z` 的输出 → Map<绝对路径, 单字母>
// `-z`：NUL 分隔、路径不转义（含空格/中文都原样）；重命名/复制占**两段**（新路径 + 原路径）
function parse_git_status(out, root) {
  const map = new Map();
  const parts = String(out).split("\0");
  for (let i = 0; i < parts.length; i++) {
    const e = parts[i];
    if (e.length < 4) continue;
    const x = e[0];
    const y = e[1];
    map.set(helix.path_join(root, e.slice(3)), git_letter(x, y));
    if (x === "R" || x === "C" || y === "R" || y === "C") i++; // 跳过原路径
  }
  return map;
}

// XY 两列 → 一个字母（`?` 未跟踪优先；其次 D/A/R，其余都算 M）
function git_letter(x, y) {
  if (x === "?" || y === "?") return "?";
  if (x === "D" || y === "D") return "D";
  if (x === "A" || y === "A") return "A";
  if (x === "R" || y === "R") return "R";
  return "M";
}

// 纯逻辑：`rg --json` 的输出 → 分组 `[{ path, rel, matches: [{ line, text }] }]`
// - `line` 是 **0 起始**（与 `helix.open_file({row})` 同口径）；rg 给的是 1 起始，这里减 1
// - 只认 `type: "match"` 事件（`begin`/`end`/`summary`/`context` 全跳过）
// - 不用 `path:line:text` 的 `split(":")`：Windows 上 `C:\\repo\\a.rs:12:x` 会被切错
//   （`plugins/examples/picker.js:40` 就是这个写法，Unix 对 / Windows 错）
function grep_groups(raw, root, limits) {
  const max_files = (limits && limits.files) || 500;
  const max_matches = (limits && limits.matches) || 2000;
  const per_file = (limits && limits.per_file) || 20;
  const groups = [];
  const by_path = new Map();
  let total = 0;
  for (const line of String(raw).split("\n")) {
    if (!line.trim()) continue;
    let ev;
    try {
      ev = JSON.parse(line);
    } catch (e) {
      continue; // 非 JSON 行不该出现（--json 保证），出现了也不该弄死整个搜索
    }
    if (!ev || ev.type !== "match" || !ev.data) continue;
    const d = ev.data;
    const path = d.path && d.path.text;
    if (!path) continue; // 非 UTF-8 路径只有 bytes 形式 ⇒ 跳过（宁缺不编）
    const text = String((d.lines && d.lines.text) || "").replace(/\r?\n$/, "");
    const ln = typeof d.line_number === "number" ? d.line_number - 1 : 0;
    let g = by_path.get(path);
    if (!g) {
      if (groups.length >= max_files || total >= max_matches) break;
      g = { path, rel: rel_display(root, path), matches: [] };
      by_path.set(path, g);
      groups.push(g);
    }
    if (g.matches.length >= per_file) continue;
    if (total >= max_matches) break;
    g.matches.push({ line: ln, text });
    total++;
  }
  return groups;
}

// shell 单引号转义
// 平台识别：核心给的 join 用的是**本平台**分隔符，据此判 Windows（比猜 USERPROFILE 可靠）
const IS_WINDOWS = helix.path_join("a", "b").includes("\\");

// shell 引号：POSIX 单引号 vs cmd 双引号（cmd 不认单引号，这是"换个平台就炸"的典型）
function shq(s) {
  s = String(s);
  if (IS_WINDOWS) {
    return '"' + s.replace(/"/g, '""') + '"';
  }
  return "'" + s.replace(/'/g, "'\\''") + "'";
}

// ============================ 状态 ============================
//
// **归属规则**（加字段前先想清楚属于哪一边 —— 两个面板"串味"的根因永远是这一行）：
//   G（全局）：编辑器事实（current/opened/diag）与全局偏好（show_hidden/follow），以及注册表
//   panel（实例）：一切"这个视图怎么摆"的东西（root/tree/cursor/mode/query/filter/索引/git）
// 设计：docs/superpowers/specs/2026-11-03-filetree-multi-instance-design.md §4.1

// 统一图标源已收进核心(helix-js/src/icons.rs):直接用 `helix.icons.*`,零依赖

const G = {
  panels: new Map(),   // id -> 面板实例（第 3 步起是注册表；第 4 步才允许放第二个）
  active: null,        // 最近一次收到按键的实例 id（核心没有焦点事件，见设计文档 §2）
  current: null,       // 当前编辑文件路径（buffer-open 更新）
  opened: null,        // Set<path>：已打开的文件（buffer.list 播种 + open/close 事件维护）
  diag: null,          // Map<绝对路径, {severity, count}>：最严重级别（见设计文档 §14）
  show_hidden: CFG.show_hidden === true, // 初值取 [plugins.filetree] show_hidden;H 切换
  follow: true,        // F 切换：buffer-open 时自动定位当前文件
  last_cwd: null,      // cwd-change 时判断"哪些实例的 root 就是旧 cwd"
};

// 面板实例：每面板一份的**视图状态**
function new_panel(root) {
  return {
    id: null,          // helix.open_panel 的返回值（null = 面板没开）
    root: root === undefined ? null : root,
    tree: null,        // 根 Node（虚拟容器）
    cursor: 0,         // 选中行索引（rows_for_view(panel) 序）
    mode: "tree",      // "tree"（导航） | "find"（按名字/路径） | "filter"（表达式） | "grep"（按内容）
    scope: "tree",     // find 范围："tree"（当前树） | "all"（全库，rg --files + nucleo）
    query: "",         // 搜索/过滤的输入串；空串 = 不过滤（等价 tree 视图）
    pre_query: null,   // 进搜索前光标所在节点路径（Esc 退出时恢复，别让用户丢位置）
    filter: "",        // 已**应用**的过滤表达式（树模式生效；`>` 里输入的是草稿）
    flat: null,        // 全库索引 { root, items, loading, error, sid, cache_key, cache_rows }
    grep: null,        // 内容搜索结果 { groups, loading, error, seq }
    git: null,         // Map<绝对路径, 单字母>（每个 root 一份：两个实例可能是两个仓库）
    eager_done: false, // 本根是否已做过一次"补加载"（eager_load_once；换根/刷新后重置）
  };
}

// 最多同时挂几个面板（第 4 步放开双实例；rail 全局只有一条，屏幕也就那么大）
const MAX_PANELS = typeof CFG.max_panels === "number" && CFG.max_panels > 0 ? CFG.max_panels : 2;

// 目标实例 = **优先级阶梯**（设计文档 §4.3）：
//   ① `G.active`（最近一次收到按键的实例 —— 核心没有焦点事件，见 §2）
//   ② 只剩一个实例时就是它
//   ③ 都没有 → null（由调用方决定是否新建）
function target_panel() {
  const active = G.active !== null ? G.panels.get(G.active) : null;
  if (active) return active;
  const all = Array.from(G.panels.values());
  return all.length === 1 ? all[0] : null;
}

function first_panel() {
  const it = G.panels.values().next();
  return it.done ? null : it.value;
}

// 解析"这个文件该落在哪个面板"：**归属优先**（root 是它的祖先）→ 退回 active/唯一。
// 顺序与设计文档 §4.3 相反（那里写的是 active 优先）：实测那样会让 reveal 把 active 面板的
// root 整个拽走，而"文件属于谁"是无歧义信息，先用它更符合预期。
function resolve_panel(path) {
  return panel_for_path(path) || target_panel();
}

// root 是 `path` 祖先的实例
function panel_for_path(path) {
  if (!path) return null;
  for (const panel of G.panels.values()) {
    if (!panel.root) continue;
    if (path === panel.root) return panel;
    if (path.startsWith(helix.path_join(panel.root, ""))) return panel;
  }
  return null;
}

// ============================ 目录读取 ============================

// `quiet`：批量补加载（eager_load）时每个没权限的目录都 echo 一条会刷屏 ⇒ 静默失败
function read_dir_sync(dir, quiet) {
  try {
    return helix.read_dir(dir);
  } catch (e) {
    if (!quiet) helix.echo("filetree: 读取目录失败 " + dir + ": " + e.message);
    return [];
  }
}

// 路径数学一律走核心：Windows 上 `\` 是分隔符、`C:\Users\x` 没有 `/` 可切
function basename(p) {
  return helix.path_name(p) || p;
}

// 目录节点的子节点（懒加载 + 缓存）
function ensure_children(node, quiet) {
  if (node.children === null) {
    node.children = sort_entries(read_dir_sync(node.path, quiet)).map((e) => ({
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

// ============================ 全库索引（rg --files + nucleo）============================
//
// 树过滤只能看到"已加载 + 预算内"的节点 ⇒ 跨预算/超大仓库还得有全库列表。
// 为什么是 `rg --files` 而不是 `read_tree`/`glob_async`：后者是裸 `fs::read_dir` 递归
// （helix-js/src/shell.rs 的 tree_matches/glob_matches），**不读 .gitignore**，
// 在 Rust 仓库里会把 target/ 几十万个文件全列出来。rg 才是"尊重 ignore 的列表"。
// argv 形式（不过 shell）⇒ 路径带空格/引号/中文/Windows 盘符都不会被 shell 解释。

const FLAT_LIMIT = 200;      // 展示/打分的上限（query 非空时参与排序的条目数）
const FLAT_MAX_ITEMS = 50000; // 索引上限：session 构建成本随条目线性增长，超出就不再收纳

function base_dir(panel) {
  return panel.root || helix.cwd() || "/";
}

// 纯逻辑：`rg --files` 的输出 → flat 条目 `[{ path, rel }]`（去空行/去 CR，相对根的显示名）
function flat_items(raw, root) {
  const out = [];
  for (const line of String(raw).split("\n")) {
    const p = line.replace(/\r$/, "").trim();
    if (!p) continue;
    out.push({ path: p, rel: rel_display(root, p) });
    if (out.length >= FLAT_MAX_ITEMS) break;
  }
  return out;
}

function list_files(root) {
  return helix.run_async(["rg", "--files", "--", root]).then(
    (out) => flat_items(out, root),
    (err) => {
      // rg 无匹配 = exit 1（常规退出，不是错误）⇒ 空表
      if (is_no_match(err)) return [];
      // 真的没 rg / 参数错 ⇒ 降级 read_tree（不忽略 .gitignore，有 FLAT_MAX_ITEMS 封顶）
      helix.echo("filetree: 未找到 rg，已降级为目录遍历（不忽略 .gitignore）");
      return helix.read_tree(root, { depth: 12 }).then((es) =>
        flat_items(es.filter((e) => !e.is_dir).map((e) => e.path).join("\n"), root),
      );
    },
  );
}

// 丢掉索引 + 释放 nucleo 会话（换根 / 刷新 / 关面板 / 卸载都要走这里）
function drop_flat(panel) {
  if (panel.flat && panel.flat.sid !== null) helix.fuzzy.drop(panel.flat.sid);
  panel.flat = null;
}

function load_flat(panel) {
  const root = base_dir(panel);
  if (panel.flat && panel.flat.root === root && (panel.flat.items || panel.flat.loading)) return;
  drop_flat(panel);
  const f = { root, items: null, loading: true, error: null, sid: null, cache_key: null, cache_rows: [] };
  panel.flat = f;
  list_files(root).then(
    (items) => {
      if (panel.flat !== f) return; // 期间换了根/关了面板 ⇒ 这次结果作废
      f.loading = false;
      f.items = items;
      refresh_find(panel);
    },
    (err) => {
      if (panel.flat !== f) return;
      f.loading = false;
      f.error = (err && err.message) || String(err);
      refresh_find(panel);
    },
  );
}

// 合成的"文件节点"：`name` 用**相对路径**（全库视图只给 basename 会一堆同名）
function flat_node(it) {
  return { name: it.rel, path: it.path, is_dir: false, expanded: false, children: null };
}

// 全库视图的行。query 不变时直接用缓存 —— `fuzzy.run` 是离核调用（~2ms/5k 条），
// 而 `rows_for_view(panel)` 一次按键会被调好几次，不能每次都重跑。
function flat_rows(panel) {
  const f = panel.flat;
  if (!f || !f.items) return [];
  if (!panel.query) {
    return f.items.slice(0, FLAT_LIMIT).map((it) => ({ node: flat_node(it), depth: 0, hit: null }));
  }
  if (f.cache_key === panel.query) return f.cache_rows;
  if (f.sid === null) f.sid = helix.fuzzy.session(f.items.map((it) => it.rel));
  const rows = helix.fuzzy.run(f.sid, panel.query, { limit: FLAT_LIMIT }).map((h) => {
    const it = f.items[h.index];
    // nucleo 不给匹配位置（docs/api/fuzzy.md）⇒ 高亮下标自己算（只有 <=FLAT_LIMIT 行）
    return { node: flat_node(it), depth: 0, hit: match_name(it.rel, panel.query) || [] };
  });
  f.cache_key = panel.query;
  f.cache_rows = rows;
  return rows;
}

// 当前视图的行 —— **唯一**取行入口。tree 模式 = 展开可见项；find 模式 = 过滤后的行。
// 所有取行的地方（node_at / 光标 / 打开 / 改名 / 删除 / 渲染）都走这里 ⇒ 过滤视图下自动成立。
function rows_for_view(panel) {
  if (!panel.tree) return [];
  if (panel.mode === "grep") return grep_rows(panel);
  if (panel.mode === "find") {
    if (panel.scope === "all") return flat_rows(panel);
    if (!panel.query) return tree_rows(panel);
    const flt = panel.filter ? compile_filter(panel.filter) : null;
    return match_tree(panel.tree, G.show_hidden, (node, rel) => {
      // 名字搜索**叠加**已应用的过滤表达式（否则"过滤后又搜出被过滤掉的文件"很怪）
      if (flt && !dsl_hit(flt, node, rel, panel.git, G.diag)) return null;
      return node_match(node, rel, panel.query);
    });
  }
  // filter 模式：输入框里的是实时预览；树模式：应用过的表达式
  const expr = panel.mode === "filter" ? panel.query : panel.filter;
  const flt = expr ? compile_filter(expr) : null;
  if (!flt) return tree_rows(panel);
  return match_tree(panel.tree, G.show_hidden, (node, rel) =>
    dsl_hit(flt, node, rel, panel.git, G.diag) ? [] : null,
  );
}

function tree_rows(panel) {
  return visible_rows(panel.tree, G.show_hidden, 0, []);
}

function node_at(panel, idx) {
  return rows_for_view(panel)[idx] || null;
}

function count_visible(panel) {
  return rows_for_view(panel).length;
}

function move(panel, delta) {
  panel.cursor = Math.max(0, Math.min(panel.cursor + delta, count_visible(panel) - 1));
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
function open_selected(panel) {
  const hit = node_at(panel, panel.cursor);
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
function enter_dir(panel) {
  const hit = node_at(panel, panel.cursor);
  if (!hit || !hit.node.is_dir) return;
  if (!hit.node.expanded) {
    ensure_children(hit.node);
    hit.node.expanded = true;
  }
  const rows = visible_rows(panel.tree, G.show_hidden, 0, []);
  const idx = rows.findIndex((r) => r.node.path === hit.node.path);
  if (idx >= 0 && idx + 1 < rows.length && rows[idx + 1].depth > hit.depth) {
    panel.cursor = idx + 1;
  }
}

// Left/h：目录展开 → 折叠；否则 → 选中父节点
function leave_dir(panel) {
  const hit = node_at(panel, panel.cursor);
  if (!hit) return;
  if (hit.node.is_dir && hit.node.expanded) {
    hit.node.expanded = false;
    return;
  }
  // 文件或已折叠目录：定位到父节点
  const rows = visible_rows(panel.tree, G.show_hidden, 0, []);
  for (let i = panel.cursor - 1; i >= 0; i--) {
    if (rows[i].depth < hit.depth) {
      panel.cursor = i;
      return;
    }
  }
}

// ============================ git 状态列 ============================

// `git -C <root> status --porcelain -z`：一次调用拿全量；不是仓库/没装 git ⇒ null（整列不画）
function refresh_git(panel) {
  if (!CFG.git_status) {
    panel.git = null;
    return;
  }
  const root = base_dir(panel);
  helix
    .run_async(["git", "-C", root, "status", "--porcelain", "-z", "--untracked-files=all"])
    .then(
      (out) => {
        // 期间换过根 ⇒ 这份结果属于旧根，丢掉（否则会把上个项目的状态标在新树上）
        panel.git = root === base_dir(panel) ? parse_git_status(out, root) : null;
      },
      () => {
        panel.git = null; // 不是 git 仓库 / 没装 git：静默（这一列只是锦上添花）
      },
    );
}

// 行尾的状态标记（渲染用）：单字母 + 主题 scope
function git_mark(panel, path) {
  if (!panel.git) return null;
  const code = panel.git.get(path);
  if (!code) return null;
  const style =
    code === "?" ? "ui.virtual" : code === "M" || code === "D" ? "error" : "ui.popup";
  return { text: code, style };
}

// ============================ 诊断标记（数据来源见设计文档 §14）============================
//
// **现状**：核心的 `emit_lsp_diagnostics` 没有调用点 ⇒ `lsp-diagnostics` 事件今天不会触发。
// 所以当下只有"当前文档"的诊断可拿（`helix.diagnostics()` 就是这个口径，
// 配 `helix.buffer.current()` 才知道它属于哪个文件）。事件 handler 仍然注册 ——
// 核心一接上，逐文档标记与 `#E` 跨文件过滤就自动生效，不用改这里。

function set_diag(path, list) {
  if (!path) return;
  if (!G.diag) G.diag = new Map();
  const sev = worst_severity(list);
  if (sev) G.diag.set(path, { severity: sev, count: list.length });
  else G.diag.delete(path);
}

// 当前文档路径（`helix.buffer.current()` + `buffer.list()` 拼出来）
function current_doc_path() {
  try {
    const cur = helix.buffer.current();
    const list = helix.buffer.list() || [];
    const hit = list.find((b) => b.id === cur);
    return hit ? hit.path || null : null;
  } catch (e) {
    return null;
  }
}

// 拉一次当前文档的诊断（只在"人-rate"的触发点上调用：面板按键 / 打开 / R）
function poll_diagnostics() {
  if (!helix.diagnostics) return;
  const path = current_doc_path();
  if (!path) return;
  let diags = null;
  try {
    diags = helix.diagnostics();
  } catch (e) {
    return;
  }
  set_diag(path, Array.isArray(diags) ? diags : []);
}

// 逐文档诊断事件（core 接上 emit 后才有数据）
function on_lsp_diagnostics(docId, diags) {
  try {
    const list = helix.buffer.list() || [];
    const hit = list.find((b) => b.id === docId);
    if (hit && hit.path) set_diag(hit.path, Array.isArray(diags) ? diags : []);
  } catch (e) {
    // 事件里拿不到文档（已关闭）⇒ 忽略
  }
}

// 行尾诊断标记：最严重级别 → 单字母 + 主题 scope
function diag_mark(path) {
  if (!G.diag) return null;
  const e = G.diag.get(path);
  if (!e) return null;
  const ch =
    e.severity === "error" ? "E" : e.severity === "warning" ? "W" : e.severity === "info" ? "i" : "h";
  const style = e.severity === "error" ? "error" : e.severity === "warning" ? "warning" : "ui.virtual";
  return { ch, style };
}

// ============================ 状态持久化（可关，v2：多面板）============================
//
// 存哪儿：<用户插件根>/filetree-state.json（**不是** <根>/filetree/ —— 那会造出用户层目录，
// 让"目录级覆盖"看起来像已经覆盖了内置插件）。`plugins_dir()` 为 null（没有用户层）⇒ 不存。
//
// v2 形状（每个**根**一条，与"哪个面板"解耦：下次开机按 root 匹配回来）：
//   { "version": 2, "panels": [ { "root": "/p/a", "filter": "*.rs", "expanded": ["/p/a/src"] } ] }
// v1（单面板，`{version:1, root, filter, expanded}`）读入时按一条迁移 —— 老用户的文件不该白丢。

const STATE_FILE = "filetree-state.json";
const STATE_MAX_EXPANDED = 200; // 单条记录最多记多少个展开目录

function state_path() {
  const dir = helix.plugins_dir ? helix.plugins_dir() : null;
  return dir ? helix.path_join(dir, STATE_FILE) : null;
}

// 纯逻辑：面板快照 → 持久化字符串。
// `entries` 是 `[{root, filter, expanded}]`；按 root **去重（后者胜）**、截断到 max 条、每条展开目录封顶。
function state_encode(entries, max) {
  const by_root = new Map();
  for (const e of entries || []) {
    if (!e || typeof e.root !== "string" || !e.root) continue;
    by_root.set(e.root, {
      root: e.root,
      filter: typeof e.filter === "string" ? e.filter : "",
      expanded: Array.isArray(e.expanded) ? e.expanded.slice(0, STATE_MAX_EXPANDED) : [],
    });
  }
  const limit = typeof max === "number" && max > 0 ? max : 2;
  return JSON.stringify({ version: 2, panels: Array.from(by_root.values()).slice(0, limit) });
}

// 纯逻辑：持久化字符串 → `[{root, filter, expanded}]`。
// 兼容 v1（单条）、容忍 garbage（返回空表）—— 读盘路径不该因为一个坏文件崩。
function state_decode(raw) {
  let st;
  try {
    st = JSON.parse(String(raw));
  } catch (e) {
    return [];
  }
  if (!st || typeof st !== "object") return [];
  if (st.version === 2 && Array.isArray(st.panels)) {
    return st.panels
      .filter((e) => e && typeof e.root === "string" && e.root)
      .map((e) => ({
        root: e.root,
        filter: typeof e.filter === "string" ? e.filter : "",
        expanded: Array.isArray(e.expanded) ? e.expanded.slice(0, STATE_MAX_EXPANDED) : [],
      }));
  }
  // v1：`{root, filter, expanded}` ⇒ 一条
  if (typeof st.root === "string" && st.root) {
    return [
      {
        root: st.root,
        filter: typeof st.filter === "string" ? st.filter : "",
        expanded: Array.isArray(st.expanded) ? st.expanded.slice(0, STATE_MAX_EXPANDED) : [],
      },
    ];
  }
  return [];
}

function panel_state_entry(panel) {
  return {
    root: panel.root,
    filter: panel.filter || "",
    expanded: collect_expanded(panel.tree, []),
  };
}

// 写：把**当前所有实例**写成 v2（关掉最后一个面板时它还在注册表里，所以"上次的位置"仍会被记住）
function save_state() {
  if (!CFG.persist) return;
  if (!state_path()) return;
  const entries = [];
  for (const panel of G.panels.values()) {
    if (panel.root === null) continue;
    entries.push(panel_state_entry(panel));
  }
  if (!entries.length) return; // 没面板就不覆盖旧文件（关到空不该抹掉记忆）
  try {
    helix.write_plugin_file(STATE_FILE, state_encode(entries, MAX_PANELS));
  } catch (e) {
    // 只读安装 / 没批 fs_write：持久化失败不该影响用面板
  }
}

function load_state() {
  if (!CFG.persist) return [];
  const p = state_path();
  if (!p) return [];
  try {
    return state_decode(helix.read_file(p));
  } catch (e) {
    return []; // 首次运行 / 文件坏了 / 没权限 ⇒ 当作"没有状态"
  }
}

// 按 root 取那一条（v2 的匹配口径：同一个根才继承折叠与过滤表达式）
function state_for_root(root) {
  for (const e of load_state()) {
    if (e.root === root) return e;
  }
  return null;
}

// 逐个展开（路径来自 state；找不到就跳过 —— 目录可能已经不在）
function restore_expanded(panel, paths) {
  for (const p of paths || []) {
    const n = find_node(panel.tree, rel_parts(panel.root, p) || []);
    if (n && n.is_dir) {
      ensure_children(n);
      n.expanded = true;
    }
  }
}

// 重建以 dir 为根的树
function set_root(panel, dir) {
  panel.root = dir;
  panel.tree = make_tree(dir, sort_entries(read_dir_sync(dir)));
  panel.cursor = 0;
  // 换根 ⇒ 过滤状态、补加载预算与全库索引全部作废（搜索词/旧索引都别跨根沿用）
  panel.mode = "tree";
  panel.scope = "tree";
  panel.query = "";
  panel.pre_query = null;
  panel.eager_done = false;
  panel.grep = null;
  panel.git = null;
  if (CFG.git_status) refresh_git(panel); // 换根 = 换仓库
  // 过滤表达式是"每个根一份"的：换根回到配置默认值，别把上一个项目的表达式带过来
  panel.filter = CFG.filter ? String(CFG.filter) : "";
  drop_flat(panel);
}

// 进 find 时**一次性**把懒加载目录补上：不补的话用户按 `/` 只能搜到已展开目录里的东西
// ——"搜不到"比"慢几十毫秒"更糟。预算上限保证大仓库也不过是一次有界的遍历。
const EAGER_DIRS = 400;
const EAGER_ENTRIES = 20000;

function eager_load_once(panel) {
  if (panel.eager_done || !panel.tree) return;
  panel.eager_done = true;
  const budget = { dirs: EAGER_DIRS, entries: EAGER_ENTRIES };
  eager_load(panel.tree, budget, new Set([panel.root]));
}

// seen 防符号链接成环（指向祖先的链接）；预算防超大树 —— 两者都不依赖 canonicalize
function eager_load(node, budget, seen) {
  for (const c of node.children || []) {
    if (!c.is_dir || seen.has(c.path)) continue;
    if (!G.show_hidden && c.name.startsWith(".")) continue;
    if (budget.dirs <= 0 || budget.entries <= 0) return;
    seen.add(c.path);
    if (c.children === null) {
      budget.dirs--;
      ensure_children(c, true);
      budget.entries -= c.children.length;
    }
    eager_load(c, budget, seen);
  }
}

// 当前选中节点的"目录上下文"：目录 → 自身；文件 → 父目录
function context_dir(panel) {
  const hit = node_at(panel, panel.cursor);
  if (!hit) return panel.root;
  if (hit.node.is_dir) return hit.node.path;
  const p = find_parent(panel.tree, hit.node.path);
  return p ? p.path : panel.root;
}

// 刷新：重读所有已展开目录，尽量恢复光标位置
function reload_tree(panel) {
  if (panel.root === null) return; // 还没开过 / cwd 变了但面板关着 ⇒ 没有树可刷（下次 open_panel 重建）
  const expanded = collect_expanded(panel.tree, []);
  const prev = (node_at(panel, panel.cursor) || {}).node && node_at(panel, panel.cursor).node.path;
  panel.tree = make_tree(panel.root, sort_entries(read_dir_sync(panel.root)));
  for (const p of expanded) {
    const n = find_node(panel.tree, rel_parts(panel.root, p) || []);
    if (n && n.is_dir) {
      ensure_children(n);
      n.expanded = true;
    }
  }
  if (panel.mode === "find") {
    // 过滤视图下重算：树是刚重建的，补加载与光标都要重新走一遍（全库索引也失效了）
    panel.eager_done = false;
    eager_load_once(panel);
    drop_flat(panel);
    if (panel.scope === "all") load_flat(panel);
    refresh_find(panel);
    return;
  }
  if (prev) {
    const rows = visible_rows(panel.tree, G.show_hidden, 0, []);
    const idx = rows.findIndex((r) => r.node.path === prev);
    panel.cursor = idx >= 0 ? idx : 0;
  } else {
    panel.cursor = 0;
  }
}

// 定位当前文件：展开路径链并选中它；文件不在 root 下时重设 root
function reveal(panel, file_path) {
  if (!file_path) return;
  if (panel.root === null) {
    set_root(panel, helix.path_parent(file_path) || panel.root);
    return;
  }
  const parts = rel_parts(panel.root, file_path);
  if (parts === null) {
    set_root(panel, helix.path_parent(file_path) || file_path);
  }
  const chain = rel_parts(panel.root, file_path);
  if (chain === null) return;
  const parent_parts = chain.slice(0, -1);
  if (parent_parts.length) {
    const parent = find_node(panel.tree, parent_parts);
    if (parent && parent.is_dir) {
      ensure_children(parent);
      parent.expanded = true;
      // 逐级展开中间目录
      for (let i = 1; i < parent_parts.length; i++) {
        const n = find_node(panel.tree, parent_parts.slice(0, i + 1));
        if (n && n.is_dir && n.children) {
          n.expanded = true;
          ensure_children(n);
        }
      }
    }
  }
  const rows = visible_rows(panel.tree, G.show_hidden, 0, []);
  const idx = rows.findIndex((r) => r.node.path === file_path);
  if (idx >= 0) panel.cursor = idx;
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

const HEADER_H = 2; // 头 + 分隔线（find 模式多一行查询行）

// 头部高度随模式变：查询行插在"头"与"分隔线"之间
function header_h(panel) {
  return HEADER_H + (panel.mode === "tree" ? 0 : 1);
}

function render_with(panel, ctx) {
  const w = Math.max(1, (ctx && ctx.width) || 32);
  const h = Math.max(1, (ctx && ctx.height) || 20);
  try {
    const rows = rows_for_view(panel);
    const list_top = header_h(panel);
    const list_h = Math.max(1, h - list_top);
    const top = scroll_top(panel.cursor, rows.length, list_h);

    // ── 头:根目录名 + 开关状态(H 隐藏 / F 跟随)──
    helix.draw.text(1, 0, [
      { text: basename(panel.root || "/"), style: "ui.text" },
      { text: (G.show_hidden ? "  H" : "") + (G.follow ? "  F" : ""), style: "ui.virtual" },
      // 已应用的过滤表达式：一直显示（不然"为什么少了文件"会变成一个谜）
      { text: panel.filter ? "  [" + panel.filter + "]" : "", style: "ui.text.focus" },
      // 头从 x=1 起 ⇒ 可用宽度是 w-1(用 w-2 会把尾巴上的状态标记截掉,实测踩过)
    ], { maxWidth: Math.max(1, w - 1) });

    // ── 查询行(find 模式):命中数右靠,空结果/载入中/失败都直说(不留给用户猜)──
    if (panel.mode !== "tree") {
      const f = panel.flat;
      const g = panel.grep;
      let mark;
      if (panel.mode === "filter") {
        mark = "  过滤 · " + rows.length + " 行 · Enter 应用 Esc 取消";
      } else if (panel.mode === "grep") {
        if (!g || g.loading) mark = "  搜索中…";
        else if (g.error) mark = "  失败: " + g.error;
        else {
          const total = g.groups.reduce((n, x) => n + x.matches.length, 0);
          mark = "  内容 · " + total + " 处 / " + g.groups.length + " 文件";
        }
      } else if (panel.scope === "all") {
        if (!f || f.loading) mark = "  全库载入中…";
        else if (f.error) mark = "  全库失败: " + f.error;
        else mark = "  全库 · " + rows.length + (rows.length ? " 项" : " · 无匹配");
      } else {
        mark = rows.length === 0 ? "  无匹配" : "  " + rows.length + " 行";
      }
      helix.draw.text(1, 1, [
        { text: panel.mode === "filter" ? "⊃ " : "⌕ ", style: "ui.text.focus" },
        { text: panel.query, style: "ui.text" },
        { text: "_", style: "ui.text.focus" },
        { text: mark, style: panel.scope === "all" ? "ui.text.focus" : "ui.virtual" },
      ], { maxWidth: Math.max(1, w - 1) });
    }
    helix.draw.hline(0, list_top - 1, w, { style: "ui.virtual" });

    // ── 树:内容坐标 + 滚动偏移 ──
    helix.draw.push_scroll(0, list_top, w, list_h, { dy: top });
      for (const { row, idx } of window_rows(rows, top, list_h)) {
        const y = list_top + idx;             // 内容坐标(与屏幕无关)
        const { node, depth } = row;
        const is_sel = idx === panel.cursor;
        const is_cur = node.path === G.current;
        if (is_sel) helix.draw.fill(0, y, w, 1, { style: "ui.selection" });
        // 图标:直接用核心 API(单一来源);关图标时目录回退 ▸/▾、文件留空格(保持两格对齐)
        const is_line = node.line !== undefined && node.line !== null;
        // 内容命中行没有图标：图标位换成**右对齐行号**（一眼看出 Enter 会跳到哪）
        const glyph = is_line
          ? String(node.line + 1).padStart(4) + " "
          : node.is_dir
            ? helix.icons.dir(node.expanded) + " "
            : (helix.icons.file(node.name) || " ") + " ";
        const style = is_sel
          ? "ui.selection"
          : is_cur
            ? "ui.popup"
            : node.is_dir || is_line
              ? "ui.virtual"
              : "ui.text";
        // 命中字符单独着色(只对**可见窗口**算:命中下标已在 filter_rows 里算好,这里只分段)
        const segs = highlight_segments(node.name, row.hit).map((seg) => ({
          text: seg.text,
          style: seg.hit ? "ui.text.focus" : style,
        }));
        helix.draw.text(1, y, [{ text: "  ".repeat(depth) + glyph, style }].concat(segs), {
          maxWidth: Math.max(1, w - 2),
        });
        // 诊断列（w-6）：E/W/i/h（只在文件行画；内容命中行共用一个 path，不重复标）
        if (!is_line && w > 8) {
          const dm = diag_mark(node.path);
          if (dm) helix.draw.text(w - 6, y, dm.ch, { style: is_sel ? "ui.selection" : dm.style });
        }
        // git 状态列:右缘倒数第二格(只在文件行画;内容命中行共用一个 path,不重复标)
        if (!is_line && w > 6) {
          const gm = git_mark(panel, node.path);
          if (gm) helix.draw.text(w - 4, y, gm.text, { style: is_sel ? "ui.selection" : gm.style });
        }
        // 右缘标记:● 当前文件 / · 已打开但非当前（内容命中行不画:同一个 path 会每行都冒出来）
        if (!is_line && w > 3) {
          const om = opened_marker(node.path, G.current, G.opened);
          if (om) {
            // 选中底色之上仍要看得见 ⇒ 当前文件与选中行用 focus 色
            const st = is_sel || om.ch === "●" ? "ui.text.focus" : om.style;
            helix.draw.text(w - 2, y, om.ch, { style: st });
          }
        }
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

// 面板入口：`root` 省略时用 cwd / 持久状态里那条。超上限直接拒绝（echo 说明原因）。
function open_panel(root) {
  if (G.panels.size >= MAX_PANELS) {
    helix.echo("filetree: 最多 " + MAX_PANELS + " 个面板（[plugins.filetree] max_panels）");
    return null;
  }
  const panel = new_panel(root);
  if (panel.root === null) {
    // 没指定 root：取**当前**工作目录（现取；`:cd` 后由 cwd-change 处理器重设）
    try {
      panel.root = helix.cwd() || "/";
    } catch (e) {
      panel.root = "/";
    }
  }
  set_root(panel, panel.root); // 建树（显式给了 root 时也必须建 —— 否则面板是空的）
  // 打开时按"配置 → 持久状态"的顺序定初值：配置 filter 是**显式**默认，持久状态是**上次**
  // （两者都在时以持久状态为准 —— 用户最后一次动作更能代表意图）
  if (!panel.filter && CFG.filter) panel.filter = String(CFG.filter);
  const st = state_for_root(panel.root);
  if (st) {
    if (st.filter) panel.filter = st.filter;
    restore_expanded(panel, st.expanded);
  }
  if (panel.filter) {
    eager_load_once(panel); // 过滤要看得到未展开目录里的东西
    if (panel.filter.indexOf("#") >= 0) refresh_git(panel);
  } else if (CFG.git_status && panel.git === null) {
    refresh_git(panel);
  }
  seed_opened();
  poll_diagnostics();
  // 已经有面板时必须**先把焦点还给编辑器**：`open_panel` 是切分"当前叶子"，
  // 否则第二个面板会把第一个挤成窄条（实测：8 列 vs 24 列）
  if (G.panels.size > 0) helix.pane.focus(0);
  panel.id = helix.open_panel({
    side: "left",
    size: 32,
    // 绑键位作用域（B 阶段）：状态栏 `layers` 段显示 `FILETREE`，
    // 且面板没消费 Esc 时核心按 `esc: "blur"` 把焦点还回编辑器（面板保留）。
    scope: SCOPE,
    // 在**根节点**上切：filetree 拿左边缘的整列（全高），之后的切分（编辑器 / 终端）
    // 都发生在右边的区域里 ⇒ 想看到的布局就是 `filetree | editor / term`。
    // 不加这条时切的是"当前叶子"：编辑器已经分过屏，树只会挂在其中一半上。
    at: "root",
    render: make_render(panel),
    onKey: make_key(panel),
    // 布局恢复 / `:plugin-reload` 关掉面板时也要摘注册表（以前没有 onClose ⇒ 状态留尸）
    onClose: () => {
      G.panels.delete(panel.id);
      if (G.active === panel.id) G.active = null;
    },
  });
  G.panels.set(panel.id, panel);
  G.active = panel.id;
  return panel;
}

// 每个实例一份渲染/按键闭包 —— 支持双实例的**支点**：状态在闭包里，不在模块级
function make_render(panel) {
  return (_focus, ctx) => render_with(panel, ctx);
}

function make_key(panel) {
  return (key) => {
    G.active = panel.id;
    // 这两件是**全局事实**（已打开集合 / 当前文档诊断）：任一实例的按键触发一次即可
    seed_opened();
    poll_diagnostics();
    return handle_key_with(panel, key);
  };
}

function close_panel(target) {
  const panel = target || target_panel() || first_panel();
  if (!panel) return;
  save_state(); // 必须在清状态之前：展开集合来自当前的树（此刻它还在注册表里）
  // ⚠️ id 必须**先取下来**：下面会把 `panel.id` 置空，若之后再 `G.panels.delete(panel.id)`
  // 删的是 `null` 键 ⇒ 真条目泄漏 ⇒ `toggle_panel` 永远走"关"分支，面板再也打不开
  // （2026-11-03 实测：Esc 关闭后 `:filetree` 无效且状态栏无任何提示）。
  const id = panel.id;
  panel.id = null;
  if (id !== null) helix.close_panel(id);
  // 关面板丢掉搜索态与全库索引：下次打开不应该还停在半截的过滤里，
  // 也不应该让一个 nucleo 会话（可能几万条）悬着
  panel.mode = "tree";
  panel.scope = "tree";
  panel.query = "";
  panel.pre_query = null;
  panel.grep = null;
  drop_flat(panel);
  if (id !== null) {
    G.panels.delete(id);
    if (G.active === id) G.active = null;
  }
}

// `:filetree` 语义（§4.3）：无 → 开；有 → 关**目标**实例（active；只剩一个时就是它）
function toggle_panel() {
  if (G.panels.size === 0) open_panel();
  else close_panel();
}

// ============================ 输入/确认弹窗 ============================

// 文本输入弹窗：字符键直接拼输入（无焦点依赖——input 组件聚焦时 Enter 会被吞）
// 按键：普通字符/Backspace 编辑 → Enter 提交 → Esc 取消
// ── 输入 / 确认弹窗 ────────────────────────────────────────────────
// 交给**核心**（`helix.dialog.*`）：渲染（圆角框 + 标题）、输入（值/光标/方向键/粘贴/IME）、
// Esc/Enter 全在核心里，插件只给标题与回调。早先这里自己拼字符 —— 天花板写在文件头
// 注释里："不支持粘贴/IME"。dapui 也用同一套 ⇒ 两个插件的外观与行为不会再漂。
function prompt(title, initial, on_submit) {
  if (!helix.dialog || !helix.dialog.prompt) {
    helix.echo("filetree: 这个版本没有 helix.dialog.prompt（输入弹窗不可用）");
    return;
  }
  helix.dialog.prompt({
    title,
    value: initial === null || initial === undefined ? "" : String(initial),
    on_submit,
  });
}

function confirm(title, on_ok) {
  if (!helix.dialog || !helix.dialog.confirm) {
    helix.echo("filetree: 这个版本没有 helix.dialog.confirm（确认弹窗不可用）");
    return;
  }
  helix.dialog.confirm({ title, on_ok });
}

// ============================ 文件操作 ============================

async function run_fs(cmd, ok_msg) {
  try {
    await helix.run_async(cmd);
    if (ok_msg) helix.echo(ok_msg);
    reload_tree(panel);
  } catch (err) {
    helix.echo("filetree: " + err.message);
  }
}

// 建文件/建目录**不走 shell**：`touch` / `mkdir -p` 是 POSIX 写法，Windows 上没有。
// 核心已有 `write_file`（同步，父目录按需创建）与 `mkdir_async`（递归）—— 用它们。
function new_file(panel) {
  const dir = context_dir(panel);
  prompt("新建文件（" + dir + "）", "", (name) => {
    const n = name.trim();
    if (!n) return;
    const path = helix.path_join(dir, n);
    try {
      // 已存在就不覆盖（`touch` 的语义；`write_file` 会截断，必须先挡一下）
      if (helix.read_dir(dir).some((e) => e.name === n)) {
        helix.echo(path + " 已存在");
        return;
      }
      helix.write_file(path, "");
      helix.echo("已创建 " + n);
      reload_tree(panel);
    } catch (err) {
      helix.echo("filetree: " + err.message);
    }
  });
}

function new_dir(panel) {
  const dir = context_dir(panel);
  prompt("新建目录（" + dir + "）", "", (name) => {
    const n = name.trim();
    if (!n) return;
    helix.mkdir_async(helix.path_join(dir, n)).then(
      () => {
        helix.echo("已创建目录 " + n);
        reload_tree(panel);
      },
      (err) => helix.echo("filetree: " + err.message),
    );
  });
}

function rename(panel) {
  const hit = node_at(panel, panel.cursor);
  if (!hit) return;
  prompt("重命名 " + hit.node.name, hit.node.name, (name) => {
    const n = name.trim();
    if (!n || n === hit.node.name) return;
    const dst = helix.path_join(helix.path_parent(hit.node.path), n);
    if (IS_WINDOWS) {
      run_fs("move " + shq(hit.node.path) + " " + shq(dst), "已重命名");
    } else {
      run_fs("mv " + shq(hit.node.path) + " " + shq(dst), "已重命名");
    }
  });
}

function remove(panel) {
  const hit = node_at(panel, panel.cursor);
  if (!hit) return;
  let cmd;
  if (IS_WINDOWS) {
    cmd = hit.node.is_dir ? "rmdir /s /q " : "del /f /q ";
  } else {
    cmd = (hit.node.is_dir ? "rm -rf " : "rm -f ") + shq(hit.node.path);
  }
  const full = IS_WINDOWS ? cmd + shq(hit.node.path) : cmd;
  confirm("删除 " + (hit.node.is_dir ? "目录 " : "文件 ") + hit.node.path + " ?", () => {
    run_fs(full, "已删除 " + hit.node.name);
  });
}

// ============================ 搜索（P0：按名字/路径过滤树）============================

function enter_find(panel) {
  if (!panel.tree) return;
  const hit = node_at(panel, panel.cursor);
  panel.pre_query = hit ? hit.node.path : null; // Esc 退出时回到这里，别让用户丢位置
  panel.mode = "find";
  panel.scope = "tree";
  panel.query = "";
  panel.cursor = 0;
  eager_load_once(panel);
}

// restore = true：光标回到进搜索前的节点（reveal 会重新展开路径链）
// find / grep 两种搜索都走这里退出（搜索态是两把椅子一个位置）
function exit_search(panel, restore) {
  const back = panel.pre_query;
  panel.mode = "tree";
  panel.scope = "tree";
  panel.query = "";
  panel.pre_query = null;
  panel.grep = null;
  if (restore && back) {
    reveal(panel, back);
  } else {
    panel.cursor = Math.max(0, Math.min(panel.cursor, count_visible(panel) - 1));
  }
}

// query 变了 ⇒ 光标回到**首个真命中**（不是被保留的祖先行）：输完字直接 Enter 就是它，
// 而不是"Enter 一下先钻进 src/"。无命中回顶部（Enter 自然什么都不做）。
function refresh_find(panel) {
  const rows = rows_for_view(panel);
  let idx = rows.findIndex((r) => r.hit);
  if (idx < 0) idx = 0;
  panel.cursor = rows.length ? Math.min(idx, rows.length - 1) : 0;
}

// find 模式的按键。字符键全部当输入（含空格）；未识别的键穿透给编辑器（不减非模态）。
// 搜索态的按键：字符键全部当输入（含 j/k/? —— 为导航而吞字会让"搜 j.js"变成导航）。
// 未识别的键（方向键以外）穿透给编辑器。
function find_key(panel, key) {
  switch (key.name) {
    case "Esc":
      exit_search(panel, true);
      return "handled";
    case "Enter": {
      const hit = node_at(panel, panel.cursor);
      if (!hit) return "handled";
      if (hit.node.is_dir) {
        // 目录 → 钻进这个目录（把 query 设成它的相对路径）：比"切换展开"更合搜索时的意图
        const parts = rel_parts(panel.root, hit.node.path) || [];
        panel.query = parts.length ? parts.join("/") + "/" : "";
        refresh_find(panel);
        return "handled";
      }
      const path = hit.node.path;
      // 先把状态回 tree：否则 `buffer-open` 的跟随逻辑在搜索态会被跳过
      panel.mode = "tree";
      panel.query = "";
      panel.pre_query = null;
      helix.open_file(path);
      helix.pane.focus(0);
      reveal(panel, path);
      return "handled";
    }
    case "Backspace":
      if (!panel.query) {
        exit_search(panel, true);
        return "handled";
      }
      panel.query = panel.query.slice(0, -1);
      refresh_find(panel);
      return "handled";
    case "Tab":
      // 在"当前树"与"全库"之间切（全库 = rg --files 列表 + nucleo 打分）
      panel.scope = panel.scope === "all" ? "tree" : "all";
      if (panel.scope === "all") load_flat(panel);
      refresh_find(panel);
      return "handled";
    case "Up": move(panel, -1); return "handled";
    case "Down": move(panel, 1); return "handled";
    case "PageUp": move(panel, -10); return "handled";
    case "PageDown": move(panel, 10); return "handled";
    case "Home": panel.cursor = 0; return "handled";
    case "End": panel.cursor = Math.max(0, count_visible(panel) - 1); return "handled";
    default: break;
  }
  // 单字符（含空格、j/k/?）一律当输入；其它键穿透
  if (key.name.length === 1) {
    panel.query += key.name;
    refresh_find(panel);
    return "handled";
  }
  return "ignore";
}

// ============================ 过滤表达式（`>` 输入，P3）============================
//
// 与 find 的区别：find 是**临时**的（Esc 就没），filter 是**应用**的（Enter 后留在树模式，
// 关面板/重开仍需显式清）。表达式由 compile_filter 解析（glob / `#` 谓词 / 子串 / `!` 排除）。

function enter_filter(panel) {
  if (!panel.tree) return;
  const hit = node_at(panel, panel.cursor);
  panel.pre_query = hit ? hit.node.path : null;
  panel.mode = "filter";
  panel.query = panel.filter; // 从已应用的表达式接着改
  panel.cursor = 0;
  eager_load_once(panel);
  // 表达式里有 `#` 谓词时才需要 git 状态表（没必要为纯 glob 起一个进程）
  if (panel.query.indexOf("#") >= 0 && panel.git === null) refresh_git(panel);
}

// Enter：把草稿变成"已应用"（写盘；`#` 谓词缺表时补读一次 git）
function apply_filter(panel) {
  panel.filter = panel.query.trim();
  panel.mode = "tree";
  panel.query = "";
  panel.pre_query = null;
  if (panel.filter.indexOf("#") >= 0) refresh_git(panel);
  refresh_after_filter(panel);
  save_state();
}

// Esc：放弃草稿（已应用的表达式不受影响）
function cancel_filter(panel) {
  const back = panel.pre_query;
  panel.mode = "tree";
  panel.query = "";
  panel.pre_query = null;
  if (back) reveal(panel, back);
  else refresh_after_filter(panel);
}

// 收起过滤后用树视图重算光标：优先停在原节点上，否则回顶部
function refresh_after_filter(panel) {
  const rows = rows_for_view(panel);
  panel.cursor = Math.max(0, Math.min(panel.cursor, rows.length - 1));
}

function filter_key(panel, key) {
  switch (key.name) {
    case "Esc":
      cancel_filter(panel);
      return "handled";
    case "Enter":
      apply_filter(panel);
      return "handled";
    case "Backspace":
      if (!panel.query) {
        panel.query = "";
        apply_filter(panel); // 删空 = 清除过滤
        return "handled";
      }
      panel.query = panel.query.slice(0, -1);
      refresh_after_filter(panel);
      return "handled";
    case "Up": move(panel, -1); return "handled";
    case "Down": move(panel, 1); return "handled";
    case "PageUp": move(panel, -10); return "handled";
    case "PageDown": move(panel, 10); return "handled";
    case "Home": panel.cursor = 0; return "handled";
    case "End": panel.cursor = Math.max(0, count_visible(panel) - 1); return "handled";
    default: break;
  }
  if (key.name.length === 1) {
    panel.query += key.name;
    refresh_after_filter(panel);
    return "handled";
  }
  return "ignore";
}

// ============================ 内容搜索（grep 结果树，P2）============================
//
// 行形状与树/过滤视图完全一致 ⇒ `node_at` / 打开 / 渲染自动成立：
//   文件行 depth 0（node.name = 相对路径，没有 `line`）
//   命中行 depth 1（node.name = 该行文本，`node.line` = **0 起始**行号）
// 不用 `rg -n --no-heading` + `split(":")`：Windows 盘符带冒号，解析必错（见 grep_groups）。

const GREP_PER_FILE = 20; // 单文件最多取多少条（传给 `rg -m`，同时是 JS 侧的第二道限制）
const GREP_MAX_FILES = 500;
const GREP_MAX_MATCHES = 2000;

function enter_grep(panel, seed) {
  if (!panel.tree) return;
  const hit = node_at(panel, panel.cursor);
  panel.pre_query = hit ? hit.node.path : null;
  panel.mode = "grep";
  panel.scope = "tree";
  panel.query = seed || "";
  panel.cursor = 0;
  panel.grep = { groups: [], loading: false, error: null, seq: 0 };
  if (panel.query) start_grep(panel);
}

// 发一次 rg。插件侧没有定时器 ⇒ 不做防抖，改成**序号门**：
// 只有最新一次请求的结果被接受，旧结果到达就丢（连续敲字时不会回跳）。
function start_grep(panel) {
  const g = panel.grep;
  if (!g) return;
  const q = panel.query;
  if (!q) {
    g.groups = [];
    g.loading = false;
    g.error = null;
    refresh_find(panel);
    return;
  }
  const root = base_dir(panel);
  const seq = ++g.seq;
  g.loading = true;
  // `-F` 字面量（写 `(` 不会报正则错）·`-m` 单文件封顶·`--json` 可解析且带行号
  helix
    .run_async(["rg", "--json", "-n", "-F", "-m", String(GREP_PER_FILE), "-e", q, "--", root])
    .then(
      (out) => {
        if (panel.grep !== g || g.seq !== seq) return; // 过期结果
        g.loading = false;
        g.groups = grep_groups(out, root, {
          files: GREP_MAX_FILES,
          matches: GREP_MAX_MATCHES,
          per_file: GREP_PER_FILE,
        });
        refresh_find(panel);
      },
      (err) => {
        if (panel.grep !== g || g.seq !== seq) return;
        g.loading = false;
        g.groups = [];
        // rg 无匹配 = exit 1（常规退出）；其它（无 rg / 参数错）才是真的错误
        const msg = String((err && err.message) || err);
        g.error = is_no_match(err) ? null : msg;
        refresh_find(panel);
      },
    );
}

// 全部分组 → 行（文件行为父、命中行为子）。折叠状态挂在 group 上（不改树、不改 S）
function grep_rows(panel) {
  const g = panel.grep;
  if (!g) return [];
  const out = [];
  for (const grp of g.groups) {
    const expanded = grp.expanded !== false;
    out.push({
      node: { name: grp.rel, path: grp.path, is_dir: false, expanded, children: null },
      depth: 0,
      hit: null,
    });
    if (!expanded) continue;
    for (const m of grp.matches) {
      out.push({
        node: {
          name: m.text === "" ? "(空行)" : m.text,
          path: grp.path,
          is_dir: false,
          line: m.line,
          children: null,
        },
        depth: 1,
        hit: literal_hits(m.text, panel.query),
      });
    }
  }
  return out;
}

function grep_key(panel, key) {
  switch (key.name) {
    case "Esc":
      exit_search(panel, true);
      return "handled";
    case "Enter": {
      const hit = node_at(panel, panel.cursor);
      if (!hit) return "handled";
      const path = hit.node.path;
      const line = hit.node.line === undefined ? 0 : hit.node.line;
      panel.mode = "tree";
      panel.query = "";
      panel.pre_query = null;
      panel.grep = null;
      helix.open_file(path, { row: line, col: 0 });
      helix.pane.focus(0);
      reveal(panel, path);
      return "handled";
    }
    case "Left":
    case "Right": {
      // 折叠/展开当前文件组（方向键是自由键：字符键全归输入，抢不了）
      const hit = node_at(panel, panel.cursor);
      if (!hit || !panel.grep) return "handled";
      const grp = panel.grep.groups.find((x) => x.path === hit.node.path);
      if (!grp) return "handled";
      grp.expanded = key.name === "Right";
      const rows = rows_for_view(panel);
      const idx = rows.findIndex((r) => r.depth === 0 && r.node.path === hit.node.path);
      panel.cursor = idx >= 0 ? idx : panel.cursor;
      return "handled";
    }
    case "Backspace":
      if (!panel.query) {
        exit_search(panel, true);
        return "handled";
      }
      panel.query = panel.query.slice(0, -1);
      start_grep(panel);
      return "handled";
    case "Up": move(panel, -1); return "handled";
    case "Down": move(panel, 1); return "handled";
    case "PageUp": move(panel, -10); return "handled";
    case "PageDown": move(panel, 10); return "handled";
    case "Home": panel.cursor = 0; return "handled";
    case "End": panel.cursor = Math.max(0, count_visible(panel) - 1); return "handled";
    default: break;
  }
  if (key.name.length === 1) {
    panel.query += key.name;
    start_grep(panel);
    return "handled";
  }
  return "ignore";
}

// ============================ 按键 ============================

function handle_key_with(panel, key) {
  // 面板每次按键重新播种"已打开"集合：核心不发 `:buffer-close` 事件，不这样 `·` 会留尸
  seed_opened();
  poll_diagnostics(); // 诊断同理：没有逐文档事件，只能按人-rate 拉"当前文档"

  // ① 先查**声明表**（可被 `[plugins.filetree.keys]` 覆盖）
  const action = tree_action(key);
  if (action === "done") return "handled"; // 用户把键覆盖成了命令，已执行
  // C-\ 回编辑器（保留面板；与终端 C-\ 语义统一）—— 必须在"修饰键放行"之前判
  if (action === "leave") {
    helix.pane.focus(0);
    return "handled";
  }
  // ② 修饰键组合（C-x / M-x）**穿透给编辑器** —— 面板是非模态的（`open_panel`，不是 popup），
  // 用户在 `[keys]` / `helix.map` 里绑的 C-* 必须照常生效；被吞掉会变成"静默失效"。
  // 对照：`arsenal` / `dashboard` 是 popup（模态），它们在 onKey 里 `return "handled"` 是对的。
  if (key.ctrl || key.alt) return "ignore";
  // ③ 搜索/过滤态优先（它们是"文本输入态"，按键归输入而不是绑定表）
  if (panel.mode === "find") return find_key(panel, key);
  if (panel.mode === "grep") return grep_key(panel, key);
  if (panel.mode === "filter") return filter_key(panel, key);

  // ④ 树模式：只按**动作名**分派（`[plugins.filetree.keys]` 把某个键换掉后，这里一行都不用改）
  switch (action) {
    case "esc":
      // `close_on_esc = false`：Esc 只回焦点（面板保留），关面板改用 `q` —— 这是"树常驻"的用法。
      // 搜索/过滤态里的 Esc 走各自的分支（先退出那种态），语义不受此配置影响。
      if (CFG.close_on_esc === false) {
        helix.pane.focus(0);
        return "handled";
      }
      close_panel();
      return "handled";
    case "close": close_panel(); return "handled";
    case "up": move(panel, -1); return "handled";
    case "down": move(panel, 1); return "handled";
    case "page-up": move(panel, -10); return "handled";
    case "page-down": move(panel, 10); return "handled";
    case "top": panel.cursor = 0; return "handled";
    case "bottom": panel.cursor = Math.max(0, count_visible(panel) - 1); return "handled";
    case "open": open_selected(panel); return "handled";
    case "enter-dir": enter_dir(panel); return "handled";
    case "leave-dir": leave_dir(panel); return "handled";
    case "up-root": {
      const hit = node_at(panel, panel.cursor);
      const dir = hit ? (hit.node.is_dir ? hit.node.path : (find_parent(panel.tree, hit.node.path) || {}).path) : null;
      if (dir && dir !== panel.root) set_root(panel, dir);
      return "handled";
    }
    case "toggle-hidden": G.show_hidden = !G.show_hidden; return "handled";
    case "reload": reload_tree(panel); refresh_git(panel); poll_diagnostics(); return "handled";
    case "toggle-follow": G.follow = !G.follow; helix.echo(G.follow ? "跟随当前文件: 开" : "跟随当前文件: 关"); return "handled";
    case "new-file": new_file(panel); return "handled";
    case "new-dir": new_dir(panel); return "handled";
    case "rename": rename(panel); return "handled";
    case "remove": remove(panel); return "handled";
    case "help": show_help(); return "handled";
    case "find": enter_find(panel); return "handled";
    case "grep": enter_grep(panel, ""); return "handled";
    case "filter": enter_filter(panel); return "handled";
    // 未映射键穿透（面板非模态）：: 命令、i 等仍可用
    default: return "ignore";
  }
}

// 从 picker(或任何外部入口)跳到一个文件：打开它；面板开着就同步定位
function pick_open(path) {
  if (!path) return;
  const panel = target_panel();
  if (panel.mode === "find") exit_search(panel, false);
  helix.open_file(path);
  if (panel.id !== null) reveal(panel, path);
}

/// 帮助弹窗的**尺寸决策**（纯函数，便于 node 测）。
///
/// 输入：内容行数、最长行宽（cell）、视口宽高；输出：弹窗宽高 + 内容区高度 + 要不要滚动。
/// 规则：宽度/高度都**不超过视口**（边框各留 1 格 + 上下留 2 行余量），
/// 内容放不下 ⇒ `need_scroll`，由调用方套 `helix.el("scroll", …)`。
function help_geometry(line_count, widest_cell, vw, vh, title_cell) {
  const CHROME = 3; // 标题 + footer + 上下边框
  const width = Math.min(
    Math.max(title_cell || 24, widest_cell + 2) + 4,
    Math.max(20, vw - 4),
  );
  const max_body = Math.max(3, vh - CHROME - 2);
  const need_scroll = line_count > max_body;
  const body_h = Math.min(Math.max(line_count, 1), max_body);
  return { width, height: body_h + CHROME, body_h, need_scroll };
}

function show_help() {
  // 内容来自作用域表（与 `:keys` 的"插件作用域"段、`[plugins.filetree.keys]` 覆盖**同源**）
  const table = helix.keymap && helix.keymap.hints ? helix.keymap.hints(SCOPE) : null;
  const lines = table ? table.split("\n") : [];
  const pick = lines.length ? lines : ["（这个版本没有 helix.keymap.hints）"];

  // ── 自适应终端 ──
  // 早先是固定 `width:72, height:min(38, 行数+4)`：小终端上**静默截断**（实测 20 行时 `q` 那行
  // 根本看不到）。现在按视口算：宽度跟最长一行走、高度跟内容走，两者都不超过视口，
  // 真放不下时内容进 `scroll` 容器（可 j/k 滚动），**不再无声丢行**。
  let vw = 80;
  let vh = 24;
  try {
    const vp = helix.viewport ? helix.viewport() : null;
    if (vp && vp[0] > 0 && vp[1] > 0) {
      vw = vp[0];
      vh = vp[1];
    }
  } catch (e) {
    // 没有视口信息（旧二进制 / 测试夹具）⇒ 用保守默认值
  }
  const text_width = (t) =>
    helix.text && helix.text.width ? helix.text.width(t) : String(t).length;
  const widest = pick.reduce((m, l) => Math.max(m, text_width(l)), 24);
  const title = "filetree 键位（[plugins.filetree.keys] 可改）";
  const footer = "搜索/过滤态：字符即输入 · Tab 切全库 · Enter 应用 · Esc 逐级退出";
  const geo = help_geometry(pick.length, widest, vw, vh, text_width(title));
  const { width, height, body_h } = geo;
  const need_scroll = geo.need_scroll;

  // 放不下才滚动（放得下时用普通 col，读起来更平）
  let offset = 0;
  const body = () =>
    need_scroll
      ? [helix.el("scroll", pick.map((l) => helix.el("text", l, { style: "ui.popup" })), {
          height: body_h,
          offset,
        })]
      : pick.map((l) => helix.el("text", l, { style: "ui.popup" }));

  helix.open_popup({
    width,
    height,
    position: "center",
    // 绑**弹层**作用域 ⇒ `:`/`C-w`/`C-p` 照常可用（Esc 仍是"关闭"，不是 blur）
    scope: HELP_SCOPE,
    render: () =>
      helix.el(
        "col",
        [
          helix.el("text", title, { style: "ui.virtual" }),
          ...body(),
          helix.el(
            "text",
            need_scroll ? footer + " · j/k 滚动" : footer,
            { style: "ui.help" },
          ),
        ],
        { gap: 0 },
      ),
    onKey: (key) => {
      const nm = key && key.name !== undefined ? String(key.name) : String(key);
      if (nm === "Esc" || nm === "q" || nm === "?") return "close";
      // 只在真滚动时接管 j/k（否则 j/k 该关弹窗？不 —— 关弹窗用 Esc/q，这里吞掉更省心）
      if (need_scroll && nm === "j") {
        offset = Math.min(Math.max(0, pick.length - body_h), offset + 1);
        return "handled";
      }
      if (need_scroll && nm === "k") {
        offset = Math.max(0, offset - 1);
        return "handled";
      }
      if (nm === "g") {
        offset = 0;
        return "handled";
      }
      if (nm === "G") {
        offset = Math.max(0, pick.length - body_h);
        return "handled";
      }
      return "handled";
    },
  });
}

// ============================ 事件 ============================

function seed_opened() {
  // 面板打开时播种一次（每帧调 buffer.list() 是浪费；之后由 open/close 事件增量维护）
  try {
    G.opened = new Set(helix.buffer.list().map((b) => b.path).filter(Boolean));
  } catch (e) {
    G.opened = new Set();
  }
}

// 注意：核心**只在 `:quit`/`:force-quit`** 发 buffer-close，`:buffer-close` 既不发票事件、
// 也不动布局（`buffer_close_by_ids_impl` 里没有任何 emit/sync）⇒ 纯粹靠事件维护会让 `·`
// 一直留着（实测）。所以权威来源是 `buffer.list()` 重新播种，触发点：
//   ① 面板打开；② **面板上任何一次按键**（人-rate，见 handle_key 顶部）；③ `R`。
// 代价：每次按键一次 `buffer.list()`（文档数级别，微秒量级）。
function on_buffer_close(doc) {
  const p = doc && doc.path;
  if (!p || !G.opened) return;
  G.opened.delete(p);
}

function on_buffer_open(doc) {
  if (!doc || !doc.path) return;
  G.current = doc.path;
  if (!G.opened) G.opened = new Set();
  G.opened.add(doc.path);
  poll_diagnostics();
  // find 模式下**不跟随**：过滤视图里"定位"会把光标从搜索结果上挪走（Esc 退出后会重新跟随）
  const panel = resolve_panel(doc.path);
  if (G.follow && panel && panel.id !== null && panel.mode === "tree") {
    reveal(panel, doc.path);
  }
}

// ============================ 注册（node 环境跳过） ============================

if (typeof helix !== "undefined") {
  // 目标实例解析：归属优先 → active/唯一 → 新建一个
  function panel_or_open(path) {
    return resolve_panel(path) || open_panel();
  }

  helix.register_command("filetree", toggle_panel, "切换文件树面板", '切换文件树面板');
  // 再开一个面板（`:filetree` 是开/关切换，不能用来加第二个）
  helix.register_command("filetree-open", (ctx) => {
    const arg = helix.command_args()[0];
    const path = arg || (ctx && (ctx.doc ? ctx.doc.path : ctx.path));
    // 根 = 显式目录 / 当前文件所在目录 / cwd
    let root = null;
    if (arg) root = arg;
    else if (path) root = helix.path_parent(path) || null;
    const panel = open_panel(root);
    if (panel && path) reveal(panel, path);
  }, "再开一个文件树面板（root = 参数或当前文件所在目录）", '再开一个文件树面板');
  helix.register_command("filetree-reveal", (ctx) => {
    // 命令 ctx 形状是 { doc: { path, ... }, cursor, selection }（兼容旧顶层 path）
    const path = ctx && (ctx.doc ? ctx.doc.path : ctx.path);
    if (!path) return;
    const panel = panel_or_open(path);
    if (panel) reveal(panel, path);
  }, "文件树定位当前文件", '在文件树里定位当前文件');

  // 全库找文件：交给**原生 picker**（nucleo 打分 + 预览 + 核心键位），插件只供数据源。
  // 定位上不重复核心 `:open <目录>`——区别是它在**面板根**上取列表，选中后事件把面板同步到该文件。
  helix.picker.define("filetree-find", {
    columns: ["name", "path"],
    items: () => {
      const root = base_dir(target_panel() || first_panel());
      return list_files(root).then((items) =>
        items.map((it) => ({
          cells: [helix.path_name(it.path) || it.rel, it.rel],
          payload: [it.path],
        })),
      );
    },
    preview: (row) => ({ path: row[0], line: 0 }),
    action: (row) => pick_open(row[0]),
  });
  helix.register_command("filetree-find", () => helix.picker.run("filetree-find"),
    "全库找文件（rg --files + 原生 picker）", '全库找文件');
  // 内容搜索的命令形态：`:filetree-search foo bar`（参数按空格拼接）——不给面板开着的用户
  // 留"必须先按 G 再打字"的弯路；面板没开就先开（要显示结果树）。
  helix.register_command("filetree-search", () => {
    const q = helix.command_args().join(" ").trim();
    const panel = panel_or_open();
    if (panel) enter_grep(panel, q);
  }, "内容搜索（rg）：:filetree-search <关键词>", '内容搜索');

  helix.on("buffer-open", on_buffer_open);
  helix.on("buffer-close", on_buffer_close);
  helix.on("lsp-diagnostics", on_lsp_diagnostics);
  // `:cd` / `:lcd` 之后跟着换根（事件发在真 chdir 之后 ⇒ `helix.cwd()` 已是新目录）。
  // 只有"root 就是旧 cwd"的实例才跟着走 —— 第 1–3 步最多一个实例，这条在双实例时才有分别。
  helix.on("cwd-change", (_doc, ev) => {
    const next = (ev && ev.cwd) || helix.cwd() || "/";
    const prev = G.last_cwd;
    G.last_cwd = next;
    // 只重设"root 就是旧 cwd"的实例；别的实例有自己的根，不该被 `:cd` 拽走（§4.3）
    for (const panel of G.panels.values()) {
      if (panel.root !== null && prev !== null && panel.root !== prev) continue;
      try {
        set_root(panel, next);
      } catch (e) {
        helix.echo("filetree: 切换目录失败 " + (e && e.message ? e.message : e));
      }
    }
  });
  helix.export({
    toggle: toggle_panel,
    open: open_panel,
    close: close_panel,
    target: target_panel,
    reveal: (path) => {
      const panel = panel_or_open();
      if (panel) reveal(panel, path);
    },
    set_root: (dir) => {
      const panel = panel_or_open();
      if (panel) set_root(panel, dir);
    },
    find: () => helix.picker.run("filetree-find"),
  });
}

// ============================ node 自检导出 ============================

if (typeof module !== "undefined" && module.exports) {
  module.exports = {
    sort_entries, visible_rows, make_tree, rel_parts, find_node, find_parent,
    collect_expanded, shq, G, new_panel, render_with, reveal, open_panel, window_rows, scroll_top,
    target_panel, panel_for_path, first_panel, close_panel,
    state_encode, state_decode, save_state, load_state, state_for_root,
    // 搜索（纯逻辑；refresh_find/enter_find 依赖 helix 状态，不导出）
    match_name, filter_rows, highlight_segments, rows_for_view, flat_items, drop_flat,
    grep_groups, literal_hits, grep_rows,
    compile_filter, dsl_hit, match_tree, glob_to_regex, parse_git_status, git_letter, is_no_match,
    list_files,
    opened_marker, worst_severity, diag_hit, diag_mark,
    help_geometry, show_help, tree_action, prompt, confirm,
  };
// render/open_panel 依赖 helix，node 测试需先 stub global.helix
}
