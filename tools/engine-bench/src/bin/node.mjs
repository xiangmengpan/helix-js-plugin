// Node/V8 —— **参考上界**，不是候选引擎。
// 同一份场景脚本、同名桩；native 场景这里测的是"JS 函数调用"，
// 与 boa/quickjs 的原生边界不可直接比（文档里会标注）。
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const scripts = join(here, "..", "..", "scripts");

const scenario = process.argv[2] || "render";
const ITERS = { eval: 20, filter: 50, render: 200, native: 1000, json: 100 }[scenario] ?? 100;

globalThis.helix = {
  get_style: (name) => "style:" + String(name),
  echo: () => {},
  plugin: () => {},
  register_command: () => {},
  on: () => {},
  map: () => {},
  set_statusline: () => {},
};

const file = scenario === "eval" ? "eval-plugin.js" : scenario + ".js";
const src = readFileSync(join(scripts, file), "utf8");

if (scenario === "eval") {
  // 解析 + 执行一份插件脚本：与 boa/quickjs 的 ctx.eval 同义
  // 间接 eval：在**全局**作用域执行（ESM 里的直接 eval 是严格模式，声明不会进 globalThis）
  for (let i = 0; i < 3; i++) (0, eval)(src); // warmup
  const t0 = process.hrtime.bigint();
  for (let i = 0; i < ITERS; i++) (0, eval)(src);
  const ns = Number(process.hrtime.bigint() - t0);
  emit(ns, ITERS, ITERS);
} else {
  (0, eval)(src);
  const run = globalThis.bench_run;
  let checksum = 0;
  for (let i = 0; i < 1; i++) checksum = run(ITERS); // warmup
  const samples = [];
  for (let r = 0; r < 5; r++) {
    const t0 = process.hrtime.bigint();
    checksum = run(ITERS);
    samples.push(Number(process.hrtime.bigint() - t0) / ITERS);
  }
  samples.sort((a, b) => a - b);
  emit(samples[2] * ITERS, ITERS, checksum);
}

function emit(ns, iters, checksum = -1) {
  process.stdout.write(
    JSON.stringify({ scenario, iters, per_op_ns: ns / iters, checksum }) + "\n"
  );
}
