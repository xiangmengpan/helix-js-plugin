use std::collections::HashMap;

use super::macros::keymap;
use super::{KeyTrie, Mode};
use helix_core::hashmap;

pub fn default() -> HashMap<Mode, KeyTrie> {
    let normal = keymap!({ "Normal mode"
        "h" | "left" => move_char_left,
        "j" | "down" => move_visual_line_down,
        "k" | "up" => move_visual_line_up,
        "l" | "right" => move_char_right,

        "t" => find_till_char,
        "f" => find_next_char,
        "T" => till_prev_char,
        "F" => find_prev_char,
        "r" => replace,
        "R" => replace_with_yanked,
        "A-." =>  repeat_last_motion,

        "~" => switch_case,
        "`" => switch_to_lowercase,
        "A-`" => switch_to_uppercase,

        "home" => goto_line_start,
        "end" => goto_line_end,

        "w" => move_next_word_start,
        "b" => move_prev_word_start,
        // 调试：F9 = 在当前行切换断点（比 `<space>G b` 顺手；调试键在暂停时也可直接按 `b`）
        "F9" => dap_toggle_breakpoint,
        "e" => move_next_word_end,

        "W" => move_next_long_word_start,
        "B" => move_prev_long_word_start,
        "E" => move_next_long_word_end,

        "v" => select_mode,
        "G" => goto_line,
        "g" => { "Goto"
            "g" => goto_file_start,
            "|" => goto_column,
            "e" => goto_last_line,
            "f" => goto_file,
            "h" => goto_line_start,
            "l" => goto_line_end,
            "s" => goto_first_nonwhitespace,
            "d" => goto_definition,
            "D" => goto_declaration,
            "y" => goto_type_definition,
            "r" => goto_reference,
            "i" => goto_implementation,
            "t" => goto_window_top,
            "c" => goto_window_center,
            "b" => goto_window_bottom,
            "a" => goto_last_accessed_file,
            "m" => goto_last_modified_file,
            "n" => goto_next_buffer,
            "p" => goto_previous_buffer,
            "k" => move_line_up,
            "j" => move_line_down,
            "." => goto_last_modification,
            "w" => goto_word,
        },
        ":" => command_mode,

        "i" => insert_mode,
        "I" => insert_at_line_start,
        "a" => append_mode,
        "A" => insert_at_line_end,
        "o" => open_below,
        "O" => open_above,

        "d" => delete_selection,
        "A-d" => delete_selection_noyank,
        "c" => change_selection,
        "A-c" => change_selection_noyank,

        "C" => copy_selection_on_next_line,
        "A-C" => copy_selection_on_prev_line,


        "s" => select_regex,
        "A-s" => split_selection_on_newline,
        "A-minus" => merge_selections,
        "A-_" => merge_consecutive_selections,
        "S" => split_selection,
        ";" => collapse_selection,
        "A-;" => flip_selections,
        "A-o" | "A-up" => expand_selection,
        "A-i" | "A-down" => shrink_selection,
        "A-I" | "A-S-down" => select_all_children,
        "A-p" | "A-left" => select_prev_sibling,
        "A-n" | "A-right" => select_next_sibling,
        "A-e" => move_parent_node_end,
        "A-b" => move_parent_node_start,
        "A-a" => select_all_siblings,

        "%" => select_all,
        "x" => extend_line_below,
        "X" => extend_to_line_bounds,
        "A-x" => shrink_to_line_bounds,

        "m" => { "Match"
            "m" => match_brackets,
            "s" => surround_add,
            "r" => surround_replace,
            "d" => surround_delete,
            "a" => select_textobject_around,
            "i" => select_textobject_inner,
        },
        "[" => { "Left bracket"
            "d" => goto_prev_diag,
            "D" => goto_first_diag,
            "g" => goto_prev_change,
            "G" => goto_first_change,
            "f" => goto_prev_function,
            "t" => goto_prev_class,
            "a" => goto_prev_parameter,
            "c" => goto_prev_comment,
            "e" => goto_prev_entry,
            "T" => goto_prev_test,
            "p" => goto_prev_paragraph,
            "x" => goto_prev_xml_element,
            "space" => add_newline_above,
        },
        "]" => { "Right bracket"
            "d" => goto_next_diag,
            "D" => goto_last_diag,
            "g" => goto_next_change,
            "G" => goto_last_change,
            "f" => goto_next_function,
            "t" => goto_next_class,
            "a" => goto_next_parameter,
            "c" => goto_next_comment,
            "e" => goto_next_entry,
            "T" => goto_next_test,
            "p" => goto_next_paragraph,
            "x" => goto_next_xml_element,
            "space" => add_newline_below,
        },

        "/" => search,
        "?" => rsearch,
        "n" => search_next,
        "N" => search_prev,
        "*" => search_selection_detect_word_boundaries,
        "A-*" => search_selection,

        "u" => undo,
        "U" => redo,
        "A-u" => earlier,
        "A-U" => later,

        "y" => yank,
        // yank_all
        "p" => paste_after,
        // paste_all
        "P" => paste_before,

        "Q" => record_macro,
        "q" => replay_macro,

        ">" => indent,
        "<" => unindent,
        "=" => format_selections,
        "J" => join_selections,
        "A-J" => join_selections_space,
        "K" => keep_selections,
        "A-K" => remove_selections,

        "," => keep_primary_selection,
        "A-," => remove_primary_selection,

        // "q" => record_macro,
        // "Q" => replay_macro,

        "&" => align_selections,
        "_" => trim_selections,

        "(" => rotate_selections_backward,
        ")" => rotate_selections_forward,
        "A-(" => rotate_selection_contents_backward,
        "A-)" => rotate_selection_contents_forward,

        "A-:" => ensure_selections_forward,

        "esc" => normal_mode,
        "C-b" | "pageup" => page_up,
        "C-f" | "pagedown" => page_down,
        "C-u" => page_cursor_half_up,
        "C-d" => page_cursor_half_down,

        // move under <space>c
        "C-c" => toggle_comments,

        // z family for save/restore/combine from/to sels from register

        "C-i" | "tab" => jump_forward, // tab == <C-i>
        "C-o" => jump_backward,
        "C-s" => save_selection,

        "space" => { "Space"
            "f" => file_picker,
            "F" => file_picker_in_current_directory,
            "e" => file_explorer,
            "." => file_explorer_in_current_buffer_directory,
            "b" => buffer_picker,
            "j" => jumplist_picker,
            "s" => lsp_or_syntax_symbol_picker,
            "S" => lsp_or_syntax_workspace_symbol_picker,
            "d" => diagnostics_picker,
            "D" => workspace_diagnostics_picker,
            "g" => changed_file_picker,
            "a" => code_action,
            "'" => last_picker,
            // 调试键位菜单（与暂停时的接管表是**同一张表**，见下 `dap_keymap()`；
            // 有单测 `dap_keymap_matches_space_g` 钉住两者一致）。
            // **不 sticky**：按完一个动作立刻回到普通模式 —— 否则菜单会一直挂着，
            // 未绑定的键（j/k/…）被吞掉，感觉"按了 space G 之后什么都动不了"。
            // `K` = Keys：当前层的**完整**键位表（`:keys` 的键位入口；
            // which-key 提示里"还有 N 个（:keys 看全部）"就是它）
            "K" => keys_help,
            "G" => { "Debug" sticky=false
                "l" => dap_launch,
                "r" => dap_restart,
                "T" => dap_terminate,
                "c" => dap_continue,
                "p" => dap_pause,
                "n" => dap_next,
                "s" => dap_step_in,
                "o" => dap_step_out,
                "t" => dap_switch_thread,
                "f" => dap_switch_stack_frame,
                "S" => dap_sessions,
                "b" => dap_toggle_breakpoint,
                "C-c" => dap_edit_condition,
                "C-l" => dap_edit_log,
                "e" => dap_enable_exceptions,
                "E" => dap_disable_exceptions,
                "v" => dap_variables,
                "K" => dap_eval_under_cursor_key,
                "R" => dap_run_to_cursor_key,
                "?" => debug_help,
            },
            "w" => { "Window"
                "C-w" | "w" => rotate_view,
                "C-s" | "s" => hsplit,
                "C-v" | "v" => vsplit,
                "C-t" | "t" => transpose_view,
                "f" => goto_file_hsplit,
                "F" => goto_file_vsplit,
                "C-q" | "q" => wclose,
                "C-o" | "o" => wonly,
                "C-h" | "h" | "left" => jump_view_left,
                "C-j" | "j" | "down" => jump_view_down,
                "C-k" | "k" | "up" => jump_view_up,
                "C-l" | "l" | "right" => jump_view_right,
                "H" => swap_view_left,
                "J" => swap_view_down,
                "K" => swap_view_up,
                "L" => swap_view_right,
                "n" => { "New split scratch buffer"
                    "C-s" | "s" => hsplit_new,
                    "C-v" | "v" => vsplit_new,
                },
            },
            "y" => yank_to_clipboard,
            "Y" => yank_main_selection_to_clipboard,
            "p" => paste_clipboard_after,
            "P" => paste_clipboard_before,
            "R" => replace_selections_with_clipboard,
            "/" => global_search,
            "k" => hover,
            "r" => rename_symbol,
            "h" => select_references_to_symbol_under_cursor,
            "c" => toggle_comments,
            "C" => toggle_block_comments,
            "A-c" => toggle_line_comments,
            "?" => command_palette,
        },
        "z" => { "View"
            "z" | "c" => align_view_center,
            "t" => align_view_top,
            "b" => align_view_bottom,
            "m" => align_view_middle,
            "k" | "up" => scroll_up,
            "j" | "down" => scroll_down,
            "C-b" | "pageup" => page_up,
            "C-f" | "pagedown" => page_down,
            "C-u" | "backspace" => page_cursor_half_up,
            "C-d" | "space" => page_cursor_half_down,

            "/" => search,
            "?" => rsearch,
            "n" => search_next,
            "N" => search_prev,
        },
        "Z" => { "View" sticky=true
            "z" | "c" => align_view_center,
            "t" => align_view_top,
            "b" => align_view_bottom,
            "m" => align_view_middle,
            "k" | "up" => scroll_up,
            "j" | "down" => scroll_down,
            "C-b" | "pageup" => page_up,
            "C-f" | "pagedown" => page_down,
            "C-u" | "backspace" => page_cursor_half_up,
            "C-d" | "space" => page_cursor_half_down,

            "/" => search,
            "?" => rsearch,
            "n" => search_next,
            "N" => search_prev,
        },

        "\"" => select_register,
        "|" => shell_pipe,
        "A-|" => shell_pipe_to,
        "!" => shell_insert_output,
        "A-!" => shell_append_output,
        "$" => shell_keep_pipe,
        "C-z" => suspend,

        "C-a" => increment,
        "C-x" => decrement,
    });
    let mut select = normal.clone();
    select.merge_nodes(keymap!({ "Select mode"
        "h" | "left" => extend_char_left,
        "j" | "down" => extend_visual_line_down,
        "k" | "up" => extend_visual_line_up,
        "l" | "right" => extend_char_right,

        "w" => extend_next_word_start,
        "b" => extend_prev_word_start,
        "e" => extend_next_word_end,
        "W" => extend_next_long_word_start,
        "B" => extend_prev_long_word_start,
        "E" => extend_next_long_word_end,

        "A-e" => extend_parent_node_end,
        "A-b" => extend_parent_node_start,

        "n" => extend_search_next,
        "N" => extend_search_prev,

        "t" => extend_till_char,
        "f" => extend_next_char,
        "T" => extend_till_prev_char,
        "F" => extend_prev_char,

        "home" => extend_to_line_start,
        "end" => extend_to_line_end,
        "esc" => exit_select_mode,

        "v" => normal_mode,
        "g" => { "Goto"
            "g" => extend_to_file_start,
            "|" => extend_to_column,
            "e" => extend_to_last_line,
            "k" => extend_line_up,
            "j" => extend_line_down,
            "w" => extend_to_word,
        },
    }));
    let insert = keymap!({ "Insert mode"
        "esc" => normal_mode,

        "C-s" => commit_undo_checkpoint,
        "C-x" => completion,
        "C-r" => insert_register,

        "C-w" | "A-backspace" => delete_word_backward,
        "A-d" | "A-del" => delete_word_forward,
        "C-u" => kill_to_line_start,
        "C-k" => kill_to_line_end,
        "C-h" | "backspace" | "S-backspace" => delete_char_backward,
        "C-d" | "del" => delete_char_forward,
        "C-j" | "ret" => insert_newline,
        "tab" => smart_tab,
        "S-tab" => insert_tab,

        "up" => move_visual_line_up,
        "down" => move_visual_line_down,
        "left" => move_char_left,
        "right" => move_char_right,
        "pageup" => page_up,
        "pagedown" => page_down,
        "home" => goto_line_start,
        "end" => goto_line_end_newline,
    });
    hashmap!(
        Mode::Normal => normal,
        Mode::Select => select,
        Mode::Insert => insert,
    )
}

