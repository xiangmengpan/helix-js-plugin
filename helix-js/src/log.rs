//! 插件日志：环形缓冲 + 宿主 sink。
//!
//! **为什么需要**（2026-11-03）：插件失败目前只走状态栏，而状态栏是**单槽**
//! （`editor.status_msg = Some(..)`，下一条消息直接盖掉，上游还留着
//! "show multiple status messages at once" 的 TODO）⇒ 用户看不到、事后也查不到。
//! 而 `helix-js` **刻意不依赖 `log` crate**（见 commands.rs 里那几条"只 log::warn 等于静默"），
//! 于是插件错误从不落盘 —— 只有 `HX_DBG_JS_THREAD` 时的 `js_dbg`（走 stderr，TUI 下看不见）。
//!
//! 这里补上这一层，三件事：
//! 1. **环缓冲**：所有插件相关失败 + 插件自己 `helix.log()` 的条目，最近 `CAPACITY` 条。
//! 2. **sink**：宿主启动时装一个函数把条目转给 `log` crate ⇒ 落进 `helix.log`
//!    （`:log-open` 能看）。helix-js 仍不依赖 `log`，依赖方向不变。
//! 3. **`helix.log(level, msg)`**：插件自己也能记（target 自动取当前插件名）。
//!
//! 查看：`:js-log`（核心命令，**不依赖任何插件** —— 插件坏了才更需要看日志）。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// 环缓冲容量。为什么不是无上限：日志是**排障用**，不是数据源；不封顶等于给面板/
/// 事件回调里的一次 typo 攒一颗内存炸弹。
pub const CAPACITY: usize = 500;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN",
            Level::Info => "INFO",
            Level::Debug => "DEBUG",
        }
    }

    /// JS 侧的宽松解析：`"error"`/`"warn"`/`"warning"`/`"info"`/`"debug"`，大小写不敏感。
    /// 认不出 ⇒ `None`（调用方退回 Info，**不静默丢条目**）。
    pub fn parse(s: &str) -> Option<Level> {
        match s.trim().to_ascii_lowercase().as_str() {
            "error" | "err" => Some(Level::Error),
            "warn" | "warning" => Some(Level::Warn),
            "info" => Some(Level::Info),
            "debug" | "trace" => Some(Level::Debug),
            _ => None,
        }
    }
}

/// 一条日志。`seq` 单调递增（清空后继续涨，便于对齐两次快照）。
#[derive(Clone, Debug)]
pub struct Entry {
    pub seq: u64,
    pub ts_ms: u128,
    pub level: Level,
    pub target: String,
    pub msg: String,
}

/// sink：把条目转给 `log` crate。`fn` 指针即可（没有闭包捕获 -> 无生命周期麻烦）。
type Sink = fn(Level, &str, &str);

fn entries() -> &'static Mutex<VecDeque<Entry>> {
    static ENTRIES: OnceLock<Mutex<VecDeque<Entry>>> = OnceLock::new();
    ENTRIES.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn sink() -> &'static Mutex<Option<Sink>> {
    static SINK: OnceLock<Mutex<Option<Sink>>> = OnceLock::new();
    SINK.get_or_init(|| Mutex::new(None))
}

fn next_seq() -> u64 {
    static SEQ: AtomicU64 = AtomicU64::new(1);
    SEQ.fetch_add(1, Ordering::Relaxed)
}

/// 宿主启动时装一次（重复装 = 后者覆盖）。**不要**在持有 `log` 锁的地方调用 `record`。
pub fn install_sink(f: Sink) {
    if let Ok(mut s) = sink().lock() {
        *s = Some(f);
    }
}

/// 记一条。sink 调用在**锁外**（否则 sink 里再 `record` 会自己锁自己）。
pub fn record(level: Level, target: &str, msg: impl AsRef<str>) {
    let entry = Entry {
        seq: next_seq(),
        ts_ms: now_ms(),
        level,
        target: target.to_string(),
        msg: msg.as_ref().to_string(),
    };
    if let Ok(mut q) = entries().lock() {
        while q.len() >= CAPACITY {
            q.pop_front();
        }
        q.push_back(entry.clone());
    }
    let f = sink().lock().ok().and_then(|s| *s);
    if let Some(f) = f {
        f(entry.level, &entry.target, &entry.msg);
    }
}

/// 便捷：`record(Level::Error, ..)`（失败路径用得最多）。
pub fn error(target: &str, msg: impl AsRef<str>) {
    record(Level::Error, target, msg);
}

/// 当前缓冲（按写入顺序；最旧在前）。
pub fn snapshot() -> Vec<Entry> {
    entries()
        .lock()
        .map(|q| q.iter().cloned().collect())
        .unwrap_or_default()
}

/// 清空，返回清掉的条数。
pub fn clear() -> usize {
    entries()
        .lock()
        .map(|mut q| {
            let n = q.len();
            q.clear();
            n
        })
        .unwrap_or(0)
}

pub fn len() -> usize {
    entries().lock().map(|q| q.len()).unwrap_or(0)
}

