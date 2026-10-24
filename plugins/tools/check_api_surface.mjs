#!/usr/bin/env node
/// helix.d.ts ↔ JS API 注册表 **自动对账**(可复跑 ✓)
///
/// 由来:`plugins/helix.d.ts` 是**手工维护**的类型定义,而 API 面由 Rust 侧注册。
/// 两边没有任何自动约束 ⇒ 会出现两类必然漂移:
///   ① 注册表有、d.ts 没有 ⇒ 插件作者**不知道有这个 API**(能力被埋没)
///   ② d.ts 有、注册表没有 ⇒ d.ts **说谎**(照它写代码 = 运行期 undefined)
///   本仓库已有一次实证:探针测出 `helix.current_buffer` 是 undefined,而 d.ts 曾把它写成可用。
///   ⇒ 把对账固化成脚本(与 check_signatures.mjs 同一思路:凡是"靠人记得"的步骤最后一定被忘掉)。
///
/// ## 权威来源(全部**从源码解析**,不需要 cargo 编译)
///   1. `helix-js/src/lib.rs` + `helix-js/src/register/*.rs`
///                                 —— `helix.*` 成员:根名 + 各命名空间成员 + 全局量(helix/TABBAR_ID)
///                                    (G6 起成员按命名空间拆到 register/ 下,lib.rs 只留拼装)
///   2. `helix-js/src/icons.rs`    —— `helix.icons.*`(经 `native_fns()` **动态**注册,lib.rs 里看不到名字)
///   3. `helix-js/src/commands.rs` —— `EVENT_WHITELIST`(`helix.on` 允许的事件名)
///
/// ## 被检查对象
///   `plugins/helix.d.ts` + 镜像副本 `runtime/plugins/helix.d.ts`(副本漂移是本仓库头号事故)
///
/// ## 第三类名字:插件补的(不算漂移)
/// JS 插件可以给命名空间加属性 —— 例如 `helix.pane.open`(`plugins/pane-open/plugin.js`)、
/// `helix.plugin.install`(`plugins/plugin/plugin.js`)。这类**注册表里没有但确实存在**的名字
/// d.ts 有权利声明 ⇒ 单独列为 ℹ 插件提供,不计入失败。
///
/// ## 用法
///   node plugins/tools/check_api_surface.mjs        # 退出码 0 = 全绿,1 = 有漂移
///
/// 范围:**只对名字**,不查签名/arity(参数个数与类型仍靠人工 + `tsc --noEmit`)。

import { readFileSync, readdirSync, statSync, existsSync } from "node:fs";
import { join, dirname, isAbsolute } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const rel = (p) => join(ROOT, p);
const read = (p) => readFileSync(isAbsolute(p) ? p : rel(p), "utf8");

/// 被检查的 d.ts(`HELIX_DTS_OVERRIDE` 仅供自检用:拿一份合成的“应该全绿”的文件跑一遍)
const DTS_PATH = process.env.HELIX_DTS_OVERRIDE || "plugins/helix.d.ts";
const JSON_MODE = process.argv.includes("--json");

/// ── 把注释替换成等长空白(保留偏移与行号)──────────────────────────
/// 必须做:d.ts 的文档注释里含 `;` / `{}` / `helix.xxx`,直接扫描会被带偏。
function maskComments(src) {
  let out = "";
  let i = 0;
  while (i < src.length) {
    if (src.startsWith("//", i)) {
      const j = src.indexOf("\n", i);
      const end = j < 0 ? src.length : j;
      out += " ".repeat(end - i);
      i = end;
    } else if (src.startsWith("/*", i)) {
      const j = src.indexOf("*/", i + 2);
      const end = j < 0 ? src.length : j + 2;
      out += src
        .slice(i, end)
        .replace(/[^\n]/g, " ");
      i = end;
    } else if (src[i] === '"' || src[i] === "'" || src[i] === "`") {
      // 字符串字面量原样保留(事件名/API 名都在里面)
      const q = src[i];
      let j = i + 1;
      while (j < src.length && src[j] !== q) j += src[j] === "\\" ? 2 : 1;
      out += src.slice(i, Math.min(j + 1, src.length));
      i = j + 1;
    } else {
      out += src[i];
      i++;
    }
  }
  return out;
}

const lineOf = (text, idx) => text.slice(0, idx).split("\n").length;

