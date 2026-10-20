# plugins/ —— 内置 JS 插件(源目录)

> **这个目录是「随软件分发的内置插件」的源。** 运行时用的是副本,有两层:
>
> | 层 | 位置 | 说明 |
> |---|---|---|
> | 内置层 | `<runtime>/plugins/` | 由 `contrib/install-plugins.sh` 从本目录装入 |
> | 用户层 | `~/.config/helix/plugins/` | **同名目录整体覆盖内置层**(不是逐文件覆盖)|
>
> 插件 **API 手册**在 [`docs/plugin-api.md`](../docs/plugin-api.md) 与 [`docs/api/`](../docs/api/)(按域分文件);
> 本文件只讲**这个目录本身**:约定 / 清单 / 工具 / 坑。

## 1. 目录约定

```
plugins/
├── init.js            # 入口:只做"点名"(裸名 → <name>/plugin.js)
├── <name>/plugin.js   # 一个插件 = 一个目录 + 固定入口 plugin.js
├── <name>/tests/      # 该插件自己的 node 测试(可跑,不参与加载)
├── examples/          # 示例 / 模板 —— **不参与加载**
├── tools/             # 开发脚本(不随软件分发)
├── helix.d.ts         # 类型定义(编辑器补全;tsc 校验)
└── jsconfig.json      # `tsc --noEmit -p plugins/jsconfig.json` 用
```

四条可判定的规则:

1. 目录名 = 插件名 = `helix.plugin("<name>")` 的 name(kebab-case,发布后不改)
2. 入口文件**一律 `plugin.js`**(不再 `index.js` / `<name>.js`)
3. `deps` 写**插件名**(如 `["icons"]`),不写文件路径
4. 用户层覆盖是**目录级**:有就是全有,没有就全用内置 —— 避免"用户的 plugin.js + 内置的 lib/"这种混合体

细则与理由见 [`docs/plugin-layout.md`](../docs/plugin-layout.md) §3–§5。

## 2. 内置插件一览(13 个)

| 插件 | 提供 | 自带 `init.js` 自动加载? |
|---|---|---|
| `icons` | `[icons] nerd_font` 配置通道 + 6 个图标函数**兼容薄壳**(图标数据已收进核心 `helix.icons.*`,新代码零依赖直接用) | ✅ |
| `tutor` | `:tutor`(教程;打开随软件分发的内容,不绑定路径) | ✅ |
| `layout` | `:layout save\|load\|list\|delete <name>`(命名布局会话) | ✅ |
| `plugin` | `:plugin`(插件管理器的 JS 侧实现:list/install/update/remove/pin/status/reload) | ✅ |
| `pane-open` | 给 `helix.pane` 补统一创建入口 `open({place, content})` | ✅ |
| `dashboard` | `:dashboard` `:dashboard-close` `:dashboard-diag`(LazyVim 风格启动屏) | ✅ |
| `picker-ui` | `:pick*` 系列(JS 自绘 picker:`:pick` `:pick-close` `:pick-core` `:pick-core-draw` `:pick-default-draw` `:pick-diag`) | ✅ |
| `terminal` | `:term` `:vterm` `:hterm` `:term-list` `:term-close` `:term-save`(浮动/分屏终端) | ✅ |
| `statusline` | 状态栏三段分栏(`set_statusline`) | ✅ |
| `filetree` | `:filetree` `:filetree-reveal`(侧边文件树 rail 面板) | ✅ |
| `which-key` | 按键提示(`set_keymap_hint` 接管原生 Info,含中文说明) | ❌ 需自己加一行 |
| `arsenal` | `:arsenal`(工具市场窗,server 管理前端) | ❌ 需自己加一行 |
| `tabbar` | 布局标签条示范组件(`helix.TABBAR_ID` 槽位) | ❌ 需自己加一行 |

每个插件的细节(键位、设计取舍、已知天花板)写在**该插件 `plugin.js` 的文件头注释**里 —— 就近维护,不在本文件重复。

## 3. 启动时加载什么

`init.js` 的查找顺序(**用户优先**):

