# 插件依赖解析实现计划(二期批次 3)

> **面向 AI 代理的工作者:** 必需子技能:使用 superpowers:subagent-driven-development(推荐)或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框(`- [ ]`)语法来跟踪进度。

**目标:** `plugin install` 读取插件的 `plugin.json` 依赖声明,递归安装 git 源依赖(先依赖后本体),循环检测 + 深度限制,失败中止。

**架构:** 纯函数层(`parse_plugin_json`、递归 `install_with_deps` + 循环/深度检测——可单测)+ typed.rs install 分支接入(本地/git 两分支都走依赖处理)。

**技术栈:** Rust(helix-term)、serde_json。

**规格:** `docs/superpowers/specs/2026-08-31-js-plugin-deps-design.md`(已批准)

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `helix-term/src/commands/plugin_manager.rs` | `PluginDeps` 解析、`install_with_deps` 递归(复用 git clone 逻辑)、循环/深度检测 |
| `helix-term/src/commands/typed.rs` | install 分支:clone/复制后调 `install_with_deps` 处理依赖(先依赖后本体) |
| `helix-term/tests/test/plugin_manager.rs` | 单测 + integration |

关键实现位置(执行时必读):

- **批次 1 已有**:`clone_to_vendor`/`name_from_url`/`git_head_commit`、ManifestEntry(commit/pinned)
- **批次 2 已有**:`git_fetch`/`git_head`/`git_origin_head`/`git_pull_ff`(依赖解析不直接用,但 install_with_deps 内部 clone 复用)
- **install 分支**(typed.rs plugin())：git-url 分支现在 clone→manifest→reload→load;改造为 clone→**install_with_deps(先依赖)**→本体 manifest→reload→load。本地分支同理(复制后读 plugin.json)
- **manifest 读改写**:`read_manifest`/`write_manifest`/`manifest_path` 已有

---

### 任务 1:纯函数层(plugin.json 解析 + 递归安装 + 循环/深度检测)

**文件:**
- 修改:`helix-term/src/commands/plugin_manager.rs`
- 测试:同文件 tests mod

- [ ] **步骤 1:写失败测试**

```rust
#[test]
fn parse_plugin_json_deps() {
    // 合法:deps 提取
    let deps = parse_plugin_json(r#"{"deps": [{"name": "a", "git": "https://g/a.git"}]}"#).unwrap();
    assert_eq!(deps.len(), 1);
    assert_eq!(deps[0].name, "a");
    // 无 deps 字段 → 空
    assert!(parse_plugin_json(r#"{}"#).unwrap().is_empty());
    // 坏 JSON → Err
    assert!(parse_plugin_json("not json").is_err());
    // 缺 name/git → Err
    assert!(parse_plugin_json(r#"{"deps": [{"name": "a"}]}"#).is_err());
}

#[test]
fn install_with_deps_cycle_detected() {
    // 构造:manifest 空,vendor 目录空;一个"假"依赖 url(不真 clone)
    // ——安装会真 clone!循环检测必须在 clone 前触发:依赖链 A → B → A
    // 单测用"已装跳过"短路 + 循环检测:第一次 install A(真 clone 一个本地临时 git 仓库),
    // 太重。**方案:循环/深度检测单独测(不 clone)**——提取纯函数 `check_cycle(stack, name)`:
    let mut stack = vec!["A".to_string()];
    assert!(check_cycle(&stack, "A")); // A 已在栈 → 循环
    stack.push("B".to_string());
    assert!(!check_cycle(&stack, "C"));
    assert!(check_cycle(&stack, "A"));
    assert!(!check_cycle(&stack, "B")); // B 在栈,但检测的是 name == 栈中任意?B 在 → 真循环
    // 修正语义:check_cycle 检测 name 是否已在栈中(任意位置)
    let mut s = vec!["A".to_string(), "B".to_string()];
    assert!(check_cycle(&s, "B"));
}

#[test]
fn depth_limit_enforced() {
    assert!(depth_ok(0, 10));
    assert!(depth_ok(9, 10));
    assert!(!depth_ok(10, 10)); // 超过 10 层 → 拒绝
}
```

