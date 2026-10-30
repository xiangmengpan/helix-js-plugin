#!/bin/sh
# 假 ar —— 与 fake-cc.sh 配套：`ar crs libfoo.a obj…` 造一个空归档即可（check 不链接）。
set -u
for a in "$@"; do
    case "$a" in
        *.a | *.lib)
            : > "$a"
            break
            ;;
    esac
done
exit 0
