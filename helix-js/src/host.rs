//! JS 引擎的**宿主**:把引擎的所有权从 thread_local 原语归到一个**显式对象**。
//!
//! 为什么值得:原来"引擎"只是一段 thread_local 状态,没有名字、无法指名、无法多份。
//! 归到 [`JsHost`] 之后,"一份 JS 运行时"成为一个可传递/可替换的对象 ——
//! `Application` 可以持有它(下一个任务)、测试可以构造多份。
//!
//! **当前仍有个默认宿主**(`DEFAULT_HOST`):`init()` 在没有外部宿主时自建,供测试与
//! 无 GUI 场景使用。
//!
//! # 状态所有权地图(G11)
//!
//! 改这里的任何状态前先问一句:**它是跨线程的吗?**
//!
//! | 状态 | 归属 | 为什么 |
//! |---|---|---|
//! | 引擎 / echo 消息 / 脚本导出 / 已加载清单 / 权限 | **宿主字段** | 每引擎一份;跨宿主共享会串状态(实测:`helix.load` 换根不失效,见 `with_script_exports`) |
//! | UI 请求队列 | **进程级 + 按宿主分桶** | 消费端在 helix-term(16 个冻结签名的 `take_ui_requests`),worker 也会 push;本体必须 `Send` |
//! | `PICKER_STATE` / `VIEWPORT` / `LAST_LAYOUT` / `CONFIGS` / `DIAGNOSTICS` | **进程级** | 消费端是 helix-term 的帧路径(每帧读 viewport/布局) |
//! | `TERM_EVENTS` / `ASYNC_EVENTS` / `LSP_RESULTS`(含 `_RX`) | **thread_local** | worker → 主线程通道,必须 `Send` |
//! | `TERM_WORKERS` / `TERM_MASTERS` / `NEXT_ASYNC_ID` | **thread_local** | pty 进程/fd 表,与"哪个 JS 宿主"无关 |
//! | `TERM_WAKE` | **进程级** | 跨线程唤醒回调(`request_redraw`) |
//!
//! 结论:**"进程级"对第二、三类是本质属性,不是遗漏**(宿主持有 boa `Context`,是 `!Send`)。
//! 完整论证与"若真需要两个隔离宿主 ⇒ 用两个进程"的结论见
//! `docs/superpowers/specs/2026-10-17-host-ownership-design.md`。
//!
//! ⚠️ **宿主永不 drop**(`ManuallyDrop`):引擎析构会释放 realm 堆,而 20 处**刻意不朽**的
//! JsValue 表(见 `state.rs`)经 Gc 句柄仍指向它 —— 实测(2026-10-11)drop 已用过的共享引擎
//! 会破坏堆(`tcache_thread_shutdown(): unaligned tcache chunk detected` /
//! `corrupted double-linked list`)。对照组:`dropping_a_fresh_context_is_safe` 证明
//! "新造 Context 的 drop"本身是安全的 ⇒ 问题在"引擎先走、引用者后走"。

/// 组件状态提供者表(组件 id → JSON 生成器)
pub(crate) type ComponentStateMap = HashMap<u64, Box<dyn Fn(u64) -> String>>;

use crate::types::{
    CursorRequest, DecorationRequest, DocSnapshot, Edit, NodeHandlers, PopupCallbacks,
    TermCallbacks, UiRequest,
};
use boa_engine::builtins::promise::ResolvingFunctions;
use boa_engine::{Context, JsValue};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::{Mutex, MutexGuard};

/// 一份 JS 运行时状态。
///
/// 只拥有**引擎与按宿主隔离的状态**(消息/导出/清单/权限);跨线程通道与帧路径缓存
/// 仍是进程级 —— 那是它们的本质属性,见模块头的「状态所有权地图」。
pub struct JsHost {
    /// 引擎(**普通 `RefCell<Context>`**)。`try_borrow_mut()` 就是**排他/重入检测** ——
    /// 引擎始终留在宿主里,不再有"借出期间槽为空"的中间态,也就不需要 `Owner`/`put_engine`
    /// 那套"该归还给谁"的判定。宿主字段级的可重入访问(`messages`/`exports`…)仍各走自己的
    /// `RefCell` ⇒ JS 回调里访问它们不会撞上引擎守卫。
    engine: RefCell<Context>,
    /// echo 消息队列(**按宿主**隔离 —— 原来是进程级 `OnceLock<Mutex<..>>`)。
    /// 访问点实测只在主线程(见 docs/superpowers/plans/2026-10-11-js-host-phaseB.md 的闸门结论),
    /// 所以用 `RefCell` 而不是 `Mutex`;若将来真有非主线程访问,它会先撞上"host not registered"。
    messages: RefCell<Vec<String>>,
    /// 加载缓存:脚本名 → `helix.export(...)` 的值(**按宿主**隔离;原来是 thread_local)
    exports: RefCell<HashMap<String, JsValue>>,
    /// 已加载脚本清单(名, 源码)—— 热重载按它重跑(**按宿主**隔离)
    loaded_scripts: RefCell<Vec<(String, String)>>,
    /// 打开的面板 id 列表(1b:原来是一张 `HashMap<宿主key, Vec<u64>>` 的进程级表)
    pub(crate) open_panels: RefCell<Vec<u64>>,
    /// 打开文档快照 JSON(1b:同款进程级键表 → 宿主字段)
    pub(crate) buffers: RefCell<String>,
    /// 平级模式快照 JSON(1b)
    pub(crate) pane_mode: RefCell<String>,
    /// pane 清单快照 JSON(1b)
    pub(crate) panes: RefCell<String>,
    /// 打开的终端:pty_id → (view_id, cmd)(1b;原来这张表的键是 pty id,但状态本就是宿主的)
    pub(crate) open_terms: RefCell<HashMap<u64, (u64, String)>>,
    /// 插件根(**有序**:自带 runtime 在前、用户目录在后)。1b:原为
    /// `HashMap<宿主key, &'static [PathBuf]>` 键表。值仍是 `Box::leak` 出来的 `&'static` ——
    /// 为了**不改** `plugin_roots() -> &'static [PathBuf]` 签名(7 个读取点依赖它);
    /// 设置是"启动一次/测试每例一次" ⇒ 泄漏有界(比原来"每次 set 都漏"更收敛)。
    pub(crate) plugin_roots: RefCell<&'static [PathBuf]>,
    /// runtime 目录(含自带 runtime;1b 同上)
    pub(crate) runtime_dirs: RefCell<&'static [PathBuf]>,
    /// 布局文件目录(`<config>/layouts`;未设置 ⇒ `None`。1b 同上)
    pub(crate) layouts_dir: RefCell<Option<&'static PathBuf>>,
    /// 插件命令注册表(name → export)。1b-3:原为 thread_local 的
    /// `RefCell<Option<&'static mut …>>`(惰性 init + `Box::leak`)⇒ 状态跟**宿主**而不是线程,且不再泄漏。
    pub(crate) registry: RefCell<HashMap<String, JsValue>>,
    /// 事件处理器(事件名 → 处理器列表)。1b-3 同上
    pub(crate) event_handlers: RefCell<HashMap<String, Vec<JsValue>>>,
    /// 弹窗/面板回调注册表(id → 回调)。1b-3 同上
    pub(crate) popups: RefCell<HashMap<u64, PopupCallbacks>>,
    /// keymap 前缀提示回调。1b-3 同上
    pub(crate) keymap_hint_hook: RefCell<Option<JsValue>>,
    /// 组件节点事件处理器(1b-3b:原 thread_local `Option<&'static mut …>` + leak)
    pub(crate) node_handlers: RefCell<HashMap<(u64, String), NodeHandlers>>,
    /// bufferline 图标钩子(1b-3b:原 thread_local `Option<&'static mut …>` + leak)
    pub(crate) buffer_icon_hook: RefCell<Option<JsValue>>,
    /// 补全图标钩子(1b-3b:原 thread_local `Option<&'static mut …>` + leak)
    pub(crate) completion_icon_hook: RefCell<Option<JsValue>>,
    /// 补全渲染钩子(1b-3b:原 thread_local `Option<&'static mut …>` + leak)
    pub(crate) completion_render_hook: RefCell<Option<JsValue>>,
    /// picker 源(onKey)(1b-3b:原 thread_local `Option<&'static mut …>` + leak)
    pub(crate) picker_sources: RefCell<HashMap<String, JsValue>>,
    /// 状态栏钩子(1b-3b:原 thread_local `Option<&'static mut …>` + leak)
    pub(crate) statusline_hook: RefCell<Option<JsValue>>,
    /// 终端回调(1b-3b:原 thread_local `Option<&'static mut …>` + leak)
    pub(crate) terms: RefCell<HashMap<u64, TermCallbacks>>,
    /// 终端 Promise(1b-3b:原 thread_local `Option<&'static mut …>` + leak)
    pub(crate) term_promises: RefCell<HashMap<u64, ResolvingFunctions>>,
    /// 最近一次 helix.export(1b-3b:原 thread_local `Option<&'static mut …>` + leak)
    pub(crate) last_export: RefCell<Option<JsValue>>,
    /// 异步 Promise(1b-3b:原 thread_local `Option<&'static mut …>` + leak)
    pub(crate) async_promises: RefCell<HashMap<u64, ResolvingFunctions>>,
    /// LSP Promise(1b-3b:原 thread_local `Option<&'static mut …>` + leak)
    pub(crate) lsp_promises: RefCell<HashMap<u64, ResolvingFunctions>>,
    /// server rows 一次性回调(1b-3b:原 thread_local `Option<&'static mut …>` + leak)
    pub(crate) server_rows_callbacks: RefCell<HashMap<u64, JsValue>>,
    /// server task 回调(1b-3b:原 thread_local `Option<&'static mut …>` + leak)
    pub(crate) server_task_callbacks: RefCell<HashMap<u64, JsValue>>,
    /// watch 回调(1b-3b:原 thread_local `Option<&'static mut …>` + leak)
    pub(crate) watch_callbacks: RefCell<HashMap<u64, JsValue>>,
    /// 1b-3c:原 thread_local(`LSP_REQUESTS`)
    pub(crate) lsp_requests: RefCell<Vec<crate::lsp::LspRequest>>,
    /// 1b-3c:原 thread_local(`INPUT_STATES`)
    pub(crate) input_states: RefCell<HashMap<(u64, String), crate::input::InputState>>,
    /// 1b-3c:原 thread_local;状态改跟**宿主**(不泄漏,但换线程不应丢)
    pub(crate) current_edits: RefCell<Vec<Edit>>,
    pub(crate) decoration_requests: RefCell<Vec<DecorationRequest>>,
    pub(crate) doc_snapshots: RefCell<Vec<DocSnapshot>>,
    pub(crate) cursor_requests: RefCell<Vec<CursorRequest>>,
    pub(crate) last_plugin_deps: RefCell<Vec<String>>,
    pub(crate) pending_command_args: RefCell<Vec<String>>,
    pub(crate) load_stack: RefCell<Vec<String>>,
    pub(crate) load_roots: RefCell<Vec<PathBuf>>,
    pub(crate) command_docs: RefCell<HashMap<String, String>>,
    pub(crate) theme_overrides: RefCell<HashMap<String, crate::theme::StyleOverride>>,
    pub(crate) component_states: RefCell<ComponentStateMap>,
    pub(crate) theme_dirty: Cell<bool>,
    pub(crate) statusline_replace: Cell<bool>,
    pub(crate) statusline_zones: RefCell<[u16; 3]>,
    pub(crate) edit_txn_depth: Cell<usize>,
    pub(crate) last_panel_id: Cell<Option<u64>>,
    pub(crate) next_popup_id: Cell<u64>,
    pub(crate) next_map_id: Cell<u64>,
    pub(crate) next_term_id: Cell<u64>,
    pub(crate) next_terminal_view_id: Cell<u64>,
    /// T0:原 `server_rows::NEXT_ID`(thread_local)—— server rows 一次性回调 id 计数器
    pub(crate) next_server_row_id: Cell<u64>,
    /// T0:原 `watch::NEXT_WATCH_ID`(thread_local)—— watcher id 计数器
    pub(crate) next_watch_id: Cell<u64>,
    /// C-1/C-2:能力清单与授权状态(按宿主隔离)
    pub(crate) permissions: RefCell<crate::permissions::PermissionState>,
    /// 编辑器版本(如 `25.07.1 (c4831272)`;由 helix-term 启动时推入)。
    /// 用于 manifest 的 `min_editor` 比较 —— helix-js 自己不知道宿主软件的版本。
    pub(crate) editor_version: RefCell<Option<String>>,
}

