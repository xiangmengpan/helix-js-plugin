# 报告:任务 2 yank-error — :yank-error 命令 + integration

计划:`docs/superpowers/plans/2026-08-30-js-yank-error.md` 任务 2
Commit:`2dc13718b`

## 实现内容(3 文件,+66)

1. `helix-term/src/commands/typed.rs`(+33):新增 `yank_error` 命令——照 `yank_diagnostic` 同款结构:Validate 才执行;参数寄存器(默认 `+`);`cx.editor.last_error` 为空则 `bail!("No error to yank")`(经 command_mode 错误路径 → set_error 展示);写入 `registers.write(reg, vec![err.to_string()])` + 状态栏 "Yanked error to register {reg}"。注册于 `yank-diagnostic` 旁,doc/completer(register)/signature(positionals 0..1)照抄其形。
2. `helix-term/tests/test/plugin_yank_error.rs`(+32,新建):integration 测试——`:plugin-load` 载入非法 JS → 命令错误经 `command_mode` 回调 → `set_error` → `last_error` 记录 → `:yank-error` 复制到 `+` 寄存器,断言寄存器值同时含 `plugin-load` 与 `bad.js`(错误文本 `'plugin-load': plugin-load: plugin script '<path>' error: SyntaxError: ...` 两子串必现,宽松匹配)。
3. `helix-term/tests/integration.rs`(+1):`mod plugin_yank_error;`(字母序,plugin_theme 与 splits 之间)。

## TDD 证据

- **RED**:首个 run 编译错误(`registers.read` 需 `&Editor` 参数、`any` 需 mut——brief 草稿的 `read('+')` 签名与现 API 不符,按 `read(name, editor)` 修正);修正后 `plugin_load_failure_yankable` 失败(断言失败,`:yank-error` 未注册)
- **GREEN**:实现命令后同测试通过,OSC52 剪贴板转义输出即 yanked 错误文本,佐证 `+` 寄存器已写入

## 验证(全部实测)

- `cargo test -p helix-term --features integration --test integration plugin_yank_error` → 1 passed
- `cargo test -p helix-view` → 70 passed + 13 passed
- `cargo test -p helix-term --lib` → 90 passed
- `cargo build -p helix-term` → Finished
- `cargo clippy -p helix-view -p helix-term` → 无 error,无新 warning(helix-core 1 warning 为既有基线)
- `cargo fmt --all --check` → 仅 `helix-js/src/picker.rs` 漂移(并行批次遗留,未入 commit)

## 自审

- 改动严格限任务 2 范围;命令逻辑与 yank_diagnostic 同构,无新增抽象
- 测试仅 1 条覆盖核心链路(load 失败 → last_error → yank 到默认寄存器),无错误路径与自定义寄存器分支按 YAGNI 未加——命令自身与 yank_diagnostic 完全同构,风险极低
- fmt 途中踩坑:手跑 `rustfmt --edition 2024` 重排了无关 import(项目 edition 2021),已 checkout 还原后以 `--edition 2021` 重排,最终 diff 仅含本任务改动

## 关注点

- 工作树仍留并行批次未提交改动:`plugins/init.js`(space-g grep 启用)与 `helix-js/src/picker.rs`(fmt 漂移)——与本任务无关,未 add 未碰
