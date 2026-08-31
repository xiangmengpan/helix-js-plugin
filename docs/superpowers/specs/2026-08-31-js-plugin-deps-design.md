# 设计:插件管理器二期批次 3——依赖解析(plugin.json)

日期:2026-08-31
状态:已批准(方向:plugin.json 声明 deps,install 时递归安装)

## 背景与目标

批次 1/2 已有 git 源 install + update + pin。本批:`plugin install` 时读取插件的 `plugin.json` 依赖声明,递归安装依赖(git 源),循环检测,失败中止。

## 方案

### 1. 声明格式(仓库根 `plugin.json`)

```json
{
  "deps": [
    { "name": "dep-a", "git": "https://github.com/foo/dep-a.git" },
    { "name": "dep-b", "git": "git@github.com:foo/dep-b.git" }
  ]
}
```

- 可选字段 `deps`;缺省 = 无依赖
- 仅支持 git 源依赖(本地依赖无意义——install 是路径,依赖声明 git 即可)
- 坏 JSON/字段缺失 → 报错(warn 不中止?**决策:坏 plugin.json → 报错中止 install**,避免装了个依赖声明损坏的插件;无 plugin.json 文件 = 无依赖,正常)

### 2. 触发时机(install 流程内)

- git 源 install:clone 完成后读 `vendor/<name>/plugin.json`
- 本地源 install:复制完成后读 `features/<name>/plugin.json`(源目录的 plugin.json 也随复制进去了)
- 依赖处理顺序:**先装依赖,再装本体**(依赖在前,本体最后)
- 递归:依赖的依赖也处理

### 3. 递归与循环

- 已装(manifest 含 name)→ 跳过(不重装)
- 循环检测:递归时维护已访问栈;A 的依赖链再次出现 A → 报错 "circular dependency: A -> ... -> A",中止
- 深度限制(防御):超过 10 层 → 报错中止

### 4. 失败语义

- 任一依赖安装失败(clone 失败/plugin.json 坏/循环)→ 中止,报错;已装的依赖保留(不回滚依赖,只中止后续)
- 本体不装(依赖失败时本体还没开始或中止)

### 5. 复用

- 依赖安装复用现有 install 的 git 分支逻辑——提取内部函数 `install_git_plugin(name, url, manifest, plugins_dir) -> Result<()>`,install 命令与依赖递归共用

## 测试策略

- **单测**:plugin.json 解析(deps 提取/坏 JSON/缺字段);递归顺序(依赖先装);循环检测(A→B→A);已装跳过
- **integration**:无副作用路径(循环检测可用本地 git 仓库模拟?——真实 git 仓库构造:临时目录 init 两个仓库互依赖,install 验证循环报错。若太重则单测覆盖 + 手动)

## 关键实现位置

- helix-term/src/commands/plugin_manager.rs(parse_plugin_json、install_with_deps 递归、循环检测)
- helix-term/src/commands/typed.rs(install 分支调用 install_with_deps)
- helix-term/tests/test/plugin_manager.rs
