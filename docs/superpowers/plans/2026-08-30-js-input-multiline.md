# input 多行(multiline)实现计划

> **面向 AI 代理的工作者:** 必需子技能:使用 superpowers:subagent-driven-development(推荐)或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框(`- [ ]`)语法来跟踪进度。

**目标:** `input` 节点加 `multiline` 开关:多行输入(Enter 换行、行感知光标移动、多行渲染),单行行为逐字节不变。

**架构:** `InputState` 加 `multiline`;`input_edit` 纯函数扩展(行感知光标);`CompNode::Input` 加 `multiline` 字段驱动 comp_layout 多行渲染。

**技术栈:** Rust(helix-js boa 运行时 + helix-term)、tokio 集成测试。

**规格:** `docs/superpowers/specs/2026-08-30-js-input-multiline-design.md`(已批准)

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `helix-js/src/input.rs` | `InputState.multiline`、`input_edit` 多行扩展 + 行 helper |
| `helix-js/src/types.rs` | `CompNode::Input` 加 `multiline: bool` |
| `helix-js/src/popup.rs` | input 节点解析 `multiline` 参数 → InputState 初始化 + CompNode |
| `helix-js/src/lib.rs` | `input_edit` 多行单测 |
| `helix-term/src/ui/comp_layout.rs` | Input 分支多行渲染 |
| `helix-term/tests/test/plugin_panel_focus.rs` 或新文件 | multiline integration 测试 |
| `docs/plugin-api.md` | input 节点文档加 multiline |

关键实现细节(执行时必读):

- **行模型**:value 含 `\n`;光标是全文本 char 索引(不变)。行 i 的范围由 `\n` 分段确定。
- **input_edit 扩展**(纯函数,仿现有 match 结构):

```rust
/// value 按 \n 分段的每行 char 范围:Vec<(start, end)>(end 不含 \n;末行含尾部)
fn line_ranges(value: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in value.chars().enumerate() {
        if c == '\n' {
            out.push((start, i));
            start = i + 1;
        }
    }
    out.push((start, value.chars().count()));
    out
}

/// 光标所在行号
fn line_of(value: &str, cursor: usize) -> usize {
    line_ranges(value).partition_point(|&(s, _)| s <= cursor).saturating_sub(1)
}
```

  `input_edit` 的 `match key` 加多行分支(仅 `state.multiline` 时激活;单行走现有分支不变):

```rust
"Enter" if state.multiline => {
    // 光标处插入 \n
    let mut chars: Vec<char> = state.value.chars().collect();
    chars.insert(state.cursor, '\n');
    state.value = chars.into_iter().collect();
    state.cursor += 1;
    Some(state.value.clone())
}
"Up" | "Down" if state.multiline => {
    let ranges = line_ranges(&state.value);
    let line = line_of(&state.value, state.cursor);
    let col = state.cursor - ranges[line].0;
    let target = match key {
        "Up" if line > 0 => line - 1,
        "Down" if line + 1 < ranges.len() => line + 1,
        _ => line,
    };
    if target != line {
        let tcol = col.min(ranges[target].1 - ranges[target].0);
        state.cursor = ranges[target].0 + tcol;
    }
    None
}
"Home" if state.multiline => {
    state.cursor = line_ranges(&state.value)[line_of(&state.value, state.cursor)].0;
    None
}
"End" if state.multiline => {
    state.cursor = line_ranges(&state.value)[line_of(&state.value, state.cursor)].1;
    None
}
```

  `"Left"`/`"Right"`/`"Backspace"`/`"Delete"` 的多行语义(在现有分支上按 multiline 调整):
  - Left:光标在行首且非首行 → 上一行行尾(`cursor - 1` 天然落点,现有 `saturating_sub(1)` 即可,但需在行首时**跳过 \n**:`if cursor > 0 && value[cursor-1] == '\n' { cursor - 1 }`——现有实现 char 索引下 `cursor-1` 就是 \n 位置,移动后落在 \n 上,需再 -1 到上一行行尾。**实现时验证**:标准行为是行首按 Left → 上一行行尾;执行时按现有 char 索引语义推导并单测锁定)
  - Right:行尾(非末行)按 Right → 跳过 \n 到下一行行首(同理)
  - Backspace:光标在行首且非首行 → 删除前一个 \n(跨行合并);现有实现 `cursor-1` 删 \n 已覆盖(删掉的是 \n,前后行合并)——**验证现有行为即可,必要时补分支**
  - Delete:光标在行尾且非末行 → 删 \n(现有实现删 cursor 处字符即 \n——验证)
  - **执行时:先写单测锁定语义,再按单测调整实现**(TDD)
