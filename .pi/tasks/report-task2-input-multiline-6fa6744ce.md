# 报告:任务 2 input multiline 接线 + 多行渲染 + integration

计划:`docs/superpowers/plans/2026-08-30-js-input-multiline.md` 任务 2
Commit:`6fa6744ce`

## 背景说明

接手时工作树已含任务 2 的完整未提交实现(父代理/前次执行遗留,含 integration 测试文件),本实现者负责:核验正确性、跑通测试、clippy/fmt 核验、补遗漏、提交。

## 实现内容(核验 + 已提交,10 文件 +241/-20)

1. **`helix-js/src/types.rs`** — `CompNode::Input` 加 `multiline: bool`(带注释)
2. **`helix-js/src/popup.rs`** — input 分支解析 `multiline`(仿 focusable 模式,缺省 false)→ InputState 初始化 + CompNode 构造(替换任务 1 的占位 `false`)
3. **`helix-js/src/input.rs`** — 新增 `input_is_multiline(popup_id, node_id)` 查询接口(term 侧按键路由用)
4. **`helix-js/src/lib.rs`** — 导出 `input_is_multiline`
5. **`helix-term/src/ui/comp_layout.rs`** — Input 分支 multiline 多行渲染:按 `\n` 分行(owned Vec),光标所在行插 `|`(remaining 逐行递减,`remaining <= llen` 落行),每行截断到 limit;`is_single_line` 对 Input 改判 `!multiline`;补单测 `input_multiline_renders_lines_with_cursor`(4 断言 + 宽度截断,含光标落 `\n` 位置属上一行行尾的边界)
6. **`helix-term/src/ui/plugin_panel.rs` / `plugin_popup.rs`** — 焦点在 input 时:multiline → Enter/Up/Down 走 `dispatch_input_key`(消费,不触发 onKey/提交);单行 → 原 onKey 路由不变
7. **`helix-term/tests/test/plugin_input_multiline.rs`**(新)+ integration.rs 注册 — 白盒 integration:Tab 聚焦多行 input → 字符插入 → Up 列保持+短行 clamp → Down 同列 → Tab 到单行 input Enter 仍 onKey(回归)→ 回多行 input Enter 插 `\n`(不提交),全链路 onChange 回显断言
8. **`docs/plugin-api.md`** — input 节点加 multiline 文档(语义表/焦点系统/示例补 `multiline: false`)

## 核验证据

- 逻辑核验:渲染与 input_edit 行模型一致(光标在 `\n` 位置 = 上一行行尾;End → 落 `\n` 位置,渲染插 `|` 于行尾——单测 `("ab\ncd",2) → "ab|","cd"` 锁定)
- `cargo test -p helix-js` → **93 passed**(全量单行回归)
- `cargo test -p helix-term --features integration --test integration plugin_input_multiline` → **1 passed**
- `... plugin_panel_focus` → **6 passed**(共享路由回归)
- `... plugin_input` → 2 passed、`plugin_popup_edit` → 1 passed、`plugin_panel` → **8 passed**(我改了 plugin_panel/plugin_popup 的 Enter/Up/Down 分支,多跑一条 suite)
- clippy:helix-js `--all-targets` 0 告警;helix-term 4 条告警全部**既有**(helix-core/text_annotations.rs private_bounds、completion.rs unnecessary_map_or——均非本任务文件,基线已有)
- fmt:本任务全部文件 `rustfmt --check` 干净(仅 picker.rs 显示漂移——picker 批次遗留,任务 1 报告已记录,不入本 commit)

## 自审

- 改动均为任务 2 计划范围内,无新增抽象;`input_is_multiline` 是最小查询接口,term 侧两处复用
- 单行路径逐字节不变(comp_layout 单行分支原样保留;plugin_panel/popup 的 multiline 分支前置守卫,单行走原代码)

## 关注点

- **`plugins/init.js` 有一处未提交改动**(取消注释 `space-g → picker.run("grep")`):属 picker 批次遗留,与本任务无关,未纳入 commit,工作树保留该改动待父代理定夺
- 任务 2 计划中的 integration 断言以"执行时推演"为准,实现与 task-1 单测锁定的行模型一致
- 全量 306 条 integration suite 未整跑(控制者环节跑);本任务相关 suite 全绿
