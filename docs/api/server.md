# `:server` —— LSP / DAP / linter / formatter 管理

> 装、卸、升级、**自检**、按项目对齐。配方来源 `[server-manager.registry]`
> （内置在 `runtime/server-manager-builtin.toml`，可按名字覆盖）。
> 受管目录：`<config>/server-manager/`（`managed/bin/<name>` 软链，`languages.toml` 自动接线）。

## 命令

| 命令 | 说明 |
|---|---|
| `:server list` | 已装 / 本机已有 / 可装 |
| `:server search <kw>` | 按名字/语言过滤配方 |
| `:server install <name> [version]` | 安装（zip/tar.* + sha256 校验），装完**自动写 `languages.toml`** |
| `:server update [name]` | 升级受管项（省略 = 全部） |
| `:server remove <name>` | 卸载受管项（并回滚 `languages.toml` 里的托管段） |
| `:server unmanage <name>` | 本机已有、不再自动挂接（不卸载） |
| `:server status` | 计数 / 受管目录 / `languages.toml` 路径；**有失败记录时开 buffer** 显示"上次失败" |
| **`:server doctor [name]`** | 自检：可执行 / 可执行位 / **有没有被某个语言引用** |
| **`:server log [name] [n]`** | 把 `helix.log` 里该 server 相关的尾部开成 buffer（默认 300 行） |
| **`:server sync [check] [prune]`** | 按 `.helix/servers.toml` 对齐：装缺的、重写配置；`check` 只看差异，`prune` 额外卸掉多余的 |
| `:server panel` | 面板（列表/过滤/状态列） |

## 装完会不会自动生效？

| kind | 行为 |
|---|---|
| `lsp` | 写 `[language-server.<name>]` + `[[language]] language-servers = [...]` ✅ |
| `dap` | 写 `[[language]] debugger = {...}` ✅ |
| `formatter` | 写 `[[language]] formatter = { command, args }`（args 来自配方的 `format-args`）✅ |
| `linter` | **不接线**（核心没有"外部 linter → 诊断"通道）；`:server doctor` 会直接告诉你，并建议用它的 LSP 模式 |

> 用户已在 `languages.toml` 里自定义了某语言 ⇒ 不覆盖，只在 `conflicts` 里给手动接法。

## 项目级"期望状态"：`.helix/servers.toml`

```toml
# 这个项目要哪些 server（团队可以提交进仓库）
server = ["rust-analyzer", "clangd"]

# 钉版本（配方没固定版本时必填；`install` 也认它）
[version]
rust-analyzer = "2025-07-01"
```

然后：

```vim
:server sync check   " 只报告差异，不动磁盘
:server sync         " 装缺的 + 重写 languages.toml
:server sync prune   " 额外卸掉"装了但清单里没有"的受管 server
```

## 失败与并发

- **失败留存**：`install/update` 失败会写 `managed/last-error.json`，`:server status` / `doctor` 直接显示（不再只在状态栏闪一下）。
- **跨进程锁**：`managed/install.lock`（flock）。两个 hx 同时装/卸 ⇒ 后一个明确报"另一个 hx 正在安装"。
- **重启生效**：`languages.toml` 的改动在下次启动 hx 时生效（`server install` 的状态栏会提示）。

## 配方字段（`[server-manager.registry.<name>]`）

```toml
kind = "lsp" | "dap" | "linter" | "formatter"
languages = ["rust"]
bin-name = "rust-analyzer"     # 可选，默认 = 名字
version = "1.2.3"              # 固定版本（url 里可用 {version}）
url = "https://…"              # 或逐平台 asset[]（Mason 形状）
sha256 = "…"                   # 有 url 时应给
strip = 1
cmd = "…"                      # 或 Tool 安装（走包管理器）
format-args = ["-q", "-"]      # 仅 formatter：写进 formatter.args
```

`format-args` 支持编辑器变量展开（如 `%{buffer_name}`）；**约定是 stdin → stdout**
（helix 的 formatter 协议）。
