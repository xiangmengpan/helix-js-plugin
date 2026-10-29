//! Server Manager（mason 式）——LSP/DAP/linter/formatter 的安装/升级/卸载管理。
//!
//! 设计(见 docs/superpowers/specs/2026-09-05-server-manager-design.md):
//! - 注册表:内置 ServerSpec 常量 + config 扩展(T4)
//! - 受管目录 `~/.local/share/helix/managed/<name>`(T1 默认;T4 可配)
//! - archive 配方:mirror URL 改写 + sha256 校验 + tar.gz/zip 解压 + managed/bin 软链
//! - languages.toml 标记段自动写(T3)

use std::path::PathBuf;

pub(crate) mod archive;
pub(crate) mod download;
pub(crate) mod install;
pub(crate) mod langs_toml;
pub(crate) mod project;
pub(crate) mod registry;
pub(crate) mod version;

// 只再导出**模块外真的在用**的名字(commands/* 的 66 处引用);模块内部的辅助函数
// 由各子模块 `use super::<mod>::…` 直接拿,不在这里绕一圈。
pub(crate) use download::{clear_download_progress, op_lock, with_download_progress};
// 只在测试目标里存在的门禁钩子(见 download.rs 的 #[cfg(test)])
#[cfg(test)]
pub(crate) use download::download_progress_hook_set;
pub(crate) use install::{
    availability, clear_error, fetch_latest, file_lock, install, installed_version, is_installed,
    last_error, managed_root, record_error, remove, run_task, update,
};
pub(crate) use langs_toml::{
    apply_server_config, disk_server_manager_table, ignored_local, is_upgradable, lang_config_file,
    needs_version, rewrite_languages_toml, set_ignored,
};
pub(crate) use version::detect_version;

pub const MANAGED_HEADER: &str = "# >>> helix-managed";

pub const MANAGED_FOOTER: &str = "# <<< helix-managed";

/// 工具可用来源
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    /// 受管安装(managed/bin 软链;可升级/卸载)
    Managed,
    /// 本地 PATH 已有(非受管;直接使用;不能卸载)
    Local(PathBuf),
    /// 缺失
    Missing,
}

#[cfg(test)]
mod tests {
    // ⚠️ 取锁一律 `unwrap_or_else(|e| e.into_inner())`(**不用** `unwrap()`):
    // 一个用例 panicking 会 poison `ENV_LOCK`,其后每个 `.lock().unwrap()` 都以
    // `PoisonError` 失败 ⇒ **一次抖动被放大成 23 个失败**(2026-10-16 `cargo test --workspace`
    // 实测:本模块一个 env 相关用例失败 ⇒ 22 个连锁 PoisonError)。根因用例是既有的
    // 顺序/并发抖动(单独跑 `cargo test -p helix-term --lib` 3/3 绿),与本次改动无关;
    // 但"一次抖动放大成一片"必须消除。
    // 子模块的条目原样可用:它们都是 `pub(crate)`,而 tests 是叶子模块,glob 不会成环。
    use super::archive::*;
    use super::download::*;
    use super::install::*;
    use super::langs_toml::*;
    use super::version::*;
    use super::*;
    use std::{io::Write as _, path::Path};

    use anyhow::Result;

    /// **端到端**:逐平台资产配方的**真实安装**。
    /// 此前只测了解析/选择/消费点 —— 而"选了资产之后能不能装、bin 能不能链上"没测过。
    /// 用 `file://` 假源(现成的 `make_tar_gz`)避免依赖网络。
    #[test]
    fn archive_assets_install_end_to_end() {
        let _m = sm_lock();
        let dir = tempfile::tempdir().unwrap();
        let arc = dir.path().join("ra.tar.gz");
        // 假包结构:顶层 <bin>-1.0.0/<bin> → strip=1 后落在 <out>/<bin>
        make_tar_gz(&arc, "asset-ra").unwrap();
        let target = registry::platform_targets()
            .first()
            .copied()
            .unwrap_or("linux_x64_gnu");
        let cfg = format!(
            r#"
[server-manager.registry.asset-ra]
kind = "lsp"
languages = ["rust"]
description = "d"
homepage = "h"
bin = "asset-ra"
version = "1.0.0"
[[server-manager.registry.asset-ra.asset]]
target = "{target}"
url = "file://{}"
"#,
            arc.display()
        );
        let tbl: toml::Table = toml::from_str(&cfg).unwrap();
        let sm = tbl
            .get("server-manager")
            .unwrap()
            .as_table()
            .unwrap()
            .clone();
        apply_server_config(Some(&sm)).unwrap();

        let spec = registry::get("asset-ra").expect("asset 配方可查");
        assert!(
            spec.is_installable(),
            "只有 asset[] 没有 url_template 的配方也应判为可装"
        );
        assert!(
            !needs_version(&spec),
            "本条资产 url 不含 {{version}} → 不需要版本"
        );

        // 用**已解析**的 spec 安装:并行测试里别的用例调用 `apply_server_config` 会改写
        // 进程级 EXT,`install(name)` 会在"查到配方"与"用配方"之间丢配方(D8 实测)。
        install_spec(&spec, "1.0.0").unwrap();
        assert!(
            managed_bin("asset-ra").exists(),
            "经逐平台资产安装后,bin 软链必须建成"
        );
    }

    /// 逐平台资产表(Mason `asset[]` 形状):解析 + 平台选择 + 两个消费点。
    ///
    /// 补这条测试的原因:能力先落地、测试后补 —— 当时只改了既有测试的构造,
    /// 解析/选择/消费点**三条都无覆盖**。
    #[test]
    fn archive_assets_per_platform() {
        let _m = sm_lock();
        let tbl: toml::Table = r#"
            [ra]
            kind = "lsp"
            languages = ["rust"]
            description = "d"
            homepage = "h"
            bin = "ra"
            needs-version = true   # asset url 也含 {version} ⇒ 与单模板同一条闸门
            [[ra.asset]]
            target = "linux_x64_musl"
            url = "https://e/{version}/ra-x86_64-unknown-linux-musl.tar.gz"
            [[ra.asset]]
            target = "linux_x64_gnu"
            url = "https://e/{version}/ra-x86_64-linux-gnu.tar.gz"
            bin = "nested/ra"
        "#
        .parse()
        .unwrap();
        let specs = parse_registry_table(&tbl).unwrap();
        assert_eq!(specs.len(), 1, "应解析出 1 个配方");
        let spec = &specs[0].1;

        let registry::Install::Archive { assets, .. } = &spec.install else {
            panic!("带 asset[] 的配方应解析为 Archive");
        };
        assert_eq!(assets.len(), 2, "两个平台条目都要解析出来");
        assert_eq!(assets[0].target, "linux_x64_musl");
        assert_eq!(assets[1].bin_rel, "nested/ra", "条目自带 bin");
        assert_eq!(assets[0].bin_rel, "", "未写 bin → 空(安装时回落到外层)");

        // 消费点①:有资产 = 可装 —— 即使没有 url_template(这正是"只有 asset[]"的配方)
        assert!(spec.is_installable(), "assets 非空应判为可装");
        // 消费点②:资产 url 含 {version} → 需要用户给版本
        assert!(
            needs_version(spec),
            "资产 url 含 {{version}} 且未固定版本 → 需要版本"
        );

        // 平台选择:选中的必须是**当前平台**的候选之一(不能跨平台误选)
        if let Some(a) = registry::pick_asset(assets) {
            assert!(
                registry::platform_targets().contains(&a.target.as_str()),
                "选中的 target 必须是当前平台的候选之一,实得 {}",
                a.target
            );
        } else {
            // 当前平台不在候选里(如 CI 上的其它架构)→ 必须回落 None(由调用方用单模板)
            assert!(
                !assets
                    .iter()
                    .any(|a| registry::platform_targets().contains(&a.target.as_str())),
                "若候选里有当前平台,就不该选不出来"
            );
        }

        // 反证:不含 {version} 的资产不算"需要版本"
        let tbl2: toml::Table = r#"
            [fixed]
            kind = "lsp"
            languages = ["x"]
            description = "d"
            homepage = "h"
            bin = "x"
            [[fixed.asset]]
            target = "linux_x64_gnu"
            url = "https://e/latest/x.tar.gz"
        "#
        .parse()
        .unwrap();
        let specs2 = parse_registry_table(&tbl2).unwrap();
        assert!(
            !needs_version(&specs2[0].1),
            "资产 url 不含 {{version}} → 不需要版本"
        );
    }

