// helix 插件入口：统一图标源（自动注册 bufferline 文件类型图标）+ 文件树面板
helix.load("icons.js");
helix.load("filetree.js");

// 方案 3 可选：状态栏 mode 图标（整行替换默认状态栏，默认不启用）
// const icons = helix.load("icons.js");
// helix.set_statusline(({ mode, path }) => {
//   const name = path ? path.split("/").pop() : "";
//   return (icons.getModeIcon(mode) + " " + name).trim();
// });

// 可选键位绑定（默认不绑，避免与既有键位冲突）
// helix.map("normal", "C-e", "filetree");
