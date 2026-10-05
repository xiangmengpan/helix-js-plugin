# 插件布局与命名约定

> 目的:定下**一个插件长什么样**(目录、文件名、入口、依赖、覆盖),让"随软件分发的内置插件"
> 与"用户自己写的插件"是**同一套规则下的两层**,而不是两套东西。
> 参考:Neovim 的 `runtimepath` / `plugin/` / `autoload/` / `after/` / `pack/*/{start,opt}`。

## 1. 先看 Neovim 怎么做的(它的核心是三条)

| 机制 | 作用 | 对我们的意义 |
|---|---|---|
| `runtimepath`(**有序多根**) | 自带 runtime 在前、用户目录在后,**后者覆盖前者** | ✅ 已实现(`PLUGIN_ROOTS` + `resolve_in`,自带在前用户殿后) |
| `plugin/` 子目录**约定** | 每个根下同名的子目录结构,扫描即可发现 | 我们要的"布局约定"就是这一层 |
| `after/` | 排在所有根之后,专门用来**覆盖**别人 | 我们用"多根顺序"替代,更简单 |
| `pack/*/start` vs `opt` + `:packadd` | 自动加载 vs 按需加载 | 暂不要(见 §7) |
| `autoload/` | 函数体**按名延迟**加载 | 暂不要(需要懒加载语义,见 §7) |

**关键收获**:Neovim 的插件树不是"一个文件",而是"**一个目录 + 固定子结构**"。

## 2. 现状的三个不一致(这是要消掉的东西)

| 不一致 | 实例 |
|---|---|
| 入口名两种 | `features/tabbar.js`(散文件) vs `features/arsenal/index.js`(目录) |
| `init.js` 点名用**文件路径** | `helix.load("features/filetree/index.js")` —— 改目录结构就得改 init |
| 依赖用**文件路径** | `helix.plugin("filetree", { deps: ["lib/icons.js"] })` —— 布局与依赖耦合 |

## 3. 建议约定(直接可执行的规则)

### 3.1 一个插件 = **一个目录**

```
<插件根>/                        # 一个「根」= 一份可覆盖的插件集合
├── init.js                      # 可选:该根的入口(用户优先,自带兜底)
├── <name>/                      # ← 插件单位。目录名 = 插件名,发布后不改
│   ├── plugin.js                #   入口(**必需**,固定名)
│   ├── plugin.json              #   可选 manifest(见 §5)
│   ├── lib/                     #   该插件**私有**模块(仅本插件 require)
│   └── tests/                   #   该插件自己的 node 测试
├── lib/                         # 跨插件**共享**库(如 icons 薄壳)
└── examples/                    # 示例/模板 —— **不参与加载**
```

### 3.2 命名规则(逐条可判定)

| 对象 | 规则 | 反例(要消除的) |
|---|---|---|
| 插件目录 | **kebab-case 的插件名**:`filetree/` `server-manager/` `which-key/` | `picker.js`(散文件)、`lsp-hover/index.js`(目录名与入口不一) |
| 入口文件 | **一律 `plugin.js`** | `index.js` / `<name>.js` / `main.js` |
| 共享库 | **只在根 `lib/`**,不放进某个插件 | 每个插件自带一份 icons |
| 示例 | **只在 `examples/`** | 模板混在 `features/` 里(已修) |
| 插件名 | 与 `helix.plugin("<name>")` 的 name **一致**;与目录名一致 | 目录 `filetree` 但 name 叫别的 |

### 3.3 入口语义

```js
// <root>/init.js —— 只做"点名",不再写文件路径
helix.load("filetree");        // 加载 <root>/filetree/plugin.js
helix.load("which-key");
```

- `helix.load("<name>")` —— 名字**不带 `/` 且不带 `.js`** 时,按**插件名**解析 → `<root>/<name>/plugin.js`
- `helix.load("<path>.js")` —— 带 `.js` 时仍按**文件路径**(后门:一次性脚本、调试)
- 顺序:**根序(自带 → 用户)→ 根内 `init.js` 用户优先**

### 3.4 依赖用**插件名**,不用文件路径

