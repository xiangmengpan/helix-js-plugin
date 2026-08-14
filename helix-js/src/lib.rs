//! JavaScript plugin runtime for the Helix editor (PoC).

mod commands;
mod icons;
mod layout;
mod popup;
mod pty;
mod shell;
mod state;
mod theme;
mod types;

pub use commands::*;
pub use icons::*;
pub use popup::*;
pub use shell::*;
pub use state::*;
pub use theme::*;
pub use types::*;

use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsString, NativeFunction};

/// 初始化线程局部运行时（幂等）：消息/UI 队列、事件通道、boa 引擎与全局 `helix` 对象。
pub fn init() {
    state::MESSAGES.get_or_init(Default::default);
    state::UI_REQUESTS.get_or_init(Default::default);
    let _ = state::LAST_LAYOUT.get_or_init(Default::default);
    state::with_term_events(|t| {
        if t.is_none() {
            let (tx, rx) = std::sync::mpsc::channel();
            *t = Some(state::wake_sender(tx));
            state::with_term_events_rx(|r| *r = Some(rx));
        }
    });
    state::with_async_events(|t| {
        if t.is_none() {
            let (tx, rx) = std::sync::mpsc::channel();
            *t = Some(state::wake_sender(tx));
            state::with_async_events_rx(|r| *r = Some(rx));
        }
    });
    state::with_engine_slot(|slot| {
        if slot.is_none() {
            let engine = Box::leak(Box::new(Context::default()));
            // ObjectInitializer 方法取 &mut self，链式必须在一个表达式内；
            // term_resize 是 cfg(unix) 的，拆成两步注册（builder 可变绑定）
            let mut builder = ObjectInitializer::new(engine);
            builder
                .function(NativeFunction::from_fn_ptr(commands::js_echo), JsString::from("echo"), 1)
                .function(
                    NativeFunction::from_fn_ptr(commands::js_register_command),
                    JsString::from("register_command"),
                    2,
                )
                .function(NativeFunction::from_fn_ptr(popup::js_open_popup), JsString::from("open_popup"), 1)
                .function(NativeFunction::from_fn_ptr(popup::js_open_panel), JsString::from("open_panel"), 1)
                .function(NativeFunction::from_fn_ptr(popup::js_close_panel), JsString::from("close_panel"), 1)
                .function(NativeFunction::from_fn_ptr(popup::js_read_dir), JsString::from("read_dir"), 1)
                .function(NativeFunction::from_fn_ptr(popup::js_open_file), JsString::from("open_file"), 1)
                .function(NativeFunction::from_fn_ptr(popup::js_move_panel), JsString::from("move_panel"), 2)
                .function(NativeFunction::from_fn_ptr(popup::js_set_buffer_icon), JsString::from("set_buffer_icon"), 1)
                .function(NativeFunction::from_fn_ptr(popup::js_el), JsString::from("el"), 2)
                .function(NativeFunction::from_fn_ptr(commands::js_on), JsString::from("on"), 2)
                .function(NativeFunction::from_fn_ptr(commands::js_map), JsString::from("map"), 3)
                .function(NativeFunction::from_fn_ptr(commands::js_set_cursor), JsString::from("set_cursor"), 2)
                .function(NativeFunction::from_fn_ptr(commands::js_set_selection), JsString::from("set_selection"), 4)
                .function(NativeFunction::from_fn_ptr(popup::js_set_statusline), JsString::from("set_statusline"), 1)
                .function(NativeFunction::from_fn_ptr(commands::js_load), JsString::from("load"), 1)
                .function(NativeFunction::from_fn_ptr(commands::js_plugin), JsString::from("plugin"), 2)
                .function(NativeFunction::from_fn_ptr(commands::js_export), JsString::from("export"), 1)
                .function(NativeFunction::from_fn_ptr(commands::js_lazy), JsString::from("lazy"), 2)
                .function(NativeFunction::from_fn_ptr(commands::js_run_command), JsString::from("run_command"), 1)
                .function(NativeFunction::from_fn_ptr(commands::js_run), JsString::from("run"), 1)
                .function(NativeFunction::from_fn_ptr(shell::js_run_async), JsString::from("run_async"), 2)
                .function(NativeFunction::from_fn_ptr(shell::js_spawn), JsString::from("spawn"), 1)
                .function(NativeFunction::from_fn_ptr(shell::js_term_write), JsString::from("term_write"), 2)
                .function(NativeFunction::from_fn_ptr(shell::js_term_kill), JsString::from("term_kill"), 1)
                .function(NativeFunction::from_fn_ptr(shell::js_read_file_async), JsString::from("read_file_async"), 2)
                .function(NativeFunction::from_fn_ptr(shell::js_write_file_async), JsString::from("write_file_async"), 3)
                .function(NativeFunction::from_fn_ptr(shell::js_stat_async), JsString::from("stat_async"), 2)
                .function(NativeFunction::from_fn_ptr(shell::js_glob_async), JsString::from("glob_async"), 2)
                .function(NativeFunction::from_fn_ptr(popup::js_open_terminal), JsString::from("open_terminal"), 1)
                .function(NativeFunction::from_fn_ptr(popup::js_term_feed), JsString::from("term_feed"), 2)
                .function(NativeFunction::from_fn_ptr(popup::js_set_terminal_mode), JsString::from("set_terminal_mode"), 2)
                .function(NativeFunction::from_fn_ptr(popup::js_term_clear), JsString::from("term_clear"), 1)
                .function(NativeFunction::from_fn_ptr(popup::js_term_save), JsString::from("term_save"), 2)
                .function(NativeFunction::from_fn_ptr(popup::js_resize_term), JsString::from("resize_term"), 2)
                .function(NativeFunction::from_fn_ptr(layout::js_split), JsString::from("split"), 2)
                .function(NativeFunction::from_fn_ptr(layout::js_close_leaf), JsString::from("close_leaf"), 1)
                .function(NativeFunction::from_fn_ptr(layout::js_zoom_leaf), JsString::from("zoom"), 1)
                .function(NativeFunction::from_fn_ptr(layout::js_unzoom), JsString::from("unzoom"), 0)
                .function(NativeFunction::from_fn_ptr(layout::js_resize_leaf), JsString::from("resize_leaf"), 2)
                .function(NativeFunction::from_fn_ptr(layout::js_resize_leaf_dir), JsString::from("layout_resize"), 3)
                .function(NativeFunction::from_fn_ptr(layout::js_swap_leaves), JsString::from("layout_swap"), 2)
                .function(NativeFunction::from_fn_ptr(layout::js_minimize_leaf), JsString::from("layout_minimize"), 2)
                .function(NativeFunction::from_fn_ptr(layout::js_focus_leaf_dir), JsString::from("layout_focus"), 2)
                .function(NativeFunction::from_fn_ptr(layout::js_swap_leaf_dir), JsString::from("layout_swap_dir"), 2)
                .function(NativeFunction::from_fn_ptr(layout::js_equalize_leaf), JsString::from("layout_equalize"), 1)
                .function(NativeFunction::from_fn_ptr(layout::js_layout_fix), JsString::from("layout_fix"), 2)
                .function(NativeFunction::from_fn_ptr(layout::js_focus_leaf), JsString::from("focus"), 1)
                .function(NativeFunction::from_fn_ptr(layout::js_get_layout), JsString::from("get_layout"), 0)
                .function(NativeFunction::from_fn_ptr(layout::js_restore_layout), JsString::from("restore_layout"), 1)
                .function(NativeFunction::from_fn_ptr(theme::js_set_theme), JsString::from("set_theme"), 1)
                .function(NativeFunction::from_fn_ptr(theme::js_reset_theme), JsString::from("reset_theme"), 0)
                .function(NativeFunction::from_fn_ptr(theme::js_get_style), JsString::from("get_style"), 1)
                .function(NativeFunction::from_fn_ptr(theme::js_theme_info), JsString::from("theme_info"), 0)
                .function(NativeFunction::from_fn_ptr(theme::js_set_theme_name), JsString::from("set_theme_name"), 1)
                .function(NativeFunction::from_fn_ptr(icons::js_set_diagnostic_icons), JsString::from("set_diagnostic_icons"), 1)
                .function(NativeFunction::from_fn_ptr(popup::js_term_list), JsString::from("term_list"), 0)
                .function(NativeFunction::from_fn_ptr(popup::js_term_close), JsString::from("term_close"), 1);
            #[cfg(unix)]
            builder.function(NativeFunction::from_fn_ptr(shell::js_term_resize), JsString::from("term_resize"), 3);
            let helix = builder.build();
            engine
                .register_global_property(JsString::from("helix"), helix, Attribute::READONLY | Attribute::NON_ENUMERABLE)
                .expect("register helix object");
            *slot = Some(engine);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // 多个测试共享全局运行时，用锁串行化避免消息队列竞争
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// v13 任务简报验证测试：read_dir 排序/is_dir、open_file、move_panel 入队 + 校验。
    /// 简报原文断言 count:2，但设置创建 3 个条目（a.txt、b.js、sub/）→ 按实际调整为 count:3；
    /// 排序断言 entries[0].name < entries[1].name 不受影响（a.txt < b.js）。
    #[test]
    fn term_wake_fires_on_event() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        let fired = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fired2 = fired.clone();
        set_term_wake(Box::new(move || {
            fired2.store(true, std::sync::atomic::Ordering::SeqCst);
        }));
        load_script(
            r#"
            helix.register_command("wk", () => {
                helix.spawn({ pty: false, cmd: "echo wake-test", onChunk: () => {}, onExit: () => {} });
            });
            "#,
        )
        .unwrap();
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command("wk", &ctx).unwrap());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !fired.load(std::sync::atomic::Ordering::SeqCst) && std::time::Instant::now() < deadline {
            // 消费事件（让 worker 继续/完成），wake 在 send 时触发
            let _ = drain_term_events();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(fired.load(std::sync::atomic::Ordering::SeqCst), "wake should fire on event send");
    }

    #[test]
    fn open_terminal_api() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("ot", () => {
            const pid = helix.open_terminal({ cmd: "cat", side: "right", size: 40 });
            helix.echo("pid:" + pid);
            helix.term_feed(pid, "abc");
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command("ot", &ctx).unwrap());
        assert!(take_messages()[0].starts_with("pid:"));
        let reqs = take_ui_requests();
        assert!(matches!(&reqs[0], UiRequest::OpenTerminal { side, size, .. } if side == "right" && *size == 40));
        assert!(matches!(&reqs[1], UiRequest::TermFeed { chunk, .. } if chunk == "abc"));
        // 校验
        assert!(load_script(r#"helix.open_terminal({ cmd: "x", side: "top", size: 10 });"#).is_err());
        assert!(load_script(r#"helix.open_terminal({ cmd: "x", side: "right" });"#).is_err()); // 缺 size
    }

    /// 交互 bash 启动不应报 "cannot set terminal process group"（缺 setsid/控制终端）。
    /// 修复前：bash job control 初始化失败向 stderr 打印该错误。
    #[test]
    fn pty_bash_interactive_no_error() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("bi", () => {
            const pid = helix.open_terminal({ cmd: "bash -i", side: "bottom", size: 10 });
            helix.echo("pid:" + pid);
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command("bi", &ctx).unwrap());
        let _ = take_messages();
        let pty_id = match &take_ui_requests()[0] {
            UiRequest::OpenTerminal { pty_id, .. } => *pty_id,
            other => panic!("expected OpenTerminal, got {other:?}"),
        };
        // 收集 bash 启动输出直到报错或超时（提示符正常出现即好）。
        // 注意：必须 resolve 事件才能触发 bridge 回调（term_feed → UI 请求），
        // 只 drain 不 resolve 会让断言假绿（输出为空）。
        let mut output = String::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
        while std::time::Instant::now() < deadline {
            for ev in drain_term_events() {
                let eid = match &ev {
                    TermEvent::Chunk(id, _) => *id,
                    TermEvent::Exit(id, _, _) => *id,
                };
                let _ = resolve_term_event(eid, ev);
            }
            for req in take_ui_requests() {
                if let UiRequest::TermFeed { chunk, .. } = req {
                    output.push_str(&chunk);
                }
            }
            if output.contains("cannot set terminal process group") {
                break;
            }
            if output.contains('$') {
                break; // 提示符出现
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = term_kill(pty_id);
        // 消费 Exit 事件，清理回调注册
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !with_terms(|m| m.is_empty()) && std::time::Instant::now() < deadline {
            let _ = drain_term_events();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            !output.contains("cannot set terminal process group"),
            "交互 bash 不应报 job control 错误，实际输出: {output:?}"
        );
    }

    #[test]
    fn open_terminal_after_kill_repro() {
        // 关闭（term_kill，模拟 Esc/q 关闭终端）后再 open_terminal：应能正常打开。
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("t3", () => {
            const pid = helix.open_terminal({ cmd: "cat", side: "bottom", size: 10 });
            helix.echo("pid:" + pid);
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command("t3", &ctx).unwrap(), "第一次 open_terminal");
        let _ = take_messages();
        let pty_id = match &take_ui_requests()[0] {
            UiRequest::OpenTerminal { pty_id, .. } => *pty_id,
            other => panic!("expected OpenTerminal, got {other:?}"),
        };
        // 模拟关闭：Esc/q → remove_type → PluginTerminal::drop → term_kill(pty_id)
        term_kill(pty_id).unwrap();
        // 消费 Exit 事件（worker 退出回调），与事件循环 drain 一致
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !with_terms(|m| m.is_empty()) && std::time::Instant::now() < deadline {
            let _ = drain_term_events();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(run_command("t3", &ctx).unwrap(), "kill 后再 open_terminal 应成功");
        let _ = take_messages();
        let _ = take_ui_requests(); // 清空第二次 open 的请求，避免污染后续测试
    }

    #[test]
    fn sidecar_apis() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        let dir = std::env::temp_dir().join(format!("helix-js-sidecar-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        std::fs::write(dir.join("b.js"), "b").unwrap();

        // 路径经 {:?}（JSON 字符串转义）注入脚本
        let dir_str = dir.to_string_lossy();
        let file_path = dir.join("a.txt");
        let file_str = file_path.to_string_lossy();
        let script = format!(
            r#"
        helix.register_command("sc", () => {{
            const entries = helix.read_dir({dir:?});
            helix.echo("count:" + entries.length + " sorted:" + (entries[0].name < entries[1].name));
            const dirs = entries.filter(e => e.is_dir);
            helix.echo("dirs:" + dirs.length + ":" + dirs[0].name);
            helix.open_file({file:?});
            helix.move_panel(7, "left");
        }});
        "#,
            dir = dir_str,
            file = file_str,
        );
        load_script(&script).unwrap();
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command("sc", &ctx).unwrap());
        let msgs = take_messages();
        assert!(msgs[0].starts_with("count:3 sorted:true"), "{msgs:?}");
        assert_eq!(msgs[1], "dirs:1:sub");
        let reqs = take_ui_requests();
        assert!(matches!(&reqs[0], UiRequest::OpenFile { path } if path.ends_with("a.txt")));
        assert!(matches!(&reqs[1], UiRequest::MovePanel { id: 7, side } if side == "left"));

        // 校验：read_dir 不存在路径 → 抛错；move_panel 非法 side → 抛错；open_file 非字符串 → 抛错
        load_script(
            r#"
        helix.register_command("bad1", () => { helix.read_dir("/nonexistent-helix-js-xyz"); });
        helix.register_command("bad2", () => { helix.move_panel(7, "top"); });
        helix.register_command("bad3", () => { helix.open_file(42); });
        "#,
        )
        .unwrap();
        assert!(run_command("bad1", &ctx).is_err());
        assert!(run_command("bad2", &ctx).is_err());
        assert!(run_command("bad3", &ctx).is_err());
    }

    #[test]
    fn theme_overrides_api() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        // 清残留脏位（本测试是唯一 set_theme 的测试，防御顺序依赖）
        let _ = take_theme_dirty();
        assert!(theme_overrides().is_empty());

        // set_theme：整体替换 + 置脏；字符串 = fg，对象 = {fg, bg, modifiers}
        load_script(
            r##"
        helix.set_theme({
            "ui.popup": "#ff00aa",
            "error": "red",
            "ui.window": { fg: "#112233", bg: "#445566", modifiers: ["italic", "bold"] },
            "ui.linenr": 42,
        });
        "##,
        )
        .unwrap();
        assert!(take_theme_dirty());
        let ov = theme_overrides();
        let popup = ov.get("ui.popup").unwrap();
        assert_eq!(popup.fg.as_deref(), Some("#ff00aa"));
        assert_eq!(popup.bg, None);
        assert!(popup.modifiers.is_empty(), "字符串值只有 fg");
        assert_eq!(ov.get("error").unwrap().fg.as_deref(), Some("red"));
        let win = ov.get("ui.window").unwrap();
        assert_eq!(win.fg.as_deref(), Some("#112233"));
        assert_eq!(win.bg.as_deref(), Some("#445566"));
        assert_eq!(win.modifiers, vec!["italic", "bold"]);
        assert!(!ov.contains_key("ui.linenr"), "非字符串非对象值应被忽略");

        // 再次 set_theme：替换而非累积
        load_script(r#"helix.set_theme({ "error": "blue" });"#).unwrap();
        assert!(take_theme_dirty());
        let ov = theme_overrides();
        assert_eq!(ov.len(), 1);
        assert_eq!(ov.get("error").unwrap().fg.as_deref(), Some("blue"));

        // 空对象 → 清空覆盖（等价的 reset）
        load_script(r#"helix.set_theme({});"#).unwrap();
        assert!(theme_overrides().is_empty());

        // reset_theme：清空 + 置脏
        load_script(r#"helix.set_theme({ "error": "red" });"#).unwrap();
        assert!(take_theme_dirty());
        load_script(r#"helix.reset_theme();"#).unwrap();
        assert!(take_theme_dirty());
        assert!(theme_overrides().is_empty());

        // 非法参数（非对象/缺参）→ JS 报错
        assert!(load_script(r#"helix.set_theme("red");"#).is_err());
        assert!(load_script(r#"helix.set_theme();"#).is_err());
    }

    #[test]
    fn echo_captures_message() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(r#"helix.echo("hello from js");"#).unwrap();
        assert_eq!(take_messages(), vec!["hello from js"]);
    }

    #[test]
    fn loaded_scripts_list() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script_named("a.js", r#"helix.register_command("a", () => {});"#).unwrap();
        load_script_named("b.js", r#"helix.register_command("b", () => {});"#).unwrap();
        let names = loaded_scripts();
        assert!(names.iter().any(|n| n == "a.js"));
        assert!(names.iter().any(|n| n == "b.js"));
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
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("where", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["cursor: 1,2"]);

        // 未注册的命令返回 false
        assert!(!run_command("nope", &ctx).unwrap());
        // 非法命令名（含空白）注册时报错
        assert!(load_script(r#"helix.register_command("bad name", () => {});"#).is_err());
    }

    #[test]
    fn panel_api() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        const pid = helix.open_panel({ side: "right", size: 30, render: () => ["p1", "p2"] });
        helix.echo("id:" + pid);
        helix.close_panel(pid);
        "#,
        )
        .unwrap();
        let reqs = take_ui_requests();
        // 编译器建议：matches! 守卫未用 id 绑定 → id: _（简报原文绑了 id，clippy 要求 0 告警）
        assert!(matches!(&reqs[0], UiRequest::OpenPanel { id: _, side, size } if side == "right" && *size == 30));
        assert!(matches!(&reqs[1], UiRequest::ClosePanel { id: _ }));
        assert!(take_messages()[0].starts_with("id:"));
        // 校验：side 白名单 / size / render
        assert!(load_script(r#"helix.open_panel({ side: "top", size: 10, render: () => [] });"#).is_err());
        assert!(load_script(r#"helix.open_panel({ side: "right", size: 10 });"#).is_err()); // 缺 render
        assert!(load_script(r#"helix.open_panel({ side: "right", size: "big", render: () => [] });"#).is_err());
        crate::state::with_open_panels(|p| p.clear()); // 清 OPEN_PANELS，防污染后续测试
    }

    #[test]
    fn panel_onkey() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        const pid = helix.open_panel({
            side: "right", size: 20,
            render: () => ["p"],
            onKey: (key) => { helix.echo("panel-key:" + key.name); return key.name === "Esc" ? "close" : "handled"; },
        });
        helix.echo("pid:" + pid);
        "#,
        )
        .unwrap();
        let reqs = take_ui_requests();
        // 编译器建议：matches! 未用 id 绑定 → id: _（简报原文绑了 id，clippy 要求 0 告警）
        assert!(matches!(&reqs[0], UiRequest::OpenPanel { id: _, .. }));
        // 编译器要求：单臂 match 非穷尽 → 改 let-else（与 popup_lifecycle 同款）
        let UiRequest::OpenPanel { id, .. } = reqs[0] else { unreachable!("expected OpenPanel") };
        assert!(take_messages()[0].starts_with("pid:"));
        // popup_key 走同一注册表：Esc → close，其他 → handled
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        let esc = PluginKey { name: "Esc".into(), shift: false, ctrl: false, alt: false };
        assert_eq!(popup_key(id, &esc, &ctx).unwrap(), PopupKeyResult::Close);
        assert_eq!(take_messages(), vec!["panel-key:Esc"]);
        // panel_has_onkey：有 onKey → true（简报测试的补充断言）
        assert!(panel_has_onkey(id));
        // 无 onKey 的面板：false（helix-term 侧据此全 Ignore 穿透，不调 popup_key）
        load_script(r#"helix.open_panel({ side: "left", size: 10, render: () => ["x"] });"#).unwrap();
        let UiRequest::OpenPanel { id: id2, .. } = take_ui_requests()[0] else { unreachable!("expected OpenPanel") };
        assert!(!panel_has_onkey(id2));
        crate::state::with_open_panels(|p| p.clear()); // 清 OPEN_PANELS（两个面板未 close），防污染
        let _ = take_ui_requests();
    }

    #[test]
    fn popup_lifecycle() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        let rendered = null;
        helix.open_popup({
            render: () => ["a", "b", "c"],
            onKey: (key) => key.name === "Down" ? "handled" : "close",
            onClose: () => helix.echo("closed:" + rendered),
        });
        "#,
        )
        .unwrap();
        let reqs = take_ui_requests();
        assert_eq!(reqs.len(), 1);
        let UiRequest::OpenPopup { id, .. } = reqs[0] else { unreachable!("expected OpenPopup") };
        assert_eq!(id, 1); // 自增从 1 开始

        let lines = render_popup(id, 40, 10, None).unwrap();
        assert_eq!(
            lines,
            Content::Lines(vec![
                StyledLine::plain("a"),
                StyledLine::plain("b"),
                StyledLine::plain("c"),
            ])
        );

        let key = PluginKey { name: "Down".into(), shift: false, ctrl: false, alt: false };
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert_eq!(popup_key(id, &key, &ctx).unwrap(), PopupKeyResult::Handled);
        let key = PluginKey { name: "Esc".into(), shift: false, ctrl: false, alt: false };
        assert_eq!(popup_key(id, &key, &ctx).unwrap(), PopupKeyResult::Close);

        close_popup(id).unwrap();
        assert_eq!(take_messages(), vec!["closed:null"]);
        assert!(render_popup(id, 40, 10, None).is_err()); // 已关闭，注册表移除
    }

    #[test]
    fn popup_default_keys_and_validation() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        // 未提供 onKey：Esc 默认关闭，其他穿透
        load_script(r#"helix.open_popup({ render: () => ["x"] });"#).unwrap();
        let id = match take_ui_requests()[0] {
            UiRequest::OpenPopup { id, .. } => id,
            _ => unreachable!("expected OpenPopup"),
        };
        let key = PluginKey { name: "Enter".into(), shift: false, ctrl: false, alt: false };
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert_eq!(popup_key(id, &key, &ctx).unwrap(), PopupKeyResult::Ignored);
        let key = PluginKey { name: "Esc".into(), shift: false, ctrl: false, alt: false };
        assert_eq!(popup_key(id, &key, &ctx).unwrap(), PopupKeyResult::Close);
        close_popup(id).unwrap();

        // render 非数组 → Err
        load_script(r#"helix.open_popup({ render: () => "not an array" });"#).unwrap();
        let id = match take_ui_requests()[0] {
            UiRequest::OpenPopup { id, .. } => id,
            _ => unreachable!("expected OpenPopup"),
        };
        assert!(render_popup(id, 40, 10, None).is_err());
        close_popup(id).unwrap();

        // 参数缺失/类型错误 → JS 报错
        assert!(load_script(r#"helix.open_popup({});"#).is_err());
        assert!(load_script(r#"helix.open_popup({ render: 42 });"#).is_err());
    }

    #[test]
    fn popup_size_position() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.open_popup({ render: () => ["x"], width: 40, height: 10, position: { row: 3, col: 4 } });
        helix.open_popup({ render: () => ["y"] });
        "#,
        )
        .unwrap();
        let reqs = take_ui_requests();
        assert_eq!(reqs.len(), 2);
        match &reqs[0] {
            UiRequest::OpenPopup { width, height, position, .. } => {
                assert_eq!(*width, Some(40));
                assert_eq!(*height, Some(10));
                assert_eq!(*position, Some((3, 4)));
            }
            other => panic!("expected OpenPopup, got {other:?}"),
        }
        match &reqs[1] {
            UiRequest::OpenPopup { width, height, position, .. } => {
                assert_eq!(*width, None);
                assert_eq!(*height, None);
                assert_eq!(*position, None);
            }
            other => panic!("expected OpenPopup, got {other:?}"),
        }
        // 非法类型
        assert!(load_script(r#"helix.open_popup({ render: () => [], width: "big" });"#).is_err());
    }

    #[test]
    fn popup_styled_lines() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.open_popup({
            render: () => [
                { text: "err: ", style: "error" },
                "plain",
                { text: "warn" },
            ],
        });
        "#,
        )
        .unwrap();
        let id = match take_ui_requests()[0] { UiRequest::OpenPopup { id, .. } => id, _ => unreachable!("expected OpenPopup") };
        let lines = render_popup(id, 40, 10, None).unwrap();
        assert_eq!(
            lines,
            Content::Lines(vec![
                StyledLine::styled("err: ", "error"),
                StyledLine::plain("plain"),
                StyledLine::plain("warn"),
            ])
        );
        close_popup(id).unwrap();
        // 非法元素（缺 text / 非字符串非对象）→ Err
        load_script(r#"helix.open_popup({ render: () => [{ style: "error" }] });"#).unwrap();
        let id = match take_ui_requests()[0] { UiRequest::OpenPopup { id, .. } => id, _ => unreachable!("expected OpenPopup") };
        assert!(render_popup(id, 40, 10, None).is_err());
        close_popup(id).unwrap();
    }

    #[test]
    fn popup_onkey_edits_doc() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.open_popup({
            render: () => ["a", "b"],
            onKey: (key, doc) => {
                if (key.name === "Enter") {
                    doc.insert(doc.cursor.row, doc.cursor.col, "XYZ");
                    return "close";
                }
                return "handled";
            },
        });
        "#,
        )
        .unwrap();
        let id = match take_ui_requests()[0] {
            UiRequest::OpenPopup { id, .. } => id,
            _ => unreachable!("expected OpenPopup"),
        };
        let ctx = CommandContext {
            path: Some("/tmp/p.rs".into()),
            text: "line1\nline2".into(),
            cursor: (1, 2),
            selection: ((0, 0), (0, 0)),
        };
        let key = PluginKey { name: "Enter".into(), shift: false, ctrl: false, alt: false };
        assert_eq!(popup_key(id, &key, &ctx).unwrap(), PopupKeyResult::Close);
        assert_eq!(
            take_edits(),
            vec![Edit { start: (1, 2), end: (1, 2), insert: "XYZ".into() }]
        );
        close_popup(id).unwrap();
    }

    #[test]
    fn buffer_icon_hook() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        assert_eq!(bufferline_icon(Some("a.rs")), None); // 未注册 → None

        load_script(
            r#"
        helix.set_buffer_icon((path) => path && path.endsWith(".rs") ? "🦀" : null);
        "#,
        )
        .unwrap();
        assert_eq!(bufferline_icon(Some("main.rs")), Some("🦀".to_string()));
        assert_eq!(bufferline_icon(Some("main.py")), None);
        assert_eq!(bufferline_icon(None), None);
    }

    #[test]
    fn doc_edits_queue() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("edit", (ctx) => {
            ctx.doc.insert(1, 2, "ab");
            ctx.doc.replace(0, 0, 0, 5, "new");
            ctx.doc.delete(3, 0, 4, 0);
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext { path: None, text: String::new(), cursor: (1, 2), selection: ((0, 0), (0, 0)) };
        run_command("edit", &ctx).unwrap();
        let edits = take_edits();
        assert_eq!(
            edits,
            vec![
                Edit { start: (1, 2), end: (1, 2), insert: "ab".into() },
                Edit { start: (0, 0), end: (0, 5), insert: "new".into() },
                Edit { start: (3, 0), end: (4, 0), insert: String::new() },
            ]
        );
        // take_edits 清空
        assert!(take_edits().is_empty());
    }

    #[test]
    fn doc_edit_validation() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        // 类型错误 → 命令失败（run_command 返回 Err），且队列被清空
        load_script(
            r#"
        helix.register_command("bad1", (ctx) => { ctx.doc.insert("x", 0, "a"); });
        helix.register_command("bad2", (ctx) => { ctx.doc.replace(0, 0, 0, 0, 42); });
        helix.register_command("bad3", (ctx) => { ctx.doc.delete(0, 0, 0); });
        "#,
        )
        .unwrap();
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command("bad1", &ctx).is_err());
        assert!(take_edits().is_empty());
        assert!(run_command("bad2", &ctx).is_err());
        assert!(run_command("bad3", &ctx).is_err());
        assert!(take_edits().is_empty());
        // 正常命令运行后队列仍有值（供 helix-term 消费）
        load_script(r#"helix.register_command("ok", (ctx) => { ctx.doc.insert(0, 0, "z"); });"#).unwrap();
        run_command("ok", &ctx).unwrap();
        assert_eq!(take_edits().len(), 1);
    }

    #[test]
    fn event_handlers() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        assert!(!has_handlers("save"));

        load_script(
            r#"
        let order = [];
        helix.on("save", (doc) => { order.push("a"); });
        helix.on("save", (doc) => { order.push("b"); });
        helix.on("mode-change", (mode, doc) => { helix.echo("mode:" + mode); });
        "#,
        )
        .unwrap();

        assert!(has_handlers("save"));
        assert!(has_handlers("mode-change"));
        assert!(!has_handlers("buffer-open"));

        // emit 带编辑队列清空 + 多处理器按注册顺序
        let ctx = CommandContext { path: Some("/tmp/e.rs".into()), text: "x".into(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        emit_event("save", &ctx, None).unwrap();
        assert!(take_edits().is_empty());

        // 事件名白名单校验 + 回调类型校验
        assert!(load_script(r#"helix.on("bogus", () => {});"#).is_err());
        assert!(load_script(r#"helix.on("save", 42);"#).is_err());

        // mode-change 处理器带 mode 参数 + echo
        emit_event("mode-change", &ctx, Some("insert")).unwrap();
        assert_eq!(take_messages(), vec!["mode:insert"]);
    }

    #[test]
    fn doc_change_event() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.on("doc-change", (doc) => { helix.echo("changed:" + doc.cursor.row); });
        "#,
        )
        .unwrap();
        assert!(has_handlers("doc-change"));
        let ctx = CommandContext { path: None, text: "x".into(), cursor: (2, 0), selection: ((0, 0), (0, 0)) };
        emit_event("doc-change", &ctx, None).unwrap();
        assert_eq!(take_messages(), vec!["changed:2"]);
        // 未注册的事件名仍然报错
        assert!(load_script(r#"helix.on("bogus", () => {});"#).is_err());
    }

    #[test]
    fn keymap_registration() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();

        // 字符串命令 → MapKey 入队
        load_script(r#"helix.map("normal", "gd", "goto-def");"#).unwrap();
        let reqs = take_ui_requests();
        assert_eq!(reqs.len(), 1);
        match &reqs[0] {
            UiRequest::MapKey { mode, key, command } => {
                assert_eq!(mode, "normal");
                assert_eq!(key, "gd");
                assert_eq!(command, "goto-def");
            }
            other => panic!("expected MapKey, got {other:?}"),
        }

        // 回调 → 注册 __mapped_N + MapKey 入队
        load_script(
            r#"
        helix.map("insert", "C-n", () => { helix.echo("cb"); });
        "#,
        )
        .unwrap();
        let reqs = take_ui_requests();
        assert_eq!(reqs.len(), 1);
        let command = match &reqs[0] {
            UiRequest::MapKey { command, .. } => command.clone(),
            other => panic!("expected MapKey, got {other:?}"),
        };
        assert!(command.starts_with("__mapped_"), "command: {command}");
        // 注册的命令可以运行（与普通插件命令同机制）
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command(&command, &ctx).unwrap());
        assert_eq!(take_messages(), vec!["cb"]);

        // 校验失败
        assert!(load_script(r#"helix.map("bogus", "x", "y");"#).is_err());
        assert!(load_script(r#"helix.map("normal", 42, "y");"#).is_err());
        assert!(load_script(r#"helix.map("normal", "x", 42);"#).is_err());
        assert!(load_script(r#"helix.map("normal", "", "y");"#).is_err());
    }

    #[test]
    fn statusline_hook() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        let ctx = StatuslineCtx {
            path: Some("/tmp/a.rs".into()),
            mode: "insert".into(),
            cursor: (3, 7),
            total_lines: 100,
            diagnostics_error: 2,
            diagnostics_warning: 1,
            window_mode: false,
        };
        assert_eq!(statusline_parts(&ctx), None);

        // 字符串 → 单段（style None）
        load_script(r#"helix.set_statusline((ctx) => ctx.mode + ":" + ctx.cursor.row);"#).unwrap();
        assert_eq!(
            statusline_parts(&ctx),
            Some(vec![StatuslinePart { text: "insert:3".into(), style: None, zone: None }])
        );

        // 数组 → 多段（字符串项 / {text, style} 项混用；style 透传）
        load_script(
            r#"
        helix.set_statusline((ctx) => [
            { text: " N ", style: "ui.statusline.normal" },
            ctx.mode + ":" + ctx.cursor.row,
            { text: String(ctx.diagnostics_error), style: "error", zone: "right" },
        ]);
        "#,
        )
        .unwrap();
        assert_eq!(
            statusline_parts(&ctx),
            Some(vec![
                StatuslinePart { text: " N ".into(), style: Some("ui.statusline.normal".into()), zone: None },
                StatuslinePart { text: "insert:3".into(), style: None, zone: None },
                StatuslinePart { text: "2".into(), style: Some("error".into()), zone: Some("right".into()) },
            ])
        );

        // 返回 null → None
        load_script(r#"helix.set_statusline(() => null);"#).unwrap();
        assert_eq!(statusline_parts(&ctx), None);
        // 抛错 → None
        load_script(r#"helix.set_statusline(() => { throw new Error("boom"); });"#).unwrap();
        assert_eq!(statusline_parts(&ctx), None);
        // set_statusline(null) 清除
        load_script(r#"helix.set_statusline(null);"#).unwrap();
        assert_eq!(statusline_parts(&ctx), None);
        // 非法参数 → JS 报错
        assert!(load_script(r#"helix.set_statusline(42);"#).is_err());
    }

    #[test]
    fn command_docs() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("doc1", () => {}, "first doc");
        helix.register_command("nodoc", () => {});
        helix.register_command("nulldoc", () => {}, undefined);
        "#,
        )
        .unwrap();
        assert_eq!(command_doc("doc1"), Some("first doc".to_string()));
        assert_eq!(command_doc("nodoc"), None);
        assert_eq!(command_doc("nulldoc"), None);
        assert_eq!(command_doc("missing"), None);
        // 非法 doc 类型 → 报错
        assert!(load_script(r#"helix.register_command("bad", () => {}, 42);"#).is_err());
    }

    #[test]
    fn plugin_reload() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script_named("a.js", r#"helix.register_command("reload-cmd", () => { helix.echo("v1"); });"#).unwrap();
        load_script_named("b.js", r#"helix.on("save", () => {});"#).unwrap();

        assert!(has_handlers("save"));
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command("reload-cmd", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["v1"]);

        // reload：清空状态后重跑全部已记录脚本
        reload_all().unwrap();
        assert!(has_handlers("save"), "handlers re-registered after reload");
        assert!(run_command("reload-cmd", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["v1"]);
        let _ = take_ui_requests(); // 清 reload_all 入队的 ClosePanel
    }

    #[test]
    fn reload_closes_open_panel_layer() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        // 面板由命令打开（不在加载时），保证 reload 重跑脚本不会自动重开面板
        load_script(
            r#"helix.register_command("open-panel", () => { helix.open_panel({ side: "right", size: 30, render: () => ["p1"] }); });"#,
        )
        .unwrap();
        let ctx = CommandContext {
            path: None,
            text: String::new(),
            cursor: (0, 0),
            selection: ((0, 0), (0, 0)),
        };
        assert!(run_command("open-panel", &ctx).unwrap());
        assert!(matches!(take_ui_requests()[0], UiRequest::OpenPanel { .. }));

        // reload：面板层还挂在 compositor 上 → 必须入队 ClosePanel 供 apply_ui_requests 移除
        reload_all().unwrap();
        let reqs = take_ui_requests();
        assert!(matches!(&reqs[0], UiRequest::ClosePanel { .. }), "layer removed after reload");
        // LAST_PANEL_ID 已清空 → :panel-close 不再误报有面板
        assert!(close_last_panel().is_err());
    }

    #[test]
    fn selection_and_cursor() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("selcmd", (ctx) => {
            helix.set_cursor(2, 3);
            helix.set_selection(0, 1, 0, 5);
            helix.echo("sel:" + ctx.selection.anchor.row + "," + ctx.selection.anchor.col + "-" + ctx.selection.head.row + "," + ctx.selection.head.col);
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext {
            path: None,
            text: "abc\ndef\nghi".into(),
            cursor: (0, 0),
            selection: ((0, 1), (0, 5)),
        };
        assert!(run_command("selcmd", &ctx).unwrap());
        let reqs = take_cursor_requests();
        assert_eq!(
            reqs,
            vec![
                CursorRequest::SetCursor { row: 2, col: 3 },
                CursorRequest::SetSelection { anchor: (0, 1), head: (0, 5) },
            ]
        );
        assert_eq!(take_messages(), vec!["sel:0,1-0,5"]);

        // 类型错误 → 命令失败（run_command 返回 Err），请求队列被清空
        load_script(r#"helix.register_command("badsel", (ctx) => { helix.set_cursor("x", 0); });"#).unwrap();
        assert!(run_command("badsel", &ctx).is_err());
        assert!(take_cursor_requests().is_empty());
    }

    #[test]
    fn shell_run() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        // 成功路径
        load_script(r#"helix.register_command("r1", () => { helix.echo(helix.run("echo hi")); });"#).unwrap();
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command("r1", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["hi\n"]);

        // 非零退出码 → 错误
        load_script(r#"helix.register_command("r2", () => { helix.run("echo boom >&2; exit 3"); });"#).unwrap();
        let err = run_command("r2", &ctx).unwrap_err().to_string();
        assert!(err.contains("command failed"), "err: {err}");
        assert!(err.contains("boom"), "stderr should be included: {err}");

        // 类型错误
        load_script(r#"helix.register_command("r3", () => { helix.run(42); });"#).unwrap();
        assert!(run_command("r3", &ctx).is_err());

        // 截断：输出超限 → 返回长度 ≤ 65536
        load_script(
            r#"
        helix.register_command("r4", () => {
            const out = helix.run("head -c 100000 /dev/zero | tr '\\0' 'x'");
            helix.echo("len:" + out.length + " tail:" + out.slice(-11));
        });
        "#,
        )
        .unwrap();
        assert!(run_command("r4", &ctx).unwrap());
        let msg = take_messages();
        assert!(msg[0].contains("tail:(truncated)"), "marker expected: {:?}", msg[0]);
        let len: usize = msg[0].strip_prefix("len:").unwrap().split(" tail:").next().unwrap().parse().unwrap();
        assert!(len <= 65536 + "(truncated)".len(), "truncated output, len={len}");
    }

    /// 轮询 drain_term_events 直到谓词命中或超时（async 测试需要）。
    /// 累积自调用以来的全部事件返回：进程事件是 Chunk(s)→Exit 的顺序流，
    /// 命中谓词的那次 drain 之前可能已有事件被取走，须一并保留按序 resolve。
    fn wait_for_term_event(pred: impl Fn(&TermEvent) -> bool) -> Vec<TermEvent> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut all = Vec::new();
        loop {
            all.extend(drain_term_events());
            if all.iter().any(&pred) {
                return all;
            }
            if std::time::Instant::now() > deadline {
                panic!("timed out waiting for term event");
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// 轮询 drain_async_events 直到谓词命中或超时，事件累积进调用方传入的 vec
    /// （跨调用共享累积：异步 fs 四个操作并发发送，后几次 wait 必须能看到先前已 drain 的事件）。
    fn wait_for_async(all: &mut Vec<AsyncEvent>, pred: impl Fn(&AsyncEvent) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            all.extend(drain_async_events());
            if all.iter().any(&pred) {
                return;
            }
            if std::time::Instant::now() > deadline {
                panic!("timed out waiting for async event");
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    #[test]
    fn async_run_and_spawn() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();

        // run_async：echo → Exit 事件携带 stdout → resolve 触发回调
        load_script(
            r#"
        helix.run_async("echo async-hello", (err, out) => {
            helix.echo("cb:" + (err ?? "ok") + ":" + (out ?? "").trim());
        });
        "#,
        )
        .unwrap();
        let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
        let TermEvent::Exit(id, code, stdout) = &events[0] else { unreachable!() };
        assert_eq!(*code, 0);
        let stdout = stdout.clone().unwrap_or_default();
        resolve_term_event(*id, TermEvent::Exit(*id, *code, Some(stdout))).unwrap();
        assert_eq!(take_messages(), vec!["cb:ok:async-hello"]);

        // spawn 流式：cat 回显
        load_script(
            r#"
        helix.register_command("sp", () => {
            const id = helix.spawn({ cmd: "cat", onChunk: (c) => helix.echo("chunk:" + c), onExit: (code) => helix.echo("exit:" + code) });
            helix.term_write(id, "hello-term\n");
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command("sp", &ctx).unwrap());
        let events = wait_for_term_event(|e| matches!(e, TermEvent::Chunk(_, c) if c.contains("hello-term")));
        let ev = events.iter().find(|e| matches!(e, TermEvent::Chunk(_, c) if c.contains("hello-term"))).expect("chunk event");
        let TermEvent::Chunk(id, chunk) = ev else { unreachable!() };
        resolve_term_event(*id, TermEvent::Chunk(*id, chunk.clone())).unwrap();
        assert!(take_messages().contains(&format!("chunk:{chunk}")));

        // term_kill：sleep 100 → kill → Exit 快到达
        load_script(
            r#"
        helix.register_command("kp", () => {
            const id = helix.spawn({ cmd: "sleep 100", onChunk: () => {}, onExit: (code) => helix.echo("killed:" + code) });
            helix.term_kill(id);
        });
        "#,
        )
        .unwrap();
        assert!(run_command("kp", &ctx).unwrap());
        let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
        let TermEvent::Exit(id, code, _) = &events[0] else { unreachable!() };
        assert!(*code != 0, "killed process should have non-zero exit");
        resolve_term_event(*id, TermEvent::Exit(*id, *code, None)).unwrap();
        assert!(take_messages()[0].starts_with("killed:"));

        // 类型校验：run_async/spawn 参数错误在 load 时即报错
        assert!(load_script(r#"helix.run_async(42, () => {});"#).is_err());
        assert!(load_script(r#"helix.run_async("x", 42);"#).is_err());
        assert!(load_script(r#"helix.spawn({ cmd: "x" });"#).is_err()); // 缺 onChunk
        // term_write 未知 id 在 load 时不会执行（命令体），须放进命令里跑
        load_script(r#"helix.register_command("badid", () => { helix.term_write(999, "x"); });"#).unwrap();
        assert!(run_command("badid", &ctx).is_err());
    }

    #[test]
    fn async_utf8_across_chunks() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();

        // run_async 聚合：19999 字节 CJK 输出（"中文\n"×2857，7 字节/行）跨多个 4096 块，
        // 每块边界都可能切开 3 字节字符——修复前逐块 from_utf8_lossy 会产出 U+FFFD。
        // head -c 19999 恰好截在行边界（19999 = 7×2857），整流是合法 UTF-8。
        load_script(
            r#"
        helix.run_async("yes 中文 | head -c 19999", (err, out) => {
            helix.echo("agg:" + (err === null) + ":" + out.length);
        });
        "#,
        )
        .unwrap();
        // 只等聚合模式的 Exit（携带 Some(stdout)），遗漏的遗留 Exit 一并 resolve 清理
        let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, Some(_))));
        let ev = events
            .iter()
            .find(|e| matches!(e, TermEvent::Exit(_, _, Some(_))))
            .expect("run_async exit");
        let TermEvent::Exit(_, code, stdout) = ev else { unreachable!() };
        assert_eq!(*code, 0);
        let out = stdout.clone().unwrap_or_default();
        assert_eq!(out.len(), 19999, "aggregated bytes intact across chunks");
        assert!(!out.contains('\u{FFFD}'), "no replacement chars in aggregated output");
        assert_eq!(out.matches("中文").count(), 2857, "CJK lines preserved (7 bytes/line)");
        for ev in &events {
            if let TermEvent::Exit(id, code, stdout) = ev {
                resolve_term_event(*id, TermEvent::Exit(*id, *code, stdout.clone())).unwrap();
            }
        }
        let msg = take_messages();
        let m = msg.iter().find(|m| m.starts_with("agg:")).expect("run_async echo");
        // JS 收到完整输出：19999 字节 = 2857 行 × 3 个 BMP 字符 = 8571 个 UTF-16 单元
        assert_eq!(m.as_str(), "agg:true:8571");

        // spawn 流式：同一输出经 onChunk 增量解码拼接，块边界不得产生 U+FFFD
        load_script(
            r#"
        helix.register_command("spcjk", () => {
            const parts = [];
            const id = helix.spawn({
                cmd: "yes 中文 | head -c 19999",
                onChunk: (c) => parts.push(c),
                onExit: (code) => helix.echo("spawn:" + code + ":" + parts.length + ":" + parts.join("")),
            });
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command("spcjk", &ctx).unwrap());
        let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
        // 按序 resolve：先 Chunk 后 Exit，JS 的 parts 才能拼全
        for ev in &events {
            match ev {
                TermEvent::Chunk(id, chunk) => {
                    resolve_term_event(*id, TermEvent::Chunk(*id, chunk.clone())).unwrap();
                }
                TermEvent::Exit(id, code, stdout) => {
                    resolve_term_event(*id, TermEvent::Exit(*id, *code, stdout.clone())).unwrap();
                }
            }
        }
        let msg = take_messages();
        let m = msg.iter().find(|m| m.starts_with("spawn:0:")).expect("spawn echo");
        let mut it = m.splitn(4, ':');
        assert_eq!(it.next(), Some("spawn"));
        assert_eq!(it.next(), Some("0"));
        let chunks: usize = it.next().unwrap().parse().expect("chunk count");
        assert!(chunks >= 4, "streaming should split into multiple chunks, got {chunks}");
        let joined = it.next().unwrap();
        assert_eq!(joined.len(), 19999, "streamed bytes intact across chunks");
        assert!(!joined.contains('\u{FFFD}'), "no replacement chars in streamed output");
        assert_eq!(joined.matches("中文").count(), 2857, "CJK lines preserved in streamed output");
    }

    /// 任务简报验证测试：四个异步 fs API（read/write/stat/glob）回调 → echo；
    /// 错误路径 err 非空；参数类型校验。
    /// 注（相对简报的测试侧调整）：wait_for_async 把事件累积进共享 vec（四个 worker
    /// 并发发送，四次顺序 wait 需共享累积）；简报注释 "resolve 全部" 落实为逐事件 resolve。
    #[test]
    fn async_fs() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        let dir = std::env::temp_dir().join(format!("helix-js-fs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "hello fs").unwrap();
        std::fs::write(dir.join("b.js"), "x").unwrap();

        load_script(&format!(r#"
        helix.register_command("fsd", () => {{
            helix.read_file_async("{dir}/a.txt", (err, content) => {{
                helix.echo("read:" + (err ?? "") + ":" + (content ?? ""));
            }});
            helix.write_file_async("{dir}/out.txt", "written", (err) => {{
                helix.echo("write:" + (err ?? "ok"));
            }});
            helix.stat_async("{dir}/a.txt", (err, st) => {{
                helix.echo("stat:" + st.size + ":" + st.is_dir);
            }});
            helix.glob_async("{dir}/*.js", (err, paths) => {{
                helix.echo("glob:" + paths.length);
            }});
        }});
    "#, dir = dir.display())).unwrap();

        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command("fsd", &ctx).unwrap());
        // 轮询 drain_async_events 直到四个回调都到（wait_for_async 辅助，仿 wait_for_term_event）
        let mut events = Vec::new();
        wait_for_async(&mut events, |e| matches!(e, AsyncEvent::FsRead(_, _)));
        wait_for_async(&mut events, |e| matches!(e, AsyncEvent::FsWrite(_, _)));
        wait_for_async(&mut events, |e| matches!(e, AsyncEvent::FsStat(_, _)));
        wait_for_async(&mut events, |e| matches!(e, AsyncEvent::FsGlob(_, _)));
        // resolve 全部（事件里带 id）→ 断言回调 echo
        for ev in events {
            let id = match &ev {
                AsyncEvent::FsRead(id, _) | AsyncEvent::FsWrite(id, _) | AsyncEvent::FsStat(id, _) | AsyncEvent::FsGlob(id, _) => *id,
            };
            resolve_async_event(id, ev).unwrap();
        }
        let msgs = take_messages();
        assert!(msgs.iter().any(|m| m == "read::hello fs"), "{msgs:?}");
        assert!(msgs.iter().any(|m| m == "write:ok"), "{msgs:?}");
        assert!(msgs.iter().any(|m| m.starts_with("stat:8:false")), "{msgs:?}");
        assert!(msgs.iter().any(|m| m == "glob:1"), "{msgs:?}");
        assert_eq!(std::fs::read_to_string(dir.join("out.txt")).unwrap(), "written");

        // review: ** 中缀（`dir/**/*.js`）——嵌套目录也要命中（独立回合：wait_for_async 为
        // any 语义，同一事件类型不能连续 wait 两次）
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/deep.js"), "d").unwrap();
        load_script(&format!(r#"
        helix.register_command("fsd2", () => {{
            helix.glob_async("{dir}/**/*.js", (err, paths) => {{
                helix.echo("glob2:" + (err ?? "") + ":" + paths.length);
            }});
        }});
    "#, dir = dir.display())).unwrap();
        assert!(run_command("fsd2", &ctx).unwrap());
        let mut g2 = Vec::new();
        wait_for_async(&mut g2, |e| matches!(e, AsyncEvent::FsGlob(_, _)));
        for ev in g2 {
            let id = match &ev {
                AsyncEvent::FsRead(id, _) | AsyncEvent::FsWrite(id, _) | AsyncEvent::FsStat(id, _) | AsyncEvent::FsGlob(id, _) => *id,
            };
            resolve_async_event(id, ev).unwrap();
        }
        let msgs2 = take_messages();
        assert!(msgs2.iter().any(|m| m == "glob2::2"), "** 应命中根目录+嵌套: {msgs2:?}");

        // 错误路径：读不存在 → err 非空
        load_script(&format!(r#"
        helix.register_command("fsbad", () => {{
            helix.read_file_async("{dir}/nope.txt", (err, content) => {{
                helix.echo("bad:" + (err !== null ? "err" : "noerr"));
            }});
        }});
    "#, dir = dir.display())).unwrap();
        assert!(run_command("fsbad", &ctx).unwrap());
        let mut bad = Vec::new();
        wait_for_async(&mut bad, |e| matches!(e, AsyncEvent::FsRead(_, _)));
        // resolve → 断言
        for ev in bad {
            let id = match &ev {
                AsyncEvent::FsRead(id, _) | AsyncEvent::FsWrite(id, _) | AsyncEvent::FsStat(id, _) | AsyncEvent::FsGlob(id, _) => *id,
            };
            resolve_async_event(id, ev).unwrap();
        }
        assert!(take_messages().iter().any(|m| m == "bad:err"));

        // 类型校验
        assert!(load_script(r#"helix.read_file_async(42, () => {});"#).is_err());
        assert!(load_script(r#"helix.read_file_async("x", 42);"#).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// review 回归：裸模式 `*.js`（基目录 = `.`，cwd 内匹配）——验证字面前缀基目录 +
    /// 前导 `./` 归一化。chdir 受 TEST_LOCK 保护（同进程单测串行，本 crate 全部测试取锁）。
    #[test]
    fn glob_bare_pattern() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        let dir = std::env::temp_dir().join(format!("helix-js-glob-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("x.js"), "x").unwrap();
        std::fs::write(dir.join("y.txt"), "y").unwrap();
        std::fs::write(dir.join("sub/z.js"), "z").unwrap();
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();
        let bare = glob_matches("*.js");
        let dbl = glob_matches("**/*.js");
        let explicit = glob_matches("./*.js"); // ./ 归一化与裸模式一致
        std::env::set_current_dir(cwd).unwrap();
        assert_eq!(bare.unwrap(), vec!["x.js"], "裸 * 不跨目录分隔符");
        assert_eq!(dbl.unwrap(), vec!["sub/z.js", "x.js"], "** 跨目录（含零层）");
        assert_eq!(explicit.unwrap(), vec!["x.js"], "前导 ./ 归一化");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 临时任务简报的验证测试。wait 条件用 Exit（chunk 是它的先导），
    /// 返回的全部事件按序 resolve（先 Chunk 后 Exit），保证通道不残留。
    /// 注意：编译报错调整——简报原文 `events[0]` 按值取会 move，改为 `&events[0]`；
    /// `assert!(true, ...)` 触发 clippy::assertions_on_constants，删除。
    #[test]
    fn node_events_and_focusables() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
            helix.open_popup({
                render: (focus) => helix.el("col", [
                    helix.el("button", "run", { id: "btn1", onPress: () => helix.echo("pressed"), style: focus === "btn1" ? "error" : null }),
                    helix.el("input", { id: "in1", value: "abc", onKey: (k) => helix.echo("key:" + k) }),
                ]),
            });
            "#,
        )
        .unwrap();
        let id = match &take_ui_requests()[0] { UiRequest::OpenPopup { id, .. } => *id, other => panic!("expected OpenPopup, got {other:?}") };
        // render 传 focus → JS render(focus, ctx) 收到（focus 样式分支由 JS 处理，这里验证渲染不崩 + focusables 收集）
        let content = render_popup(id, 60, 20, Some("btn1")).unwrap();
        let mut focusables = Vec::new();
        if let Content::Tree(node) = &content {
            focusable_node_ids(node, &mut focusables);
        }
        assert_eq!(focusables, vec!["btn1".to_string(), "in1".to_string()]);
        // 事件分发：button onPress
        assert!(dispatch_node_event(id, "btn1", None).is_ok());
        // input onKey
        assert!(dispatch_node_event(id, "in1", Some("a")).is_ok());
        let msgs = take_messages();
        assert!(msgs.contains(&"pressed".to_string()), "{msgs:?}");
        assert!(msgs.contains(&"key:a".to_string()), "{msgs:?}");
        // 未知节点 → Ok（无处理器）
        assert!(dispatch_node_event(id, "nope", None).is_ok());
        close_popup(id).unwrap();
    }

    #[test]
    fn terminal_modes_api() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
            helix.register_command("tm", () => {
                const id = helix.open_terminal({ cmd: "cat", side: "right", size: 40 });
                helix.set_terminal_mode(id, "fullscreen");
                helix.set_terminal_mode(id, "minimized");
                helix.term_clear(id);
                helix.resize_term(id, 60);
            });
            "#,
        )
        .unwrap();
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command("tm", &ctx).unwrap());
        let reqs = take_ui_requests();
        assert!(matches!(&reqs[0], UiRequest::OpenTerminal { .. }));
        assert!(matches!(&reqs[1], UiRequest::TermMode { mode, .. } if mode == "fullscreen"));
        assert!(matches!(&reqs[2], UiRequest::TermMode { mode, .. } if mode == "minimized"));
        assert!(matches!(&reqs[3], UiRequest::TermClear { .. }));
        assert!(matches!(&reqs[4], UiRequest::TermResize { size, .. } if *size == 60));
        // 非法 mode → 命令报错
        load_script(r#"helix.register_command("tm-bad2", () => { helix.set_terminal_mode(1, "sideways"); });"#).unwrap();
        assert!(run_command("tm-bad2", &ctx).is_err());
        // 类型校验（load 时即报错）
        assert!(load_script(r#"helix.term_clear("x");"#).is_err());
        assert!(load_script(r#"helix.resize_term(1, "wide");"#).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn pty_spawn() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("pty-tty", () => {
            const id = helix.spawn({ pty: true, cmd: "tty", onChunk: (c) => helix.echo("out:" + c.trim()), onExit: (code) => helix.echo("exit:" + code) });
        });
        helix.register_command("pty-size", () => {
            const id = helix.spawn({ pty: true, cmd: "stty size", onChunk: (c) => helix.echo("size:" + c.trim()), onExit: (code) => helix.echo("sizeexit:" + code) });
        });
        helix.register_command("pty-cat", () => {
            const id = helix.spawn({ pty: true, cmd: "cat", onChunk: (c) => { helix.echo("pty:" + c.trim()); helix.term_kill(id); }, onExit: (code) => helix.echo("ptyexit:" + code) });
            helix.term_write(id, "hello-pty\n"); // raw mode 下 \u{4} 不是 EOF——echo 到达后 kill
        });
        helix.register_command("pty-badresize", () => { helix.term_resize(999, 1, 1); });
        "#,
        )
        .unwrap();
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };

        // tty：stdin 是 pty → 输出 /dev/pts/N（CRLF 行尾，用 contains 断言）
        assert!(run_command("pty-tty", &ctx).unwrap());
        let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
        for ev in &events {
            match ev {
                TermEvent::Chunk(id, c) => resolve_term_event(*id, TermEvent::Chunk(*id, c.clone())).unwrap(),
                TermEvent::Exit(id, code, stdout) => {
                    resolve_term_event(*id, TermEvent::Exit(*id, *code, stdout.clone())).unwrap()
                }
            }
        }
        assert!(
            take_messages().iter().any(|m| m.contains("/dev/pts/")),
            "tty command sees a pty"
        );

        // stty size：默认 winsize 24×80（输出顺序 rows cols）
        assert!(run_command("pty-size", &ctx).unwrap());
        let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
        for ev in &events {
            match ev {
                TermEvent::Chunk(id, c) => resolve_term_event(*id, TermEvent::Chunk(*id, c.clone())).unwrap(),
                TermEvent::Exit(id, code, stdout) => {
                    resolve_term_event(*id, TermEvent::Exit(*id, *code, stdout.clone())).unwrap()
                }
            }
        }
        assert!(
            take_messages().iter().any(|m| m.contains("24 80")),
            "default winsize 24x80"
        );

        // 写 master → 子进程 stdin：cat 回显；raw mode 下 \u{4} 非 EOF——onChunk 里 kill。
        // 轮询 drain + resolve（让 onChunk 的 kill 生效）直到 Exit。
        assert!(run_command("pty-cat", &ctx).unwrap());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut got_exit = false;
        while !got_exit && std::time::Instant::now() < deadline {
            for e in drain_term_events() {
                let is_exit = matches!(e, TermEvent::Exit(_, _, _));
                let id = match &e { TermEvent::Chunk(id, _) | TermEvent::Exit(id, _, _) => *id };
                let _ = resolve_term_event(id, e);
                if is_exit { got_exit = true; }
            }
            if !got_exit { std::thread::sleep(std::time::Duration::from_millis(20)); }
        }
        assert!(got_exit, "cat should exit after kill");
        let msgs = take_messages();
        assert!(
            msgs.iter().any(|m| m.contains("hello-pty")),
            "cat echo missing: {msgs:?}"
        );
        assert!(
            msgs.iter().any(|m| m.contains("ptyexit:-1")),
            "kill exits cat: {msgs:?}"
        );

        // 校验：pty 非布尔 / resize 未知 id → 报错
        assert!(load_script(r#"helix.spawn({ pty: "yes", cmd: "tty", onChunk: () => {} });"#).is_err());
        assert!(run_command("pty-badresize", &ctx).is_err());
    }

    /// spawn 后立即 term_resize → 子进程 stty size 读到新值。
    /// 实现用 master fd 直连 ioctl（spawn 返回时已注册），无消息时序问题。
    #[test]
    #[cfg(unix)]
    fn pty_resize() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.register_command("pty-resize", () => {
            const id = helix.spawn({ pty: true, cmd: "stty size", onChunk: (c) => helix.echo("size:" + c.trim()), onExit: (code) => helix.echo("resizeexit:" + code) });
            helix.term_resize(id, 40, 100);
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command("pty-resize", &ctx).unwrap());
        let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
        for ev in &events {
            match ev {
                TermEvent::Chunk(id, c) => resolve_term_event(*id, TermEvent::Chunk(*id, c.clone())).unwrap(),
                TermEvent::Exit(id, code, stdout) => {
                    resolve_term_event(*id, TermEvent::Exit(*id, *code, stdout.clone())).unwrap()
                }
            }
        }
        let msgs = take_messages();
        // stty size 输出顺序是 rows cols（简报写 "100 40"，实为行列反了）：
        // term_resize(40, 100) → "40 100"，以实际输出为准
        assert!(
            msgs.iter().any(|m| m.contains("40 100")),
            "resize applied before stty runs: {msgs:?}"
        );
    }

    #[test]
    fn plugin_reload_top_level_const() {
        // 顶层 const/let 脚本 reload 必须成功：IIFE 包裹下重跑获得全新词法作用域
        // （无 IIFE 时全局词法环境重复声明会抛 SyntaxError: duplicate lexical declaration）
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script_named(
            "c.js",
            r#"const X = 1; helix.register_command("ccmd", () => helix.echo("ok"));"#,
        )
        .unwrap();

        reload_all().unwrap();
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command("ccmd", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["ok"]);
    }

    /// 方案 2 依赖清单：helix.plugin deps 自动拓扑加载（先依赖后自身）、
    /// 已加载去重、循环依赖报错。不碰 set_plugins_dir（OnceLock 全局，污染后续测试），
    /// 全用绝对路径。
    #[test]
    fn plugin_deps_auto_load() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        let dir = tempfile::tempdir().unwrap();
        // ponytail: 泄漏 tempdir（同文件其他测试同款）——线程池复用线程，
        // 目录被删会误伤后续 load；泄漏几个 /tmp 小文件换确定性。
        let d = dir.path().to_path_buf();
        std::mem::forget(dir);
        std::fs::create_dir_all(d.join("features")).unwrap();
        std::fs::create_dir_all(d.join("lib")).unwrap();
        std::fs::write(
            d.join("lib/icons.js"),
            r#"helix.plugin("icons", { deps: [] }); helix.export({ src: "icons" });"#,
        )
        .unwrap();
        std::fs::write(
            d.join("features/feat.js"),
            format!(
                r#"helix.plugin("feat", {{ deps: [{icons:?}] }}); helix.export({{ src: "feat" }});"#,
                icons = d.join("lib/icons.js").to_string_lossy()
            ),
        )
        .unwrap();
        let icons_abs = d.join("lib/icons.js").to_string_lossy().into_owned();
        let feat_abs = d.join("features/feat.js").to_string_lossy().into_owned();
        let script = format!(
            r#"
        helix.register_command("deps-run", () => {{
            const feat = helix.load({feat:?});
            const icons = helix.load({icons:?});
            helix.echo("feat:" + feat.src + " icons:" + icons.src + " same:" + (feat === helix.load({feat:?})));
        }});
        "#,
            feat = feat_abs,
            icons = icons_abs,
        );
        load_script_named("driver.js", &script).unwrap();
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command("deps-run", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["feat:feat icons:icons same:true"]);

        // 循环依赖：a deps b，b deps a → 报错
        let a_abs = d.join("a.js").to_string_lossy().into_owned();
        let b_abs = d.join("b.js").to_string_lossy().into_owned();
        let a_script = format!(r#"helix.plugin("a", {{ deps: [{b:?}] }});"#, b = b_abs);
        let b_script = format!(r#"helix.plugin("b", {{ deps: [{a:?}] }});"#, a = a_abs);
        std::fs::write(d.join("a.js"), a_script).unwrap();
        std::fs::write(d.join("b.js"), b_script).unwrap();
        let cyc_script = format!(
            r#"
        helix.register_command("cyc-run", () => {{
            try {{
                helix.load({a:?});
                helix.echo("no-cycle");
            }} catch (e) {{
                helix.echo("cycle:" + String(e));
            }}
        }});
        "#,
            a = a_abs,
        );
        load_script_named("cyc.js", &cyc_script).unwrap();
        assert!(run_command("cyc-run", &ctx).unwrap());
        assert!(
            take_messages()[0].contains("circular dependency"),
            "循环依赖应报错"
        );
    }

    /// 统一入口：load/export 往返 + 缓存、lazy 桩、run_command 带 ctx、未知文件报错。
    /// set_plugins_dir 是进程全局——测试用临时目录隔离。
    // ponytail: tempdir 不 drop（std::mem::forget）——线程池复用线程，后续 reload 测试
    // 会在本线程重跑 LOADED_SCRIPTS（含 init.js→load("exp.js")），目录被删会误伤；
    // 泄漏几个 /tmp 小文件换确定性。
    #[test]
    fn entry_load_export_lazy() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        let dir = tempfile::tempdir().unwrap();
        set_plugins_dir(dir.path().to_path_buf());

        // 导出 + 加载往返
        std::fs::write(dir.path().join("exp.js"), r#"helix.export({ a: 1, b: "x" });"#).unwrap();
        load_script_named("init.js", r#"helix.load("exp.js");"#).unwrap();
        // init.js 的 load 本身无法断言返回值——直接测 js_load 路径：
        // 用 helix.run_command 间接：注册命令调用 load 并把结果 echo 出来
        load_script_named(
            "driver.js",
            r#"
        helix.register_command("load-exp", () => {
            const mod = helix.load("exp.js");
            helix.echo("a:" + mod.a + " b:" + mod.b);
        });
        helix.register_command("load-cached", () => {
            const m1 = helix.load("exp.js");
            const m2 = helix.load("exp.js");
            helix.echo("same:" + (m1 === m2));
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command("load-exp", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["a:1 b:x"]);
        assert!(run_command("load-cached", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["same:true"]);

        // lazy：桩首次调用时加载 + 转执行
        std::fs::write(
            dir.path().join("lazy.js"),
            r#"helix.register_command("lazy-cmd", () => { helix.echo("lazy-ran"); });"#,
        )
        .unwrap();
        load_script_named("lazy-driver.js", r#"helix.lazy("lazy.js", "lazy-cmd");"#).unwrap();
        assert!(run_command("lazy-cmd", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["lazy-ran"]);

        // run_command 带 ctx：命令读 ctx.cursor
        load_script_named(
            "rc.js",
            r#"
        helix.register_command("where", (c) => { helix.echo("at:" + c.cursor.row + "," + c.cursor.col); });
        "#,
        )
        .unwrap();
        load_script_named(
            "rc-driver.js",
            r#"helix.register_command("call-where", () => { helix.run_command("where", { cursor: { row: 3, col: 7 } }); });"#,
        )
        .unwrap();
        assert!(run_command("call-where", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["at:3,7"]);

        // 校验：未知文件 → 抛错
        load_script_named("bad-driver.js", r#"helix.register_command("bad-load", () => { helix.load("nope.js"); });"#).unwrap();
        assert!(run_command("bad-load", &ctx).is_err());

        std::mem::forget(dir);
    }

    /// 简报验证测试：el 构造节点、render 返回树 → Content::Tree、非法节点类型 → Err
    #[test]
    fn component_nodes() {
        let _guard = TEST_LOCK.lock().unwrap();
        init();
        load_script(
            r#"
        helix.open_popup({
            render: () => helix.el("col", [
                helix.el("text", "title", { style: "error" }),
                helix.el("row", [
                    helix.el("text", "left"),
                    helix.el("text", "right", { width: 5 }),
                ], { gap: 1 }),
                helix.el("scroll", [helix.el("text", "s1"), helix.el("text", "s2")], { height: 1 }),
            ]),
        });
        helix.register_command("tree", (ctx) => {
            const n = helix.el("text", "hello", { width: 10 });
            helix.echo("type:" + n.type + " text:" + n.text + " w:" + n.width);
        });
        "#,
        )
        .unwrap();
        let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
        assert!(run_command("tree", &ctx).unwrap());
        assert_eq!(take_messages(), vec!["type:text text:hello w:10"]);

        let reqs = take_ui_requests();
        let id = match &reqs[0] { UiRequest::OpenPopup { id, .. } => *id, _ => unreachable!("expected OpenPopup") };
        match render_popup(id, 40, 10, None).unwrap() {
            Content::Tree(root) => {
                assert!(matches!(&root, CompNode::Col { .. }));
                match &root {
                    CompNode::Col { children, .. } => {
                        assert_eq!(children.len(), 3);
                        assert!(matches!(&children[0], CompNode::Text { spans, .. } if spans.len() == 1 && spans[0].text == "title" && spans[0].style.as_deref() == Some("error")));
                        assert!(matches!(&children[1], CompNode::Row { .. }));
                        assert!(matches!(&children[2], CompNode::Scroll { .. }));
                    }
                    _ => panic!(),
                }
            }
            _ => panic!("expected tree"),
        }
        close_popup(id).unwrap();
        // 非法节点类型 → Err
        load_script(r#"helix.open_popup({ render: () => ({ type: "bogus" }) });"#).unwrap();
        let id = match take_ui_requests()[0] { UiRequest::OpenPopup { id, .. } => id, _ => unreachable!("expected OpenPopup") };
        assert!(render_popup(id, 40, 10, None).is_err());
        close_popup(id).unwrap();
    }
}
