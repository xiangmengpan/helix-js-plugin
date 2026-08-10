# 设计：布局收缩（v10-③，并行 wave 1，最难）

日期：2026-08-09
状态：已批准（自主执行）

## 目标

面板打开时编辑器区域收缩（面板推挤而非覆盖）。右侧面板 = 编辑器宽度减面板宽；左侧/底部同理。

## 实现

**compositor.rs render 特化**（helix-term 内）：

```rust
pub fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
    // 布局收缩：面板存在时给非面板层减掉面板条带
    let panel = self.find::<ui::PluginPanel>();
    if let Some(panel) = panel {
        let (panel_area, rest_area) = panel.split_area(area);  // PluginPanel 提供
        for layer in &mut self.layers {
            let is_panel = layer.type_name() == std::any::type_name::<ui::PluginPanel>();
            layer.render(if is_panel { panel_area } else { rest_area }, surface, cx);
        }
    } else {
        for layer in &mut self.layers {
            layer.render(area, surface, cx);
        }
    }
}
```

- `PluginPanel::split_area(area) -> (Rect, Rect)`：按 side/size 从 area 切出面板条带 + 剩余区（right: 右侧条带；left: 左侧；bottom: 底部）
- PluginPanel::render 改为填满传入的 area（不再自算 Rect——由 compositor 分好）
- 面板层渲染顺序：在 EditorView 之后（layers 顺序天然如此——面板是后 push 的）→ 面板画在其条带上，编辑器画在收缩区，无重叠
- 事件：面板的 handle_event 不因布局改变（壁 ② 负责输入）

## 测试

- 集成：开 right 面板 size 20 → compositor 渲染进 Buffer → 断言编辑器状态栏（底部行由 EditorView 画）宽度 = area - 20（底部行右侧 20 列是面板内容而非状态栏文本）
- 或断言 buffer 底部行右侧 20 列 cell 内容属于面板（不含 "NORMAL" 等状态栏特征）

## 涉及文件

- `helix-term/src/compositor.rs`（render 特化）
- `helix-term/src/ui/plugin_panel.rs`（split_area + render 改填满）

## 非目标

- 多面板同时收缩（单面板）、面板拖拽调整、布局动画
- 收缩对 popup 定位的影响（popup 仍相对全屏定位）
