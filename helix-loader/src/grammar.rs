use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::SystemTime;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::channel,
};
use tempfile::TempPath;
use tree_house::tree_sitter::Grammar;

#[cfg(target_os = "macos")]
const DYLIB_EXTENSION: &str = "dylib";

#[cfg(all(unix, not(target_os = "macos")))]
const DYLIB_EXTENSION: &str = "so";

#[cfg(windows)]
const DYLIB_EXTENSION: &str = "dll";

#[cfg(target_arch = "wasm32")]
const DYLIB_EXTENSION: &str = "wasm";

/// **编译产物**的动态库扩展名——按 `target` 推断，而不是按 `cfg!`。
///
/// 为什么不能直接用上面的常量：`build_grammar` 跑在 **build script** 里，而 build script
/// **永远按宿主编译**。交叉编译（`--target x86_64-pc-windows-gnu`）时 `cfg!(windows)` 是 false，
/// 于是 Windows 的语法产物会被命名成 `.so`：
///
/// - `needs_recompile` 按 `.so` 比对 → 宿主已经编过的那个 `.so` 被当成本目标的产物
///   ⇒ **301 个语法全部被跳过**，一个 `.dll` 也不产出；
/// - 即使强行重建，文件名也不对，运行期按 `.dll` 找还是找不到。
///
/// 运行期的加载路径（[`get_language`] 里那个）**仍然用宿主常量**——那里的 target 就是宿主，
/// 两者不矛盾。
fn dylib_extension_for(target: Option<&str>) -> &'static str {
    let Some(target) = target else {
        return DYLIB_EXTENSION; // None = 就是宿主
    };
    if target.contains("windows") {
        "dll"
    } else if target.contains("darwin") || target.contains("apple") {
        "dylib"
    } else if target.contains("wasm") {
        "wasm"
    } else {
        "so"
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Configuration {
    #[serde(rename = "use-grammars")]
    pub grammar_selection: Option<GrammarSelection>,
    pub grammar: Vec<GrammarConfiguration>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", untagged)]
pub enum GrammarSelection {
    Only { only: HashSet<String> },
    Except { except: HashSet<String> },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrammarConfiguration {
    #[serde(rename = "name")]
    pub grammar_id: String,
    pub source: GrammarSource,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", untagged)]
pub enum GrammarSource {
    Local {
        path: String,
    },
    Git {
        #[serde(rename = "git")]
        remote: String,
        #[serde(rename = "rev")]
        revision: String,
        subpath: Option<String>,
    },
}

const BUILD_TARGET: &str = env!("BUILD_TARGET");
/// 运行编译器的那台机器（build script 永远跑在宿主上）。
///
/// 与 [`BUILD_TARGET`] 的差别只在**交叉编译**时暴露：那里是目标，这里是宿主。
const HOST_TARGET: &str = env!("HOST_TARGET");
const REMOTE_NAME: &str = "origin";

#[cfg(target_arch = "wasm32")]
pub fn get_language(name: &str) -> Result<Option<Grammar>> {
    unimplemented!()
}

#[cfg(not(target_arch = "wasm32"))]
pub fn get_language(name: &str) -> Result<Option<Grammar>> {
    let mut rel_library_path = PathBuf::new().join("grammars").join(name);
    rel_library_path.set_extension(DYLIB_EXTENSION);
    let library_path = crate::runtime_file(&rel_library_path);
    if !library_path.exists() {
        return Ok(None);
    }

    let grammar = unsafe { Grammar::new(name, &library_path) }?;
    Ok(Some(grammar))
}

fn ensure_git_is_available() -> Result<()> {
    helix_stdx::env::which("git")?;
    Ok(())
}

/// Print a notice if the current workspace has a `.helix/languages.toml` that we *would* have
/// merged but the workspace-trust gate is keeping us from.
fn warn_if_workspace_languages_skipped(trust: &crate::workspace_trust::WorkspaceTrust) {
    let workspace_languages = crate::workspace_lang_config_file();
    if !workspace_languages.exists() {
        return;
    }
    if trust
        .query_current(crate::workspace_trust::TrustQuery::LocalConfig)
        .is_trusted()
    {
        return;
    }
    println!(
        "Note: workspace `{}` was skipped because the workspace is not trusted. Run \
         `:workspace-trust` from an interactive helix session in this workspace to opt in.",
        workspace_languages.display(),
    );
}

pub fn fetch_grammars(strict: bool) -> Result<()> {
    ensure_git_is_available()?;

    // We do not need to fetch local grammars.
    let mut grammars = get_grammar_configs()?;
    grammars.retain(|grammar| !matches!(grammar.source, GrammarSource::Local { .. }));

    let total = grammars.len();
    let counter = Arc::new(AtomicUsize::new(0));

    println!("Fetching {} grammars", total);
    let counter = Arc::clone(&counter);

    let results = run_parallel(grammars, move |grammar| {
        let current = counter.fetch_add(1, Ordering::Relaxed) + 1;

        println!(
            "Fetching grammars ({}/{}): {}",
            current, total, grammar.grammar_id
        );
        fetch_grammar(grammar)
    });

    let mut errors = Vec::new();
    let mut git_updated = Vec::new();
    let mut git_up_to_date = 0;
    let mut non_git = Vec::new();

    for (grammar_id, res) in results {
        match res {
            Ok(FetchStatus::GitUpToDate) => git_up_to_date += 1,
            Ok(FetchStatus::GitUpdated { revision }) => git_updated.push((grammar_id, revision)),
            Ok(FetchStatus::NonGit) => non_git.push(grammar_id),
            Err(e) => errors.push((grammar_id, e)),
        }
    }

    non_git.sort_unstable();
    git_updated.sort_unstable_by(|a, b| a.0.cmp(&b.0));

    if git_up_to_date != 0 {
        println!("{} up to date git grammars", git_up_to_date);
    }

    if !non_git.is_empty() {
        println!("{} non git grammars", non_git.len());
        println!("\t{:?}", non_git);
    }

    if !git_updated.is_empty() {
        println!("{} updated grammars", git_updated.len());
        // We checked the vec is not empty, unwrapping will not panic
        let longest_id = git_updated.iter().map(|x| x.0.len()).max().unwrap();
        for (id, rev) in git_updated {
            println!(
                "\t{id:width$} now on {rev}",
                id = id,
                width = longest_id,
                rev = rev
            );
        }
    }

    if !errors.is_empty() {
        let len = errors.len();
        for (i, (grammar, error)) in errors.into_iter().enumerate() {
            // `{:#}`：anyhow 的 cause 链。只打 `{error}` 会丢掉真正的 io 错误，
            // 只剩一句 "Failed to execute C/C++ compiler"（交叉编译排查时吃过这个亏）。
            println!("Failure {}/{len}: {grammar} {error:#}", i + 1);
        }
        if strict {
            bail!("{len} grammars failed to fetch");
        }
    }

    Ok(())
}

pub fn build_grammars(target: Option<String>, strict: bool) -> Result<()> {
    ensure_git_is_available()?;

    let grammars = get_grammar_configs()?;

    let total = grammars.len();
    let counter = Arc::new(AtomicUsize::new(0));

    println!("Building {} grammars", grammars.len());

    let counter = Arc::clone(&counter);
    let results = run_parallel(grammars, move |grammar| {
        let current = counter.fetch_add(1, Ordering::Relaxed) + 1;

        println!(
            "Building grammars ({}/{}): {}",
            current, total, grammar.grammar_id
        );
        build_grammar(grammar, target.as_deref())
    });

    let mut errors = Vec::new();
    let mut already_built = 0;
    let mut built = Vec::new();

    for (grammar_id, res) in results {
        match res {
            Ok(BuildStatus::AlreadyBuilt) => already_built += 1,
            Ok(BuildStatus::Built) => built.push(grammar_id),
            Err(e) => errors.push((grammar_id, e)),
        }
    }

    built.sort_unstable();

    if already_built != 0 {
        println!("{} grammars already built", already_built);
    }

    if !built.is_empty() {
        println!("{} grammars built now", built.len());
        println!("\t{:?}", built);
    }

    if !errors.is_empty() {
        let len = errors.len();
        for (i, (grammar_id, error)) in errors.into_iter().enumerate() {
            // `{:#}`：带 cause 链（同上，否则看不出是哪个编译器/什么 errno）
            println!("Failure {}/{len}: {grammar_id} {error:#}", i + 1);
        }
        if strict {
            bail!("{len} grammars failed to build");
        }
    }

    Ok(())
}

// Returns the set of grammar configurations the user requests.
// Grammars are configured in the default and user `languages.toml` and are
// merged. The `grammar_selection` key of the config is then used to filter
// down all grammars into a subset of the user's choosing.
fn get_grammar_configs() -> Result<Vec<GrammarConfiguration>> {
    // `--grammar fetch/build` clones grammar sources from URLs in `languages.toml` and compiles
    // them into `.so` files helix later loads at runtime. If we let workspace
    // `.helix/languages.toml` in through `fully_trusted`, a malicious workspace could inject a
    // grammar with an attacker-controlled git source — running grammar build in that
    // directory would clone and compile attacker code
    let trust = crate::workspace_trust::WorkspaceTrust::new(Default::default());
    warn_if_workspace_languages_skipped(&trust);
    let config: Configuration = crate::config::user_lang_config(&trust)
        .context("Could not parse languages.toml")?
        .try_into()?;

    let grammars = match config.grammar_selection {
        Some(GrammarSelection::Only { only: selections }) => config
            .grammar
            .into_iter()
            .filter(|grammar| selections.contains(&grammar.grammar_id))
            .collect(),
        Some(GrammarSelection::Except { except: rejections }) => config
            .grammar
            .into_iter()
            .filter(|grammar| !rejections.contains(&grammar.grammar_id))
            .collect(),
        None => config.grammar,
    };

    Ok(grammars)
}

pub fn get_grammar_names() -> Result<Option<HashSet<String>>> {
    // See `get_grammar_configs`, same threat: workspace-local
    // `languages.toml` must not influence the grammar set without
    // explicit on-disk trust.
    let trust = crate::workspace_trust::WorkspaceTrust::new(Default::default());
    warn_if_workspace_languages_skipped(&trust);
    let config: Configuration = crate::config::user_lang_config(&trust)
        .context("Could not parse languages.toml")?
        .try_into()?;

    let grammars = match config.grammar_selection {
        Some(GrammarSelection::Only { only: selections }) => Some(selections),
        Some(GrammarSelection::Except { except: rejections }) => Some(
            config
                .grammar
                .into_iter()
                .map(|grammar| grammar.grammar_id)
                .filter(|id| !rejections.contains(id))
                .collect(),
        ),
        None => None,
    };

    Ok(grammars)
}

fn run_parallel<F, Res>(grammars: Vec<GrammarConfiguration>, job: F) -> Vec<(String, Result<Res>)>
where
    F: Fn(GrammarConfiguration) -> Result<Res> + Send + 'static + Clone,
    Res: Send + 'static,
{
    let pool = threadpool::Builder::new().build();
    let (tx, rx) = channel();

    for grammar in grammars {
        let tx = tx.clone();
        let job = job.clone();

        pool.execute(move || {
            // Ignore any SendErrors, if any job in another thread has encountered an
            // error the Receiver will be closed causing this send to fail.
            let _ = tx.send((grammar.grammar_id.clone(), job(grammar)));
        });
    }

    drop(tx);

    rx.iter().collect()
}

enum FetchStatus {
    GitUpToDate,
    GitUpdated { revision: String },
    NonGit,
}

#[derive(Copy, Clone)]
enum GitObjectFormat {
    Sha1,
    Sha256,
}

impl GitObjectFormat {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Sha1 => "sha1",
            Self::Sha256 => "sha256",
        }
    }
}

fn extract_object_format_from_revision(rev: &str) -> (GitObjectFormat, &str) {
    if let Some(stripped) = rev.strip_prefix("sha1:") {
        return (GitObjectFormat::Sha1, stripped);
    }

    if let Some(stripped) = rev.strip_prefix("sha256:") {
        return (GitObjectFormat::Sha256, stripped);
    }

    if rev.len() == 64 && rev.bytes().all(|b| b.is_ascii_hexdigit()) {
        return (GitObjectFormat::Sha256, rev);
    }

    (GitObjectFormat::Sha1, rev)
}

struct VendoredGrammar {
    dir: PathBuf,
}

impl VendoredGrammar {
    fn new(grammar: &str) -> Self {
        let dir = crate::runtime_dirs()
            .first()
            .expect("No runtime directories provided") // guaranteed by post-condition
            .join("grammars")
            .join("sources")
            .join(grammar);

        Self { dir }
    }

    /// Gets the current revision of the repo.
    fn revision(&self) -> Option<String> {
        git(&self.dir, ["rev-parse", "HEAD"]).ok()
    }

    /// Fetches grammar at the given revision.
    ///
    /// To ensure clean state, existing grammar directory is removed and re-inited
    /// before fetch operation.
    fn fetch(&self, remote: &str, rev: &str, object_format: GitObjectFormat) -> Result<()> {
        self.reinit(remote, object_format)?;

        git(&self.dir, ["fetch", "--depth", "1", REMOTE_NAME, rev])?;
        git(&self.dir, ["checkout", rev])?;

        Ok(())
    }

    /// Initializes the grammar directory.
    ///
    /// Creates directory and sets it up as a git repo, with remote set correctly.
    fn init(&self, remote: &str, object_format: GitObjectFormat) -> Result<()> {
        // Create the grammar directory if needed.
        fs::create_dir_all(&self.dir).context(format!(
            "Could not create grammar directory {:?}",
            &self.dir
        ))?;

        // Ensure directory is git initialized.
        if !self.dir.join(".git").exists() {
            git(
                &self.dir,
                ["init", "--object-format", object_format.as_str()],
            )?;
        }

        // Ensure the remote matches the configured remote, setting if needed.
        if self.remote().as_deref() != Some(remote) {
            self.set_remote(remote)?;
        }

        Ok(())
    }

    /// Removes the grammar directory before initializing again.
    fn reinit(&self, remote: &str, object_format: GitObjectFormat) -> Result<()> {
        fs::remove_dir_all(&self.dir)?;
        self.init(remote, object_format)?;
        Ok(())
    }

    /// Gets remote URL of grammar repo.
    fn remote(&self) -> Option<String> {
        git(&self.dir, ["remote", "get-url", REMOTE_NAME]).ok()
    }

    /// Sets remote URL of grammar repo.
    fn set_remote(&self, remote: &str) -> Result<()> {
        git(&self.dir, ["remote", "set-url", REMOTE_NAME, remote])
            .or_else(|_| git(&self.dir, ["remote", "add", REMOTE_NAME, remote]))?;
        Ok(())
    }
}

fn fetch_grammar(grammar: GrammarConfiguration) -> Result<FetchStatus> {
    let GrammarSource::Git {
        remote, revision, ..
    } = grammar.source
    else {
        return Ok(FetchStatus::NonGit);
    };

    let repo = VendoredGrammar::new(&grammar.grammar_id);

    let (object_format, revision) = extract_object_format_from_revision(&revision);

    // WARN: Must init before other operations are done.
    repo.init(&remote, object_format)?;

    if repo.revision().is_some_and(|rev| rev == revision) {
        return Ok(FetchStatus::GitUpToDate);
    }

    // Fetch the grammar if the revision doesn't match.
    repo.fetch(&remote, revision, object_format)?;

    Ok(FetchStatus::GitUpdated {
        revision: revision.to_string(),
    })
}

// A wrapper around 'git' commands which returns stdout in success and a
// helpful error message showing the command, stdout, and stderr in error.
fn git<I, S>(repository_dir: &Path, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let output = Command::new("git")
        .args(args)
        .current_dir(repository_dir)
        .output()?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_owned())
    } else {
        // TODO: figure out how to display the git command using `args`
        Err(anyhow!(
            "Git command failed.\nStdout: {}\nStderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        ))
    }
}

enum BuildStatus {
    AlreadyBuilt,
    Built,
}

/// 目标是不是「ELF 链接器 + 该加 RELRO」的那类（unix 但非 macOS/illumos）。
///
/// ⚠ **同样不能写 `cfg!`**：这段代码跑在 build script 里（永远按宿主编译），交叉编译到
/// Windows 时 `cfg!(unix)` 依然为真 ⇒ `-Wl,-z,relro,-z,now` 被喂给 mingw 的 ld，直接
/// `unrecognized option '-z'`（实测：301 个语法全挂在这一行）。与 [`dylib_extension_for`]
/// 是同一类坑 —— 判据必须是 `target` 参数，而不是编译这个 build script 时用的 cfg。
fn wants_relro(target: Option<&str>) -> bool {
    let Some(target) = target else {
        return cfg!(all(
            unix,
            not(any(target_os = "macos", target_os = "illumos"))
        ));
    };
    !target.contains("windows")
        && !target.contains("darwin")
        && !target.contains("apple")
        && !target.contains("wasm")
}

fn build_grammar(grammar: GrammarConfiguration, target: Option<&str>) -> Result<BuildStatus> {
    let grammar_dir = if let GrammarSource::Local { path } = &grammar.source {
        PathBuf::from(&path)
    } else {
        crate::runtime_dirs()
            .first()
            .expect("No runtime directories provided") // guaranteed by post-condition
            .join("grammars")
            .join("sources")
            .join(&grammar.grammar_id)
    };

    let grammar_dir_entries = grammar_dir.read_dir().with_context(|| {
        format!(
            "Failed to read directory {:?}. Did you use 'hx --grammar fetch'?",
            grammar_dir
        )
    })?;

    if grammar_dir_entries.count() == 0 {
        return Err(anyhow!(
            "Directory {:?} is empty. Did you use 'hx --grammar fetch'?",
            grammar_dir
        ));
    };

    let path = match &grammar.source {
        GrammarSource::Git {
            subpath: Some(subpath),
            ..
        } => grammar_dir.join(subpath),
        _ => grammar_dir,
    }
    .join("src");

    build_tree_sitter_library(&path, grammar, target)
}

fn build_tree_sitter_library(
    src_path: &Path,
    grammar: GrammarConfiguration,
    target: Option<&str>,
) -> Result<BuildStatus> {
    let header_path = src_path;
    let parser_path = src_path.join("parser.c");
    let mut scanner_path = src_path.join("scanner.c");

    let scanner_path = if scanner_path.exists() {
        Some(scanner_path)
    } else {
        scanner_path.set_extension("cc");
        if scanner_path.exists() {
            Some(scanner_path)
        } else {
            None
        }
    };
    let parser_lib_path = crate::runtime_dirs()
        .first()
        .expect("No runtime directories provided") // guaranteed by post-condition
        .join("grammars");
    let mut library_path = parser_lib_path.join(&grammar.grammar_id);
    // 按 **目标** 推扩展名（见 `dylib_extension_for` 的说明），不能用宿主常量
    library_path.set_extension(dylib_extension_for(target));

    // if we are running inside a buildscript emit cargo metadata
    // to detect if we are running from a buildscript check some env variables
    // that cargo only sets for build scripts
    if std::env::var("OUT_DIR").is_ok() && std::env::var("CARGO").is_ok() {
        if let Some(scanner_path) = scanner_path.as_ref().and_then(|path| path.to_str()) {
            println!("cargo:rerun-if-changed={scanner_path}");
        }
        if let Some(parser_path) = parser_path.to_str() {
            println!("cargo:rerun-if-changed={parser_path}");
        }
    }

    let recompile = needs_recompile(&library_path, &parser_path, scanner_path.as_ref())
        .context("Failed to compare source and binary timestamps")?;

    if !recompile {
        return Ok(BuildStatus::AlreadyBuilt);
    }

    let mut config = cc::Build::new();
    config
        .cpp(true)
        .opt_level(3)
        .cargo_metadata(false)
        .host(HOST_TARGET)
        .target(target.unwrap_or(BUILD_TARGET));
    let compiler = config.get_compiler();
    let mut command = Command::new(compiler.path());
    command.current_dir(src_path);
    for (key, value) in compiler.env() {
        command.env(key, value);
    }

    command.args(compiler.args());
    // used to delay dropping the temporary object file until after the compilation is complete
    let _path_guard;

    if compiler.is_like_msvc() {
        command
            .args(["/nologo", "/LD", "/I"])
            .arg(header_path)
            .arg("/utf-8")
            .arg("/std:c11");
        if let Some(scanner_path) = scanner_path.as_ref() {
            if scanner_path.extension() == Some("c".as_ref()) {
                command.arg(scanner_path);
            } else {
                let mut cpp_command = Command::new(compiler.path());
                cpp_command.current_dir(src_path);
                for (key, value) in compiler.env() {
                    cpp_command.env(key, value);
                }
                cpp_command.args(compiler.args());
                let object_file =
                    library_path.with_file_name(format!("{}_scanner.obj", &grammar.grammar_id));
                cpp_command
                    .args(["/nologo", "/LD", "/I"])
                    .arg(header_path)
                    .arg("/utf-8")
                    .arg("/std:c++14")
                    .arg(format!("/Fo{}", object_file.display()))
                    .arg("/c")
                    .arg(scanner_path);
                let output = cpp_command
                    .output()
                    .context("Failed to execute C++ compiler")?;

                if !output.status.success() {
                    return Err(anyhow!(
                        "Parser compilation failed.\nStdout: {}\nStderr: {}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    ));
                }
                command.arg(&object_file);
                _path_guard = TempPath::try_from_path(object_file).unwrap();
            }
        }

        command
            .arg(parser_path)
            .arg("/link")
            .arg(format!("/out:{}", library_path.to_str().unwrap()));
    } else {
        #[cfg(not(windows))]
        command.arg("-fPIC");

        command
            .arg("-shared")
            .arg("-fno-exceptions")
            .arg("-I")
            .arg(header_path)
            .arg("-o")
            .arg(&library_path);

        if let Some(scanner_path) = scanner_path.as_ref() {
            if scanner_path.extension() == Some("c".as_ref()) {
                command.arg("-xc").arg("-std=c11").arg(scanner_path);
            } else {
                let mut cpp_command = Command::new(compiler.path());
                cpp_command.current_dir(src_path);
                for (key, value) in compiler.env() {
                    cpp_command.env(key, value);
                }
                cpp_command.args(compiler.args());
                let object_file =
                    library_path.with_file_name(format!("{}_scanner.o", &grammar.grammar_id));

                #[cfg(not(windows))]
                cpp_command.arg("-fPIC");

                cpp_command
                    .arg("-fno-exceptions")
                    .arg("-I")
                    .arg(header_path)
                    .arg("-o")
                    .arg(&object_file)
                    .arg("-std=c++14")
                    .arg("-c")
                    .arg(scanner_path);
                let output = cpp_command
                    .output()
                    .context("Failed to execute C++ compiler")?;
                if !output.status.success() {
                    return Err(anyhow!(
                        "Parser compilation failed.\nStdout: {}\nStderr: {}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    ));
                }

                command.arg(&object_file);
                _path_guard = TempPath::try_from_path(object_file).unwrap();
            }
        }
        command.arg("-xc").arg("-std=c11").arg(parser_path);
        if wants_relro(target) {
            command.arg("-Wl,-z,relro,-z,now");
        }
    }

    let output = command
        .output()
        .context("Failed to execute C/C++ compiler")?;
    if !output.status.success() {
        return Err(anyhow!(
            "Parser compilation failed.\nStdout: {}\nStderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(BuildStatus::Built)
}

fn needs_recompile(
    lib_path: &Path,
    parser_c_path: &Path,
    scanner_path: Option<&PathBuf>,
) -> Result<bool> {
    if !lib_path.exists() {
        return Ok(true);
    }
    let lib_mtime = mtime(lib_path)?;
    if mtime(parser_c_path)? > lib_mtime {
        return Ok(true);
    }
    if let Some(scanner_path) = scanner_path {
        if mtime(scanner_path)? > lib_mtime {
            return Ok(true);
        }
    }
    Ok(false)
}

fn mtime(path: &Path) -> Result<SystemTime> {
    Ok(fs::metadata(path)?.modified()?)
}

/// Gives the contents of a file from a language's `runtime/queries/<lang>`
/// directory
pub fn load_runtime_file(language: &str, filename: &str) -> Result<String, std::io::Error> {
    let path = crate::runtime_file(PathBuf::new().join("queries").join(language).join(filename));
    std::fs::read_to_string(path)
}

#[cfg(test)]
mod tests {
    /// 生效的语法集合里**不该再有 sr.ht 宿主**。
    ///
    /// 由来（2026-11-04）：`git.sr.ht` 对 GitHub runner 回 `503`，而 `helix-term` 的 build script
    /// 会把 `languages.toml` 里所有未排除的语法一次性拉一遍 ⇒ 一个拉不到，`cargo check` /
    /// `cargo test` / `nix build` **全部**红。CI 实测：失败的恰好就是没被 `use-grammars.except`
    /// 排除的那几个 sr.ht 语法。
    ///
    /// 这条测试锁的是"机制真的生效"：`get_grammar_configs()` 会按 `use-grammars` 过滤，
    /// 所以只要有人往 `languages.toml` 加一个 sr.ht 语法却忘了排除，这里就会红并给出原因。
    #[test]
    fn effective_grammar_set_has_no_sr_ht_sources() {
        let grammars = super::get_grammar_configs().expect("languages.toml 应能解析");
        let offenders: Vec<(String, String)> = grammars
            .iter()
            .filter_map(|g| match &g.source {
                super::GrammarSource::Git { remote, .. } if remote.contains("sr.ht") => {
                    Some((g.grammar_id.clone(), remote.clone()))
                }
                _ => None,
            })
            .collect();
        assert!(
            offenders.is_empty(),
            "这些语法还在从 sr.ht 拉（CI 会 503 ⇒ 整个构建红），请加进 languages.toml 的 \
             `use-grammars.except`：{offenders:?}"
        );
    }
}
