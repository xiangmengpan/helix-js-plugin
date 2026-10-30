#!/bin/sh
# 薄封装：逻辑在 `cargo xtask sync-runtime`（纯 Rust ⇒ Windows 也能走）。
# 参数与环境变量**原样转发**：--runtime <dir> · --config <dir> · --dry-run ·
#   HELIX_RUNTIME · HELIX_CONFIG · DRY_RUN。语义（只更新已有副本、不新建）见 xtask 里的说明。
set -e
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
exec cargo xtask sync-runtime "$@"
