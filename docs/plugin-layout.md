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

### 12.0 ⚠️ 共同前置:键入参数要能到 JS(**挡住 12.1 与 12.2 两项**)

**实测**:`CommandContext`(`helix-js/src/types.rs:460`)**没有 args 字段** ——
而 JS 注册的命令拿到的是单个 `ctx` → **读不到键入参数**。
即 `:layout save dev` 里的 `save`/`dev`、`:plugin install <path>` 里的 `<path>`,JS 插件**都看不见**。

**唯一的分派桥**:`run_plugin_command`(`helix-term/src/commands/typed.rs:4427`)。
参数应在**这里**接上。

#### 实现建议:用**独立通道**,不要给 `CommandContext` 加字段

`CommandContext` 有 **约 57 个构造点**(3 处在 typed.rs、2 处在 helix-js 非测试代码、
其余 ≈52 处是单测)。加一个字段 = 57 处都要改 —— 正是本项目反复吃亏的"改一处漏一处、
且漏了**不报错**"的形态。

**照 `OpenScratchFile` 那次的成功做法**:开**一条独立通道** ——
`run_plugin_command` 在调用 JS 命令**之前**把 args 放进一个 thread-local,
JS 侧经一个原生访问器读(如 `helix.command_args()`),**`CommandContext` 一个字段都不动** ✓

- 位置:`run_plugin_command`(typed.rs:4427)设值;**调用后清空**(避免粘到下一个命令)
- JS 形状:`ctx.args = ["save", "dev"]`(由命令包装器注入)或 `helix.command_args()`
- 单测:入队 → 断言 JS 侧读到;命令结束后 → 断言已清空(防"粘住"这类难查的 bug)

> **这条前置不做,12.1 与 12.2 都无法做** —— 但做完它,两件事一起解锁。

### 12.1 `:layout` 会话 —— **建议先做这个**(最小)

| 项 | 事实 |
|---|---|
| 核心里有什么 | `TypableCommand` 条目 **11 行** + `fn layout` **47 行** = **58 行纯包装**(只调 `helix_js::layout_*`) |
| 前置接口 | 数据 API 早已在 helix-js(`helix.layout.save/load/list/delete`,含纯函数单测与**路径穿越防护**);**但需 §12.0 的参数通道**(本命令有子命令与名字) |
| 插件 | `plugins/layout/plugin.js`(~25 行):注册 `layout` 命令,按子命令分发到 `helix.layout.*`;失败 → `helix.echo` |
| 删除 | 58 行。**终止符是 `    },` 不是 `}`** —— 见 §11 那次"多删 450 行"的教训;**先断言范围再删** |
| init | 仓库 + 用户 `init.js` 各加 `safe_load("layout")`;两层镜像同步 |
| **测试缺口** | `:layout` 命令当前**无测试**(我加它时点明过)→ 搬迁时按 `tutor_plugin_opens_unbound_doc` 的模式补插件级 E2E(从**仓库路径**加载 → 缺文件即明确失败,不静默跳过) |
| 难度 / 风险 | **低 / 低**(纯包装,行为可逐字对照;删除有范围断言) |

### 12.2 `:plugin` 管理器(前置已齐;下面是**照抄即可**的移植规格)

#### ⚠️ 一个**顺序约束**(决定了它必须"一次性切换",不能分步)

`:plugin` 是**一个命令名**。JS 侧注册 `plugin` 会**遮挡**Rust 的那个 →
所以**必须先让 JS 插件覆盖全部子命令**,才能删 Rust 的;否则被遮挡的子命令(如 `install`)会**直接失效** ✗。
状态栏那种"逐个元素增量"在这里**不成立**。

#### Manifest 格式(实测,`helix-term/src/commands/plugin_manager.rs`)

```rust
struct ManifestEntry { source: String, kind: String /* "local"|"git" */, installed_at: String,
                       commit: Option<String> /* git 源 HEAD */, pinned: bool, files: Vec<String> }
type Manifest = HashMap<String, ManifestEntry>;          // key = 插件名
// 路径:config_dir()/plugins/manifest.json
// 读:缺文件或 JSON 坏 → **返回空表,不崩**;写:pretty JSON
```

