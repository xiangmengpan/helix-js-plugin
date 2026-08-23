# 设计:插件管理器/市场(plugin manager)

日期:2026-08-15
状态:草案(待审核)——**建议低优先级**

## 1. 动机

插件安装靠手动复制文件到 `~/.config/helix/plugins/`。生态形态才需要管理器。

## 2. API(命令式)

```js
// 命令
:plugin-install <本地路径|git-url>   // 复制/clone 到 plugins 目录
:plugin-update <name|all>            // git pull / 重新复制
:plugin-remove <name>
:plugin-list                         // 列出已装插件 + 状态

// JS API(现有 plugin 系列扩展)
helix.plugin.install("https://github.com/user/repo.git")
helix.plugin.update("repo")
helix.plugin.remove("repo")
```

## 3. 实现

- **git 源**:clone 到 `~/.config/helix/plugins/vendor/<name>`(或直接 features/);update = git pull
- **本地路径**:复制文件/目录(现有 plugin-load 的扩展)
- **清单**:`plugins/manifest.json`(name/url/version)跟踪已装插件
- **reload 联动**:安装/更新后自动 `:plugin-reload`
- 依赖 git 命令或 libgit2(新增依赖)

## 4. 边界与风险

- **网络**:git clone 走异步(复用 run_async 模式);失败回滚
- **安全**:执行外部 git —— 信任边界(只装可信源)
- **依赖解析**:插件的 deps 清单安装(递归)——复杂度
- **回滚**:update 失败保留旧版本

## 5. 判断(诚实)

- 你是**唯一使用者/维护者**:管理器价值低(手动复制 7 个插件文件即可)
- 想**开放给他人**安装才需要
- 建议:**标记为生态形态**,核心功能(watcher/LSP/配置)优先;若做,先做本地路径 install + list(最小),git 源二期

## 6. 规模

- 最小(本地 install/list/remove + manifest):约 2 任务
- git 源 + update + 依赖解析:另加 2-3 任务
