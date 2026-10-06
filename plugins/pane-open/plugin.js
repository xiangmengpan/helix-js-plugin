// pane-open/plugin.js — 给 `helix.pane` 补上**统一创建入口** `open({place, content})`。
//
// ## 为什么是"收敛"而不是"新能力"
// 侦察结论(`helix-js/src/layout.rs`):
//   `helix.split(dir, opts)` **本就支持内容** —— `opts.terminal = {cmd, size}` · `opts.panel = {…}`
//   ⇒ "带内容创建一个 pane"的能力**一直都在** ✓,缺的只是一个**统一的入口形状**
//   所以 marker #45("统一创建入口,取代 split")的落地 = 把 `split(dir, opts)` 收敛成
//   `pane.open({place, content})` —— **零 Rust 改动** ✓
//
// ## 为什么放在 JS 侧
// `helix.pane` 是 `helix` 对象上的**属性对象**(`builder.property("pane", …)`)⇒
// JS 可以给它加属性(§13 的探测已证:原生对象既可挂新属性、也可覆盖既有属性 ✓)
// ⇒ 这类"形状收敛"完全不需要动核心 ✓
helix.plugin("pane-open", { deps: [], version: "1.0" });

/// `helix.pane.open({ place, content })`
///   place  : "right" | "left" | "top" | "bottom"(也接受别名 below/above)
///   content: 直接透传给 `helix.split` 的 opts(`{terminal}` / `{panel}` / …)
/// 返回:undefined(创建是异步的,与 split 一致 —— 结果在下一帧可见)
helix.pane.open = (opts) => {
  if (!opts || typeof opts !== "object") {
    throw new Error("helix.pane.open: requires { place, content }");
  }
  const alias = { below: "bottom", above: "top" };
  const place = alias[opts.place] || opts.place || "right";
  const content = opts.content || {};
  helix.split(place, content); // 底层就是既有实现(它已支持 terminal/panel 等内容)
};
// 供测试断言"走的是 JS 层"(否则分不清新旧实现)
helix.pane.open.__hx_shim = true;
