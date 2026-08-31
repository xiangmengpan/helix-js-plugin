# 设计:插件管理器二期批次 1——git 源 install

日期:2026-08-30
状态:已批准(决策:git CLI + plugin.json 依赖声明 + pin 锁定)

## 背景与目标

一期只支持本地路径 install。本批:`plugin install <git-url>` 支持 git 仓库源——clone 到 `plugins/vendor/<name>`,manifest 记录 url + commit hash。update/pin/依赖解析/JS API 后续批次。

## 方案

### 1. 源识别(install 参数判定)

`plugin install <arg>`:
- arg 是**已存在路径** → 本地源(一期行为,不变)
- 否则按 **git-url** 处理:含 `://` 或 `git@` 或 `.git` 后缀 → clone;无法识别 → 报错 "invalid path or git url"

### 2. clone(git CLI + 异步)

- 命令:`git clone <url> <plugins>/vendor/<name>`
- 执行:复用现有异步模式(如 run_async 的 spawn worker / std::process::Command 阻塞?)**决策:同步阻塞但 git clone 小仓库通常 <1s;大仓库 UI 冻结可接受(一期 install 也是同步 fs 操作)**——用 std::process::Command 同步,失败报错不写 manifest
- 目标目录:`~/.config/helix/plugins/vendor/<name>`(vendor/ 与 features/ 分开:git 源 vs 本地源)
- name 提取:url 的 basename 去 `.git` 后缀(`https://github.com/foo/bar.git` → `bar`)
- 目标已存在 → 报错 "already installed"

### 3. manifest 扩展

```json
{
  "bar": {
    "source": "https://github.com/foo/bar.git",
    "kind": "git",
    "installed_at": "1725014400",
    "commit": "a1b2c3d...",        // clone 后 git rev-parse HEAD
    "pinned": false,               // 批次 2 用
    "files": ["vendor/bar/"]       // 复制目标(整个 vendor 目录)
  }
}
```

- commit 获取:`git -C <dir> rev-parse HEAD`(失败则 commit 记空串 + warn)
- 一期 entry 无 commit/pinned 字段 → serde 缺省(#[serde(default)])

### 4. 加载

- 入口约定:仓库根 `index.js` 存在 → install 后自动加载(`load_script_named("vendor/<name>/index.js", ...)`,照一期装后加载逻辑)
- 无 index.js → 跳过加载(可能是纯 lib/主题)
- init.js 手动 load 用相对路径 `helix.load("vendor/bar/index.js")`(与 features 同款机制,load 相对 plugins/ 目录)

### 5. 边界

- 网络失败/clone 失败 → 报错,不写 manifest(清理已 clone 的目录)
- 非法 git-url 字符串 → 报错
- remove:git 源走一期 remove 的 files 删除(vendor/<name>/ 目录);无 manifest 孤儿路径同前
- 二期后续:update(git pull + hash 对比 + pinned 跳过)、依赖解析(plugin.json)、JS API

## 测试策略

- **单测**:url → name 提取(basename/.git 后缀/https/git@);路径 vs git-url 识别;clone 失败不写 manifest(用假 url 注入失败)
- **integration**:无副作用路径(路径识别报错、非法 url 报错);真实 clone 需要网络——**不做**,靠手动验证

## 关键实现位置

- helix-term/src/commands/plugin_manager.rs(parse_source、clone_to_vendor、name_from_url、manifest 扩展)
- helix-term/src/commands/typed.rs(plugin install 分支:识别 → clone → manifest → 加载)
