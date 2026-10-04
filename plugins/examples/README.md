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

## 移动插件文件时注意

移动/重命名插件文件后,必须同步**所有**引用它的地方 —— 实测踩过:

1. `plugins/init.js`(仓库模板)—— 有测试 `plugins_init_template_loads_resolve` 守着
2. **用户 live 的 `~/.config/helix/init.js`** —— **仓库里没有,测试也够不到**,只能人工查
   (2026-09-11:picker.js 从 `features/` 移到 `examples/` 时漏了这条,导致启动时该 load 失败)
3. `helix.plugin({deps})` 里声明过它的插件
4. 集成测试里按路径加载它的用例
5. 文档 / 示例文件自己的头注释
