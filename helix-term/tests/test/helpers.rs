use std::{
    io::{Read, Write},
    mem::replace,
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::bail;
use helix_core::{diagnostic::Severity, test, Selection, Transaction};
use helix_loader::workspace_trust::WorkspaceTrust;
use helix_term::{application::Application, args::Args, config::Config, keymap::merge_keys};
use helix_view::{
    current_ref, doc,
    editor::{ImplicitTrustLevelConfig, LspConfig, WordCompletion, WorkspaceTrustConfig},
    input::parse_macro,
    Editor,
};
use tempfile::NamedTempFile;
use tokio_stream::wrappers::UnboundedReceiverStream;

#[cfg(windows)]
use crossterm::event::{Event, KeyEvent};
#[cfg(not(windows))]
use termina::event::{Event, KeyEvent};

/// Specify how to set up the input text with line feeds
#[derive(Clone, Debug, Default)]
pub enum LineFeedHandling {
    /// 默认:按平台换行
    #[default]
    /// Replaces all LF chars with the system's appropriate line feed character,
    /// and if one doesn't exist already, appends the system's appropriate line
    /// ending to the end of a string.
    Native,

    /// Do not modify the input text in any way. What you give is what you test.
    AsIs,
}

impl LineFeedHandling {
    /// Apply the line feed handling to the input string, yielding a set of
    /// resulting texts with the appropriate line feed substitutions.
    pub fn apply(&self, text: &str) -> String {
        let line_end = match self {
            LineFeedHandling::Native => helix_core::NATIVE_LINE_ENDING,
            LineFeedHandling::AsIs => return text.into(),
        }
        .as_str();

        // we can assume that the source files in this code base will always
        // be LF, so indoc strings will always insert LF
        let mut output = text.replace('\n', line_end);

        if !output.ends_with(line_end) {
            output.push_str(line_end);
        }

        output
    }
}

#[derive(Clone, Debug)]
pub struct TestCase {
    pub in_text: String,
    pub in_selection: Selection,
    pub in_keys: String,
    pub out_text: String,
    pub out_selection: Selection,

    #[allow(dead_code)] // 目前没有测试读它(保留字段是为了将来按需切换换行处理)
    pub line_feed_handling: LineFeedHandling,
}

impl<S, R, V> From<(S, R, V)> for TestCase
where
    S: Into<String>,
    R: Into<String>,
    V: Into<String>,
{
    fn from((input, keys, output): (S, R, V)) -> Self {
        TestCase::from((input, keys, output, LineFeedHandling::default()))
    }
}

impl<S, R, V> From<(S, R, V, LineFeedHandling)> for TestCase
where
    S: Into<String>,
    R: Into<String>,
    V: Into<String>,
{
    fn from((input, keys, output, line_feed_handling): (S, R, V, LineFeedHandling)) -> Self {
        let (in_text, in_selection) = test::print(&line_feed_handling.apply(&input.into()));
        let (out_text, out_selection) = test::print(&line_feed_handling.apply(&output.into()));

        TestCase {
            in_text,
            in_selection,
            in_keys: keys.into(),
            out_text,
            out_selection,
            line_feed_handling,
        }
    }
}

#[inline]
pub async fn test_key_sequence(
    app: &mut Application,
    in_keys: Option<&str>,
    test_fn: Option<&dyn Fn(&Application)>,
    should_exit: bool,
) -> anyhow::Result<()> {
    test_key_sequences(app, vec![(in_keys, test_fn)], should_exit).await
}

#[allow(clippy::type_complexity)]
pub async fn test_key_sequences(
    app: &mut Application,
    inputs: Vec<(Option<&str>, Option<&dyn Fn(&Application)>)>,
    should_exit: bool,
) -> anyhow::Result<()> {
    const TIMEOUT: Duration = Duration::from_millis(500);
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    let num_inputs = inputs.len();

    for (i, (in_keys, test_fn)) in inputs.into_iter().enumerate() {
        let (view, doc) = current_ref!(app.editor);
        let state = test::plain(doc.text().slice(..), doc.selection(view.id));

        log::debug!("executing test with document state:\n\n-----\n\n{}", state);

        if let Some(in_keys) = in_keys {
            for key_event in parse_macro(in_keys)?.into_iter() {
                let key = Event::Key(KeyEvent::from(key_event));
                log::trace!("sending key: {:?}", key);
                tx.send(Ok(key))?;
            }
        }

        let app_exited = !app.event_loop_until_idle(&mut rx_stream).await;

        if !app_exited {
            let (view, doc) = current_ref!(app.editor);
            let state = test::plain(doc.text().slice(..), doc.selection(view.id));

            log::debug!(
                "finished running test with document state:\n\n-----\n\n{}",
                state
            );
        }

        // the app should not exit from any test until the last one
        if i < num_inputs - 1 && app_exited {
            bail!("application exited before test function could run");
        }

        // verify if it exited on the last iteration if it should have and
        // the inverse
        if i == num_inputs - 1 && app_exited != should_exit {
            bail!("expected app to exit: {} != {}", should_exit, app_exited);
        }

        if let Some(test) = test_fn {
            test(app);
        };
    }

    if !should_exit {
        for key_event in parse_macro("<esc>:q!<ret>")?.into_iter() {
            tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
        }

        let event_loop = app.event_loop(&mut rx_stream);
        tokio::time::timeout(TIMEOUT, event_loop).await?;
    }

    let errs = app.close().await;

    if !errs.is_empty() {
        log::error!("Errors closing app");

        for err in errs {
            log::error!("{}", err);
        }

        bail!("Error closing app");
    }

    Ok(())
}

pub async fn test_key_sequence_with_input_text<T: Into<TestCase>>(
    app: Option<Application>,
    test_case: T,
    test_fn: &dyn Fn(&Application),
    should_exit: bool,
) -> anyhow::Result<()> {
    let test_case = test_case.into();

    let mut app = match app {
        Some(app) => app,
        None => Application::new(
            Args::default(),
            test_config(),
            test_syntax_loader(None),
            WorkspaceTrust::fully_trusted(),
        )?,
    };

    let (view, doc) = helix_view::current!(app.editor);
    let sel = doc.selection(view.id).clone();

    // replace the initial text with the input text
    let transaction = Transaction::change_by_selection(doc.text(), &sel, |_| {
        (0, doc.text().len_chars(), Some((&test_case.in_text).into()))
    })
    .with_selection(test_case.in_selection.clone());

    doc.apply(&transaction, view.id);

    test_key_sequence(
        &mut app,
        Some(&test_case.in_keys),
        Some(test_fn),
        should_exit,
    )
    .await
}

/// Generates language config loader that merge in overrides, like a user language
/// config. The argument string must be a raw TOML document.
pub fn test_syntax_loader(overrides: Option<String>) -> helix_core::syntax::Loader {
    let mut lang = helix_loader::config::default_lang_config();

    if let Some(overrides) = overrides {
        let override_toml = toml::from_str(&overrides).unwrap();
        lang = helix_loader::merge_toml_values(lang, override_toml, 3);
    }

    helix_core::syntax::Loader::new(lang.try_into().unwrap()).unwrap()
}

/// LSP 测试配置：test_config 默认 lsp.enable=false，mock LSP 测试需开启。
pub fn test_config_with_lsp() -> Config {
    let mut config = test_config();
    config.editor.lsp.enable = true;
    config
}

/// mock LSP server loader：把 .mock 文件类型接到 mock_lsp 二进制（CARGO_BIN_EXE_mock_lsp），
/// argv[1]=场景、argv[2..]=场景附加参数（如第二文件路径）。
pub fn mock_lsp_loader(scenario: &str, extra_args: &[&str]) -> helix_core::syntax::Loader {
    let mut args = vec![scenario.to_string()];
    args.extend(extra_args.iter().map(|s| s.to_string()));
    // JSON 数组字面量是合法 TOML 内联数组；command 为测试二进制绝对路径
    let toml = format!(
        r#"
[[language]]
name = "mock"
scope = "source.mock"
file-types = ["mock"]
language-servers = ["mock-lsp"]

[language-server.mock-lsp]
command = "{bin}"
args = {args}
"#,
        bin = env!("CARGO_BIN_EXE_mock_lsp"),
        args = serde_json::to_string(&args).unwrap(),
    );
    test_syntax_loader(Some(toml))
}

/// 双 server mock LSP loader：.mock2 语言挂两个 mock server（tag A/B，均 code_actions_basic）——
/// 多 server code_actions 路由测试用。命令行为 argv[1]=场景、argv[2]=tag（与 mock_lsp_loader 的 extra_args 同槽）。
pub fn dual_mock_lsp_loader() -> helix_core::syntax::Loader {
    let toml = format!(
        r#"
[[language]]
name = "mock2"
scope = "source.mock2"
file-types = ["mock2"]
language-servers = ["mock-lsp-a", "mock-lsp-b"]

[language-server.mock-lsp-a]
command = "{bin}"
args = ["code_actions_basic", "A"]

[language-server.mock-lsp-b]
command = "{bin}"
args = ["code_actions_basic", "B"]
"#,
        bin = env!("CARGO_BIN_EXE_mock_lsp"),
    );
    test_syntax_loader(Some(toml))
}

/// mock_lsp_loader + 显式 timeout（秒）；超时测试用短值（如 1s）触发客户端 per-server timeout。
pub fn mock_lsp_loader_with_timeout(
    scenario: &str,
    extra_args: &[&str],
    timeout_secs: u64,
) -> helix_core::syntax::Loader {
    let mut args = vec![scenario.to_string()];
    args.extend(extra_args.iter().map(|s| s.to_string()));
    let toml = format!(
        r#"
[[language]]
name = "mock"
scope = "source.mock"
file-types = ["mock"]
language-servers = ["mock-lsp"]

[language-server.mock-lsp]
command = "{bin}"
args = {args}
timeout = {timeout}
"#,
        bin = env!("CARGO_BIN_EXE_mock_lsp"),
        args = serde_json::to_string(&args).unwrap(),
        timeout = timeout_secs,
    );
    test_syntax_loader(Some(toml))
}

/// Use this for very simple test cases where there is one input
/// document, selection, and sequence of key presses, and you just
/// want to verify the resulting document and selection.
pub async fn test_with_config<T: Into<TestCase>>(
    app_builder: AppBuilder,
    test_case: T,
) -> anyhow::Result<()> {
    let test_case = test_case.into();
    let app = app_builder.build()?;

    test_key_sequence_with_input_text(
        Some(app),
        test_case.clone(),
        &|app| {
            let doc = doc!(app.editor);
            assert_eq!(&test_case.out_text, doc.text());

            let mut selections: Vec<_> = doc.selections().values().cloned().collect();
            assert_eq!(1, selections.len());

            let sel = selections.pop().unwrap();
            assert_eq!(test_case.out_selection, sel);
        },
        false,
    )
    .await
}

pub async fn test<T: Into<TestCase>>(test_case: T) -> anyhow::Result<()> {
    test_with_config(AppBuilder::default(), test_case).await
}

pub fn temp_file_with_contents<S: AsRef<str>>(
    content: S,
) -> anyhow::Result<tempfile::NamedTempFile> {
    let mut temp_file = tempfile::NamedTempFile::new()?;

    temp_file
        .as_file_mut()
        .write_all(content.as_ref().as_bytes())?;

    temp_file.flush()?;
    temp_file.as_file_mut().sync_all()?;
    Ok(temp_file)
}

/// Generates a config with defaults more suitable for integration tests
pub fn test_config() -> Config {
    Config {
        editor: test_editor_config(),
        keys: helix_term::keymap::default(),
        ..Default::default()
    }
}

pub fn test_editor_config() -> helix_view::editor::Config {
    helix_view::editor::Config {
        lsp: LspConfig {
            enable: false,
            ..Default::default()
        },
        // The word-index hook accumulates per-document pending changes across
        // every `DocumentDidChange` and composes them on the next event for the
        // same doc id. Each test builds a fresh `Application` that reuses
        // `DocumentId(1)`, so the previous test's pending change tries to
        // compose with the next test's `set_input` change (different lengths,
        // hits the `len_after == len` assertion).
        // Until hooks can be unregistered, keep the hook quiet by turning the
        // feature off here.
        word_completion: WordCompletion {
            enable: false,
            ..Default::default()
        },
        // Trust everything implicitly so tests don't hit popups.
        workspace_trust: WorkspaceTrustConfig {
            level: ImplicitTrustLevelConfig::Insecure,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Creates a new temporary file that is set to read only. Useful for
/// testing write failures.
pub fn new_readonly_tempfile() -> anyhow::Result<NamedTempFile> {
    let mut file = tempfile::NamedTempFile::new()?;
    let metadata = file.as_file().metadata()?;
    let mut perms = metadata.permissions();
    perms.set_readonly(true);
    file.as_file_mut().set_permissions(perms)?;
    Ok(file)
}

/// Creates a new temporary file in the directory that is set to read only. Useful for
/// testing write failures.
pub fn new_readonly_tempfile_in_dir(
    dir: impl AsRef<std::path::Path>,
) -> anyhow::Result<NamedTempFile> {
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    let metadata = file.as_file().metadata()?;
    let mut perms = metadata.permissions();
    perms.set_readonly(true);
    file.as_file_mut().set_permissions(perms)?;
    Ok(file)
}

pub struct AppBuilder {
    args: Args,
    config: Config,
    syn_loader: helix_core::syntax::Loader,
    input: Option<(String, Selection)>,
}

impl Default for AppBuilder {
    fn default() -> Self {
        Self {
            args: Args::default(),
            config: test_config(),
            syn_loader: test_syntax_loader(None),
            input: None,
        }
    }
}

impl AppBuilder {
    pub fn new() -> Self {
        AppBuilder::default()
    }

    pub fn with_file<P: Into<PathBuf>>(
        mut self,
        path: P,
        pos: Option<helix_core::Position>,
    ) -> Self {
        self.args
            .files
            .insert(path.into(), vec![pos.unwrap_or_default()]);

        self
    }

    // Remove this attribute once `with_config` is used in a test:
    #[allow(dead_code)]
    pub fn with_config(mut self, mut config: Config) -> Self {
        let keys = replace(&mut config.keys, helix_term::keymap::default());
        merge_keys(&mut config.keys, keys);
        self.config = config;
        self
    }

    pub fn with_input_text<S: Into<String>>(mut self, input_text: S) -> Self {
        self.input = Some(test::print(&input_text.into()));
        self
    }

    pub fn with_lang_loader(mut self, syn_loader: helix_core::syntax::Loader) -> Self {
        self.syn_loader = syn_loader;
        self
    }

    pub fn build(self) -> anyhow::Result<Application> {
        if let Some(path) = &self.args.working_directory {
            bail!(
                "Changing the working directory to {path:?} is not yet supported for integration tests"
            );
        }

        if let Some((path, _)) = self.args.files.first().filter(|p| p.0.is_dir()) {
            bail!(
                "Having the directory {path:?} in args.files[0] is not yet supported for integration tests"
            );
        }

        let mut app = Application::new(
            self.args,
            self.config,
            self.syn_loader,
            WorkspaceTrust::fully_trusted(),
        )?;

        if let Some((text, selection)) = self.input {
            let (view, doc) = helix_view::current!(app.editor);
            let sel = doc.selection(view.id).clone();
            let trans = Transaction::change_by_selection(doc.text(), &sel, |_| {
                (0, doc.text().len_chars(), Some((text.clone()).into()))
            })
            .with_selection(selection);

            // replace the initial text with the input text
            doc.apply(&trans, view.id);
        }

        Ok(app)
    }
}

pub async fn run_event_loop_until_idle(app: &mut Application) {
    let (_, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    app.event_loop_until_idle(&mut rx_stream).await;
}

pub fn assert_file_has_content(file: &mut NamedTempFile, content: &str) -> anyhow::Result<()> {
    reload_file(file)?;

    let mut file_content = String::new();
    file.read_to_string(&mut file_content)?;
    assert_eq!(file_content, content);

    Ok(())
}

pub fn assert_status_not_error(editor: &Editor) {
    if let Some((_, sev)) = editor.get_status() {
        assert_ne!(&Severity::Error, sev);
    }
}

/// 仓库内置插件目录(`<repo>/plugins`)。
///
/// **为什么不用 `~/.config/helix/plugins`**:那是开发机的用户层,CI / 新机器上不存在。
/// 2026-10-17 实测:删掉开发机上的镜像后,filetree / window_rail 等 5 条集成测试
/// 直接红(`No such file or directory`)—— 说明它们从来没在干净环境里跑过。
/// 集成测试要**自包含**:读仓库里的插件源码,别读开发机配置。
pub fn repo_plugins_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugins")
}

/// 单个内置插件的路径,如 `repo_plugin("filetree/plugin.js")`。
pub fn repo_plugin(rel: &str) -> std::path::PathBuf {
    repo_plugins_dir().join(rel)
}

pub fn reload_file(file: &mut NamedTempFile) -> anyhow::Result<()> {
    let path = file.path();
    let f = std::fs::OpenOptions::new()
        .write(true)
        .read(true)
        .open(path)?;
    *file.as_file_mut() = f;
    Ok(())
}

/// 发送键序列并泵事件直到空闲。
///
/// G2:原先 12 个集成测试文件各复制一份(filetree / pane_modes / plugin_components /
/// plugin_layout / plugin_layout2 / plugin_terminal_hooks / plugin_theme /
/// server_manager / splits / window_mode / window_rail / window_split),语义完全相同
/// (`parse_macro` 返回 `Vec`,直接迭代与 `.into_iter()` 等价)。
/// 进程级 cwd 是**全局**状态:任何 `chdir` 都会打到**并行**跑的其它用例上。
///
/// 实测形态:`plugin_cross_buffer` 的相对路径用例在全量并行时会输 —— 它的 `:cd` 与另一个
/// 用例的 `:cd` 交错,cwd 在两条指令之间被换走;反过来,依赖"跑测试时的 cwd"的用例
/// (如 `:pick` 的文件列表)会读到别人临时目录里。这是**用例之间**的竞争,不是被测代码
/// 的问题 ⇒ 在 harness 层解决:凡是碰 cwd 的用例都必须先拿到这把锁。
///
/// 用 tokio 的 `Mutex`(不是 `std::sync`):锁要跨 `await` 持有,而
/// `std::sync::MutexGuard` 不是 `Send`,multi_thread 的测试容器会拒绝编译。
static CWD_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 插件配置状态**曾经**是进程级的（`CONFIG_SCHEMAS` / `PLUGIN_OVERRIDES` / `CONFIGS` 三个
/// `OnceLock`）⇒ 并行用例写不同的 `[plugins.<name>]` 会互相覆盖（实测：两个 terminal 尺寸
/// 用例同跑时一方读到对方的值；filetree 用例读到别人的 `show_hidden=true`）。
///
/// 2026-10-23 起三个缓存都搬进 `HostState`（**按宿主**，见 `helix-js/src/host.rs`）⇒ 串味消失。
/// 这把锁保留作双保险：凡给 `Config.plugins` 塞值、且在断言里读插件行为的用例仍先拿它。
///
/// ```ignore
/// let _cfg = plugin_config_lock().await;
/// ```
pub static PLUGIN_CONFIG_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 拿插件配置锁（见 [`PLUGIN_CONFIG_LOCK`]）
pub async fn plugin_config_lock() -> tokio::sync::MutexGuard<'static, ()> {
    PLUGIN_CONFIG_LOCK.lock().await
}

/// 只**读** cwd 的用例(不改它)拿这把锁,防止别的用例在此期间 `chdir` 走。
///
/// ```ignore
/// let _cwd = cwd_lock().await;   // 之后随便读 cwd / 起依赖 cwd 的插件
/// ```
///
/// ⚠️ 与 [`CwdScope`] 共用同一把锁:同一个用例里**不要**同时拿两者(会自锁)。
pub async fn cwd_lock() -> tokio::sync::MutexGuard<'static, ()> {
    CWD_LOCK.lock().await
}

/// cwd 独占权 + 还原点:构造时拿锁并(可选地)切到 `dir`,Drop 时**先还原 cwd 再放锁**。
///
/// 为什么合成一个入口:两步写法
/// `let _l = cwd_lock().await; let _c = CwdGuard::enter(dir)` 能被忘掉其中一步 ——
/// 忘了锁 ⇒ 并行下与别人的 `chdir` 交错;忘了 guard ⇒ cwd 永久被污染(连累后面所有用例)。
/// 一个返回值同时管两件事,忘不掉。
///
/// Drop 顺序是安全的:`Drop::drop` 的函数体先跑(此时锁还在手上),字段随后才释放。
pub struct CwdScope {
    _lock: tokio::sync::MutexGuard<'static, ()>,
    prev: std::path::PathBuf,
}

impl CwdScope {
    /// 拿锁 + 切到 `dir`(Drop 时还原到进入前的 cwd)
    pub async fn enter(dir: &std::path::Path) -> Self {
        let lock = cwd_lock().await;
        let prev = helix_stdx::env::current_working_dir();
        // 必须走 helix-stdx(它同时更新 CWD 缓存):裸 `std::env::set_current_dir`
        // 会让 `helix.cwd()` 读到缓存里的旧值 —— 测试会假绿/假红。
        helix_stdx::env::set_current_working_dir(dir).expect("chdir 到测试目录");
        Self { _lock: lock, prev }
    }

    /// 只拿锁 + 记还原点,**不改** cwd(用例自己用 `:cd` 改,例如验证 `cwd-change` 事件)
    pub async fn pinned() -> Self {
        let lock = cwd_lock().await;
        let prev = helix_stdx::env::current_working_dir();
        Self { _lock: lock, prev }
    }
}

impl Drop for CwdScope {
    fn drop(&mut self) {
        // 函数体先于字段释放 ⇒ 还原时锁仍在手上,还原与解锁之间没有窗口
        let _ = helix_stdx::env::set_current_working_dir(&self.prev);
    }
}

pub async fn pump(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(keys)? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    app.event_loop_until_idle(&mut rx_stream).await;
    Ok(())
}

/// 状态栏文本；没有状态时返回**空串**。
///
/// 断言异步结果时**不要**再写 `get_status().unwrap()` —— 那个 `unwrap` 正是
/// `plugin_async` / `plugin_lsp_mock` 在高并发下偶发 panic 的地方（结果还没回来）。
pub fn status_text(app: &Application) -> String {
    app.editor
        .get_status()
        .map(|(s, _)| s.to_string())
        .unwrap_or_default()
}

/// 发一次键，然后**等到条件成立**（有界）。条件成立的那一轮立即返回。
///
/// # 什么时候必须用它
///
/// 断言的值若来自 `helix.run_async` / `spawn` / `read_*_async` / `stat_async` /
/// `glob_async` / `read_tree` / `helix.lsp.*` / `helix.server.rows|task` 的**回调**，
/// 就必须用本函数。原因：`event_loop_until_idle` 的"idle" 是**250ms 内没有事件**
/// （integration 循环在每个事件后 `reset_idle_timer`，`IdleTimer` 一到就返回），
/// 不等于"worker 都干完了" —— 负载高时结果会晚于 idle 回来。
/// 用 `pump` + `get_status().unwrap()` 就是在赌"250ms 内一定到"。
///
/// 超时返回 Err，信息里带**预算 / 条件描述 / 最后一次状态栏**，比 `Option::unwrap`
/// 的 panic 能定位得多。
pub async fn pump_until(
    app: &mut Application,
    keys: &str,
    budget: Duration,
    what: &str,
    mut cond: impl FnMut(&mut Application) -> bool,
) -> anyhow::Result<()> {
    pump_until_inner(app, keys, budget, what, false, &mut cond).await
}

/// 同 [`pump_until`]，但**每轮重发 `keys`**（仅用于**幂等只读探针**，如 `:pick-diag<ret>`）。
///
/// 把有副作用的命令（`:async-demo` 之类）接进来会重复执行 —— 那属于误用。
pub async fn pump_until_probe(
    app: &mut Application,
    keys: &str,
    budget: Duration,
    what: &str,
    mut cond: impl FnMut(&mut Application) -> bool,
) -> anyhow::Result<()> {
    pump_until_inner(app, keys, budget, what, true, &mut cond).await
}

async fn pump_until_inner(
    app: &mut Application,
    keys: &str,
    budget: Duration,
    what: &str,
    repeat_keys: bool,
    cond: &mut impl FnMut(&mut Application) -> bool,
) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    let send = |keys: &str| -> anyhow::Result<()> {
        for key_event in parse_macro(keys)? {
            tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
        }
        Ok(())
    };
    if !repeat_keys {
        send(keys)?;
    }

    let t0 = Instant::now();
    let deadline = t0 + budget;
    let mut last_status = String::new();
    loop {
        // 先判：条件已成立就**不空转**（通过路径的耗时由这一行保证）
        if cond(app) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "pump_until 超时：预算 {budget:?}，实际等待 {:?}，条件：{what}；最后一次状态栏: {last_status:?}",
                t0.elapsed()
            );
        }
        if repeat_keys {
            send(keys)?;
        }
        if !app.event_loop_until_idle(&mut rx_stream).await {
            bail!("pump_until 等待期间应用退出（条件：{what}）");
        }
        last_status = status_text(app);
    }
}

/// 把 `area` 整块渲染成"每行一个字符串",供断言按行比对。
///
/// 必须先 `reset_plugin_diffs()`:DiffRenderer 是有状态的,不 reset 时第二次渲染
/// 只画出变化单元,取出来的行会少内容,断言随之误判。原先 7 个文件各复制一份,
/// 其中 plugin_components 那份漏了 reset(且**从未被调用**),合并时按可调用方
/// 的语义保留 reset。
pub fn render_rows(app: &mut Application, area: helix_view::graphics::Rect) -> Vec<String> {
    let mut buf = tui::buffer::Buffer::empty(area);
    app.compositor.reset_plugin_diffs();
    let mut jobs = helix_term::job::Jobs::new();
    let mut cx = helix_term::compositor::Context {
        editor: &mut app.editor,
        scroll: None,
        jobs: &mut jobs,
        js: None,
    };
    app.compositor.render(area, &mut buf, &mut cx);
    (0..area.height)
        .map(|y| {
            buf.content
                .iter()
                .skip(y as usize * area.width as usize)
                .take(area.width as usize)
                .map(|c| c.symbol.as_str())
                .collect::<String>()
        })
        .collect()
}
