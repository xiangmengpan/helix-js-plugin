// filter_core —— picker 的**新**路径：条目只搬一次（会话句柄），每次按键只传 query。
// 与 filter.js 的差别：打分不在 JS 里，而是 helix.fuzzy.run 一次原生调用。
// 阈值：≤ 2ms / 次（一次按键后的重过滤）。
var ROWS = [];
for (var i = 0; i < 5000; i++) {
  ROWS.push(
    "src/dir" + (i % 40) + "/file_" + i + ".rs:" + (i % 500) + ":" + (i % 80) + ":fn item_" + i
  );
}
var CWD = "/home/u/proj";

// 与 picker-ui 的 view() 同构：cwd 之内显示相对路径（这层字符串处理也算进成本）
function view(p) {
  if (p.indexOf(CWD + "/") === 0) return p.slice(CWD.length + 1);
  return p;
}

// 会话（与 picker-ui 一样：条目集变化时才重建，**不计入**每次按键的成本）
var VIEWS = [];
for (var i = 0; i < ROWS.length; i++) VIEWS.push(view(ROWS[i]));
var SID = helix.fuzzy.session(VIEWS);

function bench_run(iters) {
  var acc = 0;
  for (var k = 0; k < iters; k++) {
    var hits = helix.fuzzy.run(SID, "file_1", { limit: 200 });
    var out = [];
    for (var j = 0; j < hits.length; j++) out.push(ROWS[hits[j].index]);
    acc += out.length;
  }
  return acc;
}
