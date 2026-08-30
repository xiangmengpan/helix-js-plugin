# 设计:插件管理器最小版(增强 :plugin 子命令 + manifest)

日期:2026-08-30
状态:已批准(修正版——现有 `:plugin` 子命令已含 install/remove/list 雏形,改为增强而非新建命令)

## 背景与目标

现有 `:plugin` 命令(typed.rs:5339)已有 `list/install/remove/reload/status` 子命令,但 install 只支持**单文件 copy**、remove 直接删文件、无来源追踪、无回滚、无同名检查。目标(最小版):`manifest.json` 追踪已装插件,增强现有 `install`/`remove` 子命令,`list`/`status` 语义保持。

## 节 1:manifest

路径:`~/.config/helix/plugins/manifest.json`(helix_loader::config_dir 同法)

```json
{
  "filetree": {
    "source": "./my-plugins/filetree",   // 安装时传入的路径(相对/绝对原样记)
    "kind": "local",
    "installed_at": "1725014400",        // epoch 秒(std::time,无 chrono 依赖)
    "files": ["features/filetree/index.js", "features/filetree/"]  // 复制目标(相对 plugins/)
  }
}
```

- 坏/缺 manifest → 视为空清单,命令不崩(读失败 warn + 空)
- 写失败 → 报错(install/remove 不静默)

## 节 2:增强现有 `:plugin` 子命令

### `plugin install <path>`(现有 5351,增强)

- path 可以是单文件(`.js`)或目录(含 index.js 的插件目录)
- 复制目标 `~/.config/helix/plugins/features/`:
  - 文件 → `features/<filename>`
  - 目录 → `features/<dirname>/`(递归复制)
- **同名已装**(manifest 或目标存在)→ 报错 "already installed, use :plugin remove first"(先 remove 再装)
- 成功:写 manifest + 自动 reload(现有 install 已加载,改为统一 reload_plugins)
- 失败:不写 manifest,报错(复制中途失败 → 清理已复制的部分,回滚)

### `plugin remove <name>`(现有 5367,增强)

- manifest 无此条目但文件存在(旧版装的)→ 删文件 + 提示(兼容无 manifest 的旧安装)
- 有条目 → 删 manifest 条目 + 删 files 列表对应文件/目录(逐个,忽略不存在的)
- 删除失败 → 报错(manifest 已删则记录该偏差)
- 提示用户修改 init.js 中对应 load(不自动 reload)

### `plugin list` / `status`(保持)

- list 列出已加载脚本(语义正确,不动)
- status 加后缀:`N plugins loaded from <dir>, M installed in manifest`(manifest 数量并入 status,不新建命令)

## 节 3:联动与边界

- install 后自动 reload(装完即用,复用 reload_plugins)
- remove 后不自动 reload(init.js 还 load 着会报错,提示用户改)
- 不写 init.js(保持显式)
- 同名已装:install 报错(先 remove)

## 节 4:不做(明确排除)

- git 源/update/依赖自动解析/版本 pin/JS API(helix.plugin.install 等)——二期
- lib/ 共享层安装(自装插件只用 features/;lib 依赖手动)
- 新建独立命令(:plugin-install/-installed/-remove 不建,全走现有 :plugin 子命令)

## 测试策略

- **单测**:manifest 读写(序列化/容错坏 JSON/缺文件);install 的复制逻辑(文件/目录/回滚);remove 的文件删除
- **integration**:`plugin install` 本地文件 → manifest 条目 + 文件存在;`plugin list` 输出;`plugin remove` 删条目删文件;同名重复安装报错(注意 config_dir 是真实 ~/.config——写入类测试评估污染风险,必要时只测无副作用路径,写入逻辑靠单测)

## 关键实现位置

- helix-term/src/commands/typed.rs(plugin 子命令 install/remove 分支增强 + status 后缀;现有 plugin() 函数 5339)
- helix-term/src/commands/plugin_manager.rs(新建:manifest 读写 + 复制/删除逻辑,纯函数可单测)
- helix-term/tests/test/plugin_manager.rs(integration)
