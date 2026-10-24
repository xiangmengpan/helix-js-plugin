#!/usr/bin/env node
/// `docs/plugin-api.md` ↔ JS API 注册表 **自动对账**(可复跑 ✓)
///
/// 由来:`plugins/tools/check_api_surface.mjs`(api-check)只对账 `plugins/helix.d.ts`,
/// **完全不看 `docs/plugin-api.md`** —— 于是文档会静默腐烂。实测(2026-10-16 一次人工对账):
///   · 文档里还列着 4 个**已删除**的扁平 API(`close_leaf`/`get_layout`/`resize_leaf`/
///     `restore_layout`)⇒ 照文档写 = 运行时 undefined;
///   · 17 个**已注册**的 API 文档完全没记(`define_config`/`get_config`/`watch`/`unwatch`/
///     `term_feed`/`quit`/`viewport`/`picker_state`/…);
///   · 6 个命名空间(`pane`/`layout`/`stack`/`buffer`/`icons`/`pane_mode`)几乎没列。
/// 这三类都是"靠人记得"的步骤 ⇒ 固化成脚本(与 api-check / check_signatures 同一思路)。
///
/// ## 权威来源
/// **不重复解析 Rust**:直接复用 `check_api_surface.mjs --json`(它才是与注册表对账过的),
/// 拿它输出的 `members`(命名空间 → 成员)与 `events` 当唯一真相。
///
/// ## 判据(两个方向)
///   ① **文档里点不到的名字**(死名字):`helix.X` / `helix.ns.member` 必须能在
///      `members` / 命名空间名 / 少数**属性**与**插件补的名字**白名单里找到;
///   ② **注册表有、文档没记**:每个成员要么以 `ns.member` 全名出现,要么出现在文档的
///      命名空间索引块里(`helix.<ns>.*   a, b, c`);根成员以裸名出现即可。
///   ③ 事件:注册表里的每个事件名都必须在文档里出现。
///
/// ## 用法
///   node plugins/tools/check_doc_surface.mjs     # 退出码 0 = 全绿,1 = 有漂移
///
/// 已挂进 `cargo xtask api-check`(CI 的 "Check plugin API surface" 步骤会跑到)。

