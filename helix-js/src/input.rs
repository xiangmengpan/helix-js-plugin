//! 输入组件编辑状态：InputState + input_edit 纯函数 + InputStates 存储。
use boa_engine::{Context, JsError, JsString, JsValue};
use std::cell::RefCell;
use std::collections::HashMap;

/// 单个 input 节点的编辑状态（引擎权威）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputState {
    pub value: String,
    /// 光标位置（char 索引，非字节）
    pub cursor: usize,
}

thread_local! {
    // HashMap::new 非 const fn，不能用 const 块初始化（同 state.rs COMMAND_DOCS）
    static INPUT_STATES: RefCell<HashMap<(u64, String), InputState>> = RefCell::new(HashMap::new());
}

pub fn with_input_states<T>(f: impl FnOnce(&mut HashMap<(u64, String), InputState>) -> T) -> T {
    INPUT_STATES.with(|m| f(&mut m.borrow_mut()))
}

/// 按键编辑转换（纯函数）：返回新值表示值变化（应触发 onChange）；
/// 返回 None 表示值未变（光标移动/非编辑键）。光标以 char 索引计。
// ponytail: chars()/collect 是 O(n)——补全场景输入值短（<100 字符）可接受；
// 若有大文本输入再优化为 char_indices 增量编辑。
pub fn input_edit(state: &mut InputState, key: &str) -> Option<String> {
    let len = state.value.chars().count();
    match key {
        "Backspace" if state.cursor > 0 => {
            let target = state.cursor - 1;
            let mut rest = state.value.chars();
            let before: String = rest.by_ref().take(target).collect();
            let after: String = rest.skip(1).collect();
            state.value = before + &after;
            state.cursor = target;
            Some(state.value.clone())
        }
        "Delete" if state.cursor < len => {
            let mut rest = state.value.chars();
            let before: String = rest.by_ref().take(state.cursor).collect();
            let after: String = rest.skip(1).collect();
            state.value = before + &after;
            Some(state.value.clone())
        }
        "Left" => {
            state.cursor = state.cursor.saturating_sub(1);
            None
        }
        "Right" => {
            state.cursor = (state.cursor + 1).min(len);
            None
        }
        "Home" => {
            state.cursor = 0;
            None
        }
        "End" => {
            state.cursor = len;
            None
        }
        k if k.chars().count() == 1 => {
            // 单字符插入到光标处
            let mut chars: Vec<char> = state.value.chars().collect();
            let ch = k.chars().next().unwrap();
            chars.insert(state.cursor, ch);
            state.value = chars.into_iter().collect();
            state.cursor += 1;
            Some(state.value.clone())
        }
        _ => None,
    }
}

/// 编辑键处理：查 InputStates → input_edit 转换 → 有变化则调 onChange(新值)。
/// 供 helix-term 按键路由调用（任务 3）；返回 Err 仅当 onChange 回调抛错。
pub fn dispatch_input_key(popup_id: u64, node_id: &str, key: &str) -> anyhow::Result<()> {
    use anyhow::anyhow;
    use boa_engine::object::builtins::JsFunction;

    crate::init();
    let changed = with_input_states(|m| {
        let entry = m.entry((popup_id, node_id.to_string())).or_insert_with(|| {
            InputState { value: String::new(), cursor: 0 }
        });
        input_edit(entry, key)
    });
    let Some(new_value) = changed else { return Ok(()) };
    // 调 onChange(若有)：注册在 NODE_HANDLERS，与 onKey 同机制
    crate::state::with_engine(|engine| {
        let on_change = crate::state::with_node_handlers(|h| {
            h.get(&(popup_id, node_id.to_string()))
                .and_then(|hd| hd.on_change.clone())
        });
        let Some(f) = on_change else { return Ok(()) };
        let func = f
            .as_callable()
            .and_then(JsFunction::from_object)
            .ok_or_else(|| anyhow!("node {node_id} onChange not callable"))?;
        let undefined = JsValue::undefined();
        let _: JsValue = func
            .call(&undefined, &[JsValue::from(JsString::from(new_value))], engine)
            .map_err(|e| anyhow!("node {node_id} onChange failed: {e}"))?;
        Ok(())
    })
}

