//! `fs_async` 域测试。

use super::*;

/// 同步 `helix.read_file`:① 正常读到内容 ② 缺文件时**抛错且文案含路径**
/// (与 `read_dir` 同款约定)
#[test]
fn read_file_sync_roundtrip_and_error() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("x.txt");
    std::fs::write(&f, "hi-sync").unwrap();

    let _ = take_messages();
    load_script(&format!(
        r#"helix.echo(helix.read_file("{}"));"#,
        f.display()
    ))
    .unwrap();
    assert!(
        take_messages().iter().any(|m| m == "hi-sync"),
        "应读到文件内容"
    );

    let _ = take_messages();
    load_script(&format!(
        r#"try {{ helix.read_file("{}/nope.txt"); }} catch (e) {{ helix.echo(String(e).includes("nope.txt") ? "caught" : "no-path-in-msg"); }}"#,
        dir.path().display()
    ))
    .unwrap();
    assert!(
        take_messages().iter().any(|m| m == "caught"),
        "缺文件应抛错且文案含路径,实得 {:?}",
        take_messages()
    );
}

#[test]
fn echo_captures_message() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    load_script(r#"helix.echo("hello from js");"#).unwrap();
    assert_eq!(take_messages(), vec!["hello from js"]);
}

/// **C-2 检查点覆盖"读文件系统的同族入口"**:`stat_async` / `glob_async` / `read_tree` /
/// `watch` 都会把文件系统的信息(存在性、大小、mtime、目录内容、变更时机)泄漏给插件
/// ⇒ 同属 `fs_read`。**此前这四个漏在检查点之外**(实测:只声明 `process` 的插件能枚举目录),
/// 与 `open_terminal` 是同一类洞。
///
/// 两个方向都钉:未声明 ⇒ 被拒 + 记入待授权;声明了 ⇒ 放行。
#[test]
fn fs_read_covers_stat_glob_tree_and_watch() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    crate::permissions::reset_for_test();
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_path_buf();
    std::fs::write(d.join("secret.txt"), "s").unwrap();
    std::mem::forget(dir);
    let root = d.to_string_lossy().to_string();
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    let plugin_src = |perms: &str| {
        format!(
            r#"
            helix.plugin("fs-sib", {{ permissions: {{ {perms} }} }});
            helix.register_command("sib-glob", () => {{ helix.glob_async({root:?} + "/*"); }});
            helix.register_command("sib-stat", () => {{ helix.stat_async({root:?} + "/secret.txt"); }});
            helix.register_command("sib-tree", () => {{ helix.read_tree({root:?}); }});
            helix.register_command("sib-watch", () => {{ helix.watch({root:?}, () => {{}}); }});
            "#
        )
    };
    // ① 只声明 process(没有 fs_read)⇒ 四个入口**全部**被拒,并各自记入 (fs-sib, fs_read)
    load_script(&plugin_src("process: true")).unwrap();
    for cmd in ["sib-glob", "sib-stat", "sib-tree", "sib-watch"] {
        let err = crate::host::with_state_or_init(|st| run_command(st, cmd, &ctx))
            .expect_err(&format!("{cmd} 未声明 fs_read 时应被拒"))
            .to_string();
        assert!(
            err.contains("未声明能力") && err.contains("fs_read"),
            "{cmd}: {err}"
        );
    }
    assert!(
        pending_permissions()
            .iter()
            .any(|(p, c)| p == "fs-sib" && c == "fs_read"),
        "应记入待授权:{:?}",
        pending_permissions()
    );
    // ② 声明了 fs_read ⇒ 四个入口都能走到"返回值"这一步(promise / id)
    load_script(&plugin_src("fs_read: true")).unwrap();
    for cmd in ["sib-glob", "sib-stat", "sib-tree", "sib-watch"] {
        assert!(
            crate::host::with_state_or_init(|st| run_command(st, cmd, &ctx)).unwrap_or(false),
            "{cmd} 声明 fs_read 后应放行"
        );
    }
}

