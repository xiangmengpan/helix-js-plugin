#!/usr/bin/env python3
"""把 `:plugin` 管理器的实现从 Rust 核心迁到 JS 插件(最后一刀的**机械化部分**)。

背景与完整清单见 `docs/plugin-layout.md` §12.2 / §16。
本脚本只做**删除与改名**,不含测试改造与文档 —— 那些需要人读一遍再改(清单里已列)。

## 为什么要有这个脚本

这一刀**不能分步**(§15 已实证:JS 在 Rust 的 `TypableCommand` 存在时**永远接管不到**
`:plugin`,因为 `run_plugin_command` 只是"typed 命令找不到时"的回退)。
所以它必须在**一次不被打断的执行**里做完 —— 而人力逐处删除又要反复定位。
把"已逐处验证过"的删除逻辑固化成脚本,是让下一次执行变便宜的唯一办法。

## 设计(全部来自实测)

| 目标 | 终止符 / 形状 |
|---|---|
| 顶层 `fn` | **裸 `}`** |
| `TypableCommand` 条目 | **`    },`** |
| `match` 臂 | **以下一个臂为界**(此处是 `UiRequest::ServerOp`) |
| `lib.rs` 注册块 | `.function(` … 单独一行的 `)` |
| `PluginOp` 变体 | 到 `},`,并**连带其上方的 `///` 注释** |

## 安全

- **默认 `--dry-run`**:只校验 + 打印计划,**不写任何文件** ✓
- 只有显式 `--apply` 才落盘;任何校验失败 → **一个文件都不写** ✓
- 与本次会话一贯的"先验后写"一致(该纪律已挡下 7 次脚本错误,零次损坏)
"""

import sys
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
APPLY = "--apply" in sys.argv

plan: dict[Path, str] = {}
report: list[tuple[str, int]] = []


def read(rel: str) -> list[str]:
    return (ROOT / rel).read_text().split("\n")


def top_fn(L: list[str], name: str, maxlen: int = 400) -> int:
    """删一个顶层函数(终止符 = 裸 `}`)。返回删除的行数。"""
    st = [
        k
        for k, line in enumerate(L)
        if line.startswith(f"fn {name}(")
        or line.startswith(f"pub fn {name}(")
        or line.startswith(f"pub(crate) fn {name}(")
    ]
    assert len(st) == 1, f"{name}: 期望 1 个起点,实得 {len(st)}"
    i = st[0]
    j = next(k for k in range(i, len(L)) if L[k] == "}")
    n = j - i + 1
    assert 3 <= n <= maxlen, f"{name}: 跨度 {n} 行,超出 {maxlen}"
    del L[i : j + 1]
    return n


# ── ① typed.rs:命令条目 + fn plugin + fn plugin_op + PluginOp match 臂 ──
p = "helix-term/src/commands/typed.rs"
L = read(p)
report.append((f"{p} fn plugin", top_fn(L, "plugin", 40)))
report.append((f"{p} fn plugin_op", top_fn(L, "plugin_op", 400)))

i = next(k for k, line in enumerate(L) if line.strip() == 'name: "plugin",')
st = next(k for k in range(i, -1, -1) if L[k] == "    TypableCommand {")
en = next(k for k in range(i, len(L)) if L[k] == "    },")
assert en - st + 1 <= 14, f"TypableCommand 条目跨度 {en - st + 1}"
report.append((f"{p} TypableCommand 条目", en - st + 1))
del L[st : en + 1]

i = next(k for k, line in enumerate(L) if "UiRequest::PluginOp" in line)
j = next(k for k in range(i + 1, len(L)) if "UiRequest::ServerOp" in L[k])
assert 3 <= j - i <= 12, f"match 臂跨度 {j - i}"
report.append((f"{p} PluginOp match 臂", j - i))
del L[i:j]
plan[ROOT / p] = "\n".join(L)

# ── ② helix-js:三个 API 函数 + push_plugin_op ──
p = "helix-js/src/commands.rs"
L = read(p)
for name in ("js_plugin_install", "js_plugin_update", "js_plugin_remove", "push_plugin_op"):
    report.append((f"{p} {name}", top_fn(L, name, 40)))
plan[ROOT / p] = "\n".join(L)

# ── ③ helix-js:UiRequest::PluginOp 变体(连带其上方注释) ──
p = "helix-js/src/types.rs"
L = read(p)
cand = [k for k, line in enumerate(L) if line.strip() == "PluginOp {"]
assert len(cand) == 1, f"PluginOp 变体: 期望 1 处,实得 {len(cand)}"
i = cand[0]
en = next(k for k in range(i, len(L)) if L[k].strip() == "},")
st = i
while st - 1 >= 0 and L[st - 1].strip().startswith("///"):
    st -= 1
report.append((f"{p} PluginOp 变体", en - st + 1))
del L[st : en + 1]
plan[ROOT / p] = "\n".join(L)

# ── ④ helix-js:三处注册 ──
p = "helix-js/src/lib.rs"
L = read(p)
# 真实形状(**实测**):这三个不是 `.function(…)` 注册,而是 `helix.plugin` **函数对象上的属性**,
# 由一个 `for (name, f) in [ … ]` 循环设置:
#     for (name, f) in [
#         ("install", commands::js_plugin_install as …NativeFunctionPointer),
#         ("update", …), ("remove", …),
#     ] { … }
# ⇒ 删掉这三项即删掉 Rust 侧的 `helix.plugin.install/update/remove`
#   —— 而 JS 兼容层会**覆盖同名属性**(§13 探测已证可覆盖 ✓),所以公开 API 不断 ✓
for name in ("js_plugin_install", "js_plugin_update", "js_plugin_remove"):
    i = next(k for k, line in enumerate(L) if f"commands::{name}" in line)
    st = next(k for k in range(i, -1, -1) if L[k].strip() == "(")
    en = next(k for k in range(i, len(L)) if L[k].strip() == "),")
    assert 3 <= en - st + 1 <= 6, f"数组项跨度异常: {en - st + 1}"
    report.append((f"{p} 数组项 {name}", en - st + 1))
    del L[st : en + 1]
plan[ROOT / p] = "\n".join(L)

# ── ⑤ helix-term/src/commands.rs:mod 声明 ──
p = "helix-term/src/commands.rs"
s = (ROOT / p).read_text()
decl = "pub(crate) mod plugin_manager;\n"
assert s.count(decl) == 1, "mod plugin_manager; 声明数不为 1"
report.append((f"{p} mod plugin_manager;", 1))
plan[ROOT / p] = s.replace(decl, "", 1)

# ── 打印计划 ──
print(f"{'APPLY' if APPLY else 'DRY-RUN'}: 计划删除/改写 {len(plan)} 个文件")
total = 0
for name, n in report:
    print(f"  - {name}: {n} 行")
    total += n
print(f"  另: git rm helix-term/src/commands/plugin_manager.rs(639 行)")
print(f"  合计约 {total + 639} 行")

if not APPLY:
    print("\n(未写入任何文件。确认无误后加 --apply 执行;执行后请立刻: )")
    print("  cargo check -p helix-term --all-targets")
    print("  再按 §16 的第 ①/⑥/⑦ 项改 plugin.js 名字、测试与文档")
    sys.exit(0)

for path, content in plan.items():
    path.write_text(content)
print("✅ 已落盘。下一步:git rm 模块文件 + cargo check + 按 §16 改 plugin.js/测试/文档")