impl JsHost {
    /// 建宿主,并在其中注册 `helix` 全局(建引擎的代码在 `crate::build_engine`)。
    pub fn new() -> Self {
        let mut ctx = Context::default();
        crate::build_engine(&mut ctx);
        Self {
            engine: RefCell::new(ctx),
            messages: RefCell::new(Vec::new()),
            exports: RefCell::new(HashMap::new()),
            loaded_scripts: RefCell::new(Vec::new()),
            open_panels: RefCell::new(Vec::new()),
            buffers: RefCell::new(String::new()),
            pane_mode: RefCell::new(String::new()),
            panes: RefCell::new(String::new()),
            open_terms: RefCell::new(HashMap::new()),
            plugin_roots: RefCell::new(&[]),
            runtime_dirs: RefCell::new(&[]),
            layouts_dir: RefCell::new(None),
            registry: RefCell::new(HashMap::new()),
            event_handlers: RefCell::new(HashMap::new()),
            popups: RefCell::new(HashMap::new()),
            keymap_hint_hook: RefCell::new(None),
            node_handlers: RefCell::new(HashMap::new()),
            buffer_icon_hook: RefCell::new(None),
            completion_icon_hook: RefCell::new(None),
            completion_render_hook: RefCell::new(None),
            picker_sources: RefCell::new(HashMap::new()),
            statusline_hook: RefCell::new(None),
            terms: RefCell::new(HashMap::new()),
            term_promises: RefCell::new(HashMap::new()),
            last_export: RefCell::new(None),
            async_promises: RefCell::new(HashMap::new()),
            lsp_promises: RefCell::new(HashMap::new()),
            server_rows_callbacks: RefCell::new(HashMap::new()),
            server_task_callbacks: RefCell::new(HashMap::new()),
            watch_callbacks: RefCell::new(HashMap::new()),
            lsp_requests: RefCell::new(Vec::new()),
            input_states: RefCell::new(HashMap::new()),
            current_edits: RefCell::new(Vec::new()),
            decoration_requests: RefCell::new(Vec::new()),
            doc_snapshots: RefCell::new(Vec::new()),
            cursor_requests: RefCell::new(Vec::new()),
            last_plugin_deps: RefCell::new(Vec::new()),
            pending_command_args: RefCell::new(Vec::new()),
            load_stack: RefCell::new(Vec::new()),
            load_roots: RefCell::new(Vec::new()),
            command_docs: RefCell::new(HashMap::new()),
            theme_overrides: RefCell::new(HashMap::new()),
            component_states: RefCell::new(HashMap::new()),
            theme_dirty: Cell::new(false),
            statusline_replace: Cell::new(false),
            statusline_zones: RefCell::new([1, 1, 1]),
            edit_txn_depth: Cell::new(0),
            last_panel_id: Cell::new(None),
            next_popup_id: Cell::new(1),
            next_map_id: Cell::new(1),
            next_term_id: Cell::new(1),
            next_terminal_view_id: Cell::new(1),
            next_server_row_id: Cell::new(1),
            next_watch_id: Cell::new(1),
            permissions: RefCell::new(crate::permissions::PermissionState::default()),
            editor_version: RefCell::new(None),
        }
    }
}

thread_local! {
    /// **默认宿主**:`init()` 在没有外部宿主时自建(测试 / 无 GUI)。永不 drop,理由见模块头。
    static DEFAULT_HOST: ManuallyDrop<RefCell<Option<Box<JsHost>>>> =
        const { ManuallyDrop::new(RefCell::new(None)) };
}

impl Default for JsHost {
    fn default() -> Self {
        Self::new()
    }
}

thread_local! {
    /// **活动宿主**(可选):由 `Application` 注册。未注册时用 `DEFAULT_HOST`(测试 / 无 GUI)。
    ///
    /// ⚠️ 安全性约束:`set_active` 的调用者必须保证「注册期覆盖宿主生存期」——
    /// 宿主析构前要 `clear_active()`(`Application::drop` 已做,由
    /// `host_cleared_before_drop` 守着)。句柄悬空 = 未定义行为。
    static ACTIVE: Cell<Option<NonNull<JsHost>>> = const { Cell::new(None) };
}

/// 推入编辑器版本(启动期由 helix-term 调用一次)。
///
/// 为什么由外部推:`helix-js` 是库,不知道宿主软件的版本;而 manifest 的 `min_editor`
/// 只有跟真实版本比才有意义(此前只记录不比较 ⇒ 字段是装饰)。
pub fn set_editor_version(version: &str) {
    with_host_ref_or_init(|h| *h.editor_version.borrow_mut() = Some(version.to_string()));
}

