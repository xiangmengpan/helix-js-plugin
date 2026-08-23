// features/tabbar.js — 布局标签条示范组件（JS 视图层）
// 依赖:helix.TABBAR_ID(compositor 顶部槽位)、helix.get_layout()、helix.get_component_state()
// 效果:屏幕顶部 1 行显示各叶子标签,活动叶子高亮;窗口模式二期标签条的基础示范。
helix.plugin("tabbar", { deps: ["icons.js"] });

(function () {
  const ICONS = helix.load("icons.js") || null;

  // 叶子图标:编辑器=当前文件图标;终端=终端图标;其他=面板
  function leaf_icon(leaf, layout) {
    if (leaf.id === 0) return ICONS ? ICONS.getFileIcon(layout.path || "") : "\uf15b";
    const st = helix.get_component_state(leaf.id);
    if (st && st.mode) return ICONS ? ICONS.getFileIcon("term.sh") : "\uf489"; // 终端
    return ICONS ? ICONS.getDirIcon(false) : "\uf115"; // 面板
  }

  function leaf_label(leaf, layout) {
    if (leaf.id === 0) {
      const name = layout.path ? layout.path.split("/").pop() : "editor";
      return name;
    }
    const st = helix.get_component_state(leaf.id);
    return (st && st.title) || "leaf-" + leaf.id;
  }

  helix.set_component_render(TABBAR_ID, (ctx) => {
    const layout = helix.get_layout();
    if (!layout || !layout.leafs || layout.leafs.length === 0) {
      return [{ type: "text", text: "" }];
    }
    const parts = layout.leafs.map((leaf) => {
      const active = leaf.id === layout.active;
      const icon = leaf_icon(leaf, layout);
      const label = leaf_label(leaf, layout);
      const text = (active ? "\u25b8 " : "  ") + icon + " " + label;
      return { type: "text", text, style: active ? "ui.selection" : "ui.statusline.inactive" };
    });
    return { type: "row", children: parts, gap: 2 };
  });
})();
