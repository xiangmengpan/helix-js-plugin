// which-key/plugin.js — 快捷键提示(方案 3:set_keymap_hint 接管原生 Info)
// 依赖:helix.set_keymap_hint(ctx => 多行文本|null)
// ctx = { title: 前缀名, entries: [{keys, doc}] }
// 平级模式(C-g Locked / C-p Pane / C-n Resize / C-h Move / C-y Scroll)由 compositor
// 在进入时调 keymap_hint("C-p", ...) 等 → 本文件返回对应中文键位表。
helix.plugin("which-key", { deps: [] }); // 曾声明 deps: ["lib/icons.js"],但本文件根本不用图标
// (图标已收进核心:需要时直接用 helix.icons.*,零依赖)

// 插件配置(方案 C):config.toml [plugins.which-key]
helix.define_config("which-key", {
  position: { type: "enum", default: "bottom-right", options: ["bottom-right", "bottom-left", "top-right", "top-left", "center"], doc: "提示框位置" },
  show_docs: { type: "boolean", default: true, doc: "未命中中文映射时显示英文说明" },
});
// cfg 每次 render 读取(config-reload 后即时生效)
function current_cfg() {
  return helix.get_config("which-key") || {};
}

(function () {
  // 中文说明:键组合 → 中文(按 keys 字符串匹配;未命中显示原 doc)
  const CN = {
    // ── 移动 ──
    "h": "左移一字符", "l": "右移一字符", "j": "下移一行", "k": "上移一行",
    "w": "前进一词", "b": "后退一词", "e": "词尾",
    "W": "前进一词(空白分隔)", "B": "后退一词(空白)", "E": "词尾(空白)",
    "t": "查找字符(光标前)", "f": "查找字符(光标后)",
    "T": "向前查找字符", "F": "向后查找字符",
    "0": "行首(第 0 列)", "$": "行尾", "home": "行首", "end": "行尾",
    "gg": "文档首行", "G": "跳转到行", "H": "屏幕顶行", "M": "屏幕中行", "L": "屏幕底行",
    "{": "上一段落", "}": "下一段落",
    "C-b": "上翻页", "C-f": "下翻页", "C-u": "上半页", "C-d": "下半页",
    "A-.": "重复上次移动",
    // ── 编辑 ──
    "i": "插入(光标前)", "a": "插入(光标后)", "I": "行首插入", "A": "行尾插入",
    "o": "下方新行", "O": "上方新行",
    "x": "删除字符", "d": "删除选中", "dd": "删除整行",
    "A-d": "删除(不存入寄存器)", "c": "更改(删除并插入)", "A-c": "更改(不存寄存器)",
    "u": "撤销", "U": "重做", "A-u": "更早", "A-U": "更晚",
    "y": "复制选中", "yy": "复制整行", "p": "粘贴(后)", "P": "粘贴(前)",
    "r": "替换字符", "R": "用寄存器替换", "~": "切换大小写", "`": "转小写",
    ".": "重复上次操作",
    "C-c": "切换注释",
    "C-i": "前进跳转", "tab": "前进跳转", "C-o": "后退跳转",
    "C-s": "保存选择",
    // ── 选择 ──
    "v": "进入选择模式", "V": "行选择", "C-v": "块选择",
    "s": "正则选择", "S": "分割选择", "A-s": "按换行分割选择",
    ";": "折叠选择", "A-;": "翻转选择",
    "%": "全选", "x": "向下扩展行", "X": "扩展到行边界", "A-x": "收缩到行边界",
    "C": "下一行复制选择", "A-C": "上一行复制选择",
    "A-o": "扩展选择", "A-i": "收缩选择", "A-p": "上一同级", "A-n": "下一同级",
    "A-a": "选择全部同级",
    "J": "合并选择", "A-J": "空格合并", "K": "保留选择", "A-K": "移除选择",
    ",": "保留主选择", "A-,": "移除主选择",
    "&": "对齐选择", "_": "修剪选择",
    "(": "旋转选择(后)", ")": "旋转选择(前)",
    "A-(": "旋转内容(后)", "A-)": "旋转内容(前)", "A-:": "确保选择向前",
    // ── 搜索 ──
    "/": "搜索", "?": "反向搜索", "n": "下一匹配", "N": "上一匹配",
    "*": "搜索选中词", "A-*": "搜索选中(无词界)",
    // ── 宏 ──
    "Q": "录制宏", "q": "重放宏",
    // ── 缩进/格式 ──
    ">": "缩进", "<": "反缩进", "=": "格式化",
    // ── g 前缀 ──
    "g g": "文档首行", "g |": "跳转列", "g e": "跳转末行", "g f": "打开文件",
    "g h": "行首", "g l": "行尾", "g s": "行首非空白",
    "g d": "跳转定义", "g D": "跳转声明", "g y": "跳转类型定义",
    "g r": "跳转引用", "g i": "跳转实现",
    "g t": "窗口顶行", "g c": "窗口中间", "g b": "窗口底行",
    "g a": "上次访问文件", "g m": "上次修改文件",
    "g n": "下一缓冲", "g p": "上一缓冲",
    "g k": "上移一行", "g j": "下移一行",
    "g .": "上次修改", "g w": "跳转词", "g o": "符号大纲",
    "g x": "打开链接", "g a": "代码操作", "g c": "切换注释", "g r": "重命名符号",
    // ── m 前缀(匹配/包围) ──
    "m m": "匹配括号", "m s": "添加包围", "m r": "替换包围", "m d": "删除包围",
    "m a": "选择环绕(外)", "m i": "选择环绕(内)",
    // ── [ 前缀 ──
    "[ d": "上一诊断", "[ D": "首个诊断", "[ g": "上一修改", "[ G": "首次修改",
    "[ f": "上一函数", "[ t": "上一类", "[ a": "上一参数", "[ c": "上一注释",
    "[ e": "上一条目", "[ T": "上一测试", "[ p": "上一段落", "[ x": "上一 XML 元素",
    "[ space": "上方新行",
    // ── ] 前缀 ──
    "] d": "下一诊断", "] D": "末诊断", "] g": "下一修改", "] G": "末修改",
    "] f": "下一函数", "] t": "下一类", "] a": "下一参数", "] c": "下一注释",
    "] e": "下一条目", "] T": "下一测试", "] p": "下一段落", "] x": "下一 XML 元素",
    "] space": "下方新行",
    // ── space 前缀 ──
    "space f": "文件选择器", "space F": "当前目录文件", "space e": "文件浏览器",
    "space .": "当前目录浏览器", "space b": "缓冲选择器", "space j": "跳转列表",
    "space s": "符号大纲", "space S": "工作区符号",
    "space d": "诊断列表", "space D": "工作区诊断", "space g": "修改文件",
    "space a": "代码操作", "space '": "上次选择器",
    "space G": "调试(DAP)", "space w": "窗口操作",
    "space y": "复制到剪贴板", "space Y": "复制主选择",
    "space p": "粘贴剪贴板(后)", "space P": "粘贴剪贴板(前)", "space R": "剪贴板替换",
    "space /": "全局搜索", "space k": "悬停", "space r": "重命名符号",
    "space h": "选择引用", "space c": "切换注释", "space C": "块注释",
    "space ?": "命令面板",
    // ── z/Z 前缀(视图) ──
    "z z": "居中视图", "z c": "居中视图", "z t": "顶部对齐", "z b": "底部对齐",
    "z m": "中间对齐", "z k": "上滚", "z j": "下滚",
    "z C-b": "上翻页", "z C-f": "下翻页", "z C-u": "上半页", "z C-d": "下半页",
    "z /": "搜索", "z ?": "反向搜索", "z n": "下一匹配", "z N": "上一匹配",
    // ── 其他 ──
    "esc": "返回普通模式", "space": "空间模式(命令组)",
    "C-p": "窗口模式(Pane)", "C-n": "调整尺寸", "C-h": "移动窗口",
    "C-y": "滚动回看", "C-g": "锁定(全部键交给窗口)",
    "C-space": "空间模式",
  };

  // 平级模式中文键位表(阶段①)。compositor 进入某模式时以 title=该模式前缀键调用。
  // 条目与 Rust 侧 pane_mode_hint / pane_mode_key 的键位表一一对应。
  // 前缀名(title) → 前缀键(拼完整键序列用)
  const PREFIX_KEY = {
    "Goto": "g", "Match": "m",
    "Left bracket": "[", "Right bracket": "]",
    "Space": "space", "View": "z",
    "Debug": "space G",
  };

  function render(ctx) {
    const cfg = current_cfg();
    // 平级模式:键位表的**唯一来源是引擎** —— helix.pane_mode.keymap() 由 compositor
    // 的 pane_mode_entries 推出。本文件**不再维护第二份**键位表(早先那份 PANE_HINTS
    // 硬编码表已删):两处维护必然漂移。引擎不在模式里时自然落到下面的普通 entries 路径。
    {
      // 优先用引擎推来的键位表(单一来源:compositor 的 pane_mode_entries),
      // 引擎没给(旧版/未进模式)才退回本文件的内置表。
      const pm = helix.pane_mode && helix.pane_mode.keymap ? helix.pane_mode.keymap() : null;
      if (pm && pm.keys && pm.keys.length) {
        const lines = pm.keys.map(
          (e) => e.key.padEnd(12) + ` ${e.desc}${e.enabled ? "" : `(${e.reason || "未实现"})`}`
        );
        return { text: lines.join("\n"), position: cfg.position };
      }
    }
    if (!ctx.entries || ctx.entries.length === 0) return null;
    const prefix = PREFIX_KEY[ctx.title] ? PREFIX_KEY[ctx.title] + " " : "";
    const lines = ctx.entries.map((e) => {
      // 组合键取首键(如 "g, G" → "g");前缀键 + 节点内键 拼完整序列匹配
      const first = e.keys.split(", ")[0];
      const zh = CN[prefix + e.keys] || CN[prefix + first] || CN[e.keys] || CN[first];
      return e.keys + (zh ? "  " + zh : (cfg.show_docs ? "  " + e.doc : ""));
    });
    return { text: lines.join("\n"), position: cfg.position };
  }

  helix.set_keymap_hint(render);
})();
