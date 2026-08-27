// input-completion — input 组件 + LSP 补全联动 demo：:ic 打开补全弹窗。
// 依赖：input 引擎权威值 + onChange（字符/Backspace/Delete 编辑触发）、
//       ↑↓/Enter 走 onKey、helix.lsp.completion()（Promise → CompletionItem[] | {isIncomplete, items} | null）。
// 加载：helix.load("features/input-completion/index.js") 或 :plugin-load plugins/features/input-completion/index.js
// 使用：:ic 打开弹窗 → Tab 聚焦输入框 → 打字触发补全 → ↑↓ 选候选 → Enter 确认。
// 无 LSP server 时 completion resolve null → 弹窗内显示提示行。
// 无防抖：引擎未提供 setTimeout（boa timers 未启用），onChange 直发请求。

helix.plugin("input-completion", { deps: ["lib/icons.js"] });
let ICONS = null;   // icons.js 的 exports；load 后填充，缺失回退无图标

// 弹窗级状态（render/onChange/onKey 共享闭包读取；引擎每帧调 render 重绘）
let pid = null;
let query = "";
let items = [];
let sel = 0;

const ask = () => {
  helix.lsp.completion().then((c) => {
    items = c ? (c.items ?? c) : []; // List 变体（{isIncomplete, items}）解包；标量数组直取
    sel = 0;
  }).catch((e) => {
    helix.echo("completion error: " + e.message);
    items = [];
    sel = 0;
  });
};

const label = (it) => it.label ?? String(it);

// 光标前词干起点(方案 B 兕底:无 textEdit 时替换光标前的标识符片段)。
// 纯函数,node 可测:从 col 往前扫 [\w_$] 连续段。
const wordStart = (line, col) => {
  let s = col;
  while (s > 0 && /[\w_$]/.test(line[s - 1])) s--;
  return s;
};

// 回填选中候选到 buffer(方案 B):textEdit 优先(LSP 原生替换语义),
// 兕底按光标前词干替换;词干为空(光标前是 ./空格)→ 纯插入。
const applyEdit = (doc, it) => {
  const te = it.textEdit;
  if (te && te.range && typeof te.newText === "string") {
    const { start, end } = te.range;
    doc.replace(start.line, start.character, end.line, end.character, te.newText);
    return;
  }
  const { row, col } = doc.cursor;
  const line = (doc.text.split("\n")[row] ?? "");
  const s = wordStart(line, col);
  if (s === col) doc.insert(row, col, label(it));
  else doc.replace(row, s, row, col, label(it));
};

// 弹窗级 onKey（焦点不在节点上时生效）：↑↓ 选候选、Enter 回填 buffer、Esc 关弹窗。
// 带 doc 参数(可编辑快照)——节点级 onKey 无 doc,回填只能走这层。
// 其他键返回 "ignore" 穿透给编辑器(insert mode 打字/移动正常)。
const onKey = (key, doc) => {
  if (key.name === "Up" && sel > 0) { sel--; return "handled"; }
  if (key.name === "Down" && sel < items.length - 1) { sel++; return "handled"; }
  if (key.name === "Enter") {
    const it = items[sel];
    if (it && doc) applyEdit(doc, it);
    return "close";
  }
  if (key.name === "Esc") return "close";
  return "ignore";
};

const render = (focus) => {
  const rows = [
    {
      type: "input",
      id: "q",
      value: query, // JS 传 value 仅初始化；此后引擎维护（含光标），onChange 回传新值
      width: 40,
      onChange: (v) => { query = v; ask(); },
      onKey: (k) => { // ↑↓/Enter 走节点 onKey（编辑键不会到这里）
        if (k === "Up" && sel > 0) sel--;
        else if (k === "Down" && sel < items.length - 1) sel++;
        else if (k === "Enter") {
          const it = items[sel];
          // ponytail: 节点 onKey 无 doc 参数、也无全局 doc 插入 API，demo 用 echo 示意选中；
          // 若要做真实回填，需扩展 API（如输入框回填走 set_input_value）
          helix.echo(it ? "selected: " + label(it) : "no match");
        }
      },
    },
  ];
  if (!items.length) {
    rows.push({ type: "text", text: "  (no completion)" });
  }
  items.forEach((it, i) => {
    rows.push({
      type: "text",
      text: (i === sel ? "> " : "  ") + (ICONS ? ICONS.getCompletionKindIcon(it.kind) + " " : "") + label(it),
      style: i === sel ? "ui.info" : undefined,
    });
  });
  return helix.el("col", rows);
};

helix.register_command("ic", () => {
  if (!ICONS) ICONS = helix.load("lib/icons.js") || null;
  query = "";
  items = [];
  sel = 0;
  pid = helix.open_popup({
    width: 44,
    render,
    onKey,
    onClose: () => { pid = null; },
  });
  ask(); // 打开即基于当前光标补全,直接显示候选(insert 下按 C-x 的场景)
});

// ============================ node 自检导出 ============================

if (typeof module !== "undefined" && module.exports) {
  module.exports = { wordStart, applyEdit, label };
}
