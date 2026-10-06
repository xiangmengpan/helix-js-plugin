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
// ## 已勘明的缺口(如实标注,不假装完整)
//   1. Rust 的 `list` 列的是"**已加载**"的插件名,而 JS 侧**没有**枚举已加载插件的接口 ✗
//      → 这里先给"**已安装**"(manifest 的事实),**不假称**等于"已加载"。全保真需 `helix.loaded_plugins()`。
//   2. ~~`install` 被挡住~~ → **已解锁**:新增限域接口 `helix.write_plugin_file(rel, text)`
//      (父目录按需创建;只相对路径、拒 `..`)后,本地文件/目录安装可在 JS 侧完成;
//      git 源走 `helix.run("git clone …")`。
//   3. **布局不一致(实测)**:Rust `install_target` 装进 `plugins/**features**/<name>`(旧路径),
//      而当前仓库布局是 `plugins/<name>/`。搬迁时应统一到现有布局,别把旧路径带过去。
helix.plugin("plugin-manager", { deps: [], version: "0.1" });

/// shell 单引号引用(与 statusline 插件同款)
function shq(s) {
  return "'" + String(s).replace(/'/g, "'\\''") + "'";
}

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
    case "remove": {
      const name = args[1];
      if (!name) {
        helix.echo("usage: plugin-js remove <name>");
        break;
      }
      const m = (await readManifest()) || {};
      const e = m[name];
      if (!e) {
        helix.echo("没有名为 '" + name + "' 的安装记录");
        break;
      }
      // 逐个删 manifest 记录的文件(**限域**:remove_plugin_file 只接受相对路径、拒 `..`)
      let n = 0;
      for (const rel of e.files || []) {
        try {
          if (helix.remove_plugin_file(rel)) n++;
        } catch (err) {
          /* 单个失败不中断(与 Rust 的 remove_files 一致:尽力而为) */
        }
      }
      // 等价于 Rust 的 remove_orphan:整棵目录也删掉(同样限域)
      try {
        helix.remove_plugin_file(name);
      } catch (err) {}
      delete m[name];
      try {
        await helix.write_file_async(manifestPath(), JSON.stringify(m, null, 2));
      } catch (err) {
        helix.echo("plugin remove: manifest 写入失败: " + err);
        break;
      }
      helix.echo("已移除 '" + name + "'(" + n + " 个文件;manifest 已更新)");
      break;
    }
    case "install": {
      const arg = args[1];
      if (!arg) {
        helix.echo("usage: plugin-js install <path|git-url>");
        break;
      }
      const m = (await readManifest()) || {};
      const now = new Date().toISOString();
      const isGit = /^[a-z+]+:\/\//i.test(arg) || arg.endsWith(".git");

      if (isGit) {
        // git 源:名字取 URL 末段(去掉 .git),clone 到 <plugins>/<name>
        const base = arg.replace(/\/+$/, "").split("/").pop().replace(/\.git$/, "");
        const dest = mp + "/" + base;
        try {
          await helix.run_async("git clone --depth 1 " + shq(arg) + " " + shq(dest));
        } catch (e) {
          helix.echo("plugin install: git clone 失败: " + e);
          break;
        }
        m[base] = { source: arg, kind: "git", installed_at: now, pinned: false, files: [base] };
      } else {
        // 本地路径:文件 → <name>/plugin.js;目录 → 递归复制其内容
        const base = arg.replace(/\/+$/, "").split("/").pop();
        const stem = base.replace(/\.[^.]+$/, "");
        let entries;
        try {
          entries = helix.read_dir(arg); // 同步;目录或文件都能列
        } catch (e) {
          helix.echo("plugin install: 路径不可读: " + arg);
          break;
        }
        const isDir = entries.some((e) => e && (e.is_dir === true || e.isDir === true));
        const files = [];
        if (isDir) {
          for (const e of entries) {
            const nm = e.name || e.path;
            const txt = await helix.read_file_async(arg + "/" + nm);
            helix.write_plugin_file(stem + "/" + nm, txt);
            files.push(stem + "/" + nm);
          }
        } else {
          const txt = await helix.read_file_async(arg);
          helix.write_plugin_file(stem + "/plugin.js", txt);
          files.push(stem + "/plugin.js");
        }
        m[stem] = { source: arg, kind: "local", installed_at: now, pinned: false, files: files };
      }
      try {
        await helix.write_file_async(manifestPath(), JSON.stringify(m, null, 2));
      } catch (e) {
        helix.echo("plugin install: manifest 写入失败: " + e);
        break;
      }
      const installed = Object.keys(m).pop();
      helix.echo("installed '" + installed + "', 请 :plugin-reload 生效");
      break;
    }
    case "reload":
      // 委派给 Rust 的独立命令(本步不搬它 —— 自举兜底)
      helix.echo("请用 :plugin-reload(JS 侧插件管理器的 reload 尚未接管)");
      break;
    default:
      helix.echo(
        "plugin-js: 已实现 list|status|install|remove|reload(见 §12.2)"
      );
  }
});
