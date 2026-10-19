//! `events_cwd` 域测试:`helix.cwd()` getter + `cwd-change` 事件。
//!
//! 背景:`:cd` 一直是**真** chdir(`helix_stdx::env::set_current_working_dir`),但
//! helix-js 侧此前**一次都没读过** cwd ⇒ 插件只能 `helix.run("pwd")` 问一次并缓存,
//! `:cd` 之后永远陈旧(内建 filetree 就是那个形状)。这两个入口补上后:
//! 取值**每次现取**(不会陈旧),变更**有事件**(缓存的插件能刷新)。

use super::*;
use std::path::{Path, PathBuf};

/// 把进程 cwd 换成 `dir` 并在 Drop 时换回来。
///
/// **必须**用 `helix_stdx::env::set_current_working_dir` 而不是 `std::env::set_current_dir`:
/// 前者同时更新 helix-stdx 的 `CWD` 缓存(`current_working_dir` 读的就是它),
/// 后者只 chdir ⇒ `helix.cwd()` 会返回缓存里的旧值(测试假红/假绿都见过)。
///
/// Drop 而非手工恢复:早退(`?`/panic)也要还原,否则污染同进程的其它测试。
struct CwdGuard(PathBuf);

impl CwdGuard {
    fn enter(dir: &Path) -> Self {
        let prev = helix_stdx::env::current_working_dir();
        helix_stdx::env::set_current_working_dir(dir).expect("chdir 到测试目录");
        Self(prev)
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = helix_stdx::env::set_current_working_dir(&self.0);
    }
}

fn ctx() -> CommandContext {
    CommandContext {
        docs: vec![],
        path: Some("/tmp/e.rs".into()),
        text: "x".into(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    }
}

/// `helix.cwd()` 返回绝对路径,**每次调用现取**(chdir 后立刻变)。
#[test]
fn cwd_getter_tracks_chdir() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    let dir = tempfile::tempdir().unwrap();
    let _cwd = CwdGuard::enter(dir.path());

    load_script(r#"helix.echo("cwd:" + helix.cwd());"#).unwrap();
    let got = take_messages().pop().expect("helix.cwd() 应有输出");
    let got = got.strip_prefix("cwd:").expect("前缀");

    // 与核心同一个真值(同样的 lexical canonicalize)
    assert_eq!(
        got,
        helix_stdx::env::current_working_dir().to_string_lossy(),
        "helix.cwd() 必须与核心 current_working_dir() 一致"
    );
    assert!(
        Path::new(got).is_absolute(),
        "helix.cwd() 必须是绝对路径,实际: {got}"
    );

    // 再 chdir 一次:getter 不缓存
    let sub = dir.path().join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    helix_stdx::env::set_current_working_dir(&sub).unwrap();
    load_script(r#"helix.echo("cwd:" + helix.cwd());"#).unwrap();
    let got2 = take_messages().pop().expect("第二次输出");
    assert_eq!(
        got2.strip_prefix("cwd:").unwrap(),
        helix_stdx::env::current_working_dir().to_string_lossy(),
        "chdir 之后必须立刻反映"
    );
    assert_ne!(got, got2.strip_prefix("cwd:").unwrap(), "两次 cwd 应不同");
}

/// `cwd-change` 在事件白名单里,payload 带**新的和旧的** cwd。
#[test]
fn cwd_change_event_carries_prev_and_cwd() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    assert!(!has_handlers("cwd-change"));

    load_script(
        r#"
    helix.on("cwd-change", (doc, ev) => {
        helix.echo("cwd=" + ev.cwd + " prev=" + ev.prev_cwd + " type=" + ev.type + " v=" + ev.api_version);
    });
    "#,
    )
    .unwrap();
    assert!(has_handlers("cwd-change"));

    // 有旧目录
    emit_cwd_change(&ctx(), Some(Path::new("/tmp/prev")), Path::new("/tmp/next")).unwrap();
    assert_eq!(
        take_messages(),
        vec![format!(
            "cwd=/tmp/next prev=/tmp/prev type=cwd-change v={API_VERSION}"
        )]
    );

    // 旧目录未知 → null(不是缺字段:插件可以直接判 `ev.prev_cwd === null`)
    emit_cwd_change(&ctx(), None, Path::new("/tmp/other")).unwrap();
    assert_eq!(
        take_messages(),
        vec![format!(
            "cwd=/tmp/other prev=null type=cwd-change v={API_VERSION}"
        )]
    );
}

/// 没有订阅者时是零成本空操作(发射点每 `:cd` 都会调,不能为此建引擎/报错)。
#[test]
fn cwd_change_without_handlers_is_noop() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init();
    assert!(!has_handlers("cwd-change"));
    emit_cwd_change(&ctx(), None, Path::new("/tmp/x")).unwrap();
    assert!(take_messages().is_empty());
}