```
~/.config/helix/init.js  →  ~/.config/helix/plugins/init.js  →  <runtime>/plugins/init.js
```

内置 `init.js` 用 `safe_load` 把每次加载**逐个包住**:单个插件抛错只 `helix.echo` 一条状态栏消息,然后继续。

> 由来(实测事故):某条 `load` 抛错会让 `init.js` 整体失败 ⇒ 下面所有插件都不加载,
> 一次版本错配就变成"插件全灭"。Neovim 也不会因一个插件报错就让其余全不加载。

改完插件:

```
:plugin-reload          # 重读磁盘 + 重置插件状态(命令 = 一次撤销的边界)
```

## 4. 装 / 覆盖 / 分发

| 目的 | 做法 |
|---|---|
| 把本目录装成**内置层** | `sh contrib/install-plugins.sh [<runtime>]`(`DRY_RUN=1` 预览) |
| 同步到**用户层** | `plugins/tools/sync_runtime.sh`(只更新**已有**文件,不新建 —— 首次安装用上面那条) |
| 让某个插件**开机加载** | 在自己的 `~/.config/helix/init.js` 里加一行 `safe_load("<name>")` |
| 装**第三方**插件 | `:plugin install <path\|git-url>`(管理链/来源追踪见 [`docs/api/plugin.md`](../docs/api/plugin.md)) |
| 用户**覆盖**某个内置插件 | 在 `~/.config/helix/plugins/<name>/` 放自己的版本(整目录覆盖) |

## 5. 写一个插件(最小骨架)

```js
helix.plugin("my-plugin", { deps: [], version: "1.0" });   // name 与目录名一致;deps 用插件名

helix.register_command("my-cmd", (ctx) => {               // :my-cmd;第三个参数是提示里显示的说明
  ctx.doc.insert(0, 0, "hi");                             // 一个命令 = 一次撤销
}, "我的命令");

helix.export({ helper: () => {} });                        // 可选:供别的插件 helix.load 取用
```

- `ctx` 是**只读快照** + 编辑队列(越界会 clamp;快照坐标跨 `await` 会过期)——见 [`docs/api/editing.md`](../docs/api/editing.md)
- 依赖用**插件名**:`helix.plugin("x", { deps: ["icons"] })`。
  ⚠️ **依赖在本脚本 `eval` 之后才加载** ⇒ 顶层代码不能假定依赖已执行(要放到回调/命令里用)
- 类型:`plugins/helix.d.ts`。改完跑 `tsc --noEmit -p plugins/jsconfig.json`(见 §6)

## 6. 自检工具(`plugins/tools/`)

| 脚本 | 查什么 | 用法 |
|---|---|---|
| `check_api_surface.mjs` | **注册表 ↔ `helix.d.ts` 对账**(少写的 API / 不存在的名字 / 位置过时的旧名 / 由插件补的名字)+ `runtime/plugins` 与源**逐文件**一致 | **`cargo xtask api-check`**(门禁入口)/`node plugins/tools/check_api_surface.mjs` 等价 |
| `check_signatures.mjs` | 插件 JS 语法 + `icons` 6 个函数的参数名/函数体使用/导出一致性 | `node plugins/tools/check_signatures.mjs` |
| `sync_runtime.sh` | 把源的**已有副本**更新到最新:`.js` + `helix.d.ts` / `README.md` / `jsconfig.json`(**只更新,不新建**)| `bash plugins/tools/sync_runtime.sh` |

> 为什么要有这两个 `check_*`:它们都是**一次事故**固化下来的 —— 前者因为 `helix.d.ts` 是手工维护且曾**说谎**
> (声明了核心没注册的名字),后者因为一次机械替换改坏了函数参数名而事后没人能复跑。

## 7. 测试

| 层 | 命令 | 注意 |
|---|---|---|
| 纯逻辑(node) | `node --test plugins/<name>/tests/*.test.js` | **node ≥ 24 不接受目录参数**,必须指到文件或 glob |
| 集成(真编辑器) | `cargo test -p helix-term --features integration --test integration plugin` | **必须带 `--features integration`**,否则 `running 0 tests` 假绿 |
| 类型 | `tsc --noEmit -p plugins/jsconfig.json` | 与 cargo 无关,秒级 |

