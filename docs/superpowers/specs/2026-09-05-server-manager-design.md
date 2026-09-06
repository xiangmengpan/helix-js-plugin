# Server Manager（mason 式 LSP/DAP/工具管理器）设计规格

日期:2026-09-05
状态:待审查
关联:插件管理器(`:plugin install/update/remove`,git 源+依赖解析)为同构先例

## 0. 决策记录(讨论结论)

- 目标:类 **mason.nvim** 的服务器管理器——统一注册表 + 安装/升级/卸载 LSP server、DAP adapter、linter、formatter;grammar 由 helix 原生 `:grammar` 管,不在本系统范围。
- **镜像**:下载层统一 URL 改写(`[server-manager] mirror` 全局规则 + 配方级覆盖),配方只写原版 URL。
- **配置**:自动写 `~/.config/helix/languages.toml`,用标记段(`# >>> helix-managed` … `# <<< helix-managed`)包裹受管内容,幂等合并;用户手写内容保留。
- **分层**:主体在 Rust(term 层,同插件管理器),JS 插件只做 UI 面板(filetree rail 骨架)。
- v1 配方集 ~8 个跑通全链路(LSP:rust-analyzer/pyright/tsserver/gopls/clangd;DAP:debugpy/lldb-dap;fmt/lint:black/prettier),注册表后续增量。

## 1. 目标

`helix :server` 统一管理开发服务器/工具:搜索、安装(带校验与镜像支持)、升级、卸载,并把受管工具自动接入 `languages.toml`(LSP `language-servers`、DAP `debugger` 模板)与 PATH(受管 bin 目录),开箱即用。

## 2. 现状与约束(已核实)

- LSP server 经 `languages.toml` 的 `[language-server.<name>] command=…` 配置,并被某 `[[language]] language-servers=[…]` 引用后才会启动;二进制默认期望在 PATH。
- DAP adapter 配置 = **语言级** `[[language]] debugger`(language_config().debugger,含 adapter 命令与 templates 列表)——与 LSP 同文件。
- 用户当前无 `languages.toml`(只有 config.toml);管理器将创建之,并预留标记段支持以后手写混排。
- 镜像/校验:下载层统一做 sha256 校验;网络差异(国内)经全局 mirror 规则吸收。

## 3. 目标架构

### 3.1 注册表与配方

- 内置配方表(代码常量,`mod server_manager::registry`),字段:
  ```
  ServerSpec {
    name,           // 唯一 id(命令/目录名)
    kind,           // Lsp | Dap | Linter | Formatter
    bin,            // 解压/安装后实际可执行名(可多个:如 tsserver→node)
    languages: &[&str],  // 适用语言 id(挂 language-servers/debugger 用)
    version_check,  // "<bin> --version" 类检测
    install: InstallSpec,
  }
  enum InstallSpec {
    Archive { url_template, asset_glob?, sha256, strip_components, mirror_ok },
    Tool { cmd, args_prefix },  // pip/cargo/npm 生态:受管目录内执行
  }
  ```
- 用户扩展:config.toml `[server-manager.registry.<name>]` 可覆写/新增配方(v1 支持新增 archive 类)。

### 3.2 受管目录与 PATH

- 根目录 `~/.local/share/helix/managed/`:
  - `<name>/` 安装产物;`bin/<name>`(或配方的 bin 名)可执行软链/launcher
- `managed/bin` 加入 PATH 由**外部 shell 配置**负责(安装时提示);编辑器内**不依赖 PATH**:languages.toml 一律写**绝对路径**,保证配置即用。

### 3.3 安装/升级/卸载

- **Archive**:按 url_template(经 mirror 改写)→ 下载 → sha256 校验 → 解压(可选 strip)→ 校验 bin 存在 → 软链到 managed/bin。
- **Tool**:在 managed/<name> 前缀下执行(cargo `--root`/pip `--target`/npm `--prefix` 的等价)→ 记录真实 bin 路径 → launcher 软链。
- **升级**:先 `<bin> --version` 得当前版本;重取安装;失败回滚(保留旧目录,新目录临时名+rename)。
- **卸载**:删除 managed/<name>、bin 软链;从 languages.toml 标记段移除其条目。
- 安全:下载 sha256 必填;workspace trust 之外,安装属用户显式命令,不自动装。

### 3.4 languages.toml 自动写入(标记段)

