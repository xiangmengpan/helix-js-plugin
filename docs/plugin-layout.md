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
| 2 | ~~**目录级覆盖**~~ | ✅ **已生效**:`resolve_in_with_root`(拿到提供根)+ `resolve_in_pref`(prefer 语义)+ `LOAD_ROOTS` 栈。**关键是 prefer 的对象必须是「父插件的根」而不是「文件自己的根」** —— 后者只会查回同一个文件,等于没生效(第一版就栽在这)。集成测试 scenario 4 端到端验证(内置入口 + 用户层同名 helper → helper 必须也是内置的) |
| 3 | ~~`deps` 接受**插件名**~~ | ✅ **已确认可用**:依赖走同一个 `load_script_checked` → 同一套 `entry_keys` 解析,所以 `deps: ["icons"]` 直接解析到 `icons/plugin.js`。已补集成测试(自证式:主脚本在被依赖未先加载时抛错)并把 `commands.rs` 里"deps 是文件 key"的过时注释改掉 |

> 另:**分发步骤已落地** —— `contrib/install-plugins.sh` 把 `plugins/` 装进 `<runtime>/plugins`(见 §8)。

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
| `features/{tabbar,statusline,terminal}.js` | ✅ **已迁** → `<name>/plugin.js` |
| `features/which-key.js` | ✅ **已迁** → `which-key/plugin.js`,init 改为 `helix.load("which-key")` |
| `features/{arsenal,filetree}/index.js` | ✅ **已迁** → `<name>/plugin.js`(目录整体 `git mv`,含 `tests/`) |
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

## 8. 分发:把 `plugins/` 装进 `<runtime>/plugins`

**这是"内置插件"真正成立的一步。** 此前只有**查找路径**(多根里的 `<runtime>/plugins`)、
**没有分发** —— 所以全新安装的机器上那个目录是空的,内置插件并不存在。

```sh
sh contrib/install-plugins.sh                 # 装到默认 runtime 目录
sh contrib/install-plugins.sh /path/to/rt     # 指定 runtime 目录(内部会加 /plugins)
DRY_RUN=1 sh contrib/install-plugins.sh       # 只打印将要做什么
```

`HELIX_RUNTIME` 未设置时,默认落点是与 helix 一致的 runtime 目录
(`$HOME/.config/helix/runtime`,源码构建的惯例)。

### 实测(两层的分工在真实文件系统上验过)

| 场景 | 解析到 | 含义 |
|---|---|---|
| 用户层有 `which-key/` | **用户层** | 覆盖内置 ✓ |
| 把用户层 `which-key/` 移开 | **内置层** | 内置顶上 ✓ |
| `init.js` | 用户层优先 | 用户写自己的入口即覆盖 ✓ |

### 与"迁移"的关系

插件迁移(§6)把仓库的 `plugins/` 对齐成了**内置层该有的形态**
(`<name>/plugin.js` + `lib/` + `examples/` + `init.js`),
所以现在这份树**可以直接当内置插件分发**,不需要额外转换。

## 9. 功能搬迁:先搞清"核心到底剩什么"(一次纠正)

原本的计划是"**先拆 picker 的 UI,数据源留核心**"。**现场核对后发现这个前提是错的**:

| 我以为 | 实际 |
|---|---|
| 四个源(files/grep/buffers/symbols)在核心 | **在 JS**:`plugins/examples/picker.js` 用 `helix.picker.define` 定义 |
| `examples/picker.js` 只是模板、不注册任何源 | **它注册的正是那四个源** |
| 核心 `picker.rs` = 数据源 + UI | 核心只有**源注册表 + 分发**(`define`/`run`/`invoke_action`/`invoke_preview`,571 行)与**原生 UI**(`picker.rs` 1215 + query 373 + handlers 190 + js_picker 141) |

**所以搬迁方向要反过来**:
- **数据源已经在核心之外**(JS),这部分**没有可搬的**
- 剩下在核心的是 **UI + 注册表**。而核心 UI 里最值钱的正是 **nucleo 模糊匹配/滚动/大列表虚拟化** ——
  按本文档自己的判据("**要核心能力的别拆**"),**这部分本来就不该拆**

