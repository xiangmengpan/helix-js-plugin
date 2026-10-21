//! 引擎横评 harness —— 场景在 `scripts/*.js`，这里只负责"同一份脚本、同一套桩、同一套计时"。
//!
//! 协议：每个引擎实现 [`Engine`]；`run(scenario, iters)` 返回**这一轮的总耗时 ns**。
//! 计时在 Rust 侧（`Instant`），Node 侧用 `process.hrtime.bigint()` —— 单位一致。

pub mod boa;
pub mod qjs;

use std::time::Instant;

/// 场景名（与 `scripts/*.js` 的文件名一一对应）
pub const SCENARIOS: &[&str] = &["eval", "filter", "filter_core", "render", "native", "json"];

/// 命令行覆盖（`--iters`）；0 = 不覆盖
static ITERS_OVERRIDE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn set_iters_override(n: u64) {
    ITERS_OVERRIDE.store(n, std::sync::atomic::Ordering::Relaxed);
}

/// 每个场景的 `iters`（一次 `bench_run` 里的内层次数）
pub fn iters_for(scenario: &str) -> u64 {
    let o = ITERS_OVERRIDE.load(std::sync::atomic::Ordering::Relaxed);
    if o > 0 {
        return o;
    }
    match scenario {
        // 一次 eval 就是"加载一个插件"；iters = 重复加载次数
        "eval" => 20,
        // picker 打开一次：5000 行过滤+排序
        "filter" => 50,
        // 同一批数据走进程内匹配（nucleo）
        "filter_core" => 50,
        // 一帧渲染：30 行 × 3 列
        "render" => 200,
        // native 边界：1000 次调用
        "native" => 1000,
        // 读一次布局快照
        "json" => 100,
        _ => 100,
    }
}

pub trait Engine {
    fn name(&self) -> &'static str;
    /// 跑一轮（`iters` 次内层循环），返回总耗时 ns 与一个校验值（防死代码消除）。
    fn run(&mut self, scenario: &str, iters: u64) -> anyhow::Result<(u128, f64)>;
}

#[derive(Debug, Clone)]
pub struct Row {
    pub engine: &'static str,
    pub scenario: String,
    pub iters: u64,
    /// 每次内层操作的纳秒数（中位数 of 5 轮）
    pub per_op_ns: f64,
    /// 校验值（同场景跨引擎应一致，否则说明三个桩不等价）
    pub checksum: f64,
}

/// warmup 1 轮 + 5 轮取中位数
pub fn measure(e: &mut dyn Engine, scenario: &str) -> anyhow::Result<Row> {
    let iters = iters_for(scenario);
    // warmup：不参与统计（boa 的 JIT-less 解释器也要经历一次函数体热身后才稳定）
    let _ = e.run(scenario, iters)?;
    let mut samples = Vec::new();
    let mut checksum = 0.0;
    for _ in 0..5 {
        let t0 = Instant::now();
        let (ns, sum) = e.run(scenario, iters)?;
        let elapsed = if ns == 0 {
            t0.elapsed().as_nanos()
        } else {
            ns
        };
        samples.push(elapsed as f64 / iters as f64);
        checksum = sum;
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Ok(Row {
        engine: e.name(),
        scenario: scenario.to_string(),
        iters,
        per_op_ns: samples[samples.len() / 2],
        checksum,
    })
}

/// 场景源码（编译期嵌入：跑的时候不依赖 cwd）
pub fn script(scenario: &str) -> &'static str {
    match scenario {
        "eval" => include_str!("../../scripts/eval-plugin.js"),
        "filter" => include_str!("../../scripts/filter.js"),
        "filter_core" => include_str!("../../scripts/filter_core.js"),
        "render" => include_str!("../../scripts/render.js"),
        "native" => include_str!("../../scripts/native.js"),
        "json" => include_str!("../../scripts/json.js"),
        _ => "",
    }
}

/// 单位换算的展示名（表格里用）
pub fn unit(scenario: &str) -> (&'static str, f64) {
    match scenario {
        // 一帧 = 1 次 render
        "render" => ("ms/帧", 1e6),
        // 一次 picker 决策 = 1 次 filter
        "filter" | "filter_core" => ("ms/次", 1e6),
        // 一次快照读
        "json" => ("ms/次", 1e6),
        // 单次 native 调用
        "native" => ("µs/次", 1e3),
        // 单次插件加载
        "eval" => ("ms/次", 1e6),
        _ => ("µs/次", 1e3),
    }
}

/// `helix.fuzzy.*` 的**同一份**实现（boa 与 quickjs 的桩都调它）：
/// 会话式 —— 条目只搬一次，每次 run 只做打分 + 返回前 limit 条。
pub mod fuzzy {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use nucleo::pattern::{CaseMatching, Normalization, Pattern};
    use nucleo::{Config, Matcher, Utf32Str};

    thread_local! {
        static SESSIONS: RefCell<HashMap<u64, Vec<String>>> = RefCell::new(HashMap::new());
        static NEXT_ID: RefCell<u64> = const { RefCell::new(1) };
    }

    pub fn session(items: Vec<String>) -> u64 {
        let id = NEXT_ID.with(|c| {
            let id = *c.borrow();
            *c.borrow_mut() = id + 1;
            id
        });
        SESSIONS.with(|m| {
            m.borrow_mut().insert(id, items);
        });
        id
    }

    pub fn drop_session(id: u64) {
        SESSIONS.with(|m| {
            m.borrow_mut().remove(&id);
        });
    }

    /// 与产品侧 `helix.fuzzy.run` 同构：空 query ⇒ 空；按分数降序、同分按下标升序、按 limit 截断。
    pub fn run(id: u64, query: &str, limit: usize) -> Vec<(usize, u32)> {
        if query.is_empty() {
            return Vec::new();
        }
        // 注意:**借**着用,不要把 5000 个 String 克隆一份（克隆本身就是 1.5ms 量级）
        SESSIONS.with(|m| {
            let store = m.borrow();
            let Some(items) = store.get(&id) else {
                return Vec::new();
            };
            let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
            let mut matcher = Matcher::new(Config::DEFAULT);
            let mut buf = Vec::new();
            let mut hits = Vec::new();
            for (i, s) in items.iter().enumerate() {
                if let Some(score) = pattern.score(Utf32Str::new(s, &mut buf), &mut matcher) {
                    hits.push((i, score));
                }
            }
            hits.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            if limit > 0 {
                hits.truncate(limit);
            }
            hits
        })
    }
}
