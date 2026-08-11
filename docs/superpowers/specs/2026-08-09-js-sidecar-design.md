# 设计：文件树/终端面板补全 API（v13）

日期：2026-08-09
状态：已批准（自主执行）

## 目标

补齐 VS Code 式侧边能力所需的三个 API：打开文件、面板移动、结构化读目录。然后给出文件树面板与可移动终端面板的演示插件。

## 新增 JS API

```js
helix.open_file(path)             // 在编辑器中打开文件（当前视图打开；目录/不存在 → 抛错）
helix.move_panel(id, side)        // 把面板移动到另一侧（"right"|"left"|"bottom"）
helix.read_dir(path)              // -> [{ name, is_dir, path }]：结构化列目录（同步，不递归）
```

## 实现

### helix-js

- `UiRequest::OpenFile { path: String }`、`UiRequest::MovePanel { id: u64, side: String }`
- `js_open_file`（arity 1，字符串校验 → 入队）
- `js_move_panel`（arity 2，id 数字 + side 白名单 → 入队）
- `js_read_dir`（arity 1，字符串校验 → `std::fs::read_dir` → 排序条目：`Vec<JsObject { name, is_dir, path }>`；失败抛错；无递归）

### helix-term

- **OpenFile drain**（命令/事件/异步 pump 的 drain 点）：`editor.open(&path, Action::Replace)`（复用 `:open` 的打开逻辑——`typed::open_impl` 或直接 `editor.open`；以最简可用为准，错误 set_error）
- **MovePanel drain**：`compositor` 找 PluginPanel 层按 id → `set_side(side)`（PluginPanel 加 set_side 或 compositor 辅助；下一帧渲染自动重排——compositor 每帧枚举面板）
- 面板层查找：MovePanel 的层操作需要 compositor——drain 点在命令路径（无 compositor 访问）→ 走 job 通道（dispatch_blocking）或独立全局面板注册表（以现有 ClosePanel 的实现模式为准，保持一致性）

## 测试

- **helix-js 单测**：三个 API 的校验与入队（OpenFile/MovePanel 字段断言；read_dir 对临时目录返回条目与排序）
- **集成测试**（tests/test/plugin_sidecar.rs，临时 mod）：
  - `:ft-open` 命令调 `helix.open_file(<临时文件>)` → 断言文档文本变为该文件内容（editor.documents 含它）
  - `:ft-move` 命令开面板后调 `helix.move_panel(id, "left")` → 断言面板层 side 变更（渲染断言或层内字段访问——以可实现为准）
  - read_dir 经命令 echo 条目数 → 状态栏断言

## 演示插件（控制器编写）

- `filetree.js`：右侧文件树面板——`helix.read_dir` 递归建树（懒展开），↑↓ 导航、→/Enter 展开目录、Enter 打开文件（`helix.open_file`）、Esc/← 收起
- 可移动终端：terminal 面板 + `:term-left`/`:term-right`/`:term-bottom` 命令（`helix.move_panel`）

## 非目标

- 目录监听/自动刷新（手动刷新命令）、git 状态集成进树、拖拽移动面板（命令式换侧）
- read_dir 的符号链接/权限细节（is_dir 用 entry.file_type()，错误条目跳过）

## 涉及文件

- `helix-js/src/lib.rs`（三个 API + 单测）
- `helix-term/src/commands/typed.rs`（drain：OpenFile/MovePanel）
- `helix-term/src/ui/plugin_panel.rs`（set_side）
- `helix-term/tests/test/plugin_sidecar.rs`（新）
- 演示 + 文档
