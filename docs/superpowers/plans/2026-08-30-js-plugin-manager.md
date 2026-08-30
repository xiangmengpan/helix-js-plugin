# 插件管理器最小版实现计划

> **面向 AI 代理的工作者:** 必需子技能:使用 superpowers:subagent-driven-development(推荐)或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框(`- [ ]`)语法来跟踪进度。

**目标:** `manifest.json` 追踪已装插件 + `:plugin-install`(本地路径)/`:plugin-installed`/`:plugin-remove` 三命令。

**架构:** 纯函数层(manifest 读写、复制/删除、路径映射——可单测)+ 命令层(typed.rs 照 plugin-load 注册)。install 后自动 `reload_plugins`(复用现有)。

**技术栈:** Rust(helix-term)、serde_json(已依赖)。

**规格:** `docs/superpowers/specs/2026-08-30-js-plugin-manager-design.md`(已批准)

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `helix-term/src/commands/typed.rs` | 三个命令 + 注册(TypableCommand 数组,照 plugin-load 4293 旁) |
| `helix-term/src/commands/plugin_manager.rs` | **新建**:manifest 读写 + install 复制 + remove 删除(纯函数,可单测) |
| `helix-term/tests/test/plugin_manager.rs` | **新建**:integration |
| `helix-term/tests/integration.rs` | mod 注册 |

关键实现位置(执行时必读):

- **注册**:typed.rs `TypableCommand` 静态数组(4293 plugin-reload / 4301 plugin-load 旁加三个);`Signature { positionals: (1, Some(1)) }` 照 plugin-load;completer 照 plugin-load 的 `filename`
- **reload 复用**:`reload_plugins(cx)`(typed.rs:5247)是 `:plugin-reload` 与 `:plugin reload` 共用逻辑——install 成功后直接调它
- **插件目录**:`helix_loader::config_dir().join("plugins")`(typed.rs:5305 现有用法);features 子目录 = `plugins/features`
- **manifest 路径**:`plugins/manifest.json`
- **plugin-load 参照**(5406):参数解析 `args.first()`、错误风格 `anyhow!(...)`、apply_ui_requests drain
- **复制**:std::fs::copy / 目录递归复制(手写 walk,参照 read_tree 的 walk 模式);不引第三方(避免依赖)

---

### 任务 1:纯函数层(manifest + install/remove 逻辑)

**文件:**
- 创建:`helix-term/src/commands/plugin_manager.rs`
- 修改:`helix-term/src/commands/mod.rs` 或 typed.rs 的 mod 声明(看 typed.rs 的 mod 组织,commands 模块怎么挂)
- 测试:`plugin_manager.rs` 底部 tests mod(纯函数)

- [ ] **步骤 1:写失败测试(manifest 读写 + 容错)**

```rust
#[test]
fn manifest_read_write_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("manifest.json");
    // 缺文件 → 空
    let m = read_manifest(&path).unwrap();
    assert!(m.is_empty());
    // 写读回
    let entry = ManifestEntry { source: "./x".into(), kind: "local".into(), installed_at: "t".into(), files: vec!["features/x.js".into()] };
    write_manifest(&path, &[("x".to_string(), entry.clone())]).unwrap();
    let m = read_manifest(&path).unwrap();
    assert_eq!(m.get("x").unwrap().source, "./x");
}

#[test]
fn manifest_bad_json_is_empty_not_panic() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("manifest.json");
    std::fs::write(&path, "not json {{{").unwrap();
    let m = read_manifest(&path).unwrap(); // 坏 JSON → 空,不崩
    assert!(m.is_empty());
}
```

(若 tempfile 不在 dev-deps,用 std::env::temp_dir + 唯一名,参照 tree_matches 测试模式。)

- [ ] **步骤 2:运行测试确认失败**

运行:`cargo test -p helix-term plugin_manager 2>&1 | tail -10`
预期:FAIL(模块/函数未定义)。

- [ ] **步骤 3:实现纯函数**

