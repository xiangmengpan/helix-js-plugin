// which-key/plugin.js — 快捷键提示(方案 3:set_keymap_hint 接管原生 Info)
// 依赖:helix.set_keymap_hint(ctx => 多行文本|null)
// ctx = { title: 前缀名, entries: [{keys, doc}] }
// 平级模式(C-g Locked / C-p Pane / C-n Resize / C-h Move / C-y Scroll)由 compositor
// 在进入时调 keymap_hint("C-p", ...) 等 → 本文件返回对应中文键位表。
helix.plugin("which-key", {
  deps: [],
  permissions: {},
  // v2：渐进披露三件套（截断指路 / min_entries 消噪音 / 按按键频率重排）+ 通用 `:keys`
  version: "2.0",
}); // permissions: 纯渲染 // 曾声明 deps: ["lib/icons.js"],但本文件根本不用图标
// (图标已收进核心:需要时直接用 helix.icons.*,零依赖)

// 插件配置(方案 C):config.toml [plugins.which-key]
helix.define_config("which-key", {
  position: { type: "enum", default: "bottom-right", options: ["bottom-right", "bottom-left", "top-right", "top-left", "center"], doc: "提示框位置" },
  language: { type: "enum", default: "zh-CN", options: ["zh-CN", "en"], doc: "说明文字的语言:zh-CN 用本文件的中文表 / en 直接用引擎的英文 doc(等于不翻译)" },
  show_docs: { type: "boolean", default: true, doc: "zh-CN 下未命中中文映射时是否显示英文说明(en 下不看这个开关)" },
  avoid_panels: { type: "boolean", default: true, doc: "自动躲开插件面板(dapui/filetree 等):按 pane.list() 的 area 挑一个不重叠的角" },
  mode: {
    type: "enum",
    default: "box",
    options: ["box", "line", "status"],
    doc: "box=完整浮层(可避让面板) / line=**1 行浮层只列键名**+状态栏带说明的一行摘要 / status=完全不画浮层,只 echo 状态栏(可能被别的消息盖掉)",
  },
  max_lines: { type: "number", default: 18, doc: "box 模式最多几行(0=不限);超出显示「… 还有 N 个键（按 ? 看全部）」" },
  // v2：渐进披露三件套
  min_entries: {
    type: "number",
    default: 2,
    doc: "当前层子键少于这个数就不提示（0/1 = 永远提示）；消掉 j/k 这种单键层的噪音",
  },
  sort_by_frecency: {
    type: "boolean",
    default: true,
    doc: "按**最近/最常按**重排候选（截断时先丢不常用的）；数据来自引擎的按键统计",
  },
  keys_help_command: {
    type: "string",
    default: ":keys",
    doc: "提示里「按 ? 看全部」的实际入口（默认 `:keys`；改成空串就去掉这句指路）",
  },
  // 调试命中时引擎会推一次 `Debug` 键位表（每次停下都弹，容易烦）：
  //   off    = 什么都不显示（想看按 `?`）
  //   status = 只写一行状态栏（不占屏幕格子）
  //   full   = 按上面 `mode` 的形态画（默认；和前缀菜单一致）
  on_debug_hint: {
    type: "enum",
    default: "status",
    options: ["off", "status", "full"],
    doc: "调试命中时那行键位提示的形态：off 不显示 / status 只写状态栏 / full 按 mode 画",
  },
});
// cfg 每次 render 读取(config-reload 后即时生效)
function current_cfg() {
  return helix.get_config("which-key") || {};
}

