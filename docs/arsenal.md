# Arsenal(`:arsenal`)——mason 式工具市场(浮层窗)

> 插件:`plugins/features/arsenal/index.js`(init.js 里
> `helix.load("features/arsenal/index.js")` 后可用,`:arsenal` 打开)。
> 数据与动作和 [server-manager.md](server-manager.md) 同一套注册表(内置 7 配方 +
> config 扩展 + 独立配方文件),后台执行走 `helix.server.task`(见
> [plugin-api.md](plugin-api.md))。

mason 式工具市场:自绘窗框浮层主窗(居中 78%×75%,英文界面),页签式分类
(LSP/DAP/Linter/Formatter…)、`/` 模态搜索、标记批量、版本输入、行内/底栏实时
进度;安装/升级/卸载动作直接落到 server-manager 的受管目录与 `languages.toml`
挂接,`:arsenal` 即点即装。UI 改版规格:docs/superpowers/specs/2026-09-07-arsenal-ui-v2-design.md。

## 用法

```js
// init.js —— 加载插件后 :arsenal 打开市场窗
helix.load("features/arsenal/index.js");
```

- `:arsenal` 打开市场窗(自绘边框+标题头+页签条的浮层主窗)。**窗尺寸固定**为
  终端约 78%×75%(设计规格里的 `+/-` 尺寸调整未实现——引擎侧 popup 无创建后
  调尺寸 API)。若全局 config 开了 `popup-border = "all"`,容器会在自绘框外再画
  一圈双框(JS 无法探测/关闭,介意可设 `popup-border = "none"`)。
- 行布局(按**显示列宽**计算,CJK/全角双宽不越界):名称列 + 描述列(伸缩,
  超长以 `…` 截断)+ 状态列(右锚贴框);顶框嵌标题与计数,页签条在标题下,
  底部为动作键位提示或批量进度条。
- 界面文案与行描述全英文;任务失败原因透传后端错误文案(可能含中文,非 arsenal 文案)。

## 键位表

| 键 | NORMAL 态(默认) | SEARCH 态(按 `/` 进入) |
|---|---|---|
| `j`/`k` | 上下移动选中 | **查询字符**(与其它可打印字符一样进过滤) |
| `↑`/`↓` | 上下移动选中 | 上下移动选中 |
| `/` | **进入搜索**(SEARCH) | 忽略(不输入字面 `/`) |
| 可打印字符 | 忽略(字母是命令,**不再即搜**) | 进过滤(匹配 名称/语言/描述,大小写不敏感) |
| `Backspace` | – | 删末位过滤字符 |
| `Tab` / `Shift+Tab` | 页签循环:`all → lsp → dap → linter → formatter → installed → local`(双向) | 同左(查询保留) |
| `Enter` | 无标记:选中行动作(唯一动作直达,多动作开菜单);有标记:批量 install | 同左 |
| `u` | 受管已装行直接升级(needs_version 恒弹版本输入) | 查询字符 |
| `x` | 卸载直达:受管 `remove`;本机已有 `unmanage`(停用/恢复挂接 toggle) | 查询字符 |
| `i` | 信息弹窗(状态/语言/描述/主页/命令路径/下载源/版本需求);动作菜单里按 `i` 直达 | 查询字符 |
| `t` | 标记/取消标记选中行(供 Enter 批量) | 查询字符 |
| `r` | 刷新行列表 | 查询字符 |
| `C`(Shift+c) | 清版本记忆:选中行 needs_version 且有记忆 → 清该行;否则清全部(换版本逃生) | 查询字符 |
| `Esc` | 关闭主窗 | **清查询回 NORMAL**(再按 Esc 才关窗) |
| `q` | 关闭主窗 | 查询字符 |

> 设计动机:`/` 显式开启输入(避免输入名字字母误触 x/u/f/t 等命令),SEARCH 内
> 一切可打印字符都只是过滤词,移动用 `↑`/`↓`;搜 `jdtls` 这类以 j 开头的名字
> 必须先 `/` 再输入。

## 动作菜单

`Enter` 在未装且可装行 = 直接 `install`(单项直达);受管已装行打开
**update/remove 菜单**(`↑↓`/`j k` 选择,Enter 执行);本机已有行单项 `unmanage`
直达。菜单内 `i` 可先看该行信息再决定。Esc/q 关菜单回主窗。

## 版本输入(needs_version)

