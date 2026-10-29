# 调试模式（Debug Mode / 只读）设计选项

> ⚠️ **状态提示**：§1–§8.1 记录的是**第一版**（`PaneMode::Debug` 那个"模式"）。
> 它已被 **§9 的 gdb 式改版**取代（现在没有"调试模式"，暂停时键位直接接管编辑器，
> 层的统一抽象在 `input_router.rs`，见 `2026-10-27-input-layer-design.md`）。
> 面向使用者的当前行为请看 `docs/api/dap.md`。本文保留历史的原因：§8.5 的 python 攻关、
> §8.1 的"为什么当初选了模式"都是有用的决策记录。

> 状态：**设计稿**（三个方案 + 推荐，待拍板后实现）
> 关联：`compositor.rs` 的平级模式（`PaneMode`）、`dapui` 面板、`Document::apply` 编辑闸门

---

## 0. 先把问题拆成两件事

"调试时不能改文件、只能读 + 调试" 其实是两个**正交**的问题：

| 问题 | 回答的是什么 | 现在的状态 |
|---|---|---|
| ① **键位归谁** | 按 `n` 是"单步"还是"搜下一个"？按 `i` 是"进入插入"还是"step in"？ | 没有任何调试态；全靠 `space G` 前缀和面板聚焦 |
| ② **能不能改文件** | `:w` / `i` / 粘贴 / LSP 重命名 / 插件 `doc.insert` 会不会写进 buffer | 完全没有闸门：`doc.readonly` 只喂状态栏的 `[readonly]` 指示，**编辑与保存都不检查** |

把它们绑成一个"Debug 模式"看着漂亮，但会出现"我只想单步、不想锁文件"和"我想开着调试改一行再重启"两种刚需互相打架。
**所以：①做成模式，②做成独立开关，二者可以联动但默认解耦。**（推荐，见 §5）

---

## 1. 现状盘点（能复用什么）

**已有的、可直接复用**

- **平级模式机制**（`helix-term/src/compositor.rs`）：
  `PaneMode::{Normal, Locked, Pane, Resize, Move, Scroll}` + `pane_mode_key()`（接管键位、`Esc` 回 Normal、按同一个前缀键回去）
  + `pane_mode_entries()`（**键位提示的唯一来源**，which-key 直接读它）+ `publish_pane_mode()`（`helix.pane_mode.*` + `pane-mode-change` 事件）。
  加一个 `Debug` 变体 = 3 处 match + 1 个前缀键，**结构是现成的**。
- `space G` 的 DAP 命令全套（`dap_continue/pause/step_in/step_out/next/restart/terminate/variables/breakpoints`）。
- 面板键位（`n/i/o/c/p/b/w/W`）已经在 dapui 里实现，且**页脚会画键位提示**。
- 布局快照：`helix.layout.get/restore`（`layout` 插件命令），可以"记住调试前布局、退出恢复"。
- `Document.readonly` 字段 + 状态栏 `[readonly]` 指示（`ui/statusline.rs:136`）。

**缺的**

- 没有 `Debug` 模式；`dap-stopped` 时键位仍是编辑器/面板的。
- `readonly` **不参与任何检查**：`Document::apply` / `apply_temporary` 不看它（所以插件编辑、LSP code action、`d`/`i` 全都照改），`:write` 也不看它（只看文件本身的权限位）。
- 没有 `:read-only` 命令，也没有"调试停止时自动只读"的开关。

---

## 2. 方案 A：`Debug` 键位模式（"按键层面的只读"）

**是什么**：在平级模式里加 `PaneMode::Debug`，进入后由 compositor 接管键位。

```text
进入：dap-stopped 自动进 · 手动 `<space>G`（C-d 已按需求移除）· Esc 回 Normal
键位：n 单步跳过 · i 单步进入 · o 单步跳出 · c 继续 · p 暂停
      b 切花断点 · w 加监视 · W 清空监视 · t/f 线程/帧 · s 会话
      j/k 滚动视口(转发 move_visual_line) · g/G 首尾 · `/` 搜索
      Esc 退出(回到可编辑的 Normal) · ? 帮助
```

