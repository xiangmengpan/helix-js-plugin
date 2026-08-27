# final-review 修复波报告

**提交：** `1258b6b5e fix: 最终审查 4 项——集成测试 col 断言、opt_u16 错误前缀参数化、lsp 命名空间注册测试、部分对象语义文档注记`
（初版 b97097e47，按父任务指定消息 amend）

**依据：** `.superpowers/sdd/2026-08-26-js-lsp-request/final-review.md`（结论「修完再合」，1 Important + 3 Minor，合计约 7 行）

## 实现内容（4/4 全部落地）

### 1. 【Important】`helix-term/tests/test/plugin_lsp.rs` — col 维度测试缺口
`plugin_open_file_with_position` 闭包内补 3 行：`col = pos - line_to_char(line)` 断言 == 4；并补字符断言 `doc.text().char(pos) == '9'`（"line9" 第 4 字符），双重钉死 col 路径（typed.rs col 定位是全新代码，此前零覆盖）。

### 2. 【Minor 1】`helix-js/src/popup.rs` — opt_u16 错误前缀误导
`opt_u16` 两处格式串去掉硬编码 `"open_popup: "` 前缀（review 给出的两个修法中选了**文案泛化**，2 行 vs 加 `api` 参数同步 8 处调用点）。现在 `helix.lsp.hover({row:"abc"})` 报 `'row' must be a number`，open_popup/open_file/lsp 三 API 共享语义正确。

### 3. 【Minor 2】`helix-js/src/lsp.rs` — 注册名零覆盖
新增 `lsp_global_registration_names` 单测：在真实 CONTEXT 引擎上 eval 全局 `helix.lsp.hover/completion/goto_definition/document_symbols()` 四个路径，断言均返回 Promise，并核对入队请求方法顺序。任一注册名拼错 → eval 抛错 → 测试红（此前单测全走 `js_lsp_*` 直接函数，注册字符串拼错静默坏）。

### 4. 【Minor 3】`docs/plugin-api.md` §20 — 位置覆盖缺省语义未注明
`// 位置覆盖` 注释下补一行：row/col 须同传，只传其一按缺省（当前光标）静默处理——与 `row.zip(col)` 实现及 open_file 侧既有注意（§798）对齐。

## 验证（TDD 证据）

- **RED→GREEN（编译期测试先行）**：col 断言先写，首跑编译报 `E0277 can't compare char with Option<char>`（`RopeSlice::char` 返回 `char` 非 `Option<char>`，我已按 `Some('9')` 写）→ 修正为 `'9'` 后编译通过。
- `cargo test -p helix-js` → **65 passed**（含新增 `lsp_global_registration_names`）。
- `cargo test -p helix-term --features integration --test integration plugin_lsp` → **2 passed**（含补强后的 `plugin_open_file_with_position`）。
- `cargo clippy -p helix-js` → 0 warning/error；`cargo clippy -p helix-term --features integration --tests` → 51 warning 全部 pre-existing（base 同样 51，均属 plugin_statusline.rs 等未触及文件的 unused import/variable），我的文件零警告。
- `cargo fmt --check`：base 即有 673 文件漂移（pre-existing，非本次引入）；我新增代码零 fmt 差异（lsp.rs 的 2 处 fmt diff 在既有 `location_path_injection` 测试内，非新代码）。

## 文件变更

| 文件 | 变更 |
|------|------|
| `helix-term/tests/test/plugin_lsp.rs` | +7（col/字符断言） |
| `helix-js/src/popup.rs` | 2 行文案 |
| `helix-js/src/lsp.rs` | +33（注册名测试 + Source 导入） |
| `docs/plugin-api.md` | +1（缺省语义注释） |

## 自检

- 范围：仅 final-review 点名的 4 处；未顺手清理 parked 项（LspReqKind 合并等均为【可延后】）。
- 选择理由：Minor 1 取文案泛化而非 api 参数（YAGNI，最短 diff，review 明示两案皆可）。
- 无遗留 TODO/占位符。

## 关注点

无。final-review 的 4 项已清零，按 review 结论「修完再合」现可合并；其「建议 1」——真 LSP 环境手动验证（demo 弹窗/跳转）——仍需人工执行，属计划第 5 任务步骤 3 的遗留，非代码缺陷。
