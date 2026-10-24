# API:插件管理

> 详细参考。总览见 [`docs/plugin-api.md`](../plugin-api.md)。

## 实现位置(先看这个,否则会找错文件)

| 部分 | 在哪 |
|---|---|
| `:plugin <op>` 的**全部 8 个操作** | **JS 侧**:内置插件 [`plugins/plugin/plugin.js`](../../plugins/plugin/plugin.js) |
| `:plugin-load <path>` / `:plugin-reload` | Rust(两个**独立**命令,不是 `:plugin` 的子命令)|
| 文件/目录读写能力(限域) | 核心:`helix.read_file` · `write_file` · `write_plugin_file` · `remove_plugin_file` · `plugins_dir` · `loaded_plugins` |

> ⚠️ **`docs/superpowers/` 与旧文档里的 `features/<name>`、`vendor/<name>`、入口 `index.js` 都已不存在。**
> 早先这些操作在 Rust 里实现(装进 `plugins/features/`),JS 侧接管后一并删除;
> 现在插件一律是 `<插件根>/<name>/plugin.js`(约定见 [`plugin-layout.md`](../plugin-layout.md) §3)。

因为 `:plugin` 是**插件注册的命令**,所以它需要内置插件 `plugin` 被加载才存在 —— 内置 `init.js` 已包含它。
若你的 `init.js` 覆盖后没加载 `plugin`,`:plugin` 就没有了(但 `:plugin-load` / `:plugin-reload` 仍在)。

## 命令面(`:plugin <op>`,8 个操作)

| op | 行为 |
|---|---|
| `install <git-url>` | `git clone --depth 1 <url> <插件根>/<name>`(名字取 URL 末段去 `.git`),写 manifest。**装完不加载**,提示 `:plugin-reload` |
| `install <本地路径>` | **文件** → `<插件根>/<stem>/plugin.js`；**目录** → **递归**整棵树到 `<插件根>/<dirname>/`。写到一半失败会**回滚**（删掉已写入的文件，不留孤儿），manifest 不记 |
| `update [name\|all]` | 只对 `kind == "git"` 且**未 pinned** 的做 `git fetch` + `git merge --ff-only FETCH_HEAD`;成功后把 `git rev-parse HEAD` 记进 manifest 的 `commit`;失败保留旧版 |
| `pin <name>` / `unpin <name>` | 锁定/解锁(`update` 会跳过);**仅 git 源**,且必须已安装 |
| `remove <name>` | 按 manifest 的 `files` 逐个删(限域、尽力而为),再删条目;**不在 manifest 里 → 报错**(不认"孤儿"安装)|
| `list` | 第一行 = **已加载**的脚本(与核心 `loaded_scripts()` 同源);附加一行 = manifest 里**已安装**的(便于看出"装了但没加载")|
| `status` | `"{N} plugins loaded from {dir}, {M} installed ({G} git, {P} pinned)"`(集成测试按此文案断言,勿轻易改)|
| `reload` | ✅ 调 `helix.reload_plugins()`（**入队**，命令边界重跑全部脚本；与 `:plugin-reload` 同一实现）|

另有三个独立命令:

```
:plugin-load <path>     # 手动加载任意 .js 文件(开发用;不写 manifest)
:plugin-reload          # 热重载:重读磁盘 + 重置插件状态(Rust 侧实现,真正生效的是它)
:plugin-unload <name>   # 卸载**单个**插件:按属主回收它注册的一切(见下节)
```

## 卸载一个插件(`:plugin-unload <name>`)

2026-10-23 起支持**单插件**回收 —— 在这之前只有"全部重载"(`:plugin-reload` 会把所有插件
一起重新 eval,丢掉它们的运行时状态)。

```js
// 插件侧:卸载前想自己收资源(pty/watch/定时器)就在这里做
helix.plugin("filetree", {
  onUnload: () => { /* 关掉自己起的进程 / 退订 watch */ },
});
```

`:plugin-unload <name>` 跑完状态栏报告计数
(`unloaded filetree: 6 commands / 2 handlers / 1 hooks / 2 keys / 0 themes / 1 popups / 0 panels`)。