(function () {
  // ── 提示框位置:躲开插件面板 ─────────────────────────────────────────────
  // 为什么要算:pane 是有 `area`(绝对单元格几何)的,而 dapui 这类面板正好占住右侧/底部,
  // 提示框固定"右下"就会糊在面板上(实测:变量树 + 底面板 + which-key 三层叠在一起)。
  // 策略:配置的 position 只当**首选**,首选与面板重叠就按优先级换下一个角;都重叠取重叠最小的。
  const CANDIDATE_POSITIONS = ["bottom-right", "bottom-left", "top-right", "top-left"];

  /** 插件面板占的矩形(视口坐标)。没有 pane API / 没面板 ⇒ 空数组。 */
  function panel_areas() {
    try {
      const list = (helix.pane && helix.pane.list && helix.pane.list()) || [];
      return list
        .filter((p) => p && p.kind === "PluginPanel" && p.area && p.area.w > 0 && p.area.h > 0)
        .map((p) => p.area);
    } catch (e) {
      return [];
    }
  }

  /** 某个角上的提示框矩形(宽高按文本行数/最长行估算,与核心的 Info 尺寸同一口径)。 */
  function box_at(position, w, h, vw, vh) {
    const x = position.endsWith("right") ? Math.max(0, vw - w) : 0;
    const y = position.startsWith("bottom") ? Math.max(0, vh - h) : 0;
    return { x, y, w: Math.min(w, vw), h: Math.min(h, vh) };
  }

  function overlap_area(a, b) {
    const dx = Math.min(a.x + a.w, b.x + b.w) - Math.max(a.x, b.x);
    const dy = Math.min(a.y + a.h, b.y + b.h) - Math.max(a.y, b.y);
    return dx > 0 && dy > 0 ? dx * dy : 0;
  }

  /** 选位置:首选(配置)→ 其余三个角;全重叠时取重叠面积最小的那个。 */
  function pick_position(cfg, text) {
    const preferred = cfg.position || "bottom-right";
    if (cfg.avoid_panels === false) return preferred;
    const areas = panel_areas();
    if (areas.length === 0) return preferred;

    const lines = text.split("\n");
    const w = lines.reduce((m, l) => Math.max(m, l.length), 0) + 2; // 与 Info 的 width 同口径
    const h = lines.length;
    let vw = 80;
    let vh = 24;
    try {
      const vp = helix.viewport ? helix.viewport() : null;
      if (vp && vp[0] > 0 && vp[1] > 0) {
        vw = vp[0];
        vh = vp[1];
      }
    } catch (e) {
      /* 拿不到视口:用保守默认,反正只影响避让精度 */
    }

    const order = [preferred]
      .concat(CANDIDATE_POSITIONS)
      .filter((p, i, a) => a.indexOf(p) === i);
    let best = preferred;
    let best_overlap = Infinity;
    for (const pos of order) {
      const box = box_at(pos, w, h, vw, vh);
      const total = areas.reduce((sum, a) => sum + overlap_area(box, a), 0);
      if (total === 0) return pos; // 完全不碰面板:就它
      if (total < best_overlap) {
        best_overlap = total;
        best = pos;
      }
    }
    return best;
  }

  function minEntries() {
    const n = Number(current_cfg().min_entries);
    return Number.isFinite(n) && n > 0 ? Math.floor(n) : 0;
  }

  /** 「剩下的去哪看」那句指路（可关：`keys_help_command = ""`）。 */
  function more_hint(n) {
    const cmd = String(current_cfg().keys_help_command ?? ":keys").trim();
    return cmd ? `… 还有 ${n} 个键（${cmd} 看全部）` : `… 还有 ${n} 个键`;
  }

  /** v2：按引擎给的按键统计重排（没数据就原样）。 */
  function sort_entries(entries) {
    if (!current_cfg().sort_by_frecency) return entries;
    let stats = null;
    try {
      stats = helix.key_frecency ? helix.key_frecency() : null;
    } catch (e) {
      stats = null;
    }
    if (!stats || typeof stats !== "object") return entries;
    const score = (e) => Number(stats[e.keys] || stats[String(e.keys).trim()] || 0);
    // 稳定排序：分数相同保持引擎顺序（引擎顺序 = 作者定义顺序，有语义）
    return entries
      .map((e, i) => ({ e, i, s: score(e) }))
      .sort((a, b) => b.s - a.s || a.i - b.i)
      .map((x) => x.e);
  }

  /** box 模式:多于 `max_lines` 行就收成「前 n-1 行 + … 还有 N 个键（:keys 看全部）」。 */
  function cap_lines(lines, max) {
    const n = Number(max);
    if (!Number.isFinite(n) || n <= 0 || lines.length <= n) return lines;
    const keep = Math.max(1, n - 1);
    return lines.slice(0, keep).concat([more_hint(lines.length - keep)]);
  }

  /** line 模式:只列键名的**一行**（浮层用,宽度小到几乎不可能冲突）。 */
  function key_bar(lines, max) {
    const keys = lines
      .map((l) => l.split(/\s{2,}/)[0].trim())
      .filter(Boolean)
      .slice(0, max && max > 0 ? max : 24);
    return keys.join(" ");
  }

  /** 带说明的一行摘要（状态栏用）。 */
  function summarize(title, lines) {
    const head = lines.slice(0, 6).map((l) => l.replace(/\s+/g, " ").trim());
    const more =
      lines.length > head.length
        ? ` … +${lines.length - head.length}${String(current_cfg().keys_help_command ?? ":keys").trim() ? "（" + String(current_cfg().keys_help_command ?? ":keys").trim() + " 看全部）" : ""}`
        : "";
    const line = `${title} ▸ ${head.join(" · ")}${more}`;
    try {
      const vp = helix.viewport ? helix.viewport() : null;
      const w = vp && vp[0] > 8 ? vp[0] - 2 : 120;
      return helix.text && helix.text.truncate ? helix.text.truncate(line, w) : line;
    } catch (e) {
      return line;
    }
  }

  let last_line = null;
  /** status 模式：只有状态栏的一行（可能被别的消息盖掉，见 mode 的文档）。 */
  function emit_status(title, lines) {
    const line = summarize(title, lines);
    if (line !== last_line) {
      last_line = line;
      try {
        helix.echo(line);
      } catch (e) {
        /* 没有 echo（测试桩）就只吞掉 */
      }
    }
    return { hide: true }; // 核心据此一个格子都不画
  }

  /**
   * line 模式：**1 行浮层**（只有键名）+ 状态栏一行带说明的摘要。
   *
   * 为什么不再只靠状态栏：状态栏是**共享**的消息位，dap/LSP/异步回包都会 `set_status`，
   * 一行提示很容易被盖掉（实测：调试时按前缀键，摘要一闪就没了）。
   * 1 行浮层的宽度只有几十列、高度 1，配合 `pick_position` 几乎总能找到空位，
   * 而且它是 autoinfo（按键期间持久），不会被任何消息吞。
   */
  function emit_bar(title, lines) {
    const detail = summarize(title, lines);
    if (detail !== last_line) {
      last_line = detail;
      try {
        helix.echo(detail);
      } catch (e) {
        /* 同上 */
      }
    }
    return { text: key_bar(lines, 24), position: last_bar_position };
  }

  // 1 行浮层的候选位置（比完整浮层更自由：一行高几乎总能塞下）
  let last_bar_position = "bottom-right";

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
    "d": "删除选中", "dd": "删除整行",
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
    // g 前缀下没有「代码操作/切换注释/重命名符号」(那是 space 的):它们曾被重复写进 g 段,
    // 对象字面量后写者覆盖前者 ⇒ g 菜单的 a/c/r 显示成 space 的意思。已删,勿再加。
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
    // 暂停时（调试层提示表）—— 前缀 "dap "
    "dap r": "重启", "dap T": "结束会话", "dap c": "继续", "dap p": "暂停",
    "dap n": "单步跳过", "dap s": "单步进入", "dap o": "单步跳出",
    "dap t": "切换线程", "dap f": "切换栈帧", "dap S": "切换会话",
    "dap b": "切换断点", "dap v": "变量弹层",
    "dap K": "光标下求值", "dap R": "跑到光标", "dap ?": "键位速查",
    "dap C-c": "条件断点", "dap C-l": "日志断点",
    "dap e": "启用异常断点", "dap E": "禁用异常断点",
    "space y": "复制到剪贴板", "space Y": "复制主选择",
    "space p": "粘贴剪贴板(后)", "space P": "粘贴剪贴板(前)", "space R": "剪贴板替换",
    "space /": "全局搜索", "space k": "悬停", "space r": "重命名符号",
    "space h": "选择引用", "space c": "切换注释", "space C": "块注释",
    "space ?": "命令面板",
    // ── space G 前缀(DAP 调试) ──
    // 必须写全 "space G <键>":Debug 子菜单里的 l/v/c/i/t/s 都是单键,
    // 少了前缀就会退回下面的单键匹配,显示成别的前缀里的中文(如 v → 进入选择模式)。
    "space G l": "启动调试", "space G r": "重启调试会话",
    "space G b": "切换断点", "space G c": "继续执行", "space G h": "暂停执行",
    "space G i": "单步进入", "space G o": "单步跳出", "space G n": "单步跳过",
    "space G v": "变量视图", "space G t": "结束调试会话",
    "space G C-c": "编辑断点条件", "space G C-l": "编辑断点日志",
    "space G s": "切换线程/栈帧",
    "space G s t": "切换线程", "space G s f": "切换栈帧",
    "space G S": "切换调试会话",
    "space G e": "启用异常断点", "space G E": "禁用异常断点",
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
    // 标题取自引擎的 keymap 名(default.rs 的 `{ "..." ... }`),**不是**我们起的名字。
    // DAP 子菜单的真实标题是 "Debug (experimental)";写成 "Debug" 就匹配不上 →
    // 前缀丢失 → 退回单键匹配 → 整张表的中文全是别人的(这就是原来的错描述)。
    "Debug (experimental)": "space G",
    // 暂停时引擎推来的"调试层"提示表，标题就是 "Debug"（input_router::hints）
    "Debug": "dap",
    "Switch": "space G s", // space G 下的 s 子菜单
  };

  function render(ctx) {
    const cfg = current_cfg();
    // 调试命中提示：单独一条策略（每次停下都会推一次，默认只写状态栏免得挡屏幕）
    if (ctx.title === "Debug") {
      const policy = cfg.on_debug_hint || "status";
      if (policy === "off") return { hide: true };
      if (policy === "status") {
        if (ctx.entries && ctx.entries.length) {
          const list = sort_entries(ctx.entries);
          const lines = list.map((e) => {
            const zh = CN["dap " + e.keys] || CN[e.keys];
            return e.keys + (zh ? "  " + zh : "");
          });
          return emit_status("Debug", lines);
        }
        return { hide: true };
      }
      // full ⇒ 继续走下面的常规路径（按 mode 渲染）
    }
    // 输入层徽标（栈序，来自引擎的 input_router::badges）：`[DEBUG│PANE] …`
    const badges = helix.input_badges ? helix.input_badges() : "";
    const tag = (title) => (badges ? `[${badges}] ${title}` : title);
    // 平级模式:键位表的**唯一来源是引擎** —— helix.pane_mode.keymap() 由 compositor
    // 的 pane_mode_entries 推出。本文件**不再维护第二份**键位表(早先那份 PANE_HINTS
    // 硬编码表已删):两处维护必然漂移。引擎不在模式里时自然落到下面的普通 entries 路径。
    {
      // 优先用引擎推来的键位表(单一来源:compositor 的 pane_mode_entries),
      // 引擎没给(旧版/未进模式)才退回本文件的内置表。
      const pm = helix.pane_mode && helix.pane_mode.keymap ? helix.pane_mode.keymap() : null;
      if (pm && pm.keys && pm.keys.length) {
        if (pm.keys.length < minEntries()) return { hide: true };
        const lines = pm.keys.map(
          (e) => e.key.padEnd(12) + ` ${e.desc}${e.enabled ? "" : `(${e.reason || "未实现"})`}`
        );
        if (cfg.mode === "status") return emit_status(tag(pm.mode || ctx.title || "mode"), lines);
        if (cfg.mode === "line") {
          last_bar_position = pick_position(cfg, "*");
          return emit_bar(tag(pm.mode || ctx.title || "mode"), lines);
        }
        const text = cap_lines(lines, cfg.max_lines).join("\n");
        return { text, position: pick_position(cfg, text) };
      }
    }
    if (!ctx.entries || ctx.entries.length === 0) return null;
    // v2「噪音控制」：当前层子键太少就不弹（j/k/dd 那层弹出来纯属打扰）
    if (ctx.entries.length < minEntries()) return { hide: true };
    // v2「频次优先」：按引擎的按键统计重排，截断时先丢不常用的
    const ordered = sort_entries(ctx.entries);
    // 语言开关(config.toml → [plugins.which-key] language):
    //   "zh-CN"(默认)= 走下面的中文表,未命中再退回引擎 doc(show_docs 控制)
    //   "en"       = 完全不碰中文表,直接显示引擎 doc —— 也就是原生 Info 的样子
    // 注意:平级模式(C-p 等)那张表来自引擎(compositor),不受此开关影响。
    const zh_cn = cfg.language !== "en";
    const prefix = PREFIX_KEY[ctx.title] ? PREFIX_KEY[ctx.title] + " " : "";
    const lines = ordered.map((e) => {
      // 组合键取首键(如 "g, G" → "g");前缀键 + 节点内键 拼完整序列匹配
      const first = e.keys.split(", ")[0];
      const zh = zh_cn
        ? CN[prefix + e.keys] || CN[prefix + first] || CN[e.keys] || CN[first]
        : null;
      const desc = zh || (!zh_cn || cfg.show_docs !== false ? e.doc : "");
      return e.keys + (desc ? "  " + desc : "");
    });
    if (cfg.mode === "status") return emit_status(ctx.title || "keys", lines);
    if (cfg.mode === "line") {
      last_bar_position = pick_position(cfg, "*");
      return emit_bar(tag(ctx.title || "keys"), lines);
    }
    last_line = null; // 回到 box 形态时清掉状态栏去重记忆
    const text = cap_lines(lines, cfg.max_lines).join("\n");
    return { text, position: pick_position(cfg, text) };
  }

  // 仅供 node --test 直测（渲染路径不受影响）：位置选择是纯函数
  helix.export({ pick_position, panel_areas, box_at, render });

  helix.set_keymap_hint(render);
})();
