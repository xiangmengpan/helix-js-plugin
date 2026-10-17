#!/usr/bin/env node
/// 能力检查点 ↔ 实际注册的 JS API **机械对账**(可复跑 ✓)
///
/// 由来(实测三次都靠人肉撞见,不能再靠人):
///   · `helix.open_terminal` 会起 pty(**process**),却漏在检查点外;
///   · `helix.stat_async` / `glob_async` / `read_tree` / `watch` 都会泄漏文件系统信息
///     (**fs_read**),也全漏;
///   · 于是 `plugins/picker-ui` 的 manifest 声明也不完整(它用了 `read_tree` 却没声明 fs_read)。
/// "哪些 API 该受哪种能力检查"是**意图**,代码里是**事实** —— 两者必须对账。
///
/// ## 三段判据
///   A. **意图表**(下面的 `EXPECTED`)里的每个 API 必须真的注册着(改名/删除会红);
///   B. 它的实现函数体里必须有 `check_js("…", …CAP_<该能力>)`(检查点被删/降级/换错能力会红);
///   C. **哨兵**:扫描**所有**注册的 JS 函数,凡函数体里出现"碰文件系统/起进程"的痕迹、
///      却既不在 `EXPECTED` 也不在 `EXEMPT`(每条附理由)里的 ⇒ 报"疑似漏检查"。
///      C 是启发式(字面量标记 + 去注释),**允许假阴性**(委托给 helper 的可能看不见),
///      只用来让**新**入口浮出水面;真正的闸门是 B。
///
/// ## 用法
///   node plugins/tools/check_capability_surface.mjs    # 退出码 0 = 全绿,1 = 有缺口
///   (已挂进 `cargo xtask api-check`,CI 的 "Check plugin API surface" 步骤会跑到)
///
/// ## 加新 API 时
///   1. 在实现里加 `check_js("helix.xxx", CAP_…)`;
///   2. 在下面 `EXPECTED` 里登记(这是**意图**的唯一出处);
///   3. 若某函数确实**不该**受检查(如插件加载器),登记进 `EXEMPT` 并写清理由。

import { readFileSync, readdirSync, existsSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const rel = (p) => join(ROOT, p);

/// **意图表**:API 名 → 它该受哪种能力检查。这是"能力模型"的唯一声明处。
const EXPECTED = {
  // process:起子进程 / shell
  process: ["run", "run_async", "spawn", "open_terminal"],
  // fs_read:读文件系统(含元信息、枚举、监听)
  fs_read: [
    "read_file",
    "read_file_async",
    "read_dir",
    "stat_async",
    "glob_async",
    "read_tree",
    "watch",
  ],
  // fs_write:写/删文件系统
  fs_write: [
    "write_file",
    "write_file_async",
    "write_plugin_file",
    "remove_plugin_file",
  ],
};

/// **豁免**:函数体里虽然有 fs/进程痕迹,但**有意**不做能力检查 —— 每条必须写清理由。
const EXEMPT = {
  load: "插件加载器本身(依赖解析):路径经 plugin roots 解析,是既有信任域的一部分;它不是通用文件读取通道 —— 且它给**每个**插件用,单独收能力会让所有 deps 都变噪音",
};

/// 启发式标记:函数体(去注释)里出现这些字面量 ⇒ 认为它碰了对应能力。
/// 宁窄勿宽:这里的目标只是"新 API 浮出水面",不追求覆盖所有委托写法。
const MARKERS = {
  process: [
    "std::process::Command",
    "Command::new(",
    "open_pty(",
    "spawn_pty_worker(",
    "spawn_worker(",
  ],
  fs_read: [
    "std::fs::read(",
    "std::fs::read_to_string(",
    "std::fs::metadata(",
    "std::fs::read_dir(",
    "read_tree",
    "glob_matches(",
    "globset",
    "recommended_watcher",
    "notify::",
    "UiRequest::Watch",
    "load_script_checked(",
  ],
  fs_write: [
    "std::fs::write(",
    "std::fs::create_dir_all(",
    "std::fs::remove_file(",
    "std::fs::remove_dir",
    "std::fs::rename(",
    "std::fs::OpenOptions",
    "write_plugin_file(",
    "remove_plugin_file(",
  ],
};

const problems = [];
const fail = (m) => {
  problems.push(m);
  console.log("  ✗ " + m);
};
const ok = (m) => console.log("  ✓ " + m);
const section = (m) => console.log("\n" + m);

const read = (p) => {
  const full = rel(p);
  if (!existsSync(full)) {
    console.log(`  ✗ 找不到 ${p}`);
    process.exit(1);
  }
  return readFileSync(full, "utf8");
};

/// 去注释(标记只在**代码**里算数:注释里写 `std::fs::read` 不该触发)
function stripComments(s) {
  return s
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/\/\/[^\n]*/g, "")
    .replace(/^\s*\/\/\/.*$/gm, "");
}

/// 权威注册面:复用 api-check 的 json(唯一与 Rust 注册表对账过的来源)
function registryNames() {
  const script = rel("plugins/tools/check_api_surface.mjs");
  const out = execFileSync("node", [script, "--json"], {
    cwd: ROOT,
    encoding: "utf8",
    maxBuffer: 32 * 1024 * 1024,
  });
  const i = out.indexOf("{");
  const j = out.lastIndexOf("}");
  const api = JSON.parse(out.slice(i, j + 1));
  const names = new Set(api.members?.root || []);
  for (const [ns, list] of Object.entries(api.members || {})) {
    if (ns === "root") continue;
    for (const m of list) names.add(`${ns}.${m}`);
  }
  return new Set([...names].map((n) => n.split(".").pop()));
}