#### 子命令规格(取自 `typed.rs:5578` 的 `fn plugin`)

| 子命令 | 行为 | 用户可见文案(照抄) |
|---|---|---|
| (无参) | 报用法 | `usage: plugin <list\|install\|remove\|reload\|status>` |
| `list` | 列出已加载插件名 | 空 → `no plugins loaded`;否则 `plugins: a, b` |
| `install <path\|git-url>` | 缺参报错;git URL → clone;本地路径 → 复制 | 成功 → `installed '{name}', reloading...`(随后触发重载);失败 → `plugin install: invalid path or git url '{arg}'` |
| `remove <name>` | 删 manifest 里该插件的 `files` + 孤儿目录 | (文案待补读) |
| `reload` | 整树重载 | —— **JS 侧直接委派给 `:plugin-reload`**(独立命令,零新增) |
| `status` | 报告状态 | (文案待补读) |

**JS 侧现有能力覆盖情况**:`list`→`read_dir` ✓ · `install`(本地)→`read_file`+`write_file` ✓ ·
`install`(git)→`helix.run("git", …)` ✓ · manifest 读写→`read_file`/`write_file` ✓ ·
`remove`→`helix.remove_plugin_file`(**限域**)✓ · 参数→`helix.command_args()` ✓

#### ⬜ 还差**一个**薄接口:插件/config 目录的访问器(实测缺)

**实测**:`helix-js` 里**没有任何** config/插件目录的访问器 ——
`"config_dir"` / `"plugin_dir"` / `"plugins_dir"` / `"config_path"` / `"data_dir"` 的注册数**全为 0**,
把 `lib.rs` 里注册的顶层 API 名**全量列出**也确认没有(最接近的是 `runtime_path`,只服务 runtime)。

⇒ 没有它,JS 连 `plugins/manifest.json` 的**路径都凑不出来** → `list`/`install`/`remove` 全部无法开始 ✗
**所以第 1 步(哪怕只用临时命令名)也被挡住了。**

**修法很小,且是既有模式**:`state` 里**已经有** `plugin_roots()` ✓ →
加一个 `plugins_dir()`(取其首个根,或直接推入 config 下的 `plugins/`)按 **`set_runtime_dirs` 的同款方式**
在 `application.rs` 启动时推入即可 ✓ 与 §12.0、限域删除同属"一个薄接口解锁一整项"的情形。

**⇒ 移植前置最终清单**:参数通道 ✓ · 限域删除 ✓ · `reload` 复用 ✓ · **目录访问器 ⬜** · E2E 测试计划 ✓

#### ⛔ 先纠正一处**我自己写错的数**:子命令是 **8 个**,不是 5 个

我依据的是 Rust 的 **usage 文案**(`usage: plugin <list|install|remove|reload|status>`)——
但**实现比文档多**。读 `plugin_op` 的 match 才看到全部 8 个:

| 子命令 | 语义(读自源码) | JS 侧现状 |
|---|---|---|
| `list` · `install` · `remove` · `reload` · `status` | 见上一节 | ✅ 已覆盖 |
| **`update`** | `update_entries` + `refresh_commits` + 写 manifest(**git 源更新**) | ✗ **未覆盖** |
| **`pin`** / **`unpin`** | `entry.pinned = (sub == "pin")`;**仅 git 插件**(否则报 `only git plugins can be pinned`);文案 `"{sub}ned '{name}'"` | ✗ **未覆盖** |

⇒ **若现在做一次性切换,会静默丢掉 `update`/`pin`/`unpin` 三个子命令** ✗✗
(这正是"一次性切换"最危险的地方:丢掉的功能**不会报错**,只会消失。)

**所以第 2 步的真实覆盖面是 8/8,当前是 5/8。** 读测试与 dispatcher 各救了一次:
- 读**测试** → 发现 `pin`/`unpin` 存在
- 读 **dispatcher** → 发现 `update` 存在,且 usage 文案**不完整**(实现 > 文档)

#### 🚧 最后一次删除的**真实爆炸半径**(实测,2026-10-06)

`grep plugin_manager` 的结果把删除面完整画出来了:

