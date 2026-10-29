# `C-d`（调试态）与 `<space>G`（DAP 键位树）能不能融合？

> 状态：**设计稿**（三个方案 + 推荐）
> 现状：`space G` 是 sticky keymap（`keymap/default.rs:216`），`C-d` 是平级模式
> （`compositor.rs` 的 `PaneMode::Debug`）。两套键位**各写一遍**，已经出现语义漂移。

---

## 0. 先承认现状的漂移（这就是"该融合"的证据）

| 动作 | `space G` | `C-d`（Debug 模式） | 问题 |
|---|---|---|---|
| 继续 / 暂停 | `c` / **`h`** | `c` / **`p`** | 暂停两个键 |
| 单步 | `i/o/n` | `i/o/n` | 一致 |
| 切线程 / 栈帧 | **`s t` / `s f`**（两键） | **`t` / `f`**（单键） | 两套手感 |
| 结束会话 | **`t`** | 无（`t` 被线程占） | 冲突 |
| 断点条件 / 日志 | `C-c` / `C-l` | 无 | 缺失 |
| 启动 / 重启 / 会话 / 异常 | `l/r/S/e/E` | 无 | 缺失 |
| 变量 | `v` | `v` | 一致 |

同一批命令，两套表迟早会继续漂（这正是 which-key 当初"单一来源"要解决的问题的同一类）。

---

## 1. 方案 A：**Debug 模式复用 `<space>G` 那棵树**（推荐）

**一句话**：把 `space G` 的子树抽成一个可复用的 `KeyTrie`，Debug 模式**按键时查同一棵树**、**提示时用同一份 entries**。两个入口、一份定义。

```rust
// 新文件 keymap/dap.rs（或 default.rs 里的 pub fn）
pub fn dap_keymap() -> KeyTrie {        // 就是现在 default.rs:216 那段
    keymap!({ "Debug (experimental)" sticky=true
        "l" => dap_launch, ... })
}

// default.rs
"G" => crate::keymap::dap::dap_keymap(),

// compositor.rs（Debug 模式）
PaneMode::Debug => {
    let trie = crate::keymap::dap::dap_keymap();     // 或缓存一次
    match trie.lookup(key) {
        Matched(cmd)  => cmd.execute(&mut ccx),
        Pending(node) => self.debug_pending = Some(node),   // `s` 之后等第二键
        NotFound      => { /* 移动键 j/k/g/G 另外处理；其余吞掉 */ }
    }
}
// 提示：pane_mode_entries(Debug) 直接从 dap_keymap().entries() 生成
```

**融合后的键位（只有一套）**

| 键 | 动作 | 键 | 动作 |
|---|---|---|---|
| `c` / `h` | 继续 / 暂停 | `b` | 切换断点 |
| `i` / `o` / `n` | 进入 / 跳出 / 跳过 | `v` | 变量 |
| `l` / `r` | 启动 / 重启 | `t` | 结束会话 |
| `s t` / `s f` | 切线程 / 切栈帧 | `S` | 切会话 |
| `C-c` / `C-l` | 条件 / 日志断点 | `e` / `E` | 启用 / 禁用异常 |
| `j k g G` | **仅 Debug 模式**：移动/滚动（不在 DAP 树里，模式内另给） | `Esc` / `C-d` | 退出调试态 |

`space G` 完全不变（前缀式、按完即走）；`C-d` 就是"没有前缀、并且拦住编辑键"的同一批键。
**新增调试命令只改 `dap_keymap()` 一处**，which-key / 内置 Info / Debug 提示全自动跟上。

| 维度 | 评价 |
|---|---|
| 实现量 | **1.5 天**（抽 `dap_keymap()` 0.5d；Debug 路由 + 两键 pending 0.5d；提示复用 + 测试 0.5d）|
| 破坏性 | **无**：`space G` 一个键都不动；Debug 模式原来的 `p/t/f` 由 `h`/`t`/`s f` 取代（模式是新的，没有存量肌肉记忆） |
| 风险 | 中低：`keymap!` 宏在独立函数里可用；`KeyTrie` 的 lookup/entries 都是 `pub` |
| 收益 | 一份键位、一份提示、零漂移；`C-d` 与 `space G` 变成"同一系统两种节奏" |

---

## 2. 方案 B：`<space>G` 直接**进入** Debug 模式（彻底合并成一个键）

