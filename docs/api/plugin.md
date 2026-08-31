# API:插件管理

> 详细参考。总览见 [`docs/plugin-api.md`](../plugin-api.md)。

插件系统自带完整管理链(manifest 来源追踪 + 本地/git 源 + 更新/pin + 依赖解析 + JS API)。

## 命令面(`plugin <op>` 统一入口)

```
:plugin install <path|git-url>   # 本地路径复制到 features/;git-url clone 到 vendor/
:plugin update [name|all]        # git pull --ff-only;失败保留旧版;pinned/local 跳过
:plugin pin <name>               # 锁定(update 跳过);仅 git 源
:plugin unpin <name>
:plugin remove <name>            # 删 manifest 条目 + 文件(兼容无 manifest 旧安装/孤儿)
:plugin list                     # 已加载脚本
:plugin status                   # loaded/installed/git/pinned 计数
:plugin reload                   # 热重载全部
:plugin-load <path>              # 手动加载任意文件(历史命令)
:plugin-reload                   # :plugin reload 别名(历史命令)
```

## 存储与来源追踪

- `~/.config/helix/plugins/manifest.json`:`{ name: { source, kind: "local"|"git", installed_at, commit, pinned, files } }`
- 本地源 → `features/<name>`;git 源 → `vendor/<name>`;install 后自动 reload + 加载入口(index.js)
- 坏/缺 manifest → 视为空,不崩

## 依赖解析

- git 仓库根 `plugin.json`:`{ "deps": [{ "name": "dep", "git": "url" }] }`(可选字段)
- install 时递归安装依赖(先依赖后本体);循环检测 + 10 层深度限制;已装跳过;任一失败中止报错

## JS API

```js
helix.plugin("name", { deps: ["lib/icons.js"] });   // 声明加载时依赖(文件 key,拓扑加载)
helix.plugin.install("./path.js");                  // 或 git-url;装 + manifest + 依赖 + reload
helix.plugin.update();                              // 全部更新(可传 name 指定单个)
helix.plugin.remove("name");                        // 删 manifest 条目 + 文件
```

- 结果经状态栏显示(fire-and-forget);错误经 set_error;install/remove 缺参数 → 调用即报错。

## 优缺点

- **优点**:来源追踪(manifest)、卸载/更新闭环、依赖递归自动装、循环防护、装后即用(自动加载);命令面 + JS API 双入口。
- **局限**:JS API 无 Promise 返回值(结果只看状态栏);真实 git 操作同步阻塞(UI 短暂冻结);依赖装完不自动加载(本体 index.js 才加载);`./x.git` 本地路径误判为 git 源(edge case);版本 pin 只做锁定不记录期望版本。
- **边界**:路径校验防逃逸;remove 的容器目录名(`.`/`features`)拒绝;同名已装先 remove 再装。