```js
helix.plugin("filetree", { deps: ["icons"] });   // ✅ 名字
// helix.plugin("filetree", { deps: ["lib/icons.js"] });   // ❌ 路径耦合
```

### 3.5 覆盖是**目录级**,不是文件级

用户在 `~/.config/helix/plugins/filetree/` 放了自己的版本 → 加载器**整个忽略**
自带 runtime 里的 `filetree/`。

> **为什么必须目录级**:文件级覆盖会造出"混合体" —— 用户的 `plugin.js` + 自带的 `lib/`,
> 两边版本不匹配,而且用户根本不知道自己"继承"了什么。目录级覆盖语义单一:
> **有就是全有,没有就是全用内置。**

## 4. 加载器需要改的三点(这是本约定的实现代价)

| # | 改动 | 现状 |
|---|---|---|
| 1 | ~~**名字 → 目录**解析~~ | ✅ **已实现**(`state::entry_keys` + `load_script_checked`;`<name>.js` 仍可加载,向后兼容;集成测试 `bare_plugin_name_loads_plugin_entry_file` 端到端验收) |
| 2 | **目录级覆盖**:命中用户的 `<name>/` 后,不再回落自带的同名目录 | ⬜ 仍是**文件级**(`resolve_in` 从后往前找第一个存在的文件) |
| 3 | `deps` 接受**插件名**(内部转成 `<name>/plugin.js`) | ⬜ 现只接受文件 key(但**裸名现在已能解析到插件入口**,所以主要差在文档与校验) |

改动集中在 `state::resolve_in` 与 `commands::load_script_checked` 两处 —— 都是纯函数/小函数,
且 `resolve_in` 已有 4 种情形的单测可直接扩展。

## 5. 可选:`plugin.json`(manifest)

```json
{ "name": "filetree", "version": "1.0",
  "deps": ["icons"],
  "contributes": { "commands": ["filetree","filetree-reveal"] } }
```

- **只做"能零执行就读到的声明"**:插件名/版本/依赖/提供哪些命令
- 用途:`:plugin list` 清单、冲突检测(两个插件抢同一个命令名)、将来做懒加载
- 与代码的关系:`helix.plugin(name, {deps})` 保留为**运行时校验** —— manifest 与代码不一致就报错
- 不要现在做:它只有在"有第三方插件/需要懒加载"时才回本(见 §7)

## 6. 迁移步骤(现有 9 个插件 → 本约定)

| 现状 | 迁到 |
|---|---|
| `features/tabbar.js` `statusline.js` `terminal.js` | `<name>/plugin.js`(**待迁**) |
| `features/which-key.js` | ✅ **已迁** → `which-key/plugin.js`,init 改为 `helix.load("which-key")` |
| `features/arsenal/index.js` `filetree/index.js` | 改名 `index.js` → `plugin.js` |
| `examples/lsp-hover.js` `picker.js` | 保持(已在 `examples/`) |
| `lib/icons.js` | 保持(它是根级共享库;等 §6.1 落地后甚至可删) |
| `init.js` 的 `helix.load("features/x/index.js")` | `helix.load("x")` |

**建议分两步**:① 先加"名字→目录"解析(向后兼容:`features/x.js` 仍可加载);
② 再逐个 \(插件\) 迁目录 + 改 init(每迁一个跑该插件的测试 —— 本会话证明这类机械改动最容易坏)。

## 7. 为什么**不**照搬 Neovim 的全部

| Neovim 机制 | 不做的理由 |
|---|---|
| `pack/*/start` + `opt` + `:packadd` | 需要"启用/禁用"的用户界面与状态持久化;我们现在只有 9 个插件,收益不抵复杂度 |
| `autoload/`(按名延迟) | 需要"函数体延迟加载"与"名字→文件"映射两边同时改;留到有性能/启动时间实测需求时再做 |
| `after/` 目录 | 与"后加的根覆盖先加的"重复;多根顺序已经能表达,少一个概念 |

**一句话**:Neovim 的布局约定(§3)**现在就抄**;它的加载策略(§7)**先不抄**。
