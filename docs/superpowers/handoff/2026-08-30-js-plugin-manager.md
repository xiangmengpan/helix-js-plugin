# Handoff — 插件管理器批次完成(2026-08-30)

## 完成的工作

3 个 commit:

1. `5a91bc1c0` + `8cb4e8aaa` — 纯函数层:`plugin_manager.rs`(manifest 读写容错、install_target、install_copy 复制/回滚、remove_files、remove_orphan)+ 单测
2. `3e8627f2b` — 增强 `:plugin install/remove/status`:install(目录/单文件、manifest 记录、同名报错、失败回滚、装后 reload)、remove(manifest 条目 + files 删除 + 兼容旧安装)、status(manifest 计数后缀)
3. `aade9661b` + `f24a6392b` — 修复:装后加载新插件(reload 后 load,单文件/目录 index.js)+ remove_orphan 兼容 features 孤儿 + 拒绝 "." / "features" 防删光

规格:`docs/superpowers/specs/2026-08-30-js-plugin-manager-design.md`(修正版:增强现有 :plugin 子命令,不新建命令);计划:`docs/superpowers/plans/2026-08-30-js-plugin-manager.md`

## 验证状态

- 任务 1 审查 pass + 修复(Critical:单文件父目录/回滚契约);任务 2 审查 pass + 2 轮修复(装后加载/孤儿 remove/容器目录拒绝)
- helix-term --lib 94 passed、integration plugin_manager 2 passed、build/fmt/clippy 干净
- 装后可执行无自动化测试(config_dir 真实不可控),靠单测 + 手动验证

## 手动验证

```bash
mkdir -p /tmp/plugtest && echo 'helix.register_command("pt", () => helix.echo("pt-ok"));' > /tmp/plugtest/pt.js
# hx 里:
:plugin install /tmp/plugtest/pt.js   # → installed 'pt.js', reloading... + 加载
:pt                                    # → pt-ok
:plugin status                         # → N plugins loaded from ..., M installed in manifest
:plugin remove pt.js                   # → removed; update init.js if it loads it
```

## 已知边界

- install 同名已装 → 报错,先 remove(整删回滚的前提)
- remove 无 manifest 旧安装 → 删 plugins/<name> + features/<name> 双位置
- 目录插件入口约定 features/<name>/index.js;纯 lib 无入口跳过加载
- 不写 init.js(装完即用靠 install 主动加载;重启后需 init.js load)
- 二期:git 源、update、依赖自动解析、版本 pin、JS API

## 备注

- 执行期间用户并行推进多 server code_actions 批次(fmt 漂移在 picker.rs,未碰)
- 交接文档由控制者补写
