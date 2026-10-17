//! `term_pty` 域测试。

use super::*;

/// v13 任务简报验证测试：read_dir 排序/is_dir、open_file、move_panel 入队 + 校验。
/// 简报原文断言 count:2，但设置创建 3 个条目（a.txt、b.js、sub/）→ 按实际调整为 count:3；
/// 排序断言 entries[0].name < entries[1].name 不受影响（a.txt < b.js）。
#[test]
fn term_wake_fires_on_event() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    assert!(run_command("wk", &ctx).unwrap());
    // 20s(同 wait_for_term_event 的理由:并行 CI 下 5s 会假红)
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !fired.load(std::sync::atomic::Ordering::SeqCst) && std::time::Instant::now() < deadline {
        // 消费事件（让 worker 继续/完成），wake 在 send 时触发
        let _ = drain_term_events();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        fired.load(std::sync::atomic::Ordering::SeqCst),
        "wake should fire on event send"
    );
}

#[test]
fn open_terminal_api() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    assert!(run_command("ot", &ctx).unwrap());
    assert!(take_messages()[0].starts_with("pid:"));
    let reqs = take_ui_requests();
    assert!(
        matches!(&reqs[0], UiRequest::OpenTerminal(OpenTerminalReq { side, size, .. }) if side == "right" && *size == 40)
    );
    assert!(matches!(&reqs[1], UiRequest::TermFeed(TermFeedReq { chunk, .. }) if chunk == "abc"));
    // 校验
    assert!(load_script(r#"helix.open_terminal({ cmd: "x", side: "top", size: 10 });"#).is_err());
    assert!(load_script(r#"helix.open_terminal({ cmd: "x", side: "right" });"#).is_err());
    // 缺 size
}

/// 交互 bash 启动不应报 "cannot set terminal process group"（缺 setsid/控制终端）。
/// 修复前：bash job control 初始化失败向 stderr 打印该错误。
#[test]
fn pty_bash_interactive_no_error() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    assert!(run_command("bi", &ctx).unwrap());
    let _ = take_messages();
    let pty_id = match &take_ui_requests()[0] {
        UiRequest::OpenTerminal(OpenTerminalReq { pty_id, .. }) => *pty_id,
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
            if let UiRequest::TermFeed(TermFeedReq { chunk, .. }) = req {
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
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    assert!(run_command("t3", &ctx).unwrap(), "第一次 open_terminal");
    let _ = take_messages();
    let pty_id = match &take_ui_requests()[0] {
        UiRequest::OpenTerminal(OpenTerminalReq { pty_id, .. }) => *pty_id,
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
    assert!(
        run_command("t3", &ctx).unwrap(),
        "kill 后再 open_terminal 应成功"
    );
    let _ = take_messages();
    let _ = take_ui_requests(); // 清空第二次 open 的请求，避免污染后续测试
}

#[test]
fn sidecar_apis() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    assert!(run_command("sc", &ctx).unwrap());
    let msgs = take_messages();
    assert!(msgs[0].starts_with("count:3 sorted:true"), "{msgs:?}");
    assert_eq!(msgs[1], "dirs:1:sub");
    let reqs = take_ui_requests();
    assert!(
        matches!(&reqs[0], UiRequest::OpenFile(OpenFileReq { path, row: None, col: None }) if path.ends_with("a.txt"))
    );
    assert!(
        matches!(&reqs[1], UiRequest::MovePanel(MovePanelReq { id: 7, side }) if side == "left")
    );

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
fn terminal_modes_api() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    assert!(run_command("tm", &ctx).unwrap());
    let reqs = take_ui_requests();
    assert!(matches!(
        &reqs[0],
        UiRequest::OpenTerminal(OpenTerminalReq { .. })
    ));
    assert!(
        matches!(&reqs[1], UiRequest::TermMode(TermModeReq { mode, .. }) if mode == "fullscreen")
    );
    assert!(
        matches!(&reqs[2], UiRequest::TermMode(TermModeReq { mode, .. }) if mode == "minimized")
    );
    assert!(matches!(
        &reqs[3],
        UiRequest::TermClear(TermClearReq { .. })
    ));
    assert!(matches!(&reqs[4], UiRequest::TermResize(TermResizeReq { size, .. }) if *size == 60));
    // 非法 mode → 命令报错
    load_script(
        r#"helix.register_command("tm-bad2", () => { helix.set_terminal_mode(1, "sideways"); });"#,
    )
    .unwrap();
    assert!(run_command("tm-bad2", &ctx).is_err());
    // 类型校验（load 时即报错）
    assert!(load_script(r#"helix.term_clear("x");"#).is_err());
    assert!(load_script(r#"helix.resize_term(1, "wide");"#).is_err());
}

#[test]
#[cfg(unix)]
fn pty_spawn() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };

    // tty：stdin 是 pty → 输出 /dev/pts/N（CRLF 行尾，用 contains 断言）
    assert!(run_command("pty-tty", &ctx).unwrap());
    let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
    for ev in &events {
        match ev {
            TermEvent::Chunk(id, c) => {
                resolve_term_event(*id, TermEvent::Chunk(*id, c.clone())).unwrap()
            }
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
            TermEvent::Chunk(id, c) => {
                resolve_term_event(*id, TermEvent::Chunk(*id, c.clone())).unwrap()
            }
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let mut got_exit = false;
    while !got_exit && std::time::Instant::now() < deadline {
        for e in drain_term_events() {
            let is_exit = matches!(e, TermEvent::Exit(_, _, _));
            let id = match &e {
                TermEvent::Chunk(id, _) | TermEvent::Exit(id, _, _) => *id,
            };
            let _ = resolve_term_event(id, e);
            if is_exit {
                got_exit = true;
            }
        }
        if !got_exit {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
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
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    assert!(run_command("pty-resize", &ctx).unwrap());
    let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
    for ev in &events {
        match ev {
            TermEvent::Chunk(id, c) => {
                resolve_term_event(*id, TermEvent::Chunk(*id, c.clone())).unwrap()
            }
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

/// 终端钩子:emit_hook 返回第一个非 undefined;无 handler → None
#[test]
fn term_hook_emit_and_term_key() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    // 无 handler:None
    assert_eq!(emit_hook("term-key", &[JsValue::from(1_i32)]), None);
    assert_eq!(emit_term_key(1, "esc", false, false, false), None);
    assert!(!emit_term_close(1, "esc"), "无 handler 不阻止");

    load_script(
        r#"
        helix.on("term-close", () => false);
        helix.on("term-key", (id, key) => {
            if (key.code === "esc") return "minimize";
            if (key.code === "x" && key.ctrl) return "close";
            return "pass";
        });
        "#,
    )
    .unwrap();
    use crate::commands::TermKeyDecision;
    // term-key 映射
    assert!(matches!(
        emit_term_key(1, "esc", false, false, false),
        Some(TermKeyDecision::Minimize)
    ));
    assert!(matches!(
        emit_term_key(1, "x", false, true, false),
        Some(TermKeyDecision::Close)
    ));
    assert!(matches!(
        emit_term_key(1, "a", false, false, false),
        Some(TermKeyDecision::Pass)
    ));
    // term-close:false → 阻止
    assert!(emit_term_close(7, "esc"), "handler 返回 false → 阻止关闭");
    // 通知型钩子(参数不经 boa 也可构造)
    emit_hook(
        "term-resize",
        &[
            JsValue::from(1_i32),
            JsValue::from(24_i32),
            JsValue::from(80_i32),
        ],
    );
}

/// term_state 持久化读写(临时 HOME 隔离)
#[test]
fn term_state_persist_roundtrip() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    let dir = tempfile::tempdir().unwrap();
    let old_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", dir.path());
    load_script(
        r#"
        globalThis.__st = helix.term_state(42, "cwd");   // 不存在 → undefined
        helix.term_state(42, "cwd", "/tmp/proj");
        globalThis.__st2 = helix.term_state(42, "cwd");  // 写后读回
        "#,
    )
    .unwrap();
    crate::state::with_engine(|engine| {
        let v1 = engine
            .global_object()
            .get(JsString::from("__st"), engine)
            .unwrap();
        assert!(v1.is_undefined(), "不存在 → undefined");
        let v2 = engine
            .global_object()
            .get(JsString::from("__st2"), engine)
            .unwrap();
        assert_eq!(
            v2.as_string().unwrap().to_std_string_escaped(),
            "/tmp/proj",
            "写后读回"
        );
    });
    if let Some(old) = old_home {
        std::env::set_var("HOME", old);
    }
    drop(dir);
}

/// term-exit 钩子:resolve_term_event(Exit) → emit_term_exit(消息)
#[test]
fn term_exit_hook_via_resolve() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(r#"helix.on("term-exit", (id, code) => helix.echo("exit:" + code));"#).unwrap();
    let id = crate::state::next_term_id();
    // 注册回调(否则 resolve 直接 return)
    crate::state::with_terms(|m| {
        m.insert(
            id,
            crate::types::TermCallbacks {
                on_chunk: JsValue::undefined(),
                on_exit: None,
            },
        )
    });
    crate::shell::resolve_term_event(id, crate::types::TermEvent::Exit(id, 3, None)).unwrap();
    let msgs = take_messages();
    assert!(
        msgs.iter().any(|m| m == "exit:3"),
        "term-exit 钩子触发: {msgs:?}"
    );
}
