//! 引擎横评 runner：`list` / `run [--scenario NAME] [--engine NAME]`。
//!
//! 为什么自己写而不用 criterion：要横向比三个**不同引擎**，criterion 只测同一个二进制里的
//! 函数；这里的单位是"一次操作"，5 轮取中位数足够稳定（且结果要能贴进文档）。

mod engines;

use std::process::Command;

use engines::{measure, script, unit, Row, SCENARIOS};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("run");
    let scenario_filter = flag(&args, "--scenario");
    let engine_filter = flag(&args, "--engine");
    // `--iters N`：覆盖场景默认内层次数（用来验证"单次成本 vs GC 摊薄"）
    if let Some(n) = flag(&args, "--iters").and_then(|v| v.parse::<u64>().ok()) {
        engines::set_iters_override(n);
    }

    match cmd {
        "list" => {
            for s in SCENARIOS {
                println!("{s}\t{}\t{}", engines::iters_for(s), unit(s).0);
            }
            Ok(())
        }
        "run" => {
            let mut rows: Vec<Row> = Vec::new();
            for s in SCENARIOS {
                if let Some(f) = &scenario_filter {
                    if s != f {
                        continue;
                    }
                }
                if engine_filter.as_deref() != Some("quickjs") && engine_filter.as_deref() != Some("node") {
                    let mut e = engines::boa::Boa::new()?;
                    rows.push(measure(&mut e, s)?);
                }
                if engine_filter.as_deref() != Some("boa") && engine_filter.as_deref() != Some("node") {
                    match engines::qjs::Qjs::new() {
                        Ok(mut e) => rows.push(measure(&mut e, s)?),
                        Err(e) => {
                            eprintln!("! quickjs 不可用（{e}）—— 本引擎记为未测");
                        }
                    }
                }
                if engine_filter.as_deref() != Some("boa") && engine_filter.as_deref() != Some("quickjs") {
                    if let Some(row) = node_measure(s)? {
                        rows.push(row);
                    }
                }
            }
            print_table(&rows);
            write_json(&rows)?;
            Ok(())
        }
        other => {
            eprintln!("usage: engine-bench [list|run] [--scenario S] [--engine boa|quickjs|node]");
            eprintln!("未知子命令: {other}");
            std::process::exit(2);
        }
    }
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

/// Node/V8：参考**上界**（不是候选）。走子进程，读它打出来的 JSON。
fn node_measure(scenario: &str) -> anyhow::Result<Option<Row>> {
    let exe = Command::new("node")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bin/node.mjs"))
        .arg(scenario)
        .output();
    let out = match exe {
        Ok(o) => o,
        Err(e) => {
            eprintln!("! node 不可用（{e}）—— V8 参考列为空");
            return Ok(None);
        }
    };
    if !out.status.success() {
        eprintln!(
            "! node 失败（{scenario}）：{}",
            String::from_utf8_lossy(&out.stderr)
        );
        return Ok(None);
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    Ok(Some(Row {
        engine: "v8(node)",
        scenario: scenario.to_string(),
        iters: v["iters"].as_u64().unwrap_or(0),
        per_op_ns: v["per_op_ns"].as_f64().unwrap_or(-1.0),
        checksum: v["checksum"].as_f64().unwrap_or(-1.0),
    }))
}

fn print_table(rows: &[Row]) {
    println!();
    println!("| 场景 | 单位 | boa | quickjs | v8(node) | 阈值 |");
    println!("|---|---|---|---|---|---|");
    for s in SCENARIOS {
        let (label, div) = unit(s);
        let get = |eng: &str| {
            rows.iter()
                .find(|r| r.scenario.as_str() == *s && r.engine == eng)
                .map(|r| format!("{:.3}", r.per_op_ns / div))
                .unwrap_or_else(|| "—".into())
        };
        let threshold = match *s {
            "render" => "≤ 0.5 ms",
            "filter" => "≤ 2 ms",
            "native" => "≤ 1 µs",
            "json" => "≤ 1 ms",
            "eval" => "≤ 6 ms",
            _ => "",
        };
        println!(
            "| {s} | {label} | {} | {} | {} | {threshold} |",
            get("boa"),
            get("quickjs"),
            get("v8(node)")
        );
    }
    // 校验值一致性：三个桩必须等价，否则测的是桩不是引擎
    for s in SCENARIOS {
        let vals: Vec<f64> = rows
            .iter()
            .filter(|r| r.scenario.as_str() == *s)
            .map(|r| r.checksum)
            .collect();
        if vals.len() > 1 {
            let same = vals.windows(2).all(|w| (w[0] - w[1]).abs() < 1e-9);
            println!("checksum {s}: {} ({:?})", if same { "一致" } else { "不一致 ⚠" }, vals);
        }
    }
    println!();
}

fn write_json(rows: &[Row]) -> anyhow::Result<()> {
    let commit = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    let payload = serde_json::json!({
        "commit": commit,
        "machine": std::env::consts::OS,
        "profile": "release",
        "rows": rows.iter().map(|r| serde_json::json!({
            "engine": r.engine, "scenario": r.scenario, "iters": r.iters,
            "per_op_ns": r.per_op_ns, "checksum": r.checksum,
        })).collect::<Vec<_>>(),
    });
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/results");
    std::fs::create_dir_all(dir)?;
    std::fs::write(
        format!("{dir}/{commit}.json"),
        serde_json::to_string_pretty(&payload)?,
    )?;
    println!("原始数据: tools/engine-bench/results/{commit}.json");
    Ok(())
}

/// 编译期确认场景脚本都在（漏一个 include_str! 会在编译期就报）
#[allow(dead_code)]
fn _scripts_exist() -> usize {
    SCENARIOS.iter().map(|s| script(s).len()).sum()
}