    /// 写一个含 bin 的 tar.gz 到 path(供 file:// 假源)
    /// 单文件 zip(无 wrapper 目录 ⇒ strip=0)。store 模式下 zip **不带** unix mode,
    /// 正好模拟 terraform-ls / clojure-lsp 那类"只有 zip 产物"的配方。
    fn make_zip(path: &Path, bin_name: &str) -> Result<String> {
        use std::io::Write as _;
        std::fs::create_dir_all(path.parent().unwrap())?;
        let script = format!("#!/bin/sh\necho '{bin_name} 1.0.0'\n");
        let f = std::fs::File::create(path)?;
        let mut zw = zip::ZipWriter::new(f);
        zw.start_file::<_, ()>(bin_name, zip::write::SimpleFileOptions::default())?;
        zw.write_all(script.as_bytes())?;
        zw.finish()?;
        sha256_hex(path)
    }

    fn make_tar_gz(path: &Path, bin_name: &str) -> Result<String> {
        std::fs::create_dir_all(path.parent().unwrap())?;
        let file = std::fs::File::create(path)?;
        let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut ar = tar::Builder::new(enc);
        // 顶层目录 v1.0.0/
        let top = format!("{bin_name}-1.0.0");
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Directory);
        h.set_mode(0o755);
        h.set_size(0);
        ar.append_data(
            &mut h,
            format!("{top}/"),
            std::io::Cursor::new(Vec::<u8>::new()),
        )?;
        // bin 脚本
        let script = format!("#!/bin/sh\necho '{bin_name} 1.0.0'\n");
        let mut hb = tar::Header::new_gnu();
        hb.set_size(script.len() as u64);
        hb.set_mode(0o755);
        ar.append_data(&mut hb, format!("{top}/{bin_name}"), script.as_bytes())?;
        let enc = ar.into_inner()?;
        let f = enc.finish()?;
        f.sync_all()?;
        drop(f);
        // sha256
        let bytes = std::fs::read(path)?;
        use sha2::{Digest, Sha256};
        let mut hh = Sha256::new();
        hh.update(&bytes);
        Ok(hh.finalize().iter().map(|b| format!("{b:02x}")).collect())
    }

    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    /// 本模块的测试共享**进程级**状态:`SM_*` 环境变量、`EXT: OnceLock` 扩展配方缓存、
    /// managed 目录与 PATH。cargo 默认并行 ⇒ 必须串行化。
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 统一入口:取锁 + 清空扩展配方。
    ///
    /// D8(2026-10-17)实测出来的两类共享状态:
    /// 1. **`SM_*` 环境变量 / managed 目录 / PATH** —— 进程级,靠这把锁串行化;
    /// 2. **扩展配方 `EXT`** —— 原来也是进程级可写缓存,`apply_server_config` 会整体替换它。
    ///    加锁**不够**:`commands::server_tasks` 的用例会派生 `server-task-worker` 线程,
    ///    那个线程不归测试管,照样能改写。所以 `EXT` 在测试期降级成 thread-local
    ///    (见 `registry.rs`),这里再清一次只是让"本用例从空注册表开始"这件事显式可见。
    fn sm_lock() -> std::sync::MutexGuard<'static, ()> {
        // poison 不能放大:一个用例 panic 后其余用例仍要继续(见模块头注释)
        let g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        registry::set_ext(Vec::new());
        g
    }
    fn tmp_root() -> PathBuf {
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("sm-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn mirror_url_rewrite_and_file_passthrough() {
        let _m = sm_lock();
        apply_server_config(None).unwrap(); // 清掉可能残留的 mirror/ext
        assert_eq!(mirror_url("https://x/y"), "https://x/y", "无 mirror 原样");
        assert_eq!(
            mirror_url("file:///tmp/a.tar.gz"),
            "file:///tmp/a.tar.gz",
            "file:// 不受 mirror 影响"
        );
    }

    #[test]
    fn sha256_and_extract_roundtrip() {
        let _m = sm_lock();
        let root = tmp_root();
        let arc = root.join("rel.tar.gz");
        let sha = make_tar_gz(&arc, "demo-bin").unwrap();
        assert_eq!(sha.len(), 64);
        let out = root.join("x");
        extract_archive(&arc, &out, 1).unwrap();
        assert!(out.join("demo-bin").is_file(), "strip 顶层后 bin 在位");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **zip 产物必须可执行**。回归:解包只 `File::create` 写字节,不带 unix mode ⇒
    /// 装出来的 bin 是 0644,软链过去 `--version` 直接 permission denied。
    /// 现实影响:terraform-ls / clojure-lsp / elixir-ls **只有 zip** ⇒ 这些配方此前装不了。
    /// **联网**下载单文件(默认 `#[ignore]`):隔离验证 ureq+代理这一层。
    ///
    /// ```bash
    /// SM_URL=<url> cargo test -p helix-term --lib live_download -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore]
    fn live_download_smoke() {
        let Ok(url) = std::env::var("SM_URL") else {
            eprintln!("跳过:未设 SM_URL");
            return;
        };
        let dir = std::env::temp_dir().join(format!("sm-dl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("out.bin");
        let started = std::time::Instant::now();
        download_to(&url, &dest, |_, _| {}).expect("download");
        let n = std::fs::metadata(&dest).unwrap().len();
        println!("✓ {n} bytes in {:?}", started.elapsed());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **只有 `[[…asset]]` 而没有 `url`** 的配方必须按"有下载源"处理,
    /// 并且 `strip`/`bin` 取配方自己的值。
    ///
    /// 回归(实测踩到):归一化只把 `url` 补成空串,分支判断又用
    /// `url.as_deref().filter(|u| !u.is_empty())` ⇒ 落到**占位**分支,`strip` 被强制成 1、
    /// `bin` 被忽略(用 `bin-name`)⇒ 单文件归档被 strip 掉,报"找不到可执行"。
    #[test]
    fn asset_only_recipe_uses_its_own_strip_and_bin() {
        let _m = sm_lock();
        let root = tmp_root();
        let _guard = EnvGuard::new("SM_MANAGED_DIR", root.to_string_lossy().into_owned());
        let arc = root.join("src/rel.zip");
        let _sha = make_zip(&arc, "asset-only").unwrap();
        let target = *registry::platform_targets()
            .last()
            .expect("本平台应有 target");
        let doc = format!(
            r#"
[server-manager.registry.asset-only]
kind = "lsp"
languages = ["x"]
strip = 0
bin = "asset-only"

[[server-manager.registry.asset-only.asset]]
target = "{target}"
url = "file://{}"
"#,
            arc.display()
        );
        let parsed: toml::Table = toml::from_str(&doc).unwrap();
        let sm = parsed.get("server-manager").unwrap().as_table().unwrap();
        apply_server_config(Some(sm)).unwrap();

        let spec = registry::get("asset-only").expect("asset-only 应在注册表里");
        install_spec(&spec, "").unwrap_or_else(|e| panic!("只有 asset[] 的配方应可装: {e:#}"));
        assert!(managed_bin("asset-only").exists(), "managed/bin 软链已建");
        assert_eq!(
            detect_version(&managed_bin("asset-only")).unwrap(),
            "asset-only 1.0.0",
            "装出来的应该是脚本本体(strip=0),不是被 strip 成空目录"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 代理环境变量解析(纯逻辑,不联网):小写优先、空串视为未设。
    ///
    /// 回归背景:`ureq` 2.x 不读 `*_PROXY`(curl 会)⇒ 有代理的机器上 `:server install`
    /// 直接挂在直连、UI 冻住且无报错("能 curl 下、hx 下不了")。
    #[test]
    fn proxy_env_is_read_lowercase_first() {
        let _m = sm_lock();
        // 四个键全清,避免宿主环境干扰
        let _g1 = EnvGuard::new("https_proxy", String::new());
        let _g2 = EnvGuard::new("HTTPS_PROXY", String::new());
        let _g3 = EnvGuard::new("http_proxy", String::new());
        let _g4 = EnvGuard::new("HTTP_PROXY", String::new());
        let _g5 = EnvGuard::new("all_proxy", String::new());
        let _g6 = EnvGuard::new("ALL_PROXY", String::new());
        assert!(proxy_from_env().is_none(), "全空 ⇒ 不走代理(直连)");
        std::env::set_var("HTTPS_PROXY", "http://127.0.0.1:7890");
        assert!(proxy_from_env().is_some(), "大写也能读到");
        std::env::set_var("https_proxy", "http://127.0.0.1:7891");
        assert!(proxy_from_env().is_some(), "小写优先(不 panic 即可,");
        std::env::remove_var("https_proxy");
        std::env::remove_var("HTTPS_PROXY");
        assert!(proxy_from_env().is_none(), "清掉后回到直连");
    }

    /// **联网**验证某条配方真能装(默认 `#[ignore]`,不进 CI)。
    ///
    /// 用途:录/改配方后证明"URL + strip + bin + 执行位"四件事同时对 —— 光看 HEAD 200
    /// 不够(归档内布局、`.exe` 后缀、zip 执行位都只有真装一遍才暴露)。
    ///
    /// ```bash
    /// SM_RECIPE=terraform-ls SM_RECIPE_VERSION=0.39.0 \
    ///   cargo test -p helix-term --lib live_recipe -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore]
    fn live_recipe_install_smoke() {
        let Ok(name) = std::env::var("SM_RECIPE") else {
            eprintln!("跳过:未设 SM_RECIPE(见本测试注释的跑法)");
            return;
        };
        let version = std::env::var("SM_RECIPE_VERSION").unwrap_or_default();
        let root = std::env::temp_dir().join(format!("sm-live-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("managed")).unwrap();
        let _managed = EnvGuard::new(
            "SM_MANAGED_DIR",
            root.join("managed").to_string_lossy().into_owned(),
        );
        let _langs = EnvGuard::new(
            "SM_LANGS_TOML",
            root.join("languages.toml").to_string_lossy().into_owned(),
        );
        // 用**仓库**那份配方文件(不带用户的 ~/.config 覆盖),保证验的是要提交的内容
        let recipes =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../contrib/server-manager-recipes.toml");
        let _rec = EnvGuard::new("SM_SERVER_RECIPES", recipes.to_string_lossy().into_owned());

        apply_server_config(None).expect("读配方");
        install(&name, &version).unwrap_or_else(|e| panic!("装 {name} {version} 失败: {e:#}"));

        let link = managed_bin(&name);
        assert!(link.exists(), "managed/bin/{name} 应存在");
        let v = detect_version(&link).unwrap_or_else(|| panic!("{name} 装出来了但跑不起来"));
        println!("✓ {name} -> {v}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn zip_payload_bin_is_executable() {
        let _m = sm_lock();
        let root = tmp_root();
        let _guard = EnvGuard::new("SM_MANAGED_DIR", root.to_string_lossy().into_owned());
        let arc = root.join("src/rel.zip");
        let sha = make_zip(&arc, "demo-zip").unwrap();
        let src = format!("file://{}", arc.display());
        install_adhoc("demo-zip", "demo-zip", "demo-zip", 0, &src, &sha).unwrap();

        let link = managed_bin("demo-zip");
        assert!(link.exists(), "bin 软链已建");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&link).unwrap().permissions().mode();
            assert!(
                mode & 0o111 != 0,
                "zip 装出来的 bin 必须可执行,实际 {mode:o}"
            );
        }
        // 功能证明:能真的跑起来(exec 位不对这里会拿不到版本)
        assert_eq!(detect_version(&link).unwrap(), "demo-zip 1.0.0");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn archive_install_update_remove_lifecycle() {
        let _m = sm_lock();
        let root = tmp_root();
        let _guard = EnvGuard::new("SM_MANAGED_DIR", root.to_string_lossy().into_owned());
        let arc = root.join("src/rel.tar.gz");
        std::fs::create_dir_all(arc.parent().unwrap()).unwrap();
        let sha = make_tar_gz(&arc, "demo-bin").unwrap();
        let src = format!("file://{}", arc.display());
        // adhoc 安装(demo-bin:tar.gz 顶层目录,strip=1,bin_rel=demo-bin)
        install_adhoc("demo-bin", "demo-bin", "demo-bin", 1, &src, &sha).unwrap();
        assert!(managed_bin("demo-bin").exists(), "bin 软链已建");
        assert_eq!(
            detect_version(&managed_bin("demo-bin")).unwrap(),
            "demo-bin 1.0.0"
        );
        // 升级(换版本文件)→ 重装成功
        let arc2 = root.join("src/rel2.tar.gz");
        let sha2 = make_tar_gz(&arc2, "demo-bin").unwrap();
        install_adhoc(
            "demo-bin",
            "demo-bin",
            "demo-bin",
            1,
            &format!("file://{}", arc2.display()),
            &sha2,
        )
        .unwrap();
        assert_eq!(
            detect_version(&managed_bin("demo-bin")).unwrap(),
            "demo-bin 1.0.0"
        );
        // 校验失败 → 中止且不留半成品、旧版仍在
        let bad = "0".repeat(64);
        let r = install_adhoc("demo-bin", "demo-bin", "demo-bin", 1, &src, &bad);
        assert!(r.is_err(), "sha 不符应失败");
        assert!(managed_bin("demo-bin").exists(), "失败不影响旧版");
        // 卸载
        remove_adhoc("demo-bin", "demo-bin").unwrap();
        assert!(!managed_bin("demo-bin").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 写可执行的假脚本
    fn write_sh(root: &Path, name: &str, body: &str) -> PathBuf {
        let p = root.join(name);
        std::fs::write(&p, body).unwrap();
        // 可执行位:非 unix 没有这个概念(可执行性由扩展名决定) ⇒ 只在 unix 设
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    }

    /// 假"工具"脚本(模拟 pip --target / cargo --root):$1=前缀目录,$2=版本;
    /// 在 $1/bin/ 生成 bin_name 可执行,内容里带上前缀与版本以验证占位替换。
    fn make_fake_tool(root: &Path, bin_name: &str) -> PathBuf {
        let script = format!(
            "#!/bin/sh\nset -e\nmkdir -p \"$1/bin\"\n\
             printf '#!/bin/sh\\necho {bin_name} 2.0.0 via %s %s\\n' \"$1\" \"$2\" > \"$1/bin/{bin_name}\"\n\
             chmod +x \"$1/bin/{bin_name}\"\n"
        );
        write_sh(root, "fake-tool.sh", &script)
    }

    #[test]
    fn tool_install_expands_placeholders_and_links_bin() {
        let _m = sm_lock();
        let root = tmp_root();
        let _guard = EnvGuard::new("SM_MANAGED_DIR", root.to_string_lossy().into_owned());
        let fake = make_fake_tool(&root, "demo-tool");
        // args 含 {prefix}/{version} 占位——应被替换为受管目录与版本
        install_tool_adhoc(
            "demo-tool",
            "demo-tool",
            fake.to_string_lossy().as_ref(),
            &["{prefix}", "{version}"],
            "2.0.0",
        )
        .unwrap();
        let link = managed_bin("demo-tool");
        assert!(link.exists(), "bin 软链已建");
        let prefix = managed_dir("demo-tool");
        let content = std::fs::read_to_string(prefix.join("bin/demo-tool")).unwrap();
        assert!(
            content.contains(prefix.to_string_lossy().as_ref()),
            "{{prefix}} 已替换为受管目录"
        );
        assert!(content.contains("2.0.0"), "{{version}} 已替换");
        let v = detect_version(&link).unwrap();
        assert!(v.contains("2.0.0"), "detect_version 走软链读到新版本: {v}");
        // 幂等:重装(同 update 语义)后仍一致
        install_tool_adhoc(
            "demo-tool",
            "demo-tool",
            fake.to_string_lossy().as_ref(),
            &["{prefix}", "{version}"],
            "2.0.0",
        )
        .unwrap();
        assert!(managed_bin("demo-tool").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tool_failure_cleans_up_and_keeps_old_on_upgrade() {
        let _m = sm_lock();
        let root = tmp_root();
        let _guard = EnvGuard::new("SM_MANAGED_DIR", root.to_string_lossy().into_owned());
        let ok_tool = make_fake_tool(&root, "demo-tool");
        // 先装成功
        install_tool_adhoc(
            "demo-tool",
            "demo-tool",
            ok_tool.to_string_lossy().as_ref(),
            &["{prefix}", "{version}"],
            "2.0.0",
        )
        .unwrap();
        // 升级失败 → 旧版保留(§4),staging 不残留
        let fail = write_sh(&root, "fail-tool.sh", "#!/bin/sh\necho boom >&2\nexit 3\n");
        let r = install_tool_adhoc(
            "demo-tool",
            "demo-tool",
            fail.to_string_lossy().as_ref(),
            &["{prefix}"],
            "9.9",
        );
        assert!(r.is_err(), "非零退出应失败");
        assert!(
            managed_dir("demo-tool").exists(),
            "升级失败保留旧版受管目录"
        );
        let v = detect_version(&managed_bin("demo-tool")).unwrap();
        assert!(
            v.contains("2.0.0") && !v.contains("9.9"),
            "旧版本仍可检测: {v}"
        );
        assert!(
            !managed_root().join(".demo-tool.bak").exists(),
            "成功后 .bak 不残留"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tool_success_without_bin_fails_cleanly() {
        let _m = sm_lock();
        let root = tmp_root();
        let _guard = EnvGuard::new("SM_MANAGED_DIR", root.to_string_lossy().into_owned());
        let fake = write_sh(&root, "noop-tool.sh", "#!/bin/sh\nmkdir -p \"$1\"\n");
        let r = install_tool_adhoc(
            "demo-tool",
            "demo-tool",
            fake.to_string_lossy().as_ref(),
            &["{prefix}"],
            "1.0",
        );
        let msg = format!("{:?}", r);
        assert!(r.is_err() && msg.contains("未生成"), "缺 bin 应报错: {msg}");
        assert!(!managed_dir("demo-tool").exists(), "缺 bin 也清理");
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- languages.toml 标记段 ----

    /// 双 env guard:SM_MANAGED_DIR + SM_LANGS_TOML → tmp root;返回 languages 路径
    fn sm_env(root: &Path) -> (PathBuf, EnvGuard, EnvGuard, EnvGuard) {
        let langs = root.join("languages.toml");
        let ls = langs.to_string_lossy().into_owned();
        let m = root.to_string_lossy().into_owned();
        (
            langs,
            EnvGuard::new("SM_MANAGED_DIR", m),
            EnvGuard::new("SM_LANGS_TOML", ls),
            EnvGuard::new("SM_PATH", String::new()), // 禁用宿主 PATH 本地检测(hermetic)
        )
    }

    /// 假装某 registry 配方已安装(受管目录 + bin 软链)
    fn install_fake(name: &str) {
        let root = PathBuf::from(std::env::var("SM_MANAGED_DIR").unwrap());
        let src = root.join("src").join(name);
        std::fs::create_dir_all(&src).unwrap();
        write_sh(&src, name, &format!("#!/bin/sh\necho {name} 1.0\n"));
        install_from_local_dir_for_test(name, &src).unwrap();
    }

    fn count(text: &str, sub: &str) -> usize {
        text.matches(sub).count()
    }

    #[test]
    fn langs_rewrite_from_empty_builds_section_idempotent() {
        let _m = sm_lock();
        let root = tmp_root();
        let (_langs, _a, _b, _gp) = sm_env(&root);
        install_fake("rust-analyzer");
        let rw = rewrite_languages_toml().unwrap();
        assert!(rw.written);
        assert_eq!(rw.servers, vec!["rust-analyzer"]);
        assert!(rw.conflicts.is_empty());
        let text = std::fs::read_to_string(&rw.path).unwrap();
        assert_eq!(count(&text, "# >>> helix-managed"), 1);
        assert_eq!(count(&text, "# <<< helix-managed"), 1);
        assert!(text.contains("[language-server.rust-analyzer]"));
        assert!(text.contains("command = \"/"));
        assert!(
            text.contains("[[language]]\nname = \"rust\"\nlanguage-servers = [\"rust-analyzer\"]")
        );
        toml::from_str::<toml::Value>(&text).expect("生成文本可解析");
        // 幂等:再次重写文本一致
        let rw2 = rewrite_languages_toml().unwrap();
        assert_eq!(text, std::fs::read_to_string(&rw2.path).unwrap());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn langs_user_content_preserved_and_section_replaced() {
        let _m = sm_lock();
        let root = tmp_root();
        let (langs, _ga, _gb, _gp) = sm_env(&root);
        let existing = "# 用户注释\n[[language]]\nname = \"haskell\"\nscope = \"source.haskell\"\n\n\
            # >>> helix-managed\n[language-server.rust-analyzer]\ncommand = \"/old/stale/path\"\n\n\
            [[language]]\nname = \"rust\"\nlanguage-servers = [\"rust-analyzer\"]\n# <<< helix-managed\n\n\
            [[language]]\nname = \"toml\"\ncomment-token = \"#\"\n";
        std::fs::write(&langs, existing).unwrap();
        install_fake("rust-analyzer");
        let rw = rewrite_languages_toml().unwrap();
        let text = std::fs::read_to_string(&rw.path).unwrap();
        // 用户标记外内容逐字节保留
        assert!(text.contains("# 用户注释"));
        assert!(text.contains("scope = \"source.haskell\""));
        assert!(text.contains("comment-token = \"#\""));
        // 旧段被整体替换,无残留
        assert!(!text.contains("/old/stale/path"));
        assert_eq!(count(&text, "# >>> helix-managed"), 1);
        assert_eq!(count(&text, "# <<< helix-managed"), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn langs_conflict_user_defined_language_skipped_with_hint() {
        let _m = sm_lock();
        let root = tmp_root();
        let (langs, _ga, _gb, _gp) = sm_env(&root);
        let existing = "[[language]]\nname = \"rust\"\nroots = [\"my-own-root\"]\n";
        std::fs::write(&langs, existing).unwrap();
        install_fake("rust-analyzer");
        let rw = rewrite_languages_toml().unwrap();
        let text = std::fs::read_to_string(&rw.path).unwrap();
        assert_eq!(
            count(&text, "name = \"rust\""),
            1,
            "用户已自定义 rust → 不再生成同名条目"
        );
        assert!(
            text.contains("[language-server.rust-analyzer]"),
            "server 表仍生成供手动引用"
        );
        assert_eq!(rw.conflicts.len(), 1);
        assert!(rw.conflicts[0].contains("rust-analyzer"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn langs_dap_debugpy_emits_debugger_block() {
        let _m = sm_lock();
        let root = tmp_root();
        let (_langs, _a, _b, _gp) = sm_env(&root);
        install_fake("debugpy");
        let rw = rewrite_languages_toml().unwrap();
        assert!(rw.servers.is_empty(), "Dap 不进 language-server 表");
        let text = std::fs::read_to_string(&rw.path).unwrap();
        assert!(text.contains("[[language]]\nname = \"python\""));
        assert!(text.contains("debugger = { name = \"debugpy\""));
        assert!(text.contains("transport = \"stdio\""));
        assert!(text.contains("templates = [{ name = \"launch\""));
        toml::from_str::<toml::Value>(&text).expect("生成文本可解析");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn langs_bad_user_toml_and_unbalanced_marker_abort() {
        let _m = sm_lock();
        let root = tmp_root();
        let (langs, _ga, _gb, _gp) = sm_env(&root);
        let p = &langs;
        // 语法坏(无标记)→ 报错不改写
        std::fs::write(p, "this is not toml {{{\n[[language]\n").unwrap();
        let err = rewrite_languages_toml().unwrap_err().to_string();
        assert!(err.contains("语法错误"), "{err}");
        assert_eq!(
            std::fs::read_to_string(p).unwrap(),
            "this is not toml {{{\n[[language]\n"
        );
        // 只有单边标记 → 报错不改写
        std::fs::write(p, "# >>> helix-managed\nx = 1\n").unwrap();
        let err = rewrite_languages_toml().unwrap_err().to_string();
        assert!(err.contains("不完整"), "{err}");
        assert_eq!(
            std::fs::read_to_string(p).unwrap(),
            "# >>> helix-managed\nx = 1\n"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn langs_remove_clears_entry() {
        let _m = sm_lock();
        let root = tmp_root();
        let (_langs, _a, _b, _gp) = sm_env(&root);
        install_fake("rust-analyzer");
        rewrite_languages_toml().unwrap();
        remove("rust-analyzer").unwrap();
        assert!(!managed_dir("rust-analyzer").exists());
        let text = std::fs::read_to_string(root.join("languages.toml")).unwrap();
        assert!(!text.contains("rust-analyzer"), "条目已清:{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn langs_user_own_server_table_not_duplicated() {
        let _m = sm_lock();
        let root = tmp_root();
        let (langs, _ga, _gb, _gp) = sm_env(&root);
        let existing = "[language-server.rust-analyzer]\ncommand = \"/mine/custom/ra\"\n";
        std::fs::write(&langs, existing).unwrap();
        install_fake("rust-analyzer");
        let rw = rewrite_languages_toml().unwrap();
        let text = std::fs::read_to_string(&rw.path).unwrap();
        assert_eq!(
            count(&text, "[language-server.rust-analyzer]"),
            1,
            "不重复表头"
        );
        assert!(text.contains("/mine/custom/ra"), "用户 command 保留");
        assert!(rw.servers.is_empty(), "用户自带同名 → 不再生成表");
        assert!(text.contains("language-servers = [\"rust-analyzer\"]"));
        toml::from_str::<toml::Value>(&text).expect("可解析");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 关键合并语义验证:生成的 languages.toml 经 helix-loader 真实合并逻辑
    /// (merge_toml_values depth 3 → Configuration)后,rust 挂上 rust-analyzer 且
    /// command 指向受管绝对路径;python 获得 debugpy debugger。
    #[test]
    fn langs_loader_merge_resolves_managed_servers() {
        let _m = sm_lock();
        let root = tmp_root();
        let (_langs, _a, _b, _gp) = sm_env(&root);
        install_fake("rust-analyzer");
        install_fake("debugpy");
        rewrite_languages_toml().unwrap();
        let file_text = std::fs::read_to_string(root.join("languages.toml")).unwrap();
        let user: toml::Value = toml::from_str(&file_text).unwrap();
        let merged =
            helix_loader::merge_toml_values(helix_loader::config::default_lang_config(), user, 3);
        let conf: helix_core::syntax::config::Configuration = merged.try_into().unwrap();
        let ra = conf
            .language_server
            .get("rust-analyzer")
            .expect("rust-analyzer 表存在");
        assert_eq!(
            ra.command,
            managed_bin("rust-analyzer").to_string_lossy(),
            "command 被覆写为受管绝对路径"
        );
        let rust = conf
            .language
            .iter()
            .find(|l| l.language_id == "rust")
            .expect("builtin rust 条目与生成条目合并");
        assert!(rust
            .language_servers
            .iter()
            .any(|f| f.name == "rust-analyzer"));
        let py = conf
            .language
            .iter()
            .find(|l| l.language_id == "python")
            .expect("python 语言存在");
        let dap = py.debugger.as_ref().expect("python 获得 debugger");
        assert_eq!(dap.name, "debugpy");
        assert_eq!(
            dap.command,
            managed_bin("debugpy").to_string_lossy(),
            "debugger command 为受管绝对路径"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- T4:config 段(mirror + registry.<name> 扩展配方) ----

    #[test]
    fn config_mirror_and_ext_recipe_install_loop() {
        let _m = sm_lock();
        let root = tmp_root();
        let (langs, _ga, _gb, _gp) = sm_env(&root);
        // file:// 假 release(tar.gz 顶层 my-ls-1.0.0/,strip=1)
        let arc = root.join("rel.tar.gz");
        let sha = make_tar_gz(&arc, "my-ls").unwrap();
        let src_uri = format!("file://{}", arc.display());
        let cfg = format!(
            "[server-manager]\nmirror = \"https://mirror.example/\"\n\
             [server-manager.registry.my-ls]\n\
             url = \"{src}\"\nsha256 = \"{sha}\"\nstrip = 1\nbin = \"my-ls\"\n\
             languages = [\"rust\"]\nkind = \"lsp\"\nversion = \"1.0.0\"\n",
            src = src_uri
        );
        let tbl: toml::Table = toml::from_str(&cfg).unwrap();
        let sm = tbl
            .get("server-manager")
            .unwrap()
            .as_table()
            .unwrap()
            .clone();
        apply_server_config(Some(&sm)).unwrap();
        // mirror:https 前缀改写,file:// 原样
        assert_eq!(
            mirror_url("https://x/y"),
            "https://mirror.example/https://x/y"
        );
        assert_eq!(mirror_url("file:///a"), "file:///a");
        // ext 配方进注册表;内置不受影响
        let spec = registry::get("my-ls").expect("ext 配方可查");
        assert!(spec.is_installable() && spec.version.as_deref() == Some("1.0.0"));
        assert!(registry::get("rust-analyzer").is_some(), "内置仍在");
        assert!(registry::get("nope").is_none());
        // 经 ext 源真实安装(版本默认取配方 version)
        install("my-ls", spec.version.as_deref().unwrap()).unwrap();
        assert!(managed_bin("my-ls").exists(), "bin 软链已建");
        // languages.toml 重建含 my-ls 条目
        let rw = rewrite_languages_toml().unwrap();
        assert!(rw.servers.iter().any(|n| n == "my-ls"));
        let text = std::fs::read_to_string(&langs).unwrap();
        assert!(text.contains("[language-server.my-ls]"));
        assert!(text.contains("language-servers = [\"my-ls\"]"));
        // registry 路径 remove:清目录 + 重写标记段
        remove("my-ls").unwrap();
        assert!(!managed_bin("my-ls").exists());
        let text = std::fs::read_to_string(&langs).unwrap();
        assert!(!text.contains("my-ls"), "条目已清:{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn config_ext_recipe_validation_errors() {
        let _m = sm_lock();
        // url 含 {version} 但缺 version 字段
        let bad: toml::Table =
            toml::from_str("url = \"https://x/{version}/y.tar.gz\"\nbin = \"x\"\n").unwrap();
        let e = registry::parse_ext_recipe("demo", &bad)
            .unwrap_err()
            .to_string();
        assert!(e.contains("version"), "{e}");
        // 缺 bin
        let bad2: toml::Table = toml::from_str("url = \"https://x/v1/y.tar.gz\"\n").unwrap();
        let e = registry::parse_ext_recipe("demo", &bad2)
            .unwrap_err()
            .to_string();
        assert!(e.contains("'bin'"), "{e}");
        // 未知 kind
        let bad3: toml::Table =
            toml::from_str("url = \"https://x/{version}/y.tar.gz\"\nbin = \"y\"\nkind = \"wat\"\n")
                .unwrap();
        let e = registry::parse_ext_recipe("demo", &bad3)
            .unwrap_err()
            .to_string();
        assert!(e.contains("wat"), "{e}");
        // 合法:默认值(strip=1/kind=lsp/bin-name=name)
        let ok: toml::Table = toml::from_str(
            "url = \"https://x/{version}/y.tar.gz\"\nbin = \"z\"\nversion = \"1.0\"\n",
        )
        .unwrap();
        let spec = registry::parse_ext_recipe("demo", &ok).unwrap();
        assert!(matches!(spec.kind, registry::Kind::Lsp));
        assert_eq!(spec.bin_name, "demo");
        assert_eq!(spec.languages.len(), 0);
        assert_eq!(spec.version.as_deref(), Some("1.0"));
        let _m2 = _m;
    }

    // ---- 单文件 gzip release(rust-analyzer 类)+ token 展开 ----

    /// 写单文件 gzip 假 release:内容 = bin 脚本;返回 sha256
    fn make_plain_gz(path: &Path) -> Result<String> {
        use sha2::{Digest, Sha256};
        std::fs::create_dir_all(path.parent().unwrap())?;
        let script = b"#!/bin/sh\necho plain-gz 3.2.1\n";
        let enc = flate2::write::GzEncoder::new(
            std::fs::File::create(path)?,
            flate2::Compression::default(),
        );
        let mut w = std::io::BufWriter::new(enc);
        w.write_all(script)?;
        let enc = w.into_inner().unwrap().finish()?;
        enc.sync_all()?;
        let mut h = Sha256::new();
        h.update(&std::fs::read(path)?);
        Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
    }

    #[test]
    fn plain_gz_single_binary_installs_and_runs() {
        let _m = sm_lock();
        let root = tmp_root();
        let _guard = EnvGuard::new("SM_MANAGED_DIR", root.to_string_lossy().into_owned());
        let arc = root.join("ra.gz");
        let sha = make_plain_gz(&arc).unwrap();
        install_adhoc(
            "plain-gz-demo",
            "plain-gz-demo",
            "plain-gz-demo",
            0,
            &format!("file://{}", arc.display()),
            &sha,
        )
        .unwrap();
        let link = managed_bin("plain-gz-demo");
        assert!(link.exists(), "单文件 gz 落位并软链");
        let v = detect_version(&link).unwrap();
        assert!(v.contains("3.2.1"), "解码产物可执行: {v}");
        remove_adhoc("plain-gz-demo", "plain-gz-demo").unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn template_tokens_expand_os_arch_triple() {
        let v = expand_template("{version}/{os}/{arch}/{triple}", "2024-09-16");
        assert!(!v.contains('{'), "占位全展开: {v}");
        assert!(v.contains("2024-09-16"));
        assert!(v.contains(std::env::consts::OS));
        assert!(v.contains(std::env::consts::ARCH));
        assert!(!rust_triple().is_empty(), "本机平台应有 triple");
        assert!(v.contains(&rust_triple()));
        // mirror_url 仍走 file 原样 / https 前缀
    }

    // ---- 本地 PATH 已装识别(local)+ 忽略名单 ----

    /// 在 tmp bin 目录放一个可执行假工具(用于 SM_PATH 注入)
    fn fake_local_bin(root: &Path, name: &str) -> PathBuf {
        let dir = root.join("localbin");
        std::fs::create_dir_all(&dir).unwrap();
        write_sh(&dir, name, &format!("#!/bin/sh\necho {name} 9.9\n"));
        dir
    }

    #[test]
    fn availability_prefers_managed_over_local() {
        let _m = sm_lock();
        let root = tmp_root();
        let (_langs, _a, _b, _p) = sm_env(&root);
        let bin_dir = fake_local_bin(&root, "rust-analyzer");
        let _bin = EnvGuard::new("SM_PATH", bin_dir.to_string_lossy().into_owned());
        // 仅本地
        assert_eq!(
            availability("rust-analyzer"),
            Availability::Local(bin_dir.join("rust-analyzer"))
        );
        // 受管优先
        install_fake("rust-analyzer");
        assert_eq!(availability("rust-analyzer"), Availability::Managed);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn local_server_writes_path_command_and_ignore_removes_it() {
        let _m = sm_lock();
        let root = tmp_root();
        let (langs, _a, _b, _p) = sm_env(&root);
        let bin_dir = fake_local_bin(&root, "rust-analyzer");
        let _bin = EnvGuard::new("SM_PATH", bin_dir.to_string_lossy().into_owned());
        // languages.toml 对本地 server 写绝对 PATH command 并挂接 rust
        let rw = rewrite_languages_toml().unwrap();
        let text = std::fs::read_to_string(&langs).unwrap();
        assert!(text.contains("[language-server.rust-analyzer]"), "{text}");
        let cmd = bin_dir.join("rust-analyzer").to_string_lossy().into_owned();
        assert!(
            text.contains(&format!("command = \"{cmd}\"")),
            "本地 server command=PATH 绝对路径:\n{text}"
        );
        assert!(rw.servers.is_empty(), "local 不计入受管报告");
        // unmanage(忽略)→ 重建后无该 server;恢复后回来
        set_ignored("rust-analyzer", true).unwrap();
        assert!(ignored_local().contains(&"rust-analyzer".to_string()));
        rewrite_languages_toml().unwrap();
        let text = std::fs::read_to_string(&langs).unwrap();
        assert!(!text.contains("rust-analyzer"), "忽略后不再挂接:\n{text}");
        set_ignored("rust-analyzer", false).unwrap();
        rewrite_languages_toml().unwrap();
        let text = std::fs::read_to_string(&langs).unwrap();
        assert!(text.contains("[language-server.rust-analyzer]"), "恢复挂接");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn spec_description_homepage_and_upgradable() {
        let _m = sm_lock();
        // parse_ext_recipe 支持 description/homepage
        let ok: toml::Table = toml::from_str(
            "url = \"file:///v{version}/x.tar.gz\"\nbin = \"z\"\nversion = \"1.0\"\n\
             description = \"假工具\"\nhomepage = \"https://example.com\"\n",
        )
        .unwrap();
        let spec = registry::parse_ext_recipe("demo", &ok).unwrap();
        assert_eq!(spec.description, "假工具");
        assert_eq!(spec.homepage.as_deref(), Some("https://example.com"));
        // 可升级判定:受管已装 && version 有值 && != 检测版本 → true
        let _ = spec;
    }

    #[test]
    fn builtin_specs_all_have_description_and_homepage() {
        let _m = sm_lock();
        // M5:内置配方补展示数据(arsenal 行/信息弹窗显示)——缺描述/主页=遗漏
        // 8 = 7 原有 + `lldb-dap`(DAP 重构 P0-T4:让 Rust 开箱能选到一个真实存在的 adapter)
        let specs = registry::builtin_specs();
        assert_eq!(specs.len(), 8, "内置配方数变更需同步测试");
        for s in &specs {
            assert!(
                !s.description.is_empty(),
                "{} 缺 description(M5 应补中文一句)",
                s.name
            );
            assert!(
                s.homepage
                    .as_deref()
                    .is_some_and(|h| h.starts_with("https://")),
                "{} 缺 homepage 官方链接",
                s.name
            );
        }
    }

    /// 内置配方数据化:文件加载、order 保序、占位与动态版本标志语义与旧硬编码一致
    #[test]
    fn builtin_recipes_loaded_from_toml_keep_semantics() {
        let _m = sm_lock();
        let specs = registry::builtin_specs();
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "rust-analyzer",
                "gopls",
                "pyright",
                "clangd",
                "debugpy",
                "lldb-dap",
                "black",
                "prettier"
            ],
            "order 字段保序(rust-analyzer 必须首位:arsenal sel0/导航依赖)"
        );
        let ra = specs.iter().find(|s| s.name == "rust-analyzer").unwrap();
        assert_eq!(ra.kind, registry::Kind::Lsp);
        assert_eq!(ra.languages, vec!["rust"]);
        assert_eq!(ra.bin_name, "rust-analyzer");
        assert!(ra.is_installable(), "RA url 非空应可装");
        assert!(ra.version.is_none());
        assert!(
            needs_version(ra),
            "url 含 {{version}} + 无 version → 需显式版本"
        );
        // 占位条目(url = "" / cmd = "")不可装,保留展示数据。
        // ⚠ 这里查的是**内置**集合(`specs`),不是 `registry::get`:`get` 会把
        // 进程级的 `EXT`(其他用例 / 其他测试模块塞进来的扩展配方)合并进来,
        // 于是"pyright 是占位"会被用户配置里的 pyright 覆盖掉 —— D8 的间歇失败就是这条。
        // 本用例的名字与意图都只关于**内置 TOML**。
        for n in ["gopls", "pyright", "clangd", "debugpy", "black", "prettier"] {
            let s = specs.iter().find(|s| s.name == n).unwrap();
            assert!(!s.is_installable(), "{n} 应为占位(no source)");
            assert!(!s.description.is_empty() && s.homepage.is_some());
        }
        // Tool 占位:cmd 空 = Install::Tool(区别于 archive 占位)
        assert!(matches!(
            specs
                .iter()
                .find(|s| s.name == "debugpy")
                .unwrap()
                .install,
            registry::Install::Tool { ref cmd, .. } if cmd.is_empty()
        ));
    }

    /// 同名覆盖:扩展配方替掉内置(不重复);内置顺序保留,扩展追加
    #[test]
    fn ext_overrides_builtin_same_name_without_duplicate() {
        let _m = sm_lock();
        let tbl: toml::Table = toml::from_str(
            "cmd = \"npm\"\nargs = [\"install\", \"--prefix\", \"{prefix}\", \"pyright\"]\nbin-name = \"pyright-langserver\"\ndescription = \"overridden\"\n",
        )
        .unwrap();
        let ext = registry::parse_ext_recipe("pyright", &tbl).unwrap();
        registry::set_ext(vec![ext]);
        let all = registry::all();
        assert_eq!(
            all.iter().filter(|s| s.name == "pyright").count(),
            1,
            "同名不应重复出现"
        );
        let got = registry::get("pyright").unwrap();
        assert!(got.is_installable(), "get 应拿到覆盖后的可装配方");
        assert_eq!(got.description, "overridden");
        let idx_ra = all.iter().position(|s| s.name == "rust-analyzer").unwrap();
        assert_eq!(idx_ra, 0, "内置顺序不受覆盖影响");
        registry::set_ext(Vec::new());
    }

    #[test]
    fn is_upgradable_contains_semantics() {
        let _m = sm_lock();
        let root = tmp_root();
        let _guard = EnvGuard::new("SM_MANAGED_DIR", root.to_string_lossy().into_owned());
        // 配方 version=1.0.0;受管装 bin 输出 "my-ls 1.0.0"(detected 含 recipe 版本 → 同版)
        let ok: toml::Table = toml::from_str(
            "url = \"https://x/v{version}/my-ls.tar.gz\"\nbin = \"my-ls\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let spec = registry::parse_ext_recipe("my-ls", &ok).unwrap();
        let arc = root.join("my-ls.tar.gz");
        let sha = make_tar_gz(&arc, "my-ls").unwrap();
        install_adhoc(
            "my-ls",
            "my-ls",
            "my-ls",
            1,
            &format!("file://{}", arc.display()),
            &sha,
        )
        .unwrap();
        assert_eq!(
            detect_version(&managed_bin("my-ls")).unwrap(),
            "my-ls 1.0.0"
        );
        assert!(
            !is_upgradable(&spec),
            "detected 含 recipe version → 同版不可升级"
        );
        // 换装:bin 检测输出变为 "my-ls 9.9"(detected 不含 recipe 版本 → 可升级)
        let bin = managed_bin("my-ls");
        std::fs::write(&bin, "#!/bin/sh\necho my-ls 9.9\n").unwrap();
        assert_eq!(detect_version(&bin).unwrap(), "my-ls 9.9");
        assert!(
            is_upgradable(&spec),
            "detected 不含 recipe version → 可升级"
        );
        // version=None → 恒 false
        let nover: toml::Table =
            toml::from_str("url = \"https://x/v1/my-ls.tar.gz\"\nbin = \"my-ls\"\n").unwrap();
        let spec2 = registry::parse_ext_recipe("my-ls", &nover).unwrap();
        assert!(!is_upgradable(&spec2), "recipe 无 version → 不可升级");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn needs_version_archive_template_semantics() {
        let _m = sm_lock();
        // 内置活配方 rust-analyzer:url 含 {version} 且 recipe 无固定 version → 需显式版本
        let ra = registry::all()
            .into_iter()
            .find(|s| s.name == "rust-analyzer")
            .unwrap();
        assert!(
            needs_version(&ra),
            "内置 {{version}} 占位 + 无 version → needs_version"
        );
        // 惰性配方(url 空)与 Tool 类(debugpy/black) → false
        let gopls = registry::all()
            .into_iter()
            .find(|s| s.name == "gopls")
            .unwrap();
        assert!(!needs_version(&gopls), "url 空(惰性)→ false");
        let debugpy = registry::all()
            .into_iter()
            .find(|s| s.name == "debugpy")
            .unwrap();
        assert!(!needs_version(&debugpy), "Tool 安装 → false");
        // 手构 Spec 补边界:url 含 {version} + version 固定 → false;url 无 {version} → false
        let mk = |url: &str, version: Option<String>| registry::Spec {
            name: "t".into(),
            kind: registry::Kind::Lsp,
            languages: vec![],
            bin_name: "t".into(),
            description: String::new(),
            homepage: None,
            install: registry::Install::Archive {
                url_template: url.into(),
                sha256: String::new(),
                strip: 1,
                bin_rel: "t".into(),
                assets: Vec::new(),
            },
            version,
            latest_source: None,
            format_args: Vec::new(),
        };
        assert!(
            !needs_version(&mk("https://x/v{version}/t.tar.gz", Some("1.0".into()))),
            "version 固定 → false"
        );
        assert!(
            !needs_version(&mk("https://x/v1.0/t.tar.gz", None)),
            "url 无 {{version}} 占位 → false"
        );
    }

    // ---- M2a:进度聚合器 / download_to 进度 / run_task 纯函数 ----

    #[test]
    fn progress_aggregator_monotonic_throttled() {
        let _m = sm_lock();
        // 聚合器:回调每 ≥throttle_bytes 或 ≥interval 触发;结束 flush 到 100%
        let mut hits = Vec::new();
        let mut cb = |n, total| {
            hits.push((n, total));
        };
        let mut agg = ProgressAgg::new(8, std::time::Duration::from_millis(100), &mut cb);
        for _ in 0..50u64 {
            agg.add(1, Some(100)); // 共 50 字节,节流 8 → 约 6 次
        }
        agg.finish(50, Some(100));
        assert!(hits.len() >= 2, "至少节流若干次+末尾 flush: {hits:?}");
        let last = hits.last().unwrap();
        assert_eq!(last.0, 50, "末尾 flush 到总量");
        // 序列单调
        let ns: Vec<u64> = hits.iter().map(|(n, _)| *n).collect();
        assert!(ns.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn download_to_file_reports_progress_once_with_size() {
        let _m = sm_lock();
        let root = tmp_root();
        let src = root.join("payload.bin");
        let body = vec![0xabu8; 1024 * 5]; // 5KiB
        std::fs::write(&src, &body).unwrap();
        let dest = root.join("out.bin");
        let mut hits = Vec::new();
        download_to(&format!("file://{}", src.display()), &dest, |n, total| {
            hits.push((n, total));
        })
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body, "内容已复制");
        assert_eq!(
            hits,
            vec![(body.len() as u64, Some(body.len() as u64))],
            "file:// 下载完成回调一次(总大小)"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// run_task 分支冒烟(hermetic)。lib 测试无 CONFIG_FILE(config_file_opt=None),
    /// run_task 开头重读磁盘 config 会把 registry 扩展清空——install 走 file:// 假源的
    /// ok:true 全程(含阶段事件)需磁盘 config 注入配方,留给任务 5 编辑器集成;
    /// 此处覆盖可离线判定的 ok/err 分支(均不触网)。
    #[test]
    fn run_task_branches_ok_and_err_hermetic() {
        let _m = sm_lock();
        let root = tmp_root();
        let (_langs, _a, _b, _p) = sm_env(&root);
        // 未知 op / 未知 server / unmanage 缺失 → ok:false 文案
        let out = run_task("bogus", "x", None, |_, _, _| {});
        assert!(!out.ok && out.msg.contains("unknown task op"), "{out:?}");
        let out = run_task("install", "ghost", None, |_, _, _| {});
        assert!(!out.ok && out.msg.contains("unknown server"), "{out:?}");
        let out = run_task("unmanage", "ghost", None, |_, _, _| {});
        assert!(!out.ok && out.msg.contains("未安装/不在 PATH"), "{out:?}");
        // install 命中受管 → 引导 update
        install_fake("rust-analyzer");
        let out = run_task("install", "rust-analyzer", None, |_, _, _| {});
        assert!(!out.ok && out.msg.contains("已安装,用 update"), "{out:?}");
        // update 受管但配方无 version → 引导显式 version(不触网)
        let out = run_task("update", "rust-analyzer", None, |_, _, _| {});
        assert!(!out.ok && out.msg.contains("配方未给 version"), "{out:?}");
        // remove 受管 → ok:true 且清理
        let out = run_task("remove", "rust-analyzer", None, |_, _, _| {});
        assert!(out.ok && out.msg.contains("removed"), "{out:?}");
        assert!(!managed_bin("rust-analyzer").exists());
        // update 缺失 → 引导 install
        let out = run_task("update", "rust-analyzer", None, |_, _, _| {});
        assert!(!out.ok && out.msg.contains("未安装"), "{out:?}");
        // unmanage 本地 PATH 工具 → 停用挂接,再跑恢复
        let bin_dir = fake_local_bin(&root, "rust-analyzer");
        let _bin = EnvGuard::new("SM_PATH", bin_dir.to_string_lossy().into_owned());
        let out = run_task("unmanage", "rust-analyzer", None, |_, _, _| {});
        assert!(out.ok && out.msg.contains("已停用挂接"), "{out:?}");
        assert!(ignored_local().contains(&"rust-analyzer".to_string()));
        let out = run_task("unmanage", "rust-analyzer", None, |_, _, _| {});
        assert!(out.ok && out.msg.contains("已恢复挂接"), "{out:?}");
        // remove 本地工具 → 指引 unmanage
        let out = run_task("remove", "rust-analyzer", None, |_, _, _| {});
        assert!(!out.ok && out.msg.contains("unmanage"), "{out:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- 独立配方文件 + Tool 配方解析 ----

    #[test]
    fn parse_ext_recipe_tool_and_archive_mutual_exclusive() {
        let _m = sm_lock();
        // Tool:cmd+args
        let t: toml::Table = toml::from_str(
            "cmd = \"npm\"\nargs = [\"install\", \"--prefix\", \"{prefix}\", \"typescript-language-server\"]\n\
             kind = \"lsp\"\nlanguages = [\"typescript\", \"javascript\"]\n\
             description = \"TS 语言服务器\"\n",
        )
        .unwrap();
        let spec = registry::parse_ext_recipe("tsserver", &t).unwrap();
        assert!(
            matches!(&spec.install, registry::Install::Tool { cmd, args } if cmd == "npm" && args.len() == 4)
        );
        assert_eq!(spec.bin_name, "tsserver", "bin-name 缺省=配方名");
        assert!(spec.languages.contains(&"typescript".to_string()));
        assert!(!spec.install_needs_version(), "Tool 不需 version");
        // url+cmd 互斥
        let bad: toml::Table = toml::from_str("cmd = \"npm\"\nurl = \"https://x\"\n").unwrap();
        assert!(registry::parse_ext_recipe("x", &bad).is_err());
        // 两者皆缺
        let bad2: toml::Table = toml::from_str("languages = [\"a\"]\n").unwrap();
        assert!(registry::parse_ext_recipe("x", &bad2).is_err());
        // Archive 仍走 url 分支(含 {version} 需 version)
        let a: toml::Table = toml::from_str(
            "url = \"file:///v{version}/x.tar.gz\"\nbin = \"x\"\nversion = \"1.0\"\n",
        )
        .unwrap();
        let spec = registry::parse_ext_recipe("arc", &a).unwrap();
        assert!(matches!(&spec.install, registry::Install::Archive { .. }));
        assert!(spec.install_needs_version());
    }

    #[test]
    fn recipes_file_merges_and_overrides_config_registry() {
        let _m = sm_lock();
        let root = tmp_root();
        let (_langs, ga, gb, gp) = sm_env(&root);
        // 独立配方文件(经 SM_SERVER_RECIPES 覆写):tool + archive 各一
        let file = root.join("recipes.toml");
        std::fs::write(
            &file,
            "[server-manager.registry.from-file]\ncmd = \"npm\"\nargs = [\"x\"]\n\
             [server-manager.registry.both]\nurl = \"file:///v1/x.tar.gz\"\nbin = \"x\"\n",
        )
        .unwrap();
        let _gr = EnvGuard::new("SM_SERVER_RECIPES", file.to_string_lossy().into_owned());
        // config 段里同名 both → 文件后读覆盖为 archive
        let cfg: toml::Table =
            toml::from_str("[registry.both]\ncmd = \"pip3\"\nargs = [\"install\"]\n").unwrap();
        apply_server_config(Some(&cfg)).unwrap();
        assert!(registry::get("from-file").is_some(), "配方文件配方进入扩展");
        let both = registry::get("both").expect("同名存在");
        assert!(
            matches!(&both.install, registry::Install::Archive { .. }),
            "配方文件(后读)覆盖 config"
        );
        let _ = std::fs::remove_dir_all(&root);
        let _ = (ga, gb, gp);
    }

    /// 配方数据文件(contrib/)完整性:每条可解析、结构有效;仓库副本与发布物同源
    #[test]
    fn bundled_recipes_file_parses_and_is_healthy() {
        let _m = sm_lock();
        let raw = include_str!("../../../contrib/server-manager-recipes.toml");
        let v: toml::Value = toml::from_str(raw).expect("contrib 配方文件可解析");
        let reg = v
            .get("server-manager")
            .and_then(toml::Value::as_table)
            .and_then(|t| t.get("registry"))
            .and_then(toml::Value::as_table)
            .expect("含 [server-manager.registry.*]");
        assert!(reg.len() >= 40, "配方数 {} >= 40", reg.len());
        let mut installable = 0usize;
        let mut langs: std::collections::BTreeSet<String> = Default::default();
        for (name, rv) in reg {
            let tbl = rv.as_table().expect("配方是表");
            let spec = registry::parse_ext_recipe(name, tbl)
                .unwrap_or_else(|e| panic!("配方 {name} 解析失败: {e}"));
            assert!(!spec.languages.is_empty(), "配方 {name} languages 非空");
            assert!(!spec.description.is_empty(), "配方 {name} description 非空");
            langs.extend(spec.languages.iter().cloned());
            if spec.is_installable() {
                installable += 1;
            }
        }
        assert!(installable >= 20, "可装配方 >= 20, got {installable}");
        assert!(langs.len() >= 45, "覆盖语言 >= 45, got {}", langs.len());
    }

    #[test]
    fn parse_ext_recipe_explicit_empty_url_is_placeholder() {
        let _m = sm_lock();
        let t: toml::Table =
            toml::from_str("url = \"\"\n\ndescription = \"安装:见官网(仅占位引导)\"\n").unwrap();
        let spec = registry::parse_ext_recipe("lua-ls", &t).unwrap();
        assert!(!spec.is_installable(), "空 url = no source");
        assert_eq!(
            spec.bin_rel_for_test(),
            "lua-ls",
            "占位 bin_rel=配方名,免填 bin"
        );
    }

    #[test]
    fn run_task_tool_install_without_version_passes_gate() {
        let _m = sm_lock();
        let root = tmp_root();
        let (_langs, _ga, _gb, _gp) = sm_env(&root);
        // Tool 配方 /bin/false:版本门应放行(不再报"未给 version"),执行失败属另一原因
        let file = root.join("recipes.toml");
        std::fs::write(
            &file,
            "[server-manager.registry.falsey]\ncmd = \"/bin/false\"\nargs = []\n",
        )
        .unwrap();
        let _gr = EnvGuard::new("SM_SERVER_RECIPES", file.to_string_lossy().into_owned());
        let mut events = Vec::new();
        let out = run_task("install", "falsey", None, |phase, _b, _t| {
            events.push(phase.to_string())
        });
        assert!(!out.ok, "执行本身失败(预期)");
        assert!(
            !out.msg.contains("未给 version"),
            "版本门放行 Tool: {}",
            out.msg
        );
        assert!(!events.is_empty(), "阶段事件照发");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 装了 formatter ⇒ languages.toml 里要写 `formatter = { command, args }`
    /// （早先 `match kind` 只有 Lsp/Dap，formatter 走 `_ => {}`，装完什么都不写）。
    #[test]
    fn formatter_is_wired_into_languages_toml() {
        let _m = sm_lock();
        let root = tmp_root();
        let langs = root.join("languages.toml");
        let _md = EnvGuard::new(
            "SM_MANAGED_DIR",
            root.join("managed").to_string_lossy().into_owned(),
        );
        let _lt = EnvGuard::new("SM_LANGS_TOML", langs.to_string_lossy().into_owned());
        apply_server_config(None).unwrap();

        // 伪造"black 已受管"：managed/<name>/ + managed/bin/<bin> 软链
        let spec = registry::get("black").expect("内置配方应有 black");
        let dir = root.join("managed").join(&spec.name);
        std::fs::create_dir_all(&dir).unwrap();
        let bin = root.join("managed").join("bin").join(&spec.bin_name);
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        let fake = dir.join(&spec.bin_name);
        std::fs::write(&fake, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
            let _ = std::fs::remove_file(&bin);
            std::os::unix::fs::symlink(&fake, &bin).unwrap();
        }

        let out = langs_toml::rewrite_languages_toml().expect("rewrite");
        assert!(out.written, "应写了 languages.toml");
        let text = std::fs::read_to_string(&langs).unwrap();
        assert!(
            text.contains("formatter = { command ="),
            "formatter 必须写进 [[language]]: {text}"
        );
        assert!(text.contains("\"-q\""), "format-args 应写进 args: {text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 动态版本配方（`spec.version = None`）也能判"可升级"：
    /// 靠 `installed.json`（我装了啥）+ `latest.json`（上游最新，`update check` 写的）。
    #[test]
    fn upgradable_uses_recorded_and_latest_versions() {
        let _m = sm_lock();
        let root = tmp_root();
        let _md = EnvGuard::new(
            "SM_MANAGED_DIR",
            root.join("managed").to_string_lossy().into_owned(),
        );
        apply_server_config(None).unwrap();
        let spec = registry::get("rust-analyzer").expect("内置应有 rust-analyzer");
        assert!(spec.version.is_none(), "rust-analyzer 应是动态版本配方");

        // 没查过上游 ⇒ 不知道（不谎报）
        install::record_installed_version(&spec.name, "2025-01-01");
        assert!(!is_upgradable(&spec), "没有上游信息时不应报可升级");

        // 上游有更新版本 ⇒ 可升级
        install::record_latest_version(&spec.name, "2026-10-26");
        assert!(
            is_upgradable(&spec),
            "已装 2025-01-01 vs 上游 2026-10-26 ⇒ 可升级"
        );

        // 已装就是最新 ⇒ 不可升级
        install::record_installed_version(&spec.name, "2026-10-26");
        assert!(!is_upgradable(&spec), "已是最新 ⇒ 不可升级");
        install::clear_error(&spec.name);
        let _ = std::fs::remove_dir_all(&root);
    }

    struct EnvGuard(&'static str);
    impl EnvGuard {
        fn new(key: &'static str, val: String) -> Self {
            std::env::set_var(key, &val);
            EnvGuard(key)
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            std::env::remove_var(self.0);
        }
    }

    /// clear API:未达下载点的批次(remove/unmanage/下载前报错)经 clear 后当前线程
    /// 钩子必清——主线程后续同步 :server install 不会消费到残留幽灵钩子。
    #[test]
    fn clear_download_progress_drops_hook() {
        let _m = sm_lock();
        with_download_progress(Box::new(|_, _| {}));
        assert!(download_progress_hook_set(), "钩子已挂");
        clear_download_progress();
        assert!(
            !download_progress_hook_set(),
            "clear 后当前线程钩子已清(无残留)"
        );
    }

    #[test]
    fn download_progress_hook_consumed_by_install_adhoc() {
        // DL_PROGRESS 线程局部钩子:install_adhoc 下载前挂载,take 一次消费并收到字节回调
        let _m = sm_lock();
        let root = tmp_root();
        let _guard = EnvGuard::new("SM_MANAGED_DIR", root.to_string_lossy().into_owned());
        let arc = root.join("src/rel.tar.gz");
        std::fs::create_dir_all(arc.parent().unwrap()).unwrap();
        let sha = make_tar_gz(&arc, "demo-bin").unwrap();
        let src = format!("file://{}", arc.display());
        let size = std::fs::metadata(&arc).unwrap().len();
        type Hits = std::sync::Arc<std::sync::Mutex<Vec<(u64, Option<u64>)>>>;
        let hits: Hits = std::sync::Arc::default();
        let hits2 = hits.clone();
        with_download_progress(Box::new(move |n, t| {
            hits2.lock().unwrap_or_else(|e| e.into_inner()).push((n, t))
        }));
        install_adhoc("demo-bin", "demo-bin", "demo-bin", 1, &src, &sha).unwrap();
        assert!(managed_bin("demo-bin").exists());
        assert_eq!(
            hits.lock().unwrap_or_else(|e| e.into_inner()).as_slice(),
            &[(size, Some(size))],
            "file:// 复制完成回调一次(总大小),钩子已消费"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