/// 编辑器版本(未推入 ⇒ `None`;此时 `min_editor` 检查**跳过**,不误伤单测/库用法)
pub(crate) fn editor_version() -> Option<String> {
    crate::host::try_with_host_ref(|h| h.editor_version.borrow().clone()).flatten()
}

/// `ACTIVE` 的**唯一读点**。所有访问器经它 —— 裸指针的读点越少,审计面越小。
fn active_ptr() -> Option<NonNull<JsHost>> {
    ACTIVE.with(|a| a.get())
}

/// 把 `host` 注册为当前活动宿主(`Application` 用)。见 `ACTIVE` 的安全性约束。
pub fn set_active(host: &mut JsHost) {
    ACTIVE.with(|a| a.set(Some(NonNull::from(host))));
}

/// 注销活动宿主(必须在宿主析构**之前**调用)
pub fn clear_active() {
    ACTIVE.with(|a| a.set(None));
}

/// 安装默认宿主(由 `init()` 调用;幂等由调用方守 —— 已有宿主时不要覆盖)。
pub(crate) fn install_default(host: JsHost) {
    DEFAULT_HOST.with(|h| *h.borrow_mut() = Some(Box::new(host)));
}

/// **接管**已有宿主(若有) —— `Application` 优先用它,而不是新建一个引擎。
///
/// 为什么必要:集成测试每个用例都构造一个 `Application`。若每次新建引擎,
/// ①同线程会先后出现多份引擎(而注册表/队列**仍是进程级** ⇒ 拿不到任何隔离,
/// 只是白花内存与注册开销,并让"旧引擎的 JsValue 被新引擎调用"成为可能),
/// ②之前每线程一份、复用的行为被破坏。接管后:**一个线程永远只有一份引擎**。
pub fn take_existing() -> Option<Box<JsHost>> {
    DEFAULT_HOST.with(|cell| cell.borrow_mut().take())
}

/// 把宿主**停放**回默认槽(供下个 `Application` 接管)。
///
/// 槽已被占(罕见:同线程两个 Application 同时存在)或宿主不可停放时,**遗忘而不 drop** ——
/// 与模块根本不变式一致:引擎永不可 drop。
pub fn park_or_forget(host: Box<JsHost>) {
    let slot = DEFAULT_HOST.with(|cell| cell.borrow_mut().take());
    match slot {
        None => DEFAULT_HOST.with(|cell| *cell.borrow_mut() = Some(host)),
        Some(existing) => {
            DEFAULT_HOST.with(|cell| *cell.borrow_mut() = Some(existing));
            std::mem::forget(host);
        }
    }
}

/// **把宿主登记为活动宿主的守卫**(Drop 时恢复上一个)—— 兼作 panic 安全。
///
/// 为什么需要它:`helix-js` 内部有一整层“隐式解析当前宿主”的访问器
/// (`with_host_ref`/`with_engine` …),它们是历史形状,一下子改不完。
/// 泵方法(`JsHost::*`)用这个守卫把 `self` 绑定进那层隐式路径 ⇒ **外壳是显式排他的**
/// (要 `&mut JsHost`),内里继续用既有访问器。二者合起来才是 Phase D 的完整形状:
/// 入口排他靠借用检查器,内部实现不必推倒重来。
struct ActiveGuard {
    prev: Option<NonNull<JsHost>>,
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        ACTIVE.with(|a| a.set(self.prev));
    }
}

/// JS 的**泵入口**(Phase D-2):全部要求 `&mut JsHost`。
///
/// 关键性质:这些方法拿的是**独占借用**。同一个宿主在同一棵 Rust 调用树里
/// 只需两次就会撞上借用检查器 ⇒ “JS 回调里再调一次 `render_component`”
/// 不再是一个运行期 panic(借用冲突),而是**编译不过**。
/// 反例见 `helix-term/src/compositor.rs` 的 `Context::with_js` 文档与
/// `docs/superpowers/plans/2026-10-14-js-host-phaseD-and-boundary.md` §D-4。
impl JsHost {
    /// 在 `self` 上执行 `f`,期间 `self` 是活动宿主(嵌套时恢复)。
    fn pump<R>(&mut self, f: impl FnOnce() -> R) -> R {
        let prev = ACTIVE.with(|a| a.replace(Some(NonNull::from(&mut *self))));
        let _guard = ActiveGuard { prev };
        f()
    }

    /// 泵入口·运行插件命令(`helix.register_command` 注册的命令)
    pub fn run_command(
        &mut self,
        name: &str,
        ctx: &crate::types::CommandContext,
    ) -> anyhow::Result<bool> {
        self.pump(|| crate::commands::run_command(name, ctx))
    }

    /// 泵入口·兑现已就绪的 Promise 续体(`.then`/`.catch`)
    pub fn pump_jobs(&mut self) -> anyhow::Result<()> {
        self.pump(crate::shell::pump_jobs)
    }

    /// 泵入口·通知型事件(save / buffer-open / mode-change / theme-change …)
    pub fn emit_event(
        &mut self,
        name: &str,
        ctx: &crate::types::CommandContext,
        extra: Option<&str>,
    ) -> anyhow::Result<()> {
        self.pump(|| crate::commands::emit_event(name, ctx, extra))
    }

    /// 泵入口·`cwd-change`（工作目录**真的**变了；见 `apply_directory_change`）
    pub fn emit_cwd_change(
        &mut self,
        ctx: &crate::types::CommandContext,
        prev: Option<&std::path::Path>,
        cwd: &std::path::Path,
    ) -> anyhow::Result<()> {
        self.pump(|| crate::commands::emit_cwd_change(ctx, prev, cwd))
    }

    /// 泵入口·`doc-change`(带防抖窗口内合并的范围)
    pub fn emit_doc_change(
        &mut self,
        ctx: &crate::types::CommandContext,
        changes: &[crate::types::DocChange],
    ) -> anyhow::Result<()> {
        self.pump(|| crate::commands::emit_doc_change(ctx, changes))
    }

    /// 泵入口·`doc-change` 的**逐文档**版(显式带 doc id;后台 buffer 变更用)
    pub fn emit_doc_change_for(
        &mut self,
        ctx: &crate::types::CommandContext,
        doc_id: u64,
        changes: &[crate::types::DocChange],
    ) -> anyhow::Result<()> {
        self.pump(|| crate::commands::emit_doc_change_for(ctx, doc_id, changes))
    }

    /// 泵入口·`startup`;`ctx` 是**启动后的当前 buffer**(处理器据此判断"有没有打开文件")
    pub fn emit_startup(
        &mut self,
        ctx: &crate::types::CommandContext,
        open_paths: usize,
    ) -> anyhow::Result<()> {
        self.pump(|| crate::commands::emit_startup(ctx, open_paths))
    }

    /// 泵入口·`layout-change`;`kind` 是变更种类(`None` = 不携带)。
    ///
    /// 谁在用:**批量投递模式**的边界(`Compositor::flush_layout_change`,那里有 `cx.js`)。
    /// 默认的**逐点**模式不用它 —— 变更点在 27 个布局原语里,拿不到宿主 ⇒ 走隐式解析
    /// (见 `with_host_ref` 的分类文档)。
    pub fn emit_layout_change(&mut self, kind: Option<&'static str>) -> anyhow::Result<()> {
        self.pump(|| crate::commands::emit_layout_change_kind(kind))
    }

