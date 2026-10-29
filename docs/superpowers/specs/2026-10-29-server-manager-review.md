# LSP / DAP / Lint / Formatter 管理插件 —— 现状评审（缺口清单）

> 审的对象是本仓现有的 `server_manager`（核心 Rust ~3400 行）+ `:server` 命令 +
> `helix.server.*` 插件面。结论先给：**"装/卸/升级"这条链是完整的，"用完接上"和
> "跑起来之后"这两段是空的** —— 而这两段才是一个"管理插件"的价值所在。

---

## 0. 现在有什么（先摸清家底，别重复造）

| 块 | 现状 | 位置 |
|---|---|---|
| 配方 | Mason 风格 recipes（`url_template` / 逐平台 `asset[]` / 固定 version） | `server_manager/registry.rs` |
| 安装 | 下载（可走镜像）+ sha256 校验 + 解压（zip/tar.*）+ 软链 `managed/bin/` | `download.rs` / `install.rs` |
| 卸载 | 删目录 + **重写 languages.toml**（托管段带标记） | `langs_toml.rs::split_managed` |
| 接线 | 只接 **LSP**（`[language-server.X]`）和 **DAP**（`[language.debugger]`） | `langs_toml.rs::build_section` |
| 命令 | `server list/search/install/update/remove/unmanage/status/panel` | `server_ops/*` |
| 插件面 | `helix.server.{list,search,install,update,remove,status,rows,task}` + 进度事件 | `helix.d.ts` |
| 观测 | `:server status`（装了没/版本/本机已有/managed dir） | `server_ops/status.rs` |
| 健康 | `:health` 只在启动时检查**语言配置里**的命令能不能跑，并给"可 install"提示 | `health.rs:270` |

**已经做得很对的地方**（别改）：数字签名式的 sha256 校验、临时文件 + rename、
"用户自带同名语言服务就不覆盖"、`unmanage`（本地工具不卸只停用自动挂接）、
`split_managed` 的托管段标记（卸载可干净回滚）。

---

## 1. 最大的洞：**Linter / Formatter 装完是死的**

```rust
// langs_toml.rs::build_section
match e.spec.kind {
    registry::Kind::Lsp => slot.0.push(...),
    registry::Kind::Dap => slot.1 = Some(e.spec),
    _ => {}        // ← Linter / Formatter 走到这里，什么都不写
}
```

后果：
- registry 里能"安装"一个 formatter（比如 `rustfmt`），装完 `languages.toml` **一个字都不变** ⇒ 用户装了却**没生效**，还以为坏了；
- linter 更彻底：核心根本没有"跑外部 linter → 诊断进编辑器"的通道（helix 的诊断只有 LSP 一条来源）。

### 建议（按代价从低到高）

| 方案 | 说明 | 代价 |
|---|---|---|
| **① formatter 真接线** | `Kind::Formatter` ⇒ 写进 `[[language]] formatter = { command = "<managed bin>", args = [...] }`（用户已有 formatter 就不覆盖，进 `conflicts` 提示） | 半天，纯改 `build_section` + 测试 |
| **② linter 降级为"命令提供者"** | 不自造诊断管道：装完只在 `:server` 里标"已装（命令：xxx）"，并在 `languages.toml` 注释里给出手动接法（多数 linter 有 LSP 模式：ruff / biome / deno lint / vscode-eslint） | 半天 |
| **③ linter 通道（真做）** | 新增"外部诊断源"：跑命令 → 解析（regex 模板 / sarif / `--format=json`）→ 注入 diagnostics，带缓存与按需触发 | **3–5 天**（还牵扯 UI：诊断来源标记、`:lint` 命令、防抖） |

**我的建议**：先 ①②，③ 单独立项。理由：现在 90% 的 linter 都有 LSP 形态，
自造解析管道的长期维护成本极高（lint 输出格式随版本变）。

---

## 2. 第二大的洞：**没有"运行时真相"**

`:server status` 回答的是"装没装"。用户真正会问的是：

- 这个 LSP **现在在跑吗**？为什么没起来？（二进制存在但 `--version` 挂、缺动态库、权限 0644、路径没进配置…）
- 它**这次**启动的 stderr 是什么？
- 我改完配置**生效了吗**（需要重启吗）？

现在这些只能翻 `~/.cache/helix/helix.log`。

### 建议

1. **`:server doctor [name]`**：逐项自检并给一句话结论
   ```
   rust-analyzer  ✓ 可执行 (v0.3.2100)
                  ✗ 未被任何语言引用（rust 的 language-servers = ["ruff"]）
                  ✓ initialize 成功（42ms）
   ```
   - 可执行性 + `--version`（已有 `detect_version` 可复用）
   - **配置引用检查**：这个 server 有没有真的被某个语言的 `language-servers` / `formatter` / `debugger` 指到（这是"装了但不生效"的最常见原因）
   - 可选的 `initialize` 冒烟（stub JSON-RPC，超时 2s）——比"能不能跑"强得多
2. **`:server log [name]`**：把某 server 的 stderr 尾部开成 buffer（像 `:debug-log` 那样复用 buffer，不新开垃圾）。
3. **失败留存**：安装失败的 `last_error` + 日志路径写进 status，而不是一闪而过的 status 行。

工作量：1–1.5 天（doctor 最值钱）。

---

## 3. 顺手能补的七个缺口

