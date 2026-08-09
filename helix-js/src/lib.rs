//! JavaScript plugin runtime for the Helix editor (PoC).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use anyhow::{anyhow, Result};
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsString, JsValue, NativeFunction, Source};

/// 插件命令收到的只读上下文快照（由 helix-term 序列化编辑器状态得到）
pub struct CommandContext {
    pub path: Option<String>,
    pub text: String,
    pub cursor: (usize, usize),
}

// boa 的 Context/JsValue 是 !Send（Rc GC 堆），不能用 static 全局共享，
// 所以引擎按线程存放（编辑器主线程是唯一调用者）；MESSAGES 跨线程共享。
// ponytail: boa 0.21.1 的 Context drop 有 double-finalize UAF（valgrind 证实，
// debug 下 glibc tcache 偶发 abort），因此 Box::leak 泄漏到 'static 规避——
// 进程退出时 OS 回收。升级 boa 修复后应改回正常持有。
thread_local! {
    static CONTEXT: RefCell<Option<&'static mut Context>> = RefCell::new(None);
    static REGISTRY: RefCell<HashMap<String, JsValue>> = RefCell::new(HashMap::new());
}
static MESSAGES: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

/// 创建 boa 上下文并注册全局 `helix` 对象（幂等）
pub fn init() {
    MESSAGES.get_or_init(Default::default);
    CONTEXT.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            let engine = Box::leak(Box::new(Context::default()));
            let helix = ObjectInitializer::new(engine)
                .function(NativeFunction::from_fn_ptr(js_echo), JsString::from("echo"), 1)
                .build();
            engine
                .register_global_property(JsString::from("helix"), helix, Attribute::READONLY | Attribute::NON_ENUMERABLE)
                .expect("register helix object");
            *slot = Some(engine);
        }
    });
}

fn js_echo(_this: &JsValue, args: &[JsValue], context: &mut Context) -> boa_engine::JsResult<JsValue> {
    let text: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    MESSAGES
        .get()
        .expect("MESSAGES initialized")
        .lock()
        .expect("messages lock")
        .push(text);
    Ok(JsValue::undefined())
}

/// 求值一段插件脚本；脚本里可调用 `helix.register_command` / `helix.echo`
pub fn load_script(src: &str) -> Result<()> {
    init();
    CONTEXT.with(|cell| {
        let mut binding = cell.borrow_mut();
        let engine = binding.as_mut().expect("CONTEXT initialized by init()");
        engine
            .eval(Source::from_bytes(src))
            .map(|_| ())
            .map_err(|e| anyhow!("plugin script error: {e}"))
    })
}

/// 取走并清空 echo 消息队列
pub fn take_messages() -> Vec<String> {
    init();
    std::mem::take(&mut *MESSAGES.get().unwrap().lock().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // 多个测试共享全局运行时，用锁串行化避免消息队列竞争
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn echo_captures_message() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(r#"helix.echo("hello from js");"#).unwrap();
        assert_eq!(take_messages(), vec!["hello from js"]);
    }
}
