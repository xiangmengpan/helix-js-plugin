# 设计:插件管理器最小版(本地 install/list/remove + manifest)

日期:2026-08-30
状态:已批准(分节讨论确认)

## 背景与目标

插件安装靠手动复制到 `~/.config/helix/plugins/`,无来源追踪、无移除机制。目标(最小版):`manifest.json` 追踪已装插件 + 三个命令(install/list/remove),本地路径源。git 源/update/依赖解析/版本 pin 二期。

## 节 1:manifest

路径:`~/.config/helix/plugins/manifest.json`(helix_loader::config_dir 同法)

```json
{
  "filetree": {
    "source": "./my-plugins/filetree",   // 安装时传入的路径(相对/绝对原样记)
    "type": "local",
    "installed_at": "2026-08-30T12:00:00Z",
    "files": ["features/filetree/index.js", "features/filetree/"]  // 复制目标(相对 plugins/)
  }
}
```

- 坏/缺 manifest → 视为空清单,命令不崩(读失败 warn + 空)
- 写失败 → 报错(install/remove 不静默)

## 节 2:命令(typed.rs,照 plugin-load 注册)

### `:plugin-install <path>`

- path 可以是单文件(`.js`)或目录(含 index.js 的插件目录)
- 复制目标 `~/.config/helix/plugins/features/`:
  - 文件 → `features/<filename>`
  - 目录 → `features/<dirname>/`(递归复制)
- 目标已存在 → 提示覆盖(再确认后覆盖;最小版:直接报错"already installed, use :plugin-remove first"更安全——**决策:同名已装则报错,先 remove 再装**)
- 成功:写 manifest + 自动 `:plugin-reload`
- 失败:不写 manifest,报错(复制中途失败 → 清理已复制的部分,回滚)

### `:plugin-list`

- 无 manifest 条目 → "No plugins installed"提示
- 输出格式:状态栏多行或 echo 拼接——`name  source  type  installed_at`(与现有插件列表输出风格一致;若 :plugin-list 已有(列出已加载脚本),新命令叫 `:plugin-manifest-list`?**决策:新命令名 `:plugin-installed` 避免与现有 :plugin-list 冲突**)

### `:plugin-remove <name>`

- manifest 无此条目 → 报错 "not installed"
- 删 manifest 条目 + 删 files 列表对应的文件/目录(逐个,忽略不存在的)
- 删除失败 → 报错(manifest 已删则记录该偏差)
- 提示用户修改 init.js 中对应 load(不自动 reload)

## 节 3:联动与边界

- install 后自动 `:plugin-reload`(装完即用)
- remove 后不自动 reload(init.js 还 load 着会报错,提示用户改)
- 不写 init.js(保持显式)
- 同名已装:install 报错(先 remove)

## 节 4:不做(明确排除)

- git 源/update/依赖自动解析/版本 pin/JS API(helix.plugin.install 等)——二期
- lib/ 共享层安装(自装插件只用 features/;lib 依赖手动)

## 测试策略

- **单测**:manifest 读写(序列化/容错坏 JSON/缺文件);install 的复制逻辑(文件/目录/回滚);remove 的文件删除
- **integration**:`:plugin-install` 本地文件 → manifest 条目 + 文件存在 + reload 生效;`:plugin-installed` 输出;`:plugin-remove` 删条目删文件;同名重复安装报错

## 关键实现位置

- helix-term/src/commands/typed.rs(三个命令 + 注册,照 plugin-load)
- helix-term/src/plugins/manager.rs 或 typed.rs 内(manifest 读写 + 复制/删除逻辑,纯函数可单测)
- helix-term/tests/test/plugin_manager.rs(integration)
