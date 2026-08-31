# 插件 git 源 install 实现计划(二期批次 1)

> **面向 AI 代理的工作者:** 必需子技能:使用 superpowers:subagent-driven-development(推荐)或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框(`- [ ]`)语法来跟踪进度。

**目标:** `plugin install <git-url>` 支持 git 仓库源——clone 到 `plugins/vendor/<name>`,manifest 记 commit hash,装后自动加载。

**架构:** 纯函数层(url 识别/name 提取/commit 获取——可单测)+ `git clone` 同步执行(一期 install 也是同步 fs 操作,大仓库冻结可接受);manifest 加 `commit`/`pinned` 字段(serde default 兼容一期条目)。

**技术栈:** Rust(helix-term)、std::process::Command(系统 git CLI)。

**规格:** `docs/superpowers/specs/2026-08-31-js-plugin-git-install-design.md`(已批准)

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `helix-term/src/commands/plugin_manager.rs` | `is_git_url`/`name_from_url`/`clone_to_vendor`/`git_head_commit`/ManifestEntry 扩展 |
| `helix-term/src/commands/typed.rs` | plugin install 分支:识别 → clone → manifest → 加载 |
| `helix-term/tests/test/plugin_manager.rs` | integration(无副作用路径) |

关键实现位置(执行时必读):

- **一期 install 分支**(typed.rs,plugin() 函数内):现有 `if !src.exists()` 报错路径——改为"路径不存在时尝试 git-url"分支
- **ManifestEntry**(plugin_manager.rs):现字段 source/kind/installed_at/files——加 `commit: Option<String>` + `pinned: bool`(`#[serde(default)]`)
- **一期装后加载**(typed.rs):`entry.is_file()` 判断单文件/目录 index.js——git 源入口 `vendor/<name>/index.js` 走同一逻辑(相对名 `vendor/<name>/index.js`)
- **git 命令**:`std::process::Command::new("git").args(["clone", url, dir])`;commit 用 `git -C dir rev-parse HEAD`

---

### 任务 1:纯函数层(url 识别/name 提取/commit/clone)

**文件:**
- 修改:`helix-term/src/commands/plugin_manager.rs`
- 测试:同文件 tests mod

- [ ] **步骤 1:写失败测试**

```rust
#[test]
fn url_identification_and_name() {
    assert!(is_git_url("https://github.com/foo/bar.git"));
    assert!(is_git_url("git@github.com:foo/bar.git"));
    assert!(is_git_url("https://github.com/foo/bar")); // 无 .git 也是 url(含 ://)
    assert!(!is_git_url("./local/plugin.js"));
    assert!(!is_git_url("/abs/path/plugin"));
    assert!(!is_git_url("plain-name")); // 无法识别 → 非 url 非路径 → 调用方报错
    assert_eq!(name_from_url("https://github.com/foo/bar.git"), "bar");
    assert_eq!(name_from_url("git@github.com:foo/bar.git"), "bar");
    assert_eq!(name_from_url("https://github.com/foo/baz"), "baz");
}

#[test]
fn clone_failure_returns_err() {
    // 假 url(git 不存在/网络不通)→ Err;不 panic
    let dir = std::env::temp_dir().join(format!("hx_clone_fail_{}", std::process::id()));
    assert!(clone_to_vendor("https://127.0.0.1:1/nope/nope.git", &dir).is_err());
    std::fs::remove_dir_all(&dir).ok();
}
```

(注意:clone 失败测试会真跑 git clone 到本地端口 1——失败快;若 CI 无 git 则 `#[ignore]` 标注或查 `which git`,计划执行时看环境。)

预期:FAIL(函数未定义)。

- [ ] **步骤 2:运行测试确认失败**

运行:`cargo test -p helix-term url_identification_and_name 2>&1 | tail -8`

- [ ] **步骤 3:实现**

```rust
/// git url 识别:含 :// 或 git@ 前缀或 .git 后缀
pub fn is_git_url(arg: &str) -> bool {
    arg.contains("://") || arg.starts_with("git@") || arg.ends_with(".git")
}

/// url → 插件名:basename 去 .git 后缀
pub fn name_from_url(url: &str) -> String {
    let base = url.rsplit('/').next().unwrap_or(url);
    base.strip_suffix(".git").unwrap_or(base).to_string()
}

/// git clone 到 vendor/<name>;失败清理半成品目录
pub fn clone_to_vendor(url: &str, dst: &std::path::Path) -> anyhow::Result<()> {
    let status = std::process::Command::new("git")
        .args(["clone", url])
        .arg(dst)
        .status()
        .map_err(|e| anyhow!("git clone failed (is git installed?): {e}"))?;
    if !status.success() {
        let _ = std::fs::remove_dir_all(dst);
        return Err(anyhow!("git clone '{url}' failed with {status}"));
    }
    Ok(())
}

/// 当前 commit hash;失败 → None(warn 由调用方或此处 log)
pub fn git_head_commit(dir: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["-C"])
        .arg(dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        None
    }
}
```

ManifestEntry 加字段:

```rust
#[serde(default)]
pub commit: Option<String>,
#[serde(default)]
pub pinned: bool,
```

- [ ] **步骤 4:运行测试确认通过**

运行:`cargo test -p helix-term url_identification_and_name clone_failure_returns_err 2>&1 | tail -8` + `cargo build -p helix-term`
预期:PASS + 编译过。

- [ ] **步骤 5:Commit**

```bash
git add helix-term/src/commands/plugin_manager.rs
git commit -m "feat(term): 插件 git 源纯函数——url 识别/name 提取/clone/commit 获取 + manifest 扩展"
```

---

### 任务 2:install 分支接线 + integration

**文件:**
- 修改:`helix-term/src/commands/typed.rs`(plugin install 分支)
- 修改:`helix-term/tests/test/plugin_manager.rs`(integration 无副作用路径)
- 测试:同文件

- [ ] **步骤 1:写失败测试(integration,无副作用)**

```rust
#[tokio::test(flavor = "multi_thread")]
async fn plugin_install_invalid_arg_reports() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n")?;
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            // 无法识别的参数(非路径非 url)→ 报错,不崩
            (Some(":plugin install plain-name<ret>"), Some(&|app| {
                let (status, _) = app.editor.get_status().unwrap();
                assert!(status.as_ref().contains("invalid"));
            })),
        ],
        false,
    )
    .await?;
    Ok(())
}
```

预期:FAIL(现有 install 分支对 plain-name 走 `!src.exists()` 报 "not found"——若消息不同则调整断言;核心是"报错不崩")。

- [ ] **步骤 2:运行测试确认失败**

运行:`cargo test -p helix-term --features integration --test integration plugin_manager 2>&1 | tail -10`

- [ ] **步骤 3:实现 install 分支**

typed.rs plugin() 的 install 分支改造(一期代码基础上):

```rust
"install" => {
    let Some(arg) = args.get(1) else {
        return Err(anyhow!("usage: plugin install <path|git-url>"));
    };
    let src = std::path::Path::new(arg);
    let plugin_manager = ...; // use
    // git-url 分支
    if plugin_manager::is_git_url(arg) {
        let name = plugin_manager::name_from_url(arg);
        let vendor_dir = helix_loader::config_dir().join("plugins").join("vendor");
        let target = vendor_dir.join(&name);
        if target.exists() {
            return Err(anyhow!("plugin install: '{name}' already installed, use :plugin remove first"));
        }
        std::fs::create_dir_all(&vendor_dir)?;
        plugin_manager::clone_to_vendor(arg, &target)?;
        let commit = plugin_manager::git_head_commit(&target);
        let mut manifest = plugin_manager::read_manifest(&plugin_manager::manifest_path())?;
        manifest.insert(name.clone(), plugin_manager::ManifestEntry {
            source: arg.to_string(),
            kind: "git".into(),
            installed_at: /* 一期同款 epoch 秒 */,
            commit,
            pinned: false,
            files: vec![format!("vendor/{name}/")],
        });
        plugin_manager::write_manifest(&plugin_manager::manifest_path(), &manifest)?;
        cx.editor.set_status(format!("installed '{name}', reloading..."));
        reload_plugins(cx)?;
        // 装后加载:vendor/<name>/index.js(照一期逻辑,entry = target.join("index.js"),相对名 vendor/<name>/index.js)
        // ……(复用一期装后加载代码块,入口路径换成 vendor/<name>/index.js)
        return Ok(());
    }
    // 一期本地路径分支(不变):!src.exists() 报错文案改 "invalid path or git url: '{arg}'"
    if !src.exists() {
        return Err(anyhow!("plugin install: invalid path or git url '{arg}'"));
    }
    // ……一期其余逻辑不变
}
```

(装后加载代码块与一期重复——考虑提取小 helper `load_installed_entry(plugins_dir, rel_name, abs_entry)`,一期/二期共用,避免复制。实现时若提取,保持一期行为不变。)

- [ ] **步骤 4:测试转绿**

运行:`cargo test -p helix-term --features integration --test integration plugin_manager 2>&1 | tail -10` + `cargo test -p helix-term --lib plugin_manager` + `cargo build -p helix-term`
预期:PASS。

- [ ] **步骤 5:Commit**

```bash
git add helix-term/src/commands/typed.rs helix-term/tests/test/plugin_manager.rs
git commit -m "feat(term): plugin install 支持 git-url(git CLI clone 到 vendor/ + manifest 记 commit + 装后加载)"
```

---

## 收尾

- [ ] `cargo fmt --all --check`(本批次文件)+ `cargo clippy -p helix-term 2>&1 | tail -3`(既有 warning 除外)+ `cargo test -p helix-term --lib`
- [ ] 手动验证(用户):`hx` 里 `:plugin install https://github.com/helix-editor/helix.git`(小仓库或任意公开仓库,验证 clone + manifest commit + 加载;用完后 `:plugin remove <name>`);无网络环境跳过
- [ ] 交接文档 `docs/superpowers/handoff/2026-08-31-js-plugin-git-install.md`(简短)
- [ ] Commit 交接
