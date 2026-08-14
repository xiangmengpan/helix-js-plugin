// one-dark.js — One Dark 主题插件
//
// 安装：复制到 ~/.config/helix/plugins/one-dark.js，
//       并在 init.js 里加一行  helix.load("one-dark.js")
//
// 原理：helix.set_theme({ scope: color }) 把每个 scope 的 fg 覆盖为 One Dark 配色，
//       合并到当前基础主题上。可随时重新应用：:theme-one-dark
//
// 限制：JS 主题 API 只覆盖 fg（背景/修饰符不可设，见 theme_overrides_value）。
//       背景色来自基础主题——建议搭配深色基础主题使用：
//       config.toml 里 theme = "onedarker" 或 "default"。
//       改完背景后执行 :theme-one-dark 重新应用即可。

const palette = {
  bg:        "#282c34", // 背景（仅作参考，API 设不了 bg）
  fg:        "#abb2bf", // 前景
  comment:   "#5c6370", // 注释
  red:       "#e06c75", // 变量/标签/错误
  orange:    "#d19a66", // 数字/参数/属性
  yellow:    "#e5c07b", // 类型/类/常量
  green:     "#98c379", // 字符串
  cyan:      "#56b6c2", // 运算符
  blue:      "#61afef", // 函数/关键字
  purple:    "#c678dd", // 关键字/标记
  gutter:    "#4b5263", // 行号
};

function apply() {
  helix.set_theme({
    // ── 语法 ─────────────────────────────────────────────
    "comment": palette.comment,
    "comment.line": palette.comment,
    "comment.block": palette.comment,
    "comment.documentation": palette.comment,

    "constant": palette.yellow,
    "constant.numeric": palette.orange,
    "constant.numeric.integer": palette.orange,
    "constant.numeric.float": palette.orange,
    "constant.builtin": palette.orange,
    "constant.builtin.boolean": palette.orange,
    "constant.character": palette.orange,
    "constant.character.escape": palette.cyan,
    "constant.string": palette.green,
    "constant.string.escape": palette.cyan,
    "constant.other": palette.yellow,
    "constant.other.symbol": palette.yellow,
    "constant.other.color": palette.cyan,

    "constructor": palette.yellow,

    "function": palette.blue,
    "function.builtin": palette.blue,
    "function.method": palette.blue,
    "function.macro": palette.blue,
    "function.special": palette.blue,
    "function.special.title": palette.yellow,
    "function.variable": palette.blue,
    "function.variable.special": palette.blue,

    "keyword": palette.purple,
    "keyword.control": palette.purple,
    "keyword.control.conditional": palette.purple,
    "keyword.control.repeat": palette.purple,
    "keyword.control.exception": palette.red,
    "keyword.control.import": palette.purple,
    "keyword.directive": palette.blue,
    "keyword.function": palette.purple,
    "keyword.operator": palette.purple,
    "keyword.return": palette.purple,
    "keyword.special": palette.purple,

    "label": palette.red,
    "module": palette.blue,
    "namespace": palette.yellow,

    "operator": palette.cyan,
    "operator.arithmetic": palette.cyan,
    "operator.assignment": palette.cyan,
    "operator.comparison": palette.cyan,
    "operator.logical": palette.cyan,

    "parameter": palette.orange,
    "property": palette.red,
    "punctuation": palette.fg,
    "punctuation.bracket": palette.fg,
    "punctuation.delimiter": palette.fg,
    "punctuation.special": palette.cyan,

    "special": palette.yellow,
    "string": palette.green,
    "string.regexp": palette.green,
    "string.special": palette.green,
    "string.special.path": palette.green,
    "string.special.symbol": palette.green,
    "string.special.url": palette.cyan,

    "tag": palette.red,
    "tag.attribute": palette.orange,
    "tag.error": palette.red,
    "tag.special": palette.red,

    "type": palette.yellow,
    "type.builtin": palette.yellow,
    "type.enum": palette.yellow,
    "type.enum.variant": palette.orange,
    "type.parameter": palette.orange,

    "variable": palette.red,
    "variable.builtin": palette.red,
    "variable.other": palette.red,
    "variable.parameter": palette.orange,
    "variable.property": palette.red,
    "variable.special": palette.red,

    "embedded": palette.cyan,
    "error": palette.red,

    // ── markup ────────────────────────────────────────────
    "markup.heading": palette.blue,
    "markup.list": palette.red,
    "markup.quote": palette.yellow,
    "markup.raw": palette.green,
    "markup.raw.block": palette.green,
    "markup.raw.inline": palette.green,
    "markup.link.url": palette.cyan,
    "markup.link.text": palette.purple,
    "markup.bold": palette.orange,
    "markup.italic": palette.purple,
    "markup.strikethrough": palette.comment,
    "markup.underline": palette.cyan,

    // ── UI（只设 fg；背景随基础主题）──────────────────────
    "ui.text": palette.fg,
    "ui.text.focus": palette.fg,
    "ui.text.inactive": palette.comment,
    "ui.virtual": palette.comment,
    "ui.virtual.whitespace": palette.comment,
    "ui.virtual.indent-guide": palette.comment,
    "ui.virtual.ruler": palette.comment,
    "ui.virtual.jump-label": palette.purple,
    "ui.virtual.jump-label.selected": palette.fg,

    "ui.linenr": palette.gutter,
    "ui.linenr.selected": palette.fg,
    "ui.cursor.match": palette.cyan,
    "ui.cursor.primary": palette.bg, // 反色块上显示深色

    "ui.error": palette.red,
    "ui.warning": palette.yellow,
    "ui.info": palette.blue,
    "ui.hint": palette.green,
    "ui.diagnostics.error": palette.red,
    "ui.diagnostics.warning": palette.yellow,
    "ui.diagnostics.info": palette.blue,
    "ui.diagnostics.hint": palette.green,
    "ui.diagnostics.unnecessary": palette.comment,
    "ui.debug": palette.orange,

    "ui.statusline": palette.fg,
    "ui.statusline.inactive": palette.comment,
    "ui.help": palette.fg,
    "ui.popup": palette.fg,
    "ui.window": palette.comment,
    "ui.menu": palette.fg,
    "ui.menu.selected": palette.bg,
    "ui.menu.scroll": palette.gutter,
    "ui.menu.scroll.focus": palette.fg,
    "ui.gutter": palette.comment,
    "ui.gutter.selected": palette.fg,
  });
  helix.echo("one-dark theme applied");
}

apply();

helix.register_command("theme-one-dark", () => {
  apply();
}, "重新应用 One Dark 主题配色");
