#!/bin/sh
# 把仓库的 plugins/ 装进 helix 的 **runtime 目录** —— 即"随软件分发的内置插件"层。
#
# 两个根的分工(docs/plugin-layout.md §3.5):
#   <runtime>/plugins/          ← 内置层(本脚本写入)
#   ~/.config/helix/plugins/    ← 用户层,**同名目录整体覆盖内置层**
# 加载顺序:内置在前、用户殿后 → 用户覆盖内置。
#
# 用法:
#   sh contrib/install-plugins.sh                 # 装到默认 runtime 目录
#   sh contrib/install-plugins.sh /path/to/rt     # 装到指定 runtime 目录(会加 /plugins)
#   DRY_RUN=1 sh contrib/install-plugins.sh       # 只打印将要做什么
#
# 说明:这是"内置插件"的**分发**步骤。此前只有查找路径、没有分发,
# 所以全新安装的机器上 `<runtime>/plugins` 是空的,内置插件并不存在。
set -eu

src="$(cd "$(dirname "$0")/.." && pwd)/plugins"
if [ "$#" -ge 1 ] && [ -n "${1:-}" ]; then
    root="$1"
else
    root="${HELIX_RUNTIME:-$HOME/.config/helix/runtime}"
fi
dest="$root/plugins"

if [ ! -d "$src" ]; then
    echo "找不到源目录: $src" >&2
    exit 1
fi

echo "源   : $src"
echo "目标 : $dest"
if [ -n "${DRY_RUN:-}" ]; then
    echo "(DRY_RUN:不做任何写入)"
    find "$src" -type f | sed "s|^$src/|  将安装: |"
    exit 0
fi

mkdir -p "$dest"
# 整棵树拷过去:插件目录 + lib/(共享) + examples/(示例) + init.js + helix.d.ts
cp -R "$src"/. "$dest"/

echo "已安装文件数: $(find "$dest" -type f | wc -l)"
echo "插件目录: $(find "$dest" -maxdepth 1 -mindepth 1 -type d -printf '%f ' 2>/dev/null || ls -d "$dest"/*/ 2>/dev/null | xargs -n1 basename | tr '\n' ' ')"
echo
echo "提示:用户层 ~/.config/helix/plugins/ 里的同名目录会**整体覆盖**这里的内置版本。"
