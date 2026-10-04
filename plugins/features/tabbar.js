// features/tabbar.js — 布局标签条示范组件（JS 视图层）
// 依赖:helix.TABBAR_ID(compositor 顶部槽位)、helix.get_layout()、helix.get_component_state()
// 效果:屏幕顶部 1 行显示各叶子标签,活动高亮;点击标签聚焦。
helix.plugin("tabbar", { deps: [] }); // 图标已收进核心,不再需要 deps

// 插件配置(方案 C):config.toml [plugins.tabbar]
helix.define_config("tabbar", {
  show_dirty: { type: "boolean", default: true, doc: "标签显示未保存标记" },
  max_labels: { type: "number", default: 12, doc: "标签条最多显示数" },
});
const CFG = helix.get_config("tabbar") || {};

(function () {
  // 图标:直接用核心 API(原先 `helix.load("lib/icons.js") || null` + 各处 `ICONS ? … : 回退` 已删)

  // 最近一次渲染的标签边界 [{id, x0, x1}](点击命中用)
  let boundaries = [];

  // 叶子图标:编辑器=文件图标(无路径回退);终端=终端图标;其他=面板
  function leaf_icon(leaf, layout) {
    if (leaf.id === 0) return helix.icons.file("");
    const st = helix.get_component_state(leaf.id);
    if (st && st.mode) return helix.icons.file("term.sh"); // 终端
    return helix.icons.dir(false); // 面板
  }

  function leaf_label(leaf) {
    if (leaf.id === 0) return "editor";
    const st = helix.get_component_state(leaf.id);
    return (st && st.title) || "leaf-" + leaf.id;
  }

  // 估算文本宽度(列):ASCII 1 列,其余(CJK/图标)2 列
  function text_width(s) {
    let w = 0;
    for (const ch of s) w += ch.codePointAt(0) > 0x2ff ? 2 : 1;
    return w;
  }

  helix.set_component_render(TABBAR_ID, (ctx) => {
    const layout = helix.get_layout();
    if (!layout || !layout.leafs || layout.leafs.length === 0) {
      // 无布局数据时兜底:显示编辑器标签(active=0)
      boundaries = [{ id: 0, x0: 0, x1: 10 }];
      return [{ type: "text", text: "\u25b8  editor", style: "ui.selection" }];
    }
    const gap = 2;
    let x = 0;
    const parts = layout.leafs.map((leaf) => {
      const active = leaf.id === layout.active;
      const icon = leaf_icon(leaf, layout);
      const label = leaf_label(leaf);
      const text = (active ? "\u25b8 " : "  ") + icon + " " + label;
      const w = text_width(text);
      boundaries.push({ id: leaf.id, x0: x, x1: x + w });
      x += w + gap;
      return { type: "text", text, style: active ? "ui.selection" : "ui.statusline.inactive" };
    });
    return { type: "row", children: parts, gap };
  });

  // ── 交互:点击标签聚焦(component-event 命中)──
  helix.on("component-event", (id, ev) => {
    if (id !== TABBAR_ID || ev.kind !== "click") return false;
    const hit = boundaries.find((b) => ev.x >= b.x0 && ev.x < b.x1);
    if (!hit) return false;
    helix.focus(hit.id);
    return true;
  });
})();
