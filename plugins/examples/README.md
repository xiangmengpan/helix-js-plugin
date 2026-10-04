# plugins/examples

这里放的是**示例 / 模板 / demo**,不是随软件运行的功能插件。

放 `features/` 会让人以为它们在跑,所以挪出来:

| 文件 | 是什么 | 为什么在这 |
|---|---|---|
| `lsp-hover.js` | `:lsp-hover` / `:lsp-goto` 的 LSP 主动请求**demo** | 功能与内置 `space k`(hover)、`gd`(跳转)重叠,只是演示 `helix.lsp.*` 怎么用 |
| `picker.js` | 自定义 picker 源的**模板**(它自己不注册任何源) | 文件头就写着"复制本文件即可自定义源";`init.js` 里那行是注释掉的 |

用法:照 `features/` 里的插件一样 `:plugin-load <path>` 或 `helix.load("<path>")`,
只是**不会**被内置 `init.js` 加载。

功能插件在 `plugins/features/`;共享库在 `plugins/lib/`。
