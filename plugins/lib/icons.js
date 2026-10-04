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
