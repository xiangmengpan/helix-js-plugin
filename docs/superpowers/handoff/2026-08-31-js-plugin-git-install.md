# Handoff — 插件 git 源 install 批次完成(2026-08-31)

## 完成的工作

4 个 commit(内联执行——子代理两次探索超时,控制者直接实现):

1. `996a82ebf` — git 源纯函数:`is_git_url`/`name_from_url`/`clone_to_vendor`/`git_head_commit` + ManifestEntry 扩展(commit/pinned,serde default)
2. `ce108a964` — 测试修正(缺省字段断言改手写 legacy JSON,修 task-1 审查 Minor)
3. `fa7f4cdcb` — install 分支接线:git-url 识别 → clone 到 `plugins/vendor/<name>` → manifest(commit/pinned)→ reload → 装后加载 index.js;本地分支报错文案改 "invalid path or git url";提取 `load_installed_entry` 共用
4. `0e7ab1a7d` + fmt 2 commits — 跨源同名拒绝(manifest contains_key)+ fmt

规格:`docs/superpowers/specs/2026-08-31-js-plugin-git-install-design.md`;计划:`docs/superpowers/plans/2026-08-31-js-plugin-git-install.md`

## 验证状态

- task-1 审查 pass(1 Minor 已修);task-2 审查 pass(3 Minor 计划继承,1 个已修:跨源同名)
- helix-term --lib plugin_manager 9 passed、integration plugin_manager 3 passed、build clean
- 真实 git clone 需网络,无自动化——手动验证

## 手动验证

```bash
hx
:plugin install https://github.com/foo/bar.git   # clone 到 vendor/bar + manifest 记 commit + 装后加载 index.js
:plugin status                                    # manifest 计数含 git 源
:plugin remove bar                                # 删 vendor/bar + manifest 条目
```

## 已知边界(Minor 继承)

- `./x.git` 本地路径误路由到 clone(本地路径含 .git 后缀)——edge case,未修
- scp 无斜杠 url(`git@host:name`)的 name 提取含 `:`,Windows 上非法——当前 Linux 可接受
- commit=None(rev-parse 失败)无 warn——cosmetic
- git 源 update/pin/依赖解析/JS API——下一批

## 备注

- 执行期间用户并行推进装饰/粘贴规格(2a85668d6)
- 子代理在本批次连续 2 次探索超时(任务 1、以及派给任务 1 的实现者)——本批改内联执行;下批若再派子代理,简报已足够小,超时模式需要关注(可能环境问题)