预期:FAIL(函数未定义)。

- [ ] **步骤 2:运行测试确认失败**

运行:`cargo test -p helix-term parse_plugin_json_deps 2>&1 | tail -8`

- [ ] **步骤 3:实现**

```rust
#[derive(Debug, Clone, serde::Deserialize)]
pub struct PluginDep {
    pub name: String,
    pub git: String,
}

/// plugin.json 解析:deps 可选;坏 JSON/缺字段 → Err;无文件 → Ok(空)
pub fn parse_plugin_json(raw: &str) -> anyhow::Result<Vec<PluginDep>> {
    #[derive(serde::Deserialize)]
    struct PluginJson {
        #[serde(default)]
        deps: Vec<PluginDep>,
    }
    let parsed: PluginJson = serde_json::from_str(raw)?;
    // 缺 name/git:serde 自动 Err(非 Option 字段缺失即错)
    Ok(parsed.deps)
}

/// 循环检测:name 已在栈(任意位置)→ 循环
pub fn check_cycle(stack: &[String], name: &str) -> bool {
    stack.iter().any(|s| s == name)
}

/// 深度限制:depth(当前层数,0 = 根)<= max 放行
pub fn depth_ok(depth: usize, max: usize) -> bool {
    depth < max
}

/// 递归安装依赖。visited:已装(manifest 键)集合;stack:当前依赖链(循环检测)。
/// 已装跳过;循环/超深 → Err;clone 失败 → Err。
/// 返回 Err 时已装的依赖保留(不回滚)。
#[allow(clippy::too_many_arguments)]
pub fn install_with_deps(
    name: &str,
    git_url: &str,
    plugins_dir: &Path,
    manifest: &mut Manifest,
    visited: &mut Vec<String>,
    stack: &mut Vec<String>,
    depth: usize,
) -> anyhow::Result<()> {
    if !depth_ok(depth, 10) {
        return Err(anyhow!("dependency depth exceeds 10 at '{name}'"));
    }
    if check_cycle(stack, name) {
        return Err(anyhow!(
            "circular dependency: {} -> {name}",
            stack.join(" -> ")
        ));
    }
    if manifest.contains_key(name) || visited.contains(&name.to_string()) {
        return Ok(()); // 已装/处理中 → 跳过
    }
    // 先装依赖
    let vendor_dir = plugins_dir.join("vendor");
    let target = vendor_dir.join(name);
    if !target.exists() {
        std::fs::create_dir_all(&vendor_dir)?;
        clone_to_vendor(git_url, &target)?;
    }
    // 读本体的 plugin.json(可能在 clone 里)
    let deps = match std::fs::read_to_string(target.join("plugin.json")) {
        Ok(raw) => parse_plugin_json(&raw)?,
        Err(_) => Vec::new(), // 无 plugin.json = 无依赖
    };
    stack.push(name.to_string());
    for dep in &deps {
        install_with_deps(&dep.name, &dep.git, plugins_dir, manifest, visited, stack, depth + 1)?;
    }
    stack.pop();
    // 本体记入 manifest(依赖全部成功后)
    manifest.insert(
        name.to_string(),
        ManifestEntry {
            source: git_url.to_string(),
            kind: "git".into(),
            installed_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs().to_string())
                .unwrap_or_default(),
            commit: git_head_commit(&target),
            pinned: false,
            files: vec![format!("vendor/{name}/")],
        },
    );
    visited.push(name.to_string());
    Ok(())
}
```

**语义澄清**:visited 防重复处理(同一依赖在多个插件里声明),stack 防循环。manifest 写入在依赖成功后。注意:循环检测在 stack push 前检查(栈里含 name 即循环)。

