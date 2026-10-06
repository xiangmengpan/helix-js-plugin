// plugin/plugin.js — 插件管理器(`:plugin`)的 **JS 侧实现,第 1 步**。
//
// ## 为什么命令名暂时叫 `plugin-js`
// `:plugin` 是**一个命令名** —— JS 注册 `plugin` 会**遮挡** Rust 的同名命令。
// 而 Rust 侧的 `install`/`remove`/`status` 还没迁完 → 现在抢占会让那些子命令**直接失效**。
// 所以先挂临时名验证行为,等子命令补齐后**一次性改名**(见 docs/plugin-layout.md §12.2)。
//
// ## 本步覆盖(只用**现有** API,不新增接口)
//   `list`   —— 列出 **manifest 里已安装**的插件(注意:与 Rust 的 `list`(=已**加载**)语义不同,见下)
//   `status` —— 报告插件目录与 manifest 状态
//   `reload` —— **委派**给 Rust 的 `:plugin-reload`(独立命令,零新增)
//
// ## 已勘明的缺口(本步**不做**,如实标注)
//   Rust 的 `list` 列的是"**已加载**的插件名",而 JS 侧**没有**枚举已加载插件的接口 ✗
//   → 要做到语义一致,还需一个薄接口(如 `helix.loaded_plugins()`)。这里先给"已安装",
//     它是 manifest 的事实,且**不会假称**等于"已加载"。
helix.plugin("plugin-manager", { deps: [], version: "0.1" });

/// manifest 路径:`helix.plugins_dir()`(= 用户层插件根,`config_dir/plugins`)
function manifestPath() {
  const dir = helix.plugins_dir();
  return dir ? dir + "/manifest.json" : null;
}

/// 读 manifest:缺文件/坏 JSON → **空表**(与 Rust 侧 `read_manifest` 的"不崩"语义一致)
async function readManifest() {
  const p = manifestPath();
  if (!p) return null;
  let raw;
  try {
    raw = await helix.read_file_async(p);
  } catch (e) {
    return {}; // 缺文件
  }
  if (!raw) return {};
  try {
    const m = JSON.parse(raw);
    return m && typeof m === "object" ? m : {};
  } catch (e) {
    return {}; // 坏 JSON → 空(对齐 Rust)
  }
}

helix.register_command("plugin-js", async () => {
  const args = helix.command_args();
  const sub = args[0] || "status";
  const mp = manifestPath();
  if (!mp) {
    helix.echo("plugin: 插件目录不可用(helix.plugins_dir() 返回 null)");
    return;
  }
  switch (sub) {
    case "list": {
      const m = await readManifest();
      const names = Object.keys(m || {}).sort();
      helix.echo(
        names.length === 0
          ? "no plugins installed (manifest empty)"
          : "installed(" + names.length + "): " + names.join(", ")
      );
      break;
    }
    case "status": {
      const m = await readManifest();
      const n = Object.keys(m || {}).length;
      helix.echo("plugin dir: " + mp + " · manifest entries: " + n);
      break;
    }
    case "reload":
      // 委派给 Rust 的独立命令(本步不搬它 —— 自举兜底)
      helix.echo("请用 :plugin-reload(JS 侧插件管理器的 reload 尚未接管)");
      break;
    default:
      helix.echo(
        "plugin-js: 本步只实现 list|status|reload;" +
          " install/remove 待迁移后由 :plugin 提供(见 §12.2)"
      );
  }
});
