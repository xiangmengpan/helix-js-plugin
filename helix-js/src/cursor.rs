// cursor-move / selection-change 事件 emit(白名单已含两事件名,见 commands.rs)。
// 参数形状见 docs/superpowers/specs/2026-08-15-cursor-move-design.md:
//   cursor-move:      (docId, { row, col, mode })
//   selection-change: (docId, { count, primary: {row, col} })

use boa_engine::object::builtins::JsFunction;
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsString, JsValue};

use crate::state::{with_engine, with_event_handlers};

/// 构造 {row, col, mode} 对象调用 "cursor-move" handler(参数 [docId, ev])。
/// 返回 true=有 handler 被调用;false=无 handler/空(零开销,无副作用)。
pub fn emit_cursor_move(doc_id: u64, row: usize, col: usize, mode: &str) -> bool {
    crate::init();
    let handlers = with_event_handlers(|h| h.get("cursor-move").cloned());
    let Some(handlers) = handlers else {
        return false;
    };
    if handlers.is_empty() {
        return false;
    }
    with_engine(|engine| {
        let ev = ObjectInitializer::new(engine)
            .property(JsString::from("row"), JsValue::from(row), Attribute::all())
            .property(JsString::from("col"), JsValue::from(col), Attribute::all())
            .property(
                JsString::from("mode"),
                JsValue::from(JsString::from(mode)),
                Attribute::all(),
            )
            .build();
        call_handlers(
            &handlers,
            &[JsValue::from(doc_id), JsValue::from(ev)],
            engine,
        )
    })
}

/// 构造 {count, primary: {row, col}} 对象调用 "selection-change" handler。
/// 返回 true=有 handler 被调用;false=无 handler/空。
pub fn emit_selection_change(
    doc_id: u64,
    count: usize,
    primary_row: usize,
    primary_col: usize,
) -> bool {
    crate::init();
    let handlers = with_event_handlers(|h| h.get("selection-change").cloned());
    let Some(handlers) = handlers else {
        return false;
    };
    if handlers.is_empty() {
        return false;
    }
    with_engine(|engine| {
        let primary = ObjectInitializer::new(engine)
            .property(
                JsString::from("row"),
                JsValue::from(primary_row),
                Attribute::all(),
            )
            .property(
                JsString::from("col"),
                JsValue::from(primary_col),
                Attribute::all(),
            )
            .build();
        let ev = ObjectInitializer::new(engine)
            .property(
                JsString::from("count"),
                JsValue::from(count),
                Attribute::all(),
            )
            .property(JsString::from("primary"), primary, Attribute::all())
            .build();
        call_handlers(
            &handlers,
            &[JsValue::from(doc_id), JsValue::from(ev)],
            engine,
        )
    })
}

/// 按注册顺序调用 handler;任一调用成功即 true(与 emit_component_event 同构)。
fn call_handlers(handlers: &[JsValue], args: &[JsValue], engine: &mut Context) -> bool {
    let undefined = JsValue::undefined();
    for handler in handlers {
        let Some(func) = handler.as_callable().and_then(JsFunction::from_object) else {
            continue;
        };
        if func.call(&undefined, args, engine).is_ok() {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{commands::load_script, state::take_messages};
    use std::sync::Mutex;

    // 多个测试共享全局运行时(与 lib.rs tests 同模式),锁串行化
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn cursor_move_emits_row_col_mode() {
        let _guard = TEST_LOCK.lock().unwrap();
        crate::init();
        load_script(
            r#"
            helix.on("cursor-move", (docId, ev) => {
                helix.echo("m:" + docId + ":" + ev.row + ":" + ev.col + ":" + ev.mode);
            });
            "#,
        )
        .unwrap();
        assert!(emit_cursor_move(7, 3, 11, "insert"));
        assert_eq!(take_messages(), vec!["m:7:3:11:insert"]);
    }

    #[test]
    fn selection_change_emits_count_and_primary() {
        let _guard = TEST_LOCK.lock().unwrap();
        crate::init();
        load_script(
            r#"
            helix.on("selection-change", (docId, ev) => {
                helix.echo("s:" + docId + ":" + ev.count + ":" + ev.primary.row + ":" + ev.primary.col);
            });
            "#,
        )
        .unwrap();
        assert!(emit_selection_change(9, 2, 5, 8));
        assert_eq!(take_messages(), vec!["s:9:2:5:8"]);
    }

    #[test]
    fn no_handler_returns_false_without_side_effect() {
        let _guard = TEST_LOCK.lock().unwrap();
        crate::init();
        // 未注册任何 handler:直接返回 false,不 panic、无消息
        assert!(!emit_cursor_move(1, 0, 0, "normal"));
        assert!(!emit_selection_change(1, 1, 0, 0));
        assert!(take_messages().is_empty());
    }
}