/// 暂停时接管的调试键（**唯一来源**）。
///
/// 比 `space G` 菜单少一个 `l`（launch）：① 暂停时没有"启动"可做；② 面板里 `l` 是"展开节点"
/// （j/k/h/l 树导航），保留它才不会出现"同键异义"。菜单仍是这些键的超集。
pub fn dap_keymap() -> KeyTrie {
    keymap!({ "Debug" sticky=true
        "r" => dap_restart,
        "T" => dap_terminate,
        "c" => dap_continue,
        "p" => dap_pause,
        "n" => dap_next,
        "s" => dap_step_in,
        "o" => dap_step_out,
        "t" => dap_switch_thread,
        "f" => dap_switch_stack_frame,
        "S" => dap_sessions,
        "b" => dap_toggle_breakpoint,
        "C-c" => dap_edit_condition,
        "C-l" => dap_edit_log,
        "e" => dap_enable_exceptions,
        "E" => dap_disable_exceptions,
        "v" => dap_variables,
        "K" => dap_eval_under_cursor_key,
        "R" => dap_run_to_cursor_key,
        "?" => debug_help,
    })
}

/// 运行时生效的调试键表（`dap_keymap()` + `[keys.debug]` 覆盖）。
///
/// 为什么用全局缓存:`input_router::debug_key()` 在**每次按键**上被调用，而它拿不到
/// `Config`（keymap 在 `EditorView` 里、config 在 `Application` 里）。所以配置加载时把
/// 合并结果存这里，按键时只做一次廉价 clone。
static DEBUG_KEYMAP: std::sync::OnceLock<
    std::sync::RwLock<std::sync::Arc<crate::keymap::KeyTrie>>,