import { readFileSync, existsSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { join, dirname, isAbsolute } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const rel = (p) => join(ROOT, p);
const read = (p) => readFileSync(isAbsolute(p) ? p : rel(p), "utf8");

const DOC = process.env.HELIX_DOC_OVERRIDE || "docs/plugin-api.md";

/// 注册表里**不是函数成员**、文档有权提到的名字。
/// `api_version` / `permissions` 是 `builder.property(...)` 注册的(root 属性),
/// api-check 的成员解析只看 `.function(...)` ⇒ 这里显式放行。
const PROPERTY_ALLOW = new Set(["api_version", "permissions"]);

/// **插件补的名字**(注册表没有,但运行时确实存在 —— 见 api-check 的同类处理)。
/// 文档提到它们是合法的;新增时请同时在 `plugins/*/plugin.js` 里找到赋值处。
const PLUGIN_PROVIDED = new Set([
  "plugin.install",
  "plugin.update",
  "plugin.remove",
  "pane.open",
]);

const problems = [];
const fail = (m) => {
  problems.push(m);
  console.log("  ✗ " + m);
};
const ok = (m) => console.log("  ✓ " + m);
const section = (m) => console.log("\n" + m);

/// —— 权威面:复用 api-check 的 json 输出(它是唯一与 Rust 注册表对账过的来源)——
function registry() {
  const script = rel("plugins/tools/check_api_surface.mjs");
  if (!existsSync(script)) {
    console.log(`  ✗ 找不到 ${script}(无法取得权威 API 面)`);
    process.exit(1);
  }
  const out = execFileSync("node", [script, "--json"], {
    cwd: ROOT,
    encoding: "utf8",
    maxBuffer: 32 * 1024 * 1024,
  });
  // 防御:即使脚本在某些分支往 stdout 打了提示,也只取 JSON 主体
  const start = out.indexOf("{");
  const end = out.lastIndexOf("}");
  if (start < 0 || end < start) {
    console.log("  ✗ api-check --json 输出里找不到 JSON 主体");
    process.exit(1);
  }
  return JSON.parse(out.slice(start, end + 1));
}

const api = registry();
const members = api.members || {};
const events = new Set(api.events || []);
const namespaces = new Set(Object.keys(members));
const flat = new Set(members.root || []);
// 可调用根名(`helix.plugin(...)` 是 FunctionObjectBuilder 注册的,不在 members 里)
const callables = new Set(api.callables?.root || []);
const qualified = new Set();
for (const [ns, list] of Object.entries(members)) {
  if (ns === "root") continue;
  for (const m of list) qualified.add(`${ns}.${m}`);
}

const doc = read(DOC);

/// 文档里出现的 `helix.X` / `helix.ns.member`(去掉文件名与**占位符**误伤)
function docMentions() {
  const out = new Set();
  // 先剥掉两类非 API 写法:
  //   · 文件名(`helix.d.ts` / `helix.toml` / `helix.json` …);
  //   · 说明"名字形态"的占位符(`helix.X` / `helix.ns.X` —— 本文档的复核段落里就出现过)。
  const cleaned = doc
    .replace(/helix\.(?:d\.ts|toml|json|md)\b/g, "helix_FILE")
    .replace(/helix\.(?:[A-Z]|ns\.)/g, "helix_PLACEHOLDER");
  for (const m of cleaned.matchAll(/helix\.([a-z_][a-z0-9_]*(?:\.[a-z_][a-z0-9_]*)?)/g)) {
    out.add(String(m[1]));
  }
  return out;
}

/// 文档的命名空间索引块:`helix.<ns>.*   a, b, c`
function indexPath() {
  const idx = new Map();
  for (const m of doc.matchAll(/helix\.([a-z_][a-z0-9_]*)\.\*\s+([^\n]+)/g)) {
    idx.set(m[1], new Set(m[2].split(",").map((s) => s.trim()).filter(Boolean)));
  }
  return idx;
}

const mentions = docMentions();
const idx = indexPath();

section("── ① 文档里点不到的名字(死名字 / 手误)──");
{
  const dead = [];
  for (const name of [...mentions].sort()) {
    if (flat.has(name) || qualified.has(name) || namespaces.has(name)) continue;
    if (callables.has(name)) continue;
    if (PROPERTY_ALLOW.has(name) || PLUGIN_PROVIDED.has(name)) continue;
    dead.push(name);
  }
  if (dead.length) {
    for (const d of dead) {
      fail(`${DOC}: helix.${d} 在注册表里不存在(已删/改名?先跑 api-check 看真名)`);
    }
  } else {
    ok(`文档提到的 ${mentions.size} 个 helix.* 名字都能在注册表/白名单里找到`);
  }
}

section("── ② 注册表有、文档没记 ──");
{
  const missing = [];
  const wordIn = (name) => new RegExp(`\\b${name}\\b`).test(doc);
  for (const name of flat) if (!wordIn(name)) missing.push(name);
  for (const [ns, list] of Object.entries(members)) {
    if (ns === "root") continue;
    for (const m of list) {
      if (doc.includes(`${ns}.${m}`)) continue;
      if (idx.get(ns)?.has(m)) continue;
      missing.push(`${ns}.${m}`);
    }
  }
  if (missing.length) {
    for (const m of missing) {
      fail(`${DOC}: 注册表有 ${m},文档没记(加到速查索引或命名空间索引块)`);
    }
  } else {
    ok(`注册表 ${flat.size + qualified.size} 个成员都有文档条目`);
  }
}

section("── ③ 事件(EventName ↔ EVENT_WHITELIST)──");
{
  const missing = [...events].filter((e) => !doc.includes(e)).sort();
  if (missing.length) {
    for (const e of missing) fail(`${DOC}: 事件 ${e} 没在文档里出现`);
  } else {
    ok(`${events.size} 个事件都在文档里`);
  }
}

section("── 汇总 ──");
console.log(
  `  注册表:${flat.size + qualified.size} 个成员 / ${namespaces.size - 1} 个命名空间(不含根) · ${events.size} 个事件`,
);
if (problems.length) {
  console.log(`\n✗ 有漂移(${problems.length} 处):${DOC} 与注册表不一致`);
  console.log(
    "\n修法:删掉文档里已不存在的名字 / 把缺的成员补进速查索引(命名空间成员可补进" +
      "「命名空间与事件索引」块)。插件补的名字请加到本脚本的 PLUGIN_PROVIDED 白名单。",
  );
  process.exit(1);
}
console.log("\n✓ 全绿:docs/plugin-api.md 与注册表一致");