- [ ] **步骤 4:运行测试确认通过**

运行:`cargo test -p helix-term parse_plugin_json_deps install_with_deps_cycle_detected depth_limit_enforced 2>&1 | tail -8` + `cargo build -p helix-term`
预期:PASS + 编译过。

- [ ] **步骤 5:Commit**

```bash
git add helix-term/src/commands/plugin_manager.rs
git commit -m "feat(term): 插件依赖解析纯函数——plugin.json 解析/循环检测/深度限制/递归安装(先依赖后本体)"
```

---

### 任务 2:install 分支接入 + integration

**文件:**
- 修改:`helix-term/src/commands/typed.rs`(git-url 分支 + 本地分支接 install_with_deps)
- 修改:`helix-term/tests/test/plugin_manager.rs`
- 测试:同文件

- [ ] **步骤 1:确认测试策略(无新 integration)**

递归安装/循环/深度/解析全部由任务 1 纯函数单测覆盖;真实递归需要网络或本地 git 仓库构造(重、环境依赖)——**本任务不加新 integration 测试**。现有 plugin_manager integration(4 个)回归即可。若回归发现 install 行为变化,补断言。

- [ ] **步骤 2:实现 install 分支接入**

git-url 分支(clone 后):

```rust
// 依赖安装(先依赖后本体):install_with_deps 负责 clone 依赖 + 本体 manifest
let plugins_dir = helix_loader::config_dir().join("plugins");
let mut manifest = plugin_manager::read_manifest(&plugin_manager::manifest_path())?;
let mut visited = Vec::new();
let mut stack = Vec::new();
plugin_manager::install_with_deps(
    &name, arg, &plugins_dir, &mut manifest, &mut visited, &mut stack, 0,
)?;
plugin_manager::write_manifest(&plugin_manager::manifest_path(), &manifest)?;
```

**注意**:install_with_deps 自己 clone 本体(若 target 不存在)并写 manifest——替换现有 git 分支的 clone_to_vendor + manifest insert 代码,避免重复 clone。现有分支的"target 已存在报错"逻辑保留在 install_with_deps 前?install_with_deps 对已装(manifest)跳过——同名已装检查放前面(现有逻辑)。

本地分支:复制后读 features/<name>/plugin.json 的 deps 并递归安装(复用 install_with_deps,但本体是 local 已装,调用时 manifest 已 insert 本体 → 依赖递归时 visited 含本体?——**本地分支简化:读完 plugin.json 后,把 deps 交给 install_with_deps 逐个安装(本体不入其管理,只装依赖)**——实现时提取"装单个依赖"的辅助或直接循环调 install_with_deps(用假 url?不行)。**方案:本地分支循环调用 install_with_deps,传入本体已插入的 manifest(依赖会检查 manifest.contains_key(本体名)?? 依赖名 ≠ 本体名,OK);visited 预置本体名防循环(依赖链回到本体时检测)**。

- [ ] **步骤 3:测试转绿**

运行:`cargo test -p helix-term --lib plugin_manager` + `cargo test -p helix-term --features integration --test integration plugin_manager` + `cargo build -p helix-term`
预期:PASS。

- [ ] **步骤 4:Commit**

```bash
git add helix-term/src/commands/typed.rs helix-term/tests/test/plugin_manager.rs
git commit -m "feat(term): plugin install 接入依赖解析(先依赖后本体,本地/git 源,循环报错)"
```

---

## 收尾

- [ ] `cargo fmt --all --check`(本批次文件)+ `cargo clippy -p helix-term 2>&1 | tail -3` + `cargo test -p helix-term --lib`
- [ ] 手动验证(用户):构造两个本地 git 仓库互依赖 → install 报循环;一个依赖一个 → 先装依赖
- [ ] 交接文档 `docs/superpowers/handoff/2026-08-31-js-plugin-deps.md`(简短)
- [ ] Commit 交接
