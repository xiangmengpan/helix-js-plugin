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

/// ## 数据源（2026-10-22 起）
///
/// 能力声明**搬进了表**：`helix-js/src/api.rs` 的 `Entry.caps`。`cargo xtask api-describe`
/// 把它导出成 `target/api-surface.json`，本脚本只读这份 JSON。
///
/// 于是：
/// * **段 A（意图表里的 API 是否存在）** 变成"每个带 caps 的成员是否都能在源码里找到实现"
///   —— 表与代码漂移会红；
/// * **段 B（实现里是否有 check_js）取消**：检查点由 `api::native()` 在**派发点**统一执行，
///   "忘记写检查"这个失败模式已经不存在（表里不写 caps 才是问题，而那由段 C 兜）；
/// * **段 C（哨兵）保留**：函数体里出现 fs/进程痕迹、但表里 caps 为空 ⇒ 报"疑似漏检查"。
///   这是唯一还需要读源码的部分，也是"新增 API 忘了声明能力"的最后一道网。
///
/// **意图表**:API 名 → 它该受哪种能力检查。这是"能力模型"的唯一声明处。
const readSync = (p) => readFileSync(p, "utf8");

const SURFACE = JSON.parse(readSync(rel("target/api-surface.json")));
if (!SURFACE.members) {
  console.log("  ✗ target/api-surface.json 里没有 members（先跑 cargo xtask api-describe）");
  process.exit(1);
}

/// 期望表：能力 → 成员名（**全名**：`server.list`；根成员就是 `run`）
const EXPECTED = {};
for (const m of SURFACE.members) {
  for (const cap of m.caps || []) {
    const name = m.ns ? `${m.ns}.${m.name}` : m.name;
    (EXPECTED[cap] ||= []).push(name);
  }
}


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
// 全部顶层模块（实现函数散在 layout_api/theme/draw 等文件里；按目录扫，避免再手工维护清单）
for (const f of readdirSync(rel("helix-js/src")).filter((n) => n.endsWith(".rs")))
  SRC[f.replace(/\.rs$/, "")] = stripComments(read(`helix-js/src/${f}`));

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

section("── A. 表里的实现函数是否都能在源码里找到 ──");
{
  const missing = [];
  for (const m of SURFACE.members) {
    if (!m.fname) continue; // Dynamic / Const / FunctionObject 没有直接实现体
    if (!bodyOf(m.fname)) missing.push(`${m.ns ? m.ns + "." : ""}${m.name} → ${m.fname}`);
  }
  if (missing.length) for (const x of missing) fail(`表里登记的实现函数在源码里找不到: ${x}`);
  else ok(`${SURFACE.members.filter((m) => m.fname).length} 个实现函数都能定位`);
}

section("── B. 能力声明的一致性 ──");
{
  // 派发点统一执行（`api::native`），这里只校验"声明本身自洽"：
  // 有 caps 的成员必须是普通函数（Const/Dynamic 不该带 caps）。
  const bad = SURFACE.members.filter((m) => (m.caps || []).length && m.kind !== "fn");
  if (bad.length) for (const m of bad) fail(`${m.ns || "helix"}.${m.name}: 非函数成员不该声明 caps`);
  else ok(`${Object.values(EXPECTED).flat().length} 个受保护 API 的 caps 声明自洽（由派发点执行）`);
}

section("── C. 哨兵:函数体碰到 fs/进程但表里没声明能力 ──");
{
  const exempt = EXEMPT;
  const unknown = [];
  for (const m of SURFACE.members) {
    if (!m.fname) continue;
    if ((m.caps || []).length) continue; // 已声明
    if (exempt[m.name]) continue;
    const body = bodyOf(m.fname);
    if (!body) continue;
    const hit = Object.entries(MARKERS)
      .filter(([, pats]) => pats.some((p) => body.includes(p)))
      .map(([cap]) => cap);
    if (hit.length) unknown.push(`helix.${m.ns ? m.ns + "." : ""}${m.name} → ${m.fname} 碰到 ${hit.join("/")}`);
  }
  if (unknown.length) {
    for (const u of unknown) fail(`疑似漏声明:${u} —— 在 api.rs 的 caps 里声明,或加进 EXEMPT 说明理由`);
  } else {
    ok(`没有"碰到 fs/进程却没声明能力"的入口(已豁免 ${Object.keys(exempt).length} 个,均附理由)`);
  }
}

section("── 汇总 ──");
const total = Object.values(EXPECTED).flat().length;
console.log(
  `  能力检查点:${total} 个 API / ${Object.keys(EXPECTED).length} 种能力 · 豁免 ${Object.keys(EXEMPT).length} 个`,
);
if (problems.length) {
  console.log(`\n✗ 有缺口(${problems.length} 处)`);
  console.log("\n修法:在 helix-js/src/api.rs 的对应行补 `caps`（或加进 EXEMPT 说明理由）;");
  console.log("      确实不该受检查的,登记进 EXEMPT 并写清理由。");
  process.exit(1);
}
console.log("\n✓ 全绿:能力检查点与注册的 JS API 一致");
