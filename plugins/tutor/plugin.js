// tutor/plugin.js — `:tutor`(教程)。
//
// **这是本 fork 第一个"从核心搬到插件"的功能**(与 Neovim 把 `tutor.vim` 放进
// `runtime/plugin/` 同一个模式):
//   · 内容本来就是数据:`runtime/tutor`(50KB),一直在核心之外
//   · 核心原先只有 11 行(打开那个文件),现在这 11 行也搬到了这里
//
// 用到两个薄接口(都是为此新增的):
//   `helix.runtime_path(name)` —— JS 侧拿不到 runtime 目录(helix-js 不依赖 helix-loader),
//                                 所以由 helix-term 启动时推入
//   `open_file(p, { scratch: true })` —— **不绑定路径**:用户 `:w` 不会覆盖 runtime 里的原文件
//                                        (Rust 版原本也是 `set_path(None)`,这是安全前提)
helix.plugin("tutor", { deps: [], version: "1.0" });

helix.register_command("tutor", () => {
  const p = helix.runtime_path("tutor");
  if (!p) {
    helix.echo("找不到 runtime/tutor(安装不完整?)");
    return;
  }
  helix.open_file(p, { scratch: true });
});
