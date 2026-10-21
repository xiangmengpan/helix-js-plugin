# engine-bench

Helix 插件引擎横评：**boa**（产品现用）/ **QuickJS**（rquickjs）/ **V8**（Node，仅参考上界）。

- 独立 crate（自己的 `[workspace]`），**不是主 workspace 成员** —— 不把 quickjs 的 C 依赖带进主仓构建。
- 结果与决策：[`docs/architecture-plugin-engine-bench.md`](../../docs/architecture-plugin-engine-bench.md)。

```bash
cargo run --release -- list            # 场景清单 + 单位 + 默认 iters
cargo run --release -- run             # 三引擎 × 五场景，打表 + 写 results/<commit>.json
cargo run --release -- run --scenario json --engine boa --iters 1   # 单点复核
```

场景脚本在 `scripts/`：三引擎跑**同一份**源码，桩同名同 arity；每个场景打印 checksum，
三引擎不一致就说明 harness 不等价（不要读那张表）。

新增场景：`scripts/<name>.js` 里定义 `bench_run(iters)`，并在
`src/engines/mod.rs` 的 `SCENARIOS` / `iters_for` / `script` / `unit` 各加一行。
