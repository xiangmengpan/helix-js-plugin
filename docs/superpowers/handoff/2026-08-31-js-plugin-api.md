# Handoff — 插件 JS API 批次完成 + 二期全部完成(2026-08-31)

## 完成的工作(本批)

1. `e934287e5` — helix-js:`helix.plugin.install/update/remove` 参数校验 + `UiRequest::PluginOp` + 单测
2. `f7a6c4449` — term:`plugin_op`/`reload_plugins`/`load_installed_entry` 改收 `&mut Editor`(命令与 JS 共用);apply_ui_requests 的 PluginOp 分支(dispatch_blocking + Err set_error);plugin-api 文档;integration(JS remove 未安装报错)

规格:`docs/superpowers/specs/2026-08-31-js-plugin-api-design.md`;计划:`docs/superpowers/plans/2026-08-31-js-plugin-api.md`

## 二期全部完成(4 批)

| 批 | 功能 | commits |
|---|---|---|
| 1 | git 源 install(clone 到 vendor/ + manifest commit) | 996a82ebf..f529595e9 |
| 2 | update/pin/unpin(fetch 对比 ff-pull 失败保留旧版) | 7bf556385..02e80b78a |
| 3 | 依赖解析(plugin.json 递归 + 循环/深度检测) | c030dc145..7e55da6ea |
| 4 | JS API(helix.plugin.install/update/remove) | e934287e5..f7a6c4449 |

## 验证状态

- helix-js 94 passed、helix-term --lib 102 passed、integration plugin_manager 5 passed、build/fmt clean(clippy 仅既有 text_annotations + 用户 input.rs 1 个)
- 注意:中途 helix.plugin 从函数改函数对象(可调用 + 属性)导致 24 测试短暂失败,已修(94 恢复)

## 二期使用总览

```bash
:plugin install <path|git-url>   # 本地/git,manifest 记录,依赖解析,装后加载
:plugin update [name|all]        # git pull(ff-only),pinned 跳过,失败保留旧版
:plugin pin/unpin <name>         # 锁定/解锁
:plugin remove <name>            # 删 manifest + 文件(兼容旧安装/孤儿)
:plugin status                   # loaded/installed/git/pinned 计数
```
```js
helix.plugin.install(path_or_url);   // JS 侧镜像(结果走状态栏)
helix.plugin.update();
helix.plugin.remove(name);
```

## 已知边界(累计)

- 插件声明:加载时 `helix.plugin(name, {deps})`(文件 key);包依赖:git 仓库根 `plugin.json`(deps: [{name, git}])
- 依赖装完不自动加载(本体 index.js 加载)
- 真实 git 操作(网络)无自动化——手动验证
- update 非快进(本地有提交)报错保留旧版

## 备注

- 用户并行批次:装饰 slice_range(application.rs 测试,曾与我 stash 冲突,已恢复用户版本)、plugin_paste.rs(新测试,未提交在工作区)、input.rs(mut warning)
- 工作区未提交:helix-js/src/picker.rs、integration.rs、plugin_paste.rs(用户)
- stash 状态:stash@{0}(已 pop 的旧 WIP,可 drop)、stash@{1}(更早 panel focus WIP)