/// 整份快照的纯文本形式（`:js-log` 用）。空缓冲也给出**明确**的一句，
/// 而不是一个空白 buffer（"什么都没有"和"功能坏了"要能区分）。
pub fn format_all() -> String {
    let items = snapshot();
    if items.is_empty() {
        return "（插件日志为空）\n\n插件可以用 helix.log(\"error\", \"...\") 记录；\n\
                插件失败会自动记在这里（最近 500 条）。\n"
            .to_string();
    }
    let mut out = String::new();
    out.push_str(&format!(
        "helix 插件日志 —— {} 条（最多 {CAPACITY}）\n\n",
        items.len()
    ));
    for e in items {
        out.push_str(&format!(
            "{} {:<5} [{}] {}\n",
            fmt_ts(e.ts_ms),
            e.level.as_str(),
            e.target,
            e.msg
        ));
    }
    out
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// 本地时区拿不到（不引依赖）⇒ 按 UTC 的 `HH:MM:SS.mmm` 打印；同一次会话内相对顺序永远正确。
fn fmt_ts(ms: u128) -> String {
    let secs = (ms / 1000) % 86_400;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60,
        ms % 1000
    )
}

// ────────────────────────── JS API：helix.log ──────────────────────────

/// `helix.log(msg)` / `helix.log(level, msg)`。
///
/// 两种形态都收（只给一个字符串 ⇒ Info）：插件作者不会为打一行日志去查枚举。
/// `target` 取**当前属主**（注册期/回调期绑定的插件名），拿不到就 `"plugin"`。
pub(crate) fn js_log(
    _this: &boa_engine::JsValue,
    args: &[boa_engine::JsValue],
    context: &mut boa_engine::Context,
) -> boa_engine::JsResult<boa_engine::JsValue> {
    use boa_engine::JsValue;

    let first = args.first().cloned().unwrap_or(JsValue::undefined());
    let (level, msg) = match args.len() {
        0 => (Level::Info, String::new()),
        1 => (
            Level::parse(
                &first
                    .to_string(context)
                    .map(|s| s.to_std_string_escaped())?,
            )
            .unwrap_or(Level::Info),
            // 单参形态：那个参数**就是消息**
            first.to_string(context)?.to_std_string_escaped(),
        ),
        _ => {
            let lvl_str = first.to_string(context)?.to_std_string_escaped();
            let msg = args[1].to_string(context)?.to_std_string_escaped();
            (Level::parse(&lvl_str).unwrap_or(Level::Info), msg)
        }
    };
    let target = crate::permissions::current_owner().unwrap_or_else(|| "plugin".to_string());
    record(level, &target, msg);
    Ok(JsValue::undefined())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 这些用例共享进程级缓冲 ⇒ 串行（并各自先清空）。
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        static L: OnceLock<Mutex<()>> = OnceLock::new();
        L.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn record_snapshot_and_clear() {
        let _g = lock();
        clear();
        record(Level::Info, "filetree", "hello");
        error("filetree", "boom");
        let snap = snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].target, "filetree");
        assert_eq!(snap[1].level, Level::Error, "error() 记的是 Error");
        assert!(snap[0].seq < snap[1].seq, "seq 单调递增");
        assert_eq!(clear(), 2);
        assert_eq!(len(), 0);
    }

    #[test]
    fn ring_buffer_is_capped() {
        let _g = lock();
        clear();
        for i in 0..(CAPACITY + 10) {
            record(Level::Debug, "t", format!("m{i}"));
        }
        assert_eq!(len(), CAPACITY, "容量封顶");
        let snap = snapshot();
        assert_eq!(snap[0].msg, "m10", "最旧的被挤掉");
        clear();
    }

    #[test]
    fn format_all_empty_is_explicit() {
        let _g = lock();
        clear();
        let text = format_all();
        assert!(text.contains("日志为空"), "空缓冲要给明确说明: {text}");
        assert!(text.contains("helix.log"), "顺便告诉用户怎么记: {text}");
    }

    #[test]
    fn format_all_has_level_target_msg() {
        let _g = lock();
        clear();
        error("my-plugin", "回调炸了");
        let text = format_all();
        assert!(text.contains("ERROR"), "{text}");
        assert!(text.contains("[my-plugin]"), "{text}");
        assert!(text.contains("回调炸了"), "{text}");
        clear();
    }

    #[test]
    fn level_parse_is_lenient() {
        assert_eq!(Level::parse("ERROR"), Some(Level::Error));
        assert_eq!(Level::parse(" warning "), Some(Level::Warn));
        assert_eq!(Level::parse("warn"), Some(Level::Warn));
        assert_eq!(Level::parse("Info"), Some(Level::Info));
        assert_eq!(Level::parse("debug"), Some(Level::Debug));
        assert_eq!(
            Level::parse("nope"),
            None,
            "认不出要 None（由调用方退回 Info）"
        );
    }

    #[test]
    fn sink_receives_entries_and_is_installed_once() {
        let _g = lock();
        static HITS: AtomicU64 = AtomicU64::new(0);
        fn collect(level: Level, target: &str, msg: &str) {
            assert_eq!(level, Level::Error);
            assert_eq!(target, "s");
            assert_eq!(msg, "x");
            HITS.fetch_add(1, Ordering::Relaxed);
        }
        clear();
        install_sink(collect);
        error("s", "x");
        assert_eq!(HITS.load(Ordering::Relaxed), 1);
        // 卸载 sink（别的用例不该被它串到）
        if let Ok(mut s) = sink().lock() {
            *s = None;
        }
        clear();
    }
}
