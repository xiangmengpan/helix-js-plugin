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
    /// 多行输入开关：true 时 Enter 换行、光标行感知移动（单行行为不变）
    pub multiline: bool,
}

thread_local! {
    // HashMap::new 非 const fn，不能用 const 块初始化（同 state.rs COMMAND_DOCS）
    static INPUT_STATES: RefCell<HashMap<(u64, String), InputState>> = RefCell::new(HashMap::new());
}

pub fn with_input_states<T>(f: impl FnOnce(&mut HashMap<(u64, String), InputState>) -> T) -> T {
    INPUT_STATES.with(|m| f(&mut m.borrow_mut()))
}

/// value 按 \n 分段的每行 char 范围：(start, end)（end 不含 \n；末行含尾部）。
/// 行尾即下一个 \n 的位置——光标落在 \n 上视为上一行行尾。
fn line_ranges(value: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in value.chars().enumerate() {
        if c == '\n' {
            out.push((start, i));
            start = i + 1;
        }
    }
    out.push((start, value.chars().count()));
    out
}

/// 光标所在行号（光标落在 \n 上属上一行，即该行行尾）
fn line_of(value: &str, cursor: usize) -> usize {
    line_ranges(value)
        .partition_point(|&(s, _)| s <= cursor)
        .saturating_sub(1)
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
        "Home" if state.multiline => {
            let ranges = line_ranges(&state.value);
            state.cursor = ranges[line_of(&state.value, state.cursor)].0;
            None
        }
        "End" if state.multiline => {
            let ranges = line_ranges(&state.value);
            state.cursor = ranges[line_of(&state.value, state.cursor)].1;
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
        // 多行：Enter 光标处插入 \n（单行 Enter 仍是提交语义，落 _ 分支）
        "Enter" if state.multiline => {
            let mut chars: Vec<char> = state.value.chars().collect();
            chars.insert(state.cursor, '\n');
            state.value = chars.into_iter().collect();
            state.cursor += 1;
            Some(state.value.clone())
        }
        // 多行：Up/Down 行间移动保持列，短行 clamp 到行尾
        "Up" | "Down" if state.multiline => {
            let ranges = line_ranges(&state.value);
            let line = line_of(&state.value, state.cursor);
            let col = state.cursor - ranges[line].0;
            let target = match key {
                "Up" if line > 0 => line - 1,
                "Down" if line + 1 < ranges.len() => line + 1,
                _ => line,
            };
            if target != line {
                let tcol = col.min(ranges[target].1 - ranges[target].0);
                state.cursor = ranges[target].0 + tcol;
            }
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
        let entry = m
            .entry((popup_id, node_id.to_string()))
            .or_insert_with(|| InputState {
                value: String::new(),
                cursor: 0,
                multiline: false,
            });
        input_edit(entry, key)
    });
    let Some(new_value) = changed else {
        return Ok(());
    };
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
            .call(
                &undefined,
                &[JsValue::from(JsString::from(new_value))],
                engine,
            )
            .map_err(|e| anyhow!("node {node_id} onChange failed: {e}"))?;
        Ok(())
    })
}

/// 光标处插入整段文本（粘贴用）：返回新值（单次状态变更，调用方触发一次 onChange）。
pub fn input_insert_batch(state: &mut InputState, text: &str) -> String {
    let mut chars: Vec<char> = state.value.chars().collect();
    let mut ins: Vec<char> = text.chars().collect();
    let rest: Vec<char> = chars.split_off(state.cursor);
    let ins_len = ins.len();
    state.value = chars.into_iter().chain(ins).chain(rest).collect();
    state.cursor += ins_len;
    state.value.clone()
}

