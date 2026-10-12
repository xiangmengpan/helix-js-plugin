//! The event system makes use of global to decouple different systems.
//! However, this can cause problems for the integration test system because
//! it runs multiple helix applications in parallel. Making the globals
//! thread-local does not work because a applications can/does have multiple
//! runtime threads. Instead this crate implements a similar notion to a thread
//! local but instead of being local to a single thread, the statics are local to
//! a single tokio-runtime. The implementation requires locking so it's not exactly efficient.
//!
//! Therefore this function is only enabled during integration tests and behaves like
//! a normal static otherwise. I would prefer this module to be fully private and to only
//! export the macro but the macro still need to construct these internals so it's marked
//! `doc(hidden)` instead

use std::ops::Deref;

#[cfg(not(feature = "integration_test"))]
pub struct RuntimeLocal<T: 'static> {
    /// inner API used in the macro, not part of public API
    #[doc(hidden)]
    pub __data: T,
}

#[cfg(not(feature = "integration_test"))]
impl<T> Deref for RuntimeLocal<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.__data
    }
}

#[cfg(not(feature = "integration_test"))]
#[macro_export]
macro_rules! runtime_local {
    ($($(#[$attr:meta])* $vis: vis static $name:ident: $ty: ty = $init: expr;)*) => {
        $($(#[$attr])* $vis static $name: $crate::runtime::RuntimeLocal<$ty> = $crate::runtime::RuntimeLocal {
            __data: $init
        };)*
    };
}

#[cfg(feature = "integration_test")]
pub struct RuntimeLocal<T: 'static> {
    /// 键是 `Option<Id>`:`None` = **不在任何 tokio runtime 上下文的线程**(rayon worker)。
    /// 若这里用裸 `Id` 并 `Handle::current()`,rayon worker 上会 panic ⇒ rayon 的
    /// `AbortIfPanic` 直接 abort 整个进程(见下方 `deref_works_without_tokio_runtime`)。
    data: parking_lot::RwLock<
        hashbrown::HashMap<Option<tokio::runtime::Id>, &'static T, foldhash::fast::FixedState>,
    >,
    init: fn() -> T,
}

#[cfg(feature = "integration_test")]
impl<T> RuntimeLocal<T> {
    /// inner API used in the macro, not part of public API
    #[doc(hidden)]
    pub const fn __new(init: fn() -> T) -> Self {
        Self {
            data: parking_lot::RwLock::new(hashbrown::HashMap::with_hasher(
                foldhash::fast::FixedState::with_seed(12345678910),
            )),
            init,
        }
    }
}

#[cfg(feature = "integration_test")]
impl<T> Deref for RuntimeLocal<T> {
    type Target = T;
    fn deref(&self) -> &T {
        // `try_current()`:非 tokio 线程(rayon worker)回落到 `None` 桶,而不是 panic。
        let id = tokio::runtime::Handle::try_current().ok().map(|h| h.id());
        let guard = self.data.read();
        match guard.get(&id) {
            Some(res) => res,
            None => {
                drop(guard);
                let data = Box::leak(Box::new((self.init)()));
                let mut guard = self.data.write();
                guard.insert(id, data);
                data
            }
        }
    }
}

#[cfg(feature = "integration_test")]
#[macro_export]
macro_rules! runtime_local {
    ($($(#[$attr:meta])* $vis: vis static $name:ident: $ty: ty = $init: expr;)*) => {
         $($(#[$attr])* $vis static $name: $crate::runtime::RuntimeLocal<$ty> = $crate::runtime::RuntimeLocal::__new(|| $init);)*
    };
}

#[cfg(all(test, feature = "integration_test"))]
mod tests {
    use super::*;

    /// **没有 tokio runtime 的线程也必须能用** —— rayon worker 就是这种线程。
    ///
    /// 由来(2026-10-11 实测):集成测试并发跑时,`nucleo`(picker 的模糊匹配)在 **rayon worker**
    /// 上 tick;从那里访问任何被本夹具包住的全局 ⇒ `Handle::current()` panic
    /// (*"there is no reactor running…"*)⇒ rayon `AbortIfPanic` ⇒ **abort 整个测试进程**
    /// (SIGABRT,且 panic 消息随 libtest 捕获缓冲一起丢,所以一直"无诊断")。
    /// 本测试把这条路径钉住:普通线程里 deref 不得 panic。
    #[test]
    fn deref_works_without_tokio_runtime() {
        static V: RuntimeLocal<Vec<u32>> = RuntimeLocal::__new(Vec::new);
        let h = std::thread::spawn(|| V.len());
        assert_eq!(h.join().expect("普通线程里 deref 不应 panic"), 0);
    }
}