| # | 缺口 | 影响 | 修法 |
|---|---|---|---|
| 1 | **没有项目级配置**（只有全局 `languages.toml`） | 团队无法把 server 版本钉在仓库里；不同项目要不同版本就得手改全局 | 支持 `.helix/servers.toml`（`lsp = [...]`, `formatter = [...]`, `version = {...}`），优先级 项目 > 全局；配合 `server sync` 对齐 |
| 2 | **没有"期望状态"（声明式）** | 现在是"命令式"：装一个、卸一个。换机器要重来 | `server sync [--check]`：按配置把"该装的装上、该卸的卸掉、该接的接上"，一次输出 diff |
| 3 | **没有上游版本探测** | `update` 只能按配方固定版本或手输版本；用户不知道有没有新版 | `server update --check`：registry 支持 `version_source = "github-release:owner/repo"`，查最新 tag |
| 4 | **跨进程没有锁** | 两个 hx 同时装/卸同一个 server 可能互相踩目录（`op_lock` 只是进程内 Mutex） | managed root 下放 `install.lock`（`flock`），拿不到就"另一个 hx 正在安装" |
| 5 | **workspace trust 不参与** | `[server-manager.registry]` 里写自定义 url 就会下载并**执行**二进制；一旦将来支持项目级配置，等于"打开仓库即执行任意代码" | 下载/执行前查 `editor.workspace_trust`；项目级 registry **必须**显式确认 |
| 6 | **多 server 同语言的优先级/禁用没有入口** | `language-servers = ["a","b"]` 谁先、谁关掉，只能手改 toml | `:server attach/detach <server> <lang>`，UI 里勾选 |
| 7 | **发现性为零** | 用户不知道 `:server` 存在 | `:health` 的提示已经指路；再加 `<space>` 菜单一项 + 文档页（`docs/api/server.md` 现在没有） |

---

## 4. 插件面（`helix.server.*`）的缺口

现在能"操作"，不能"观察/编排"：

| 想要 | 现状 | 建议 API |
|---|---|---|
| 看某语言的**生效配置**（哪些 server 挂上、冲突在哪） | ✗ | `helix.server.effective(lang)` → `{ lsp: [...], formatter: {...}, dap: {...}, conflicts: [...] }` |
| 启用/禁用/排序 | ✗ | `helix.server.attach(server, lang, {priority})` / `detach` |
| 运行时状态 | ✗ | `helix.server.running()` → `[{name, lang, state, pid, started_at, last_error}]` |
| 读日志尾 | ✗ | `helix.server.log(name, lines)` |
| 事件 | 只有 `task` 的进度回调 | `helix.on("server-state")`（starting/ready/failed + 原因） |

**注意**：`rows()` 已经是"一次性回调"式（`cb` 调用一次即失效），做**面板**够用；
但如果插件要做"实时刷新的管理面板"，需要 `server-state` 事件而不是轮询。

---

## 5. 如果只做三件事（优先级）

1. **formatter 接线**（半天）—— 现成的洞，装完就该生效；
2. **`:server doctor`**（1 天）—— 把"为什么没生效"从翻日志变成一句话；
3. **"期望状态 + sync"**（1.5 天）—— 从"命令式安装器"升级成"可复现的环境管理"，这是插件真正的卖点（Mason 也没做到）。

linter 通道、项目级 registry、上游版本探测排在后面。

---

## 6. 自检清单（设计评审用）

- [ ] 安装失败时，用户能不能**不翻日志**知道原因？
- [ ] 装完之后，**会不会自动生效**？（不生效有没有明确提示 + 手动接法）
- [ ] 卸载是不是**可回滚**（配置里托管段之外一个字都不动）？
- [ ] 同一台机器**两个 hx 并发**装/卸会不会打架？
- [ ] 换机器 / 新同事入职，能不能**一条命令**复现同一套环境？
- [ ] 打开一个**不可信仓库**，会不会因为项目级配置就执行任意二进制？
- [ ] 版本升级失败，能不能**回到上一个可用版本**？
- [ ] 有没有"这个语言我到底该装哪个"的入口（能力矩阵 / 推荐）？


---

## 7. 落地记录（2026-10-29，按本文件的建议顺序）

| # | 项 | 状态 |
|---|---|---|
| 1 | **formatter 接线** | ✅ `build_section` 写 `[[language]] formatter = { command, args }`；配方新增 `format-args`（black=`["-q","-"]`、prettier=`["--stdin-filepath","%{buffer_name}"]`）；单测 `formatter_is_wired_into_languages_toml` |
| 2 | **linter 降级** | ✅ 不接线，但 `:server doctor` 明确说"核心没有 lint 通道，请用 LSP 模式" |
| 3 | **`:server doctor [name]`** | ✅ 可执行 / 可执行位 / **是否被某语言引用**（用**已加载的语言配置**判断，不是 grep 文本）+ 结论行 |
| 4 | **`:server log [name] [n]`** | ✅ `helix.log` 过滤尾部进 buffer（复用同一个） |
| 5 | **失败留存** | ✅ `managed/last-error.json`；`:server status` 有失败时开 buffer 显示 |
| 6 | **跨进程锁** | ✅ `managed/install.lock`（flock，非阻塞）；install/update/remove 都取 |
| 7 | **期望状态 + sync** | ✅ `.helix/servers.toml`（`server = [...]` + `[version]`）；`:server sync [check] [prune]` |
| 8 | 文档 | ✅ 新增 `docs/api/server.md`（命令表 / 接线行为 / 配方字段 / 失败与并发） |

**没做**（诚实记录）：`<space>i` 的键位入口（键位路径推 picker 在那一帧不稳，已回退，
`:server panel` 命令仍可用）；`update --check`（上游最新版本探测）、项目级 registry 的信任门禁、
多 server 同语言的优先级 UI —— 这三个仍按原建议排在后面。
