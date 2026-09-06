# arsenal 市场窗 UI 改版(自绘窗框 / 分类 Tab / `/` 搜索模态 / 全英文)设计规格

日期:2026-09-07
状态:待用户审查
上游:2026-09-06 arsenal-design.md(能力/数据/任务通道不变,本文只改**前台渲染与键位**);实现计划另行产出。

## 0. 决策记录(用户确认)

- **方案甲**:arsenal 主窗由插件内容**自绘窗框**(顶框嵌标题、底框),不依赖容器 `popup-border` 配置;容器只提供 `ui.popup` 背景色。⚠ 若用户全局 config 已设 `popup-border="all"`,容器会在自绘框外再画一圈 → 文档注明关闭或接受双框(JS 无法读 config,不自动探测)。
- **分类用 Tab**:lsp/dap/linter/formatter 是不同页签,Tab 键切换(用户点名);保留 `all / installed / local` 为默认页签以延续既有过滤能力(用户可改)。原 `f` 循环键**移除**。
- **搜索改模态**:NORMAL 态字母全走命令(放弃"字符即搜");按 `/` 进入 SEARCH,此后可打印字符进查询;Esc 退 SEARCH(清查询)、再按才关窗。触发键 `/`(用户点名)。
- **Q1 = 是**:内置配方描述(中文数据)一并翻英——arsenal 渲染面不出现中文。
- 不变量:行数据/动作/任务通道/批量语义全部沿用,零后端能力改动。

## 1. 目标

改版后 arsenal 主窗:有自绘边框与标题头;lsp/dap/linter/formatter(+all/installed/local)页签式切换;`/` 模态搜索;全部渲染文案(行描述、标签、帮助、回显、错误兜底、任务 phase)为英文;宽高与版式贴合容器内宽、状态列贴框内右缘、选中行整行铺满;颜色只用现有主题 scope(无硬编码)。

## 2. 现状与证据(2026-09-07 真渲染抓屏)

集成测试 `server_arsenal_ui_smoke` 渲染 120×40 画布实测,主窗 78%×75% 居中(93×30 @ (13,5)):

- 无边框、无标题条:内容直接浮在 `ui.popup` 背景上;顶栏一行 `arsenal · all · 52/52 servers` 用 `ui.virtual` 暗色,不像标题头。
- 行版式:名称列定宽 24 → 描述**紧贴名称无分隔**;描述弹性列把状态列推到 14 宽固定槽,状态**右缘空洞**(靠补空格凑满宽),观感"宽高/排版不对"。
- 行描述大量中文,来自 **Rust 内置配方**(仅 7 条 `.describe()`)+ **contrib/server-manager-recipes.toml** 描述 + 测试假配方;UI 文案/帮助/echo 亦全中文。
- 搜索为"字符即搜":无过滤时按 `j/k/f/t` 等任意字母直接进搜索。
- 顶部大框(整画布)是编辑器窗口外框,与 arsenal 无关;确认 JS 侧无标题/边框能力 → 自绘。

## 3. 主窗设计

### 3.1 行结构与尺寸

容器:center,`78%×75%` 不变。内容行结构(自上而下):

| 行 | 内容 | 高度 |
|---|---|---|
| 0 | 顶框线 `┌ arsenal ─…─ 52 recipes ─┐`(标题嵌框,右侧总数) | 1 |
| 1 | 页签条(tab bar)+ 右侧搜索/标记提示 | 1 |
| 2..H-3 | 列表行 | H-4 |
| H-2 | 帮助行(常态)/ 搜索帮助 / busy 进度行(互斥) | 1 |
| H-1 | 底框线 `└──…──┘` | 1 |

- 内宽 `W = ctx.width`(已含左右框 1 字符空间:所有内容行首尾留 1 空格,框竖线画在 0 与 W-1)。所有行(含列表空行)恒铺满 `W-2` 可视宽 → 无尾部空洞。
- 列表可见行数 `LIST_H = ctx.height - 4`;`S.sel` 居中滚动窗口不变(见现状实现)。`ctx.height < 6` 时仅渲染框与提示行(防御,终端极少见)。
- 顶框右侧计数文案:`<visible>/<total> recipes`(如 `3/52 recipes`;单数 1 recipe 不特殊处理,统一 recipes)。

