// features/which-key.js — 快捷键提示(方案 3:set_keymap_hint 接管原生 Info)
// 依赖:helix.set_keymap_hint(ctx => 多行文本|null)
// ctx = { title: 前缀名, entries: [{keys, doc}] }
// 窗口模式(C-w)由 compositor 在进入时调 keymap_hint("C-w", ...) → 本文件返回中文键位表。
helix.plugin("which-key", { deps: ["lib/layout.js"] });

(function () {
  // 中文说明:键组合 → 中文(按 keys 字符串匹配;未命中显示原 doc)
  const CN = {
    // 移动
    "h": "左移一字符",
    "l": "右移一字符",
    "j": "下移一行",
    "k": "上移一行",
    "w": "前进一词",
    "b": "后退一词",
    "e": "词尾",
    "0": "行首(第 0 列)",
    "$": "行尾",
    "gg": "文档首行",
    "G": "文档末行",
    "H": "屏幕顶行",
    "M": "屏幕中行",
    "L": "屏幕底行",
    "{": "上一段落",
    "}": "下一段落",
    // 编辑
    "i": "插入(光标前)",
    "a": "插入(光标后)",
    "I": "行首插入",
    "A": "行尾插入",
    "o": "下方新行",
    "O": "上方新行",
    "x": "删除字符",
    "d": "删除选中",
    "dd": "删除整行",
    "u": "撤销",
    "U": "重做",
    "y": "复制选中",
    "yy": "复制整行",
    "p": "粘贴(后)",
    "P": "粘贴(前)",
    "c": "更改(删除并插入)",
    "r": "替换字符",
    ".": "重复上次操作",
    // 选择
    "v": "字符选择",
    "V": "行选择",
    "C-v": "块选择",
    // 搜索
    "/": "搜索",
    "n": "下一匹配",
    "N": "上一匹配",
    "*": "搜索选中词",
    // g 前缀组
    "g d": "跳转定义",
    "g o": "符号大纲",
    "g g": "文档首行",
    "g x": "打开链接",
    "g a": "代码操作",
    "g c": "切换注释",
    "g r": "重命名符号",
    "g p": "粘贴上一内容",
    // 窗口/面板
    "C-w": "窗口模式",
    "C-space": "空间模式",
    // 其他
    "esc": "返回普通模式",
    "tab": "缩进",
  };

  // 窗口模式中文键位表(compositor 进入 C-w 时调用)
  const C_W_HINT = [
    "h j k l   聚焦(左/下/上/右)",
    "H J K L   交换窗口",
    "C-h C-j C-k C-l  尺寸 ∓5%",
    "x         关闭窗口",
    "z         最小化/还原",
    "f         最大化/还原",
    "Esc / C-w 退出窗口模式",
  ].join("\n");

  function render(ctx) {
    // 窗口模式:compositor 传入 title="C-w"
    if (ctx.title === "C-w") return C_W_HINT;
    // 其他前缀:键位 + 中文说明(未命中映射用原 doc)
    if (!ctx.entries || ctx.entries.length === 0) return null;
    const lines = ctx.entries.map((e) => {
      const zh = CN[e.keys] || CN[e.keys.split(", ")[0]];
      return e.keys + (zh ? "  " + zh : "  " + e.doc);
    });
    return lines.join("\n");
  }

  helix.set_keymap_hint(render);
})();