```rust
// helix-term/src/commands/plugin_manager.rs
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ManifestEntry {
    pub source: String,
    pub kind: String, // "local"(二期 "git")
    pub installed_at: String,
    pub files: Vec<String>, // 相对 plugins/ 的复制目标
}

pub type Manifest = HashMap<String, ManifestEntry>;

pub fn manifest_path() -> PathBuf {
    helix_loader::config_dir().join("plugins").join("manifest.json")
}

/// 读 manifest;缺文件/坏 JSON → 空(不崩);IO 错误 warn + 空
pub fn read_manifest(path: &Path) -> anyhow::Result<Manifest> {
    let Ok(raw) = fs::read_to_string(path) else { return Ok(Manifest::new()) };
    serde_json::from_str(&raw).map_err(|e| anyhow!("manifest parse failed: {e}"))
}

pub fn write_manifest(path: &Path, m: &Manifest) -> anyhow::Result<()> {
    let raw = serde_json::to_string_pretty(m)?;
    fs::write(path, raw).map_err(|e| anyhow!("write manifest: {e}"))
}

/// install 目标路径:文件 → features/<name>;目录 → features/<name>/
pub fn install_target(path: &Path) -> anyhow::Result<(String, PathBuf)> {
    let plugins = helix_loader::config_dir().join("plugins").join("features");
    let name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| anyhow!("invalid path: '{}'", path.display()))?;
    let target = plugins.join(name);
    Ok((name.to_string(), target))
}

/// 复制安装(文件或目录递归);中途失败 → 回滚已复制的部分
pub fn install_copy(src: &Path, dst: &Path) -> anyhow::Result<Vec<String>> {
    let mut copied = Vec::new();
    let result = (|| -> anyhow::Result<()> {
        if src.is_dir() {
            for entry in walkdir(src)? {
                let rel = entry.strip_prefix(src)?;
                let to = dst.join(rel);
                if entry.is_dir() {
                    fs::create_dir_all(&to)?;
                } else {
                    if let Some(p) = to.parent() { fs::create_dir_all(p)?; }
                    fs::copy(&entry, &to)?;
                }
                copied.push(format!("features/{}/{}", dst.file_name().unwrap().to_string_lossy(), rel.to_string_lossy()));
            }
        } else {
            fs::copy(src, dst)?;
            copied.push(format!("features/{}", dst.file_name().unwrap().to_string_lossy()));
        }
        Ok(())
    })();
    if result.is_err() {
        // 回滚:删已复制的
        for c in &copied {
            let _ = fs::remove_file(helix_loader::config_dir().join("plugins").join(c));
        }
    }
    result?;
    Ok(copied)
}

/// 删除安装的文件/目录(逐个,忽略不存在)
pub fn remove_files(files: &[String]) {
    let plugins = helix_loader::config_dir().join("plugins");
    for f in files {
        let p = plugins.join(f);
        if p.is_dir() { let _ = fs::remove_dir_all(p); }
        else { let _ = fs::remove_file(p); }
    }
}

fn walkdir(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d)? {
            let e = e?;
            let p = e.path();
            out.push(p.clone());
            if p.is_dir() { stack.push(p); }
        }
    }
    Ok(out)
}
```

(dev-deps 若无 tempfile,测试用 std::env::temp_dir + pid 唯一目录。)

- [ ] **步骤 4:运行测试确认通过**

运行:`cargo test -p helix-term plugin_manager 2>&1 | tail -10` + `cargo build -p helix-term`
预期:PASS + 编译过。

- [ ] **步骤 5:Commit**

```bash
git add helix-term/src/commands/plugin_manager.rs helix-term/src/commands/typed.rs helix-term/src/commands/mod.rs
git commit -m "feat(term): 插件管理器纯函数层——manifest 读写容错 + install 复制/回滚 + remove 删除"
```

---

### 任务 2:增强 :plugin 子命令(install/remove + status 后缀)+ integration

**文件:**
- 修改:`helix-term/src/commands/typed.rs`(plugin() 函数 5339 的 install/remove 分支 + status)
- 创建:`helix-term/tests/test/plugin_manager.rs`
- 修改:`helix-term/tests/integration.rs`

- [ ] **步骤 1:写失败测试(integration,无副作用路径)**

plugin_manager.rs(照 plugin_lsp.rs 模式;config_dir 是真实 ~/.config,写入类路径会污染——**只测无副作用路径**:`plugin list` 输出、`plugin status` 含 manifest 计数后缀;install/remove 写入逻辑靠单测覆盖)**:

```rust
#[tokio::test(flavor = "multi_thread")]
async fn plugin_list_and_status() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "hello\n")?;
    test_key_sequences(
        &mut AppBuilder::new().with_file(file, None).build()?,
        vec![
            (Some(":plugin list<ret>"), None),
            (Some(":plugin status<ret>"), Some(&|app| {
                let (status, _) = app.editor.get_status().unwrap();
                // status 含 manifest 计数后缀("installed in manifest")
                assert!(status.as_ref().contains("installed"));
            })),
        ],
        false,
    )
    .await?;
    Ok(())
}
```