| 牵连 | 位置 | 处理 |
|---|---|---|
| `mod plugin_manager;` | `helix-term/src/commands.rs:3` | 删 |
| `plugin_manager.rs`(639 行) | 同名文件 | 删 |
| **`fn plugin` 只有 **9 行** | `typed.rs:5578` | ⚠️ 说明真代码在它调用的**辅助函数**里(5604–5781 那段引用 `plugin_manager::`)→ **必须一并删** |
| `TypableCommand` 条目(8 行) | `typed.rs:4335` | 删 |
| **集成测试** | `helix-term/tests/test/plugin_manager.rs` | ⚠️⚠️ **真阻塞** |

**为什么测试是真阻塞**:`test/plugin_manager.rs` 里有 `:plugin list` / `:plugin install plain-name`
等用例,断言的是 **Rust 侧**行为(例如"list 应显示**已加载**插件的路径");
`integration.rs:39` 还注册了 `mod plugin_manager;`。**现在删,这些测试立刻变红** ✗。

⇒ 于是它与前面记下的**第 1 个缺口串起来了**:那些测试要的正是"**已加载**插件"这个语义,
而 JS 侧**给不出**(只有 manifest 的"已安装")→ 要全保真,**必须先有 `helix.loaded_plugins()`** ✓

#### ✅ 前置③的最后一个未知点也已读清:测试改造 = **每个用例加一步**

`tests/test/plugin_manager.rs` 的 5 个用例结构一致:

```rust
let _plugin_guard = PLUGIN_TEST_LOCK.lock().await;
let dir = tempfile::tempdir()?;                       // 每个用例自造 fixture
test_key_sequences(
    &mut AppBuilder::new().with_file(file, None).build()?,
    vec![
        (Some(":plugin-load <fixture>.js<ret>"), None),   // ← 现有:载入 fixture
        (Some(":plugin list<ret>"), Some(&|app| assert!(…))),
```

⇒ **改造 = 在每串序列的 `:plugin` 之前,插入一次 `:plugin-load <repo>/plugins/plugin/plugin.js`** ✓
(照 `tutor_plugin_*` / `layout_plugin_*` 的既定做法:**从仓库路径加载**,文件缺失即**明确失败**、
不静默跳过)。**断言可原样保留** —— 因为文案我已逐字对齐 ✓

**一个必须注意的保真点**:这 5 个用例刻意"**只测无副作用路径**"(它们用 `nope`/`plain-name` 这类
假名字,以免动到真实的 `~/.config/plugins`)✓ —— 我的 JS 实现**恰好满足**:`pin`/`unpin` 在
"未安装"时就 `break`,**不会写 manifest** ✓;`list`/`status` 只读 ✓。**这一点不能改坏**:
若 `pin nope` 变成"先建条目再报错",测试就会真的写用户的配置 ✗

#### ⛔ 又一重纠缠:`plugin_op` 是 **JS API `helix.plugin.*` 的共享后端**

`grep plugin_op` 查出它**有 2 个调用者**(不止命令本身):

| 调用点 | 是什么 |
|---|---|
| `typed.rs:5585` | `fn plugin`(TypedCommand —— 可随命令一起删 ✓) |
| **`typed.rs:5365`** | **`helix.plugin.*` 这个 JS API 的桥** ✗✗ |

而 `helix-js/src/commands.rs:362/383/402` 的 `push_plugin_op("install"/"update"/"remove", …)`
正是往它送请求。⇒ **删掉 `plugin_op` 会直接打断一个公开 API** ✗

**范围核实(一条 grep 定论)**:

| 维度 | 结果 |
|---|---|
| **有插件在用吗** | **一个都没有**(`plugins/*.js` grep 为空 ✓) |
| 是否文档化 | ✗ 是:`docs/plugin-api.md:116,282` · `docs/api/plugin.md:37-39`(带示例) |
| 是否有测试 | ✗ 有:`helix-js/src/lib.rs:3803-3808` · `tests/test/plugin_manager.rs:148-157` |