### 修正后的候选(按"拆出去会不会变差"排序)

| 候选 | 判断 |
|---|---|
| picker 的**过滤/列表渲染** | ❌ 别拆:依赖 nucleo(核心能力),JS 重写只会更慢更差 |
| picker 的**源注册表/分发** | ⚠️ 可搬但无收益:它很薄,且是 UI 的入口 |
| **面板(panel)层 → 叶子** | ✅ 已实质完成(②-3:面板早已住在布局树里) |
| **状态栏(zones/渲染)** | 🟡 可考虑:JS 已在做(statusline 插件),核心只留槽位与同步 |
| **终端标题条/钩子** | ✅ 已经是 JS(`terminal/plugin.js`) |

> **结论**:picker 不是合适的第一个搬迁目标 —— 它核心的部分恰好是"该留核心"的那部分。
> 真正符合判据的候选是**状态栏**:核心留槽位与数据同步,渲染已在 JS 里。

## 10. 核心功能盘点:还剩什么值得搬(一次有数据的结论)

对 `helix-term/src/ui/*.rs`(共 15349 行)做了逐模块盘点,按**"搬出去会不会变差"**分类:

| 模块 | 行数 | 性质 | 搬迁判断 |
|---|---|---|---|
| `layout.rs` | 2592 | 布局树模型 | ❌ 核心能力(②-1/④ 的产物) |
| `editor.rs` | 1835 | 编辑器核心 | ❌ 核心 |
| `plugin_terminal.rs` | 1557 | 终端宿主(pty + alacritty) | ❌ 核心能力(插件是 JS 薄层,已如此) |
| `comp_layout.rs` | 1244 | 组件布局引擎 | ❌ 核心 |
| `picker.rs` | 1215 | 原生列表 UI(**nucleo 过滤/虚拟化**) | ❌ 别拆(见 §9) |
| `completion.rs` | 1012 | 补全菜单 | ❌ 核心(`set_completion_icon` 已把图标交给 JS) |
| `prompt.rs` / `popup.rs` / `menu.rs` / `markdown.rs` | 800/583/498/392 | 交互原语 | ❌ 核心原语(插件在其上渲染,`plugin_popup.rs` 379 行已是 JS 侧胶水) |
| **`statusline.rs`** | **733** | **上游默认状态栏:约 25 个 `render_*` 元素** | 🟡 **唯一真候选** |

### 关于状态栏(唯一的候选)与我的结论

`statusline.rs` 的 733 行里,hook/zones 只出现 **3 次** —— 它基本是**上游那套完整默认渲染**
(mode/spinner/diagnostics/selections/position/encoding/…)。
而 JS 侧 `statusline/plugin.js`(**100 行**)已能**替换**它(`set_statusline` 的 replace 模式)。

**这里有个刻意的取舍,不是疏忽**:
- **核心留着完整默认渲染 = 插件坏了/没加载时仍有可用状态栏**(健壮性)
- **全部搬到插件 = 少 733 行,但"插件一挂就没有状态栏"**(拿健壮性换整洁)

**建议**:如果要搬,遵循一条规则 —— **核心只留"最小可用回退"**(mode + 文件名 + 位置,约 100 行),
**丰富元素留给插件**。这样既减核心的"功能代码",又不牺牲健壮性。

### 总结论

> 除状态栏外,核心 UI 里**没有"该搬的功能"** —— 剩下的都是原语与引擎。
> 而数据源、键盘提示、图标、弹窗、面板、终端标题、状态栏渲染**都已经是 JS**。
>
> 也就是说:**"把功能从核心拆出去"这件事,主体已经做完了**;剩下的不是搬迁,
> 而是**维持那条缝的健康**(本文件的约定、两层覆盖、错误隔离、分发,都是为此)。

## 11. `:tutor` —— 第一个"真搬迁"(✅ **已完成**,附实录)

### 为什么是它