- **节点解析**(popup.rs input 分支 ~1165):`let multiline = obj.get("multiline")...is_undefined => false, v => try_js_into::<bool>`(仿任务 1 的 focusable 模式)→ InputState 初始化 `InputState { value, cursor: c, multiline }` + `CompNode::Input { value, cursor, width, id, flex, multiline }`。Occupied 分支读状态时 multiline 已存于状态。
- **渲染**(comp_layout.rs:311 Input 分支):multiline 时按 \n 分行渲染:

```rust
CompNode::Input { value, cursor, width, multiline, .. } => {
    let limit = width.unwrap_or(u16::MAX).min(viewport.0) as usize;
    let mut lines = value.split('\n').collect::<Vec<_>>();
    // 光标所在行插 |(光标 char 索引 → 行内偏移)
    if *multiline {
        let mut remaining = *cursor;
        for (i, line) in lines.iter_mut().enumerate() {
            let llen = line.chars().count();
            if remaining <= llen {
                let mut chars: Vec<char> = line.chars().collect();
                chars.insert(remaining.min(chars.len()), '|');
                *line = chars.into_iter().collect::<String>().leak(); // 借用问题——用 owned Vec 替代
                break;
            }
            remaining -= llen + 1; // +1 跳过 \n
        }
        // 每行截断到 limit,生成多行 StyledLine
        lines.iter().map(|l| StyledLine::plain(l.chars().take(limit).collect())).collect()
    } else {
        // 现有单行逻辑不变
        ...
    }
}
```

  **注意 `split('\n')` 返回借用,插 `|` 需 owned——执行时用 `value.split('\n').map(|l| l.to_string()).collect::<Vec<_>>()` 再改,或按实际最小改动**(渲染语义:多行 StyledLine 数组;布局高度由既有布局机制处理,多行渲染超出视口被裁剪——边界已注明)。
- **测试命令**:`cargo build -p helix-term`(实现者);`cargo test -p helix-js`(单测,~11s 快——实现者也可跑);`cargo test -p helix-term --features integration --test integration plugin_panel_focus`(控制者);clippy 零;fmt clean(注意并行批次 fmt 漂移——只保证本批次文件)。

---

### 任务 1:input_edit 多行扩展 + 单测(纯函数主体)

**文件:** input.rs、lib.rs

- [ ] **步骤 1:写失败单测(多行语义)**

`helix-js/src/lib.rs` 测试模块加(用 `crate::input::input_edit` + 手工 InputState):

```rust
#[test]
fn input_edit_multiline() {
    let mut s = InputState { value: "ab\ncd".into(), cursor: 1, multiline: true };
    // Enter 光标处插 \n
    assert_eq!(input_edit(&mut s, "Enter"), Some("a\nb\ncd".into()));
    assert_eq!(s.cursor, 2);
    // Home/End 行级
    input_edit(&mut s, "End");  // cursor → 行尾
    // (按实际光标位置补断言——执行时按行模型推演)
}
```

  完整断言清单(执行时逐条推演精确光标值):
  1. Enter 光标处插 \n,cursor 推进
  2. Up:列保持(光标 (row=1, col=1) → 上移 (0,1));短行 clamp 行尾
  3. Down:同理下移
  4. Home → 行首;End → 行尾
  5. Left:行首按 Left → 上一行行尾;Right:行尾按 Right → 下一行行首
  6. Backspace:行首删 \n 合并行;Delete:行尾删 \n 合并
  7. 单行 input(multiline: false)行为与现有完全一致(回归:现有测试全绿)

  预期:FAIL(Enter/Up 等无 multiline 分支)。