**编辑类键不绑定 ⇒ 被吞掉**（`i`、`a`、`d`、`x`、`y`、`p` 之类按下去什么都不会发生）。

| 维度 | 评价 |
|---|---|
| 拦截范围 | **只挡键盘编辑**。`:w`、`:lsp-rename`、插件 `doc.insert`、LSP code action **照旧能改** |
| 实现量 | **1 天**（enum + 3 处 match + 前缀键 + 键位表 + 单测；which-key/dapui 页脚自动跟着变） |
| 风险 | 低；最坏情况是"某个键没绑 ⇒ 按了没反应"，与 zellij 模式一致，`Esc`/`?` 可救 |
| 观感 | 调试时"键盘归调试"——和 zellij / VS Code 的 debug 键位思路一致 |
| 缺点 | 用户想边调边改代码就必须先 `Esc`（多一步）；纯 `:` 操作不受影响 |

**变体 A+（更严格一点）**：Debug 模式下把 `:` 也接管成一个窄命令集（只允许 `:debug-*` / `:dapui*` / `:q`），其余拒绝。
代价 +0.5 天，收益是"手滑 `:w` 不会存半改的文件"。

---

## 3. 方案 B：只读闸门（真正的"不能改"）

**是什么**：加一个**编辑闸门**，让"不能写 buffer"成为核心不变量，而不是按键约定。

**闸门必须覆盖这四个入口**（漏一个就等于没做）：

| # | 入口 | 位置 |
|---|---|---|
| 1 | 命令编辑（`i/a/d/c/x/p`、`:d`…） | `Document::apply` / `apply_temporary` 开头 early-return |
| 2 | 插件编辑（`doc.insert/replace/delete`、`helix.dapui` 的装饰不算） | 同 1（`apply_plugin_edits` 最终也走 `Document::apply`） |
| 3 | LSP 侧写入（`:lsp-rename`、code action、format） | 同 1 |
| 4 | 保存（`:w`/`:wq`/`autosave`） | `:write` 命令 + `auto_save` handler 检查同一个标志 |

**状态**：新增 `Document.readonly_forced: bool`（**不复用** `readonly`：那个是"文件权限位"，语义不同）；
状态栏已经会画 `[readonly]`（把两来源或起来即可）。

**命令**：`:read-only`（切当前文档）/ `:read-only!`（切全部打开的文档）/ `:set-readonly-all`。

**与调试联动**（可选）：

```toml
[plugins.dapui]
readonly_on_stop = "off"    # off | doc(当前文件) | all(所有打开的文档)
```
`dap-stopped` 置位、`dap-continued`/`dap-terminated` 恢复（记住置位前的值，别把"本来是只读的文件"解锁了）。

| 维度 | 评价 |
|---|---|
| 拦截范围 | **五个入口全挡**（含插件与 LSP）；这才是"只能读" |
| 实现量 | **2–3 天**（核心 4 处闸门 + 命令 + 测试 + 状态提示） |
| 风险 | 中。**静默吞编辑是最糟的体验** ⇒ 必须：① 状态栏明确"只读（调试）" ② 每次被拦打一条 `Debug mode: buffer is read-only` ③ `:read-only!` 一键解 |
| 缺点 | "改一行再重启"这条高频路径会被挡（所以默认 `readonly_on_stop = "off"`） |

---

## 4. 方案 C：三合一「Debug 视角」（A + B + 布局/焦点快照）

**是什么**：进入调试即切换"视角"：Debug 键位模式 + 相关文档只读 + 面板布局预设（右树 + 底 Output）+ 焦点回编辑器；退出时**完整恢复**（布局、焦点、只读状态）。

```text
dap-stopped → 记住 { layout, focused_leaf, readonly(doc) } → 应用 { Debug 模式, 右树+底 Output, 焦点=编辑器 }
continue/terminate/Esc → 恢复 { layout, focused_leaf, readonly(doc) }
```