需显式版本的前提:配方下载源 url 含 `{version}` 占位**且配方未固定 version**
(内置 `rust-analyzer` 是唯一活例子;config 扩展解析已强制 `{version}` 必须配
`version` → 恒不需要)。

- **install** 无该行版本记忆 → 弹窗取版本,Enter 提交并**记忆**(批量复用免重输)。
- **update 恒弹版本输入**:记忆只在弹窗里预填供参照/修改,**绝不跳过输入直发**——
  升级到新版本永远可达(换版入口不被记忆锁死)。
- 弹窗内键位:字符输入 · `Enter` 提交 · `Backspace` 删 · **`C` 清该行记忆**
  (清记忆+清空预填)· `Esc` 取消(空版本 Enter 也视同取消)。
- 主窗 `Shift+C`:清选中行该配方的记忆,选中行无记忆则清全部版本记忆
  (批量换版本前先清,避免旧记忆被复用直发)。

## 批量安装

`t` 标记多行(未装 + 有下载源),`Enter` 批量 `install`:needs_version 行用各自的
版本记忆(该行没输过版本会因服务端"配方未给 version"失败,汇总到完成回显)。
提交即清标记,防二次 Enter 重复投。批量进度见下。

## 行字段与状态列

行对象(`helix.server.rows` 回传)除名字/语言外含:
`installed local version installable upgradable needs_version bin source
description homepage`。

- **状态列**:受管 `✓ 版本` / 可升级 `▲ 版本` / 本机 `local 版本` / 未装 `–` /
  无下载源 `no source`;任务运行中显示该行 `pct% 阶段`。
- `bin`(命令路径)与 `source`(下载源)在 `i` 信息弹窗展示。
- `description`/`homepage` 显示在行描述列与信息弹窗。内置 7 配方与独立配方文件
  (contrib)均带英文描述 + 官方主页;config 扩展配方可选配置:

```toml
[server-manager.registry.my-ls]
url = "…"
# …其余字段见 server-manager.md config 段……
description = "My language server"
homepage = "https://example.com/my-ls"
```

## 后台任务与进度

`helix.server.task(items, cb)` 入队即回:worker 串行执行 install/update,事件
(`phase` downloading/writing config、`progress` bytes/total、`done`/`error`)经
JS 泵回 → **行内进度**(受管下载行显示 `pct%`)+ **底栏进度条**
(`▮▮▮▯▯▯▯ 30% (1/2) op name`,含失败名单)。**事件推送即唤醒主循环重绘**
(不等按键/下一次 idle):提交后无需按键,进度/done 自动收敛上屏。echo 约定:
单项成功只在完成时报服务端 msg(不重复汇总),**失败与多于 1 项的批量**才在末尾
给 "batch done N/M" 汇总(英文)。主窗关闭不取消进行中的任务(进度不再渲染,
完成/失败的 echo 仍会出现在状态栏)。

**并发写互斥**:后台任务(worker 批次)与 `:server install/update/remove/unmanage`、
`:server panel` Enter 共用一把写锁(`server_manager::OP_LOCK`),任意两个写流不会
交错(整 op 含 languages.toml 重写);后台下载中主线程写命令会阻塞到其结束——
**后台任务进行中建议不用 `:server` 写命令**(锁只防坏不防等,见 server-manager.md)。

## 与 `:server` / `:server panel` / `:server-manager` 关系

- `:server` 命令族(install/update/remove/unmanage/status):命令行界面,同一套
  注册表与受管目录。
- `:server panel`:原生 picker 面板(输入即过滤,Enter=install/update),无分类/
  标记/批量/版本输入/进度。
- arsenal(本插件):浮层市场窗——页签分类、`/` 搜索、标记批量、版本输入、行/批量
  进度、信息弹窗一应俱全,取代 `:server-manager` rail 面板(已 deprecated,见
  `plugins/features/server-manager/index.js` 头注释)。

## 配方从哪来(50+ 语言)

arsenal 行 = 内置 registry(7 条)+ config.toml `[server-manager.registry.*]` + 独立配方文件
(`~/.config/helix/server-manager-recipes.toml`,复制自仓库 `contrib/server-manager-recipes.toml`),
同名后源覆盖前源。仓库文件覆盖 50+ 语言:包管理器 Tool(npm/pip3/gem/dotnet)一键可装,
`url=""` 占位条目显示 no source 并按 description 引导。改配方文件即生效(每次操作前重读),零重编译。