- 删掉 sticky keymap，`space G` 只做 `enter_debug_mode`；所有键位/提示来自 `pane_mode_entries(Debug)`。
- 优点：概念最干净——"调试只有一种键位"。
- 缺点（我不建议）：
  1. **破坏存量**：`book/src/keymap.md`、`docs`、你 config 里的 `space G b`、以及所有用户的肌肉记忆；
  2. "只想打个断点"变成 `space G` → `b` → `Esc` 三步，且期间 `i/d/:` 被拦；
  3. 一次性动作（launch/attach picker）本来就是"菜单式"更自然，锁进模式反而别扭。

---

## 3. 方案 C：不融合，只加"升降级"与一致性测试（最小方案）

- `space G` 保持；Debug 模式里 `space G` 仍可用（现在就是这样）；两边键位**用测试钉住**：
  「Debug 模式绑定的每个动作，都必须能在 `space G` 树里找到同名命令」。
- 再加两个便利：`space G D` = 进入调试态；Debug 里 `?` = 显示 `space G` 的完整表。
- 优点：**0.5 天、0 风险**；缺点：仍是两份表（测试只能防"命令消失"，防不了"键位取向漂移"，比如暂停是 `h` 还是 `p`）。

---

## 4. 推荐

**做 A**。理由：

1. 它正面回答了"能不能融合"——**能，而且融合点不在"键"上，在"键位表的来源"上**。`space G` 是**前缀式一次性触发**，`C-d` 是**无前缀的沉浸态**，两个入口各有价值；共享一张表才是既融合又不互相绑架的做法。
2. 实现量 1.5 天、对存量零破坏、`space G` 的文档与用户配置一行都不用改。
3. 与既有的两处"单一来源"教训一致：which-key 的键位表（`pane_mode_entries` 是唯一来源）和 DAP 断点表（`BreakpointStore` 是唯一来源）。

**落地步骤**

| 步 | 内容 | 验证 |
|---|---|---|
| 1 | 把 `default.rs:216` 的 Debug 子树抽成 `dap_keymap()`，`default()` 引用它 | `cargo test`（keymap 一致性）+ 真机 `space G` 行为不变 |
| 2 | `compositor` 的 `PaneMode::Debug` 改成"查 `dap_keymap()` + 执行 `MappableCommand`"，并处理 `s` 的两键 pending | 单测：`c`→`dap_continue`、`s t`→`dap_switch_thread`、`j`→移动、`i`→被挡 |
| 3 | `pane_mode_entries(Debug)` 从 `dap_keymap().entries()` 生成 | which-key 的 `C-d` 提示与 `space G` 一致（pty 截屏） |
| 4 | 测试钉住"Debug 动作 ⊂ `space G` 树"（防回退） | `cargo test` |

**明确不做**：不在 `space G` 里再加一套重复的 Debug 键；不做 `space G` 进模式（方案 B）；
`j/k/g/G` 这种"编辑器移动"不塞进 `dap_keymap()`（它不是调试动作，塞进去 `space G` 的提示会变噪音）。

---

## 5. 验收清单

- [ ] `space G` 的键位、提示、文档**完全不变**
- [ ] `C-d` 里按 `c/h/i/o/n/b/v/t/s t/s f/S/e/E/C-c/C-l` 与 `space G` 后按同键**行为逐条一致**
- [ ] `C-d` 里按 `i`（和 `space G i` 的 step-in 同键）——**注意**：`i` 在 Debug 里是 step-in，不是"插入"；编辑键拦截表要排除已绑定的键
- [ ] two-key `s t` / `s f` 在 Debug 里有可见的第二键提示（which-key 的 `Switch` 子表）
- [ ] 新增调试命令只改 `dap_keymap()`，Debug 提示与 `space G` 同时生效


---

## 6. 落地（2026-10-26）：选了"B 的入口 + A 的单一来源"

需求最终定为：**`<space>G` 直接进调试态，移除 `C-d`**。实现上仍是 A 的核心（一张表）：

- `keymap/default.rs::dap_keymap()` = 调试键位唯一来源；`space` 下的 `G` 改成命令 `enter_debug_mode`。
- `PaneMode::Debug` 的按键路由 = `dap_keymap().search(&[key])`（`s` 为 Node 时进 pending，等第二键）；
  未命中先试 `j/k/g/G`，再试编辑键拦截，其余**穿透**给编辑器。
- 提示表（`pane_mode_entries(Debug)`）与 `dap_keymap()` 的键集由单测钉住。
- `space G b` / `space G l` 这类老用法**语义不变**（只是先进入模式）。
