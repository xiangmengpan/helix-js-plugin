//! Logging support for `hx`.

use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// 日志文件大小上限（字节）。`0` = 不限制。
///
/// 用全局原子量而不是把它塞进 `Logger`：`log` crate 的 logger 只能装一次，而**上限来自
/// config**，config 又是在 logging 之后才加载的 ⇒ 由 `set_max_size()` 在启动后补上。
static MAX_SIZE: AtomicU64 = AtomicU64::new(crate::config::DEFAULT_LOG_MAX_SIZE);

/// 设定上限（`0` = 不限制）。启动时由 `main` 在读完 config 后调用；也可以在运行期改。
pub fn set_max_size(bytes: u64) {
    MAX_SIZE.store(bytes, Ordering::Relaxed);
}

pub fn max_size() -> u64 {
    MAX_SIZE.load(Ordering::Relaxed)
}

/// 带上限的文件 sink：**写入会超过上限时就清空重写**（覆盖写，不轮转）。
///
/// 为什么不是"切到 helix.log.1"：日志是排障用的，一轮转就得先猜去哪天的文件里找；
/// 覆盖写保证"最新的 N MB 永远在同一个路径"。代价是丢历史 —— 需要历史就调大上限或另存。
struct CappedFile {
    file: File,
    /// 已写入的字节数（跨进程用文件长度初始化）
    written: u64,
}

impl CappedFile {
    fn open(path: &Path) -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)?;
        let mut me = CappedFile {
            written: file.metadata().map(|m| m.len()).unwrap_or(0),
            file,
        };
        // 启动时就已经超了（上次会话写的）⇒ 立刻清空，别等下一次写入
        me.truncate_if_needed(0)?;
        Ok(me)
    }

    /// 空出 `incoming` 字节的位置：装不下就清空重来。
    fn truncate_if_needed(&mut self, incoming: usize) -> std::io::Result<()> {
        let limit = max_size();
        if limit == 0 {
            return Ok(()); // 0 = 不限制
        }
        if self.written.saturating_add(incoming as u64) > limit {
            self.file.set_len(0)?;
            self.file.seek(SeekFrom::Start(0))?;
            self.written = 0;
        }
        Ok(())
    }
}

impl Write for CappedFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.truncate_if_needed(buf.len())?;
        let n = self.file.write(buf)?;
        self.written = self.written.saturating_add(n as u64);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// Minimal `log::Log` implementation: a level filter plus a single line-buffered sink.
struct Logger {
    level: log::LevelFilter,
    sink: Mutex<Box<dyn Write + Send>>,
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        if let Ok(mut sink) = self.sink.lock() {
            let _ = writeln!(
                sink,
                "{} {} [{}] {}",
                log_timestamp(),
                record.target(),
                record.level(),
                record.args()
            );
        }
    }

    fn flush(&self) {
        if let Ok(mut sink) = self.sink.lock() {
            let _ = sink.flush();
        }
    }
}

fn install(
    level: log::LevelFilter,
    sink: Box<dyn Write + Send>,
) -> Result<(), log::SetLoggerError> {
    log::set_boxed_logger(Box::new(Logger {
        level,
        sink: Mutex::new(sink),
    }))?;
    log::set_max_level(level);
    Ok(())
}

/// Install the global logger writing to `path`（不存在就创建，**追加**写，带大小上限）。
pub fn init_file(level: log::LevelFilter, path: &Path) -> std::io::Result<()> {
    let sink = CappedFile::open(path)?;
    install(level, Box::new(sink)).map_err(std::io::Error::other)
}

/// Install the global logger writing to stdout (used by integration tests).
#[cfg(feature = "integration")]
pub fn init_stdout(level: log::LevelFilter) {
    let _ = install(level, Box::new(std::io::stdout()));
}

/// RFC3339-style UTC timestamp for a log line: `YYYY-MM-DDTHH:MM:SS.mmm`.
pub fn log_timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format_timestamp(now.as_secs(), now.subsec_millis())
}

