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
use std::cell::RefCell;
use std::mem::ManuallyDrop;

/// 一份 JS 运行时状态。当前只拥有引擎;队列/缓存留在进程级(Phase B)。
pub struct JsHost {
    engine: RefCell<Option<Box<Context>>>,
}

impl JsHost {
    /// 建宿主,并在其中注册 `helix` 全局(建引擎的代码在 [`crate::build_engine`])。
    pub fn new() -> Self {
        let mut ctx = Context::default();
        crate::build_engine(&mut ctx);
        Self {
            engine: RefCell::new(Some(Box::new(ctx))),
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

/// 安装默认宿主(由 `init()` 调用;幂等由调用方守 —— 已有宿主时不要覆盖)。
pub fn install_default(host: JsHost) {
    DEFAULT_HOST.with(|h| *h.borrow_mut() = Some(Box::new(host)));
}

/// 是否已有宿主(`init()` 的幂等判断用它,而**不是**"槽是否为空"—— 引擎借出期间槽也是空的)
pub fn engine_exists() -> bool {
    DEFAULT_HOST.with(|h| h.borrow().is_some())
}

/// 借出的引擎:Drop 时自动**归还**(panic 安全 —— 闭包 panic 也不会把引擎弄丢)。
struct EngineBorrow(Option<Box<Context>>);

impl std::ops::Deref for EngineBorrow {
    type Target = Context;
    fn deref(&self) -> &Context {
        self.0.as_deref().expect("EngineBorrow 非空")
    }
}
impl std::ops::DerefMut for EngineBorrow {
    fn deref_mut(&mut self) -> &mut Context {
        self.0.as_deref_mut().expect("EngineBorrow 非空")
    }
}
impl Drop for EngineBorrow {
    fn drop(&mut self) {
        if let Some(engine) = self.0.take() {
            DEFAULT_HOST.with(|cell| {
                if let Some(host) = cell.borrow_mut().as_mut() {
                    *host.engine.borrow_mut() = Some(engine);
                }
            });
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
    DEFAULT_HOST.with(|cell| {
        let taken = cell
            .borrow_mut()
            .as_mut()
            .and_then(|host| host.engine.borrow_mut().take());
        // 区分两种"取不到":宿主不存在(未 init) vs 引擎已被借出(重入)
        let exists = engine_exists();
        let Some(engine) = taken else {
            if exists {
                panic!(
                    "helix-js: engine used again while already borrowed \
                     (同一线程在 JS 调用树内重入;见 commands.rs 里 helix.load 的注释)"
                );
            }
            panic!("helix-js: host not registered — call init() first");
        };
        let mut guard = EngineBorrow(Some(engine));
        f(&mut guard)
    })
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
