# Arsenal(`:arsenal`)——mason 式工具市场(浮层窗)

> 插件:`plugins/features/arsenal/index.js`(init.js 里
> `helix.load("features/arsenal/index.js")` 后可用,`:arsenal` 打开)。
> 数据与动作和 [server-manager.md](server-manager.md) 同一套注册表(内置 7 配方 +
> config 扩展),后台执行走 `helix.server.task`(见 [plugin-api.md](plugin-api.md))。

mason 式工具市场:浮层主窗(居中 78%×75%)列出全部配方行,支持**即搜 + 分类 +
标记批量 + 版本输入 + 行内/底栏实时进度**;安装/升级/卸载动作直接落到
server-manager 的受管目录与 `languages.toml` 挂接,`:arsenal` 即点即装。

## 用法

```js
// init.js —— 加载插件后 :arsenal 打开市场窗
helix.load("features/arsenal/index.js");
```

- `:arsenal` 打开市场窗(浮层主窗居中;**Esc 先清过滤再关、q 直接关**;子弹窗
  Esc 只关自己回主窗)。
- 行布局:名称列 + 描述列(伸缩)+ 状态列;顶部标题 `arsenal [过滤] · 分类 ·
  N/M servers`;底部为动作键位提示或批量进度条。

## 键位表

| 键 | 动作 |
|---|---|
| `j`/`k` 或 `↑`/`↓` | 上下移动选中 |
| 可打印字符 | 即搜(匹配 名称/语言/描述,大小写不敏感) |
| `Backspace` | 删末位过滤字符 |
| `f` | 分类循环:`all → lsp → dap → linter → formatter → installed → local` |
| `t` | 标记/取消标记选中行(供 Enter 批量) |
| `Enter` | 无标记:选中行动作(唯一动作直达,update/remove 等多动作开菜单);有标记:批量 install |
| `u` | 受管已装行直接升级(needs_version 恒弹版本输入) |
| `x` | 卸载直达:受管 `remove`;本机已有 `unmanage`(停用/恢复挂接 toggle) |
| `i` | 信息弹窗(状态/语言/描述/主页/命令路径/下载源/版本需求);**动作菜单里按 `i` 直达该行信息**(菜单保留) |
| `C`(Shift+c) | 清版本记忆:选中行 needs_version 且有记忆 → 清该行;否则清全部(换版本逃生) |
| `Esc` | 先清过滤再关(子弹窗:关自己回主窗) |
| `q` | 关闭主窗 |

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

- **状态列**:受管 `✓ 版本` / 可升级 `▲ 版本` / 本机 `本机 版本` / 未装 `–` /
  无下载源 `no source`;任务运行中显示该行 `pct% 阶段`。
- `bin`(命令路径)与 `source`(下载源)在 `i` 信息弹窗展示。
- `description`/`homepage` 显示在行描述列与信息弹窗。内置 7 配方已带中文描述 +
  官方主页(M5);config 扩展配方可选配置:

```toml
[server-manager.registry.my-ls]
url = "…"
# …其余字段见 server-manager.md config 段……
description = "我的语言服务器"
homepage = "https://example.com/my-ls"
```

## 后台任务与进度

`helix.server.task(items, cb)` 入队即回:worker 串行执行 install/update,事件
(`phase` 下载中/写入配置、`progress` bytes/total、`done`/`error`)经 JS 泵回 →
**行内进度**(受管下载行显示 `pct%`)+ **底栏进度条**(`▮▮▮▯▯▯▯ 30% (1/2) 行名 op`,
含失败名单)。echo 约定:单项成功只在完成时报服务端 msg(不重复汇总),**失败与多于 1 项的批量**才在末尾给"批量完成 N/M"汇总。主窗关闭不取消进行中的任务(进度不再
渲染,完成/失败的 echo 仍会出现在状态栏)。

## 与 `:server` / `:server panel` / `:server-manager` 关系

- `:server` 命令族(install/update/remove/unmanage/status):命令行界面,同一套
  注册表与受管目录。
- `:server panel`:原生 picker 面板(输入即过滤,Enter=install/update),无分类/
  标记/批量/版本输入/进度。
- arsenal(本插件):浮层市场窗——搜索、分类、标记批量、版本输入、行/批量进度、
  信息弹窗一应俱全,取代 `:server-manager` rail 面板(已 deprecated,见
  `plugins/features/server-manager/index.js` 头注释)。
