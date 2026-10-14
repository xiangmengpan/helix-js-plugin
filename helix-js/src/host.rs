//! JS 引擎的**宿主**:把引擎的所有权从 thread_local 原语归到一个**显式对象**。
//!
//! 为什么值得:原来"引擎"只是一段 thread_local 状态,没有名字、无法指名、无法多份。
//! 归到 [`JsHost`] 之后,"一份 JS 运行时"成为一个可传递/可替换的对象 ——
//! `Application` 可以持有它(下一个任务)、测试可以构造多份。
//!
//! **当前仍有个默认宿主**(`DEFAULT_HOST`):`init()` 在没有外部宿主时自建,供测试与
//! 无 GUI 场景使用。队列/缓存仍是进程级(`OnceLock`),不在本步(Phase B)。
//!
//! ⚠️ **宿主永不 drop**(`ManuallyDrop`):引擎析构会释放 realm 堆,而 20 处**刻意不朽**的
//! JsValue 表(见 `state.rs`)经 Gc 句柄仍指向它 —— 实测(2026-10-11)drop 已用过的共享引擎
//! 会破坏堆(`tcache_thread_shutdown(): unaligned tcache chunk detected` /
//! `corrupted double-linked list`)。对照组:`dropping_a_fresh_context_is_safe` 证明
//! "新造 Context 的 drop"本身是安全的 ⇒ 问题在"引擎先走、引用者后走"。

use crate::types::UiRequest;
use boa_engine::{Context, JsValue};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::ptr::NonNull;
use std::sync::{Mutex, MutexGuard};

/// 一份 JS 运行时状态。当前只拥有引擎;队列/缓存留在进程级(Phase B)。
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

/// 把 `host` 注册为当前活动宿主(`Application` 用)。见 `ACTIVE` 的安全性约束。
pub fn set_active(host: &mut JsHost) {
    ACTIVE.with(|a| a.set(Some(NonNull::from(host))));
}

/// 注销活动宿主(必须在宿主析构**之前**调用)
pub fn clear_active() {
    ACTIVE.with(|a| a.set(None));
}

/// 安装默认宿主(由 `init()` 调用;幂等由调用方守 —— 已有宿主时不要覆盖)。
pub fn install_default(host: JsHost) {
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
    if let Some(ptr) = ACTIVE.with(|a| a.get()) {
        // SAFETY: 同 with_host_ref
        let host: &JsHost = unsafe { &*ptr.as_ptr() };
        return Some(f(host));
    }
    DEFAULT_HOST.with(|cell| cell.borrow().as_deref().map(f))
}

/// 同 [`with_messages`],但无宿主时给 `None`(取走/查询路径不得建宿主)。
pub(crate) fn try_with_messages<T>(f: impl FnOnce(&mut Vec<String>) -> T) -> Option<T> {
    try_with_host_ref(|h| f(&mut h.messages.borrow_mut()))
}

/// 按**只读**方式访问当前宿主(不改所有权)。
pub(crate) fn with_host_ref<T>(f: impl FnOnce(&JsHost) -> T) -> T {
    if let Some(ptr) = ACTIVE.with(|a| a.get()) {
        // SAFETY: 注册期覆盖宿主生存期(见 ACTIVE 的约束)
        let host: &JsHost = unsafe { &*ptr.as_ptr() };
        return f(host);
    }
    DEFAULT_HOST.with(|cell| match cell.borrow().as_deref() {
        Some(host) => f(host),
        None => panic!("helix-js: host not registered — call init() first"),
    })
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
    if let Some(ptr) = ACTIVE.with(|a| a.get()) {
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
    if let Some(ptr) = ACTIVE.with(|a| a.get()) {
        return ptr.as_ptr() as u64;
    }
    DEFAULT_HOST.with(|cell| match cell.borrow().as_deref() {
        Some(host) => host as *const JsHost as u64,
        None => panic!("helix-js: host not registered — call init() first"),
    })
}

/// 取当前宿主 key;没有宿主则**先 `init()`** —— 供 `set_*` 这类可能早于初始化的调用使用
/// (直接 panic 会把"设置目录"这种无害调用变成致命错误)。
pub(crate) fn current_host_key_or_init() -> u64 {
    if ACTIVE.with(|a| a.get()).is_none() && DEFAULT_HOST.with(|h| h.borrow().is_none()) {
        crate::init();
    }
    current_host_key()
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
    ACTIVE.with(|a| a.get()).is_some() || DEFAULT_HOST.with(|h| h.borrow().is_some())
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
        let _g = crate::tests::TEST_LOCK.lock().unwrap();
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
        let _g = crate::tests::TEST_LOCK.lock().unwrap();
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
        let _g = crate::tests::TEST_LOCK.lock().unwrap();
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
        let _g = crate::tests::TEST_LOCK.lock().unwrap();
        crate::init();
        let r = std::panic::catch_unwind(|| with_engine(|_| panic!("boom")));
        assert!(r.is_err(), "闭包应当 panic");
        with_engine(|e| assert!(e.eval(Source::from_bytes("1+1")).is_ok()));
    }

    /// **宿主未注册**时任何入口都要明确报错(不是 UB、不是静默 no-op)。
    /// 末尾必须恢复宿主 —— 同进程还有别的测试要用它。
    #[test]
    fn unregistered_host_reports_clearly() {
        let _g = crate::tests::TEST_LOCK.lock().unwrap();
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
        let _g = crate::tests::TEST_LOCK.lock().unwrap();
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
    /// **仍在进程级**、不随宿主走 ⇒ 今天能断言的隔离只是**引擎级**的;
    /// 注册表隔离属 Phase B(队列/缓存搬进宿主)。第一版测试写成断言「命令隔离」⇒ 当场红 ——
    /// 这个红正好把真实边界钉了下来。
    #[test]
    fn two_hosts_are_isolated() {
        let _g = crate::tests::TEST_LOCK.lock().unwrap();
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
        let _g = crate::tests::TEST_LOCK.lock().unwrap();
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
    /// 引擎 —— 而注册表/队列**仍是进程级**,多份引擎换不来任何隔离,只白花内存与注册开销。
    #[test]
    fn park_and_take_reuses_the_same_engine() {
        let _g = crate::tests::TEST_LOCK.lock().unwrap();
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

    /// **队列要按宿主隔离,而不是进程级共享**(T1 步骤 2 的目标行为)。
    ///
    /// 今天是 `OnceLock<Mutex<Vec<String>>>`(进程级) ⇒ 一个线程 `echo` 的消息,
    /// **另一个线程**能取到 ⇒ 本测试会红。搬进宿主后,另一线程自建的宿主看不到它 ⇒ 绿。
    #[test]
    fn messages_are_per_host_not_process_global() {
        let _g = crate::tests::TEST_LOCK.lock().unwrap();
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
        let _g = crate::tests::TEST_LOCK.lock().unwrap();
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
        let _g = crate::tests::TEST_LOCK.lock().unwrap();
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
        let _g = crate::tests::TEST_LOCK.lock().unwrap();
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
        let _g = crate::tests::TEST_LOCK.lock().unwrap();
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
        let _g = crate::tests::TEST_LOCK.lock().unwrap();
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
