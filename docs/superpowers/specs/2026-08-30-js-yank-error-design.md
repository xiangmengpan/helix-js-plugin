# 设计:yank-error(错误可复制)

日期:2026-08-30
状态:已批准(方案确认:最近一条 + `+` 寄存器)

## 背景与目标

插件命令报错(如 `:plugin-reload` 失败)走 `set_error` → 状态栏一次性显示,被下一条消息覆盖即丢失,无法复制/留存。目标:任何 `set_error` 的错误可事后复制(`:yank-error`)。

## 方案(方案 a 最小版)

### 1. 错误历史存储(helix-view/src/editor.rs)

`Editor` struct 加字段:

```rust
/// 最近一次 set_error 的文本(供 :yank-error 复制);None = 无错误
pub last_error: Option<Cow<'static, str>>,
```

### 2. set_error 记录(editor.rs:1546)

```rust
pub fn set_error<T: Into<Cow<'static, str>>>(&mut self, error: T) {
    let error = error.into();
    self.last_error = Some(error.clone());   // 新增
    self.status_msg = Some((error, Severity::Error));
}
```

- 唯一改动点;所有 `set_error` 调用(插件命令、普通命令、加载错误)自动覆盖
- 不记录 set_warning(只错误;warning 不进,避免噪音)

### 3. 新命令 `:yank-error [register]`(helix-term/src/commands/typed.rs)

- 注册名 `yank-error`,fun 与 `yank_diagnostic` 平行
- Validate 时:取 `cx.editor.last_error`,写寄存器(默认 `+`,可传单字符寄存器,同 yank_diagnostic 参数语义)
- 无错误 → `bail!("No error to yank")`(与 "No diagnostics under primary selection" 同款)
- 成功 → `set_status(format!("Yanked error to register {reg}"))`

### 4. 边界

- 最近一条:新的 set_error 覆盖旧的(最小版不做历史列表)
- 与 yank-diagnostic 平行,互不影响(yank-diagnostic 继续服务 LSP 诊断)
- 不覆盖剪贴板:只有显式 :yank-error 才写寄存器

## 测试策略

- **单测**:set_error 记录 last_error(editor 单测);无错误时 yank-error 报错
- **integration**:`:plugin-reload` 失败(指向不存在插件)→ set_error → `:yank-error` → 寄存器内容 = 错误文本;无错误路径报错提示

## 关键实现位置

- helix-view/src/editor.rs(Editor struct + set_error)
- helix-term/src/commands/typed.rs(yank_error 命令 + 注册)
- helix-term/tests/test/(integration,复用 plugin_reload 错误路径)