### 3.2 页签条(tab bar)

页签序:`all · lsp · dap · linter · formatter · installed · local`(对应现 `KINDS` 全序,语义 = 行过滤不变)。

- `Tab` → 下一个页签(循环);`Shift+Tab`(key.shift=true)→ 上一个(循环)。切页签 `S.sel=0`。
- 激活页签样式 `ui.selection`(整块反色),非激活 `ui.virtual`;分隔 `│`。整行拆成多个 text 节点(`el("row",…)`)以便逐页签着色;右侧(`search:/py` / `2 marked`,截断保护)单节点贴右。
- 键位语义:搜索态下 Tab 仍切页签(查询保留,防误触需求大于防跳转);行内无 Tab 焦点。

### 3.3 搜索模态

状态 `S.filter` 保持;键位语义改为**模态**:

| 状态 | 可打印字符 | `/` | Backspace | Esc | Enter | ↑↓ | j/k | q |
|---|---|---|---|---|---|---|---|---|
| NORMAL | 忽略(消费) | 进 SEARCH(`S.filter=""`) | – | 关窗 | 动作/批量 | 移动 | 移动 | 关窗 |
| SEARCH | 追加进查询 | 忽略 | 删尾 | 清查询退 NORMAL | 动作/批量 | 移动 | 移动(仅此二键) | 查询字符 |

- SEARCH 内 `x/u/f/t/r/C/i` 等一律为查询字符(与 I2 精神一致,但触发由 `/` 显式开启)。
- 顶框标题在 SEARCH 态不变;页签条右侧显示 `/query`(超长截断),便于看到输入状态。
- 默认动作入口(Enter:有标记→批量,无→动作菜单)与现实现一致,SEARCH 内 Enter 同样生效。

### 3.4 行模型(列宽修正)

列 = `mark(2) + glyph(1) + space(1) + name | gap(1) | desc | gap(1) | status`,总宽恒 `W-2`:

- name 列:定宽 `min(24, 0.34*(W-2))`,超长截断(去尾补 `…` 可选,不加)。
- desc 列:弹性 = 剩余宽度;截断到列宽。
- status 列:固定 `ST_W=14`,`status_text()` 结果左对齐填槽,槽右缘 = 行右缘(即内容右边界),不再有"状态后空洞"。
- 行选中样式 `ui.selection` 作用于整行文本节点 → 反色条铺满行宽。
- 空列表:`(no matches — press Esc to clear / q to close)` 样式 `ui.virtual`。

### 3.5 busy / 错误

- busy:底帮助行整行替换为进度行(现 `busy_line()`):`▮▮▮▯ 45% (1/2) demo-bin install`;任务中页签/搜索仍可用(只读刷新);`S.busy` 守卫不变量照旧。
- render 兜底:错误文案改 `arsenal render error: <msg>`(样式 `ui.popup`),不再透中文。
- 行状态 `status_text`:受管 `✓ v` / 可升级 `▲ v` / 本机已有 `local`(现"本机"→英文,含版本拼 `local <v>`)/ 未装 `–` / `no source` 不变;行 busy 覆写同现状。

### 3.6 主题

只用现有 scope,禁硬编码颜色:框线/页签非激活/帮助/顶框计数 `ui.virtual`;激活页签/选中行 `ui.selection`;busy 行 `ui.popup`(含 fg 语义由主题回填);错误行同 `ui.popup`。行默认 fg 由引擎回填 `ui.text`。

## 4. 浮层小窗统一(动作菜单 / 版本输入 / 信息)

- 三个小窗与主窗同语言同视觉:加**顶框线嵌标题** + **底框线**(复用同一 helper,如 `frame(title, body_lines, W)`),正文行首尾 1 空格。
- 尺寸:现 width/height 数值保留(菜单 44/信息 68/版本输入 54),内容宽度 = width-2。
- 文案全英文:动作标签 `install / update / remove / unmanage`(去中文括注);信息行标签 `status: / languages: / description: / homepage: / command: / source: / version:`;版本输入提示 `requires explicit version ({version} in source URL)`、`type to edit · Enter submit · C clear memory · Backspace delete · Esc cancel`;菜单帮助 `↑↓/jk select · Enter run · i info · Esc/q close`。
- `helix.echo` 与状态栏回显英文:开窗重复 `arsenal: already open (Esc/q to close)`、忙拒 `arsenal: a task is running, please wait…`、批量汇总 `arsenal: batch done n/m`(+`failed: …`)、清记忆/空版本/无动作等全部英化。

