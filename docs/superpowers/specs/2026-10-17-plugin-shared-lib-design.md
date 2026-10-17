# G9（C 批）· JS 插件共享层 `plugins/lib/` 设计规格

日期:2026-10-17
状态:**待审查**(铁律 5:C 批先出 spec,批准前不动代码)
关联:[`../architecture-audit.md`](../../architecture-audit.md) §5 G9 · [`../../plugin-layout.md`](../../plugin-layout.md) · [`../../plugin-api.md`](../../plugin-api.md)

## 0. 决策记录(需要你拍板的三点)

1. **原 G9 的收益被实测高估**。体检说「`wc_*` 等重复助手 → 共享层」,实测**跨插件完全相同的函数只有 4 个 / 9 份副本 / 共 35 行**(见 §2.1)。
2. 因此本 spec 的主结论是:**建议不做 G9 的原形态**,改为二选一(§3):
   - **方案 A(推荐)**:不做。把实测结论写回 `architecture-audit.md`,G9 关闭为「已评估,收益不足」。
   - **方案 B(可选,~1 小时)**:只把 4 个**逐字相同**的助手抽到 `plugins/lib/ui.js`,并**确立** `plugins/lib/` 这个约定(为将来铺路)。
3. **明确不做**:统一「同名但实现不同」的 5 个函数(`render` / `handle_key` / `row_el` / `move` / `current_cfg`)。它们是各自面板的交互骨架,合并属于行为重构(C 批里风险/收益比最差的一类),而且 `handle_key` 两份分别是 110 / 44 行。

## 1. 目标

判断「JS 插件之间该不该有共享层」,若该有,给出**最小**落法;若不该有,给出**可复现的证据**让这件事一次性结案(避免下一个人再来一遍)。

## 2. 现状与约束(已核实)

### 2.1 重复度量(方法:抽每个 `plugins/*/plugin.js` 的顶层 `function` 体,去注释+压空白+哈希)

13 个插件、115 个顶层函数。结果:

| 类别 | 数量 | 明细 |
|---|---|---|
| **逐字相同**(归一化后哈希一致) | **4 组 / 9 份 / 35 行** | `guard`(7 行 ×2:dashboard, picker-ui)<br>`say`(3 行 ×2:dashboard, picker-ui)<br>`shq`(3 行 ×3:filetree, plugin, statusline)<br>`hasProcess`(3 行 ×2:plugin, statusline) |
| **同名但实现不同** | 5 个名字 | `render` ×3(filetree 16 / picker-ui 23 / statusline 77 行)<br>`handle_key` ×2(arsenal 110 / filetree 44)<br>`row_el` ×2(16 / 11)· `move` ×2(5 / 3)· `current_cfg` ×2(3 / 3) |

结论:能**零风险**合并的只有 35 行;剩下 300+ 行的「重复」是三个面板各自的交互骨架,形似而神不同。

### 2.2 插件布局的硬约束(改 `plugins/` 必须遵守)

- 目录名 = 插件名 = `helix.plugin("<name>")`,kebab-case,发布后不改;入口一律 `plugin.js`。
- 用户层 `~/.config/helix/plugins/<name>/` 是**目录级整体覆盖** —— 共享层若被放在某个插件目录里,用户覆盖那个插件就会**连带弄坏所有依赖方**。
- 因此共享层只能放 `plugins/lib/`(独立目录、不是插件),由各插件用**相对名** `helix.load("lib/ui.js")` 或 `deps: ["lib/ui.js"]` 拉取(相对名解析走的正是 `plugins/<root>/…` 多根查找)。
- 改完必须:`sh contrib/install-plugins.sh`(否则跑的是 `runtime/plugins/` 旧副本)+ `cargo xtask api-check` + `tsc --noEmit -p plugins/jsconfig.json`。
- **插件侧没有模块系统**:`helix.export` / `helix.load` 返回的是值;共享层只能导出对象字面量,没有 tree-shaking、没有按需加载,每个插件都要显式 `helix.load`。

### 2.3 API 面影响

抽共享层**不新增**任何 `helix.*` API(`helix.d.ts` / 文档 / 能力检查点三闸门不受影响);但若共享层使用了 `run`/`run_async` 等能力,能力检查点仍按**插件**登记,需要确认 `plugins/lib/ui.js` 自身要不要 `helix.plugin()` 声明 —— **方案 B 的约定是:共享层不声明 plugin、不申请能力,纯函数**。