| 维度 | 评价 |
|---|---|
| 收益 | 最接近"VS Code debug perspective"的整体观感；用户不用记"什么时候开哪个面板" |
| 实现量 | **4–5 天**（布局快照现成，但"焦点恢复"与"只读快照"都要处理易碎边界） |
| 风险 | 高：布局恢复失败会让人"面板全乱"；多会话/多窗口时"记住哪一份"难定义 |
| 适合 | 只做调试的工作流；不适合"一边调一边改" |

---

## 5. 方案 D：不做模式（对照方案）

**论据**：调试最高频的循环是"改一行 → 重启 → 再看"，模式会把这条路上加两次切换。
现有手段已覆盖：`space G` 全套命令、dapui 面板键、`auto_open_*` 布局、`Focus` 进出。

若只想"防手滑"，那只需 B 的 `:read-only` 手动开关（0.5 天，不含自动联动），不必引入模式。

---

## 6. 推荐：**A（必做）+ B（独立开关，默认关）+ 不做 C**

理由：

1. **①键位归谁**确实需要"调试态"——否则 `n/i/o/c/p` 这套面板键只在面板聚焦时有效，焦点一回编辑器就散掉。A 恰好复用 `PaneMode`，代价 1 天，且 which-key/dapui 页脚自动同步（单一来源）。
2. **②能不能改**是策略问题，不该焊进模式：做成 `:read-only` + `readonly_on_stop = off|doc|all`，默认 **off**（保留"改一行再重启"），需要"真只读"的人自己开。
3. C 的收益主要是观感，代价与风险都最高；等 A+B 稳定后，如果确实想要"视角"，再单独做（布局快照那部分已有 `helix.layout.get/restore`）。

**落地顺序**

| 步 | 内容 | 估时 | 验证 |
|---|---|---|---|
| 1 | `PaneMode::Debug` + 键位表 + `C-d`/`Esc` + 自动进入（配置 `enter_on_stop`） | 1 天 | 单测（键位表/进出）+ 真机（单步/继续/断点/监视） |
| 2 | which-key 与 dapui 页脚跟随（**已经自动**，只需确认 mode 标题与文案） | 0.5h | pty 截屏 |
| 3 | `Document.readonly_forced` + 四个闸门 + `:read-only`/`:read-only!` | 2 天 | 单测（apply/apply_temporary/write/autosave）+ 集成（插件编辑被拒） |
| 4 | `readonly_on_stop = off|doc|all`（dapui 侧，用新 API `helix.doc.set_readonly`? 或核心读配置） | 0.5 天 | 集成（stop→只读；continue→恢复） |
| 5（可选 A+） | Debug 模式下 `:` 只允许 `:debug-*` / `:dapui*` / `:q` | 0.5 天 | 单测 |

**明确不做**（写在这里免得反复讨论）：不把"只读"做成隐式默认；不做"调试时禁止保存到磁盘"以外的磁盘级锁定；
不在 Debug 模式里重绑全局前缀（`space` 仍可用，因为面板键位是"焦点在面板"的补充，不是替代）。

---

## 7. 验收清单

- [ ] `dap-stopped` 后 `n/i/o/c/p` 直接生效（**不需要先聚焦面板**）；`i` 不再进插入模式
- [ ] `Esc` 回 Normal（可编辑），`C-d` 再回 Debug
- [ ] which-key 显示 `DEBUG` 模式的键位表；dapui 页脚一致
- [ ] `:read-only` 后 `i`/`d`/粘贴/`:lsp-rename`/插件 `doc.insert` **全部**被拒，且状态栏有明确提示
- [ ] `:read-only!` 解除；`readonly_on_stop="doc"` 时 continue 后自动解除
- [ ] 退出调试不会把"本来就是只读（文件权限）"的文件解锁

---

## 8. 落地记录（2026-10-26，已实现 A + B + 联动）

