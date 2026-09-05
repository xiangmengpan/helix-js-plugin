use crate::compositor::{Component, Context, Event, EventResult};
use crate::ui::editor::EditorView;
use helix_view::graphics::Rect;
use helix_view::ViewId;
use tui::buffer::Buffer as Surface;

/// 单 view 编辑器叶子：显示指定 view（绑定 buffer）到叶子矩形。
/// view 本体注册在 editor.tree（register_flat，可被 core 路由 current! 命中）,
/// 本叶子只持 id;渲染时从 tree 拉取并就地设置 view.area。
/// 按键 Ignored → 事件路由兜底到 leaf 0 的键位处理,按 tree.focus(=活动叶 view)命中正确 doc。
pub struct BufferLeaf {
    pub view_id: ViewId,
}

impl Component for BufferLeaf {
    fn handle_event(&mut self, _event: &Event, _cx: &mut Context) -> EventResult {
        EventResult::Ignored(None)
    }
    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        surface.set_style(area, cx.editor.theme.get("ui.background"));
        let vid = self.view_id;
        // view 已被移除(叶子关闭竞态/外部 tree 清理)→ 白屏,不 panic
        if !cx.editor.tree.contains(vid) {
            return;
        }
        // 树内 canonical view:就地赋值叶子几何(render_view 依赖 view.area 裁剪)
        cx.editor.tree.get_mut(vid).area = area;
        let editor = &*cx.editor;
        let view = editor.tree.get(vid);
        let Some(doc) = editor.document(view.doc) else {
            return;
        };
        let is_focused = editor.tree.focus == vid;
        EditorView::render_view(editor, doc, view, area, surface, is_focused, true);
    }
    fn type_name(&self) -> &'static str {
        "BufferLeaf"
    }
}
