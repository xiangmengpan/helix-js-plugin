# 插件 update + pin 实现计划(二期批次 2)

> **面向 AI 代理的工作者:** 必需子技能:使用 superpowers:subagent-driven-development(推荐)或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框(`- [ ]`)语法来跟踪进度。

**目标:** `plugin update [name|all]`(git 源 fetch + 对比 + ff-only pull,失败保留旧版,pinned/local 跳过)+ `plugin pin/unpin` + status pinned 计数。

**架构:** git 操作纯函数(`git_fetch`/`git_head`/`git_head_origin`/`git_pull_ff`,std::process::Command)+ update 汇总逻辑(逐条目,失败继续)+ typed.rs 命令分支。

**技术栈:** Rust(helix-term)、std::process::Command。

**规格:** `docs/superpowers/specs/2026-08-31-js-plugin-update-design.md`(已批准)

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `helix-term/src/commands/plugin_manager.rs` | git fetch/head 对比/ff pull 纯函数;update_plugin + pin/unpin 逻辑 |
| `helix-term/src/commands/typed.rs` | plugin() 加 update/pin/unpin 分支 + status 增强 |
| `helix-term/tests/test/plugin_manager.rs` | 单测 + integration(无副作用路径) |

关键实现位置(执行时必读):

- **批次 1 已有**:`git_head_commit`(plugin_manager.rs)、`is_git_url`/`name_from_url`/`clone_to_vendor`、ManifestEntry(commit/pinned)
- **git 命令模式**:照 `git_head_commit` 的 `Command::new("git").args(["-C"]).arg(dir)...` 模式
- **plugin() 函数**(typed.rs):现有分支 list/install/remove/reload/status——加 update/pin/unpin
- **manifest 读改写**:`read_manifest`/`write_manifest`/`manifest_path` 已有

---

### 任务 1:纯函数层(git fetch/对比/ff pull + update/pin 逻辑)

**文件:**
- 修改:`helix-term/src/commands/plugin_manager.rs`
- 测试:同文件 tests mod

- [ ] **步骤 1:写失败测试(纯函数 + 逻辑)**

```rust
#[test]
fn update_skips_local_and_pinned() {
    // 构造 manifest:local 条目 + pinned git 条目
    // update_all(entries) → 返回 (updated, up_to_date, skipped, failed)
    // local + pinned → 都 skipped,无 git 命令执行
    let dir = tempfile::tempdir().unwrap();
    let entries = vec![
        ("loc".to_string(), ManifestEntry { source: "./x".into(), kind: "local".into(), installed_at: "t".into(), commit: None, pinned: false, files: vec!["features/x.js".into()] }),
        ("pin".to_string(), ManifestEntry { source: "https://g/p.git".into(), kind: "git".into(), installed_at: "t".into(), commit: Some("a".into()), pinned: true, files: vec!["vendor/p/".into()] }),
    ];
    let (updated, up, skipped, failed) = update_entries(&entries, &dir.path().join("plugins"));
    assert_eq!(updated, 0);
    assert_eq!(up, 0);
    assert_eq!(skipped, 2); // local + pinned
    assert_eq!(failed, 0);
}

#[test]
fn update_missing_dir_fails_entry() {
    // git 条目但 vendor/<name> 目录不存在 → failed
    let dir = tempfile::tempdir().unwrap();
    let entries = vec![
        ("gone".to_string(), ManifestEntry { source: "https://g/g.git".into(), kind: "git".into(), installed_at: "t".into(), commit: Some("a".into()), pinned: false, files: vec!["vendor/gone/".into()] }),
    ];
    let (updated, _up, skipped, failed) = update_entries(&entries, &dir.path().join("plugins"));
    assert_eq!(updated, 0);
    assert_eq!(failed, 1);
}
```

预期:FAIL(`update_entries` 未定义)。

- [ ] **步骤 2:运行测试确认失败**

运行:`cargo test -p helix-term update_skips_local_and_pinned 2>&1 | tail -8`

- [ ] **步骤 3:实现**