/// 从 openIdx(指向 `{`）找到配对的 `}` 下标(相对同一字符串)
function matchBrace(text, openIdx) {
  let d = 0;
  for (let i = openIdx; i < text.length; i++) {
    if (text[i] === "{") d++;
    else if (text[i] === "}") {
      d--;
      if (d === 0) return i;
    }
  }
  return -1;
}

// ══════════════════════════ Rust 侧:解析注册表 ══════════════════════════

/// 返回 { members: Map<ns, Map<name, {src, line}>>, callables: Map<name,...>,
///        globals: Map<name,...>, events: Map<name,...> }
function parseRustRegistry() {
  // **唯一数据源**：`helix-js/src/api.rs` 的表 → `cargo xtask api-describe` 导出的 JSON。
  //
  // 历史：这里曾经用 700 行正则解析 `lib.rs` + `register/*.rs`（容器归属、成员、挂载、
  // 全局量、icons 动态名、事件白名单…）。那张表存在之后，解析源码就成了"第二份真相"：
  // 表改了脚本未必认得出（变量改名/换文件都会失明）。现在只读 JSON。
  const src = "target/api-surface.json";
  if (!existsSync(rel(src))) {
    console.log(`  ✗ 缺 ${src}（先跑 cargo xtask api-describe）`);
    process.exit(1);
  }
  const surface = JSON.parse(read(src));
  const members = new Map(); // ns → Map(name → info)；"root" = helix 根上的函数
  const callables = new Map(); // 函数对象根名（helix.plugin）
  const globals = new Map(); // 引擎全局量（TABBAR_ID）
  const attached = new Set(); // 出现过的命名空间
  const push = (map, ns, name, info) => {
    if (!map.has(ns)) map.set(ns, new Map());
    map.get(ns).set(name, info);
  };
  for (const m of surface.members) {
    const info = { src: "helix-js/src/api.rs", line: 0, where: m.where, rust: m.fname || m.name };
    if (m.global) {
      globals.set(m.name, info);
      continue;
    }
    if (m.ns) {
      push(members, m.ns, m.name, info);
      attached.add(m.ns);
      continue;
    }
    if (m.kind === "function_object") push(callables, "root", m.name, info);
    else push(members, "root", m.name, info);
  }
  for (const g of surface.globals || []) {
    globals.set(g, { src: "helix-js/src/api.rs", line: 0, where: `global: ${g}` });
  }
  const events = new Map(
    (surface.events || []).map((e) => [e, { src: "helix-js/src/api.rs", line: 0, where: `events: ${e}` }]),
  );
  return { members, callables, globals, events, attached };
}

// ══════════════════════════ d.ts 侧:解析声明 ══════════════════════════

