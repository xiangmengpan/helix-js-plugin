// icons.js — 统一图标映射表（nerd font）
// 依赖清单：无（共享层，其他插件经 deps 自动加载）
helix.plugin("icons", { deps: [], version: "1.0" });
// 唯一图标源：方案 1（bufferline 文件类型图标，load 时自动注册）、
// 方案 2（filetree 树内图标）、方案 3（状态栏 mode 图标）、
// 方案 4（诊断标记图标，Rust 侧经 helix.set_diagnostic_icons 同步）都从这里取。
// 前提：终端使用 nerd font（Windows Terminal / vscode 终端设置字体）。
// 用法：helix.load("icons.js") 自动注册 bufferline 图标；其他插件
//       const icons = helix.load("icons.js"); icons.getFileIcon(path);

// ============================ 映射表 ============================

const ICONS = {
  // 扩展名 → 文件类型图标（nerd font devicons）
  file: {
    rs: "\ue7a8",            // rust
    js: "\ue74e",            // javascript
    mjs: "\ue74e",
    cjs: "\ue74e",
    ts: "\ue628",            // typescript
    tsx: "\ue7ba",
    jsx: "\ue7ba",
    json: "\ue60b",
    toml: "\ue615",
    yaml: "\ue615",
    yml: "\ue615",
    md: "\uf48a",            // markdown
    markdown: "\uf48a",
    py: "\ue606",            // python
    c: "\ue61e",
    h: "\uf0fd",             // header
    cpp: "\ue61d",
    cc: "\ue61d",
    hpp: "\uf0fd",
    go: "\ue626",
    java: "\ue204",
    rb: "\ue739",            // ruby
    php: "\ue73d",
    html: "\uf13b",
    htm: "\uf13b",
    css: "\ue749",
    scss: "\ue603",
    sass: "\ue603",
    less: "\ue758",
    sh: "\uf489",            // terminal
    bash: "\uf489",
    zsh: "\uf489",
    fish: "\uf489",
    sql: "\uf1c0",           // database
    lua: "\ue620",
    swift: "\ue755",
    kt: "\ue634",            // kotlin
    kts: "\ue634",
    scala: "\ue737",
    dart: "\ue798",
    r: "\uf25d",
    ex: "\ue62d",            // elixir
    exs: "\ue62d",
    erl: "\ue7b1",           // erlang
    hs: "\ue777",            // haskell
    clj: "\ue768",           // clojure
    elm: "\ue62c",
    zig: "\ue6a9",
    vue: "\ufd42",
    svelte: "\ue697",
    sol: "\ue73c",           // solidity
    txt: "\uf15c",
    log: "\uf18d",
    pdf: "\uf1c1",
    doc: "\uf1c2",
    docx: "\uf1c2",
    xls: "\uf1c3",
    ppt: "\uf1c4",
    zip: "\uf410",
    tar: "\uf410",
    gz: "\uf410",
    rar: "\uf410",
    "7z": "\uf410",
    png: "\uf1c5",           // image
    jpg: "\uf1c5",
    jpeg: "\uf1c5",
    gif: "\uf1c5",
    svg: "\uf1c5",
    ico: "\uf1c5",
    webp: "\uf1c5",
    mp3: "\uf001",           // audio
    wav: "\uf001",
    flac: "\uf001",
    ogg: "\uf001",
    mp4: "\uf03d",           // video
    avi: "\uf03d",
    mkv: "\uf03d",
    mov: "\uf03d",
    exe: "\uf17d",
    dll: "\uf17d",
    deb: "\uf306",
    rpm: "\uf306",
    lock: "\uf023",          // lockfile
    conf: "\ue615",
    ini: "\ue615",
    env: "\uf462",
    "": "\uf016",            // 无扩展名 → 默认文件
  },

  // 特殊文件名（优先于扩展名匹配）
  special: {
    "readme": "\uf48a",
    "makefile": "\uf489",
    "cmakelists.txt": "\ue615",
    "dockerfile": "\uf308",
    "justfile": "\ue615",
    "license": "\uf718",
    "licence": "\uf718",
    ".gitignore": "\uf1d3",
    ".gitattributes": "\uf1d3",
    ".gitmodules": "\uf1d3",
    "package.json": "\ue718",
    "cargo.toml": "\ue7a8",
    "go.mod": "\ue626",
    "pyproject.toml": "\ue606",
    "requirements.txt": "\ue606",
    "config.toml": "\ue615",
    "init.js": "\ue74e",
  },

  // 目录图标（折叠 / 展开）
  dir: {
    closed: "\uf07b",
    open: "\uf07c",
  },

  // 状态栏模式图标（方案 3）
  mode: {
    normal: "\uf04b",        // play
    insert: "\uf040",        // pencil
    select: "\uf0ca",        // list
  },

  // 诊断标记图标（方案 4：helix.set_diagnostic_icons 从这取）
  diagnostic: {
    error: "\uf00d",         // ×
    warning: "\uf12a",       // ⚠
    info: "\uf129",          // ℹ
    hint: "\uf0eb",          // lightbulb
  },

  // git 变更状态图标（filetree / statusline 用）
  git: {
    M: "\uf403",             // modified ~
    A: "\uf402",             // added +
    D: "\uf41a",             // deleted
    R: "\uf417",             // renamed
    C: "\uf419",             // copied
    U: "\uf404",             // untracked
    "??": "\uf404",
  },

  // LSP CompletionItemKind(数字)→ 补全候选图标。
  // 码位已验证于 CaskaydiaCove Nerd Font 3.x(用 fontTools 扫 cmap 确认字形存在):
  // 大部分是 symbol-* 区(\uea8x-\ueb6x);Function 无专用 symbol 图标用 code(\uf121)。
  completion: {
    1: "\uf031",   // Text          (font)
    2: "\uea8c",   // Method        (symbol-method)
    3: "\uf121",   // Function      (code)
    4: "\ueb5b",   // Constructor   (symbol-class)
    5: "\ueb5f",   // Field         (symbol-field)
    6: "\uea88",   // Variable      (symbol-variable)
    7: "\ueb5b",   // Class         (symbol-class)
    8: "\ueb61",   // Interface     (symbol-interface)
    9: "\uea8b",   // Module        (symbol-namespace)
    10: "\ueb65",  // Property      (symbol-property)
    11: "\uea90",  // Unit          (symbol-numeric)
    12: "\uea8f",  // Value         (symbol-boolean)
    13: "\uea95",  // Enum          (symbol-enum)
    14: "\ueb62",  // Keyword       (symbol-keyword)
    15: "\ueb66",  // Snippet       (symbol-snippet)
    16: "\ueb5c",  // Color         (symbol-color)
    17: "\ueb60",  // File          (symbol-file)
    18: "\ueb36",  // Reference     (references)
    19: "\uf07b",  // Folder        (dir.closed)
    20: "\ueb5e",  // EnumMember    (symbol-enum-member)
    21: "\ueb5d",  // Constant      (symbol-constant)
    22: "\uea91",  // Struct        (symbol-structure)
    23: "\uea86",  // Event         (symbol-event)
    24: "\ueb64",  // Operator      (symbol-operator)
    25: "\uea92",  // TypeParameter (symbol-parameter)
  },
};