⇒ **可行,但必须带兼容路径**:把 `helix.plugin.install/update/remove` **由 JS 管理器自己对外提供**
(它已经实现了这三个 ✓ —— 所以是**再导出一遍**,不是新逻辑 ✓),同时更新那 2 处文档 + 2 处测试。
**顺带发现一处不一致**:`plugins/helix.d.ts` **没有**这个 API 的声明 ✗(文档有、类型声明没有)。

**所以"最后一刀"的真实前置是两件**:① `helix.loaded_plugins()`(补 `list` 保真)
② **同步改造 `test/plugin_manager.rs`**(改用 JS 提供的 `:plugin`,或按新语义重写断言)
③ 然后才是改名 + 一次性删除 + E2E(含"断言 `:plugin-reload` 仍在")

#### 落地顺序(每步都可验,但**第 2 步必须一次做完**)

1. **先写 JS 插件的骨架 + `list`/`status`/`reload`**(此时**不要注册 `plugin` 命令名**,先用临时名如 `plugin-js` 验证行为)✓
2. **补齐 `install`/`remove` 后,一次性把命令名改成 `plugin` 并删 Rust 的**(`TypableCommand` 条目 + `fn plugin` + `plugin_manager.rs` 整个模块 + `mod` 声明)—— **这一步不可分**
3. 端到端测试(照 `layout_plugin_routes_args_and_persists` 的模式):
   - 预置 manifest + 假插件目录 → `plugin list` 应列出 → `plugin remove <name>` 应删文件且**不动别的**
   - `plugin install <临时目录>` → 断言文件被复制 + manifest 被写
   - **断言 `plugin-reload` 仍在**(自举兜底不能被删)



**先纠正一个印象**:它不只是文件操作。`plugin_manager.rs`(639 行)实测含
**manifest 读写** · **安装复制**(`install_target`/`install_copy`/`walkdir`)· **删除**
(`remove_files`/`remove_orphan`)· **完整 git 集成**(`is_git_url`/`clone_to_vendor`/
`git_head_commit`/`git_fetch`/`git_pull_ff`/`git_origin_head`)—— 即"**从 Git URL 装插件并更新**"。

它的核心依赖很薄:`helix_loader::config_dir`(3 处)+ `std::fs::{write, remove_file, remove_dir_all}`。

#### 逐项判据(JS 现有能力 vs 缺什么)

| 子命令 / 内部件 | JS 侧现状 | 结论 |
|---|---|---|
| `list` / `status` | `read_dir` + `read_file` ✅ | **可直接搬**(只读) |
| `install <local path>` | `read_file` + `write_file` ✅(目录树用 `read_dir` 递归) | **可直接搬** |
| `install <git url>` / 更新 | **`helix.run`/`spawn` 可执行 `git`** ✅ | **可直接搬**(用 `git clone/fetch/pull` 命令,不必新 API) |
| manifest 读写 | `read_file`/`write_file` ✅ | 可直接搬 |
| **`remove <name>`** | ✅ **接口已实现**:`helix.remove_plugin_file(rel)` —— **限域**(只相对路径 · 拒 `..` · `canonicalize` 复核只落插件根内),`true` 删了 / `false` 幂等 / 非法路径抛错;**测试断言"拒绝时根外文件仍在"** | **可直接搬** |
| **`reload`** | ✅ **不需要新钩子**:`:plugin-reload` 本就是**独立命令**(`typed.rs:4320`)→ JS 插件**委派**给它即可 | **可直接搬** |

#### 结论:**前置全齐,只剩"写插件 + 删 Rust"**

| 前置 | 状态 |
|---|---|
| 参数通道(§12.0) | ✅ `helix.command_args()` |
| 删除(限域) | ✅ `helix.remove_plugin_file(rel)` |
| 重载 | ✅ **复用** `:plugin-reload`(独立命令,零新增) |
| 其余(list/install/manifest/git) | ✅ 现有能力足够(`read_dir`·`read_file`·`write_file`·`helix.run` 驱动 git) |

**剩下的是机械活**:写 `plugins/plugin/plugin.js`(覆盖 `list|install|remove|status`,把 `reload`
委派给 `:plugin-reload`)+ 删 Rust 639 行 + init 两处 + 端到端测试。