fn format_timestamp(secs: u64, millis: u32) -> String {
    let days = (secs / 86_400) as i64;
    let tod = secs % 86_400;
    let (hour, min, sec) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    let (year, month, day) = civil_from_days(days);

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}.{millis:03}")
}

/// Howard Hinnant's `civil_from_days`: days since the Unix epoch (1970-01-01,
/// UTC) to `(year, month, day)`. Exact for the whole proleptic Gregorian range.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::format_timestamp;

    #[test]
    fn timestamp_matches_known_instants() {
        // epoch
        assert_eq!(format_timestamp(0, 0), "1970-01-01T00:00:00.000");
        // next day, and time-of-day + millis
        assert_eq!(
            format_timestamp(86_400 + 3661, 7),
            "1970-01-02T01:01:01.007"
        );
        // a leap day: 2024-02-29T12:30:45.123 UTC == 1709209845 s
        assert_eq!(
            format_timestamp(1_709_209_845, 123),
            "2024-02-29T12:30:45.123"
        );
        // 2000-03-01 (the algorithm's era boundary)
        assert_eq!(format_timestamp(951_868_800, 0), "2000-03-01T00:00:00.000");
    }
}

#[cfg(test)]
mod cap_tests {
    use super::*;
    use std::io::Read;

    /// `MAX_SIZE` 是进程级全局量 ⇒ 用例串行（每个用例自己设值、结束恢复默认）。
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        static L: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        L.get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    fn read_all(path: &Path) -> String {
        let mut s = String::new();
        File::open(path).unwrap().read_to_string(&mut s).unwrap();
        s
    }

    #[test]
    fn writes_are_kept_under_the_limit() {
        let _g = lock();
        set_max_size(1024);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("h.log");
        let mut f = CappedFile::open(&path).unwrap();
        writeln!(f, "hello").unwrap();
        f.flush().unwrap();
        assert_eq!(read_all(&path), "hello\n");
        set_max_size(crate::config::DEFAULT_LOG_MAX_SIZE);
    }

    #[test]
    fn exceeding_the_limit_truncates_and_restarts() {
        let _g = lock();
        set_max_size(16); // 够写 3 行 5 字节，第 4 行就超了
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("h.log");
        let mut f = CappedFile::open(&path).unwrap();
        for i in 0..4 {
            writeln!(f, "line{i}").unwrap(); // 每行 6 字节
        }
        f.flush().unwrap();
        let text = read_all(&path);
        assert!(
            !text.contains("line0"),
            "超限后应清空重写，旧行不留: {text:?}"
        );
        assert!(text.contains("line3"), "最新一行必须还在: {text:?}");
        assert!(text.len() as u64 <= 16, "重写后不该超上限: {text:?}");
        set_max_size(crate::config::DEFAULT_LOG_MAX_SIZE);
    }

    #[test]
    fn zero_means_unlimited() {
        let _g = lock();
        set_max_size(0);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("h.log");
        let mut f = CappedFile::open(&path).unwrap();
        for _ in 0..1000 {
            writeln!(f, "0123456789").unwrap();
        }
        f.flush().unwrap();
        assert_eq!(read_all(&path).lines().count(), 1000, "0 = 不限制");
        set_max_size(crate::config::DEFAULT_LOG_MAX_SIZE);
    }

    #[test]
    fn opening_an_oversized_file_truncates_it_immediately() {
        let _g = lock();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("h.log");
        // 先用"不限制"写一份大文件（模拟历史会话攒下的 18MB）
        set_max_size(0);
        {
            let mut f = CappedFile::open(&path).unwrap();
            for _ in 0..100 {
                writeln!(f, "0123456789").unwrap();
            }
            f.flush().unwrap();
        }
        assert!(std::fs::metadata(&path).unwrap().len() > 500);
        // 再把上限调小并重新打开 ⇒ 立刻清空（不必等下一次写入）
        set_max_size(64);
        let _f = CappedFile::open(&path).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            0,
            "启动时已超限的文件要当场清空"
        );
        set_max_size(crate::config::DEFAULT_LOG_MAX_SIZE);
    }
}