// ============================ 工具函数 ============================

/// 文件名 → 图标（特殊名优先，其次扩展名，最后默认文件）
function getFileIcon(path) {
  const name = (path || "").split("/").pop() || "";
  const lower = name.toLowerCase();
  if (ICONS.special[lower]) return ICONS.special[lower];
  const ext = lower.includes(".") ? lower.split(".").pop() : "";
  return ICONS.file[ext] ?? ICONS.file[""];
}

/// 目录图标
function getDirIcon(expanded) {
  return expanded ? ICONS.dir.open : ICONS.dir.closed;
}

/// 状态栏模式图标
function getModeIcon(mode) {
  return ICONS.mode[mode] ?? "";
}

/// 诊断级别图标
function getDiagnosticIcon(severity) {
  return ICONS.diagnostic[severity] ?? "";
}

/// git 状态码 → 图标
function getGitIcon(status) {
  return ICONS.git[status] ?? "";
}

/// LSP kind 数字 → 补全图标;未知/缺省 → ""(无图标)
function getCompletionKindIcon(kind) {
  return ICONS.completion[kind] ?? "";
}

// ============================ 注册 ============================

if (typeof helix !== "undefined") {
  // 方案 1：bufferline 文件类型图标（load 即生效）
  helix.set_buffer_icon(getFileIcon);
  // 内置补全菜单 kind 图标（load 即生效；无图标/未注册时引擎回退默认文本）。
  // 注册放在本脚本 IIFE 内（无嵌套 load）——boa 嵌套 eval 污染下闭包安全。
  helix.set_completion_icon((k) => getCompletionKindIcon(k));
  helix.export({
    ICONS,
    getFileIcon,
    getDirIcon,
    getModeIcon,
    getDiagnosticIcon,
    getGitIcon,
    getCompletionKindIcon,
  });
}

// ============================ node 自检导出 ============================

if (typeof module !== "undefined" && module.exports) {
  module.exports = { ICONS, getFileIcon, getDirIcon, getModeIcon, getDiagnosticIcon, getGitIcon, getCompletionKindIcon };
}
