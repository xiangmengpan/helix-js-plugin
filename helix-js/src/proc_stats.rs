//! `helix.proc_stats()` —— **本进程**的 CPU 时间与常驻内存（跨平台）。
//!
//! 由来：statusline 插件的 `cpu` / `mem` 段一开始在 JS 里读 `/proc/self/*`，
//! 那是 Linux-only 的做法（macOS/Windows 没有 `/proc` ⇒ 两段只能静默消失）。
//! 采样这类"平台私事"应该在核心做一次，插件只负责**格式化与节流**：
//!
//! | 平台 | CPU 时间 | 常驻内存 |
//! |---|---|---|
//! | Linux | `getrusage(RUSAGE_SELF)`（utime+stime） | `/proc/self/status` 的 `VmRSS` |
//! | macOS | 同上 | `task_info(mach_task_self(), MACH_TASK_BASIC_INFO).resident_size` |
//! | Windows | `GetProcessTimes`（kernel+user） | `K32GetProcessMemoryInfo().WorkingSetSize` |
//!
//! 为什么**不**退回 `ps` / `tasklist`：那要每秒 spawn 一个进程，
//! 为一个状态栏格子付这个代价不值（也是本仓库对 `run_async` 的一贯态度）。
//!
//! ## 能力清单：**不收** `fs_read`
//!
//! Linux 分支确实读文件（`/proc/self/status`），但那是**本进程自省**：
//! 没有路径参数 ⇒ 不能被放大成"读任意文件"，macOS/Windows 分支更是纯 syscall。
//! `check_capability_surface.mjs` 的段 C 靠"函数体里有没有 `std::fs::`"来浮出水面，
//! 这里的读操作在私有 helper 里、入口函数申请了 `&[]` —— **是刻意的判定，不是漏网**。

use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsResult, JsString, JsValue};

/// 一次采样（单位固定：毫秒 / KB）。`None` = 该平台/该次拿不到。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProcStats {
    pub cpu_ms: Option<u64>,
    pub rss_kb: Option<u64>,
}

impl ProcStats {
    fn is_empty(&self) -> bool {
        self.cpu_ms.is_none() && self.rss_kb.is_none()
    }
}

/// 采样一次。**不做节流**（节流留给调用方：状态栏是每帧钩子，只有那边知道间隔）。
pub fn sample() -> ProcStats {
    ProcStats {
        cpu_ms: cpu_ms(),
        rss_kb: rss_kb(),
    }
}

// ─────────────────────────── CPU 时间 ───────────────────────────

#[cfg(unix)]
fn cpu_ms() -> Option<u64> {
    // RUSAGE_SELF 汇总**本进程所有线程**的 utime+stime —— 正是状态栏要的口径
    // （Linux 与 macOS 同一套 API，省掉两套代码）。
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: `getrusage` 只写 `ru`；参数与 `rusage` 布局由 libc 保证
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) } != 0 {
        return None;
    }
    let sec = |t: libc::timeval| t.tv_sec.max(0) as u64;
    let usec = |t: libc::timeval| t.tv_usec.max(0) as u64;
    let us =
        (sec(ru.ru_utime) + sec(ru.ru_stime)) * 1_000_000 + usec(ru.ru_utime) + usec(ru.ru_stime);
    Some(us / 1000)
}

#[cfg(windows)]
fn cpu_ms() -> Option<u64> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    // FILETIME 是 100ns 单位
    fn ticks(t: &FILETIME) -> u64 {
        ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64
    }

    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: 四个 out 指针都指向栈上有效内存；句柄来自 GetCurrentProcess
    let ok = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    };
    if ok == 0 {
        return None;
    }
    Some((ticks(&kernel) + ticks(&user)) / 10_000)
}

#[cfg(not(any(unix, windows)))]
fn cpu_ms() -> Option<u64> {
    None
}

// ─────────────────────────── 常驻内存 ───────────────────────────