/// JS 强制改值（候选回填/清空）：`helix.set_input_value(popupId, nodeId, value)`。cursor 置末尾。
pub fn js_set_input_value(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let popup_id: u64 = args.first().unwrap_or(&JsValue::undefined()).try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("set_input_value: popup id must be a number")))
    })?;
    let node_id: String = args.get(1).unwrap_or(&JsValue::undefined()).try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("set_input_value: node id must be a string")))
    })?;
    let value: String = args.get(2).unwrap_or(&JsValue::undefined()).try_js_into(ctx).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from("set_input_value: value must be a string")))
    })?;
    with_input_states(|m| {
        m.insert((popup_id, node_id), InputState { cursor: value.chars().count(), value });
    });
    Ok(JsValue::undefined())
}

/// 弹窗关闭清理：删该 popup 的全部 InputStates（生命周期）。
pub fn clear_popup_inputs(popup_id: u64) {
    with_input_states(|m| m.retain(|(pid, _), _| *pid != popup_id));
}

/// 该 input 节点是否已渲染（有引擎状态）。未渲染的节点返回 false。
/// 供 helix-term 按键路由判断：有状态 → 编辑键走 dispatch_input_key，否则按 button 处理。
pub fn input_has_state(popup_id: u64, node_id: &str) -> bool {
    with_input_states(|m| m.contains_key(&(popup_id, node_id.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{close_popup, load_script, render_popup, take_messages, take_ui_requests, CompNode, Content, UiRequest};
    use std::sync::Mutex;

    // 与 lib.rs 测试同模式：共享全局运行时用锁串行化
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn input_edit_basic() {
        let _guard = TEST_LOCK.lock().unwrap();
        crate::init();
        let mut s = InputState {
            value: "abc".into(),
            cursor: 3,
        };
        // 末尾追加
        assert_eq!(input_edit(&mut s, "d"), Some("abcd".to_string()));
        assert_eq!(s.cursor, 4);
        // 中间插入
        s = InputState {
            value: "abc".into(),
            cursor: 1,
        };
        assert_eq!(input_edit(&mut s, "X"), Some("aXbc".to_string()));
        assert_eq!(s.cursor, 2);
        // Backspace 中间删
        assert_eq!(input_edit(&mut s, "Backspace"), Some("abc".to_string()));
        assert_eq!(s.cursor, 1);
        // Backspace 光标在首 → 无变化
        s = InputState {
            value: "abc".into(),
            cursor: 0,
        };
        assert_eq!(input_edit(&mut s, "Backspace"), None);
        assert_eq!(s.value, "abc");
        // Delete 删光标后
        s = InputState {
            value: "abc".into(),
            cursor: 0,
        };
        assert_eq!(input_edit(&mut s, "Delete"), Some("bc".to_string()));
        assert_eq!(s.cursor, 0);
        // Left/Right/Home/End 只动光标,不触发 onChange(返回 None)
        s = InputState {
            value: "abc".into(),
            cursor: 2,
        };
        assert_eq!(input_edit(&mut s, "Left"), None);
        assert_eq!(s.cursor, 1);
        assert_eq!(input_edit(&mut s, "Right"), None);
        assert_eq!(s.cursor, 2);
        assert_eq!(input_edit(&mut s, "Home"), None);
        assert_eq!(s.cursor, 0);
        assert_eq!(input_edit(&mut s, "End"), None);
        assert_eq!(s.cursor, 3);
        // 非编辑键 → None
        assert_eq!(input_edit(&mut s, "Up"), None);
        assert_eq!(input_edit(&mut s, "Enter"), None);
        assert_eq!(input_edit(&mut s, "Tab"), None);
    }

    /// 任务 2 集成验证：渲染初始化 InputStates → dispatch_input_key 改状态 + 触发 onChange
    /// → set_input_value 强制改值 → close_popup 清理生命周期。
    #[test]
    fn input_dispatch_and_set() {
        let _guard = TEST_LOCK.lock().unwrap();
        crate::init();
        load_script(
            r#"
        helix.open_popup({
            render: () => helix.el("col", [
                { type: "input", id: "q", value: "a", onChange: (v) => { helix.echo("chg:" + v); } },
            ]),
        });
        "#,
        )
        .unwrap();
        let reqs = take_ui_requests();
        let id = match &reqs[0] {
            UiRequest::OpenPopup { id, .. } => *id,
            _ => unreachable!("expected OpenPopup"),
        };
        // 未渲染的 input：无状态（任务 3 按键路由据此判断）
        assert!(!input_has_state(id, "q"));
        // 首次渲染：JS 传 value 初始化 InputStates
        match render_popup(id, 40, 10, None).unwrap() {
            Content::Tree(CompNode::Col { children, .. }) => {
                assert!(matches!(&children[0], CompNode::Input { value, cursor, .. }
                    if value == "a" && *cursor == 1));
            }
            _ => panic!("expected tree"),
        }
        assert!(input_has_state(id, "q"));
        // 编辑键：改状态 + 触发 onChange（onChange 里 echo）
        dispatch_input_key(id, "q", "b").unwrap();
        assert_eq!(take_messages(), vec!["chg:ab"]);
        // 引擎状态覆盖 JS 传值：再渲染仍是引擎里的 "ab"
        match render_popup(id, 40, 10, None).unwrap() {
            Content::Tree(CompNode::Col { children, .. }) => {
                assert!(matches!(&children[0], CompNode::Input { value, cursor, .. }
                    if value == "ab" && *cursor == 2));
            }
            _ => panic!("expected tree"),
        }
        // 非编辑键：不动状态、不触发 onChange（Left 只动光标）
        dispatch_input_key(id, "q", "Left").unwrap();
        assert!(take_messages().is_empty());
        with_input_states(|m| assert_eq!(m.get(&(id, "q".to_string())).unwrap().cursor, 1));
        // set_input_value：JS 强制改值，cursor 置末尾
        load_script(&format!("helix.set_input_value({id}, \"q\", \"setval\");")).unwrap();
        match render_popup(id, 40, 10, None).unwrap() {
            Content::Tree(CompNode::Col { children, .. }) => {
                assert!(matches!(&children[0], CompNode::Input { value, cursor, .. }
                    if value == "setval" && *cursor == 6));
            }
            _ => panic!("expected tree"),
        }
        // 生命周期：close_popup 清空该 popup 的 InputStates
        close_popup(id).unwrap();
        with_input_states(|m| assert!(!m.contains_key(&(id, "q".to_string()))));
    }

    #[test]
    fn input_edit_utf8_and_clamp() {
        let _guard = TEST_LOCK.lock().unwrap();
        crate::init();
        // 多字节字符:光标按 char 计
        let mut s = InputState {
            value: "你好".into(),
            cursor: 1,
        };
        assert_eq!(input_edit(&mut s, "啊"), Some("你啊好".to_string()));
        assert_eq!(s.cursor, 2);
        // Right 越界 clamp 到末尾
        s = InputState {
            value: "ab".into(),
            cursor: 2,
        };
        assert_eq!(input_edit(&mut s, "Right"), None);
        assert_eq!(s.cursor, 2);
        // 空值 Backspace/Delete 安全
        s = InputState {
            value: "".into(),
            cursor: 0,
        };
        assert_eq!(input_edit(&mut s, "Backspace"), None);
        assert_eq!(input_edit(&mut s, "Delete"), None);
    }
}