/// 注册表:JS 名 → 实现函数路径(可能多名同名,如 picker.run / run ⇒ 全部保留)
function registrations() {
  // G6 起注册表拆到 `helix-js/src/register/*.rs`(lib.rs 只留拼装),两处都要扫。
  const files = [
    "helix-js/src/lib.rs",
    ...readdirSync(join(ROOT, "helix-js/src/register"))
      .filter((n) => n.endsWith(".rs"))
      .sort()
      .map((n) => `helix-js/src/register/${n}`),
  ];
  const lib = files.map((f) => stripComments(read(f))).join("\n");
  const out = [];
  for (const m of lib.matchAll(
    /from_fn_ptr\(\s*([\w:]+)\s*\),\s*JsString::from\("([a-z_0-9]+)"\)/g,
  )) {
    out.push({ fnPath: m[1], name: m[2] });
  }
  return out;
}

const SRC = {};
for (const f of ["commands", "shell", "popup", "watch", "picker", "input", "lsp"])
  SRC[f] = stripComments(read(`helix-js/src/${f}.rs`));

/// 取函数体(同名函数只在**限定路径**指向的文件里找;找不到就看所有文件)
function bodyOf(fnPath) {
  const name = fnPath.split("::").pop();
  const mod = fnPath.includes("::") ? fnPath.split("::")[0].replace(/^crate::/, "") : null;
  const files = mod && SRC[mod] ? [mod] : Object.keys(SRC);
  for (const f of files) {
    const s = SRC[f];
    for (const m of s.matchAll(new RegExp(`fn\\s+${name}\\s*\\(`, "g"))) {
      const open = s.indexOf("{", m.index);
      if (open < 0) continue;
      let d = 0;
      for (let j = open; j < s.length; j++) {
        if (s[j] === "{") d++;
        else if (s[j] === "}") {
          d--;
          if (d === 0) return s.slice(open, j + 1);
        }
      }
    }
  }
  return null;
}

const registered = registryNames();
const regs = registrations();

section("── A. 意图表里的 API 是否真的注册着 ──");
{
  const missing = [];
  for (const [cap, names] of Object.entries(EXPECTED))
    for (const n of names) if (!registered.has(n)) missing.push(`${n}(${cap})`);
  if (missing.length) for (const m of missing) fail(`EXPECTED 里的 ${m} 在注册表里不存在(改名/删除?)`);
  else {
    const total = Object.values(EXPECTED).flat().length;
    ok(`意图表 ${total} 个 API 全部注册着`);
  }
}

section("── B. 每个 API 的实现里是否真有对应检查点 ──");
{
  let checked = 0;
  for (const [cap, names] of Object.entries(EXPECTED)) {
    for (const name of names) {
      // 同名多注册(picker.run / run):取**任意**一个有该检查点的实现即可
      const bodies = regs.filter((r) => r.name === name).map((r) => bodyOf(r.fnPath));
      const hit = bodies.find((b) => b && b.includes(`check_js("`));
      if (!hit) {
        fail(`helix.${name}: 实现里**没有** check_js(应受 ${cap} 检查)`);
        continue;
      }
      const m = hit.match(/check_js\(\s*"[^"]+"\s*,\s*crate::permissions::CAP_(\w+)/);
      if (!m) {
        fail(`helix.${name}: 有 check_js 但取不到能力常量(写法变了?)`);
        continue;
      }
      const got = m[1].toLowerCase();
      if (got !== cap) fail(`helix.${name}: 检查点用的是 ${got.toUpperCase()},意图表要求 ${cap}`);
      else checked++;
    }
  }
  if (checked === Object.values(EXPECTED).flat().length)
    ok(`${checked} 个 API 的检查点与意图表一致`);
}

section("── C. 哨兵:碰到 fs/进程但没登记的入口 ──");
{
  const expectedNames = new Set(Object.values(EXPECTED).flat());
  const seen = new Set();
  const unknown = [];
  for (const { name, fnPath } of regs) {
    const key = `${name}@${fnPath}`;
    if (seen.has(key)) continue;
    seen.add(key);
    if (expectedNames.has(name) || EXEMPT[name]) continue;
    const body = bodyOf(fnPath);
    if (!body) continue;
    const caps = Object.entries(MARKERS)
      .filter(([, pats]) => pats.some((p) => body.includes(p)))
      .map(([cap]) => cap);
    if (caps.length) unknown.push(`helix.${name} (${fnPath}) 碰到 ${caps.join("/")}`);
  }
  if (unknown.length) {
    for (const u of unknown) fail(`疑似漏检查:${u} —— 加 check_js 并登记进 EXPECTED,或加进 EXEMPT 说明理由`);
  } else {
    const exempt = Object.keys(EXEMPT).length;
    ok(`没有"碰到 fs/进程却未登记"的入口(已豁免 ${exempt} 个,均附理由)`);
  }
}

section("── 汇总 ──");
const total = Object.values(EXPECTED).flat().length;
console.log(
  `  能力检查点:${total} 个 API / ${Object.keys(EXPECTED).length} 种能力 · 豁免 ${Object.keys(EXEMPT).length} 个`,
);
if (problems.length) {
  console.log(`\n✗ 有缺口(${problems.length} 处)`);
  console.log("\n修法:实现里加 `check_js(\"helix.xxx\", crate::permissions::CAP_…)` 并登记进 EXPECTED;");
  console.log("      确实不该受检查的,登记进 EXEMPT 并写清理由。");
  process.exit(1);
}
console.log("\n✓ 全绿:能力检查点与注册的 JS API 一致");