> **自举原则(仍然适用)**:`:plugin-reload` **不要删** —— 管理器自己挂了时,它是唯一的救援路径。



| 项 | 事实 |
|---|---|
| 核心依赖 | **只依赖 `helix_loader::config_dir`(3 处)** —— **不依赖任何核心能力**;其余是目录/文件操作 |
| 前置接口 | **§12.0 的参数通道**(`list\|install <path>\|remove <name>` 都要读参数) |
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
| 难度 / 风险 | 中(**但见下面的更正**:不是"纯 JS 增量") |
| 验证 | 本套件有 `render_rows` 辅助 → 每补一个元素做渲染断言 |

#### ⚠️ 勘路更正:"纯 JS 增量"这个说法**不成立**

实测 `StatuslineCtx`(`helix-js/src/types.rs:477`,注释写明"轻量上下文,不含 `doc.text`,避免每帧克隆全文")
**只有 9 个字段**:`path` · `mode` · `cursor` · `total_lines` · `diagnostics_error` ·
`diagnostics_warning` · `window_mode` · `active_leaf_type` · `active_leaf_path`。

而 JS 状态栏**已经把这 9 个全部用上**了 → 要补**任何**新元素(encoding · line-ending · modified ·
read-only · selections · file-type · spinner …),**核心必须先多暴露字段** ⇒ **有前置**,不是纯 JS 增量。

另一个纠正:**"位置百分比"JS 侧已经有了**(`plugin.js:80` 的 `Math.round((row/total)*100)+"%"`),
所以原计划里"补百分比"是**多余的**;`~8/25` 的覆盖数应理解为"9 个 ctx 字段,而非 25 个核心元素"。

#### ✅ 已定位:扩 ctx 只需**改 1 处** → 前置很便宜

生产构造点在 **`helix-term/src/ui/statusline.rs:79`**:

```rust
let js_ctx = helix_js::StatuslineCtx { … };          // ← 唯一的生产构造点
…
let js_parts = helix_js::statusline_parts(&js_ctx);  // 同文件 :117 调用钩子
```

全仓 `StatuslineCtx` 的使用点只有 4 处:结构体定义(`types.rs:478`)· 钩子入口
(`popup.rs:1677`)· **唯一生产构造点(`statusline.rs:79`)** · 2 处单测构造。

⇒ **扩字段 = 改 1 处** ✓ 与 §12.0 的 `CommandContext`(**~57 处**构造点)形成鲜明对比 ——
**状态栏的前置因此是三项里最便宜的**:按元素逐个"在 ctx 加一个字段 + 在 `statusline.rs:79` 填上 +
在 JS 里渲染",每步都能独立验证。

> **勘路方法上的一条教训**:前一轮我用 `grep -r "StatuslineCtx {" helix-term/src --include=*.rs`
> **没命中**,于是推断"生产路径是别的方式"—— **错了**:不带 `--include` 的同义搜索就命中了
> (`helix-term/src/ui/statusline.rs:79`)。**"搜不到"不等于"不存在"**,换一种搜索方式再确认,
> 比基于漏检去推断架构要省得多。

### 建议顺序

**`:layout` → 状态栏 → `:plugin`**

- `:layout`:最小 + 零前置 → 能**再次走通整条搬迁流程**(第二次会快很多)
- 状态栏:纯 JS 增量,可**逐元素验证**
- `:plugin`:**自举**,需要先定"reload 兜底"策略,放最后

## 13. 兼容层 `helix.plugin.*` 的具体设计(最后一刀的前置)

迁移 `:plugin` 时,`plugin_op` 是命令与 **`helix.plugin.*` JS API 的共享后端**
(API 桥在 `typed.rs:5365`,`helix-js` 侧由 `push_plugin_op` 送请求)。
该 API **无插件使用**,但**公开文档化**(`docs/plugin-api.md` · `docs/api/plugin.md`)且有测试
(`helix-js/src/lib.rs:3803-3808` · `tests/test/plugin_manager.rs:148-157`)⇒ **必须带兼容路径**。

### 三步(都在 `plugins/plugin/plugin.js` 内)

