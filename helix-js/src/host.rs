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

use boa_engine::Context;
use std::cell::{Cell, RefCell};
use std::mem::ManuallyDrop;
use std::ptr::NonNull;

/// 一份 JS 运行时状态。当前只拥有引擎;队列/缓存留在进程级(Phase B)。
pub struct JsHost {
    engine: RefCell<Option<Box<Context>>>,
    /// echo 消息队列(**按宿主**隔离 —— 原来是进程级 `OnceLock<Mutex<..>>`)。
    /// 访问点实测只在主线程(见 docs/superpowers/plans/2026-10-11-js-host-phaseB.md 的闸门结论),
    /// 所以用 `RefCell` 而不是 `Mutex`;若将来真有非主线程访问,它会先撞上"host not registered"。
    messages: RefCell<Vec<String>>,
}

impl JsHost {
    /// 建宿主,并在其中注册 `helix` 全局(建引擎的代码在 [`crate::build_engine`])。
    pub fn new() -> Self {
        let mut ctx = Context::default();
        crate::build_engine(&mut ctx);
        Self {
            engine: RefCell::new(Some(Box::new(ctx))),
            messages: RefCell::new(Vec::new()),
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

/// 引擎从哪个宿主借出 —— 归还时必须回到**同一个**,否则等于把引擎搬到别的宿主里
#[derive(Clone, Copy)]
enum Owner {
    Active,
    Default,
}

fn take_engine() -> Option<(Box<Context>, Owner)> {
    if let Some(ptr) = ACTIVE.with(|a| a.get()) {
        // SAFETY: 注册期覆盖宿主生存期(见 ACTIVE 的约束)
        let host: &mut JsHost = unsafe { &mut *ptr.as_ptr() };
        return host.engine.borrow_mut().take().map(|e| (e, Owner::Active));
    }
    DEFAULT_HOST
        .with(|cell| {
            cell.borrow_mut()
                .as_mut()
                .and_then(|h| h.engine.borrow_mut().take())
        })
        .map(|e| (e, Owner::Default))
}

/// 把引擎归还给它来的那个宿主。
///
/// 两个宿主都已不在时**遗忘而不 drop** —— 与本模块的根本不变量一致:
/// drop 一个用过的引擎会破坏堆,宁可泄漏。
fn put_engine(engine: Box<Context>, owner: Owner) {
    match owner {
        Owner::Active => {
            if let Some(ptr) = ACTIVE.with(|a| a.get()) {
                // SAFETY: 同 take_engine
                let host: &mut JsHost = unsafe { &mut *ptr.as_ptr() };
                *host.engine.borrow_mut() = Some(engine);
                return;
            }
            std::mem::forget(engine);
        }
        Owner::Default => {
            let mut slot = DEFAULT_HOST.with(|cell| cell.borrow_mut().take());
            match slot.as_mut() {
                Some(host) => {
                    *host.engine.borrow_mut() = Some(engine);
                    DEFAULT_HOST.with(|cell| *cell.borrow_mut() = slot);
                }
                None => std::mem::forget(engine),
            }
        }
    }
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
    let mut slot = DEFAULT_HOST.with(|cell| cell.borrow_mut().take());
    match slot {
        None => DEFAULT_HOST.with(|cell| *cell.borrow_mut() = Some(host)),
        Some(existing) => {
            DEFAULT_HOST.with(|cell| *cell.borrow_mut() = Some(existing));
            std::mem::forget(host);
        }
    }
}

/// 按**只读**方式访问当前宿主(不改所有权)。
fn with_host_ref<T>(f: impl FnOnce(&JsHost) -> T) -> T {
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

/// 是否已有可用宿主(`init()` 的幂等判断用它,而**不是**"槽是否为空"—— 引擎借出期间槽也是空的)
pub fn engine_exists() -> bool {
    ACTIVE.with(|a| a.get()).is_some() || DEFAULT_HOST.with(|h| h.borrow().is_some())
}

/// 借出的引擎:Drop 时自动**归还**(panic 安全 —— 闭包 panic 也不会把引擎弄丢)。
struct EngineBorrow {
    engine: Option<Box<Context>>,
    owner: Owner,
}

impl std::ops::Deref for EngineBorrow {
    type Target = Context;
    fn deref(&self) -> &Context {
        self.engine.as_deref().expect("EngineBorrow 非空")
    }
}
impl std::ops::DerefMut for EngineBorrow {
    fn deref_mut(&mut self) -> &mut Context {
        self.engine.as_deref_mut().expect("EngineBorrow 非空")
    }
}
impl Drop for EngineBorrow {
    fn drop(&mut self) {
        if let Some(engine) = self.engine.take() {
            put_engine(engine, self.owner);
        }
    }
}

/// 借出宿主里的引擎执行 `f`;**同一线程重入会得到指名道姓的报错**。
///
/// 为什么用"取出→归还"而不是"一直持着借用":借用会跨越整棵 JS 调用树,重入时的 panic 落在
/// 本函数内部、看不出是谁在占用;取出后宿主槽为空,重入能被**明确识别**,配合
/// `#[track_caller]` 直指调用点。
#[track_caller]
pub(crate) fn with_engine<T>(f: impl FnOnce(&mut Context) -> T) -> T {
    // 区分两种"取不到":宿主不存在(未 init) vs 引擎已被借出(重入)
    let exists = engine_exists();
    let Some((engine, owner)) = take_engine() else {
        if exists {
            panic!(
                "helix-js: engine used again while already borrowed \
                 (同一线程在 JS 调用树内重入;见 commands.rs 里 helix.load 的注释)"
            );
        }
        panic!("helix-js: host not registered — call init() first");
    };
    let mut guard = EngineBorrow {
        engine: Some(engine),
        owner,
    };
    f(&mut guard)
}

// ── 测试专用访问器 ────────────────────────────────────────────────
/// 测试专用:把引擎**整体取出**(宿主槽置 None)。用于验证"所有权真的在宿主里"。
///
/// ⚠️ **取出后必须原物放回,不得 drop**:drop 已用过的引擎会破坏堆(理由见模块头)。
#[cfg(test)]
pub(crate) fn take_engine_for_test() -> Option<Box<Context>> {
    DEFAULT_HOST.with(|cell| {
        cell.borrow_mut()
            .as_mut()
            .and_then(|host| host.engine.borrow_mut().take())
    })
}

/// 测试专用:把引擎放回宿主槽里(应当与 [`take_engine_for_test`] 取出的是同一个)。
#[cfg(test)]
pub(crate) fn set_engine_for_test(engine: Box<Context>) {
    DEFAULT_HOST.with(|cell| {
        if let Some(host) = cell.borrow_mut().as_mut() {
            *host.engine.borrow_mut() = Some(engine);
        }
    });
}

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

    /// 宿主里是**有主的** `Box<Context>`:能整体取出、**原物放回**,且期间状态不丢。
    ///
    /// 取出→放回而不是取出→丢弃:同进程内全部单测共用一个默认宿主,
    /// 丢掉它会让后面的测试找不到 `helix` 全局。
    #[test]
    fn engine_slot_is_owned_and_reusable() {
        let _g = crate::tests::TEST_LOCK.lock().unwrap();
        crate::init();
        crate::load_script(r#"helix.register_command("survives-take", () => {});"#)
            .expect("注册一个命令用做“引擎身份”的标记");

        let taken = take_engine_for_test().expect("init() 后引擎应在宿主里");
        assert!(
            std::panic::catch_unwind(|| with_engine(|_| {})).is_err(),
            "引擎已取出 ⇒ 再借必须报错"
        );

        set_engine_for_test(taken); // 原物放回
        assert!(
            crate::command_names().iter().any(|n| n == "survives-take"),
            "放回的必须是同一个引擎(否则注册表随旧引擎一起没了)"
        );
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
        with_engine(|_| {
            crate::init(); // 借出期间再调
            assert!(
                take_engine_for_test().is_none(),
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
