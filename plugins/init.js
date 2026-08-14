// helix 插件入口（启动文件，helix 自动加载 ~/.config/helix/init.js）：
// 只列功能插件；共享依赖（icons）由各插件 helix.plugin deps 清单自动加载。
// 目录分层：plugins/lib（共享层）· plugins/features（功能插件）。
helix.load("features/terminal.js");
helix.load("features/statusline.js");
helix.load("features/filetree/index.js");

// 方案 3 可选：状态栏 mode 图标（整行替换默认状态栏，默认不启用）
// const icons = helix.load("lib/icons.js");
// helix.set_statusline(({ mode, path }) => {
//   const name = path ? path.split("/").pop() : "";
//   return (icons.getModeIcon(mode) + " " + name).trim();
// });

// 可选键位绑定（默认不绑，避免与既有键位冲突）
// helix.map("normal", "C-e", "filetree");
