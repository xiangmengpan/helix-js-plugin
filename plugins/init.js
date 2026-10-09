// helix 插件入口（启动文件，helix 自动加载 ~/.config/helix/init.js）：
// 只列功能插件,**全部按名字点名**(裸名 → <name>/plugin.js,见 docs/plugin-layout.md §3.3)。
// icons 只剩 [icons] 的 define_config,所以在这显式加载一次;各功能插件用 helix.icons.* 零依赖。
// 目录分层：plugins/lib（共享层）· plugins/features（功能插件）。
// 图标配置模块([icons] nerd_font 的 define_config 在它里面)——显式加载一次。
// 各功能插件已不再依赖它:需要图标直接用 helix.icons.*(核心单一来源)。
// ── 插件加载包装:单个插件失败**不应**拖垮整个 init.js ──
// (实测事故:某条 load 抛错 → init.js 整体失败 → 下面所有插件都不加载,
//  于是一次版本错配变成"插件全灭"。Neovim 也不会因一个插件报错就让其余全不加载。)
// 报错到状态栏,然后继续。
function safe_load(p) {
  try {
    return helix.load(p);   // 内部必须用原生 helix.load(否则自己调自己 → 栈溢出)
  } catch (e) {
    helix.echo("插件 " + p + " 加载失败: " + (e && e.message ? e.message : e));
    return null;
  }
}

safe_load("icons");
safe_load("tutor");
safe_load("layout");
safe_load("plugin");
safe_load("pane-open");
// 【已临时停用】dashboard 启动屏有 bug(无法退出/文件打不开/e 卡死):全局键位未自动解绑
// safe_load("dashboard"); // helix.pane.open({place,content})统一创建入口   // :plugin 管理器(JS 侧第 1 步,临时命令名 plugin-js)   // :layout 会话(- 已从核心搬到插件)   // 教程(:tutor)—— 已从核心搬到插件
safe_load("terminal");
safe_load("statusline");
safe_load("filetree");

// 方案 3 可选：状态栏 mode 图标（整行替换默认状态栏，默认不启用）
// const icons = helix.load("icons");
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