1. **重构**:把 `register_command("plugin-js", async () => { … })` 里的逻辑抽成
   `async function runOp(sub, arg)`;命令壳只做 `helix.command_args()` → `runOp(sub, args[1])` ✓
2. **对外导出**(三个能力**都已实现** ⇒ 是"再导出一遍",不是新逻辑):
   ```js
   helix.plugin.install = (arg)  => runOp("install", arg);
   helix.plugin.update  = (name) => runOp("update", name);
   helix.plugin.remove  = (name) => runOp("remove", name);
   ```
3. **可断言的标记**(否则测试分不清走原生还是 JS 兼容层):
   ```js
   helix.plugin.install.__hx_shim = true;
   ```

### ⚠️ 唯一未知点:动手前先探测

`helix.plugin` 是**声明函数**(`helix.plugin(name, opts)`),Rust 侧在**同一函数对象**上挂了
`install`/`update`/`remove`。**JS 能否覆盖/添加这些属性?** 探测只需一行 + 一条断言:

```js
helix.plugin.__probe = "ok";   // 零副作用;Rust 侧断言读回 === "ok"
```

- **可挂** → 按上面三步做 ✓
- **不可挂**(原生对象冻结/只读)→ 改走"**JS 导出模块**":管理器用 `helix.export` 暴露
  `{install, update, remove}`,文档改为 `const mgr = helix.load("plugin"); mgr.install(…)`
  —— **这是一次公开 API 变更**,所以更该先探测再动手。

### ✅ 探测结果(2026-10-06):**可行**

测试 `helix-js::tests::plugin_object_accepts_new_and_existing_properties` **通过**:

| 探测 | 结果 |
|---|---|
| 挂**新**属性(`helix.plugin.__probe = "ok"`) | ✅ 可挂 |
| **覆盖既有**属性(`helix.plugin.install = () => "shim"`,替换 Rust 挂上的那个) | ✅ **可覆盖** |

⇒ **兼容层方案成立**(§13 的三步可以直接做),**不需要**改走"JS 导出模块"
—— 也就是说 **`helix.plugin.*` 这个公开 API 不会变更** ✓

**读值方式**:`helix.echo(...)` + `take_messages()`(与 helix-term 的运行路径同一机制)。
(第一版我用 `load_script(...).as_string()` → 编译不过:它返回 `()` ✗ —— 又一例"先读 API,别猜"。)

## 14. 引擎重入 panic 的排查结论(`plugin-js` 端到端测试暴露)

补 `plugin-js` 的行为测试时,在应用内跑到 `helix-js/src/state.rs:138` panic。
该行是 **`CONTEXT.with(|cell| f(&mut cell.borrow_mut()))`**(`with_engine_slot`)⇒ **引擎重入** ✗

### 一个被证伪的假设(先记下来,免得下次重走)

我最初假设"**`helix.echo` 在 `await` 之后调用会重入**" ✗ —— **读了实现发现是错的**:

```rust
pub(crate) fn js_echo(_this, args, context: &mut Context) -> …  // 用**传入的 context**,不借全局
    MESSAGES.get().expect(…).lock().expect(…).push(text);        // 只碰一个全局 Mutex
```

