// icons.js — **薄壳代理**:图标表已收进核心(`helix-js/src/icons.rs`,7 组 145 条)。
//
// 为什么收进核心:图标是"看外观的东西",硬编码在插件里 → 用户改不了、升级被覆盖;
// 而且"字体不支持怎么降级"的策略原先**被每个消费方各写一遍**(filetree 自己写 ▸/▾,
// statusline 自己 `|| null`)。收进核心后:数据一份 · 降级策略一份 · 插件零依赖可用。
//
// 本文件保留同名导出,只为现有消费方(filetree/statusline/tabbar/which-key)**零改动**。
// **新代码请直接用 `helix.icons.file(path)` 等** —— 不必再声明 `deps`。
//
// 已删除:旧的原始表导出 `ICONS`(经查**无任何消费方**使用,它们只用下面 6 个函数)。
helix.plugin("icons", { deps: [], version: "2.0" });

// ─────────────────────────────────────────────────────────────────────────
// ⚠️ 两个已知点(改这个文件前请先读):
//
// 1. **时序**:下面的 `helix.icons.enabled(...)` 在**本文件被加载时**执行一次。
//    改了 config.toml 后 `:config-reload` **不会**重新应用(该机制本是"插件读取时
//    实时生效",而这里是把值**推**进核心开关)。绕过办法:再 `:plugin-reload`。
//
// 2. **终态**:更正确的位置是 `helix-view` 的 Config 里加 `[editor.icons]`,
//    启动时由 Rust 直接调 `helix_js::icons::set_icons_enabled(...)`。
//    那样连本文件与 init.js 里那行显式加载都能删掉 —— 图标是核心外观,
//    本不该依赖一个 JS 薄壳被加载。
//    (没直接做:它动编辑器核心配置结构,需与 `:config-reload` 语义一起设计。)
// ─────────────────────────────────────────────────────────────────────────

// ── 用户配置(官方 config.toml 通道,不需要改代码)──
// 用法:config.toml 里写
//     [icons]
//     nerd_font = false
// 图标是**外观**,本该跟配置走 —— 之前它硬编码在插件里,用户既改不了、升级还会被覆盖。
helix.define_config("icons", {
  nerd_font: {
    type: "boolean",
    default: true,
    doc: "终端字体是否含 nerd font 字形;关掉后文件类图标不显示,目录回退为 ▸/▾",
  },
});

// 应用配置(在核心侧设置,所以 Rust 侧消费方(bufferline/gutter)同样受益)
{
  const cfg = helix.get_config("icons") || {};
  helix.icons.enabled(cfg.nerd_font !== false);
}

// 参数一律先归一为字符串/布尔:Rust 侧绑定要求正确类型,
// 而旧实现靠 JS 对象的隐式键强制转换(number/undefined 都能当键用)。
// 这几个 `String(...)` 就是补上那层转换。
function getFileIcon(path) {
  return helix.icons.file(String(path ?? ""));
}
function getDirIcon(expanded) {
  return helix.icons.dir(!!expanded);
}
function getModeIcon(mode) {
  return helix.icons.mode(String(mode ?? ""));
}
function getDiagnosticIcon(severity) {
  return helix.icons.diagnostic(String(severity ?? ""));
}
function getGitIcon(status) {
  return helix.icons.git(String(status ?? ""));
}
function getCompletionKindIcon(kind) {
  // LSP kind 是**数字**,旧实现靠 `obj[42]` 的隐式转换;这里显式转字符串
  return helix.icons.completion(String(kind ?? ""));
}

if (typeof helix !== "undefined") {
  // 加载时副作用(**必须保留**):bufferline 文件图标 / 补全菜单 kind 图标
  helix.set_buffer_icon(getFileIcon);
  helix.set_completion_icon((k) => getCompletionKindIcon(k));
  helix.export({
    getFileIcon,
    getDirIcon,
    getModeIcon,
    getDiagnosticIcon,
    getGitIcon,
    getCompletionKindIcon,
  });
}

if (typeof module !== "undefined" && module.exports) {
  module.exports = {
    getFileIcon,
    getDirIcon,
    getModeIcon,
    getDiagnosticIcon,
    getGitIcon,
    getCompletionKindIcon,
  };
}