    /// 泵入口·渲染组件(返回 `Content`)
    ///
    /// # 重入为什么是**编译错**(D-4 的反例)
    ///
    /// 泵入口拿的是 `&mut JsHost`。JS 回调运行期间,引擎是从**同一个宿主**里借出去的
    /// (`RefCell` 借用守卫),所以回调再想进一次宿主、就得再要一份 `&mut JsHost` ——
    /// 而那份宿主正处于排他借用中。下面把“回调体”建模成一个必须与入口**同时存在**的闭包
    /// (而不是“入口返回后再调一次”—— 那种是先后调用,本来就没问题):
    ///
    /// ```compile_fail,E0501
    /// use helix_js::host::JsHost;
    ///
    /// /// 模型:一个“会回到回调里”的泵入口(实际形态就是 `host.*` 泵方法:
    /// /// 宿主被排他借走,回调由宿主自己调用)。
    /// fn js_entry(host: &mut JsHost, callback: &dyn Fn()) {
    ///     let _ = host;
    ///     callback();
    /// }
    ///
    /// fn from_js_callback(host: &mut JsHost) {
    ///     js_entry(host, &|| {
    ///         // 这里就是“JS 回调内部”:手上只有引擎借用,没有宿主。
    ///         // 再进一次 ⇒ 与外面那份排他借用冲突。
    ///         js_entry(host, &|| {});
    ///     });
    /// }
    /// ```
    ///
    /// 语言上的要点:**归属**的两种写法差别很大 —— “先 `render_component` 拿返回值、
    /// 再调一次”是合法的(借用已结束),而“在本次入口还在跑的时候再进一次”不合法。
    /// 后者就是重入,而这里用借用检查器挡住了它。
    ///
    /// 边界(不夸大):两条**旧的自由函数**(`helix_js::render_component` 等)走的是
    /// 那层隐式宿主解析,重入仍会在运行期报“engine used again while already borrowed”。
    /// 它们的存在理由是 D-3 那三类“拿不到 `cx`”的调用点;新代码应当走 `host.*` 泵方法。
    ///
    /// 对照:纯**查询**入口(`has_component_render`/`command_names`/`input_has_state`)
    /// 不拿宿主也不执行 JS,从回调里调它们合法、也不该被收进来。
    pub fn render_component(
        &mut self,
        id: u64,
        width: u16,
        height: u16,
        focus: Option<&str>,
    ) -> anyhow::Result<crate::types::Content> {
        self.pump(|| crate::popup::render_component(id, width, height, focus))
    }

    /// 泵入口·带输入签名缓存的组件渲染
    pub fn render_component_cached(
        &mut self,
        id: u64,
        width: u16,
        height: u16,
        focus: Option<&str>,
        sig: &str,
    ) -> anyhow::Result<crate::types::Content> {
        self.pump(|| crate::popup::render_component_cached(id, width, height, focus, sig))
    }

    /// 泵入口·渲染弹窗
    pub fn render_popup(
        &mut self,
        id: u64,
        width: u16,
        height: u16,
        focus: Option<&str>,
    ) -> anyhow::Result<crate::types::Content> {
        self.pump(|| crate::popup::render_popup(id, width, height, focus))
    }

    /// 泵入口·把按键送给节点回调(`onKey`)
    pub fn dispatch_input_key(
        &mut self,
        popup_id: u64,
        node_id: &str,
        key: &str,
    ) -> anyhow::Result<()> {
        self.pump(|| crate::input::dispatch_input_key(popup_id, node_id, key))
    }

    /// 泵入口·把粘贴文本送给节点回调
    pub fn dispatch_input_paste(
        &mut self,
        popup_id: u64,
        node_id: &str,
        text: &str,
    ) -> anyhow::Result<()> {
        self.pump(|| crate::input::dispatch_input_paste(popup_id, node_id, text))
    }

    /// 泵入口·把节点事件(`onPress`/`onChange`)送给回调
    pub fn dispatch_node_event(
        &mut self,
        view_id: u64,
        node_id: &str,
        key: Option<&str>,
    ) -> anyhow::Result<()> {
        self.pump(|| crate::commands::dispatch_node_event(view_id, node_id, key))
    }

    /// 泵入口·弹窗/面板节点按键(`onKey` + 节点路由),返回插件决定
    pub fn popup_key(
        &mut self,
        id: u64,
        key: &crate::types::PluginKey,
        ctx: &crate::types::CommandContext,
    ) -> anyhow::Result<crate::types::PopupKeyResult> {
        self.pump(|| crate::popup::popup_key(id, key, ctx))
    }

    /// 泵入口·组件节点上的鼠标/输入事件;返回 true = 插件消费了该事件
    pub fn emit_component_event(&mut self, id: u64, kind: &str, x: u16, y: u16) -> bool {
        self.pump(|| crate::commands::emit_component_event(id, kind, x, y))
    }

    /// 泵入口·终端按键钩子(`term-key`),返回决策
    pub fn emit_term_key(
        &mut self,
        pty_id: u64,
        code: &str,
        shift: bool,
        ctrl: bool,
        alt: bool,
    ) -> Option<crate::commands::TermKeyDecision> {
        self.pump(|| crate::commands::emit_term_key(pty_id, code, shift, ctrl, alt))
    }

    /// 泵入口·终端显示模式通知(`term-mode-change`)
    pub fn emit_term_mode(&mut self, pty_id: u64, mode: &str) {
        self.pump(|| crate::commands::emit_term_mode(pty_id, mode))
    }

    /// 泵入口·终端尺寸通知(`term-resize`)
    pub fn emit_term_resize(&mut self, pty_id: u64, rows: u16, cols: u16) {
        self.pump(|| crate::commands::emit_term_resize(pty_id, rows, cols))
    }

    /// 泵入口·终端标题通知(`term-title`)
    pub fn emit_term_title(&mut self, pty_id: u64, title: &str) {
        self.pump(|| crate::commands::emit_term_title(pty_id, title))
    }

    /// 泵入口·终端开门通知(`term-open`)
    pub fn emit_term_open(&mut self, pty_id: u64, cmd: &str) {
        self.pump(|| crate::commands::emit_term_open(pty_id, cmd))
    }

    /// 泵入口·兑现 term 事件(worker 回投)对应的 Promise
    pub fn resolve_term_event(
        &mut self,
        id: u64,
        event: crate::types::TermEvent,
    ) -> anyhow::Result<()> {
        self.pump(|| crate::shell::resolve_term_event(id, event))
    }

    /// 泵入口·兑现异步 fs 事件对应的 Promise
    pub fn resolve_async_event(
        &mut self,
        id: u64,
        event: crate::types::AsyncEvent,
    ) -> anyhow::Result<()> {
        self.pump(|| crate::shell::resolve_async_event(id, event))
    }

    /// 泵入口·兑现 LSP 请求对应的 Promise
    pub fn resolve_lsp(
        &mut self,
        id: u64,
        result: Result<Option<String>, String>,
    ) -> anyhow::Result<()> {
        self.pump(|| crate::lsp::resolve_lsp(id, result))
    }

    /// 泵入口·按**源码**加载脚本(启动引导 `init.js` 用)
    ///
    /// 边界说明:JS 侧的 `helix.load(name)`(插件依赖解析)走的是 `load_script_checked`,
    /// 它**不能**收宿主 —— 它是在引擎已借出、正在执行 JS 的时候被调用的(那正是
    /// "重入拿不到 `&mut JsHost`"的设计后果)。所以宿主显式只覆盖**Rust 侧**的加载入口。
    pub fn load_script(&mut self, src: &str) -> anyhow::Result<()> {
        self.pump(|| crate::commands::load_script(src))
    }

    /// 泵入口·按**名字**加载脚本(名字用于报错定位与热重载记录)
    pub fn load_script_named(&mut self, name: &str, src: &str) -> anyhow::Result<()> {
        self.pump(|| crate::commands::load_script_named(name, src))
    }

    /// 泵入口·`pane-mode-change`(平级模式切换;与 `helix.pane_mode.*` 快照同一来源)
    pub fn emit_pane_mode_change(&mut self, mode: &str) {
        self.pump(|| crate::commands::emit_pane_mode_change(mode));
    }

    /// 泵入口·缓冲区行图标(`helix.set_bufferline_icon`;返回值给渲染用)
    pub fn bufferline_icon(&mut self, path: Option<&str>) -> Option<String> {
        self.pump(|| crate::popup::bufferline_icon(path))
    }

    /// 泵入口·光标移动事件(`cursor-move`)
    pub fn emit_cursor_move(&mut self, doc_id: u64, row: usize, col: usize, mode: &str) -> bool {
        self.pump(|| crate::cursor::emit_cursor_move(doc_id, row, col, mode))
    }

    /// 泵入口·选区变化事件(`selection-change`)
    pub fn emit_selection_change(
        &mut self,
        doc_id: u64,
        count: usize,
        primary_row: usize,
        primary_col: usize,
    ) -> bool {
        self.pump(|| crate::cursor::emit_selection_change(doc_id, count, primary_row, primary_col))
    }

    /// 泵入口·热重载:重跑已加载的全部脚本(`:plugin-reload` / `:plugin reload`)
    pub fn reload_all(&mut self) -> anyhow::Result<()> {
        self.pump(crate::commands::reload_all)
    }

    // 注意:这里**没有** `picker_invoke_action` 的泵版本。
    // Picker 的 action 回调契约(`ui::picker::Picker::new` 的闭包)只给 `&mut Context`,**不带宿主**;
    // preview/onKey 的契约才带 `js: Option<&mut JsHost>`。所以 action 只能走自由函数
    // `crate::picker::invoke_action`(隐式解析宿主),这不是遗漏而是契约形状决定的。
    // 原有一个同名的泵方法从未被接线(2026-10-17 收尾体检发现并删除)。

