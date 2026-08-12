// terminal.js — 终端管理器（lazyvim 风格）：浮动 toggle、分屏、列表、按 id 关闭
// 依赖 Rust 增强：浮动终端层（set_terminal_mode "floating"）、C-\ 模式穿透、
// 滚动缓冲查看、term_list/term_close、宽字符。Unix pty（Windows 不可用）。
// 交互：
//   - 终端内 C-\ → 终端 normal 模式（j/k/gg/G/PageUp/PageDown 滚动缓冲；i/a/Esc 回输入）
//   - 终端 normal 模式再 C-\ → 收起浮窗回编辑器（若浮动）
//   - 终端 insert 模式 Esc → 关闭终端

// ============================ 状态 ============================

let floatId = null;        // 浮动单例终端的 view_id（toggle 用）
let lastId = null;         // 最近打开的终端（term-close 默认目标）

function shell() {
  try {
    const s = helix.run("echo ${SHELL:-bash}").trim();
    return s || "bash";
  } catch (e) {
    return "cmd";
  }
}

// ============================ 命令 ============================

// 浮动终端 toggle：开 → 复用单例浮窗；再开 → 关闭
helix.register_command("term", () => {
  if (floatId !== null && helix.term_list().some((t) => t.view_id === floatId)) {
    helix.term_close(floatId);
    floatId = null;
    return;
  }
  floatId = helix.open_terminal({ cmd: shell(), side: "bottom", size: 10 });
  helix.set_terminal_mode(floatId, "floating");
});

// 分屏终端：vterm 右侧 / hterm 底部
helix.register_command("vterm", () => {
  lastId = helix.open_terminal({ cmd: shell(), side: "right", size: 40 });
});
helix.register_command("hterm", () => {
  lastId = helix.open_terminal({ cmd: shell(), side: "bottom", size: 15 });
});

// 列出所有终端（view_id + cmd）
helix.register_command("term-list", () => {
  const terms = helix.term_list();
  helix.echo(
    terms.length
      ? terms.map((t) => "[" + t.view_id + "] " + t.cmd).join("  ")
      : "无终端"
  );
});

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
});

// ============================ 事件 ============================

// 终端退出（pty 进程结束）→ 清理浮动单例引用
helix.on("buffer-open", () => {
  if (floatId !== null && !helix.term_list().some((t) => t.view_id === floatId)) {
    floatId = null;
  }
});