- [ ] **步骤 2:实现 input_edit 扩展 + 行 helper**(见关键细节;TDD:按单测调整)

`InputState` 加 `pub multiline: bool`(现有构造点 popup.rs:1189 补——**任务 2 才真正接线,任务 1 先加字段,编译错误指向构造点则补默认值**,或任务 1 一并加 `multiline: false` 占位)。

- [ ] **步骤 3:单测转绿 + 提交**

```bash
cargo test -p helix-js input_edit
cargo test -p helix-js   # 全量确认单行回归
```

```bash
git add helix-js
git commit -m "feat(js): input_edit 多行扩展——Enter 换行/Up-Down 列保持/Home-End 行级/跨行移动与合并(纯函数)"
```

---

### 任务 2:节点接线 + 多行渲染 + integration

**文件:** types.rs、popup.rs、comp_layout.rs、plugin_panel_focus.rs(或新文件)、plugin-api.md

- [ ] **步骤 1:CompNode + 节点解析 + 渲染**

1. `types.rs` `CompNode::Input` 加 `multiline: bool`(编译错误会指出所有构造点——comp_layout.rs:652/661 测试构造、popup.rs:1196)
2. `popup.rs` input 分支:解析 multiline(仿 focusable 模式,缺省 false)→ InputState 初始化带 multiline + CompNode::Input 带 multiline
3. `comp_layout.rs:311` Input 分支:multiline 多行渲染(见关键细节;注意 borrow/owned)

- [ ] **步骤 2:写失败 integration 测试**

`helix-term/tests/test/plugin_panel_focus.rs`(或新 `plugin_input_multiline.rs`,按既有 input 测试位置)加:

```rust
// multiline input:Enter 插入换行(不提交);Up/Down 光标行间移动
#[tokio::test(flavor = "multi_thread")]
async fn plugin_input_multiline_enter_inserts_newline() -> anyhow::Result<()> {
    // open_panel({focusable:true, render: () => ({type:"column", children:[
    //   {type:"input", id:"m", multiline:true, value:"ab\ncd"}
    // ]})})
    // Tab 聚焦 input → 光标(行尾)→ 输入 "X" → Enter → 值含 \n(白盒 set_input_value/读回)
    // 断言:值 "ab\ncdX\n" 或按光标实际位置(执行时推演)
}
```

  白盒断言模式参照 plugin_panel_focus.rs 既有测试(JS 状态读回)。

- [ ] **步骤 3:转绿 + 文档 + 提交**

```bash
cargo build -p helix-term
cargo test -p helix-js
cargo test -p helix-term --features integration --test integration plugin_panel_focus   # 控制者
cargo clippy --all-targets 2>&1 | tail -3
```

`docs/plugin-api.md` input 节点小节加 `multiline`(Enter 换行/Up-Down 行间/Home-End 行级;提交用外部 button 或 onKey;单行不变)。

```bash
git add helix-js helix-term docs/plugin-api.md
git commit -m "feat(js,term): input multiline 接线——节点参数/多行渲染 + integration"
```

---

## 自检记录

**规格覆盖度:**
- 2.1 API(multiline 开关)→ 任务 2 步骤 1-2 ✓
- 2.2 语义(Enter 换行/Up-Down 列保持/Home-End 行级/Left-Right 跨行/Backspace-Delete 合并/单行不变)→ 任务 1 步骤 1-2 ✓
- 2.3 数据流(InputState.multiline/input_edit/渲染/set_input_value 不变)→ 任务 1 步骤 2、任务 2 步骤 1 ✓
- 2.4 边界(布局高度/单行不变/IME 不做)→ 任务 2 步骤 3 ✓
- 3.1 单测 7 项 → 任务 1 步骤 1-3 ✓
- 3.2 integration 3 项 → 任务 2 步骤 2-3 ✓

**占位符扫描:** 无 TODO/待定;Left/Right/Backspace/Delete 的精确语义标注"执行时按单测锁定"(TDD 而非预先断言)。✓

**类型一致性:** `multiline: bool` 在 InputState/CompNode::Input/js 解析三处一致;input_edit 签名不变。✓