> = std::sync::OnceLock::new();

fn debug_keymap_slot() -> &'static std::sync::RwLock<std::sync::Arc<crate::keymap::KeyTrie>> {
    DEBUG_KEYMAP.get_or_init(|| std::sync::RwLock::new(std::sync::Arc::new(dap_keymap())))
}

/// 配置加载/重载时写入（`Config::load` 调）。`None` = 回到内置表。
pub fn set_debug_keymap(tree: Option<crate::keymap::KeyTrie>) {
    let next = std::sync::Arc::new(tree.unwrap_or_else(dap_keymap));
    *debug_keymap_slot()
        .write()
        .unwrap_or_else(|e| e.into_inner()) = next;
}

/// 当前生效的调试键表（含用户 `[keys.debug]` 覆盖）。
pub fn debug_keymap() -> std::sync::Arc<crate::keymap::KeyTrie> {
    debug_keymap_slot()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

#[cfg(test)]
mod dap_keymap_tests {
    use crate::key;
    use crate::keymap::KeyTrie;

    fn keys_of(tree: &KeyTrie) -> Vec<String> {
        let KeyTrie::Node(root) = tree else {
            panic!("应该是 Node");
        };
        root.entries().into_iter().map(|(k, _)| k).collect()
    }

    /// 暂停接管表 ⊆ `space G` 菜单，且菜单只多一个 `l`（launch）。
    #[test]
    fn dap_keymap_matches_space_g() {
        let maps = super::default();
        let menu = maps
            .get(&helix_view::document::Mode::Normal)
            .cloned()
            .expect("Normal 键位表");
        use std::str::FromStr;
        let space_key = helix_view::input::KeyEvent::from_str("space").expect("space 键");
        let space = menu.search(&[space_key]);
        assert!(space.is_some(), "应有 space 前缀");
        let g = space.unwrap().search(&[key!('G')]).expect("space G 存在");
        let mut paused = keys_of(&super::dap_keymap());
        let mut menu = keys_of(g);
        menu.retain(|k| k != "l"); // 菜单多一个 launch
        paused.sort();
        menu.sort();
        assert_eq!(paused, menu, "两边键集必须一致（菜单只多 `l`）");
    }

    /// 表本身钉住：删/加键必须是有意的（gdb 式单键，无两键序列）。
    #[test]
    fn dap_keymap_keys_are_stable() {
        assert_eq!(
            keys_of(&super::dap_keymap()),
            vec![
                "r", "T", "c", "p", "n", "s", "o", "t", "f", "S", "b", "C-c", "C-l", "e", "E", "v",
                "K", "R", "?"
            ]
        );
    }
}
