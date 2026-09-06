# arsenal 市场窗 UI 改版(自绘窗框/分类 Tab/`/` 搜索模态/全英文)实现计划

日期:2026-09-07
规格:docs/superpowers/specs/2026-09-07-arsenal-ui-v2-design.md(已用户批准,会话自主执行)
模式:单文件小步 commit,每任务独立验证。

## 文件地图

- `plugins/features/arsenal/index.js` — 主改:渲染(自绘框/页签/列宽/省略号)、搜索模态、Tab 键、英文文案、wc_* helper、弹窗英文+框、module.exports 增纯函数。
- `plugins/features/arsenal/tests/arsenal.test.js` — 语义(模态搜索/移除 f/Tab)、echo 英文、wc helper、行夹具 description 英文。
- `helix-term/tests/test/server_manager.rs` — arsenal 集成断言英文化 + 按键序列改 Tab// + 新增 Tab//CJK 回归(含真渲染验证脚本片段,验证后移除)。
- `helix-term/src/commands/server_manager.rs` — 7 条内置描述 + phase 2 条("下载中"/"写入配置")英化。
- `contrib/server-manager-recipes.toml` — 描述英化。
- `docs/arsenal.md` — 键位/布局/搜索说明同步。

## 键位语义(定稿,与规格一致)

- NORMAL:可打印字符**不搜索**;`j/k/↑↓` 移动;`/` 进 SEARCH;Tab 下一个页签/Shift+Tab 上一个;Enter 动作(有标记→批量);i/t/x/u/r/C/q/Esc 同现状。
- SEARCH:一切可打印字符进查询(含 j/k/q/f/x/u/t/r/i 等);Backspace 删尾;↑↓ 移动;Enter 动作/批量;Tab 切页签(查询保留);Esc 清查询回 NORMAL。`/` 在 SEARCH 内忽略。
- 移除 `f` 分类循环与 NORMAL 即搜。

## 任务

### T1 JS 纯逻辑 + node 测试(先测后码,最小环)
- wc_len/wc_trunc/wc_pad;S 语义:SEARCH 态判定 = S.searching(新增布尔,或复用 filter+flag;定:`S.searching`),handle_key 重写分支、Esc/关窗语义、Tab/Shift+Tab 循环(移除 cycle_kind,新增 tab_next/tab_prev)。
- 英文:status_text local 前缀、所有 echo/失败/批量汇总/弹窗文案、菜单 label 去中文括注、info/version 文案、render 兜底;busy 提示。
- node 测试:更新旧 I2/I4/M-f 断言为新语义英文;新增:wc 夹具(CJK 双宽/截断/不中腰)、NORMAL 字母不搜索、`/` 进、Esc 退、SEARCH 全可打印进、Tab 循环。
- 验证:`node --test plugins/features/arsenal/tests/arsenal.test.js`;commit。

### T2 主窗渲染(窗框/页签/列宽)+ 集成冒烟
- main_render 重建:顶框(嵌标题+计数)、tab 行(逐页签 el row 节点)、列表行(右侧贴边 status;wc 截断+`…`)、帮助/搜索提示/busy 行、底框;空态/错误兜底英文。
- 弹窗(menu/version/info)同框样式 + 英文。
- 集成测试 server_manager.rs:断言文案英文化、f 序列改 `<tab>`、新增:Tab 切 lsp(黑/prettier 消失)、`/` + 过滤 + Esc 回、CJK 描述注入回归(截断不越右缘);真渲染抓屏确认版式。
- 验证:`cargo test --features integration --test integration server_arsenal`(smoke+batch);commit。

### T3 数据英化(Rust/contrib)
- 内置 7 描述英文(短句,风格一致);phase "downloading"/"writing config";contrib 描述英文;测试假描述 "Fake demo tool"(integration 内)。`cargo fmt`;相关单测/集成复跑;node 不受影响。
- 验证:上述 cargo 测试仍绿 + fmt;commit。

### T4 文档与收尾
- docs/arsenal.md 键位/布局/搜索/描述更新;确认渲染面无 CJK(真渲染 grep)。
- 回归:`node --test arsenal` + `cargo test --features integration --test integration server_arsenal` 全绿 + cargo fmt --check;commit。

## 风险/注意
- comp_layout `row` 子节点逐行拼接(逐页签样式可行);Text 截断按 char,故 JS 必须先把每行列宽压到 ≤ W(宽字符安全)。
- 引擎 text 行超 viewport 会被裁:所有行字符串列宽精确 W,不依赖引擎裁切。
- 集成测试运行较慢(编译 ~40s+运行),改动后批量复跑一次即可。
- 后端错误原因透传不英化(规格非目标);arsenal 前缀/包装全英文。