预期:FAIL(status 无 manifest 后缀)。

- [ ] **步骤 2:运行测试确认失败**

运行:`cargo test -p helix-term --features integration --test integration plugin_manager 2>&1 | tail -15`
预期:FAIL(断言不满足)。

- [ ] **步骤 3:实现增强**

typed.rs 的 `plugin()` 函数(5339):

**install 分支**(替换现有 5351 单文件 copy):

```rust
"install" => {
    let Some(path) = args.get(1) else {
        return Err(anyhow!("usage: plugin install <path>"));
    };
    let src = std::path::Path::new(path);
    if !src.exists() {
        return Err(anyhow!("plugin install: '{path}' not found"));
    }
    let (name, target) = plugin_manager::install_target(src)?;
    if target.exists() {
        return Err(anyhow!("plugin install: '{name}' already installed, use :plugin remove first"));
    }
    let files = plugin_manager::install_copy(src, &target)?;
    let mut manifest = plugin_manager::read_manifest(&plugin_manager::manifest_path())?;
    manifest.insert(name.clone(), plugin_manager::ManifestEntry {
        source: path.to_string(),
        kind: "local".into(),
        installed_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs().to_string())
            .unwrap_or_default(),
        files,
    });
    plugin_manager::write_manifest(&plugin_manager::manifest_path(), &manifest)?;
    cx.editor.set_status(format!("installed '{name}', reloading..."));
    reload_plugins(cx)?;
}
```

**remove 分支**(替换现有 5367 直接删文件):

```rust
"remove" => {
    let Some(name) = args.get(1) else {
        return Err(anyhow!("usage: plugin remove <name>"));
    };
    let path = plugin_manager::manifest_path();
    let mut manifest = plugin_manager::read_manifest(&path)?;
    match manifest.remove(name) {
        Some(entry) => {
            plugin_manager::remove_files(&entry.files);
            plugin_manager::write_manifest(&path, &manifest)?;
            cx.editor.set_status(format!("removed '{name}'; update init.js if it loads it"));
        }
        // 兼容无 manifest 的旧安装:直接删文件
        None => {
            let plugin_dir = helix_loader::config_dir().join("plugins");
            std::fs::remove_file(plugin_dir.join(name))
                .map_err(|e| anyhow!("failed to remove '{name}': {e}"))?;
            cx.editor.set_status(format!("removed '{name}'; update init.js if it loads it"));
        }
    }
}
```

**status 分支**(加 manifest 计数后缀):

```rust
"status" => {
    let plugin_dir = helix_loader::config_dir().join("plugins");
    let installed = plugin_manager::read_manifest(&plugin_manager::manifest_path())?.len();
    cx.editor.set_status(format!(
        "{} plugins loaded from {}, {} installed in manifest",
        helix_js::loaded_scripts().len(),
        plugin_dir.display(),
        installed
    ));
}
```

(plugin_manager:: 模块 use 或全路径;install 分支旧的 `plugin_name_from_path`/copy/load_script_named 替换为增强版;`validate_plugin_name` 在 remove 分支保留或并入 manifest 查名)

- [ ] **步骤 4:测试转绿**

运行:`cargo test -p helix-term --features integration --test integration plugin_manager 2>&1 | tail -15` + `cargo test -p helix-term plugin_manager` + `cargo build -p helix-term`
预期:PASS。

- [ ] **步骤 5:Commit**

```bash
git add helix-term/src/commands/typed.rs helix-term/tests/test/plugin_manager.rs helix-term/tests/integration.rs
git commit -m "feat(term): :plugin install/remove 增强(manifest 追踪/目录/回滚/同名报错)+ status 计数后缀"
```

---

## 收尾

- [ ] `cargo fmt --all --check`(本批次文件)+ `cargo clippy -p helix-term 2>&1 | tail -3`(既有 warning 除外)+ `cargo test -p helix-term --lib`
- [ ] 手动验证(用户):`mkdir -p /tmp/plugtest && echo 'helix.register_command("pt", () => helix.echo("pt-ok"));' > /tmp/plugtest/pt.js`,hx 里 `:plugin-install /tmp/plugtest/pt.js` → 状态栏 installed → `:pt` 可执行;`:plugin-installed` 显示;`:plugin-remove pt` → 删文件,init.js 有 load 会提示
- [ ] 交接文档 `docs/superpowers/handoff/2026-08-30-js-plugin-manager.md`(简短)
- [ ] Commit 交接
