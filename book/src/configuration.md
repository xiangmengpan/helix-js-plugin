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

