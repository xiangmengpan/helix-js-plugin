//! 调试启动配置（`.helix/launch.toml` / `.vscode/launch.json` / `languages.toml` 模板）的集成验证。
//!
//! 解析/合并/变量替换的细节在 `helix-term/src/dap_config.rs` 的单测里；这里验证的是
//! **接到编辑器上**之后的行为：工作区怎么定位、当前语言怎么过滤、快照里 `source` 对不对。

use super::*;

/// 建一个"工作区"：有 `.helix/`（⇒ 被认作 workspace root）+ 一个 rust 文件。
fn workspace(dir: &std::path::Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir.join(".helix")).unwrap();
    let f = dir.join("main.rs");
    std::fs::write(&f, "fn main() {}\n").unwrap();
    f
}

#[tokio::test(flavor = "multi_thread")]
async fn debug_configs_merge_toml_json_and_lang() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let ws = dir.path();
    let file = workspace(ws);
    // .vscode/launch.json（JSONC：带注释）
    std::fs::create_dir_all(ws.join(".vscode"))?;
    std::fs::write(
        ws.join(".vscode/launch.json"),
        r#"{
          // VS Code 风格
          "version": "0.2.0",
          "configurations": [
            { "type": "lldb-dap", "request": "launch", "name": "run",
              "program": "/from-json", "args": ["--a"] },
            { "type": "lldb-dap", "request": "launch", "name": "json-only",
              "program": "${workspaceFolder}/target/debug/x" }
          ]
        }"#,
    )?;
    // .helix/launch.toml：同名覆盖 program
    std::fs::write(
        ws.join(".helix/launch.toml"),
        r#"
        [[config]]
        name = "run"
        program = "${workspaceFolder}/target/debug/app"
        cwd = "${workspaceFolder}"
        "#,
    )?;

    let app = AppBuilder::new().with_file(file, None).build()?;
    let (configs, warnings) = helix_term::commands::load_launch_configs(&app.editor);
    assert_eq!(warnings, Vec::<String>::new(), "两个文件都应能解析");

    let run = configs.iter().find(|c| c.name == "run").expect("run 配置");
    assert_eq!(run.source.as_str(), "toml", "同名 ⇒ 记成 toml");
    assert_eq!(
        run.args["program"],
        format!("{}/target/debug/app", ws.display()),
        "${{workspaceFolder}} 应已展开"
    );
    assert_eq!(run.args["args"][0], "--a", "未覆盖字段沿用 json");
    assert_eq!(run.adapter.as_deref(), Some("lldb-dap"));

    let json_only = configs
        .iter()
        .find(|c| c.name == "json-only")
        .expect("json 配置");
    assert_eq!(json_only.source.as_str(), "json");
    assert_eq!(
        json_only.args["program"],
        format!("{}/target/debug/x", ws.display())
    );

    // 语言模板兜底也在（lldb-dap 的 binary/attach 模板）
    assert!(
        configs.iter().any(|c| c.source.as_str() == "lang"),
        "languages.toml 模板应作为兜底出现: {configs:?}"
    );

    // 快照（`helix.dap.configs()` 的数据源）与之一致
    let json = helix_term::commands::dap_configs_json(&app.editor);
    assert!(json.contains("\"source\":\"toml\""), "{json}");
    assert!(json.contains("\"source\":\"json\""), "{json}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn debug_config_with_bad_variable_is_skipped_loudly() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let ws = dir.path();
    let file = workspace(ws);
    std::fs::write(
        ws.join(".helix/launch.toml"),
        r#"
        [[config]]
        name = "broken"
        program = "${nope}/x"

        [[config]]
        name = "ok"
        program = "/ok"
        "#,
    )?;

    let app = AppBuilder::new().with_file(file, None).build()?;
    let (configs, warnings) = helix_term::commands::load_launch_configs(&app.editor);
    assert!(
        !configs.iter().any(|c| c.name == "broken"),
        "坏变量那条应被跳过: {configs:?}"
    );
    assert!(configs.iter().any(|c| c.name == "ok"));
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("broken") && w.contains("nope")),
        "必须报出是哪个配置的哪个变量: {warnings:?}"
    );
    Ok(())
}

/// `:debug-start` 的 Picker 必须**同时**列出三级来源（这是"找得到"的关键：
/// 用户写进 `launch.toml` 的配置名要能在启动器里看见）。
///
/// 用 `render_rows`（渲染到 buffer 读文本）而不是进程内 pty —— 断言稳定，不依赖终端差分渲染。
#[tokio::test(flavor = "multi_thread")]
async fn debug_start_picker_lists_all_sources() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let ws = dir.path();
    let file = workspace(ws);
    std::fs::create_dir_all(ws.join(".vscode"))?;
    std::fs::write(
        ws.join(".vscode/launch.json"),
        r#"{"configurations":[{"type":"lldb-dap","request":"launch","name":"run with args","program":"/x"}]}"#,
    )?;
    std::fs::write(
        ws.join(".helix/launch.toml"),
        "[[config]]\nname = \"run\"\nprogram = \"/y\"\n",
    )?;

    let mut app = AppBuilder::new().with_file(file, None).build()?;
    // `<space>G l` = dap_launch（打开启动器 Picker）
    pump(&mut app, "<space>Gl").await?;
    let text = render_rows(&mut app, helix_view::graphics::Rect::new(0, 0, 120, 40)).join("\n");
    assert!(text.contains("run"), "toml 配置名应出现: {text}");
    assert!(text.contains("run with args"), "json 配置名应出现: {text}");
    assert!(text.contains("toml"), "来源标注 toml: {text}");
    assert!(text.contains("json"), "来源标注 json: {text}");
    assert!(text.contains("lang"), "语言模板兜底也应列出: {text}");
    Ok(())
}
