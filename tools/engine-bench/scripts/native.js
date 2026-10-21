// native —— JS → 原生函数边界的单次成本（1000 次 = iters）。
// 对应真实负载：每帧状态栏、每次渲染里的 helix.get_style / helix.buffer.list。
// 阈值：≤ 1µs / 次。
function bench_run(iters) {
  var acc = 0;
  for (var k = 0; k < iters; k++) {
    var s = helix.get_style("ui.text");
    acc += s.length;
  }
  return acc;
}
