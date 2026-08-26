// input-completion — input 组件 + LSP 补全联动 demo：:ic 打开补全弹窗。
// 依赖：input 引擎权威值 + onChange（字符/Backspace/Delete 编辑触发）、
//       ↑↓/Enter 走 onKey、helix.lsp.completion()（Promise → CompletionItem[] | {isIncomplete, items} | null）。
// 加载：helix.load("features/input-completion/index.js") 或 :plugin-load plugins/features/input-completion/index.js
// 使用：:ic 打开弹窗 → Tab 聚焦输入框 → 打字触发补全 → ↑↓ 选候选 → Enter 确认。
// 无 LSP server 时 completion resolve null → 弹窗内显示提示行。
// 无防抖：引擎未提供 setTimeout（boa timers 未启用），onChange 直发请求。

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
      text: (i === sel ? "> " : "  ") + label(it),
      style: i === sel ? "ui.info" : undefined,
    });
  });
  return helix.el("col", rows);
};

helix.register_command("ic", () => {
  query = "";
  items = [];
  sel = 0;
  pid = helix.open_popup({
    width: 44,
    render,
    onClose: () => { pid = null; },
  });
});