#[test]
fn shell_run() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    // 成功路径
    load_script(r#"helix.register_command("r1", () => { helix.echo(helix.run("echo hi")); });"#)
        .unwrap();
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    assert!(crate::host::with_state_or_init(|st| run_command(st, "r1", &ctx)).unwrap());
    assert_eq!(take_messages(), vec!["hi\n"]);

    // 非零退出码 → 错误
    load_script(r#"helix.register_command("r2", () => { helix.run("echo boom >&2; exit 3"); });"#)
        .unwrap();
    let err = crate::host::with_state_or_init(|st| run_command(st, "r2", &ctx))
        .unwrap_err()
        .to_string();
    assert!(err.contains("command failed"), "err: {err}");
    assert!(err.contains("boom"), "stderr should be included: {err}");

    // 类型错误
    load_script(r#"helix.register_command("r3", () => { helix.run(42); });"#).unwrap();
    assert!(crate::host::with_state_or_init(|st| run_command(st, "r3", &ctx)).is_err());

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
    assert!(crate::host::with_state_or_init(|st| run_command(st, "r4", &ctx)).unwrap());
    let msg = take_messages();
    assert!(
        msg[0].contains("tail:(truncated)"),
        "marker expected: {:?}",
        msg[0]
    );
    let len: usize = msg[0]
        .strip_prefix("len:")
        .unwrap()
        .split(" tail:")
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        len <= 65536 + "(truncated)".len(),
        "truncated output, len={len}"
    );
}

#[test]
fn async_run_and_spawn() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();

    // run_async:Promise 模式 → Exit 事件 → resolve → pump_jobs 执行 .then
    load_script(
        r#"
    helix.run_async("echo async-hello").then((out) => {
        helix.echo("cb:ok:" + out.trim());
    });
    "#,
    )
    .unwrap();
    let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
    let TermEvent::Exit(id, code, stdout) = &events[0] else {
        unreachable!()
    };
    assert_eq!(*code, 0);
    crate::host::with_state_or_init(|st| {
        resolve_term_event(st, *id, TermEvent::Exit(*id, *code, stdout.clone()))
    })
    .unwrap();
    pump_jobs().unwrap();
    assert_eq!(take_messages(), vec!["cb:ok:async-hello"]);

    // run_async 错误路径:非零退出 → reject → .catch
    load_script(
        r#"
    helix.run_async("echo boom >&2; exit 3").catch((e) => {
        helix.echo("raerr:" + e.message);
    });
    "#,
    )
    .unwrap();
    let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
    let TermEvent::Exit(id2, code2, _) = &events[0] else {
        unreachable!()
    };
    assert_ne!(*code2, 0);
    crate::host::with_state_or_init(|st| {
        resolve_term_event(st, *id2, TermEvent::Exit(*id2, *code2, None))
    })
    .unwrap();
    pump_jobs().unwrap();
    let msgs = take_messages();
    assert!(msgs[0].contains("raerr:"), "{msgs:?}");

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
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    assert!(crate::host::with_state_or_init(|st| run_command(st, "sp", &ctx)).unwrap());
    let events =
        wait_for_term_event(|e| matches!(e, TermEvent::Chunk(_, c) if c.contains("hello-term")));
    let ev = events
        .iter()
        .find(|e| matches!(e, TermEvent::Chunk(_, c) if c.contains("hello-term")))
        .expect("chunk event");
    let TermEvent::Chunk(id, chunk) = ev else {
        unreachable!()
    };
    crate::host::with_state_or_init(|st| {
        resolve_term_event(st, *id, TermEvent::Chunk(*id, chunk.clone()))
    })
    .unwrap();
    assert!(take_messages().contains(&format!("chunk:{chunk}")));

    // 流式 **stderr**：单线程 poll 必须把两条流都读出来（改前各有读线程，改后共用一个循环）
    load_script(
        r#"
    helix.register_command("se", () => {
        const id = helix.spawn({ cmd: "echo OUT; echo ERR >&2; echo LAST", onChunk: (c) => helix.echo("c:" + c.trim()), onExit: () => {} });
    });
    "#,
    )
    .unwrap();
    assert!(crate::host::with_state_or_init(|st| run_command(st, "se", &ctx)).unwrap());
    let events = wait_for_term_event(|e| matches!(e, TermEvent::Chunk(_, c) if c.contains("LAST")));
    let joined: String = events
        .iter()
        .filter_map(|e| match e {
            TermEvent::Chunk(_, c) => Some(c.clone()),
            _ => None,
        })
        .collect();
    assert!(joined.contains("OUT"), "stdout 流式输出: {joined}");
    assert!(joined.contains("ERR"), "stderr 流式输出: {joined}");
    for e in &events {
        if let TermEvent::Chunk(id, c) = e {
            crate::host::with_state_or_init(|st| {
                resolve_term_event(st, *id, TermEvent::Chunk(*id, c.clone()))
            })
            .unwrap();
        }
    }
    let _ = take_messages();

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
    assert!(crate::host::with_state_or_init(|st| run_command(st, "kp", &ctx)).unwrap());
    let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
    let TermEvent::Exit(id, code, _) = &events[0] else {
        unreachable!()
    };
    assert!(*code != 0, "killed process should have non-zero exit");
    crate::host::with_state_or_init(|st| {
        resolve_term_event(st, *id, TermEvent::Exit(*id, *code, None))
    })
    .unwrap();
    assert!(take_messages()[0].starts_with("killed:"));

    // 类型校验：run_async/spawn 参数错误在 load 时即报错
    // 注:原 helix.run_async("x", 42) 断言是回调校验报错,现 JS 多参天然容忍 → 删除
    assert!(load_script(r#"helix.run_async(42);"#).is_err());
    assert!(load_script(r#"helix.spawn({ cmd: "x" });"#).is_err()); // 缺 onChunk
                                                                    // term_write 未知 id 在 load 时不会执行（命令体），须放进命令里跑
    load_script(r#"helix.register_command("badid", () => { helix.term_write(999, "x"); });"#)
        .unwrap();
    assert!(crate::host::with_state_or_init(|st| run_command(st, "badid", &ctx)).is_err());
}

#[test]
fn async_utf8_across_chunks() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();

    // run_async 聚合：19999 字节 CJK 输出（"中文\n"×2857，7 字节/行）跨多个 4096 块，
    // 每块边界都可能切开 3 字节字符——修复前逐块 from_utf8_lossy 会产出 U+FFFD。
    // head -c 19999 恰好截在行边界（19999 = 7×2857），整流是合法 UTF-8。
    load_script(
        r#"
    helix.run_async("yes 中文 | head -c 19999").then((out) => {
        helix.echo("len:" + out.length + " tail:" + out.slice(-11));
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
    let TermEvent::Exit(_, code, stdout) = ev else {
        unreachable!()
    };
    assert_eq!(*code, 0);
    let out = stdout.clone().unwrap_or_default();
    assert_eq!(out.len(), 19999, "aggregated bytes intact across chunks");
    assert!(
        !out.contains('\u{FFFD}'),
        "no replacement chars in aggregated output"
    );
    assert_eq!(
        out.matches("中文").count(),
        2857,
        "CJK lines preserved (7 bytes/line)"
    );
    for ev in &events {
        if let TermEvent::Exit(id, code, stdout) = ev {
            crate::host::with_state_or_init(|st| {
                resolve_term_event(st, *id, TermEvent::Exit(*id, *code, stdout.clone()))
            })
            .unwrap();
        }
    }
    pump_jobs().unwrap();
    let msg = take_messages();
    let m = msg
        .iter()
        .find(|m| m.starts_with("len:"))
        .expect("run_async echo");
    // JS 收到完整输出：19999 字节 = 2857 行 × 3 个 BMP 字符 = 8571 个 UTF-16 单元
    assert!(m.starts_with("len:8571 tail:"), "full output length: {m}");
    assert!(m.contains("中文"), "tail should be CJK: {m}");

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
    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    assert!(crate::host::with_state_or_init(|st| run_command(st, "spcjk", &ctx)).unwrap());
    let events = wait_for_term_event(|e| matches!(e, TermEvent::Exit(_, _, _)));
    // 按序 resolve：先 Chunk 后 Exit，JS 的 parts 才能拼全
    for ev in &events {
        match ev {
            TermEvent::Chunk(id, chunk) => {
                crate::host::with_state_or_init(|st| {
                    resolve_term_event(st, *id, TermEvent::Chunk(*id, chunk.clone()))
                })
                .unwrap();
            }
            TermEvent::Exit(id, code, stdout) => {
                crate::host::with_state_or_init(|st| {
                    resolve_term_event(st, *id, TermEvent::Exit(*id, *code, stdout.clone()))
                })
                .unwrap();
            }
        }
    }
    let msg = take_messages();
    let m = msg
        .iter()
        .find(|m| m.starts_with("spawn:0:"))
        .expect("spawn echo");
    let mut it = m.splitn(4, ':');
    assert_eq!(it.next(), Some("spawn"));
    assert_eq!(it.next(), Some("0"));
    let chunks: usize = it.next().unwrap().parse().expect("chunk count");
    assert!(
        chunks >= 4,
        "streaming should split into multiple chunks, got {chunks}"
    );
    let joined = it.next().unwrap();
    assert_eq!(joined.len(), 19999, "streamed bytes intact across chunks");
    assert!(
        !joined.contains('\u{FFFD}'),
        "no replacement chars in streamed output"
    );
    assert_eq!(
        joined.matches("中文").count(),
        2857,
        "CJK lines preserved in streamed output"
    );
}

/// 任务简报验证测试：四个异步 fs API（read/write/stat/glob）Promise → echo；
/// 错误路径 reject（e.message）；参数类型校验。
/// 注（相对简报的测试侧调整）：wait_for_async 把事件累积进共享 vec（四个 worker
/// 并发发送，四次顺序 wait 需共享累积）；简报注释 "resolve 全部" 落实为逐事件 resolve。
#[test]
fn async_fs() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    let dir = std::env::temp_dir().join(format!("helix-js-fs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "hello fs").unwrap();
    std::fs::write(dir.join("b.js"), "x").unwrap();

    load_script(&format!(
        r#"
    helix.register_command("fsd", () => {{
        helix.read_file_async("{dir}/a.txt").then((content) => {{
            helix.echo("read::" + content);
        }});
        helix.write_file_async("{dir}/out.txt", "written").then(() => {{
            helix.echo("write:ok");
        }});
        helix.stat_async("{dir}/a.txt").then((st) => {{
            helix.echo("stat:" + st.size + ":" + st.is_dir);
        }});
        helix.glob_async("{dir}/*.js").then((paths) => {{
            helix.echo("glob:" + paths.length);
        }});
    }});
"#,
        dir = dir.display()
    ))
    .unwrap();

    let ctx = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    assert!(crate::host::with_state_or_init(|st| run_command(st, "fsd", &ctx)).unwrap());
    // 轮询 drain_async_events 直到四个回调都到（wait_for_async 辅助，仿 wait_for_term_event）
    let mut events = Vec::new();
    wait_for_async(&mut events, |e| matches!(e, AsyncEvent::FsRead(_, _)));
    wait_for_async(&mut events, |e| matches!(e, AsyncEvent::FsWrite(_, _)));
    wait_for_async(&mut events, |e| matches!(e, AsyncEvent::FsStat(_, _)));
    wait_for_async(&mut events, |e| matches!(e, AsyncEvent::FsGlob(_, _)));
    // resolve 全部（事件里带 id）→ 断言回调 echo
    for ev in events {
        let id = match &ev {
            AsyncEvent::FsRead(id, _)
            | AsyncEvent::FsWrite(id, _)
            | AsyncEvent::FsStat(id, _)
            | AsyncEvent::FsGlob(id, _)
            | AsyncEvent::FsTree(id, _) => *id,
        };
        crate::host::with_state_or_init(|st| resolve_async_event(st, id, ev).unwrap());
    }
    pump_jobs().unwrap();
    let msgs = take_messages();
    assert!(msgs.iter().any(|m| m == "read::hello fs"), "{msgs:?}");
    assert!(msgs.iter().any(|m| m == "write:ok"), "{msgs:?}");
    assert!(
        msgs.iter().any(|m| m.starts_with("stat:8:false")),
        "{msgs:?}"
    );
    assert!(msgs.iter().any(|m| m == "glob:1"), "{msgs:?}");
    assert_eq!(
        std::fs::read_to_string(dir.join("out.txt")).unwrap(),
        "written"
    );

    // review: ** 中缀（`dir/**/*.js`）——嵌套目录也要命中（独立回合：wait_for_async 为
    // any 语义，同一事件类型不能连续 wait 两次）
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub/deep.js"), "d").unwrap();
    load_script(&format!(
        r#"
    helix.register_command("fsd2", () => {{
        helix.glob_async("{dir}/**/*.js").then((paths) => {{
            helix.echo("glob2::" + paths.length);
        }});
    }});
"#,
        dir = dir.display()
    ))
    .unwrap();
    assert!(crate::host::with_state_or_init(|st| run_command(st, "fsd2", &ctx)).unwrap());
    let mut g2 = Vec::new();
    wait_for_async(&mut g2, |e| matches!(e, AsyncEvent::FsGlob(_, _)));
    for ev in g2 {
        let id = match &ev {
            AsyncEvent::FsRead(id, _)
            | AsyncEvent::FsWrite(id, _)
            | AsyncEvent::FsStat(id, _)
            | AsyncEvent::FsGlob(id, _)
            | AsyncEvent::FsTree(id, _) => *id,
        };
        crate::host::with_state_or_init(|st| resolve_async_event(st, id, ev).unwrap());
    }
    pump_jobs().unwrap();
    let msgs2 = take_messages();
    assert!(
        msgs2.iter().any(|m| m == "glob2::2"),
        "** 应命中根目录+嵌套: {msgs2:?}"
    );

    // 错误路径：读不存在 → err 非空
    load_script(&format!(
        r#"
    helix.register_command("fsbad", () => {{
        helix.read_file_async("{dir}/nope.txt").catch((e) => {{
            helix.echo("bad:" + (e !== null ? "err" : "noerr"));
        }});
    }});
"#,
        dir = dir.display()
    ))
    .unwrap();
    assert!(crate::host::with_state_or_init(|st| run_command(st, "fsbad", &ctx)).unwrap());
    let mut bad = Vec::new();
    wait_for_async(&mut bad, |e| matches!(e, AsyncEvent::FsRead(_, _)));
    // resolve → 断言
    for ev in bad {
        let id = match &ev {
            AsyncEvent::FsRead(id, _)
            | AsyncEvent::FsWrite(id, _)
            | AsyncEvent::FsStat(id, _)
            | AsyncEvent::FsGlob(id, _)
            | AsyncEvent::FsTree(id, _) => *id,
        };
        crate::host::with_state_or_init(|st| resolve_async_event(st, id, ev).unwrap());
    }
    pump_jobs().unwrap();
    assert!(take_messages().iter().any(|m| m == "bad:err"));

    // 类型校验：path 参数错误在 load 时即报错（回调校验已随 async_cb 删除，多参天然容忍）
    assert!(load_script(r#"helix.read_file_async(42);"#).is_err());

    let _ = std::fs::remove_dir_all(&dir);
}

/// review 回归：裸模式 `*.js`（基目录 = `.`，cwd 内匹配）——验证字面前缀基目录 +
/// 前导 `./` 归一化。chdir 受 TEST_LOCK 保护（同进程单测串行，本 crate 全部测试取锁）。
#[test]
fn glob_bare_pattern() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
    assert_eq!(
        dbl.unwrap(),
        vec!["sub/z.js", "x.js"],
        "** 跨目录（含零层）"
    );
    assert_eq!(explicit.unwrap(), vec!["x.js"], "前导 ./ 归一化");
    let _ = std::fs::remove_dir_all(&dir);
}
