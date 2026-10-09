# 报告:任务 1 input_edit 多行扩展 + 单测(纯函数主体)

计划:`docs/superpowers/plans/2026-08-30-js-input-multiline.md` 任务 1
Commit:`420d73965`

## 实现内容

1. **`helix-js/src/input.rs`**
   - `InputState` 加 `pub multiline: bool`
   - 新增行模型 helper:`line_ranges`(按 `\n` 分段的每行 char 范围,`end` 不含 `\n`,末行含尾部)、`line_of`(`partition_point` 求光标所在行;光标落在 `\n` 上属上一行行尾)
   - `input_edit` 加多行分支(仅 `state.multiline` 激活,单行路径逐字节不变):
     - `Enter` → 光标处插 `\n`,cursor 推进
     - `Up`/`Down` → 行间移动保持列,短行 clamp 到行尾;首行 Up/末行 Down 不动
     - `Home`/`End` → 行首/行尾(带 guard 的新分支,原单行分支保留为 fallback)
   - **Left/Right/Backspace/Delete 零改动**——验证现有 char 索引实现已天然覆盖多行语义:行尾 = `\n` 位置(独占 end 模型),行首 Left → `cursor-1` 即上一行行尾;行尾 Right → `cursor+1` 即下一行行首;行首 Backspace 删 `cursor-1` 的 `\n` 合并;行尾 Delete 删 cursor 处 `\n` 合并。与单测锁定一致
   - `dispatch_input_key` 默认 entry 补 `multiline: false`
   - `js_set_input_value` **保留既有 multiline 标志**(`m.get(...).map(|s| s.multiline).unwrap_or(false)`):set_input_value 只改值,若覆盖新建状态会把多行 input 静默降级为单行(任务 2 接线的潜在 bug,1 行修掉)

2. **`helix-js/src/popup.rs`** — Vacant entry 初始化补 `multiline: false` 占位(任务 2 从节点参数解析接线)

3. **单测 `input_edit_multiline`**(input.rs `mod tests`,与现有 input_edit 测试同处)覆盖 brief 完整断言清单:Enter 插 `\n` 推进 / Up 列保持 + 短行 clamp / Down 下移 / 首行 Up·末行 Down 不动 / Home-End 行级 / Left 行首→上一行行尾 / Right 行尾→下一行行首 / Backspace 行首删 `\n` 合并 / Delete 行尾删 `\n` 合并

## TDD 证据

- **RED**:加字段 + 单测(无多行分支)跑 `cargo test -p helix-js input_edit_multiline` → FAILED
  `left: None, right: Some("a\nb\ncd")`(Enter 无 multiline 分支)
- **GREEN**:加行 helper + match 分支后 `cargo test -p helix-js input_edit` → 3 passed(含 2 个单行回归)

## 测试与验证

- `cargo test -p helix-js input_edit` → 3 passed
- `cargo test -p helix-js` → 93 passed(全量单行回归;连跑 8 次全绿)
- `cargo clippy -p helix-js --all-targets` → 零告警
- `cargo fmt -p helix-js` 后**还原了 picker.rs 的 fmt 漂移**(另一批次的遗留,不属于本任务,不入 commit)

## 自审

- **YAGNI**:Left/Right/Backspace/Delete 未加冗余分支——现有 char 索引语义已正确,单测锁定;加了反而重复逻辑
- 行 helper 均私有,无未用代码;Home/End 分支重复调用 line_ranges 是 O(n) 小开销,既有 ponytail 注释已覆盖(输入值 <100 字符)
- `multiline` 字段在 helix-js 内唯一消费点是 input_edit;term 侧渲染由任务 2 接

## 关注点

- **观察到的 1 次全量测试红**(37 passed / 56 failed):56 个失败全在 lib.rs 测试,符合 TEST_LOCK 中毒级联特征(某 lib.rs 测试偶发 panic 毒化其锁,后续全部失败)。后续 8 次连跑全绿,master 基线也绿;本任务单测用 input.rs 独立 TEST_LOCK,不可能毒化 lib.rs 锁。判定为**既有 flaky(锁中毒级联),与本改动无关**,未定位到具体毒化测试(无法复现)
- popup.rs:1189 的 `multiline: false` 是任务 2 接线点(解析节点参数),任务 1 按 brief 占位