| 设计项 | 落点 | 说明 |
|---|---|---|
| `PaneMode::Debug` | `helix-term/src/compositor.rs` | 前缀键 `C-d`；`Esc`/`C-d` 退出；状态栏显示 `DEBUG` |
| 自动进入/退出 | `editor.debug_mode_wanted`（`helix-view`）+ `handlers/dap.rs` 的 Stopped/Continued/Terminated/Exited + `compositor.rs` 在 Normal 时应用 | 输入态层/Insert/终端直通时不抢 |
| 调试键位（单一来源） | `pane_mode_entries(Debug)` + `pane_mode_key` | `n/i/o/c/p/b/t/f/s/v/j/k/g/G`；which-key 与内置 Info 自动跟随 |
| 编辑键拦截 | 同上（白名单外的一小撮编辑键 consume + 一条状态提示） | 其余键**照旧落到叶子**（滚动/`:`/窗口操作不受影响）|
| 强制只读闸门 | `Document.readonly_forced` + `apply_inner` early-return + `:write` 检查 + `insert_mode` 提示 | 命令编辑 / 插件 `doc.insert` / LSP code action **全覆盖**；状态栏 `[readonly]` 也认它 |
| `:read-only [all]` | `commands/typed.rs` | 切换当前 buffer / 全部打开的 buffer；错误提示统一 `Buffer is read-only` |
| JS API | `helix.set_readonly(on, all?)` | 插件侧开/关（`run_async` 是 **shell**，不能跑 typed 命令 ⇒ 必须给个 API）|
| 调试联动 | `[plugins.dapui] readonly_on_stop = "off"|"doc"|"all"` | 停止时上锁、继续/结束时解除；只撤销自己置的那次（手动 `:read-only` 不会被误解开）|

**顺带修的**：DAP 事件处理器的**属主丢失** —— `helix_js::dap::emit_event` 调用 handler 时没切
`current_owner`，导致 handler 里调受保护 API（`process`/fs）被拒（真实现象：`dap-stopped` 里设只读报
"没有插件上下文，拒绝 'process'"）。

**与设计稿的差异**

- 不叫 `:read-only!`，用 `:read-only all`（typed 命令的 `!` 后缀不是参数）。
- 只读用的是新字段 `readonly_forced`（不是复用文件权限位的 `readonly`）——两者语义不同，都喂状态栏。
- 步 5（Debug 模式下 `:` 白名单）**未做**：`:` 仍可用（`：read-only`/`:w` 有各自的闸门）。

**验证**（真机 + 单测）

```
dapui 46 · which-key 20 = node 66 passed
helix-term 194 · helix-view 79 · helix-js 247 passed
api-check ✓ · rustfmt ✓ · release 已安装

命中 → 自动 DEBUG + 只读（[readonly] + 消息「只读已开（all）」）
DEBUG 里 i → 被挡（不进插入）；n → 单步（帧 4→5）
Esc 退出调试态后 i → 「Buffer is read-only」；:read-only 解除后 i → 进插入
```


### 8.1 变更（2026-10-26 晚）：`<space>G` 直接进入调试态，键位表合并

按需求把两个入口合并成一个：

- `<space>G` 不再是 sticky 键位菜单，而是命令 `enter_debug_mode` → 进入 `PaneMode::Debug`
  （**同一帧生效**：compositor 在事件末尾再检查一次 `editor.debug_mode_wanted`，
  所以紧接着的那个调试键不会被吃掉）。
- **`C-d` 已移除**（进/出都改用 `<space>G` / `Esc`）。
- 调试键位的**唯一来源** = `keymap/default.rs::dap_keymap()`：Debug 模式按键时按它路由、
  提示也从它生成。旧菜单的键一个没变（`l/r/b/c/h/i/o/n/v/t/C-c/C-l/s t/s f/S/e/E`），
  所以 `space G b`、`space G l` 这些肌肉记忆**照旧成立**（现在是"进模式 + 按同一个键"）。
- 额外只属于调试态的键：`j/k/g/G` 移动滚动、`Esc` 退出；状态栏显示 `DEBUG`。
- 单测：`dap_keymap` 的键集被钉住；`s` 必须是两键子菜单（`s t`/`s f` 的 pending 依赖它）。