/// 粘贴处理：整段插入 + 一次 onChange（与 dispatch_input_key 同 onChange 机制）。
pub fn dispatch_input_paste(popup_id: u64, node_id: &str, text: &str) -> anyhow::Result<()> {
    use anyhow::anyhow;
    use boa_engine::object::builtins::JsFunction;

    crate::init();
    let new_value = with_input_states(|m| {
        let entry = m
            .entry((popup_id, node_id.to_string()))
            .or_insert_with(|| InputState {
                value: String::new(),
                cursor: 0,
                multiline: false,
            });
        input_insert_batch(entry, text)
    });
    // 调 onChange(若有)：与 dispatch_input_key 同机制
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
            .call(
                &undefined,
                &[JsValue::from(JsString::from(new_value))],
                engine,
            )
            .map_err(|e| anyhow!("node {node_id} onChange failed: {e}"))?;
        Ok(())
    })
}

/// JS 强制改值（候选回填/清空）：`helix.set_input_value(popupId, nodeId, value)`。cursor 置末尾。
pub fn js_set_input_value(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let popup_id: u64 = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "set_input_value: popup id must be a number",
            )))
        })?;
    let node_id: String = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "set_input_value: node id must be a string",
            )))
        })?;
    let value: String = args
        .get(2)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "set_input_value: value must be a string",
            )))
        })?;
    with_input_states(|m| {
        // 保留既有 multiline 标志：set_input_value 只改值,不改节点的多行属性
        let multiline = m
            .get(&(popup_id, node_id.clone()))
            .map(|s| s.multiline)
            .unwrap_or(false);
        m.insert(
            (popup_id, node_id),
            InputState {
                cursor: value.chars().count(),
                value,
                multiline,
            },
        );
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

/// 该 input 是否多行（按键路由用）：multiline 时 Enter/Up/Down 是编辑键
/// （换行/行间移动,走 dispatch_input_key）；单行时 Enter/Up/Down 走 onKey（提交/候选导航）。
pub fn input_is_multiline(popup_id: u64, node_id: &str) -> bool {
    with_input_states(|m| {
        m.get(&(popup_id, node_id.to_string()))
            .map(|s| s.multiline)
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        close_popup, load_script, render_popup, take_messages, take_ui_requests, CompNode, Content,
        UiRequest,
    };
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
            multiline: false,
        };
        // 末尾追加
        assert_eq!(input_edit(&mut s, "d"), Some("abcd".to_string()));
        assert_eq!(s.cursor, 4);
        // 中间插入
        s = InputState {
            value: "abc".into(),
            cursor: 1,
            multiline: false,
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
            multiline: false,
        };
        assert_eq!(input_edit(&mut s, "Backspace"), None);
        assert_eq!(s.value, "abc");
        // Delete 删光标后
        s = InputState {
            value: "abc".into(),
            cursor: 0,
            multiline: false,
        };
        assert_eq!(input_edit(&mut s, "Delete"), Some("bc".to_string()));
        assert_eq!(s.cursor, 0);
        // Left/Right/Home/End 只动光标,不触发 onChange(返回 None)
        s = InputState {
            value: "abc".into(),
            cursor: 2,
            multiline: false,
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
            multiline: false,
        };
        assert_eq!(input_edit(&mut s, "啊"), Some("你啊好".to_string()));
        assert_eq!(s.cursor, 2);
        // Right 越界 clamp 到末尾
        s = InputState {
            value: "ab".into(),
            cursor: 2,
            multiline: false,
        };
        assert_eq!(input_edit(&mut s, "Right"), None);
        assert_eq!(s.cursor, 2);
        // 空值 Backspace/Delete 安全
        s = InputState {
            value: "".into(),
            cursor: 0,
            multiline: false,
        };
        assert_eq!(input_edit(&mut s, "Backspace"), None);
        assert_eq!(input_edit(&mut s, "Delete"), None);
    }

    /// 任务 1：multiline 多行语义（Enter 换行、Up/Down 列保持、Home/End 行级、
    /// 跨行移动与合并）。光标为全文本 char 索引；行尾（不含 \n）即 \n 位置。
    #[test]
    fn input_edit_multiline() {
        let _guard = TEST_LOCK.lock().unwrap();
        crate::init();
        // Enter：光标处插入 \n，cursor 推进
        let mut s = InputState {
            value: "ab\ncd".into(),
            cursor: 1,
            multiline: true,
        };
        assert_eq!(input_edit(&mut s, "Enter"), Some("a\nb\ncd".into()));
        assert_eq!(s.cursor, 2);
        // Home/End：行级（End 落在 \n 位置 = 该行行尾）
        s = InputState {
            value: "ab\ncd".into(),
            cursor: 4,
            multiline: true,
        };
        assert_eq!(input_edit(&mut s, "Home"), None);
        assert_eq!(s.cursor, 3);
        assert_eq!(input_edit(&mut s, "End"), None);
        assert_eq!(s.cursor, 5);
        // Up：列保持；短行 clamp 到行尾
        s = InputState {
            value: "ab\ncd".into(),
            cursor: 4,
            multiline: true,
        };
        assert_eq!(input_edit(&mut s, "Up"), None);
        assert_eq!(s.cursor, 1);
        s = InputState {
            value: "a\nlong".into(),
            cursor: 5,
            multiline: true,
        };
        assert_eq!(input_edit(&mut s, "Up"), None);
        assert_eq!(s.cursor, 1);
        // Down：同理下移
        s = InputState {
            value: "ab\ncd".into(),
            cursor: 1,
            multiline: true,
        };
        assert_eq!(input_edit(&mut s, "Down"), None);
        assert_eq!(s.cursor, 4);
        // 首行 Up / 末行 Down：不动
        assert_eq!(input_edit(&mut s, "Up"), None);
        assert_eq!(s.cursor, 1);
        s = InputState {
            value: "ab\ncd".into(),
            cursor: 4,
            multiline: true,
        };
        assert_eq!(input_edit(&mut s, "Down"), None);
        assert_eq!(s.cursor, 4);
        // Left：行首 → 上一行行尾（\n 位置）；Right：行尾 → 下一行行首
        s = InputState {
            value: "ab\ncd".into(),
            cursor: 3,
            multiline: true,
        };
        assert_eq!(input_edit(&mut s, "Left"), None);
        assert_eq!(s.cursor, 2);
        s = InputState {
            value: "ab\ncd".into(),
            cursor: 2,
            multiline: true,
        };
        assert_eq!(input_edit(&mut s, "Right"), None);
        assert_eq!(s.cursor, 3);
        // Backspace：行首删 \n 合并行；Delete：行尾删 \n 合并行
        s = InputState {
            value: "ab\ncd".into(),
            cursor: 3,
            multiline: true,
        };
        assert_eq!(input_edit(&mut s, "Backspace"), Some("abcd".into()));
        assert_eq!(s.cursor, 2);
        s = InputState {
            value: "ab\ncd".into(),
            cursor: 2,
            multiline: true,
        };
        assert_eq!(input_edit(&mut s, "Delete"), Some("abcd".into()));
        assert_eq!(s.cursor, 2);
    }

    #[test]
    fn input_insert_batch_basic() {
        let _guard = TEST_LOCK.lock().unwrap();
        crate::init();
        // 光标中间插入整段(含 \n)
        let mut s = InputState {
            value: "ab".into(),
            cursor: 1,
            multiline: true,
        };
        assert_eq!(input_insert_batch(&mut s, "x\ny"), "ax\nyb".to_string());
        assert_eq!(s.cursor, 4);
        // 光标末尾
        let mut s = InputState {
            value: "ab".into(),
            cursor: 2,
            multiline: true,
        };
        assert_eq!(input_insert_batch(&mut s, "cd"), "abcd".to_string());
        assert_eq!(s.cursor, 4);
        // 空值
        let mut s = InputState {
            value: String::new(),
            cursor: 0,
            multiline: false,
        };
        assert_eq!(
            input_insert_batch(&mut s, "line1\nline2"),
            "line1\nline2".to_string()
        );
        assert_eq!(s.cursor, 11);
    }
}
