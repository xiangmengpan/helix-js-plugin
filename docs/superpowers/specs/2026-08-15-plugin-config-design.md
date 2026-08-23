# 设计:插件配置读取(get_config)

日期:2026-08-15
状态:草案(待审核)

## 1. 动机

插件不能读 `config.toml` 或自己的配置,参数写死在插件代码里。受益:
- 插件参数化:`helix.get_config("filetree.show_hidden")` 而非写死
- 用户改 config.toml 即生效,不改插件代码
- 插件自带配置约定(如 `[plugins.xxx]` 段)

## 2. API

```js
// 读 config.toml 中 [plugins] 段的值
helix.get_config("filetree.hidden_default")
// → 值(字符串/数字/布尔)或 undefined

// 读取当前 Editor 配置的通用键(如 "auto-info" → 布尔)
helix.get_config("auto-info")
```

## 3. 设计要点

### 3.1 配置段约定

在 `config.toml` 加 `[plugins]` 段(或 `[plugin.xxx]` 每插件一段):

```toml
[plugins]
filetree_hidden_default = true
terminal_shell = "/bin/zsh"
```

命名:扁平 `key = value`(避免 TOML 嵌套解析复杂度);插件侧 `helix.get_config("filetree_hidden_default")`。

### 3.2 通用配置只读

`helix.get_config("auto-info")` 读 EditorConfig 现有字段(白名单,防止插件改配置):
- 白名单字段:`auto-info`/`auto-save`/`mouse`/`scrolloff` 等(Helix Config 结构字段)
- 实现:helix-term 序列化白名单字段 → JSON 缓存(仿 buffers 每帧?或惰性——config 变更少,事件时更新即可)

## 4. 边界

- **只读**:get_config 只读;配置修改走 `:config-reload`
- **白名单**:通用字段白名单(不暴露内部/敏感);plugins 段任意读
- **类型**:返回字符串/数字/布尔(TOML 原生类型);复杂值(数组)暂不支持

## 5. 待决

1. `[plugins]` 段命名与层级(扁平 vs `[plugin.filetree]` 分段)
2. 通用配置是否需要(config.toml 里 helix.get_config 与原生 config 访问重复?插件主要要 plugins 段)
3. 配置变更事件(`on("config-change")` 通知插件 reload)是否本期做

## 6. 规模

- helix-term:config.toml 的 [plugins] 解析 + 白名单字段序列化 + helix-js get_config(约 1-2 任务)
- 测试(约 0.5 任务)
