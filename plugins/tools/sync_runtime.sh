#!/usr/bin/env bash
# 把**源**(仓库 plugins/,git 跟踪 ✓)镜像到运行时副本。
#
# 由来:`<runtime>/plugins` 与 `~/.config/helix/plugins` 都是**手工拷贝**的副本 ✗
#   ⇒ 改了源却忘同步 ⇒ 插件行为"没变化"(本会话踩过:picker-ui 的 ② 改动 ✗)。
#   一次编码事故的教训:凡是"靠人记得"的步骤,最后一定被忘掉 ⇒ 固化成脚本 ✓
#
# 覆盖范围(2026-10 补):**不只 `*.js`**。原先只遍历 `$src/*/*.js` 与 `$src/*.js`,
#   于是 `helix.d.ts` / `README.md` / `jsconfig.json` **从来没被同步过** ✗ ——
#   而 `helix.d.ts` 正是最常改、且"改了不生效也看不出来"的那个(类型只影响补全)。
#   现在同样覆盖根级 `.json` / `.md` / `.d.ts` 与插件目录内的 `.json` / `.md`。
#
# 语义(有意保守):**只更新已存在的目标文件,不新建**。首次安装请用
#   `contrib/install-plugins.sh`(它整棵树拷过去,会建目录/新文件)。
#   为什么要分开:同步脚本会跑在用户已经配置好的目录上,不该往里塞新东西。
set -euo pipefail
src="$(cd "$(dirname "$0")/.." && pwd)"
runtime="${HELIX_RUNTIME:-$HOME/.config/helix/runtime}/plugins"
user="${HELIX_CONFIG:-$HOME/.config/helix}/plugins"
for dst in "$runtime" "$user"; do
  [ -d "$dst" ] || { echo "  跳过(不存在): $dst"; continue; }
  n=0
  for f in "$src"/*/*.js "$src"/*/*.json "$src"/*/*.md \
           "$src"/*/tests/*.js \
           "$src"/*.js "$src"/*.json "$src"/*.md "$src"/*.d.ts; do
    [ -e "$f" ] || continue
    rel="${f#"$src"/}"
    case "$rel" in tools/*|examples/*) continue;; esac   # 开发用,不装 ✓
    if [ -e "$dst/$rel" ]; then
      cmp -s "$f" "$dst/$rel" || { cp "$f" "$dst/$rel"; n=$((n+1)); echo "  更新 $rel → $dst"; }
    elif [ -e "$dst/${rel%%/*}" ]; then
      # 源里新增的**插件目录**不代建(那是安装的活);但已存在的插件目录下的新文件
      # (典型:`<plugin>/tests/*.js`)必须镜像 —— `cargo xtask api-check` 会比对它们。
      mkdir -p "$(dirname "$dst/$rel")"
      cp "$f" "$dst/$rel"; n=$((n+1)); echo "  新增 $rel → $dst"
    fi
  done
  echo "  $dst: 更新 $n 个文件"
done