**按属主回收的产物**(属主 = 注册期绑定的插件名,与权限同一套判据):

| 产物 | 怎么收 |
|---|---|
| 命令(含 `helix.map` 生成的 `__mapped_N`) | 从注册表/命令说明里删 |
| 事件处理器 | 按**下标**双侧同步删(`handler_owners` 与处理器列表,绝不单边 retain) |
| 单例钩子(状态栏/图标/补全/键位提示) | 属主是该插件 ⇒ 清槽(状态栏还要复位 replace) |
| 弹窗 / 面板 | 入队 `ClosePopup` / `ClosePanel`,并从 `popups` 表摘掉 |
| 键位(`helix.map`) | 每条入队 `MapKey{unbind}` → 核心 `remove_binding`(真摘叶) |
| 主题覆盖(`helix.set_theme`) | 摘掉该插件的 scope + 置脏(下次 drain 还原基准主题) |
| picker 源 / 节点回调 / watch | 从对应表删 / 入队 `Unwatch` |
| 导出与加载清单 | 从 `script_exports` / `loaded_scripts` 摘掉 |

**做不到的**(不假装做到):

- **JS 全局变量与闭包**:同一 boa Realm 里无法回收(顶层的 `const`/`let` 仍在)。
  卸载后重新加载同名插件时,新脚本有独立的 IIFE 作用域,但**它看到的全局仍带着旧值**。
- **插件自己起的 pty / 外部资源**:不做猜测性清理 ⇒ 请在 `onUnload` 里自己关。
- **异步 promise / LSP 请求的后续回调**:只摘属主;晚到的回调会按"无属主"fail-closed
  (报"未声明能力"),不会静默跑到别的插件名下。

**与 `:plugin-reload` 的差别**:

| | `:plugin-unload <name>` | `:plugin-reload` |
|---|---|---|
| 范围 | 单个插件 | 全部 |
| 其它插件的运行时状态 | **保留** | 丢失(全部重新 eval) |
| `onUnload` | 跑 | 跑(每个属主都跑一遍) |
| 之后重新加载 | 不会(用 `:plugin-load` / `:plugin-reload`) | 会(重读磁盘) |

实现:**一份**清理逻辑(`helix_js::unload_plugin`),`reload_all` 就是"对每个已知属主调它,
再按老的规矩兜底清一次没有属主的注册"。

## 存储与来源追踪

- **manifest**:`<插件根>/manifest.json`,即 `helix.plugins_dir()` —— **最后一个插件根 = 用户层**
  (`~/.config/helix/plugins/manifest.json`)
- 条目形状:
  ```json
  { "<name>": { "source": "...", "kind": "local|git", "installed_at": "2026-…",
                "pinned": false, "files": ["<name>/plugin.js", "…"], "commit": "…" } }
  ```
  - `commit` **只在 `update` 时写入**;`install` 不写
  - `files` 是**相对插件根的路径**列表 —— `remove` 就按它逐个删,所以它同时是**卸载清单**
- 读写都走**同步** API(`helix.read_file` / `helix.write_file`)
- **坏 JSON / 缺文件 → 视为空表,不崩**(JS 侧 `readManifest()` 与 Rust 侧同语义)

## 依赖:两种,别混

