// helix 插件入口（启动文件，helix 自动加载 ~/.config/helix/init.js）：
// 只列功能插件。图标表已收进核心(helix.icons.*,零依赖);lib/icons.js 只剩 [icons] 配置,
// 所以在这显式加载一次 —— 各插件**不再**需要 deps: ["lib/icons.js"]。
// 目录分层：plugins/lib（共享层）· plugins/features（功能插件）。
// 图标配置模块([icons] nerd_font 的 define_config 在它里面)——显式加载一次。
// 各功能插件已不再依赖它:需要图标直接用 helix.icons.*(核心单一来源)。
helix.load("lib/icons.js");
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

// 可选：picker.js 内置选择器源（files/grep/buffers/symbols）——默认不加载，需要时取消注释：
// helix.load("examples/picker.js");   // 模板,非功能插件
// helix.map("normal", "space-f", () => helix.picker.run("files"));  // 文件选择
// helix.map("normal", "space-g", () => helix.picker.run("grep"));   // 全文搜索
