// 微基准：nucleo 打分在 5000 条路径上的变体对比（不经 JS 引擎）
#[path = "../engines/mod.rs"]
mod engines;

use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Utf32Str};

fn rows() -> Vec<String> {
    (0..5000)
        .map(|i| format!("src/dir{}/file_{}.rs:{}:{}:fn item_{}", i % 40, i, i % 500, i % 80, i))
        .collect()
}

fn bench(label: &str, rows: &[String], norm: Normalization, cfg: Config, iters: u32) {
    for _ in 0..3 {
        let _ = run(rows, "file_1", norm, cfg.clone());
    }
    let t0 = std::time::Instant::now();
    let mut c = 0;
    for _ in 0..iters {
        c += run(rows, "file_1", norm, cfg.clone());
    }
    std::hint::black_box(c);
    println!(
        "{label:<40} {:>7.3} ms/次 (hits={})",
        t0.elapsed().as_secs_f64() * 1e3 / f64::from(iters),
        c / iters as usize
    );
}

fn run(rows: &[String], q: &str, norm: Normalization, cfg: Config) -> usize {
    let pattern = Pattern::parse(q, CaseMatching::Smart, norm);
    let mut m = Matcher::new(cfg.clone());
    let mut buf = Vec::new();
    let mut n = 0;
    for s in rows {
        if pattern.score(Utf32Str::new(s, &mut buf), &mut m).is_some() {
            n += 1;
        }
    }
    n
}

fn main() {
    let r = rows();
    bench("Smart + Normalization::Smart", &r, Normalization::Smart, Config::DEFAULT, 200);
    bench("Smart + Normalization::Never", &r, Normalization::Never, Config::DEFAULT, 200);
    bench(
        "Smart+Never + match_paths",
        &r,
        Normalization::Never,
        Config::DEFAULT.match_paths(),
        200,
    );
    bench(
        "Ignore+Never + match_paths",
        &r,
        Normalization::Never,
        Config::DEFAULT.match_paths(),
        200,
    );
}
