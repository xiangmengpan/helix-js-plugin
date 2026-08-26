//! 输入组件编辑状态：InputState + input_edit 纯函数 + InputStates 存储。
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

#[cfg(test)]
mod tests {
    use super::*;
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
