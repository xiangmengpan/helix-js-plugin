# 任务 5 报告：demo 插件 + 文档

**提交：** `d4a33fc47 feat(plugins): lsp-hover demo + plugin-api LSP 章节`

## 实现内容

### 1. `plugins/features/lsp-hover/index.js`（新建）

demo 插件，两个命令：

- `:lsp-hover` — `await helix.lsp.hover()`，null → `helix.echo("no hover")`；否则归一 `Hover.contents`（字符串 | `{value}` | 二者数组 → 文本行）后用 `open_popup` + `helix.el("col", [...text 节点])` 弹窗展示（多行拆分）。
- `:lsp-goto` — `await helix.lsp.goto_definition()`，空 → echo；否则取首个 `Location`，用注入的 `l.path` + `open_file(path, { row: range.start.line, col: range.start.character })` 跳转定位。

### 2. `docs/plugin-api.md`（修改，+69 行）

- 目录追加 `[20. LSP 请求](#20-lsp-请求)`。
- §19 API 速查索引：`open_file` 签名补第二参数 `{ row?, col? }`，新增 `helix.lsp.hover/completion/goto_definition/document_symbols` 四行。
- 新增 §20「LSP 请求」章节：4 方法签名 + LSP 请求映射表、位置覆盖/缺省光标快照语义、path 便利字段说明、空/错语义表（无 server/不支持 → resolve null；协议 error/异常 → reject；悬挂无超时）、完整示例代码。

## 与计划的偏差（均已修正为正确形态）

计划里的 demo 用了 `helix.el({ type: "col", children: [...] })` 单对象形式——**实测 `helix.el(type, arg, opts)` 的 type 必须是字符串**（`popup.rs:453`，`try_js_into::<String>` 会失败），已改为 `helix.el("col", [...])`。其余（`l.range.start.line/character`、`c.value ?? c` 归一）与实现一致。

## 验证

- `node --check plugins/features/lsp-hover/index.js` → syntax OK。
- API 名逐一核对实现：`helix-js/src/lib.rs:154` 注册 `helix.lsp` 命名空间，4 个方法名与 demo 完全一致；`helix-js/src/popup.rs:351` `js_open_file` 第二参数 `{row, col}` 与 demo 用法一致。
- demo 的 async 命令 + `.then` 续体模式与既有集成测试 `helix-term/tests/test/plugin_lsp.rs`（`plugin_lsp_no_server_resolves_null`）同构——该测试已覆盖「async 命令 + await helix.lsp.hover() + 无 server → null → 续体 echo」全链路。
- **无 Rust 代码改动**（本次 diff 仅 1 个新 JS 文件 + 1 个 md 文档），未触发 cargo 测试。
- 手动验证（真实 LSP 环境，如 Rust/TS 项目）需人工执行：`cargo build` 后 `:plugin-load plugins/features/lsp-hover/index.js`，`:lsp-hover` 弹窗 / `:lsp-goto` 跳转。

## 自检

- 范围：仅计划列出的两个文件；未加第三个命令（completion/symbols demo 属 YAGNI，文档已覆盖 API）。
- 文档与实现一致：语义表（null/reject/悬挂）与 `resolve_lsp` 实现及设计文档一致；path 注入（Location `uri` / LocationLink `target_uri` 两形态）描述准确。
- 无遗留 TODO/占位符。

## 关注点

无。唯一提醒：§20 追加在 §19 速查索引之后（按计划「追加」要求，避免重编号 7-19 造成大 diff）；索引已同步更新，阅读顺序影响可忽略。
