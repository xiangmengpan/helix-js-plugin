# 任务1 实现报告 — InputStates + input_edit 纯函数

**提交：** `3aa2a55bf feat(js): input 编辑状态机 InputStates + input_edit 纯函数`
**依据：** `.superpowers/sdd/2026-08-26-js-input-completion/task-1-brief.md`

## 实现内容

1. **新建 `helix-js/src/input.rs`**（164 行）：
   - `InputState { value: String, cursor: usize }`（char 索引光标，derive Debug/Clone/PartialEq/Eq）
   - `INPUT_STATES: thread_local RefCell<HashMap<(u64, String), InputState>>` + `with_input_states` 访问器
   - `input_edit(state, key) -> Option<String>` 纯函数：插入/Backspace/Delete/Left/Right/Home/End/非编辑键→None，UTF-8 按 char 处理，光标 clamp
   - `#[cfg(test)]` 两个单测 `input_edit_basic` + `input_edit_utf8_and_clamp`（TEST_LOCK + `crate::init()` 模式，同 lsp.rs）
2. **`helix-js/src/lib.rs`**：`mod input;` + `pub use input::{input_edit, with_input_states, InputState};`（2 行）

## TDD 证据

- **RED**：先写测试，`cargo test -p helix-js input_edit` → 编译失败（E0422/E0425/E0432：InputState/input_edit/with_input_states 未定义）。
- **GREEN**：实现后 `cargo test -p helix-js input_edit` → 2 passed；全量 `cargo test -p helix-js` → **67 passed, 0 failed**（65 既有 + 2 新增）。

## 对简报参考代码的修正（3 处，均不改变语义）

1. **E0505**：简报 Backspace/Delete 分支的 `rest.by_ref().take(n).chain(rest.skip(1))` 借用/移动冲突无法编译 → 改为两次 collect 后字符串拼接。
2. **E0015**：简报 `const { RefCell::new(HashMap::new()) }` 不合法（`HashMap::new` 非 const fn）→ 改 `RefCell::new(HashMap::new())`，对齐 state.rs:83-84 既有先例（COMMAND_DOCS 同款注释）。
3. **可见性矛盾**：简报 input.rs 写 `pub(crate) fn with_input_states`、lib.rs 写 `pub use input::with_input_states` —— 二者矛盾（pub use 重导出 pub(crate) 项 = E0365；保持 pub(crate) 则 dead_code 警告，CI 是 `clippy -- -D warnings` 会红）→ 将 `with_input_states` 提为 `pub`，lib.rs 按简报原文 `pub use`。任务 2 起 popup.rs 经 `crate::with_input_states` 消费。
4. 简报的未使用闭包 `char_at` 删掉（dead code，clippy 会红）。

## 验证

- `cargo test -p helix-js` → 67 passed（两次独立运行确认；一次 48 失败为 `pty_bash_interactive_no_error` 既有 flaky（bash -i 启动 4s 超时），panic 污染 TEST_LOCK 连锁失败，重跑即过，与本次改动无关）
- `cargo clippy -p helix-js --all-targets -- -D warnings` → 0 warning/error
- `rustfmt --check helix-js/src/input.rs` → clean（lib.rs 的 fmt 漂移为 HEAD 既有，早前报告已记录 673 文件 base 漂移，本次未引入）

## 文件变更

| 文件 | 变更 |
|------|------|
| `helix-js/src/input.rs` | 新建（实现 + 2 单测） |
| `helix-js/src/lib.rs` | +2 行（mod input + pub use） |

## 关注点

1. **工作区并发 fmt**：提交前后检测到 13 个未触及文件（commands/popup/layout 等）被并发 `cargo fmt` 全量格式化（00:21:31，非本会话操作，疑为父终端并行操作）。已按范围约束**未提交**这 13 个文件，仅提交 input.rs + lib.rs；它们仍以 dirty 状态留在工作区，需父进程处置（提交或还原）。期间 lib.rs 曾被整文件重排，已 checkout 回 HEAD 后仅重放本任务 2 行。
2. **`with_input_states` 公开化**：见修正 3。若父进程要求保持 crate 私有，可改 `pub(crate) fn` + 移除 lib.rs 重导出，任务 2 内用 `crate::input::with_input_states` 引用——但需接受 clippy dead_code 直到任务 2 落地。
3. **任务 2 依赖**：`input_edit`/`InputState`/`with_input_states` 均已按任务 2 简报所需签名就绪（key 语义：`"Backspace"`/`"Delete"`/`"Left"`/`"Right"`/`"Home"`/`"End"`/单字符）。