## 5. 文案与数据英化范围(arsenal 渲染面零中文)

1. **JS 全量 UI 文案**(见 §3/§4 清单 + index.js 内所有字符串字面量)。验收:渲染画布 grep 无 CJK。
2. **Rust 内置配方描述 7 条**(helix-term/src/commands/server_manager.rs builtins,`.describe()`):转英文短描述(如 rust-analyzer → `"Rust language server"`;译文表实现时按行编写,保持同长度级)。
3. **contrib/server-manager-recipes.toml** 描述段(TS/JSON/Bash/… 语言服务器类,共 ~4+ 条)转英文。
4. **任务 phase 文案 2 条**(run_task `progress("下载中",…)`/`"写入配置"`)→ `"downloading"` / `"writing config"`。
5. 集成测试假配方 description(`假 demo 工具`)顺带改英文样例 `"Fake demo tool"`(仅测试数据)。
6. **非目标**(明示边界):`:server` 命令族自身的中文状态/回显、languages.toml 冲突提示、后端 `Err` 上下文文案(如 `gzip 解压失败` 类)不在本改版;任务失败原因经 `(msg)` 透传可能含中文——arsenal 失败汇总行前缀为英文,原因原样透传,文档注明(避免范围蔓延到整个 server manager i18n)。

## 6. 键位表(最终)

```
NORMAL:  ↑↓ / j k 移动      /  进搜索            Enter 动作菜单(有标记→批量)
         i 信息   t 标记     x 卸/unmanage  u 升级  r 刷新
         C(Shift+c) 清版本记忆                  Tab/Shift+Tab 切页签   q/Esc 关窗
SEARCH:  可打印字符进查询    Backspace 删尾      ↑↓ / j k 移动
         Enter 动作/批量     Tab 切页签          Esc 清查询回 NORMAL
busy 中: 写动作拒绝(echo),其余照常
```

移除:`f` 分类循环、NORMAL 态"字符即搜"。

## 7. 测试影响与验证

- **helix-term/tests/test/server_manager.rs** `server_arsenal_ui_smoke` 断言之中文文案改英文:`rust-analyzer 需显式版本`→`requires explicit version` 系、`命令: (未安装)`→`command: (not installed)`、`下载源:`→`source:`、`update 升级`/`remove 卸载`→`update`/`remove`、kind 标题断言 `· lsp ·`→页签激活断言(含 `lsp` 激活样式串/`all` 位置)、`受管/` 类(`:server status`)不动。新增 Tab 切页签、`/` 进搜索、Esc 退搜索断言;既有 `f` 用法按键序列改写(如 `f`→`<tab>`),I2 注释更新。
- **plugins/features/arsenal/tests/arsenal.test.js**:echo/文案断言同步英文;键位用例覆盖 `handle_key`:NORMAL 字母不再进 filter、`/` 后才可输、SEARCH Esc 清退。
- **node 级现有行夹具** description 英文化即可,断言不动则删相关断言。
- 截图验收:渲染无中文、框线/页签/右贴状态可见;`docs/superpowers/specs/2026-09-06-arsenal-design.md` 顶部加指针指向本规格;`docs/arsenal.md` 键位/布局/搜索说明更新(语言:中文文档保留,内容与新键位一致)。
- 回归:`cargo test --features integration` arsenal 相关 + node --test arsenal + `cargo fmt/clippy` 干净(涉及 Rust 字符串改动)。

## 8. 自检

- 占位符:无 TODO/待定;译文表实现期落地。
- 一致性:§3/§4 文案与 §5.1 验收一致;键位表与 §3.3 表一致;不变量(能力零改动)全文未违背。
- 范围:单一实现计划可覆盖(JS + 2 Rust 文件 + 测试/文档),无需拆分。
- 歧义收敛:页签集 = 7 项(用户点名 4 项 + all/installed/local 默认保留);`/` SEARCH 内二次 `/` 忽略(不输入字面 `/`);SEARCH 内 `q` 是查询字符——已明示。
