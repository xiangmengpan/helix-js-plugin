# Rust（以及 C++）栈帧名太丑 —— 方案（待审核）

> **已实现（2026-10-28，按方案 A + keeper 档）**。下面是原始分析与落地细节。
>
> 现象：Rust 调试时右侧栈帧长这样（本机实测，`/tmp/rsdbg`）：
>
> ```
> ▸ _RNvYFEuINtNtNtCs…      function.rs:250
> ▸ _RINvNtNtCs2AWtUs…     backtrace.rs:166
> ▸ _RNCINvNtCs2AWtUs…     rt.rs:206
> ▸ call_once<(), (dyn core::ops::function::Fn<(), Output=i32> + core::marker::Sync + ...
> ▸ do_call<&(dyn core::ops::function::Fn<(), Output=i32> + ...
> ▸ catch_unwind<i32, &(dyn core::ops::function::Fn<(), Output=i32> + ...
> ```
>
> 想要的：`main` / `add` / `sum` / `len`。

---

## 1. 根因（已确认，不是我们的 bug）

| 环节 | 事实 |
|---|---|
| Rust 编译器 | 用 **v0 mangling**（`_RNv…` / `_RINv…` / `_RNCINv…`），Rust 1.59 起的默认方案之一 |
| lldb | 对 v0 **只认得一部分**：`_ZN…`（legacy）能解，`_RNv…` 这类原样返回 |
| DAP | `stackTrace` 的 `frame.name` **就是 lldb 给的字符串** ⇒ 我们只管显示，没做后处理 |

验证（同一条符号两种工具）：