#[cfg(target_os = "linux")]
fn rss_kb() -> Option<u64> {
    // 用 VmRSS(kB) 而不是 statm 的页数：不依赖页大小(arm64 有 16K 页的机器)
    let text = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = text.lines().find(|l| l.starts_with("VmRSS:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(target_os = "macos")]
#[allow(deprecated)] // `mach_task_self` 被 libc 标了 deprecated(建议换 mach2 crate)——
                     // 为一个调用引一个依赖不值;两个真实用户(macOS CI)也不受影响
fn rss_kb() -> Option<u64> {
    use std::mem::size_of;

    let mut info: libc::mach_task_basic_info_data_t = unsafe { std::mem::zeroed() };
    let mut count =
        (size_of::<libc::mach_task_basic_info_data_t>() / size_of::<libc::natural_t>()) as u32;
    // SAFETY: `task_info` 只写 `info`(最多 `count` 个 natural_t);flavor 与结构体匹配
    let kr = unsafe {
        libc::task_info(
            libc::mach_task_self(),
            libc::MACH_TASK_BASIC_INFO as libc::task_flavor_t,
            (&mut info as *mut libc::mach_task_basic_info_data_t).cast::<libc::integer_t>(),
            &mut count,
        )
    };
    if kr != 0 {
        return None;
    }
    Some(info.resident_size / 1024)
}

#[cfg(windows)]
fn rss_kb() -> Option<u64> {
    use std::mem::size_of;
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut counters = PROCESS_MEMORY_COUNTERS::default();
    // SAFETY: 句柄来自 GetCurrentProcess;`counters` 是栈上有效内存,长度按结构体大小传入
    let ok = unsafe {
        K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
    };
    if ok == 0 {
        return None;
    }
    Some((counters.WorkingSetSize / 1024) as u64)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn rss_kb() -> Option<u64> {
    None
}

// ─────────────────────────── JS 绑定 ───────────────────────────

/// `helix.proc_stats()` → `{ cpu_ms, rss_kb }`；两样都拿不到 ⇒ `null`。
///
/// 字段缺省用 `0` 表达"这一项拿不到"，插件据此单独隐藏那一段
/// （例如 Windows 上万一 `K32GetProcessMemoryInfo` 失败，CPU 照样有值）。
pub(crate) fn js_proc_stats(
    _this: &JsValue,
    _args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let s = sample();
    if s.is_empty() {
        return Ok(JsValue::null());
    }
    Ok(JsValue::from(
        ObjectInitializer::new(ctx)
            .property(
                JsString::from("cpu_ms"),
                JsValue::from(s.cpu_ms.unwrap_or(0) as f64),
                Attribute::all(),
            )
            .property(
                JsString::from("rss_kb"),
                JsValue::from(s.rss_kb.unwrap_or(0) as f64),
                Attribute::all(),
            )
            .build(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 本机(CI 上三平台都跑)必须给出**可用**读数：RSS > 0，且 CPU 时间是单调不减的。
    ///
    /// 退化情形(全 None)曾经是"平台分支写错"的典型症状 ⇒ 用断言钉住。
    #[test]
    fn samples_are_available_on_this_platform() {
        let a = sample();
        assert!(!a.is_empty(), "cpu 与 rss 都拿不到: {a:?}");
        let rss = a.rss_kb.expect("rss 应可用");
        assert!(rss > 0, "rss 应为正数: {rss}");
        let cpu1 = a.cpu_ms.expect("cpu 应可用");
        // 忙一阵，再采一次：CPU 时间必须不减（精度兜底：>= 而不是 >）
        let mut acc = 0u64;
        for i in 0..2_000_000u64 {
            acc = acc.wrapping_add(i % 7);
        }
        assert!(acc > 0);
        let cpu2 = sample().cpu_ms.expect("cpu 应可用");
        assert!(cpu2 >= cpu1, "cpu_ms 不应回退: {cpu1} -> {cpu2}");
    }

    /// JS 面：能拿到对象，字段是数字（不需要任何能力授权）
    #[test]
    fn js_returns_object_with_numeric_fields() {
        crate::init();
        crate::load_script(
            r#"
            const s = helix.proc_stats();
            helix.echo("proto:" + (s ? typeof s.cpu_ms + ":" + typeof s.rss_kb : "null"));
            helix.echo("pos:" + (s && s.rss_kb > 0 ? "yes" : "no"));
            "#,
        )
        .expect("调用不应抛错");
        let msgs = crate::take_messages();
        assert_eq!(msgs[0], "proto:number:number", "两个字段都该是数字");
        assert_eq!(msgs[1], "pos:yes", "本机 rss 应为正数: {msgs:?}");
    }
}
