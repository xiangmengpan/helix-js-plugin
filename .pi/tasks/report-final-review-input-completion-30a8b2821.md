# final-review 修复波报告（input 组件升级 + 补全联动）

**提交：** `30a8b2821 fix: 最终审查 2 项——input 光标渲染测试覆盖、按键表 Space 行修正`

**依据：** `.superpowers/sdd/2026-08-26-js-input-completion/final-review.md`（结论「修完再合」，1 Important + 1 Minor）。ledger `progress.md` 已追加完成行；tasks.json id 9 → completed。

## 实现内容（2/2 全部落地）

### 1. 【I1·Important】comp_layout.rs — 光标 `|` 渲染零测试覆盖

`#[cfg(test)] mod tests` 新增 2 个测试（+1 个 `input_node` 构造 helper）：

- `input_renders_cursor_bar_in_place`：`"abc"` cursor 1 → `a|bc`；末尾 cursor 3 → `abc|`；空值 → `|`；**UTF-8 按 char 索引**：`"中文"` cursor 1 → `中|文`（钉死 char 而非 byte 索引，off-by-one 高发点）。
- `input_cursor_clamps_and_truncates`：cursor 99 越界 → clamp 末尾 `abc|`（防 Vec::insert panic 路径）；width 截断（先插 `|` 再截断：cursor 0 + width 3 → `|ab`）；viewport 宽度优先于 width。

**TDD 证据：**
- GREEN：`cargo test -p helix-term --lib ui::comp_layout` → 27 passed（含新 2 个，行为已正确，属钉死既有语义）。
- RED（mutation 验证测试有效）：临时把 clamp 改 `(*cursor).min(chars.len().saturating_sub(1))` → 两个新测试**全部 FAILED**（末尾光标 off-by-one + 越界插 panic 均被捕获）；随后还原，重跑 27 passed。

### 2. 【M1·Minor】Space 键事实错误 — 文档 + 死分支 + 注释

已核实 `key_to_plugin_key`（plugin_popup.rs:238-256）对 `Char(' ')` 产出 `" "`，管线**不存在 "Space" 键名**——空格实际命中单字符编辑臂：

- `docs/plugin-api.md` 按键表：`| Enter / Space | → onKey("Enter"/"Space")` → `| Enter | → onKey("Enter")（空格键是单字符编辑键，命中首行插入输入框）`。
- `plugin_popup.rs` 路由：`"Enter" | "Space" =>` 删掉永不可达的 `"Space"` 备选（review 明确定性为 dead 分支；零行为变化，Enter 路径有集成测试覆盖），注释同步为「Enter：input 有状态 → onKey("Enter")；否则（button）→ onPress（空格键是单字符，走下方编辑臂；key_to_plugin_key 无 "Space" 键名）」。

## 验证

- `cargo test -p helix-js --lib` → **68 passed**。
- `cargo test -p helix-term --lib ui::comp_layout` → 27 passed；`ui::plugin_popup` → 3 passed。
- `cargo test -p helix-term --features integration --test integration plugin_input` → 2 passed；`plugin_` 组 → **73 passed**。
- `cargo clippy -p helix-term --lib` → 0 warning/error（comp_layout/plugin_popup 无新告警）。
- fmt：新增行已对齐 rustfmt（helper/两条构造/UTF-8 断言按 rustfmt 输出改写）；剩余 diff 全为 base 既有全仓漂移（git stash 验证 flex_of 等漂移在 base 即存在，非本次引入）。

## 自检

- **范围**：仅 final-review 点名的 I1 + M1；未顺手处理 parked 项（M2-M11 按 review 结论全部延后）。
- **M1 超出 review 字面修法**（review 说「1 行表格 + 注释同步」）：额外删了 match 臂的 dead `"Space"` 备选——这是 review 自己定性为「永不可达 dead 分支」的根因清理，零行为变化，注释随之诚实；不删的话注释与代码将互相矛盾。
- **tasks.json** id 9 状态由 in_progress 修正为 completed（此前 786e0bb20 已提交但状态未翻转）；`progress.md` 追加 fix-wave 完成行（.superpowers 为 gitignored 本地账本，不入 commit，与全仓惯例一致）。

## 关注点

1. **docs line 338「焦点在 button 上：Enter/Space → onPress」**（6e2241e0f 引入，早于本计划）：按当前代码，空格在 button 上走编辑臂 → `onKey(" ")` 而非 onPress。属**前代文档遗留**，不在本计划 review 范围（review 只点名 input 表行），未动；如需修正请另行决定。
2. **偶发 flaky**：两次 cargo 进程并行跑（共享 target 目录）时出现一次 helix-js 47 失败 + 一次 integration SIGABRT；全部顺序重跑即绿（68/68、73/73）。非本次改动引入，为既有并发构建竞态。
3. **人工验证遗留**（final-review 建议 1，非代码缺陷）：真 LSP 环境手动验证 `:ic` 出候选、`set_input_value` 回填。
