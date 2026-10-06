// layout/plugin.js — 命名布局的会话保存/载入:`:layout save|load|list|delete <name>`
//
// **第二个"从核心搬到插件"的功能**(第一个是 `:tutor`)。
// 搬迁前:核心 `typed.rs` 里 58 行(条目 11 + `fn layout` 47),而它只是 `helix.layout.*` 的包装。
// 搬迁的关键前置是 **§12.0 参数通道** —— 否则插件读不到 `save`/`<name>`。
//
// 数据 API 一直在 helix-js 里(`helix.layout.save/load/list/delete`),带纯函数单测与**路径穿越防护**;
// 本插件只做"解析子命令 + 转发 + 反馈"。
helix.plugin("layout", { deps: [], version: "1.0" });

helix.register_command("layout", () => {
  const args = helix.command_args();
  const sub = args[0];
  const name = args[1];
  if (!sub) return helix.echo("usage: layout save|load|list|delete <name>");
  try {
    switch (sub) {
      case "save":
        if (!name) return helix.echo("usage: layout save <name>");
        helix.layout.save(name);
        helix.echo("已保存布局 " + name + "(保存目录见 :config-dir/layouts)");
        break;
      case "load":
        if (!name) return helix.echo("usage: layout load <name>");
        helix.layout.load(name);
        helix.echo("已载入布局 " + name);
        break;
      case "list": {
        const names = helix.layout.list();
        helix.echo(
          names.length === 0 ? "没有已保存的布局" : "布局(" + names.length + "): " + names.join(" · ")
        );
        break;
      }
      case "delete": {
        if (!name) return helix.echo("usage: layout delete <name>");
        const hit = helix.layout.delete(name);
        helix.echo(hit ? "已删除布局 " + name : "没有名为 " + name + " 的布局");
        break;
      }
      default:
        helix.echo("layout: unknown subcommand '" + sub + "'(save|load|list|delete)");
    }
  } catch (e) {
    helix.echo("layout " + sub + ": " + (e && e.message ? e.message : e));
  }
});