| 事实(已核实) | 含义 |
|---|---|
| Rust 侧只有 **11 行**(`typed.rs:2378` 的 `fn tutor` = `open(runtime_file("tutor"))`) | 核心几乎没逻辑 |
| 内容 **50842 字节**在 `runtime/tutor` | **内容早已是数据**,不在代码里 |
| Neovim 的 `tutor.vim` **正是内置插件** | 有先例,模式一致 |
| **`:tutor` 没有任何测试**(`helix-term/tests/` 里 grep 不到) | 搬迁不会破坏现有测试;**但意味着这条路径当前无覆盖**,搬迁时应补一条 |

### 前置①:`helix.runtime_path(name)`

**实测约束**:`helix-js` **不依赖 `helix-loader`**(Cargo.toml 里没有),所以 JS 侧**拿不到** runtime 目录
—— 必须由 helix-term **启动时推入**,与既有两处同一模式:

- `application.rs:170` `set_plugin_roots(plugin_roots_for(runtime_dirs(), config_dir()))`
- `application.rs:176` `set_layouts_dir(config_dir()/layouts)`

照此加 `set_runtime_dirs(runtime_dirs())`,并在 `state.rs` 写**纯函数解析器**
(从一个有序目录列表里取第一个存在的 `<dir>/<name>`)——**纯函数才便于单测**,同 `resolve_in` / `plugin_roots_for`。

### 前置②:"打开但**不绑定路径**"

Rust 版的 `:tutor` 有一句安全措施:

```rust
doc_mut!(cx.editor).set_path(None);   // 防止误保存覆盖原始 tutor 文件
```

而 JS 的 `helix.open_file(path)` 会**绑定路径** → 用户 `:w` 会**写进 runtime 目录**。
**少了这层,搬迁就是安全回退**。所以要二选一:

- `helix.open_file(path, { scratch: true })` —— 语义清晰,推荐
- 或单独暴露 `helix.doc.set_path(id, null)` 之类的薄原语

### 搬迁步骤(按此顺序,每步可验证)

1. 加**前置①**(启动推入 + 纯函数解析器 + 单测)
2. 加**前置②**(`open_file` 的 `scratch` 选项 + 单测)
3. 写 `plugins/tutor/plugin.js`:注册 `tutor` 命令 → `helix.open_file(helix.runtime_path("tutor"), { scratch: true })`
4. 删 Rust 的 `fn tutor` + 它的 `TypableCommand` 条目(`typed.rs:3997`)
5. `init.js`(仓库 + 用户)加 `safe_load("tutor")`;**两层镜像都同步**(用户层 + 内置层,后者用 `contrib/install-plugins.sh`)
6. **补一条插件级测试**(当前 `:tutor` 无覆盖)—— 断言命令存在 + 打开后 doc 的 `path` 为 None(即"未绑定",这正是前置②的意义)

### ✅ 完成实录(2026-10-06)

| 步 | 结果 |
|---|---|
| 前置①`helix.runtime_path(name)` | `state::{RUNTIME_DIRS, runtime_file_in, runtime_path}` + JS 绑定 + 启动推入;纯函数单测(按序解析/不存在/绝对路径/空列表) |
| 前置②`open_file(p, { scratch: true })` | **新增一个请求** `OpenScratchFile`(而非给既有 `OpenFile` 加字段 —— 后者会让所有构造点都要改,漏一处是**静默**错);JS 路由有单测 |
| 插件 | `plugins/tutor/plugin.js`(注册 `tutor` 命令) |
| 核心 | 删 `fn tutor`(12 行)+ `TypableCommand` 条目(11 行)= **23 行** |
| init | 仓库与用户 `init.js` 各加 `safe_load("tutor")`;两层镜像同步 |
| 测试 | 新增**插件级端到端**(`tutor_plugin_opens_unbound_doc`):断言①找到 runtime 内容 ②`doc.path().is_none()`(**scratch 生效**)③内容真载入(>1000 字符) |

