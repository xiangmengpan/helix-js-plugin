#!/usr/bin/env node
/// 插件签名/语法检查(可复跑 ✓)
///
/// 由来:本会话有一次**机械替换**把 `getModeIcon` 等 6 个函数的**参数名**改坏了 ✗,
/// 当时只做了一次性人工核对 ⇒ 事后没人能再跑一遍。本脚本把它固化下来 ✓。
///
/// 查三件事:
///   ① 所有插件 JS 的**语法** ✓(node --check 等价)
///   ② `plugins/icons/plugin.js` 的 **6 个图标函数**:存在性 + **参数名逐字一致** + 参数**在函数体内被用到**
///      (后者专门抓"参数被替换成别的名字"这类事故 ✓)
///   ③ 两个导出块(`helix.export` / `module.exports`)**导出同一组 6 个名字** ✓
///
/// 用法:`node plugins/tools/check_signatures.mjs`(退出码 0 = 全绿)

import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
let bad = 0;
const fail = (m) => { console.log("  ✗ " + m); bad++; };
const ok = (m) => console.log("  ✓ " + m);

/// —— 收集所有插件 JS ——
const files = [];
(function walk(d) {
  for (const e of readdirSync(d)) {
    const p = join(d, e);
    if (statSync(p).isDirectory()) { if (e !== "node_modules" && e !== "tools") walk(p); }
    else if (e.endsWith(".js") || e.endsWith(".mjs")) files.push(p);
  }
})(root);

/// —— ① 语法 ——
console.log(`① 语法(${files.length} 个文件)`);
for (const f of files) {
  try { new (async function(){}).constructor(readFileSync(f, "utf8")); }
  // 用 Function 构造器做**语法**检查(不执行 ⇒ 无副作用 ✓)
  catch (e) { fail(`${f.replace(root + "/", "")}: ${e.message}`); }
}
if (!bad) ok(`${files.length} 个文件语法均可解析 ✓`);

/// —— ② icons 的 6 个函数:参数名逐字一致 + 函数体内用到 ——
const ICONS = "icons/plugin.js";
const EXPECT = [
  ["getFileIcon", "path"],
  ["getDirIcon", "expanded"],
  ["getModeIcon", "mode"],
  ["getDiagnosticIcon", "severity"],
  ["getGitIcon", "status"],
  ["getCompletionKindIcon", "kind"],
];
console.log(`② ${ICONS} 的 6 个图标函数`);
const src = readFileSync(join(root, ICONS), "utf8");
for (const [name, param] of EXPECT) {
  const re = new RegExp(`function\\s+${name}\\s*\\(([^)]*)\\)\\s*\\{([\\s\\S]*?)\\n\\}`, "m");
  const m = re.exec(src);
  if (!m) { fail(`${name}: 未找到定义`); continue; }
  const got = m[1].trim();
  if (got !== param) { fail(`${name}(${got}) —— 期望 (${param})(**参数名被改过?**)`); continue; }
  // 函数体里必须真的用到这个参数(抓"形参名与使用处不一致" ✓)
  if (!new RegExp(`\\b${param}\\b`).test(m[2])) {
    fail(`${name}: 形参 \`${param}\` 在函数体内**未被使用**(疑似签名/使用处脱节)`);
    continue;
  }
  ok(`${name}(${param}) ✓ 且形参在体内被使用`);
}

/// —— ③ 两个导出块导出同一组名字 ——
console.log("③ 导出块一致性");
const grab = (re) => { const m = re.exec(src); return m ? m[1].split(/[,\s]+/).filter(Boolean) : null; };
const a = grab(/helix\.export\(\{([\s\S]*?)\}\)/);
const b = grab(/module\.exports\s*=\s*\{([\s\S]*?)\}/);
if (!a) fail("未找到 helix.export({…})");
if (!b) fail("未找到 module.exports = {…}");
if (a && b) {
  const sa = [...a].sort().join(","), sb = [...b].sort().join(",");
  if (sa !== sb) fail(`两处导出不一致:\n      helix.export: ${a}\n      module.exports: ${b}`);
  else ok(`两处均为同一组 ${a.length} 个:${a.join(", ")}`);
  const want = EXPECT.map(([n]) => n).sort().join(",");
  if (sa !== want) fail(`导出集合与 6 个图标函数不符(期望 ${want})`);
  else ok("导出集合 == 6 个图标函数 ✓");
}

console.log(bad === 0 ? "\n结果:全绿 ✓" : `\n结果:${bad} 处问题 ✗`);
process.exit(bad === 0 ? 0 : 1);