### 8.2 体验增强（本轮）

| 项 | 键 / 命令 | 实现 |
|---|---|---|
| 调试态帮助 | `?` | `debug_help`（一行键位速查；完整表在 which-key 的 `Debug` 提示） |
| 跑到光标 | `R` · `:debug-run-to-cursor` | 打**临时断点**（`editor.debug_temp_breakpoint`）→ continue；`Event::Stopped` 命中即摘，`Terminated/Exited` 兜底清理 |
| 光标求值 | `K` · `:debug-eval-under-cursor` | 取光标下标识符 → `evaluate(context: hover, frameId: 当前帧)` → 结果进状态栏；**失败也给消息**（不再吞进日志） |
| 删单条监视 | 树里 `d` | 只接管 `w0..` 节点，其它节点返回 `ignore`（穿透） |
| 改变量值 | 树里 `e` | `helix.dap.request("setVariable", {variablesReference: 父作用域, name, value})`（lldb-dap 支持；弹层初值用**原始值**，不带 ` :类型`） |
| 断点未验证原因 | 断点页 | `bp_label` 带上 adapter 的 `message`（`◯ !no executable`） |

**顺带修的一个真 bug**：终端送来的大写字母带 `SHIFT`（`Char('K') + SHIFT`），而键位表里
`"K"` 编码成 `Char('K')`（无修饰）⇒ `KeyTrie::search` 直接查**查不到**，"按了没反应"。
`debug_lookup()` 现在会再试一次"翻转 SHIFT"的变体。


### 8.3 体验增强·第二批

| 项 | 入口 | 实现 |
|---|---|---|
| 复制变量值 | 树里 `y` | 新 API `helix.set_register(text, register?)` → `UiRequest::SetRegister` → 写未命名寄存器 `"`（同时写系统剪贴板 `+`）；`p` 即可粘贴 |
| 变量树过滤 | 树里 `/` | `T.filter` + `flatten()` 的"自身或后代命中"规则；命中的分支自动展开 |
| 协议流水 | `:debug-log [N\|clear]` | `helix-dap/src/transcript.rs` 环形缓冲（500 条，body 截 4KB），记录点与 `-vv` 的 `dap ->/<-` 同址 |

**没做的两项（有意）**

- **构建输出流式**（B10）：要在 worker 线程逐行推 `dap-output`，涉及新的"后台线程 → 主线程事件"
  通道与泵时序；价值是"等 `cargo build` 时 Output 不空白"，建议单独一轮做。
- **调试态 `:` 白名单**（B12）：`Document::apply` + `:write` 的只读闸门已经兜底，先不加
  命令白名单 —— 它会挡掉 `:theme`/`:echo` 这类无害命令，收益不抵麻烦。


### 8.4 C 档第一轮

| 项 | 状态 | 说明 |
|---|---|---|
| 调试视角（布局快照） | ✅ | `[plugins.dapui] perspective = true`：停止时 `helix.layout.get()` 存布局，继续/终止 `restore()`；单测钉住"只存一次/只恢复一次" |
| 零配置配方 | ⚠️ 部分 | 脚本语言的 `program` 检测本来就有；**适配器命令缺失时的报错**改成了"怎么修"（列 `languages.toml` 的 `command`/`args` 与文档位置）。`js-debug-dap` 仍需自配 argv |
| python 配方 | ❌ 回滚 | 加了内置 `debugpy` 模板并验证到 `initialize ok`，但 `launch` 拿不到回包（debugpy 等 `configurationDone`，而本 fork 的时序是先等 launch 响应）⇒ 会话会挂住，已回滚，文档里标为已知问题 |
| 多会话并排 / 时间线 | ❌ 未做 | 需要 UI 重构（树只画 active 会话） |
| 反向调试 / 内存视图 | ❌ 不做 | lldb-dap 的 `supportsStepBack`/`memoryReferences` 为 false，做了也是空壳 |


