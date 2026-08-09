# 设计：command palette 集成（并行特性 A）

日期：2026-08-09
状态：已批准

## 目标

让插件命令出现在 `:space` 命令面板（command palette）中，选中即执行。

## 现状与缺口

- `:` 命令行补全已含插件命令（v1 已合并 command_names）
- palette 选中 Typable 命令 → `MappableCommand::execute` → 静态表 miss → 插件命令回退（v4 已打通）
- **唯一缺口**：`command_palette`（commands.rs:3597）的数据源不含插件命令

## 改动（commands.rs，约 5 行）

`command_palette` 的 commands 迭代器追加：

```rust
.chain(helix_js::command_names().into_iter().map(|name| MappableCommand::Typable {
    name,
    args: String::new(),
    doc: String::new(),
}))
```

## 测试

- 集成测试（`tests/test/plugin_palette.rs`，新文件）：
  - `:plugin-load` 注册插件命令
  - `:space` 打开 palette → 下移选择插件命令 → `<ret>` → 状态栏断言命令 echo 输出
  - palette 导航键与既有 picker 测试模式一致（参照 tests/test/ 里 picker 相关测试）

## 非目标

- palette 中插件命令的 doc/签名显示（doc 为空字符串，可后补）
- 分组/排序优化

## 涉及文件

- `helix-term/src/commands.rs`（command_palette 数据源）
- `helix-term/tests/integration.rs`（mod 声明，合并时由控制器统一加）
- `helix-term/tests/test/plugin_palette.rs`（新，集成测试）
