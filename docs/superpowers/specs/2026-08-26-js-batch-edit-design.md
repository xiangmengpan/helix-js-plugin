# 设计:批量编辑事务(begin_edit/end_edit)

日期:2026-08-26
状态:草案(待审核)

## 1. 动机

泵循环**每帧** `take_edits` 并应用(application.rs:412),async 命令里 `await` 前后的多次 `doc.insert/replace/delete` 会被拆成多个事务(多次撤销)。`begin_edit()/end_edit()` 让插件显式声明事务边界:期间编辑积压,end 时合并应用为一个 Transaction(一次撤销)。同步命令现状已是一个命令一个事务,本 API 主要服务 async 跨 await 场景,对同步命令无行为变化。

## 2. API

```js
helix.begin_edit();   // 开启事务(可嵌套,计数)
helix.end_edit();     // 关闭事务(计数归零时允许编辑出队)
```

- 0 参;嵌套安全(深度计数)
- 无返回值

## 3. 实现

### helix-js(state.rs + commands.rs)

- `EDIT_TXN_DEPTH: Cell<usize>` + `with_txn_depth` 访问器
- `js_begin_edit` / `js_end_edit`:深度 +1 / -1(饱和到 0)
- `take_edits` 入口拦截(唯一改动点,所有调用路径共用):

```rust
pub fn take_edits() -> Vec<Edit> {
    if state::with_txn_depth(|d| *d > 0) {
        return Vec::new();  // 事务打开:积压不消费
    }
    state::with_edits(std::mem::take)
}
```

- lib.rs 注册 `begin_edit` / `end_edit`(0 参)

### 事务语义

- 事务打开期间,泵循环/命令路径/弹窗路径的 `take_edits` 均返回空——编辑留在队列
- `end_edit` 后,下一帧泵循环 `take_edits` 取到全部积压 → `apply_plugin_edits` 一次应用 = 一个 Transaction = 一次撤销
- `end_edit` 只关深度,应用交给既有泵循环,无新应用路径

## 4. 边界

- **begin 不 end**(插件 bug):编辑积压,下一个命令开始时 `run_command` 清空队列 → 编辑丢弃,不崩。文档注明
- 深度计数仅在归零瞬间放行;嵌套 begin/end 匹配

## 5. 验证

- **helix-js 单测**:begin 后 take_edits 返回空(积压);end 后取到积压编辑;嵌套 begin/end;end 多余调用(深度 0 时)不崩
- **integration 测试**(plugin_batch 或并入现有):async 命令 begin → await → insert → await → insert → end → 断言一次撤销(undo 一次回退两处编辑)。测试环境默认禁 LSP 不影响(不依赖 server)

## 6. 规模

- helix-js state.rs/commands.rs/lib.rs 3 文件 + 测试,约 1 任务。