### 8.5 python 调试攻关记录（2026-10-26 末，**未落地**，留证）

**目标**：`:debug-start` 在 `.py` 上零配置可用（`pip install debugpy` 之后）。

**做过的改动与结论**

1. 内置 `debugpy` 模板（`python3 -m debugpy.adapter`）→ `initialize` 正常，
   capabilities 里 `supportsSetVariable` / `supportsEvaluateForHovers` 都是 true。
2. 但 `launch` **一直不回包**：debugpy 要等 `configurationDone`，而老实现是
   「spawn → initialize → **await launch** → 才 `insert_client`」⇒ receiver 还没进注册表，
   `initialized` 事件泵不起来 ⇒ 死锁。**这个根因是确定的**。
3. 改法①：把 launch 排成 `DapIntent::StartRequest`，在 `insert_client` 之后派发 →
   `launch 已确认` 出现了，会话也能停在断点（树里 `▾ add pyhello.py:2` + `b = 3 :int` ✓）。
4. 但**停止之后整个输入路径不再响应**（`Compositor::handle_event` 一次都不再被调用，
   按键全部失效；CPU 只有 5%，不是死循环）。所有 DAP 请求（threads/stackTrace/scopes/
   variables/evaluate）都有回包，所以不是"等某个请求超时"。
5. 加过两个对照实验都无效：① `pump_jobs` 重入保护（保留了下来，见下）；
   ② 把 launch 换成 `tokio::spawn` 彻底不占主线程 job。
   ⇒ 冻结点在 python 的 `Stopped` 处理之后，与 launch 无关，尚未定位。

**根因 B 找到了（2026-10-26 深夜续）**：`debugpy` 会把**被调试进程塞进 tty 的前台进程组**
（`tcsetpgrp`）。之后编辑器就成了后台进程组，读 tty 拿 `EIO`/`SIGTTIN` ⇒ termina 的输入流
彻底断掉。症状完全吻合：`Compositor::handle_event` 一次都不再被调用、CPU 空闲、DAP 日志正常、
`ps -o stat` 里 debuggee 带 `+`（前台组）而编辑器没有。

**修法**：spawn adapter 时 `setsid()`（`helix-dap/src/client.rs` 的 stdio 分支）—— adapter 自成
会话、没有控制终端，就改不了我们的 tty。顺带也隔离了所有 adapter 的后代进程。

**结果**：python 零配置调试**通了**（release 实测：命中 `add pyhello.py:2` → `n` → `:3` → `K` 求值），
C/C++ 路径无回归（命中 `add hello.c:4` → `n` → `:5` → `c` 结束）。

**保留下来的一个真修复**：`helix_js::pump_jobs` 的**重入保护** —— boa 的 job 队列不可重入，
在泵内再泵会挂死事件循环；`resolve` 侧"泵一次"的写法必须靠它兜底。

**下次接手需要的线索**：复现脚本 `/tmp/wkhx/pyrel*.py`；症状是
「python 命中后 `handle_event` 不再被调用、CPU 空闲、DAP 日志正常」；
首要怀疑方向是 `helix-view/src/handlers/dap.rs` 的 `Event::Stopped` 之后的**主线程 block_on**
（`debugger.request::<Threads>` 之类），以及 dapui 在 python 变量上触发的虚拟文本/装饰路径。


### 8.6 python 调试落地（本轮结论）

- `DapIntent::StartRequest`（insert_client 之后再发 launch）**保留**；
- `languages.toml` 内置 `debugpy` 模板 **保留**（`pip install debugpy` 后 `:debug-start` 直接跑当前文件）；
- `helix-dap` 的 stdio spawn 加 `setsid()`（上面 §8.5 的根因 B）。


---

## 9. 模型改版：gdb 式（2026-10-26，已落地）

用户反馈「两个面板 + 调试态 + 面板键」像散沙，选了 **gdb 式：一套键随时可用**。落地要点：

