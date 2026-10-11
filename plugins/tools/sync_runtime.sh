#!/usr/bin/env bash
# 把**源**(仓库 plugins/,git 跟踪 ✓)镜像到运行时副本。
#
# 由来:`<runtime>/plugins` 与 `~/.config/helix/plugins` 都是**手工拷贝**的副本 ✗
#   ⇒ 改了源却忘同步 ⇒ 插件行为"没变化"(本会话踩过:picker-ui 的 ② 改动 ✗)。
#   一次编码事故的教训:凡是"靠人记得"的步骤,最后一定被忘掉 ⇒ 固化成脚本 ✓
set -euo pipefail
src="$(cd "$(dirname "$0")/.." && pwd)"
runtime="${HELIX_RUNTIME:-$HOME/.config/helix/runtime}/plugins"
user="${HELIX_CONFIG:-$HOME/.config/helix}/plugins"
for dst in "$runtime" "$user"; do
  [ -d "$dst" ] || { echo "  跳过(不存在): $dst"; continue; }
  n=0
  for f in "$src"/*/*.js "$src"/*.js; do
    [ -e "$f" ] || continue
    rel="${f#"$src"/}"
    case "$rel" in tools/*|examples/*) continue;; esac   # 开发用,不装 ✓
    if [ -e "$dst/$rel" ]; then
      cmp -s "$f" "$dst/$rel" || { cp "$f" "$dst/$rel"; n=$((n+1)); echo "  更新 $rel → $dst"; }
    fi
  done
  echo "  $dst: 更新 $n 个文件"
done
