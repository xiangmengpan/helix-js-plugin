// terminal.js — 终端管理器（lazyvim 风格）：浮动 toggle、分屏、列表、按 id 关闭
// 依赖清单：无（独立功能）
helix.plugin("terminal", {
  deps: [],
  version: "1.0",
  schema_version: 1,
  // 取默认 shell(`echo ${SHELL:-bash}`)⇒ 起进程
  permissions: { process: true },
});
// 依赖 Rust 增强：浮动终端层（set_terminal_mode "floating"）、C-\ 模式穿透、
// 滚动缓冲查看、term_list/term_close、宽字符。Unix pty（Windows 不可用）。
// 交互：
//   - 终端内 C-\ → 终端 normal 模式（j/k/gg/G/PageUp/PageDown 滚动缓冲；i/a/Esc 回输入）
//   - 终端 normal 模式再 C-\ → 收起浮窗回编辑器（若浮动）
//   - 终端 insert 模式 Esc → 关闭终端

// 插件配置(方案 C):config.toml [plugins.terminal]
// 注意 `shell` 与核心 `[editor] shell` **不是一回事**:后者是 `:sh`/管道用的数组,
// 这里是终端的单个可执行名(留空 ⇒ 读 `$SHELL`,再回退 bash)。
helix.define_config("terminal", {
  shell: { type: "string", default: "", doc: "终端用的 shell 可执行名;留空 = 读 $SHELL,再回退 bash" },
  float_size: { type: "number", default: 10, doc: ":term 浮动终端的初始高度(行,>0)" },
  vterm_size: { type: "number", default: 40, doc: ":vterm 右侧分屏的宽度(列,>0)" },
  hterm_size: { type: "number", default: 15, doc: ":hterm 底部分屏的高度(行,>0)" },
});
// 取值一律**现取**(不在顶层缓存对象):`:config-reload` 只重建配置缓存、不重跑插件脚本,
// 缓存下来的旧对象会让改配置"没反应"。filetree 那种 `const CFG = ...` 是同一个坑的旧写法。
//
// 尺寸兜底:0/负数/非数字一律回到默认(没人想要 0 行的终端)
const size_of = (v, dflt) => (typeof v === "number" && v > 0 ? Math.floor(v) : dflt);

// ============================ 状态 ============================

let floatId = null;        // 浮动单例终端的 view_id（toggle 用）
let lastId = null;         // 最近打开的终端（term-close 默认目标）

// 用 const 箭头函数而非 function 声明：boa 0.21 下 function 声明与 let 绑定同作用域
// 时闭包槽位会错乱（第二次调用读到的 shell 是 floatId 的数字值 → "not a callable function"）
const shell = () => {
  // ① 显式配置优先（[plugins.terminal] shell）
  const cfg = (helix.get_config("terminal") || {}).shell;
  if (typeof cfg === "string" && cfg.trim()) return cfg.trim();
  // ② 否则读环境变量（每次现取，`:cd`/改环境后仍正确）
  try {
    const s = helix.run("echo ${SHELL:-bash}").trim();
    return s || "bash";
  } catch (e) {
    return "cmd";
  }
};

// ============================ 命令 ============================

// 浮动终端 toggle：开 → 复用单例浮窗；再开 → 关闭
helix.register_command("term", () => {
  if (floatId !== null && helix.term_list().some((t) => t.view_id === floatId)) {
    helix.term_close(floatId);
    floatId = null;
    return;
  }
  floatId = helix.open_terminal({
    cmd: shell(),
    side: "bottom",
    size: size_of((helix.get_config("terminal") || {}).float_size, 10),
  });
  helix.set_terminal_mode(floatId, "floating");
}, '底部打开终端');

// 分屏终端：vterm 右侧 / hterm 底部
helix.register_command("vterm", () => {
  lastId = helix.open_terminal({
    cmd: shell(),
    side: "right",
    size: size_of((helix.get_config("terminal") || {}).vterm_size, 40),
  });
}, '右侧竖分屏开终端');
helix.register_command("hterm", () => {
  lastId = helix.open_terminal({
    cmd: shell(),
    side: "bottom",
    size: size_of((helix.get_config("terminal") || {}).hterm_size, 15),
  });
}, '下侧横分屏开终端');

// 列出所有终端（view_id + cmd）
helix.register_command("term-list", () => {
  const terms = helix.term_list();
  helix.echo(
    terms.length
      ? terms.map((t) => "[" + t.view_id + "] " + t.cmd).join("  ")
      : "无终端"
  );
}, '列出所有终端');

// 关闭终端：默认最近打开的；传参不可用（插件命令无参）——用浮动单例或最近
helix.register_command("term-close", () => {
  const terms = helix.term_list();
  if (floatId !== null && terms.some((t) => t.view_id === floatId)) {
    helix.term_close(floatId);
    floatId = null;
  } else if (terms.length) {
    helix.term_close(terms[terms.length - 1].view_id);
  } else {
    helix.echo("无终端");
  }
}, '关掉当前终端');

// 持久化保存终端内容（scrollback + 屏幕）到文件：默认
// ~/.cache/helix/term-<view_id>.log；终端内 normal 模式 y 可复制可视区到剪贴板
helix.register_command("term-save", () => {
  const terms = helix.term_list();
  const t = terms.find((x) => x.view_id === floatId) || terms[terms.length - 1];
  if (!t) {
    helix.echo("无终端");
    return;
  }
  helix.term_save(t.view_id);
}, '把终端缓冲存成文件');

// ============================ 事件 ============================

// 终端退出（pty 进程结束）→ 清理浮动单例引用
helix.on("buffer-open", () => {
  if (floatId !== null && !helix.term_list().some((t) => t.view_id === floatId)) {
    floatId = null;
  }
});
