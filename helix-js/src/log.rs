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

    /// 严重度序号（越小越严重）。过滤按"**至少**这么严重"的惯例：
    /// `:js-log warn` = warn + error，与 `RUST_LOG`/日志库的用法一致。
    fn rank(self) -> u8 {
        match self {
            Level::Error => 0,
            Level::Warn => 1,
            Level::Info => 2,
            Level::Debug => 3,
        }
    }

    /// 本条是否达到 `min` 的严重度（`min = Warn` ⇒ Error 与 Warn 都算）。
    pub fn at_least(self, min: Level) -> bool {
        self.rank() <= min.rank()
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
        push_capped(&mut q, entry.clone());
    }
    let f = sink().lock().ok().and_then(|s| *s);
    if let Some(f) = f {
        f(entry.level, &entry.target, &entry.msg);
    }
}

/// 环缓冲写入（纯逻辑，便于单测）：满了先挤掉最旧的。
///
/// 之所以抽出来：`record()` 动的是**进程级**缓冲，别的测试（并行）随时在往里写 ⇒ 在全局量上
/// 断言"恰好 CAPACITY 条"必然偶发红。不变式本身在这里测，全局那层只测"自己的条目在不在"。
fn push_capped(q: &mut VecDeque<Entry>, entry: Entry) {
    while q.len() >= CAPACITY {
        q.pop_front();
    }
    q.push_back(entry);
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

/// 整份快照（不过滤）。
pub fn format_all() -> String {
    format_filtered(None)
}

/// 按**最低级别**过滤后的纯文本形式（`:js-log` 用；`None` = 全部）。
///
/// 语义是"**至少**这么严重"（`:js-log warn` = warn + error），日志的通行惯例。
/// 三条边界都要**说清楚**，否则用户会以为日志坏了：
/// - 完全空缓冲 → 怎么记 + 会记多少；
/// - 有条目但被过滤空 → 说"没有 ≥ X 的条目"并给出总数（否则像是丢了）；
/// - 过滤生效 → 头部写明过滤条件与"X / 共 Y"。
pub fn format_filtered(min: Option<Level>) -> String {
    let all = snapshot();
    if all.is_empty() {
        return "（插件日志为空）\n\n插件可以用 helix.log(\"error\", \"...\") 记录；\n\
                插件失败会自动记在这里（最近 500 条）。\n"
            .to_string();
    }
    let items: Vec<&Entry> = match min {
        Some(m) => all.iter().filter(|e| e.level.at_least(m)).collect(),
        None => all.iter().collect(),
    };
    if items.is_empty() {
        return format!(
            "（没有 {} 及更严重的条目；缓冲里共 {} 条 —— 直接 :js-log 看全部）\n",
            min.map(Level::as_str).unwrap_or("?"),
            all.len()
        );
    }
    let mut out = String::new();
    match min {
        Some(m) => out.push_str(&format!(
            "helix 插件日志 —— {} 条（级别 ≥ {}；共 {} 条 / 最多 {CAPACITY}）\n\n",
            items.len(),
            m.as_str(),
            all.len()
        )),
        None => out.push_str(&format!(
            "helix 插件日志 —— {} 条（最多 {CAPACITY}）\n\n",
            all.len()
        )),
    }
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
        // 单参形态：那个参数**就是消息**，级别一律 info。
        // 不能"顺手猜级别"：`helix.log("error")` 是想记一条内容为 "error" 的消息，
        // 猜成级别会让它变成一条空 error —— 与 d.ts 的 `log(msg)` 声明也对不上。
        1 => (
            Level::Info,
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

    /// 缓冲是**进程级**的 ⇒ 所有动它的用例必须共用同一把锁。
    /// 用 `crate::tests::TEST_LOCK`（而不是本模块私有的一把）：`tests/log_api.rs` 走 JS 面
    /// 也会清空它，两把锁各锁各的等于没锁（实测：单跑绿、组跑红）。
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        crate::tests::TEST_LOCK
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

    /// 封顶不变式：纯函数上钉死（不受并发写者影响 —— 这条最早写在全局缓冲上，偶发红）。
    #[test]
    fn push_capped_drops_the_oldest() {
        let _g = lock();
        let mut q: VecDeque<Entry> = VecDeque::new();
        for i in 0..(CAPACITY + 10) {
            push_capped(
                &mut q,
                Entry {
                    seq: i as u64,
                    ts_ms: 0,
                    level: Level::Debug,
                    target: "t".to_string(),
                    msg: format!("m{i}"),
                },
            );
        }
        assert_eq!(q.len(), CAPACITY, "容量封顶");
        assert_eq!(q.front().unwrap().msg, "m10", "最旧的被挤掉");
        assert_eq!(q.back().unwrap().msg, format!("m{}", CAPACITY + 9));
    }

    /// 全局层只断言"自己的条目在里面"（别的测试并行写进来的条目不算错）。
    #[test]
    fn record_puts_the_entry_into_the_global_buffer() {
        let _g = lock();
        clear();
        record(Level::Warn, "capped-test", "marker-entry");
        let snap = snapshot();
        assert!(
            snap.iter()
                .any(|e| e.msg == "marker-entry" && e.target == "capped-test"),
            "自己的条目必须在缓冲里: {snap:?}"
        );
        assert!(len() <= CAPACITY, "任何时候都不该超过容量");
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
    fn format_filtered_means_at_least_that_severe() {
        let _g = lock();
        clear();
        error("t", "E1");
        record(Level::Warn, "t", "W1");
        record(Level::Info, "t", "I1");
        record(Level::Debug, "t", "D1");

        let warn_up = format_filtered(Some(Level::Warn));
        assert!(
            warn_up.contains("E1") && warn_up.contains("W1"),
            "{warn_up}"
        );
        assert!(
            !warn_up.contains("I1") && !warn_up.contains("D1"),
            "info/debug 不该出现: {warn_up}"
        );
        assert!(
            warn_up.contains("级别 ≥ WARN"),
            "头部要写明过滤条件: {warn_up}"
        );
        assert!(
            warn_up.contains("共 4 条"),
            "要给出总数（否则像是丢了）: {warn_up}"
        );

        let err_only = format_filtered(Some(Level::Error));
        assert!(err_only.contains("E1"));
        assert!(!err_only.contains("W1"), "error 档只留 error: {err_only}");

        let all = format_filtered(None);
        assert!(
            all.contains("E1") && all.contains("D1"),
            "不过滤时全在: {all}"
        );
        assert!(!all.contains("级别 ≥"), "不过滤不该出现过滤标记: {all}");
        clear();
    }

    #[test]
    fn format_filtered_empty_after_filter_says_so_and_gives_total() {
        let _g = lock();
        clear();
        record(Level::Info, "t", "只有 info");
        let text = format_filtered(Some(Level::Error));
        assert!(text.contains("没有 ERROR 及更严重的条目"), "{text}");
        assert!(text.contains("共 1 条"), "要说明缓冲里有几条: {text}");
        assert!(text.contains(":js-log"), "要告诉用户怎么看全部: {text}");
        clear();
    }

    #[test]
    fn at_least_ordering() {
        assert!(Level::Error.at_least(Level::Warn));
        assert!(Level::Warn.at_least(Level::Warn));
        assert!(!Level::Info.at_least(Level::Warn));
        assert!(Level::Debug.at_least(Level::Debug));
        assert!(Level::Error.at_least(Level::Error));
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