| 类型 | 声明处 | 现状 |
|---|---|---|
| **加载期依赖** | 脚本内 `helix.plugin("<name>", { deps: ["icons"] })` | ✅ **有效**:`deps` 用**插件名**(文件 key 也仍接受);`helix.load` 拓扑加载 + **按 key 缓存** + **循环依赖报错**(给出完整链 `a -> b -> a`)|
| **安装期依赖** | git 仓库根 `plugin.json` 的 `deps` | ❌ **判定不做**(不是"还没做")。理由见 [`plugin-api.md` §4](../plugin-api.md#判定为不做不是待办):git 源只有一个可装版本,求解器代价 >> 收益。第三方插件的依赖请在 README 里让用户手动装 |

⚠️ 加载期依赖有一个**反直觉的时序**(代码注释里专门纠正过):
**依赖是在本脚本 `eval` 之后才加载的** ⇒ 脚本顶层代码不能假定依赖已经执行过,
要用依赖就必须延迟到回调 / 命令里(否则拿到的是 `undefined`,而且**不报错**)。

## JS API

```js
// ① 核心提供:声明加载期依赖(deps 用插件名;拓扑加载 + 循环检测)+ 卸载钩子
helix.plugin("my-plugin", {
  deps: ["icons"],
  version: "1.0",
  onUnload: () => { /* :plugin-unload / :plugin-reload 前跑一次;不能阻止卸载 */ },
});

// ② 内置插件 `plugin` 提供(兼容层):与 :plugin install|update|remove 是同一实现
helix.plugin.install("<git-url>");
helix.plugin.update();          // 不传 = 全部;可传单个 name
helix.plugin.remove("name");    // 必须已在 manifest 里
```

- `helix.plugin` **本身**由核心注册(可调用对象);`.install` / `.update` / `.remove` 是
  `plugins/plugin/plugin.js` **挂上去的**(带 `__hx_shim` 标记,供测试判别"走的确实是 JS 层")
  ⇒ 这三个入口需要内置 `plugin` 插件被加载
- 结果**经状态栏**显示(fire-and-forget,**无 Promise**);缺参数 → echo `usage: …`(不抛错);
  真正的失败(如 `remove` 一个没装的)才抛错

## 已知问题

1. 同一文件里还有两处过时注释:"命令名仍为临时名 `plugin-js`"(实际已注册为 `plugin`)、
   "写路径仍须 `await`"(现已用同步的 `write_file` / `write_plugin_file`)。

> **本地安装曾长期不可用**(2026-10-19 修):`helix.read_dir` 对**文件**返回 ENOTDIR ⇒ 单文件装不了;
> 目录分支只拷一层、遇到子目录 `read_file` 抛错且**已写入的文件留在盘上**。
> 现在 `read_dir(文件) → []`(见 [`process-fs.md`](process-fs.md)),JS 侧据此区分"文件/目录/非法路径",
> 目录递归拷贝、失败回滚。回归:`plugin_install_single_file_local` /
> `plugin_install_dir_recursive` / `plugin_install_failure_leaves_no_orphan`。

## 优缺点

**优点**
- **来源追踪**:manifest 记录 source / kind / installed_at / pinned / commit / files
- **卸载闭环**:`files` 即卸载清单,`remove` 按它逐个删(单个失败不中断,尽力而为)
- **命令行与 JS API 同一实现**,不会两边漂移
- **限域**:写/删只接受**相对路径**、拒 `..`、`canonicalize` 后必须落在插件根内(双保险)
  ⇒ 即使 manifest 被改坏,也删不到插件根之外
- 坏/缺 manifest 不崩

**局限**
- **git 操作同步阻塞**:`helix.run("git clone …")` 在主线程跑 ⇒ 大仓库 clone 期间界面冻结
- 无 Promise 返回值,结果只能看状态栏
- **装完不自动加载**,必须再 `:plugin-reload`
- 本地安装**不建空子目录**(没有文件要写);入口固定是 `plugin.js`,不影响加载
- `./x.git` 这类本地路径会被**误判成 git 源**(`arg.endsWith(".git")`)
- `pin` 只做"锁定不更新",**不记录期望版本**
- 同名安装直接**覆盖** manifest 条目与文件(不做"先 remove 再装");git 源若目标目录已存在,
  `git clone` 会直接失败
- **manifest 里没有就不认**:不兼容"无 manifest 的历史安装 / 孤儿目录"

## 相关

- [`docs/plugin-layout.md`](../plugin-layout.md) —— 目录约定、两层覆盖、分发
- [`plugins/README.md`](../../plugins/README.md) —— 内置插件清单与开发工具
- [`docs/api/process-fs.md`](process-fs.md) —— 进程执行与**异步**文件系统(`read_file_async` 等)、同步 `read_dir` 的语义
- 注:**同步** `helix.read_file` / `write_file`(本页的安装/卸载所依赖的)目前只在
  [`plugins/helix.d.ts`](../../plugins/helix.d.ts) 里有签名,`process-fs.md` 尚未收录。