    /// 泵入口·picker preview 回调(`define` 的 preview → `{path, line}`)
    pub fn picker_invoke_preview(
        &mut self,
        source: &str,
        payload: &[String],
    ) -> Option<(String, usize)> {
        self.pump(|| crate::picker::invoke_preview(source, payload))
    }

    /// 泵入口·picker `onKey` 钩子;返回 true = 插件消费了该键
    #[allow(clippy::too_many_arguments)]
    pub fn picker_key_hook(
        &mut self,
        source: &str,
        key: &str,
        ctrl: bool,
        alt: bool,
        shift: bool,
        query: &str,
        selected: i64,
    ) -> bool {
        self.pump(|| crate::picker::picker_key_hook(source, key, ctrl, alt, shift, query, selected))
    }

    /// 泵入口·把 `helix.server.rows` 的行数据回投给 JS 回调
    pub fn deliver_server_rows(
        &mut self,
        id: u64,
        rows: Vec<crate::types::ServerRow>,
    ) -> anyhow::Result<()> {
        self.pump(|| crate::server_rows::deliver(id, rows))
    }

    /// 泵入口·兑现 fs-watcher 事件对应的 JS 回调(`watch::resolve_watch_events`)
    pub fn resolve_watch_events(&mut self) -> anyhow::Result<()> {
        self.pump(crate::watch::resolve_watch_events)
    }

    /// 泵入口·兑现 server 后台任务事件对应的 JS 回调(`server_tasks::resolve_events`)
    pub fn resolve_server_task_events(&mut self) -> anyhow::Result<()> {
        self.pump(crate::server_tasks::resolve_events)
    }

    /// 泵入口·状态栏插件钩子(`helix.set_statusline`)
    pub fn statusline_parts(
        &mut self,
        ctx: &crate::types::StatuslineCtx,
    ) -> Option<Vec<crate::types::StatuslinePart>> {
        self.pump(|| crate::popup::statusline_parts(ctx))
    }
}

/// 同 [`with_host_ref`],但**没有宿主时先 `crate::init()`** —— 供 `set_*`/注册这类
/// "可能早于初始化"的调用使用(与原来的 `current_host_key_or_init()` 语义一致)。
pub(crate) fn with_host_ref_or_init<T>(f: impl FnOnce(&JsHost) -> T) -> T {
    if try_with_host_ref(|_| ()).is_none() {
        crate::init();
    }
    with_host_ref(f)
}

/// 与 [`with_host_ref`] 相同,但**没有宿主时返回 `None`**(而不是 panic)。
///
/// 给**查询类**入口用:它们被放在每帧路径上,**不得**为了回答一个布尔而顺手建宿主
/// (`crate::init()` 在没有宿主时会 `install_default(JsHost::new())` —— 整个 boa 引擎)。
pub(crate) fn try_with_host_ref<T>(f: impl FnOnce(&JsHost) -> T) -> Option<T> {
    if let Some(ptr) = active_ptr() {
        // SAFETY: 同 with_host_ref
        let host: &JsHost = unsafe { &*ptr.as_ptr() };
        return Some(f(host));
    }
    DEFAULT_HOST.with(|cell| cell.borrow().as_deref().map(f))
}

/// C-2:能力授权状态的宿主访问器(无宿主 ⇒ `None`,由 `permissions` 模块走兜底表)
pub(crate) fn try_with_permission_state<T>(
    f: impl FnOnce(&mut crate::permissions::PermissionState) -> T,
) -> Option<T> {
    try_with_host_ref(|h| f(&mut h.permissions.borrow_mut()))
}

/// 同 [`with_messages`],但无宿主时给 `None`(取走/查询路径不得建宿主)。
pub(crate) fn try_with_messages<T>(f: impl FnOnce(&mut Vec<String>) -> T) -> Option<T> {
    try_with_host_ref(|h| f(&mut h.messages.borrow_mut()))
}

/// **唯一的 legacy 隐式宿主解析**(Phase D 的**分类结论**;2026-10-16 全量核对过一遍)。
///
/// 现状:helix-term 侧 **35 个"会执行 JS"的入口全部有宿主显式形态**
/// (`JsHost::*` 泵方法,见上面的 `impl JsHost`)。仍然走这条隐式路径的只剩两类:
///
/// 1. **结构上拿不到宿主 / 为保真度而有意保留**(不是待办 —— 每条都写明"为什么"):
///    * `Compositor::sync_layout_cache` → `layout-change`:**为了逐点语义而故意**保留 ——
///      它挂在 27 个布局原语里,攒到帧首再发会让"同一批变更的多条事件都在最终布局上观察",
///      依赖变更点当时状态的插件会看到不一致的中间态(2026-10-16 因此改回逐点发);
///    * `plugin_terminal::feed` → `term-title`:调用方是 `job::dispatch_blocking` 的
///      **`Send` 闭包**(`&mut JsHost` 是 `!Send`);
///    * `picker::invoke_preview`(`ui/js_picker.rs`):preview 闭包的签名只给 `&Editor`;
///    * `completion_kind_icon` / `render_completion_row`(`ui/completion.rs`):在
///      `PickerColumn::format` 这个**格式化闭包**里被调,闭包存在 picker 里、比 `cx` 活得久;
///    * JS 侧 `helix.load(name)`:它在引擎**已经借出**时被调用 —— 正是"重入拿不到
///      `&mut JsHost`"这件事本身(即 D-4 反例的现实版本)。
/// 2. **不执行 JS,拿宿主只为定位队列/快照**:
///    * 状态读写:`take_*` / `set_viewport` / `set_picker_state` / `cache_*` /
///      `theme_overrides` / `set_layout_change_kind`(已删)/ `reset_theme` …;
///    * 纯查询:`has_handlers` / `has_component_render` / `input_has_state` /
///      `command_names` / `popup_exists` / `panel_has_onkey` …(走 `try_with_host_ref`:
///      无宿主 ⇒ **不建**宿主、给默认值);
///    * 通道/worker:`server_tasks::push_event` / `watch::push_watch_event` / `drain_*`(跨线程);
///    * 宿主生命周期:`set_active` / `clear_active` / `take_existing` / `park_or_forget`。
///
/// 换句话说:这条路的名字就是"**跨线程通道 / Send 闭包 / 比 cx 活得久的闭包专用**",
/// 而不是"新代码的顺手通道"。需要排他性证明的新代码应调 `JsHost::*` 泵方法。
/// 按**只读**方式访问当前宿主(不改所有权)。
pub(crate) fn with_host_ref<T>(f: impl FnOnce(&JsHost) -> T) -> T {
    if let Some(ptr) = active_ptr() {
        // SAFETY: 注册期覆盖宿主生存期(见 ACTIVE 的约束)
        let host: &JsHost = unsafe { &*ptr.as_ptr() };
        return f(host);
    }
    DEFAULT_HOST.with(|cell| match cell.borrow().as_deref() {
        Some(host) => f(host),
        None => panic!("helix-js: host not registered — call init() first"),
    })
}

/// 从 **helix-term 侧**入队一条要上状态栏的消息(与插件 `helix.echo` 同一个队列,
/// 由下个命令/事件边界 drain)。
///
/// 为什么需要一个公开入口:宿主显式的边界代码(如 `Compositor::flush_layout_change`
/// 发现"处理器反复改布局"要丢弃事件)**必须让失败可见** —— 只 `log::warn!` 等于静默
/// (本仓已有一次实证:静默的失败浪费了好几轮)。
pub fn push_message(msg: impl Into<String>) {
    with_messages(|v| v.push(msg.into()));
}

/// echo 消息队列的访问器(**按宿主**隔离)。入队 / 取走都走它。
pub(crate) fn with_messages<T>(f: impl FnOnce(&mut Vec<String>) -> T) -> T {
    with_host_ref(|h| f(&mut h.messages.borrow_mut()))
}

/// 加载缓存的访问器(**按宿主**隔离)。
///
/// `helix.load` 按**脚本名**命中这个缓存,而且**换根不会失效** ⇒ 它必须是每宿主一份:
/// 否则同一线程上先跑的测试会把属于**另一个引擎**的 export 留给后面的测试(T5-b1 的由来)。
pub(crate) fn with_script_exports<T>(f: impl FnOnce(&mut HashMap<String, JsValue>) -> T) -> T {
    with_host_ref(|h| f(&mut h.exports.borrow_mut()))
}

/// 已加载脚本清单的访问器(**按宿主**隔离)
pub(crate) fn with_loaded_scripts<T>(f: impl FnOnce(&mut Vec<(String, String)>) -> T) -> T {
    with_host_ref(|h| f(&mut h.loaded_scripts.borrow_mut()))
}

