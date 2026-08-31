# 设计:插件管理器二期批次 2——update + pin

日期:2026-08-31
状态:已批准(方向:git CLI + pin 锁定跳过)

## 背景与目标

批次 1 已有 git 源 install(manifest 记 commit)。本批:`plugin update [name|all]` 更新 git 源插件(fetch + 对比 + pull,失败保留旧版)+ `plugin pin/unpin`(锁定跳过更新)。

## 方案

### 1. `plugin update [name|all]`(默认 all)

逐条目处理(manifest 顺序):

- **local 源**(kind != "git")→ 跳过 + 汇总提示("skipped N local")
- **pinned=true** → 跳过 + 汇总提示("skipped N pinned")
- **git 源**:
  1. `git -C <dir> fetch origin`(失败 → 报错,该条目不更新,继续其他)
  2. 对比:`git -C <dir> rev-parse HEAD` vs `git -C <dir> rev-parse origin/HEAD`
  3. 相同 → "up to date";不同 → `git -C <dir> pull --ff-only`(快进合并;冲突/非快进 → 报错,工作树保留旧版)
  4. 成功 → 更新 manifest commit 为新 HEAD
- 结果汇总状态栏:"updated A, up to date B, skipped C, failed D"

### 2. `plugin pin <name>` / `plugin unpin <name>`

- 切换 manifest 条目的 `pinned` 字段(true/false)
- 仅 git 源可 pin(local 报错 "only git plugins can be pinned")
- 未安装 → 报错

### 3. status 增强

- `plugin status` 输出带 pinned 标记:`N loaded, M installed (K git, L pinned)`

### 4. 边界

- pull 失败/冲突:git pull 失败不改工作树(旧版保留),manifest commit 不更新——天然回滚
- 目录被删除/损坏(vendor/<name> 不存在):update 报错 + 提示重装
- update all 中途一个失败:继续其他条目,失败汇总
- 网络失败:fetch 报错,条目跳过

## 测试策略

- **单测**:pinned 跳过逻辑、local 跳过逻辑、commit 对比(用本地 git 仓库:init + commit + fetch 模拟?——真实 git 操作可做:临时目录 init 裸仓库 + clone + 新 commit + update,验证 commit 更新。若太重则纯逻辑单测 + 手动)
- **integration**:无副作用路径(pin 未安装报错、local pin 报错)

## 关键实现位置

- helix-term/src/commands/plugin_manager.rs(git_fetch/git_head_compare/git_pull_ff 纯函数;update_plugin 逻辑)
- helix-term/src/commands/typed.rs(plugin() 加 update/pin/unpin 分支)
- helix-term/tests/test/plugin_manager.rs
