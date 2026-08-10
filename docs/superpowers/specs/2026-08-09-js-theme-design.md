# 设计：改主题（v10-②，并行 wave 1）

日期：2026-08-09
状态：已批准（自主执行）

## 目标

插件实时修改主题：`helix.set_theme({scope: color})` 覆盖任意主题 scope 颜色，`helix.reset_theme()` 还原。

## API

```js
helix.set_theme({ "ui.popup": "#ff00aa", "error": "red", "warning": "yellow" });
helix.reset_theme();
```

- set_theme 对象：scope → 颜色字符串（"red"/"#rrggbb" 等，theme palette 支持的解析）
- 覆盖即时生效（重建 Theme 并替换 editor.theme）
- reset_theme：清空覆盖，恢复启动时的基础主题
- 非法 scope/颜色：忽略该条目（不报错）或报错——以实现最简为准（推荐：非法颜色忽略）

## 实现

- **helix-view/src/theme.rs**（小改动）：
  - `merge_themes` 从私有改 `pub`（toml Value 递归合并，已被 loader 使用）
  - 新增 `ThemeLoader::load_raw(name: &str) -> Result<Value>`（读主题文件返回原始 toml Value；load() 内部可复用）——或 application.rs 直接读文件路径，以 loader 实际结构为准
- **helix-term**：启动时捕获基础主题 toml Value（`BASE_THEME_TOML: OnceLock<Value>`）；`apply_theme_overrides(editor, &HashMap<String,String>)`：base + overrides（`{scope: {fg: color}}`）→ `Theme::from(merged)` → `editor.theme = Arc::new(theme)`；`reset` 用 base 直接重建
- **helix-js**：`THEME_OVERRIDES`（泄漏容器 HashMap<String,String>）+ `js_set_theme`（对象迭代校验）+ `js_reset_theme`；`take_theme_changed() -> bool`（drain 标记）或直接 `pub fn theme_overrides() -> Option<HashMap>`——以"变化才触发重建"为准
- **drain 接线**：命令/事件/pump 的 drain 点统一加 theme 检查（变化 → apply_theme_overrides）

## 测试

- 单测：js_set_theme 解析（对象/非法颜色忽略/空对象）；reset 清空
- 集成：`helix.set_theme({"ui.popup": "#ff00aa"})` → 断言 `editor.theme.get("ui.popup").fg` 变化；`reset_theme` → 恢复

## 涉及文件

- `helix-view/src/theme.rs`（pub merge_themes + load_raw 或等价）
- `helix-js/src/lib.rs`（API + 存储 + 单测）
- `helix-term/src/application.rs`（base 捕获 + apply + drain）
