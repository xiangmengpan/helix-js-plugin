# 设计:helix.picker(方案 C 最小版——核心骨架 + JS 数据源)

日期:2026-08-30
状态:已批准(分节讨论确认)

## 背景与目标

核心已有泛型 `Picker<T, D>`(ui/picker.rs):nucleo 过滤、多列、FileLocation 预览(`with_preview`)、历史、动态查询,多个内置命令(:open/:symbols/:grep)各自实例化。缺的是**插件可定义源**。目标:核心 Picker 骨架保留(性能原生),`helix.picker.define/run` 把"候选数据 + 列格式 + 预览 + 选中动作"开给 JS;内置插件 `picker.js` 提供 4 个默认源。**不做 builder 旋钮透传**(用户明确砍掉)。

## 节 1:核心 API

### 1.1 `helix.picker.define(name, config)`(同步注册)

```js
helix.picker.define("files", {
  columns: ["name", "path"],                    // 显示列名(核心算列宽)
  items: () => [["main.rs", "src/main.rs"], ...], // 每行数组,长度 = columns
  preview: (row) => ({ path: row[1], line: 0 }),  // 可选;返回 {path, line} → FileLocation 预览
  action: (row) => helix.open_file(row[1]),       // 可选;Enter 回调
});
```

- `items` 同步返回二维字符串数组;可选 async(返回 Promise,resolve 后注入)
- `preview`/`action` 可选;缺省:无预览、Enter 关闭 picker
- 注册到 helix-js 的源表:`HashMap<String, JsValue>`(config 对象引用)
- 重复 define 同名 → 覆盖

### 1.2 `helix.picker.run(name)`(调起)

- 查源表;未注册 → 报错(回显错误)不崩
- 调 `items()` 拿候选(异步则等待 resolve,期间显示加载态/空列表)
- 核心构建 `Picker<PickerRow>`:

```rust
pub struct PickerRow {
    pub cells: Vec<String>,    // 显示列,长度 = columns 数
    pub payload: Vec<String>,  // 与 cells 相同(JS 数据本身);action/preview 回调时原样回传
}
```

- 过滤:nucleo 对**第一列**(`cells[0]`)模糊匹配(与内置 picker 同款 Atom::Fuzzy)
- 渲染:多列表格,列宽核心计算(复用 Picker 现有 Column/widths 机制);列数 = `columns.len()`,超出截断
- 键位/滚动/历史:核心现有行为,零改动
- Enter:调 JS `action(row)`(payload 数组传入);缺省 → 关闭
- 预览:有 `preview` 时,选中项经 `preview(payload)` 返回 `{path, line}` → 复用现有 FileLocation 预览渲染

### 1.3 与内置命令的关系

- 并存不替代:内置 :open/:symbols/:grep 原样保留
- `Picker<PickerRow>` 是新驱动通道,不改现有 `Picker<T, D>` 泛型接口

## 节 2:递归文件列表 API

`helix.read_dir(path)`(同步,不递归)不够 files 源用。新增:

```js
helix.read_tree(path, { depth })  // async → [{path, name, is_dir}]
```

- 异步递归列出 `path` 下文件树(先序),`depth` 限制深度(缺省无限制)
- 返回 `[{path, name, is_dir}]`,**目录先行、同级按名排序**
- 复用 notify/文件系统层现有异步模式(参照 read_dir 的 JS 绑定 + tokio spawn)

## 节 3:内置插件 `plugins/features/picker.js`

| 源 | 数据 | 预览 | action |
|---|---|---|---|
| files | `helix.read_tree(cwd)`(cwd = 与 :open 一致) | 文件 → 行 0 | `helix.open_file(path)` |
| grep | `helix.run_async("rg -n --no-heading <query> <dir>")` 解析 `file:line:text`(相对路径拼 cwd) | `{path, line}` | 打开文件定位行 |
| buffers | 现成 `helix.buffers()` | 文件预览 | `helix.focus_buffer(id)` |
| symbols | 现成 `lsp.document_symbols()` | 当前文档 `{path, line = selectionRange 起始行}` | 跳转 |

- grep 的 query 来源:picker 内输入过滤的是核心(过滤候选),**grep 源需要以 query 重跑命令**——边界:最小版 grep 源在 `helix.picker.run("grep")` 时以空 query 列出全部(rg 输出有限制),或接受"输入过滤仅过滤已加载候选"。**决策:最小版 grep 用输入框前缀作 query 重跑**需动态查询钩子——核心 `Picker` 有 `with_dynamic_query`(builder 旋钮,用户砍了)。**因此最小版:grep 一次加载(当前目录全部文件行号,加 `-l` 限制规模或接受大结果),输入过滤走 nucleo**。真正"按输入重跑 rg"留到旋钮版。规格明示此边界。
- 插件注册键位示例(`helix.map`)写入插件注释,不默认绑定

## 节 4:边界(最小版明确不做)

- 候选一次性返回,非流式(万级文件目录有启动延迟;升级:流式/分批 API)
- 无 builder 旋钮透传(dynamic_query/历史/默认动作等保持核心默认)
- 预览仅文件(FileLocation);无 grep 上下文/任意文本预览
- 无核心 `:picker` 命令与源切换 UI(源由插件自己调起)
- grep 不做"按输入重跑",仅一次性加载 + nucleo 过滤

## 测试策略

- **helix-js 单测**:define 注册/覆盖/参数校验(columns 与 items 行宽不匹配 → 报错);run 未注册 → 报错;action/preview 缺省行为;async items resolve 注入
- **helix-term 单测**:`PickerRow` 构建/第一列过滤/列宽计算/Enter 调 JS action(payload 回传正确);preview 转 FileLocation
- **integration**:`helix.picker.define/run` 端到端——定义源 → run → 候选出现 → Enter 触发 action(echo 断言);未注册源报错路径

## 不做的(明确排除)

- 旋钮透传、流式注入、任意文本预览、核心源切换 UI、grep 动态重跑——均留后续(旋钮版/流式版)

## 关键实现位置

- helix-js/src/picker.rs(新):define/run 注册表 + 参数校验 + items 调用
- helix-js/src/lib.rs(native fn 注册)、state.rs(源表存储)
- helix-term/src/ui/picker.rs 或新 ui/js_picker.rs:`PickerRow` + 驱动(过滤/渲染/Enter/preview 桥)
- helix-js/src/fs.rs(read_tree)
- plugins/features/picker.js(内置插件 4 源)
- docs/plugin-api.md(两节:helix.picker + read_tree)