```rust
/// git fetch origin;失败 → Err
pub fn git_fetch(dir: &Path) -> anyhow::Result<()> {
    let status = std::process::Command::new("git")
        .args(["-C"]).arg(dir).args(["fetch", "origin"])
        .status()
        .map_err(|e| anyhow!("git fetch failed: {e}"))?;
    if status.success() { Ok(()) } else { Err(anyhow!("git fetch failed with {status}")) }
}

/// 当前 HEAD;失败 → Err
pub fn git_head(dir: &Path) -> anyhow::Result<String> { /* 照 git_head_commit,但 Err 化 */ }

/// origin/HEAD;失败 → Err(可能无远端跟踪,报错)
pub fn git_origin_head(dir: &Path) -> anyhow::Result<String> {
    let out = std::process::Command::new("git")
        .args(["-C"]).arg(dir).args(["rev-parse", "origin/HEAD"])
        .output().map_err(|e| anyhow!("git rev-parse origin/HEAD failed: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(anyhow!("git rev-parse origin/HEAD failed"))
    }
}

/// ff-only pull;失败(冲突/非快进)→ Err,工作树保留旧版
pub fn git_pull_ff(dir: &Path) -> anyhow::Result<()> {
    let status = std::process::Command::new("git")
        .args(["-C"]).arg(dir).args(["pull", "--ff-only"])
        .status().map_err(|e| anyhow!("git pull failed: {e}"))?;
    if status.success() { Ok(()) } else { Err(anyhow!("git pull --ff-only failed with {status}")) }
}

/// 更新一批条目。返回 (updated, up_to_date, skipped, failed)。
/// local/pinned 跳过;git 目录缺失 failed;fetch 失败 failed;head 相同 up_to_date;
/// 不同 → ff pull,成功 updated(调用方更新 manifest commit)。
/// 注意:本函数不改 manifest——只返回统计 + 需要更新 commit 的条目列表。
pub fn update_entries(
    entries: &[(String, ManifestEntry)],
    plugins_dir: &Path,
) -> (usize, usize, usize, usize) {
    let mut updated = 0; let mut up = 0; let mut skipped = 0; let mut failed = 0;
    for (name, e) in entries {
        if e.kind != "git" || e.pinned {
            skipped += 1;
            continue;
        }
        let dir = plugins_dir.join("vendor").join(name);
        if !dir.is_dir() {
            failed += 1;
            continue;
        }
        match (git_fetch(&dir), git_head(&dir), git_origin_head(&dir)) {
            (Ok(()), Ok(local), Ok(origin)) if local == origin => up += 1,
            (Ok(()), Ok(_), Ok(_)) => match git_pull_ff(&dir) {
                Ok(()) => updated += 1,
                Err(_) => failed += 1,
            },
            _ => failed += 1,
        }
    }
    (updated, up, skipped, failed)
}

/// 更新 manifest 中已更新条目的 commit(update 成功后调用)。
pub fn refresh_commits(
    manifest: &mut Manifest,
    plugins_dir: &Path,
    names: &[String],
) {
    for n in names {
        if let Some(e) = manifest.get_mut(n) {
            if let Ok(h) = git_head(&plugins_dir.join("vendor").join(n)) {
                e.commit = Some(h);
            }
        }
    }
}
```

**注意**:update_entries 与 refresh_commits 分离(纯函数不改 manifest;命令层调 refresh_commits 写回)——单测方便。命令层需要"哪些条目 updated 了"——update_entries 只返回计数。**调整**:返回 `(Vec<String> updated_names, usize up, usize skipped, usize failed)`——updated_names 给 refresh_commits 用。测试断言相应改。

- [ ] **步骤 4:运行测试确认通过**

运行:`cargo test -p helix-term update_skips_local_and_pinned update_missing_dir_fails_entry 2>&1 | tail -8` + `cargo build -p helix-term`
预期:PASS + 编译过。

- [ ] **步骤 5:Commit**

```bash
git add helix-term/src/commands/plugin_manager.rs
git commit -m "feat(term): 插件 update 纯函数——git fetch/head 对比/ff pull + 跳过与失败统计"
```

---

### 任务 2:命令分支(update/pin/unpin + status 增强)

**文件:**
- 修改:`helix-term/src/commands/typed.rs`
- 修改:`helix-term/tests/test/plugin_manager.rs`
- 测试:同文件

- [ ] **步骤 1:写失败测试(integration,无副作用)**