文件 `~/.config/helix/languages.toml`,结构:
```
# >>> helix-managed
[language-server.rust-analyzer]
command = "/home/.../managed/rust-analyzer"
# 该语言条目仅在用户未自定义同名语言时生成;已存在则仅合并 language-servers 缺名
[[language]]
name = "rust"
language-servers = ["rust-analyzer"]
# (DAP 适配器)同语言条目补 debugger = { ...command... templates=[...] }
# <<< helix-managed
```
- **幂等**:安装/升级/卸载都重写整个标记段(从当前文件剥离标记段→按已装清单重建→写回);标记外用户内容原样保留。
- **合并规则**:某 `[[language]] name=X` 用户已在标记外自定义 → 标记内**不再**生成同名 language 条目,避免冲突;server 的生效靠用户在自定义条目中引 `language-servers` 补名?——不,此时在用户条目上无法安全改,方案:标记段仍生成 `[[language]] name=X` 最小条目,**helix 合并语义下后者会覆盖同名**——冲突。→ **决策**:语言条目只在"标记内 + 用户未自定义该语言"时生成;若用户自定义了该语言,`server install` 完成后在状态栏提示"手动在你的 `[[language]] name=X` 加 language-servers"并给可复制片段。幂等优先,不猜用户配置。

### 3.5 命令与 UI

- typed 命令族(签名/补全对齐插件管理器):
  `:server list [kind?]` / `:server search <kw>` / `:server install <name>` / `:server update [name]` / `:server remove <name>` / `:server status`
- JS API 镜像(UI 用):`helix.server.list/install/update/remove` → UiRequest::ServerOp(与 PluginOp 同构)。
- UI 插件(可选交付):picker/rail 面板(filetree 骨架):搜索过滤、状态列(未装/已装 vX/可升级)、Enter=安装、u=升级、x=移除;`C-w` 聚焦。
- health 联动:语言服务器缺失的提示后附 `:server install <name>` 建议(v1 在 health.rs 增 hint;不做自动)。

### 3.6 配置

`config.toml`:
```
[server-manager]
dir = "~/.local/share/helix/managed"   # 可改
mirror = ""                             # 例 https://ghproxy.com/ ;对 archive 下载做前缀/改写
[server-manager.registry.<name>]        # 用户扩展配方(v1: archive)
```

## 4. 边界与错误处理

| 场景 | 行为 |
|---|---|
| 下载 404 / sha256 不符 | 中止,报错;不留半成品(临时目录清理) |
| 升级失败 | 保留旧版,报错;不写 languages.toml 变更 |
| mirror 为空且 URL 不可达 | 报错提示配置 mirror |
| 已装再 install | 提示已装 + 版本;用 update 升级 |
| remove 正在被 LSP 使用的 server | 照删配置项;提示重启后生效(不杀进程) |
| 用户 languages.toml 语法坏 | 不改写,报错指出(标记段剥离需先解析成功) |
| 语言已被用户自定义 | 按 §3.4 只提示手动引用,不覆盖 |

## 5. 测试计划

- 单元(registry):配方解析/版本检测命令构造;URL mirror 改写;标记段 剥离/重建/幂等(多次 install/update/remove 结果稳定);用户自定义语言条目不被覆盖。
- 集成:
  - `:server install`(archive 假源:本地 http/file:// 服务迷你 release)→ languages.toml 生成含 command 绝对路径;语言可用
  - update/remove 后标记段正确
  - 用户手写 languages.toml 内容在重写后保留;坏文件不改写
  - tool 类配方(pip --target 迷你包)安装与 launcher
  - UI 面板 list/install/remove 冒烟(可后续单独交付)
- 回归:插件管理器/现有 suite 不受影响(独立模块)。

## 6. 里程碑

1. registry + 受管目录 + archive 安装/升级/卸载(含 sha256、mirror 改写)
2. tool 类安装(pip/cargo/npm 前缀)
3. languages.toml 标记段读写 + 合并规则 + DAP debugger 写入
4. `:server` 命令族 + config 段
5. UI 面板插件 + health 提示
6. 文档 + 全量回归

每期独立可交付;3 完成后"rust-analyzer 一键装+自动接语言"即闭环,可用性最高。

## 7. 风险

- **language-servers 挂接正确性**:helix 只启动"被某语言引用"的 server;标记段按配方 languages 生成最小 `[[language]]`,需验证与内置 language 配置合并语义(选 rust 在 3 期先验证)。
- **用户配置冲突**:合并规则保守(§3.4),宁可提示不覆盖。
- 网络/镜像差异:file:///本地假源用于测试,不依赖外网。
- 配方维护成本:先 ~8 个,v1 后按需扩注册表(纯数据)。