/// 当前宿主 key;**没有宿主 ⇒ None**(不进 init、不改状态)。
///
/// 给"跨线程投递按 task_id 路由"的回落用:宿主线程一定有宿主(Application::new 里
/// `set_active`),worker / 纯单测线程可能没有 —— 那种情况不能拿 `current_host_key()`
/// (会 panic),也不该为了"取个 key"在 worker 上 `init()` 建一份引擎。
pub(crate) fn try_current_host_key() -> Option<u64> {
    if let Some(ptr) = active_ptr() {
        return Some(ptr.as_ptr() as u64);
    }
    DEFAULT_HOST.with(|cell| {
        cell.borrow()
            .as_deref()
            .map(|host| host as *const JsHost as u64)
    })
}

/// 当前宿主的**稳定标识**(宿主永不 drop ⇒ 地址稳定且不会被复用)。
pub(crate) fn current_host_key() -> u64 {
    if let Some(ptr) = active_ptr() {
        return ptr.as_ptr() as u64;
    }
    DEFAULT_HOST.with(|cell| match cell.borrow().as_deref() {
        Some(host) => host as *const JsHost as u64,
        None => panic!("helix-js: host not registered — call init() first"),
    })
}

/// **按宿主分桶**的 UI 请求队列。
///
/// 为什么用"分桶"而不是"把 `UI_REQUESTS` 变成宿主的字段":这个队列的**消费端在 helix-term**
/// (16 个 `take_ui_requests()` 调用点,签名冻结),而宿主是 `!Send`;分桶让**63 个 push 点
/// 一行都不用改**(`UI_REQUESTS.get().unwrap().lock().unwrap().push(..)` 原样可用),
/// 同时让每个宿主(每个线程/每个 Application)只看见自己那一桶 —— 这正是过去那把
/// 全局 `PLUGIN_TEST_LOCK`(已删)手工模拟的隔离;分桶后由类型保证,不再需要串行。
pub(crate) struct UiRequests(Mutex<HashMap<u64, Vec<UiRequest>>>);

impl Default for UiRequests {
    fn default() -> Self {
        Self(Mutex::new(HashMap::new()))
    }
}

/// `UiRequests::lock()` 的返回类型 —— 单独起名只为避开 clippy::type_complexity 的超长签名。
pub(crate) type UiRequestsLock<'a> = Result<
    UiRequestsGuard<'a>,
    std::sync::PoisonError<MutexGuard<'a, HashMap<u64, Vec<UiRequest>>>>,
>;

impl UiRequests {
    pub(crate) fn lock(&self) -> UiRequestsLock<'_> {
        let key = current_host_key();
        self.0.lock().map(|map| UiRequestsGuard { map, key })
    }
}

pub(crate) struct UiRequestsGuard<'a> {
    map: MutexGuard<'a, HashMap<u64, Vec<UiRequest>>>,
    key: u64,
}

impl std::ops::Deref for UiRequestsGuard<'_> {
    type Target = Vec<UiRequest>;
    fn deref(&self) -> &Vec<UiRequest> {
        self.map.get(&self.key).expect("刚 entry 过")
    }
}
impl std::ops::DerefMut for UiRequestsGuard<'_> {
    fn deref_mut(&mut self) -> &mut Vec<UiRequest> {
        self.map.entry(self.key).or_default()
    }
}

/// **按宿主分桶**的"已打开面板 id"清单(与 `UiRequests` 同款做法)。
///
/// 原来是进程级 `OnceLock<Mutex<Vec<u64>>>`;`state.rs` 那条注释说"打开与 reload 可能在 tokio
/// 不同线程执行"—— 那条前提已在 T1 步骤 1 逐条查证并推翻(见
/// docs/superpowers/plans/2026-10-11-js-host-phaseB.md 的闸门结论)。分桶后,并发测试各看各的桶。
pub(crate) fn with_open_panels<T>(f: impl FnOnce(&mut Vec<u64>) -> T) -> T {
    with_host_ref(|h| f(&mut h.open_panels.borrow_mut()))
}

pub fn engine_exists() -> bool {
    active_ptr().is_some() || DEFAULT_HOST.with(|h| h.borrow().is_some())
}

/// 借出宿主里的引擎执行 `f`;**同一线程重入会得到指名道姓的报错**。
///
/// 排他靠 `JsHost::engine` 的 `RefCell` 借用守卫(`try_borrow_mut`),而不是"把引擎取出宿主槽":
/// 引擎始终留在宿主里 ⇒ 没有"借出期间槽为空"的中间态,也不需要 `Owner`/`put_engine` 判定
/// "该归还给谁";而重入仍是**运行时可识别**的(借用冲突 ⇒ 下面这条 panic 直指调用点)。
#[track_caller]
pub(crate) fn with_engine<T>(f: impl FnOnce(&mut Context) -> T) -> T {
    with_host_ref(|host| match host.engine.try_borrow_mut() {
        Ok(mut engine) => f(&mut engine),
        Err(_) => panic!(
            "helix-js: engine used again while already borrowed \
             (同一线程在 JS 调用树内重入;见 commands.rs 里 helix.load 的注释)"
        ),
    })
}

// ── 测试专用访问器 ────────────────────────────────────────────────
/// 测试专用:把默认宿主**摘掉**(模拟"未注册")。用 `forget` 而不是丢弃 ——
/// 直接 `= None` 会析构 `Box<JsHost>` ⇒ 触发"drop 已用过的引擎"那条破坏堆的路径。
#[cfg(test)]
pub(crate) fn clear_host_for_test() {
    DEFAULT_HOST.with(|cell| {
        if let Some(host) = cell.borrow_mut().take() {
            std::mem::forget(host);
        }
    });
}

#[cfg(test)]
mod engine_ownership_tests {
    use super::*;
    use boa_engine::Source;