纯逻辑测试的做法:插件把纯函数放在文件底部 `module.exports`(engine 加载时 `module` 未定义会自动跳过),
node 侧注入最小 `global.helix` 桩即可直接 require —— 例子见 `plugins/arsenal/tests/arsenal.test.js`。

## 8. 改这个目录时的五个坑(都踩过)

1. **镜像漂移**:`plugins/` 是源,`<runtime>/plugins` 与 `~/.config/helix/plugins` 是**副本** ⇒
   改了源没同步 = "改了却不生效"。真实事故:`picker-ui` 的一处改动就是这么一直没生效的。
   → 用 `sync_runtime.sh` 同步,或让 `cargo xtask api-check` 替你查(它会逐文件比;脚本本体在
     `plugins/tools/check_api_surface.mjs`,xtask 只是它的门禁入口,CI 也跑这一条)。
2. **`deps` 的时序反直觉**:依赖在本脚本 `eval` **之后**才加载(见 §5)。
3. **每帧路径别引 JS**:实测 JS 自绘 picker ≈ **3.2 ms/帧/个可见 picker** = 60fps 预算的 **19.2%**
   ⇒ 核心渲染仍是默认路径;判定清单见 [`docs/plugin-vs-core.md`](../docs/plugin-vs-core.md)。
4. **改了 API 忘了改 `helix.d.ts`**:d.ts 是**手工维护**的 ⇒ 加/改/删 API 后跑 §6 的对账脚本。
5. **在加载时缓存 cwd**:cwd 是**可变**的(`:cd` / `:lcd` / `:pop-directory` 会真 chdir),
   而插件的模块状态活到进程结束。旧写法 `helix.run("pwd")` 只能在加载时问一次 —— 之后
   永远陈旧;它还需要 `process` 能力,只读插件根本拿不到自己的 cwd。
   → 用 `helix.cwd()`(**每次现取**,不收能力),需要**跟随**变更时监听 `cwd-change`
   (`ev.cwd` / `ev.prev_cwd`,发在 chdir **之后**)。
   真实事故:`filetree` 的 `S.root` 在 `:cd` 后永远停在启动目录(2026-10 `ed636875e` 修)。
   同理:任何**基于相对路径算好的值**(展开状态、监听目标、列表缓存)在加载时钉住都会慢慢说谎。

## 9. 已知不一致(待收)

- `plugins/plugin/plugin.js` 里写的是 `helix.plugin("plugin-manager")`,与目录名 `plugin` 不一致
  (约定要求二者一致,§1 规则 1)。实际生效的是**目录名**(`helix.load("plugin")`),name 目前只作元数据。
- 内置 `init.js` 只加载 10 个插件,**未加载 `which-key` / `arsenal` / `tabbar`**;
  而顶层 `README.md` 把 which-key 中文提示列为已实现特性 ⇒ 全新安装(直接用内置 init.js)看不到它,
  需要自己加 `safe_load("which-key")`(见 §4)。

## 10. 相关文档

| 文档 | 内容 |
|---|---|
| [`docs/plugin-api.md`](../docs/plugin-api.md) | 插件 API 总览与索引(**从这里进**) |
| [`docs/api/`](../docs/api/) | 按域详细:plugin · events · loading-commands · config · ui · editing · lsp · terminal-layout · picker-theme · process-fs |
| [`docs/plugin-layout.md`](../docs/plugin-layout.md) | 本目录的约定与迁移史(§3 约定 / §8 分发) |
| [`docs/plugin-vs-core.md`](../docs/plugin-vs-core.md) | 什么该插件化、什么该留核心(三闸门 + 四条红线) |
| [`docs/guide/`](../docs/guide/) | 教程 01–10(概览 → 启动/按键/编辑/渲染 → 深入 core/view/term/lsp) |
| [`docs/superpowers/`](../docs/superpowers/README.md) | 历史设计规格与交接记录(**不要当当前行为依据**) |
