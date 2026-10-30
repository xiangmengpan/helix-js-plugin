#!/bin/sh
# 薄封装：逻辑在 `cargo xtask install-plugins`（纯 Rust ⇒ Windows 也能走）。
# 保留这个路径是因为 README / docs 多处引用它；参数与环境变量**原样转发**：
#   位置参数 [<runtime-dir>] · --runtime <dir> · --dry-run · HELIX_RUNTIME · DRY_RUN
#
# 实现与等价性验证（与旧 46 行脚本逐字节对比过）见
# docs/superpowers/specs/2026-10-30-cross-platform-design.md §5。
set -e
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
exec cargo xtask install-plugins "$@"
