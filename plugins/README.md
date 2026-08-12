# plugins/ — JS 插件（helix 魔改版插件系统）

插件系统说明见 [`docs/plugin-api.md`](../docs/plugin-api.md)。

## 安装

把 `.js` 文件复制到 `~/.config/helix/plugins/`，并在 `~/.config/helix/init.js` 中 `helix.load(...)`：

```bash
cp plugins/{icons.js,filetree.js} ~/.config/helix/plugins/
cp plugins/init.js ~/.config/helix/init.js   # 入口示例（自动加载 icons + filetree）
```

> 启动只自动加载 `~/.config/helix/init.js`；其他插件必须经 `helix.load` 导入。
> 修改插件后 `:plugin-reload` 生效（重读磁盘 + 重置插件状态）。

## icons.js — 统一图标映射表（nerd font）

唯一图标源：方案 1（bufferline 文件类型图标，load 即自动注册）、方案 2（filetree 树内图标）、
方案 3（状态栏 mode 图标）、方案 4（诊断标记图标，Rust 侧 `helix.set_diagnostic_icons`）都从这里取。

| 映射 | 内容 | 获取 |
|------|------|------|
| `ICONS.file` | 扩展名 → 文件类型图标（rust/js/ts/md/py/...） | `getFileIcon(path)` |
| `ICONS.special` | 特殊文件名（README/Dockerfile/Cargo.toml/...） | `getFileIcon(path)`（优先匹配） |
| `ICONS.dir` | 目录图标（折叠/展开） | `getDirIcon(expanded)` |
| `ICONS.mode` | 状态栏模式图标 | `getModeIcon(mode)` |
| `ICONS.diagnostic` | 诊断标记图标 | `getDiagnosticIcon(sev)` |
| `ICONS.git` | git 变更状态图标 | `getGitIcon(status)` |

```js
helix.load("icons.js");                          // 自动注册 bufferline 图标
const icons = helix.load("icons.js");            // 其他插件取映射表
icons.getFileIcon("src/main.rs");                // rust 图标
helix.set_diagnostic_icons(icons.ICONS.diagnostic); // 方案 4：诊断标记列图标
```

**前提**：终端使用 nerd font（Windows Terminal 设置字体 / vscode `terminal.integrated.fontFamily`）。
图标表可自行增删（`ICONS.file` 是普通对象）。

## statusline.js — 状态栏美化（lazyvim 风格右侧信息）

依赖 `set_statusline` 分段样式（Rust 增强）：右侧从右到左显示
mode 色块 → 文件名（带类型图标）→ git 分支（异步缓存）→ 诊断计数（红/黄）→ 行:列 → 百分比 → 总行数。

```js
helix.load("statusline.js");   // init.js 已含；依赖 icons.js
```

- mode 色块用主题自带 `ui.statusline.normal/insert/select` scope（跟随主题）
- git 分支在 buffer-open 时 `run_async` 查询一次并缓存（状态栏渲染零开销）
- 左侧保持 helix 默认（可在 `config.toml` 的 `[statusline]` 调整元素）
- 自定义分段：`helix.set_statusline((ctx) => [{ text, style? }, "str", ...])`

## terminal.js — 终端管理器（lazyvim 风格）

依赖 Rust 增强：浮动终端层、C-\ 模式穿透、滚动缓冲、term_list/term_close、宽字符。Unix pty。

| 命令 | 行为 |
|------|------|
| `:term` | 浮动终端 toggle（单例复用：开 → 居中浮窗；再开 → 关闭） |
| `:vterm` / `:hterm` | 右侧 / 底部 分屏终端 |
| `:term-list` | 列出所有终端（view_id + cmd） |
| `:term-close` | 关闭浮动单例（无则最近打开的） |

终端内按键：`C-\` 切 normal（j/k/gg/G/PageUp/PageDown 滚动缓冲查看）；normal 再 `C-\` 回编辑器（收起浮窗）；
normal 内 `i`/`a`/`Esc` 回输入、`q` 关闭；insert 模式 `Esc` 关闭。

## filetree.js — 侧边文件树面板

类 lazyvim/neo-tree 的目录树。`:filetree` 开关面板，`:filetree-reveal` 定位当前文件。

| 操作 | 键位 |
|------|------|
| 打开文件 / 展开折叠目录 | `Enter` / `o` |
| 展开并选中首子项 / 折叠或回父节点 | `l` / `Right`、`h` / `Left` |
| 移动选中行 | `↑` / `↓`、`j` / `k`、`PageUp` / `PageDown`、`Home` / `End` |
| 上级目录（重设树根） | `P` / `Backspace` |
| 切换隐藏文件 | `H` |
| 刷新（保留展开态与光标） | `R` |
| 新建文件 / 新建目录 | `a` / `A`（输入弹窗） |
| 重命名 / 删除 | `m` / `d`（删除有确认弹窗） |
| 跟随当前文件 | `F` 开关（buffer-open 自动 reveal） |
| 键位帮助 | `?` |
| 关闭面板 | `q` / `Esc` |

未映射的键穿透给编辑器：面板打开时 `:命令`、`i` 插入等照常可用。

### 设计说明

- **纯逻辑与运行时分离**：排序/扁平化/路径链/转义为纯函数（无 helix 依赖），可被 node 直接测试：
  ```bash
  node --check plugins/filetree.js
  node -e "require('./plugins/filetree.test.js')"  # 或直接跑仓库测试
  ```
  （`filetree.test.js` 在 `~/.config/helix/plugins/`，未入仓——自检方式见下）
- **选中行由 JS 维护**：面板无节点点击命中（API 限制，见 plugin-api.md §18），渲染按行号高亮 `ui.selection`。
- **虚拟化渲染**：以光标为中心截取面板高度窗口，`scroll` 只布局窗口行；超过 2 万可见行截断提示。
- **懒加载**：目录首次展开才 `read_dir`（同步，每层一次）；折叠不丢 children（缓存）。
- **已知天花板**：纯键盘（无鼠标命中）；删除 `rm -rf` 无回收站；`helix.run("pwd")` 首次开面板时同步一次。

### 自检

纯逻辑断言（node，无需 helix）：

```bash
node -e "
const assert = require('assert');
const m = require('./plugins/filetree.js');
assert.deepStrictEqual(
  m.visible_rows(m.make_tree('/r', [{name:'a',is_dir:true,path:'/r/a'},{name:'b',is_dir:false,path:'/r/b'}]), false, 0, []).map(r=>r.node.name),
  ['a','b']
);
console.log('ok');
"
```

集成测试（真实编辑器环境）：`helix-term/tests/test/filetree.rs`，覆盖 reveal/展开/打开、隐藏切换、新建文件、关闭面板。

### 集成测试里的产品代码修复

开发 filetree 测试时发现并修复了两处真实 bug（含回归测试）：

- `Compositor::reset_plugin_diffs` 只重置 layers 里的面板 diff，漏掉布局树叶子里的面板——新 surface 渲染空白（`plugin_layout2.rs` 回归测试）。
- 面板/终端叶子吞键的已知问题在面板侧：未映射键现在穿透，面板打开时 `:命令` 仍可用。
