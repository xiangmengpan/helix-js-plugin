// json —— JSON.parse 一份 ~53KB 的布局快照（helix.get_layout() 的读取成本）。
// 快照在加载期用 JS 自己拼（三引擎等价，不靠宿主塞字符串）。
// 阈值：≤ 1ms / 次。
var BIG = (function () {
  var panes = [];
  for (var i = 0; i < 500; i++) {
    panes.push({
      id: i,
      kind: "editor",
      place: "tiled",
      focused: i === 0,
      stack: [],
      area: { x: i % 120, y: i % 40, w: 80, h: 30 },
    });
  }
  return JSON.stringify({ panes: panes });
})();

function bench_run(iters) {
  var acc = 0;
  for (var k = 0; k < iters; k++) {
    var o = JSON.parse(BIG);
    acc += o.panes.length;
  }
  return acc;
}
