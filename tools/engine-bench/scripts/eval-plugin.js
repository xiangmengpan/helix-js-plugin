// eval-plugin —— 模拟一个真实插件的**加载期**（helix 启动时 17 个插件各跑一遍）。
//
// 只做纯 JS 的工作（定义函数、建表、注册命令/事件），不碰 fs/进程
// —— 那部分在真实环境里是异步的，不该算进解析/执行成本。
// 阈值：≤ 100ms / 17 个插件（即单个 ≤ 6ms）。
helix.plugin("bench-plugin", { deps: [], permissions: {} });

// ── 配置 schema（真实插件都有） ─────────────────────────────
var CONFIG = {
  show_hidden: false,
  width: 30,
  follow: true,
  icons: { dir: "📁", file: "📄" },
};

function cfg(key, fallback) {
  return CONFIG[key] === undefined ? fallback : CONFIG[key];
}

// ── 纯函数工具（插件里最典型的一层） ──────────────────────────
function basename(p) {
  var i = p.lastIndexOf("/");
  return i < 0 ? p : p.slice(i + 1);
}

function dirname(p) {
  var i = p.lastIndexOf("/");
  return i < 0 ? "." : p.slice(0, i);
}

function join(a, b) {
  if (!a || a === ".") return b;
  return a.charAt(a.length - 1) === "/" ? a + b : a + "/" + b;
}

function indent(depth) {
  var s = "";
  for (var i = 0; i < depth; i++) s += "  ";
  return s;
}

function isHidden(name) {
  return name.charAt(0) === ".";
}

function sortEntries(entries) {
  entries.sort(function (a, b) {
    if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
    return a.name < b.name ? -1 : a.name > b.name ? 1 : 0;
  });
  return entries;
}

// ── 假数据（真实插件从 helix.read_tree 拿，这里造同样的形状） ──
function fakeTree(depth, width) {
  var out = [];
  for (var i = 0; i < width; i++) {
    var name = "item_" + i;
    out.push({ name: name, is_dir: i % 5 === 0, path: join("src", name) });
    if (depth > 0 && i % 7 === 0) {
      var kids = fakeTree(depth - 1, 2);
      for (var j = 0; j < kids.length; j++) out.push(kids[j]);
    }
  }
  return out;
}

// ── 渲染（插件最热的一段） ────────────────────────────────────
function renderRows(entries, cursor) {
  var rows = [];
  for (var i = 0; i < entries.length; i++) {
    var e = entries[i];
    if (!cfg("show_hidden", false) && isHidden(e.name)) continue;
    var icon = e.is_dir ? CONFIG.icons.dir : CONFIG.icons.file;
    var line = indent(e.is_dir ? 0 : 1) + icon + " " + e.name;
    rows.push(i === cursor ? "▸ " + line : "  " + line);
  }
  return rows;
}

// ── 命令（每个插件 3–6 个） ───────────────────────────────────
helix.register_command("bench-toggle", function () {
  CONFIG.show_hidden = !CONFIG.show_hidden;
  helix.echo("hidden=" + CONFIG.show_hidden);
});

helix.register_command("bench-wide", function () {
  CONFIG.width = CONFIG.width >= 60 ? 30 : CONFIG.width + 10;
  helix.echo("width=" + CONFIG.width);
});

helix.register_command("bench-follow", function () {
  CONFIG.follow = !CONFIG.follow;
  helix.echo("follow=" + CONFIG.follow);
});

helix.register_command("bench-render", function () {
  var entries = sortEntries(fakeTree(2, cfg("width", 30)));
  var rows = renderRows(entries, 0);
  helix.echo("rows=" + rows.length + " first=" + (rows[0] || ""));
});

helix.register_command("bench-report", function () {
  var entries = fakeTree(2, cfg("width", 30));
  var dirs = 0;
  var files = 0;
  for (var i = 0; i < entries.length; i++) {
    if (entries[i].is_dir) dirs++;
    else files++;
  }
  helix.echo("dirs=" + dirs + " files=" + files);
});

// ── 事件（真实插件普遍订阅 2–4 个） ───────────────────────────
helix.on("buffer-open", function (doc) {
  if (!doc || !doc.path) return;
  var base = basename(doc.path);
  CONFIG.last = base;
});

helix.on("cwd-change", function (doc) {
  CONFIG.cwd = (doc && doc.path && dirname(doc.path)) || ".";
});

helix.on("doc-change", function (doc) {
  if (!doc) return;
  CONFIG.dirty = true;
});

// ── 键位（真实插件 1–3 条） ──────────────────────────────────
helix.map("normal", "C-b", "bench-toggle");
helix.map("normal", "C-w", "bench-wide");

// ── 状态栏（可选能力） ───────────────────────────────────────
helix.set_statusline(function (ctx) {
  return "bench " + (CONFIG.last || "—");
});

// 让"这段代码真的执行了"可被断言（防引擎把顶层优化掉）
var BENCH_READY = true;
