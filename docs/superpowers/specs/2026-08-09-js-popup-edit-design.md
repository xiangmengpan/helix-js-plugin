# 设计：弹窗编辑（v7）

日期：2026-08-09
状态：已批准（自主执行）

## 目标

弹窗 `onKey` 回调获得可编辑的 doc 句柄——弹窗菜单"选中即插入"成为可能。

## 新增/变更 API

```js
helix.open_popup({
  render: () => [...],
  onKey: (key, doc) => {          // ← doc 为新增第二参
    if (key.name === "Enter") {
      doc.insert(doc.cursor.row, doc.cursor.col, "inserted");
      return "close";
    }
    return "ignore";
  },
});
```

- `doc` 与命令 ctx.doc 同构（path/text/cursor + insert/replace/delete 编辑队列）
- 编辑在按键处理后应用：一次按键入队的编辑 = 一个事务（一个撤销点）
- onKey 未提供时默认行为不变（Esc→Close 其他→Ignore，无 doc 需要）
- 弹窗是模态层：打开期间文档不被其他途径修改；每次按键重新序列化快照——上次按键的编辑对下次可见

## 实现

### helix-js

- `popup_key(id, key)` 签名改为 `popup_key(id, key, ctx: &CommandContext) -> Result<PopupKeyResult>`：onKey 调用参数从 `(key)` 变为 `(key, doc)`（doc 经 doc_to_js 构建，含 cursor）
- 既有 popup_key 单测更新（补 ctx 参数，传空 CommandContext）
- 新增单测：onKey 里 `doc.insert(...)` → take_edits 返回正确编辑

### helix-term（ui/plugin_popup.rs）

`PluginPopup::handle_event`（当前忽略 cx）：
1. 构建 CommandContext（current_ref! 序列化，与 emit_plugin_event 同款代码）
2. `helix_js::popup_key(self.id, &key, &ctx)`
3. 结果处理不变（Close→pop+close_popup；Handled→Consumed；Ignored→Ignored）
4. 无论结果如何：drain `take_cursor_requests` + `take_edits` + `take_messages` 并应用/显示（复用 apply_cursor_requests/apply_plugin_edits；apply_plugin_edits 在 typed.rs，需 pub(crate) 可见）

> 注意：Close 路径的 Callback 里也会 take_messages（既有逻辑）——编辑应用应在 pop 之前同步完成（handle_event 内），避免弹窗关闭后文档变化延迟。

## 测试

- **helix-js 单测**：popup_key 传 ctx → onKey 编辑 → take_edits 正确；既有 popup_key 测试适配新签名
- **集成测试**（tests/test/plugin_popup_edit.rs）：`:snippet` 弹窗（↑↓ 选择 + Enter 插入）→ Down×2 → Enter → 断言文档包含选中片段且弹窗关闭

## 非目标

- onKey doc 的 selection（只给 cursor——YAGNI）
- 渲染期编辑（render 回调无 doc——只 onKey）
- 多弹窗并存编辑

## 涉及文件

- `helix-js/src/lib.rs`（popup_key 签名 + doc 参数 + 单测）
- `helix-term/src/ui/plugin_popup.rs`（handle_event 接线：ctx 构建 + drain 应用）
- `helix-term/tests/test/plugin_popup_edit.rs`（新）