⇒ **`echo` 安全** ✓。而"再借会 panic"那条注释说的是 **`helix.load`**
(`commands.rs:73-77`:"load 可能发生在命令运行中(lazy 桩),外层 `run_command` 正持有
CONTEXT 的 RefCell 借用,再借会 panic")⇒ 这是本代码库里**已知的一类约束** ✓

### 可执行的推论

真正的约束是:**命令执行期间,凡是**重新借全局引擎**的 API 都可能 panic** ✗。
而 `plugin-js` 的 `list`/`status`/`install`/`update` 都在**命令处理函数**里调用
**异步 API**(`read_file_async` / `run_async`)⇒ 命中这一类 ✗

**关键事实**:`helix-js` **没有同步的 `read_file`**(注册表里只有 `read_file_async`/`write_file_async`/
`stat_async`/`glob_async`;同步的只有 `read_dir`)✗ —— 于是插件被**逼上异步路径** ✗

**⇒ 最干净的修法:加一个同步 `helix.read_file(path)`** ✓
- 它同时解决两件事:`plugin-js` 不必再 async(从而避开重入 ✗)· 与 `read_dir` 对称(都是同步读)✓
- 落地后 `plugin-js` 可整体改为同步,重入问题**从根上消失** ✓,那个端到端测试也就能加回来了

### 未查清

`read_file_async` / `run_async` 的**具体实现是否借 `with_engine`**(它们的函数名与我猜的不同,
本轮没定位到)✗ —— 这是下一步的第一件事:确认后即可确定"哪些 API 不能在命令里用"的完整清单。

### 追加更正(同日):**第二个假设也被证伪**

我接着假设"`read_file_async` / `run_async` 借了全局引擎" ✗ —— **查实现又错了**:

| 函数 | 位置 | 前 30 行内 `with_engine` |
|---|---|---|
| `js_read_file_async` | `helix-js/src/shell.rs:604` | **0** |
| `js_run_async` | `helix-js/src/shell.rs:298` | **0** |

⇒ 它们**自己不重入引擎** ✗。那 panic(`with_engine_slot`,`state.rs:138`)只可能来自
**异步续体 / promise 落定**那一段(Lua? 不 —— 是 boa 的 promise 在后续 tick 里要**重新进引擎**去 settle)✗
**⇒ 下一步该追的是"promise 落定时谁借了引擎",而不是这两个 API 本身。**

### 但修法结论**不变**:加同步 `helix.read_file(path)`

理由从"异步 API 有问题"改为"**绕过整条 async/promise 机制**"✓ ——
`plugin-js` 全程同步后就**构造性地**避开这一类重入 ✗,且与既有的同步 `read_dir` 对称 ✓

### 本轮方法论上的一条账

三个假设,读了实现之后**错了两个**(`echo` 重入 ✗ · 异步 API 借引擎 ✗),一个成立(引擎重入这个**现象** ✓)：

> **假设很便宜(一条 grep),照着假设改代码很贵。**
> 两次"先验假设"都避免了去改**不是原因的地方** —— 而改错地方还会把真原因盖住。

## 15. 怎么**证明** JS 接管了 `:plugin`(防止"假绿")

改名尝试失败时暴露的真问题:**插件加载失败 ⇒ `:plugin` 回落到 Rust ⇒ 测试照样全绿** ✗✗
⇒ 现成套件**分不清"JS 接管"与"JS 没加载"**。若不先补判据,改名的"绿"毫无意义,
而真正的验证要等到旧实现删光之后才发生 —— **自举迁移里最坏的"假成功"** ✗

### 可用判据(设计为**新旧实现输出不同**)

Rust 的 `list` 只输出一行(`no plugins loaded` / `plugins: a, b`);
而 JS 侧**多输出一行** `installed(N): …`(**仅当 manifest 非空**)✓

⇒ 做法:在 5 个用例里把插件根指向**预置了 `manifest.json` 的临时目录** ✓
然后断言 `:plugin list` 的状态里**含 `installed(`** ✓

| 情形 | Rust 应答 | JS 应答 |
|---|---|---|
| 空 manifest | 不含 `installed(` | 不含 ✗(不可区分) |
| **预置 manifest** | 仍不含 ✗ | **含** ✓✓ |

⇒ 判据成立,且**顺带覆盖了 manifest 读取路径**(本来就没测 ✓)

### 备选(间接但更简单)

`plugin-js` 这个名字在改名后**必须消失** ⇒ 断言 `:plugin-js list` 走不到旧 manager ✓
—— 但"命令不存在"与"命令出错"在状态栏上不易区分 ✗,所以**首选上面那条** ✓

### 修正后的执行顺序(三步,前两步都能独立验证)

1. **修好改名补丁** —— 且**改完立刻 `node --check`**(上次我漏了,代价是一次回退 ✗)
2. **加判据**:5 个用例预置 manifest + 断言 `installed(` 出现 ⇒ **证明 JS 真的在应答** ✓
3. **改名 → 一次性删 Rust**(条目 + `fn plugin` + `plugin_op` + API 桥 + `push_plugin_op`
   + `plugin_manager.rs` + `mod`)+ 文档 + 断言 `:plugin-reload` 仍在 ✓