| 旧 | 新 |
|---|---|
| `PaneMode::Debug`（占位模式，拦编辑键，`space G` 进入） | **删掉**。暂停时用 `EditorView::handle_keymap_event` 的**前置接管**：`dap_keymap()` 里有的键直接执行，其余照旧 |
| 两套键位（编辑器 `l/r/t/C-c`… vs 面板 `t/f/s/R`…） | **一套**：面板对会话键一律 `ignore`（冒泡），只剩"跟选中项有关"的键 |
| `t` = 结束会话 / 切线程 同键异义 | `T` 结束、`t` 切线程（两边同义） |
| 面板 `l` 展开 vs 编辑器 `l` 启动 | 暂停表**不含** `l`（暂停时没东西可启动）；菜单仍保留 `l`。单测钉住"暂停表 ⊆ 菜单表，菜单只多 `l`" |
| 命中即拦编辑键 | 编辑照常（只读改成 `readonly_on_stop`，默认 `off`） |
| 会话结束后留一个空树 | `terminated`/`exited` 自动收起**自己开的**面板并恢复布局（`perspective`） |

关键实现：

- `Client::is_paused()` + `Editor::is_debug_paused()`；
- `ui/editor.rs::handle_keymap_event` 前置：`mode == Normal && count.is_none() && keymaps.pending().is_empty() && is_debug_paused()` ⇒ 查 `dap_keymap().search(&[event])`（含 SHIFT 翻转兜底）；
- `keymap/default.rs`：`space G` 与 `dap_keymap()` 是同一张表，有测试 `dap_keymap_matches_space_g`。


---

## 10. 丝滑度优化（2026-10-26 末，已落地）

gdb 之所以顺，一半在键位，一半在**反馈**：单步之后你总知道自己在哪、手上有什么。这轮补的就是反馈层：

| 项 | 之前 | 现在 |
|---|---|---|
| 停止位置 | 状态栏只有 `Thread 1 stopped because of breakpoint` | `add  —  Thread 1  /tmp/demo/hello.c:4`（函数 + 位置） |
| 光标 | 停在哪行都不知道，得自己去找 | **自动跳过去并居中**（只在当前 buffer 就是它时，不抢 buffer） |
| 手上有什么 | 要么开树一层层点，要么看 REPL | 树头部一行摘要 `#1 ⏸ t1 │ a=2 b=3 s=5`（当前帧 Locals，随单步/切帧更新） |
| 内联值 | 想学 VS Code 写行尾 | 试了，`set_virtual_text` **只能插在列 0**（会把代码右移）⇒ 默认关，摘要改放树头部 |
| 面板生命周期 | 命中开、终止留着空树 | 命中开；**正常退出全收**、**异常退出保留 Output**（切到该页并报退出码） |
| 停止时的 `?` | — | 一行键位速查（含"编辑不受影响"） |

顺带修：`expand_node` 的变量缓存与 `load_roots` 的 scopes 缓存现在同源（`T.cache[node.id]`），
否则"自动展开的 Locals"不进缓存，摘要/后续展开都要重拉一次。


### 10.1 REPL 补强（同轮）

| 项 | 实现 | 坑 |
|---|---|---|
| 多行表达式 | `unclosed_brackets()`（忽略字符串里的括号）：`Enter` 未配平时追加 `\n`，提示符变 `... `；配平后整段求值（lldb 能直接吃带换行的表达式） | 输入框仍是一行：换行显示成 `⏎` |
| 清屏 | REPL 页 `C-l` 清转录（gdb 同款）；其它 ctrl 键照旧放行 | — |
| 历史持久化 | `~/.cache/helix/dapui-repl-history`，最多 200 条、去重保序，多行条目转义成 `\n` | **必须同步读**：`run_async` 的 promise 在插件初始化/`open_bottom` 这些时机不被泵，`then/catch` 都不进 ⇒ 历史静默为空；换成 `helix.run('printf %s "$HOME"')` + `read_file`（阻塞 ~1ms）才确定 |
| 权限 | dapui 增加 `fs_read` / `fs_write`（仅用于这一个缓存文件，已在 `helix.plugin` 注释里写明） | — |