```rust
#[tokio::test(flavor = "multi_thread")]
async fn plugin_pin_uninstalled_reports() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n")?;
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (
                Some(":plugin pin nope<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(status.as_ref().contains("not installed"), "got: {status}");
                }),
            ),
            (
                Some(":plugin unpin nope<ret>"),
                Some(&|app| {
                    let (status, _) = app.editor.get_status().unwrap();
                    assert!(status.as_ref().contains("not installed"), "got: {status}");
                }),
            ),
        ],
        false,
    )
    .await?;
    Ok(())
}
```

预期:FAIL(pin/unpin 未注册分支,命令报错或行为不符)。

- [ ] **步骤 2:运行测试确认失败**

运行:`cargo test -p helix-term --features integration --test integration plugin_manager 2>&1 | tail -10`

- [ ] **步骤 3:实现命令**

typed.rs plugin() 加分支:

```rust
"update" => {
    let target = args.get(1).cloned(); // None = all
    let plugins_dir = helix_loader::config_dir().join("plugins");
    let path = plugin_manager::manifest_path();
    let mut manifest = plugin_manager::read_manifest(&path)?;
    let entries: Vec<(String, plugin_manager::ManifestEntry)> = match &target {
        Some(name) => manifest.get(name).map(|e| (name.clone(), e.clone())).into_iter().collect(),
        None => manifest.clone().into_iter().collect(),
    };
    if entries.is_empty() {
        cx.editor.set_status("nothing to update (no git plugins installed)");
        return Ok(());
    }
    let (updated_names, up, skipped, failed) = plugin_manager::update_entries(&entries, &plugins_dir);
    if !updated_names.is_empty() {
        plugin_manager::refresh_commits(&mut manifest, &plugins_dir, &updated_names);
        plugin_manager::write_manifest(&path, &manifest)?;
    }
    cx.editor.set_status(format!(
        "updated {}, up to date {}, skipped {}, failed {}",
        updated_names.len(), up, skipped, failed
    ));
}

"pin" | "unpin" => {
    let Some(name) = args.get(1) else {
        return Err(anyhow!("usage: plugin {sub} <name>"));
    };
    let path = plugin_manager::manifest_path();
    let mut manifest = plugin_manager::read_manifest(&path)?;
    let Some(entry) = manifest.get_mut(name) else {
        return Err(anyhow!("plugin {sub}: '{name}' not installed"));
    };
    if entry.kind != "git" {
        return Err(anyhow!("plugin {sub}: only git plugins can be pinned"));
    }
    entry.pinned = sub == "pin";
    plugin_manager::write_manifest(&path, &manifest)?;
    cx.editor.set_status(format!("{sub}ned '{name}'"));
}
```

status 分支增强(加 K git, L pinned):

```rust
"status" => {
    let plugin_dir = helix_loader::config_dir().join("plugins");
    let manifest = plugin_manager::read_manifest(&plugin_manager::manifest_path())?;
    let git_count = manifest.values().filter(|e| e.kind == "git").count();
    let pinned_count = manifest.values().filter(|e| e.pinned).count();
    cx.editor.set_status(format!(
        "{} plugins loaded from {}, {} installed ({} git, {} pinned)",
        helix_js::loaded_scripts().len(),
        plugin_dir.display(),
        manifest.len(),
        git_count,
        pinned_count
    ));
}
```

- [ ] **步骤 4:测试转绿**

运行:`cargo test -p helix-term --features integration --test integration plugin_manager 2>&1 | tail -10` + `cargo test -p helix-term --lib plugin_manager` + `cargo build -p helix-term`
预期:PASS。现有 status 断言(plugin_list_and_status 测 contains("installed"))——新文案仍含 "installed",不破坏。

- [ ] **步骤 5:Commit**

```bash
git add helix-term/src/commands/typed.rs helix-term/tests/test/plugin_manager.rs
git commit -m "feat(term): plugin update/pin/unpin 命令 + status 带 git/pinned 计数"
```

---

## 收尾

- [ ] `cargo fmt --all --check`(本批次文件)+ `cargo clippy -p helix-term 2>&1 | tail -3` + `cargo test -p helix-term --lib`
- [ ] 手动验证(用户):装一个本地 git 仓库插件(如 `git clone` 到临时目录改 URL 不行——用任意公开小仓库),`:plugin update` 看输出;`:plugin pin <name>` 后再 update 跳过;真实远端更新验证可选
- [ ] 交接文档 `docs/superpowers/handoff/2026-08-31-js-plugin-update.md`(简短)
- [ ] Commit 交接