```
$ echo '_RNvCs1TQt3UJmI7b_5rsdbg4main' | llvm-cxxfilt
rsdbg::main                    ← 能解
$ lldb: image lookup -n '_RNv…4main'
Summary: rsdbg`_RNvCs1TQt3UJmI7b_5rsdbg4main at main.rs:5   ← lldb 原样
```

---

## 2. 三个方案

| | 做法 | 代价 | 结论 |
|---|---|---|---|
| **A（推荐）** | 核心侧 demangle + 显示侧简化 | 0.5 天，1 个依赖（`rustc-demangle`，本机 cargo 缓存里已有 0.1.27，可离线编） | ✅ |
| B | 纯 JS 里 demangle | v0 的规范要实现几百行（backref、泛型、闭包…），易错 | ✗ |
| C | 调外部 `llvm-cxxfilt` 子进程 | 依赖外部工具、跨平台差、每次栈回溯 fork 一次 | ✗ |

---

## 3. 方案 A 的详细设计

### 3.1 核心侧：demangle

- 位置：`helix-dap`（`StackFrame` 落到我们结构体那一刻）或 `helix-view/src/handlers/dap.rs` 的帧转换处
- 规则：只处理"看起来像符号"的名字（`^_R` / `^_ZN` / 含 `::h[0-9a-f]{16}`），
  `rustc_demangle::try_demangle` 成功就用它，失败**原样保留**（不猜）
- 顺带去掉 lldb 加的前后缀：`[inlined] ` 前缀、` at file.rs:12:3` 尾巴
- 影响面：树里的帧、`f` 栈帧 picker、停止时的状态栏消息、`:debug-log` —— **一处修，四处都对**

### 3.2 显示侧：缩短（dapui，纯字符串，可关）

按顺序：

| 步骤 | 例 |
|---|---|
| 去 hash | `rsdbg::add::h1a2b3c4d5e6f7890` → `rsdbg::add` |
| 去泛型（`<…>` 配平后整体删） | `drop_in_place<alloc::vec::Vec<u8>>` → `drop_in_place` |
| 去 `{closure#N}` / `{{closure}}` | `main::{closure#0}` → `main` |
| 取**最后一段**路径 | `rsdbg::add` → `add`；`core::ptr::drop_in_place` → `drop_in_place` |
| 保留方法限定（可选） | `Point::len` → `len`（默认）；`frame_name = "keeper"` 时保留 `Point::len` |

**重名消歧**（重要）：同一屏里出现两个 `add` 时，第二个显示 `add (mod::add)`
—— 只对重名的那几个加后缀，其余保持干净。

### 3.3 另一个噪音源（顺手一起修）

现在"有源码的 std 帧"会和你的帧平铺在一起（`function.rs:250`、`rt.rs:206` …）。
规则改成：**名字仍是未解析的 mangled 符号**（`^_R…`）视为系统帧，折叠进
`… +N system frames`，哪怕它带源码。

### 3.4 配置

```toml
[plugins.dapui]
frame_name = "short"     # 默认：main / add / len
# frame_name = "full"    # rsdbg::add / core::ptr::drop_in_place
```

### 3.5 验收

- Rust 例子（`/tmp/rsdbg`）：树里出现 `main` / `add`，不再有 `_RNv…`
- C 例子（`/tmp/demobuild`）：`add` / `main` 不变；`__libc_start_main@@GLIBC_2.34` 去掉 `@@…` 版本尾巴
- C++ 例子（`std::vector<int>::push_back`）：显示 `push_back`；`operator<<` **不能**被泛型剥离误伤
- 单测 6 条：去 hash / 去泛型 / 保 `operator<<` / 闭包 / 取末段 / 重名消歧

---

## 4. 需要的确认

1. 方法/函数要不要保留类型限定？`len` vs `Point::len`（我建议**默认 `len`**，配置可切 `keeper` 档）
2. 系统帧判定要不要按 §3.3 收紧（mangled ⇒ 一律折叠）？我建议**要**
3. `frame_name` 三档够不够：`short` / `keeper`（保留 `Type::method`）/ `full` ？

---

## 5. 另外（已做，不用审）

「变量后面一个空格」已经改好：默认 `a = 2  int`（不再补列宽）。
想要列对齐的话开 `[plugins.dapui] align_columns = true`。


---

## 6. 落地记录

| 环节 | 实现 |
|---|---|
| demangle | `helix-dap/src/client.rs::clean_symbol()`，`stack_trace()` 两处（Client 与句柄版）都对 `frame.name` 调用 —— 树/`f` picker/状态栏/`:debug-log` 一次全好。依赖 `rustc-demangle = "0.1"`（本机缓存已有，离线可编）|
| 去修饰 | `[inlined] ` 前缀、` at file.rs:12:3` 尾巴、`@@GLIBC_2.34` / `@GLIBC_2.2.5`（单 `@` 只在后面像版本号时切）|
| 缩短 | dapui `frame_display(raw, mode)`：去泛型（尖括号配平，`operator<<` 不误伤）、去 `::h<16hex>`、去 `{closure#N}`、取末段 |
| 档位 | `[plugins.dapui] frame_name = "short"(默认) | "keeper" | "full"` |
| 消歧 | 同屏**可见**帧重名才补 `(模块)`；同一函数的多帧（`rsdbg::main` 与 `main`）不算重名 |
| 系统帧收紧 | 没源码 / 仍是 mangled / **源码不在工作区**（`helix.cwd()` 前缀判断）⇒ 折进 `… +N system frames`（文案去掉"(no source)"） |

**实测**（`/tmp/rsdbg`，`cargo build` 的 debug 二进制）：

```
之前：▸ _RNvYFEuINtNtNtCs…      function.rs:250
      ▸ call_once<(), (dyn core::ops::function::Fn<(), Output=i32> + …
      ▸ _RINvNtNtCs2AWtUs…     backtrace.rs:166
现在：›▾ main()                 main.rs:8
        ▾ Locals
           p = rsdbg:…(+20)  rsdbg::Point
      ▸ … +17 system frames
```

C 例子的 `add()` / `main()` 不受影响（`__libc_start_main@@GLIBC_2.34` 的版本尾巴也一并去掉）。
**单测**：`clean_symbol` 4 条（helix-dap）+ `frame_display/looks_mangled` 与"mangled 折叠"2 条（dapui）。
