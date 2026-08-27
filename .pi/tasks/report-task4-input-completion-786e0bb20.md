# 任务 4 实现报告：demo 插件 + 文档（input 组件升级 + 补全联动）

计划：`docs/superpowers/plans/2026-08-26-js-input-completion.md` 任务 4（tasks.json id 9）
Commit：`786e0bb20 feat(plugins): input-completion demo + plugin-api input 章节更新`

## 实现内容

1. **demo 插件** `plugins/features/input-completion/index.js`（新建）：
   `:ic` 打开补全弹窗——input 引擎权威值 + onChange 触发 `helix.lsp.completion()`（Promise），
   ↑↓ 走节点 onKey 导航候选，Enter 选中 echo，无 server 时显示提示行。状态用 IIFE 闭包共享
   （与 filetree/index.js 同模式）；焦点样式按 render(focus)。

2. **文档** `docs/plugin-api.md`：
   - §10 input 节点示例改为引擎权威语义（onChange 属性、光标 `|` 渲染、原始对象形式）；
   - 焦点系统新增 **input 按键语义表**（字符/Backspace/Delete→编辑+onChange；Left/Right/Home/End→仅移光标；Up/Down/Enter→onKey；Tab 移焦点；Esc 关弹窗）+ set_input_value 说明；
   - 全局 API 表补 `helix.set_input_value(popup_id, node_id, value)`。

3. **冒烟测试** `helix-term/tests/test/plugin_input.rs` 追加 `plugin_input_completion_demo`：
   对**真实仓库 demo 文件**走 `:plugin-load` → `:ic` → `<tab>a<ret>`，断言状态栏 "no match"——
   端到端验证 demo 可加载、命令注册、Tab 聚焦、编辑键 onChange、Enter 路由 onKey。

## TDD 证据

- 测试先行：先写冒烟测试（期望 "no match" 链路）→ 跑通即证明 demo 文件在真实引擎可运行。
- GREEN：`plugin_input_completion_demo` ok；`plugin_input` 模块 2 passed；
  `cargo test -p helix-js --lib` 68 passed（回归全绿）。

## 与计划的偏差（均已在代码注释注明）

1. **去掉 setTimeout 防抖**：计划 demo 草稿用 120ms 防抖，但引擎未提供 setTimeout
   （boa timers 未启用，全仓无先例）——直接调用即会在 onChange 里抛 ReferenceError。
   改为 onChange 直发请求。
2. **样式用 scope 字符串**：raw input 节点不支持 style（parse_node 不解析）、raw text 节点
   style 只接受主题 scope 字符串（obj_opt_str），计划草稿的 `{fg:"yellow"}` 对象形式无效。
   候选高亮改用 `"ui.info"`，input 上移除 style。
3. **text 属性名**：raw text 节点用 `text`（非草稿的 `content`）。
4. **Enter 用 echo 示意选中**：节点 onKey 无 doc 参数、也无全局 doc 插入 API，
   真实回填需扩展 API（计划已注明"demo 简单起见 echo"）。ponytail 注释标注。

## 文件变更

| 文件 | 变更 |
|------|------|
| `plugins/features/input-completion/index.js` | 新建（~90 行 demo） |
| `docs/plugin-api.md` | +22/-3（input 章节 + API 表） |
| `helix-term/tests/test/plugin_input.rs` | +33（demo 冒烟测试） |

## 自检

- **YAGNI**：无新抽象/新依赖；复用 `test_key_sequences` 测试基建；未动任何 Rust 生产代码。
- **边界**：items 为空时 Enter → "no match" 不崩（Down 条件 `sel < -1` 恒假）；completion reject 有 catch。
- **遗留**：`helix.el("input", ...)` 不透传 onChange（el() 白名单缺 onChange，raw 形式可用）——已在文档注明，未改代码（超任务范围）；真实回填需 doc 插入 API（扩展项）。

## 交接

计划任务 4 完成。整个计划（任务 1-4）交付物齐备；最终 review 提到的「真 LSP 环境手动验证 :ic 出候选」仍属人工验证项。