/// 从一个 interface 体里取深度 1 的成员名;`name: {…}` 递归取子成员,`name: IFace` 记录引用
function extractMembers(body, base) {
  const out = [];
  let i = 0;
  while (i < body.length) {
    // 调用签名(`(name: string, opts: {…}): void;`)没有名字,必须整段跳过 ——
    // 否则里面 `opts: {…}` 会被当成成员扫出来(false positive)。
    if (body[i] === "(") {
      let d = 0;
      let j = i;
      for (; j < body.length; j++) {
        if (body[j] === "(") d++;
        else if (body[j] === ")") {
          d--;
          if (d === 0) break;
        }
      }
      const semi = body.indexOf(";", j);
      i = semi < 0 ? j + 1 : semi + 1;
      continue;
    }
    const m = /^([A-Za-z_$][\w$]*)\s*(\(|:)/.exec(body.slice(i));
    const boundaryOk = i === 0 || /[\s;{},]/.test(body[i - 1]);
    if (!m || !boundaryOk) {
      i++;
      continue;
    }
    // 成员结束:深度 0 处的 `;` 或收尾的 `}`
    let j = i;
    let d = 0;
    let end = -1;
    while (j < body.length) {
      const c = body[j];
      if (c === "{") d++;
      else if (c === "}") {
        d--;
        if (d < 0) {
          end = j;
          break;
        }
      } else if (c === ";" && d === 0) {
        end = j;
        break;
      }
      j++;
    }
    if (end < 0) end = body.length;
    const seg = body.slice(i, end);
    const entry = { name: m[1], idx: base + i };
    if (m[2] === ":") {
      const ob = seg.indexOf("{");
      if (ob >= 0) {
        const cb = matchBrace(seg, ob);
        entry.children = extractMembers(seg.slice(ob + 1, cb), base + i + ob + 1);
      } else {
        const ref = /^[A-Za-z_$][\w$]*\s*:\s*([A-Za-z_$][\w$]*)/.exec(seg);
        if (ref) entry.ref = ref[1];
      }
    }
    out.push(entry);
    i = end + 1;
  }
  return out;
}

function parseDts() {
  const raw = read(DTS_PATH);
  const src = maskComments(raw);
  const at = (idx) => ({ src: DTS_PATH, line: lineOf(src, idx) });

  const ifaces = new Map(); // name -> {body, bodyOff, callable}
  for (const m of src.matchAll(/(?:export\s+)?interface\s+([A-Za-z_$][\w$]*)\s*\{/g)) {
    const open = src.indexOf("{", m.index);
    const close = matchBrace(src, open);
    const body = src.slice(open + 1, close);
    ifaces.set(m[1], { body, bodyOff: open + 1, callable: /^\s*\(/m.test(body) });
  }

  const main = ifaces.get("Helix");
  if (!main) {
    console.log("  ✗ 没在 helix.d.ts 里找到 `interface Helix`");
    return null;
  }

  const members = new Map([["root", new Map()]]);
  const callables = new Map();
  for (const e of extractMembers(main.body, main.bodyOff)) {
    if (e.children || e.ref) {
      // 命名空间 → 子成员;`foo: IFace` 则把该 interface 的成员算进来
      const srcObj = e.ref ? ifaces.get(e.ref) : null;
      const kids = e.children || (srcObj ? extractMembers(srcObj.body, srcObj.bodyOff) : []);
      const map = new Map();
      for (const k of kids) map.set(k.name, at(k.idx));
      members.set(e.name, map);
      if (srcObj && srcObj.callable) callables.set(e.name, at(e.idx));
    } else {
      members.get("root").set(e.name, at(e.idx));
    }
  }

  const events = new Map();
  const ev = /type\s+EventName\s*=([\s\S]*?);/.exec(src);
  if (ev) {
    for (const m of ev[1].matchAll(/"([^"]+)"/g)) {
      events.set(m[1], at(ev.index + ev[1].indexOf(m[0])));
    }
  } else {
    console.log("  ⚠ 自检:没在 helix.d.ts 里找到 `type EventName`");
  }

  const globals = new Map();
  for (const m of src.matchAll(/declare\s+const\s+([A-Za-z_$][\w$]*)\s*:/g)) {
    globals.set(m[1], at(m.index));
  }

  return { members, callables, events, globals };
}

// ══════════════════════ 第三类名字:内置插件补的属性 ══════════════════════

function parsePluginAugmentations() {
  const out = new Map(); // "ns/name" 或 "root/name" -> "plugins/x/plugin.js:12"
  const walk = (dir) => {
    for (const e of readdirSync(rel(dir))) {
      const p = join(dir, e);
      const st = statSync(rel(p));
      if (st.isDirectory()) {
        if (e !== "node_modules" && e !== "tools") walk(p);
      } else if (e.endsWith(".js")) {
        const src = maskComments(read(p));
        const note = (idx) => `${p}:${lineOf(src, idx)}`;
        for (const m of src.matchAll(/(?<![\w.$])helix\.([A-Za-z_$][\w$]*)\.([A-Za-z_$][\w$]*)\s*=(?!=)/g)) {
          out.set(`${m[1]}/${m[2]}`, note(m.index));
        }
        for (const m of src.matchAll(/(?<![\w.$])helix\.([A-Za-z_$][\w$]*)\s*=(?!=)/g)) {
          out.set(`root/${m[1]}`, note(m.index));
        }
      }
    }
  };
  if (existsSync(rel("plugins"))) walk("plugins");
  return out;
}

// ══════════════════════════════ 对账 ══════════════════════════════

const problems = [];
const infos = [];
const section = (title) => console.log(`\n${title}`);

/// 名字在**别处**存在时的提示:d.ts 的路径过时(≠ 名字根本不存在)—— 修法不同
function elsewhereHint(ns, name, rustAll, aug) {
  for (const [other, m] of rustAll) {
    if (other !== ns && m.has(name)) {
      return `另有 helix.${other === "root" ? "" : other + "."}${name} ⇒ **d.ts 路径过时**`;
    }
  }
  for (const key of aug.keys()) {
    const [ans, aname] = key.split("/");
    if (aname === name && ans !== ns) {
      return `由插件提供在 helix.${ans === "root" ? "" : ans + "."}${name} ⇒ **d.ts 路径过时**`;
    }
  }
  return "";
}

function compareSet(label, rustMap, dtsMap, opts = {}) {
  const missing = []; // 注册表有,d.ts 没有
  const stale = []; // d.ts 有,两边都没有
  const aug = []; // d.ts 有,插件补的
  for (const [name, info] of rustMap) if (!dtsMap.has(name)) missing.push([name, info]);
  for (const [name, info] of dtsMap) {
    if (rustMap.has(name)) continue;
    const key = `${opts.ns || "root"}/${name}`;
    if (opts.augmentations?.has(key)) aug.push([name, info, opts.augmentations.get(key)]);
    else stale.push([name, info]);
  }
  if (!missing.length && !stale.length && !aug.length) {
    console.log(`  ✓ ${label}:一致(${rustMap.size} 项)`);
  } else {
    if (missing.length) {
      console.log(`  ✗ ${label}:d.ts **缺** ${missing.length} 项(注册表有,d.ts 没有)`);
      for (const [n, i] of missing) console.log(`      ${n}  ← ${i.where || `${i.src}:${i.line}`}${i.rust ? ` (${i.rust})` : ""}`);
    }
    if (stale.length) {
      console.log(`  ✗ ${label}:d.ts **多** ${stale.length} 项(两边都没有 ⇒ 照它写会 undefined)`);
      for (const [n, i] of stale) {
        const hint = elsewhereHint(opts.ns || "root", n, opts.rustAll || new Map(), opts.augmentations || new Map());
        console.log(`      ${n}  ← ${i.where || `${i.src}:${i.line}`}${hint ? `  → ${hint}` : ""}`);
      }
    }
    if (aug.length) {
      console.log(`  ℹ ${label}:${aug.length} 项由插件提供(不算漂移)`);
      for (const [n, i, p] of aug) console.log(`      ${n}  ← ${i.where || `${i.src}:${i.line}`} ← 提供者 ${p}`);
    }
    if (missing.length || stale.length) problems.push(`${label}: ${missing.length} 缺 / ${stale.length} 多`);
    if (aug.length) infos.push(`${label}: ${aug.length} 项由插件提供`);
  }
  return missing.length + stale.length;
}

function main() {
  const rust = parseRustRegistry();

  // `--json`:导出注册表(权威面)供 CI/diff/生成夹具用 —— 不做对比,直接退出
  if (JSON_MODE) {
    const dump = (m) => Object.fromEntries([...m].map(([ns, v]) => [ns, [...v.keys()].sort()]));
    console.log(
      JSON.stringify(
        {
          members: dump(rust.members),
          callables: dump(rust.callables),
          globals: [...rust.globals.keys()].sort(),
          events: [...rust.events.keys()].sort(),
        },
        null,
        1,
      ),
    );
    return;
  }

  console.log("helix.d.ts ↔ JS API 注册表 对账");
  console.log(
    "权威来源:helix-js/src/lib.rs + register/*.rs · icons.rs · commands.rs(从源码解析,不编译)",
  );

  const dts = parseDts();
  if (!dts) process.exit(1);
  const aug = parsePluginAugmentations();

  // 每个命名空间单独对账(root + 各 ns);两侧键的并集都要覆盖
  const nsNames = [...new Set([...rust.members.keys(), ...dts.members.keys()])].sort();
  for (const ns of nsNames) {
    const label = ns === "root" ? "helix.*" : `helix.${ns}.*`;
    compareSet(label, rust.members.get(ns) || new Map(), dts.members.get(ns) || new Map(), {
      ns,
      augmentations: aug,
      rustAll: rust.members,
    });
  }

  section("── 可调用根名(helix.plugin(…))──");
  compareSet("可调用", rust.callables.get("root") || new Map(), dts.callables, {});

  section("── 全局量(declare const …)──");
  compareSet("全局量", rust.globals, dts.globals, {});

  section("── 事件白名单(EVENT_WHITELIST ↔ type EventName)──");
  compareSet("事件", rust.events, dts.events, {});

  section("── 镜像副本(仓库头号事故:改了源忘了同步)──");
  const mirror = "runtime/plugins";
  const MIRROR_SKIP = ["tools"]; // 开发脚本不随软件分发(见 contrib/install-plugins.sh)
  // “已安装”的判据:**镜像里有成规模的 `.js`**(≥5;`install-plugins.sh` 会装 20+)。
  // 为什么**不用**“有没有 helix.d.ts”:那个文件被**误跟踪过一次**(2026-10-13 的提交把本机镜像
  // 加进了 git)⇒ 裸 checkout 里它存在 ⇒ 走严格比对 ⇒ 报一片“镜像缺文件”而**假红**(CI 实测)。
  // 也不用“目录是否存在”:`runtime/plugins/` 有历史遗留的跟踪文件 `one-dark.js`。
  const looksInstalled = (dir) => {
    if (!existsSync(rel(dir))) return false;
    let n = 0;
    const walk = (d) => {
      for (const e of readdirSync(rel(d))) {
        if (MIRROR_SKIP.includes(e)) continue;
        const p = join(d, e);
        if (statSync(rel(p)).isDirectory()) walk(p);
        else if (e.endsWith(".js")) n++;
      }
    };
    walk(dir);
    return n >= 5;
  };
  if (DTS_PATH !== "plugins/helix.d.ts") {
    console.log("  · 跳过:使用了 HELIX_DTS_OVERRIDE(自检模式)");
  } else if (!looksInstalled(mirror)) {
    console.log(
      `  · 跳过:${mirror} 看起来没装运行副本(没有成规模的 .js)⇒ 装了才查:` +
        "contrib/install-plugins.sh <runtime>",
    );
  } else {
    // 整棵树逐文件比(不只 d.ts)——“改了源忘了同步”是这个目录的通病
    const listFiles = (dir, base = "") => {
      const out = [];
      for (const e of readdirSync(rel(dir))) {
        if (MIRROR_SKIP.includes(e)) continue;
        const p = `${base}${e}`;
        if (statSync(rel(join(dir, e))).isDirectory()) out.push(...listFiles(join(dir, e), `${p}/`));
        else out.push(p);
      }
      return out;
    };
    const srcFiles = new Set(listFiles("plugins"));
    const dstFiles = new Set(listFiles(mirror));
    const differs = [...srcFiles].filter(
      (f) => dstFiles.has(f) && read(`plugins/${f}`) !== read(`${mirror}/${f}`),
    );
    const onlySrc = [...srcFiles].filter((f) => !dstFiles.has(f));
    if (!differs.length && !onlySrc.length) {
      console.log(`  ✓ ${mirror} 与 plugins/ 逐文件一致(${srcFiles.size} 个文件)`);
    } else {
      for (const f of differs) console.log(`  ✗ 内容不同:${mirror}/${f}`);
      for (const f of onlySrc) console.log(`  ✗ 镜像缺文件:${mirror}/${f}`);
      console.log("      同步: contrib/install-plugins.sh <runtime> 或 plugins/tools/sync_runtime.sh");
      problems.push(`镜像副本与源不一致(${differs.length} 内容不同 / ${onlySrc.length} 缺失)`);
    }
  }

  // ── 汇总 ──
  section("── 汇总 ──");
  const total = [...rust.members.values()].reduce((s, m) => s + m.size, 0);
  // 命名空间数**不含根**（与 check_doc_surface.mjs 同一口径；`rust.members` 含根 `helix`）
  console.log(`  注册表:${total} 个成员 / ${rust.members.size - 1} 个命名空间 · ${rust.events.size} 个事件`);
  for (const i of infos) console.log(`  ℹ ${i}`);
  if (problems.length) {
    console.log(`\n✗ 有漂移(${problems.length} 处):`);
    for (const p of problems) console.log(`    ${p}`);
    console.log("\n修法:补齐 d.ts 缺失项 / 删掉 d.ts 里不存在的名字(插件提供的名字请确认在 plugins/*/plugin.js 里确有赋值)");
    process.exit(1);
  }
  console.log("\n✓ 全绿:helix.d.ts 与注册表一致");
}

main();
