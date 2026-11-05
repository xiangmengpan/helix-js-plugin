//! 插件**作用域键位**（`helix.keymap.*`，A 阶段）的端到端回归。
//!
//! 这条测试钉住三件事：
//! 1. 插件能声明作用域，`resolve()` 认得**两种键形状**（onKey 对象 / 字符串）；
//! 2. `?`/`:keys` 的"插件作用域"段与声明表同源（不再各写一份）；
//! 3. `extends: "panel"` 的预设真的被展开（`Esc` = 回焦点，2026-11-04 定案）。
//!
//! 设计文档：`docs/superpowers/plans/2026-11-04-panel-keybinding-scopes.md`

use super::*;

/// 声明 + 解析：对象与字符串命中同一条绑定，未声明的键返回 null
#[tokio::test(flavor = "multi_thread")]
async fn plugin_keys_scope_resolve_both_key_shapes() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("ks.txt");
    std::fs::write(&file, "x\n")?;
    let plugin_path = dir.path().join("kscope.js");
    std::fs::write(
        &plugin_path,
        r#"
        helix.register_command("ks-probe", () => {
          const out = [];
          // ① 字符串形状
          const a = helix.keymap.resolve("ks-demo", "q");
          out.push("str=" + (a && a.action));
          // ② onKey 原样对象形状
          const b = helix.keymap.resolve("ks-demo", { name: "q", ctrl: false, alt: false, shift: false });
          out.push("obj=" + (b && b.action));
          // ③ 预设（extends: "panel"）：Esc 回焦点
          const c = helix.keymap.resolve("ks-demo", "<esc>");
          out.push("esc=" + (c && c.action));
          // ④ 未声明
          out.push("miss=" + String(helix.keymap.resolve("ks-demo", "z")));
          // ⑤ 提示同源
          out.push("hints=" + Boolean(helix.keymap.hints("ks-demo")));
          helix.echo(out.join(" "));
        }, "探针");

        helix.keymap.scope("ks-demo", {
          extends: "panel",
          keys: { "q": "close-me" },
        });
        "#,
    )?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some(&format!(":plugin-load {}<ret>", plugin_path.display())),
                None,
            ),
            (
                Some(":ks-probe<ret>"),
                Some(&|app| {
                    let status = app
                        .editor
                        .get_status()
                        .map(|(msg, _sev)| msg.to_string())
                        .unwrap_or_default();
                    assert!(
                        status.contains("str=close-me"),
                        "字符串形状应命中: {status:?}"
                    );
                    assert!(
                        status.contains("obj=close-me"),
                        "对象形状应与字符串命中**同一条**绑定: {status:?}"
                    );
                    assert!(
                        status.contains("esc=blur"),
                        "extends:panel 的 Esc 应是「回焦点」: {status:?}"
                    );
                    assert!(
                        status.contains("miss=null"),
                        "未声明键应返回 null: {status:?}"
                    );
                    assert!(status.contains("hints=true"), "hints 应有内容: {status:?}");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}

// 注：`:keys` 里的"插件作用域"段**没**在这里做端到端断言 —— 驱动真实 UI 打开
// help buffer 会让 `test_key_sequences` 的收尾（`<esc>:q!`）行为不稳定（实测挂住 30 分钟）。
// 该段的**内容**由 `helix-js::keymap_scope::section()` 的单测钉住（见该模块 tests），
// `keys_help` 里只是把它拼进 buffer。

// 注：`helix.map` 覆盖内置键的**状态栏提示**没有在这里做端到端断言 ——
// 驱动真实 UI 送 `<esc>` 后 `test_key_sequences` 的收尾会挂住（实测 852 秒未退，
// 两次尝试都是 `expected app to exit: false != true`）。
// 提示的**内容**由 `Keymaps::describe_override` 的单测钉住（`keymap::plugin_binding_tests`），
// `handle_map_key` 里只是 `set_warning` 那一行。
