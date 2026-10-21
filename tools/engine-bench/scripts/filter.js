// filter —— 对应 picker 的 items()：5000 行候选 → 子串过滤 → 排序 → 映射成行对象。
// 阈值：≤ 2ms / 次（一次打开 picker 的决策成本）。
var N = 5000;
var ROWS = [];
for (var i = 0; i < N; i++) {
  ROWS.push(
    "src/dir" + (i % 40) + "/file_" + i + ".rs:" + (i % 500) + ":" + (i % 80) + ":fn item_" + i
  );
}

function bench_run(iters) {
  var acc = 0;
  for (var k = 0; k < iters; k++) {
    var q = "file_1";
    var hits = [];
    for (var i = 0; i < ROWS.length; i++) {
      if (ROWS[i].indexOf(q) >= 0) hits.push(ROWS[i]);
    }
    hits.sort();
    var out = [];
    for (var j = 0; j < hits.length; j++) {
      var parts = hits[j].split(":");
      out.push({ label: parts[0], detail: parts[1] + ":" + parts[2], text: parts[3] || "" });
    }
    acc += out.length;
  }
  return acc;
}
