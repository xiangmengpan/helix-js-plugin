# Server Manager（mason 式）实现计划

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法来跟踪进度。

**目标：** `:server` 统一管理 LSP/DAP/linter/formatter：注册表 + 镜像安装/升级/卸载 + 自动写 languages.toml（标记段）。

**架构：** 新模块 `helix-term/src/commands/server_manager.rs`（同 plugin_manager.rs 并置）；下载层统一 mirror URL 改写 + sha256；languages.toml 标记段文本手术；`:server` typed 命令 + `UiRequest::ServerOp`；JS 只做 UI。

**技术栈：** Rust（tokio 下载/reqwest？检查依赖→优先复用 crate 内已有 http 客户端能力，否则加 reqwest 或 min reqwest-blocking）；TOML 处理沿用现有 toml crate。

**规格：** `docs/superpowers/specs/2026-09-05-server-manager-design.md`

---

## 文件结构

- 创建 `helix-term/src/commands/server_manager.rs`：registry/ServerSpec、受管目录、download(mirror+sha256)、install/update/remove、languages.toml 标记段读写、`:server` 命令实现。
- 创建 `helix-term/src/commands/server_manager/registry.rs`：内置配方常量表 + config 扩展解析。
- 修改 `helix-term/src/commands.rs`：`pub(crate) mod server_manager;`。
- 修改 `helix-term/src/commands/typed.rs`：注册 6 个 typed 命令 + `UiRequest::ServerOp` 分发（PluginOp 同构）。
- 修改 `helix-js/src/commands.rs` / `types.rs` / `lib.rs`：`helix.server.*` JS API + ServerOp 变体。
- 修改 `helix-term/src/config.rs`：`Config` 加 `server_manager: Option<toml::Table>`（或结构体）。
- 修改 `helix-term/src/health.rs`：缺失 server 提示（低优先，可后置）。
- 创建 `helix-term/src/commands/server_manager/tests.rs` + 集成 `helix-term/tests/test/server_manager.rs`。
- 文档：README/plugin-api（后置）。

---

## 任务 1：registry + 受管目录 + archive 安装/升级/卸载（sha256 + mirror）

**文件：** `server_manager/registry.rs`、`server_manager.rs`、`commands.rs`

- [ ] **步骤 1：失败单测**：配方解析/查表；`mirror_url(url)`（file:// 原样、https+mirror 前缀改写）；安装流程用 **file:// 假 release**（测试夹具在 `tests/fixtures/releases/<name>/<ver>/` 放打包 tar.gz+sha256）。
- [ ] **步骤 2：实现**
  - `ServerSpec`/`InstallSpec::Archive{ url_template, sha256, strip }`；内置表（rust-analyzer / pyright / tsserver / gopls / clangd / debugpy(Tool? 见 T2) / black / prettier…——archive 类先列 4-5 个）。
  - 下载（blocking 或已有异步设施）+ 校验 + 解压（`tar`/`zip` crate？查依赖；否则用 `tar` crate + 已有 gzip 解压）→ `managed/<name>` 临时目录 + rename。
  - `managed/bin` 软链/launcher（`std::os::unix::fs::symlink`）。
  - update（version_check 检测 → 重装）与 remove。
- [ ] **步骤 3：跑单测 + fmt + commit**
  `cargo test -p helix-term server_manager`；`git commit -m "feat(server): registry + archive 安装/升级/卸载(mirror+sha256+managed/bin)"`

---

## 任务 2：tool 类配方（pip/cargo/npm 前缀）

- [ ] **步骤 1：失败单测**：Tool 配方执行于 `managed/<name>`（本地假"工具"脚本模拟 pip --target），launcher 生成正确。
- [ ] **步骤 2：实现** `InstallSpec::Tool{ cmd, args }`：命令在受管目录内执行；记录 bin 绝对路径；幂等/失败清理。
- [ ] **步骤 3：测试 + commit**

---

## 任务 3：languages.toml 标记段读写与合并 + DAP debugger

**文件：** `server_manager.rs`（languages 段）、测试同任务 1

- [ ] **步骤 1：失败单测**：剥离/重建/幂等；用户标记外内容保留；坏文件不改写报错；用户已自定义某语言 → 不生成该语言条目（只留提示信息由命令层给）。
- [ ] **步骤 2：实现**
  - 文本手术：`# >>> helix-managed` … `# <<< helix-managed` 之间整体替换为重建 TOML 文本（`[language-server.<n>] command=绝对路径` + 最小 `[[language]]` 条目 + DAP `debugger` 子块）。
  - 重建输入 = 已装清单（遍历 managed 目录 + registry 匹配 languages）。
  - **验证点**：rust 的最小 `[[language]]` 与内置配置合并生效（集成：装 rust-analyzer 假源 → languages.toml 生成 → 语言可用/health 不报缺失）。
- [ ] **步骤 3：测试 + commit**
  `git commit -m "feat(server): languages.toml 标记段自动写(幂等合并,含 DAP debugger)"`

---

## 任务 4：`:server` 命令族 + config 段 + JS API

- [ ] **步骤 1：集成测试**：typed `:server list/install/update/remove/status` 冒烟（file:// 假源）；`helix.server.list()` JS 可调。
- [ ] **步骤 2：实现**
  - typed.rs 注册 6 命令（签名/补全仿 plugin 命令）；`[server-manager]` config 段（dir/mirror/registry 覆写）读入。
  - UiRequest::ServerOp{op,arg} + JS `helix.server.list/install/update/remove`。
- [ ] **步骤 3：测试 + commit**

---

## 任务 5：UI 面板 + health 提示（可选拆交付）

- [ ] filetree rail 骨架插件：列表/搜索/状态/install(u)/remove(x)；依赖 T4 的 ServerOp。
- [ ] health.rs：检测到"语言 server 缺失且注册表有配方"→ 附 `:server install <name>` 行。
- [ ] commit

---

## 任务 6：文档 + 全量回归

- [ ] README/plugin-api 增加 server manager 段；`cargo test`（view/js/term 全量）、clippy、fmt；`git commit -m "docs(server): …"`

---

## 自检备注

- 覆盖：规格 §3.1/3.2/3.3→T1/T2；§3.4→T3；§3.5/3.6→T4/T5；§4 错误→各任务；§5 测试→各任务；§6 里程碑→T1..T6。
- 依赖先查：解压用现有 crate（tar/gzip）；下载是否已有可用设施（避免无谓加依赖——规格允许 file:// 测试免外网）。
- 类型一致性：`ServerSpec/InstallSpec/ServerOp` 名先定义后引用；`server_manager::registry` 路径统一。
