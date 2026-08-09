//! JavaScript plugin runtime for the Helix editor (PoC).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use anyhow::{anyhow, Result};
use boa_engine::object::builtins::JsFunction;
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsError, JsString, JsValue, NativeFunction, Source};

/// 插件命令收到的只读上下文快照（由 helix-term 序列化编辑器状态得到）
pub struct CommandContext {
    pub path: Option<String>,
    pub text: String,
    pub cursor: (usize, usize),
}

// boa 的 Context/JsValue 是 !Send（Rc GC 堆），不能用 static 全局共享，
// 所以引擎按线程存放（编辑器主线程是唯一调用者）；MESSAGES 跨线程共享。
// drop 顺序：所有公共函数先调用 init()（先触达 CONTEXT），故线程销毁时
// REGISTRY 先于 CONTEXT drop，JsValue 的 GC 引用在 CONTEXT 销毁前释放。
// ponytail: 实现时观察到进程退出阶段偶发 tcache 崩溃（疑似 Context drop 的
// double-finalize，但独立复现未能确认），故 Box::leak 泄漏到 'static 规避——
// 进程退出时 OS 回收，对 PoC 无实际代价。升级 boa 后应改回正常持有。
thread_local! {
    static CONTEXT: RefCell<Option<&'static mut Context>> = const { RefCell::new(None) };
    // HashMap::new 非 const fn（1.90），REGISTRY 不能用 const 块初始化
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
                .function(
                    NativeFunction::from_fn_ptr(js_register_command),
                    JsString::from("register_command"),
                    2,
                )
                .build();
            engine
                .register_global_property(JsString::from("helix"), helix, Attribute::READONLY | Attribute::NON_ENUMERABLE)
                .expect("register helix object");
            *slot = Some(engine);
        }
    });
}

fn js_register_command(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let func = args.get(1).cloned().unwrap_or(JsValue::undefined());
    if name.is_empty() || name.chars().any(char::is_whitespace) {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(format!(
            "invalid command name: {name:?}"
        )))));
    }
    REGISTRY.with(|r| r.borrow_mut().insert(name, func));
    Ok(JsValue::undefined())
}

/// 把 CommandContext 转成 JS 对象 { doc: { path, text }, cursor: { row, col } }
fn ctx_to_js(ctx: &CommandContext, engine: &mut Context) -> boa_engine::JsResult<JsValue> {
    let doc = ObjectInitializer::new(engine)
        .property(
            JsString::from("path"),
            match &ctx.path {
                Some(p) => JsValue::from(JsString::from(p.clone())),
                None => JsValue::null(),
            },
            Attribute::all(),
        )
        .property(
            JsString::from("text"),
            JsValue::from(JsString::from(ctx.text.clone())),
            Attribute::all(),
        )
        .build();
    let cursor = ObjectInitializer::new(engine)
        .property(
            JsString::from("row"),
            JsValue::from(ctx.cursor.0 as f64),
            Attribute::all(),
        )
        .property(
            JsString::from("col"),
            JsValue::from(ctx.cursor.1 as f64),
            Attribute::all(),
        )
        .build();
    Ok(JsValue::from(
        ObjectInitializer::new(engine)
            .property(JsString::from("doc"), doc, Attribute::all())
            .property(JsString::from("cursor"), cursor, Attribute::all())
            .build(),
    ))
}

/// 运行插件命令。返回 Ok(true) 表示已运行，Ok(false) 表示未注册
pub fn run_command(name: &str, ctx: &CommandContext) -> Result<bool> {
    init();
    let func = REGISTRY.with(|r| r.borrow().get(name).cloned());
    let Some(func) = func else { return Ok(false) };

    let func = func
        .as_callable()
        .and_then(JsFunction::from_object)
        .ok_or_else(|| anyhow!("registered value for '{name}' is not a function"))?;

    CONTEXT.with(|cell| {
        let mut binding = cell.borrow_mut();
        let engine = binding.as_mut().expect("CONTEXT initialized by init()");
        let arg = ctx_to_js(ctx, engine)
            .map_err(|e| anyhow!("failed to build command context: {e}"))?;
        let undefined = JsValue::undefined();
        func.call(&undefined, &[arg], engine)
            .map(|_| true)
            .map_err(|e| anyhow!("plugin command '{name}' failed: {e}"))
    })
}

/// 已注册的插件命令名（供命令行补全）
pub fn command_names() -> Vec<String> {
    init();
    REGISTRY.with(|r| r.borrow().keys().cloned().collect())
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
    std::mem::take(&mut *MESSAGES.get().expect("MESSAGES not initialized").lock().expect("messages lock poisoned"))
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

    #[test]
    fn register_and_run_command() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("where", (ctx) => {
            helix.echo("cursor: " + ctx.cursor.row + "," + ctx.cursor.col);
        });
        "#,
        )
        .unwrap();

        let ctx = CommandContext {
            path: Some("/tmp/demo.rs".to_string()),
            text: "hello\nworld".to_string(),
            cursor: (1, 2),
        };
        assert!(run_command("where", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["cursor: 1,2"]);

        // 未注册的命令返回 false
        assert!(!run_command("nope", &ctx).unwrap());
        // 非法命令名（含空白）注册时报错
        assert!(load_script(r#"helix.register_command("bad name", () => {});"#).is_err());
    }
}