## 3. 方案

### 方案 A(推荐):不做,记录结案

- 动作:在 `architecture-audit.md` 的 G9 条目写入 §2.1 的度量表,状态改「已评估,收益不足」。
- 理由:35 行 / 13 个插件(0.8%)的收益,换来的是「每个插件多一行 `helix.load`、多一个用户可覆盖目录、多一处 reload 顺序耦合」;
  `--ignore` 掉的 `reload_*` 测试刚好证明**加载顺序与覆盖**是这个仓库的高频事故点。

### 方案 B(可选):只抽 4 个逐字相同的助手

```
plugins/lib/ui.js          # 新增:非插件目录,不声明 helix.plugin()
  export function guard(fn) { … }        (7 行,原样搬)
  export function say(msg) { … }         (3 行,原样搬)
  export function shq(s) { … }           (3 行,原样搬)
  export function hasProcess() { … }     (3 行,原样搬)
```
消费方(dashboard / picker-ui / filetree / plugin / statusline):
```js
const UI = helix.load("lib/ui.js");   // 相对名
helix.plugin("<name>", { deps: ["lib/ui.js"], … });
```
- 5 个插件各改 2 行(deps + load),删本地 1–3 个函数。
- **收益**:−35 行 + 确立约定;**代价**:5 个插件多一个加载依赖。
- 完成判据:`grep -c "function guard\|function say\|function shq\|function hasProcess" plugins/*/plugin.js` = 0;`sh contrib/install-plugins.sh` 后跑 377 条集成测试不变;`tsc --noEmit` 绿;api-check 三段绿。

## 4. 边界与错误处理

- 共享层**不得**吞错:函数保持纯,不做 try/catch(方案 B 的四函数原样搬,行为零变化)。
- 共享层**不得**申请能力(`permissions`),否则能力检查点会把它当插件入口登记。
- 若将来真的出现「一个大到值得独立的 JS 能力」(例如 diff 算法),应走 **Rust 侧原生 API + `helix.d.ts`**,而不是 JS 共享层 —— 那样才进三道门禁。

## 5. 测试计划

| 方案 | 验证 |
|---|---|
| A(不做) | 无代码改动;`architecture-audit.md` 更新 + 本 spec 存档 |
| B(抽 4 个) | `sh contrib/install-plugins.sh` → `cargo integration-test`(377)· `cargo xtask api-check`(三段)· `tsc --noEmit -p plugins/jsconfig.json` · 手工冒烟:`:plugin-reload` 后 dashboard / picker / filetree / statusline 命令仍可用 |

## 6. 里程碑

| 阶段 | 内容 | 预估 |
|---|---|---|
| M1 | 拍板 A / B | 你,一次 |
| M2(仅 B) | 建 `plugins/lib/ui.js` + 5 个插件各改 2 行 | 30–60 分钟 |
| M3(仅 B) | 安装副本 + 全量验证 | 30 分钟 |

## 7. 风险

| 风险 | 影响 | 缓解 |
|---|---|---|
| 用户覆盖 `plugins/lib/` | 所有依赖方一起坏 | 文档写明「别覆盖 lib」;方案 A 连这也省了 |
| 加载顺序(reload 时 lib 未就绪) | `UI.guard is not a function` | 每个消费方在 `helix.plugin()` 里声明 `deps`(引擎保证先加载) |
| 把「形似」当「相同」去合并 | 面板交互回归,且测试覆盖不到 | 方案 B 只合并**哈希一致**的 4 个;同名不同实现一律不动 |

---

**附:复现命令**

```bash
python3 - <<'PY'
import re, glob, hashlib, collections
def items(path):
    src=open(path).read(); out=[]
    for m in re.finditer(r'^(?:async\s+)?function\s+(\w+)\s*\(([^)]*)\)\s*\{', src, re.M):
        j=src.index('{', m.end()-1); d=0
        for k in range(j,len(src)):
            if src[k]=='{': d+=1
            elif src[k]=='}':
                d-=1
                if d==0: break
        norm=re.sub(r'\s+',' ',re.sub(r'//.*','',src[j:k+1]))
        out.append((m.group(1), src[j:k+1].count(chr(10))+1, hashlib.sha1(norm.encode()).hexdigest()[:8]))
    return out
c=collections.Counter(h for p in glob.glob('plugins/*/plugin.js') for _,_,h in items(p))
print([h for h,n in c.items() if n>1])
PY
```
