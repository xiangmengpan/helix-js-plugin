#!/bin/sh
# 假 C 编译器 —— 只为 `cargo check/clippy --target <foreign>` 服务。
#
# 为什么需要它：整仓的 Windows 交叉 check 会撞上两个**仍是 C 依赖**的 build script
# （`zstd-sys`、`tree-house-bindings`），它们要为目标平台找一个 C 编译器。
# 而 `check`/`clippy` **不链接**，产物不会被任何东西消费 ⇒ 让假编译器"造出空产物"就够。
#
# 用法（见 .github/workflows/build.yml 的 Windows compile surface 步骤）：
#   HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 \
#   CC_x86_64_pc_windows_gnu=contrib/ci/fake-cc.sh \
#   AR_x86_64_pc_windows_gnu=contrib/ci/fake-ar.sh \
#   cargo clippy --workspace --all-targets --target x86_64-pc-windows-gnu -- -D warnings
#
# `HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1` 是另一半：`helix-term/build.rs` 会为**所有**语言
# 编译 tree-sitter 语法（几十个 C 文件），交叉编译时那才是大头，且与"类型检查"无关。
#
# 口径提醒：这**只**证明"编得过"。运行期正确性（pty/进程/路径）仍需 Windows runner/实机。
set -u
out=""
prev=""
for a in "$@"; do
    if [ "$prev" = "-o" ]; then out="$a"; fi
    prev="$a"
done
# 有 -o 就是"编译一个目标文件"；没有就是 cc-rs 的探测调用（--version / -E 探针）⇒ 直接成功
[ -n "$out" ] && : > "$out"
exit 0
