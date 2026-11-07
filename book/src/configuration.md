# Configuration

To override global configuration parameters, create a `config.toml` file located in your config directory:

- Linux and Mac: `~/.config/helix/config.toml`
- Windows: `%AppData%\helix\config.toml`

> 💡 You can easily open the config file by typing `:config-open` within Helix normal mode.

Example config:

```toml
theme = "onedark"

[editor]
line-number = "relative"
mouse = false
# `layout-change` 事件的投递方式:false(默认,逐点发)/ true(同窗口合并成一条)
js-layout-change-batch = false
# 焦点移动的作用域（Pane 模式 h/j/k/l 与 helix.pane.focus_dir）
#   geometry(默认) = 纯几何：按屏幕位置找最近的方向邻居，可以跨组
#   group          = 组是导航的最小单位：只在最近的组内部移动，到组边界就停住
focus-scope = "geometry"
# 折叠候选来源：
#   indent(默认)  = 缩进启发式，零语言配置
#   manual        = 只认手动折叠（z f / z a）
#   tree-sitter   = 该语言的 folds.scm（没写这个文件 ⇒ 自动退 indent）
#   lsp           = LSP textDocument/foldingRange（P2，未实现 ⇒ 先按 indent 兜底）
fold-method = "indent"
# 光标/搜索闯进折叠区时：
#   open(默认) = 自动展开（vim foldopen 手感，任何时候都看得见光标）
#   skip       = 跳过：光标被送到折叠块之后第一行，折叠保持关着
fold-jump = "open"
# 自动折叠至少要藏几行（避免把 1 行折起来）
fold-min-lines = 2
# 折叠态跨会话持久化（~/.local/state/helix/fold-state.json，按文件路径存行号区间）
fold-persist = true
# gutter 折叠标记列：
#   auto(默认) = 文档已有折叠时才占一列；always = 一直占（一打开就看得到可折处）；hide = 不画
# 标记：▶ = 已折（正文不上屏）、▼ = 可折但没折
fold-column = "auto"

[editor.cursor-shape]
insert = "bar"
normal = "block"
select = "underline"

[editor.file-picker]
hidden = false

[logging]
# 日志文件（`:log-open` 那个）的大小上限，单位字节；写入会超过它就**清空重写**（不轮转）。
# 默认 10 MiB；`0` = 不限制。只认用户级 config：workspace 的 `.helix/config.toml` 里写这个**不生效**。
max-size = 10485760
```

You can use a custom configuration file by specifying it with the `-c` or
`--config` command line argument, for example `hx -c path/to/custom-config.toml`.
You can reload the config file by issuing the `:config-reload` command. Alternatively, on Unix operating systems, you can reload it by sending the USR1
signal to the Helix process, such as by using the command `pkill -USR1 hx`.

Finally, you can have a `config.toml` and a `languages.toml` local to a project by putting it under a `.helix` directory in your repository.
Its settings will be merged with the configuration directory and the built-in configuration.

## Plugin configuration

Plugins declare their own keys (`helix.define_config`) and they live under
`[plugins.<plugin-name>]`. Plugin keys keep the **exact** names the plugin declared (usually
`snake_case` — unlike `[editor]`, which is `kebab-case`), and a misspelled key is reported
(`filetree.show_hiden: not declared in define_config (ignored)`) instead of being silently
dropped.

Plugins that snapshot their config when they load only pick up new values after
`:plugin-reload`; `:config-reload` alone re-validates and is enough for plugins that read on
use. `filetree` is a snapshot-at-load plugin.

### `[plugins.filetree]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `show_hidden` | boolean | `false` | Show dotfiles in the tree. Also toggled at runtime with `H`. |
| `filter` | string | `""` | Filter expression applied when the panel opens (same syntax as the `>` bar: globs, `#M`/`#?` git predicates, `#E`/`#W` diagnostics, `!` negation). Empty = no filter. |
| `git_status` | boolean | `false` | Show the git status column (`M`/`A`/`D`/`R`/`?`) and enable the `#` git predicates. Reads `git status` once when the panel opens and on `R`. |
| `max_panels` | number | `2` | How many tree panels may be open at once (`:filetree-open` adds one; further attempts are refused with a message). `1` = single panel. |
| `close_on_esc` | boolean | `true` | `true`: `Esc` closes the panel. `false`: `Esc` only returns focus to the editor and keeps the panel — use `q` to close. Handy when the tree is a permanent left column. |
| `persist` | boolean | `false` | Remember the filter expression and the expanded directories **per root** and restore them next time. Written to `filetree-state.json` next to the user plugin root (`<config-dir>/plugins/`); `false` = nothing is written. |

```toml
[plugins.filetree]
show_hidden = true
git_status = true
filter = "!*.lock"
close_on_esc = false
max_panels = 2
persist = true
```

Commands: `:filetree` (toggle), `:filetree-open [dir]` (second panel; root = the given dir or the
current file's directory), `:filetree-reveal`, `:filetree-find` (native picker over `rg --files`),
`:filetree-search <query>` (content search).

Keys: `Enter`/`o` open · `h`/`l` collapse/enter · `<Up>`/`<Down>` move · `/` search by name
(`Tab` switches to the whole repo) · `G` content search (`←`/`→` collapse a file group) · `>`
filter expression · `H` dotfiles · `F` follow the current file · `R` refresh · `a`/`A` new
file/directory · `m` rename · `d` delete · `P` go to parent · `q` close · `?` help.