**两处防空过措施**(否则测试"绿得毫无意义"):从**仓库**路径加载(缺失即明确失败,不静默跳过)·
手动 `set_runtime_dirs`(集成进程里启动推入不执行 → 否则 `runtime_path` 为 null、插件只 echo 一句错、
而**初始空文档的 path 本来也是 None** → 断言会假通过)。

**过程中的一次事故**:删 `TypableCommand` 条目时用"裸 `}`"当终止符,而真实结束是 `    },`
→ **多删 450 行**;靠 `git diff --stat` 一眼看出不对 → `git checkout` 回退 → 重做时**先断言范围**。
教训:**删"块"的终止符必须与真实文本一致**(同"锚点必须读出来")。

## 12. 三个候选的施工图(按"最容易 → 最难",数据均为实测)

### 12.1 `:layout` 会话 —— **建议先做这个**(最小、零前置)

| 项 | 事实 |
|---|---|
| 核心里有什么 | `TypableCommand` 条目 **11 行** + `fn layout` **47 行** = **58 行纯包装**(只调 `helix_js::layout_*`) |
| 前置接口 | **零** —— API 早已在 helix-js(`helix.layout.save/load/list/delete`),且有纯函数单测与**路径穿越防护** |
| 插件 | `plugins/layout/plugin.js`(~25 行):注册 `layout` 命令,按子命令分发到 `helix.layout.*`;失败 → `helix.echo` |
| 删除 | 58 行。**终止符是 `    },` 不是 `}`** —— 见 §11 那次"多删 450 行"的教训;**先断言范围再删** |
| init | 仓库 + 用户 `init.js` 各加 `safe_load("layout")`;两层镜像同步 |
| **测试缺口** | `:layout` 命令当前**无测试**(我加它时点明过)→ 搬迁时按 `tutor_plugin_opens_unbound_doc` 的模式补插件级 E2E(从**仓库路径**加载 → 缺文件即明确失败,不静默跳过) |
| 难度 / 风险 | **低 / 低**(纯包装,行为可逐字对照;删除有范围断言) |

### 12.2 `:plugin` 管理器(最大,但**自包含**)

| 项 | 事实 |
|---|---|
| 核心依赖 | **只依赖 `helix_loader::config_dir`(3 处)** —— **不依赖任何核心能力**;其余是目录/文件操作 |
| JS 侧可用 | `read_dir` · `write_file(_async)` · `helix.load` ✓ → **JS 可实现** |
| 自举风险 | 插件管理器**本身是插件**:它若加载失败,就不能用它管理插件 |
| 建议做法 | **分两步**:先搬 `list`/`install`/`remove`(纯文件操作),**`reload` 暂时留在 Rust 作兜底**(它要触发整树重载) |
| 难度 / 风险 | 中 / 中(自举 + 需要决定兜底策略) |

### 12.3 状态栏丰富元素(视觉收益最大)

| 项 | 事实 |
|---|---|
| 核心 | `ui/statusline.rs` **733 行**(约 25 个 `render_*`) |
| JS 现状 | `statusline/plugin.js` 已替换它,但只覆盖 **~8 个元素**:mode · path · cursor · total_lines · 诊断(error/warning)· window_mode · active_leaf_path/type |
| 搬迁 = | ① JS 侧补齐其余(selections · encoding · line-ending · file-type · spinner · position-% · read-only · modified · base-name …)② 核心**只留最小回退**(~100 行:mode + 文件名 + 位置) |
| **为何要留回退** | 刻意取舍:**插件一挂就没有状态栏** vs 少 733 行。规则见 §10 |
| 难度 / 风险 | 中(纯 JS 增量,但**要逐项对照核心的条件与样式**,细节容易漂移) |
| 验证 | 本套件有 `render_rows` 辅助 → 每补一个元素做渲染断言 |

### 建议顺序

**`:layout` → 状态栏 → `:plugin`**

- `:layout`:最小 + 零前置 → 能**再次走通整条搬迁流程**(第二次会快很多)
- 状态栏:纯 JS 增量,可**逐元素验证**
- `:plugin`:**自举**,需要先定"reload 兜底"策略,放最后
