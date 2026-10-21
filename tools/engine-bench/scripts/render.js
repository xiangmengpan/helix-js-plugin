// render —— 对应 helix-js 的 bench_picker_frame（immediate-mode 渲染 30 行 × 3 列）
// 与产品侧脚本逐字同构：split("\t") → 逐行建对象 → 选中行查一次样式。
// 阈值：≤ 0.5ms / 帧（60fps 预算的 3%）。
var N_ROWS = 30;

function bench_run(iters) {
  var acc = 0;
  for (var k = 0; k < iters; k++) {
    var rows = [];
    for (var i = 0; i < N_ROWS; i++) {
      rows.push("src/file_" + i + ".rs\t12:3\tfn item_" + i);
    }
    var cur = 12;
    var out = [{ type: "text", text: "JS-DRAWN  q=abc" }];
    for (var j = 0; j < rows.length; j++) {
      var cells = rows[j].split("\t");
      var mark = j === cur ? "\u25b8 " : "  ";
      var line = { type: "text", text: mark + cells.join("   ") };
      if (j === cur) {
        var st = helix.get_style("ui.text.focus");
        if (st) line.style = st;
      }
      out.push(line);
    }
    acc += out.length;
  }
  return acc;
}
