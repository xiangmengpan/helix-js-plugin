# Server Manager(`:server`)——LSP/DAP/linter/formatter 统一管理

> 实现计划:docs/superpowers/plans/2026-09-05-server-manager.md
> 设计规格:docs/superpowers/specs/2026-09-05-server-manager-design.md

mason 式工具管理器:统一注册表 + 下载安装/升级/卸载(mirror 支持 + sha256 校验)+
自动把受管 LSP/DAP 接进 `languages.toml`(标记段),开箱即用。

## 命令族

```
:server list                    # 全部配方 + 安装状态
:server search <kw>             # 按名称/语言过滤
:server panel                   # 原生 picker 面板(输入即过滤;Enter=install/update)
:server-manager                 # JS rail 面板(:server-manager 插件;j/k Enter x r q)
:server install <name> [version]
:server update [name]           # 无 name = 更新全部已装
:server remove <name>
:server status                  # 目录/已装统计/版本/languages.toml 路径
```

JS API:`helix.server.list/search/install/update/remove/status`(经
`UiRequest::ServerOp`,单向,结果显示在状态栏)+ `helix.server.rows(cb)`——
term→JS 行数据回传(面板渲染用;行对象 {name,kind,languages,installed,version,installable},
一次性回调)。

## 目录布局

- 受管根目录 `~/.local/share/helix/managed/`
  - `<name>/` 安装产物;`bin/<name>` 可执行软链(把 `<managed>/bin` 加入 PATH
    后终端里也能直接用)
  - 测试可用环境变量 `SM_MANAGED_DIR` 覆写根目录、`SM_LANGS_TOML` 覆写
    languages.toml 路径(勿在生产使用)
- languages.toml:受管内容写在
  `# >>> helix-managed` … `# <<< helix-managed` 标记段之间,整段由管理器重建;
  段外用户手写内容逐字节保留。**语言已被你在段外自定义时,管理器不生成该语言
  条目**(避免覆盖),只提示手动挂接 `language-servers`/`debugger`。

## 配置(config.toml)

```toml
[server-manager]
mirror = ""   # https 下载前缀改写(如 https://ghproxy.com/);file:// 与本地路径原样

# url_template 支持占位:{version}(由配方 version/install 参数提供)、{os}/{arch}、
# {triple}(rust 常用 target,见 rust_triple());单文件 .gz release 直接解码安装

# 用户扩展配方(v1:archive 类;url 含 {version} 时必填 version)
[server-manager.registry.rust-analyzer]
url = "https://github.com/rust-lang/rust-analyzer/releases/download/{version}/rust-analyzer-x86_64-unknown-linux-gnu.gz"
sha256 = ""       # 空 = 跳过校验(下载仍可用;建议填以保障完整性)
strip = 1         # 解压剥离的顶层目录数
bin = "rust-analyzer"        # 解压后相对可执行路径(相对解压根)
bin-name = "rust-analyzer"   # managed/bin 下的链接名(默认 = 配方名)
version = "2024-09-16"       # install/update 使用的版本
kind = "lsp"      # lsp | dap | linter | formatter(默认 lsp;dap 生成 [[language]] debugger)
languages = ["rust"]   # 挂接的语言(生成 [[language]] 条目引用)
```

安装/卸载自动重写 `languages.toml` 标记段;**重启编辑器后新 LSP/DAP 生效**
(运行时语言配置在启动时加载)。已装再 `install` 会报错提示改走 `update`;
`remove` 正在使用的 server 只删配置,不杀进程,重启后不再启动。

内置注册表(v1:rust-analyzer/gopls/pyright/clangd/debugpy/black/prettier):
**rust-analyzer** 已带真实源模板(`{triple}` 自动展开,version 由 config version
或 `install <name> <version>` 给;sha256 空 = 跳过校验);其余为**惰性占位**
(install 报"下载源未配置")——真实 URL 是 OS 相关数据,经
`[server-manager.registry.<name>]` 提供(镜像场景本就该配自己的源/校验)。

## health 联动

`hx --health` 的语言行若检测到"语言 server 缺失且注册表有可装配方(未装)",
会附一行 `✘ 可经 server manager 安装:server install <name>`。
