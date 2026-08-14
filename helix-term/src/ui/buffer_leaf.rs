use crate::compositor::{Component, Context, Event, EventResult};
use crate::ui::editor::EditorView;
use helix_view::graphics::Rect;
use tui::buffer::Buffer as Surface;

/// 单 view 编辑器叶子：显示指定 view（绑定 buffer）到叶子矩形。
/// 一期：按键 Ignored（路由兜底到编辑器叶子 id=0）；多 view/跨叶移动二期。
/// view 独立持有（不进 editor.tree，避免主编辑器叶子重复渲染该 view）。
pub struct BufferLeaf {
    pub view: helix_view::view::View,
}

impl Component for BufferLeaf {
    fn handle_event(&mut self, _event: &Event, _cx: &mut Context) -> EventResult {
        EventResult::Ignored(None)
    }
    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        surface.set_style(area, cx.editor.theme.get("ui.background"));
        let Some(doc) = cx.editor.document(self.view.doc) else {
            return;
        };
        self.view.area = area;
        let is_focused = cx.editor.tree.focus == self.view.id;
        EditorView::render_view(cx.editor, doc, &self.view, area, surface, is_focused, true);
    }
    fn type_name(&self) -> &'static str {
        "BufferLeaf"
    }
}