    /// 引擎是**宿主持有的同一份**:跨多次借用状态不丢(注册的命令还在),且宿主身份不变。
    ///
    /// (改自 `engine_slot_is_owned_and_reusable`:那时靠"取出引擎"来证明所有权/可复用,
    /// 现在引擎是普通 `RefCell<Context>` 字段 ⇒ 用**身份 + 状态延续**来钉同一件事。)
    #[test]
    fn engine_stays_in_host_across_borrows() {
        let _g = crate::tests::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::init();
        let key_before = current_host_key();
        crate::load_script(r#"helix.register_command("survives-reborrow", () => {});"#)
            .expect("注册一个命令用做“引擎身份”的标记");

        assert!(
            crate::command_names()
                .iter()
                .any(|n| n == "survives-reborrow"),
            "跨借用后注册表应仍在(否则引擎被换掉了)"
        );
        assert_eq!(current_host_key(), key_before, "宿主身份不得变");
        with_engine(|e| {
            let v = e.eval(Source::from_bytes("typeof helix")).unwrap();
            assert_eq!(v.to_string(e).unwrap().to_std_string_escaped(), "object");
        });
    }

    /// `init()` 幂等:第二次不得重建引擎(重建会把注册命令/事件/键位全清掉)。
    #[test]
    fn init_is_idempotent_and_keeps_registry() {
        let _g = crate::tests::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::init();
        crate::load_script(r#"helix.register_command("keep-me", () => {});"#).unwrap();
        crate::init(); // 再来一次
        assert!(
            crate::command_names().iter().any(|n| n == "keep-me"),
            "init() 不得重建引擎"
        );
    }

    /// 重入必须**点名**:消息含 "again while already borrowed"。
    #[test]
    fn nested_borrow_is_named_not_opaque() {
        let _g = crate::tests::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::init();
        let err = std::panic::catch_unwind(|| with_engine(|_| with_engine(|_| {})))
            .expect_err("重入必须报错");
        let msg = err
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default();
        assert!(
            msg.contains("again while already borrowed"),
            "错误信息应点名重入,实得: {msg}"
        );
    }

    /// `f` 里 panic ⇒ 借出的引擎**必须归还**(挡"不用 RAII"的实现)。
    #[test]
    fn engine_returned_after_panic_in_closure() {
        let _g = crate::tests::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::init();
        let r = std::panic::catch_unwind(|| with_engine(|_| panic!("boom")));
        assert!(r.is_err(), "闭包应当 panic");
        with_engine(|e| assert!(e.eval(Source::from_bytes("1+1")).is_ok()));
    }

    /// **宿主未注册**时任何入口都要明确报错(不是 UB、不是静默 no-op)。
    /// 末尾必须恢复宿主 —— 同进程还有别的测试要用它。
    #[test]
    fn unregistered_host_reports_clearly() {
        let _g = crate::tests::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::init();
        clear_host_for_test();
        let err =
            std::panic::catch_unwind(|| with_engine(|_| {})).expect_err("未注册宿主时必须报错");
        let msg = err
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default();
        // 恢复:重建宿主,免得后面的测试全挂
        install_default(JsHost::new());
        assert!(
            msg.contains("host not registered"),
            "未注册宿主的信息应点名,实得: {msg}"
        );
    }

    /// **借出期间**调用 `init()` 不得重建宿主:宿主在借出期间**仍然存在**(只是引擎被取走)
    /// ⇒ `init()` 必须幂等早退。
    ///
    /// 这条比 Steps 1 的同名测试更硬:若真去重建,`install_default` 会**替换**旧的
    /// `Box<JsHost>` ⇒ 析构一个**正被借出**的引擎 = 注释里那条破坏堆的路径。
    #[test]
    fn init_while_borrowed_does_not_rebuild_host_engine() {
        let _g = crate::tests::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::init();
        let key_before = current_host_key();
        with_engine(|_| {
            crate::init(); // 借出期间再调
            assert_eq!(
                current_host_key(),
                key_before,
                "init() 不得在引擎借出期间重建(宿主)"
            );
        });
        with_engine(|e| assert!(e.eval(Source::from_bytes("1+1")).is_ok()));
    }

    /// **两个宿主的引擎互不干扰**(Task 3:隔离性的第一个可测证据)。
    ///
    /// 它同时是「活动宿主句柄」的行为测试:切到新宿主后,旧引擎里的**全局变量**看不见;
    /// 切回来又看得见。
    ///
    /// ⚠️ **边界(实测)**:命令/事件/钩子的注册表(`REGISTRY`/`EVENT_HANDLERS`…)
    /// **仍在进程级**、不随宿主走 ⇒ 今天能断言的隔离只是**引擎级**的。
    /// 第一版测试写成断言「命令隔离」⇒ 当场红 —— 这个红正好把真实边界钉了下来。
    /// 为什么**不再**计划把它们搬进宿主:见模块头「状态所有权地图」与
    /// `docs/superpowers/specs/2026-10-17-host-ownership-design.md` §0.1。
    #[test]
    fn two_hosts_are_isolated() {
        let _g = crate::tests::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::init();
        with_engine(|e| {
            e.eval(Source::from_bytes("globalThis.marker = 1;"))
                .unwrap();
        });

        let mut other = JsHost::new();
        set_active(&mut other);
        with_engine(|e| {
            let v = e
                .eval(Source::from_bytes("typeof globalThis.marker"))
                .unwrap();
            assert_eq!(
                v.to_string(e).unwrap().to_std_string_escaped(),
                "undefined",
                "新宿主的引擎里不该有旧宿主的全局变量(引擎级隔离)"
            );
        });

        clear_active();
        with_engine(|e| {
            let v = e
                .eval(Source::from_bytes("String(globalThis.marker)"))
                .unwrap();
            assert_eq!(
                v.to_string(e).unwrap().to_std_string_escaped(),
                "1",
                "切回后旧引擎状态应完好"
            );
        });
        // 引擎不可 drop ⇒ 把宿主安全带出去(既不 drop 也不让句柄悬空)
        std::mem::forget(other);
    }

    /// **负结果记录**:注册表是进程级(不随宿主持有),里面存的是 `JsValue`;Task 2 之后
    /// 同一线程上会先后存在多个引擎,于是"用**新引擎**去调**旧引擎**的 JsValue"是可能的。
    /// 本测试就是那条路径 —— 结论:**boa 容忍它,不会立即崩**。
    ///
    /// 为什么留着它:并发 SIGABRT 排查时,"跨引擎 JsValue"是第一个被怀疑的机制;
    /// 有了这条测试,下次不必再重新推一遍(它同时守住这条路径继续可用)。
    #[test]
    fn cross_engine_jsvalue_is_dangerous() {
        let _g = crate::tests::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::init();
        // ① 在宿主 A 的引擎里注册命令(REGISTRY 里留下一个属于 A 的 JsValue)
        crate::load_script(r#"helix.register_command("cross-probe", () => helix.echo("x"));"#)
            .unwrap();
        // ② 同一线程换到新宿主 B
        let mut other = JsHost::new();
        set_active(&mut other);
        // ③ 用 **B 的引擎**去调 **A 的 JsValue**
        let ctx = crate::CommandContext {
            docs: vec![],
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        let r = crate::run_command("cross-probe", &ctx);
        assert!(
            r.is_ok(),
            "跨引擎调用目前是**可用**的(负结果;若某天 boa 收紧,这条会红)"
        );
        let _ = crate::take_messages();
        clear_active();
        std::mem::forget(other);
    }

    /// **接管/停放往返后必须是同一个引擎**(这是"一个线程一份引擎"的机制保证)。
    ///
    /// 由来:集成测试每个用例都构造一个 `Application`;若每个都新建引擎,同线程会先后出现多份
    /// 引擎 —— 而注册表**仍是进程级**,多份引擎换不来隔离,只白花内存与注册开销
    /// (哪些状态按宿主、哪些必须进程级:见模块头「状态所有权地图」)。
    #[test]
    fn park_and_take_reuses_the_same_engine() {
        let _g = crate::tests::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::init();
        with_engine(|e| {
            e.eval(Source::from_bytes("globalThis.reuse_marker = 7;"))
                .unwrap();
        });

        let host = take_existing().expect("init() 之后应有默认宿主可接管");
        assert!(take_existing().is_none(), "接管后槽应为空");

        park_or_forget(host);
        with_engine(|e| {
            let v = e
                .eval(Source::from_bytes("String(globalThis.reuse_marker)"))
                .unwrap();
            assert_eq!(
                v.to_string(e).unwrap().to_std_string_escaped(),
                "7",
                "接管 + 停放之后必须还是**同一个**引擎"
            );
        });
    }

    /// **宿主 key 随活动宿主切换,并在 `clear_active` 后回落到默认宿主**(G11a 不变量)。
    ///
    /// 为什么值得单独钉:`current_host_key` 是「按宿主分桶」的唯一标识(UI 请求、
    /// 跨线程投递路由都用它)。它若在切换后漂移,分桶就会串 —— 而这条**没有**任何
    /// 直接的断言保护过。
    #[test]
    fn host_key_is_stable_per_host_and_falls_back() {
        let _g = crate::tests::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::init();
        let default_key = current_host_key();
        assert_eq!(
            default_key,
            current_host_key(),
            "同一宿主两次取 key 必须相同(按宿主分桶的前提)"
        );

        let mut other = JsHost::new();
        set_active(&mut other);
        let other_key = current_host_key();
        assert_ne!(other_key, default_key, "切到别的宿主后 key 必须变");

        clear_active();
        assert_eq!(
            current_host_key(),
            default_key,
            "clear_active 后应回落到默认宿主(同一份引擎 ⇒ 同一个 key)"
        );
        std::mem::forget(other);
    }

    /// **查询类入口不得顺手建宿主**(G11a 不变量)。
    ///
    /// 由来:`init()` 在没有宿主时会 `install_default(JsHost::new())` —— 那是整个 boa 引擎。
    /// 查询类入口(`engine_exists` / `try_current_host_key` / `try_with_*`)被放在每帧路径上,
    /// 为一个布尔建引擎是静默的性能事故。
    ///
    /// 做法:**在一条干净线程上**验(thread_local 的 `DEFAULT_HOST` 在该线程为空),
    /// 这样不用动测试线程的默认宿主、也就不会和并行用例互相踩。
    #[test]
    fn queries_do_not_create_host() {
        std::thread::spawn(|| {
            assert!(!engine_exists(), "无宿主时 engine_exists() 必须为 false");
            assert!(
                try_current_host_key().is_none(),
                "无宿主时 try_current_host_key() 必须给 None(而不是 init())"
            );
            assert!(
                try_with_host_ref(|_| ()).is_none(),
                "无宿主时 try_with_host_ref() 必须给 None"
            );
            // 关键:以上三个查询不得留下宿主
            assert!(!engine_exists(), "查询之后仍然不得有宿主");
        })
        .join()
        .expect("线程内断言失败");
    }

    /// **队列要按宿主隔离,而不是进程级共享**(T1 步骤 2 的目标行为)。
    ///
    /// 今天是 `OnceLock<Mutex<Vec<String>>>`(进程级) ⇒ 一个线程 `echo` 的消息,
    /// **另一个线程**能取到 ⇒ 本测试会红。搬进宿主后,另一线程自建的宿主看不到它 ⇒ 绿。
    #[test]
    fn messages_are_per_host_not_process_global() {
        let _g = crate::tests::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::init();
        crate::load_script(r#"helix.echo("from-main-thread");"#).unwrap();

        let got = std::thread::spawn(|| {
            crate::init(); // 该线程自建宿主(与主线程那份是两份)
            crate::take_messages()
        })
        .join()
        .expect("子线程不应 panic");

        assert!(
            got.is_empty(),
            "队列应是**按宿主**隔离的,而不是进程级共享;实得 {got:?}"
        );
        let _ = crate::take_messages(); // 清掉主线程那条,免得污染后续测试
    }

    /// **UI 请求队列也要按宿主隔离**(T1 步骤 3 的目标行为)。
    ///
    /// 今天它是进程级 `OnceLock<Mutex<..>>` ⇒ 主线程排的请求,子线程能取到 ⇒ 本测试红。
    #[test]
    fn ui_requests_are_per_host_not_process_global() {
        let _g = crate::tests::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::init();
        // 走全局名字的队列 —— 它已按宿主分桶(见 UiRequests),所以这行无需改动
        crate::state::UI_REQUESTS
            .get()
            .unwrap()
            .lock()
            .unwrap()
            .push(crate::types::UiRequest::Quit);

        let got = std::thread::spawn(|| {
            crate::init(); // 该线程自建宿主
            crate::take_ui_requests()
        })
        .join()
        .expect("子线程不应 panic");

        assert!(
            got.is_empty(),
            "UI 请求队列应**按宿主**隔离,而不是进程级共享;实得 {} 项",
            got.len()
        );
        let _ = crate::take_ui_requests();
    }

    /// **打开的面板清单也要按宿主隔离**(T1 步骤 4 的目标行为)。
    ///
    /// 今天它是进程级 `OnceLock<Mutex<Vec<u64>>>` ⇒ 主线程记下的面板 id,子线程能读到
    /// ⇒ 并发测试会互相"抢面板"(任务 1 时代留下的不一致)。
    #[test]
    fn open_panels_are_per_host_not_process_global() {
        let _g = crate::tests::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::init();
        crate::state::with_open_panels(|p| p.push(42));

        let got = std::thread::spawn(|| {
            crate::init(); // 该线程自建宿主
            crate::state::with_open_panels(|p| p.clone())
        })
        .join()
        .expect("子线程不应 panic");

        assert!(
            got.is_empty(),
            "面板清单应按宿主隔离,而不是进程级共享;实得 {got:?}"
        );
        let _ = crate::state::with_open_panels(std::mem::take);
    }

    /// **布局目录也要按宿主隔离**(T5-a 的目标行为)。
    ///
    /// 今天它是进程级 `OnceLock<PathBuf>`(首设生效、不可重置)⇒ 并行跑时"谁先跑谁赢",
    /// 后面的测试静默拿到别人的目录 —— 这正是本会话 5 个"单用例偶发"的机制。
    #[test]
    fn layouts_dir_is_per_host_not_process_global() {
        let _g = crate::tests::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::init();
        crate::state::set_layouts_dir(std::path::PathBuf::from("/tmp/LAYOUTS-RED"));
        assert_eq!(
            crate::state::layouts_dir().map(|p| p.to_string_lossy().to_string()),
            Some("/tmp/LAYOUTS-RED".to_string()),
            "本宿主应看到自己刚设的目录"
        );

        let got = std::thread::spawn(|| {
            crate::init(); // 该线程自建宿主
            crate::state::layouts_dir().map(|p| p.to_string_lossy().to_string())
        })
        .join()
        .expect("子线程不应 panic");

        // ⚠️ 结尾必须覆盖回默认:本测试改的是**本宿主的桶**,不清会污染同进程其它用到 layouts 的测试
        crate::state::set_layouts_dir(std::env::temp_dir());
        assert!(
            got.is_none(),
            "另一线程的宿主不该看到别人设的 layouts 目录;实得 {got:?}"
        );
    }

    /// **加载缓存 / 已加载脚本清单要按宿主隔离**(T5-b1)。
    ///
    /// 关键:必须**同一线程换宿主**才算数 —— 第一版测试用子线程,天然被 `thread_local` 隔离,
    /// 于是"改造前就通过"(一条关于测试本身的发现,已记入账本)。
    /// `SCRIPT_EXPORTS` 按**脚本名**命中、换根不失效 ⇒ 同一线程上先跑的测试会把属于**另一个引擎**
    /// 的 export 留在缓存里,后面的测试 `helix.load(同名)` 拿到陈旧对象 —— 这正是 4 个 `reload_*`
    /// 测试删掉"跳过 guard"后变红的原因。
    #[test]
    fn load_caches_are_per_host_not_process_global() {
        let _g = crate::tests::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::init();
        crate::state::with_script_exports(|m| {
            m.insert(
                "probe.js".to_string(),
                boa_engine::JsValue::from(boa_engine::JsString::from("A")),
            );
        });
        crate::state::with_loaded_scripts(|s| s.push(("probe.js".to_string(), "src".to_string())));

        // 同一线程换到另一个宿主(真实污染场景:同一线程先后建多个宿主 —— 串行跑时
        // 所有测试共用同一条线程)
        let mut other = JsHost::new();
        set_active(&mut other);
        let got = (
            crate::state::with_script_exports(|m| m.contains_key("probe.js")),
            crate::state::with_loaded_scripts(|s| s.iter().any(|(n, _)| n == "probe.js")),
        );
        clear_active();
        std::mem::forget(other); // 引擎不可 drop

        // ⚠️ 清理本宿主的桶,免得污染同进程其它测试
        crate::state::with_script_exports(|m| {
            m.remove("probe.js");
        });
        crate::state::with_loaded_scripts(|s| s.retain(|(n, _)| n != "probe.js"));
        assert_eq!(got, (false, false), "加载缓存应按宿主隔离;实得 {got:?}");
    }

    /// `BUFFERS`(打开文档快照)必须按宿主隔离 —— 这张表是 `helix.buffer.list()` /
    /// `helix.buffer.current()` 的数据源,原先是**进程级** `OnceLock<Mutex<String>>`:
    /// 并行跑时"谁最后写谁赢",一个用例会读到**别人的** buffer 列表(实测:
    /// `plugin_terminal_hooks` 的 `buffer_traversal_and_focus` 拿到另一个用例的 `z.txt`)。
    ///
    /// ⚠️ 断言必须**比身份**(各自看到各自那份),不能只比"空/非空" —— 我第一版就是先把自己那份
    /// 清成空串再换宿主,于是"另一宿主读到空"必然成立 ⇒ **假绿**(本会话第三次踩同类陷阱)。
    #[test]
    fn buffers_are_per_host_not_process_global() {
        let _g = crate::tests::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::init();
        crate::state::cache_buffers(r#"{"current":1,"buffers":[{"name":"A-RED.txt"}]}"#);

        // 同一线程换到另一个宿主(真实污染场景),让**它**写自己那份
        let mut other = JsHost::new();
        set_active(&mut other);
        crate::state::cache_buffers(r#"{"current":2,"buffers":[{"name":"B-BLUE.txt"}]}"#);
        let other_saw = crate::state::buffers_json();
        clear_active();
        std::mem::forget(other); // 引擎不可 drop

        // 回到宿主 A:必须仍看到**自己**那份
        let a_saw = crate::state::buffers_json();
        crate::state::cache_buffers(""); // 清理本宿主的桶,免得污染同进程其它测试

        assert!(
            other_saw.contains("B-BLUE.txt"),
            "另一宿主应看到自己写的快照;实得 {other_saw:?}"
        );
        assert!(
            a_saw.contains("A-RED.txt"),
            "宿主 A 的快照不该被别的宿主覆盖;实得 {a_saw:?}"
        );
    }

    /// **对照实验**:新造的 `Context` 丢弃是**安全**的。
    ///
    /// 为什么留着:`DEFAULT_HOST` 的注释断言"drop 已用过的共享引擎会破坏堆" ——
    /// 本测试是那个结论的**对照组**(新鲜引擎的 drop 正常)。没有对照组,"引擎必须不朽"
    /// 看起来像偷懒而不是结论;有人想删掉那句注释时,会先被这条测试问到"那为什么这个能 drop?"
    #[test]
    fn dropping_a_fresh_context_is_safe() {
        let mut e = Context::default();
        e.eval(Source::from_bytes(
            "globalThis.x = {a:1, b:'yy', c:[1,2,3]};",
        ))
        .unwrap();
        drop(e);
    }
}
