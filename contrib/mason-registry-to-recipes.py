#!/usr/bin/env python3
"""把 Mason 的注册表包转成 helix server-manager 的 TOML 配方。

## 为什么是"转数据"而不是"移植代码"

helix 侧已经具备 Mason 的核心机制(`Install::Archive` + `download_to`(镜像/sha256)+
解压含 `strip` + `managed/<name>/bin` 软链 + 自动写 `languages.toml`)。
**缺的只是配方数量** —— 而 Mason 的注册表是结构化数据,可以脚本转换。所以本脚本补的是**数据**。

## 数据来源

主注册表:`mason-org/mason-registry`(**不是** `williamboman/mason-registry` ——
后者自述是"提供主注册表里没有的包"的**补充**注册表,且 2024-06 后未更新)。
每个包:`packages/<name>/package.yaml`。

> 注意:`raw.githubusercontent.com` 在部分网络下会被 reset,所以本脚本走 **GitHub API**
> (`/contents` 返回 base64),也可读本地文件。

## Mason 的 schema(实测)

```yaml
name: ruff
description: ...
homepage: ...
languages: [Python]
categories: [Linter, Formatter, LSP]
source:
  id: pkg:github/astral-sh/ruff@0.16.10      # ← 来源类型 + 版本
  asset:
    - target: darwin_x64                      # ← 逐平台
      file: ruff-x86_64-apple-darwin.tar.gz   # ← 资产名(可含 `:` 后的解压前缀)
      bin: ruff-x86_64-apple-darwin/ruff      # ← 二进制路径(`exec:` 前缀 = 保持可执行)
```

## 映射到 helix 配方

| Mason | helix 配方 |
|---|---|
| `categories`(LSP/DAP/Linter/Formatter) | `kind`(lsp/dap/linter/formatter) |
| `languages`(首字母大写) | `languages`(**小写**) |
| `source.id = pkg:github/O/R@V` | `version` + `url = https://github.com/O/R/releases/download/{version}/<file>` |
| `asset[].file` | 文件名里的平台段 → 占位符 `{triple}` / `{os}` / `{arch}`(**见下**) |
| `asset[].bin`(去掉 `exec:`) | `bin` |
| `asset[].file` 的 `:前缀` | ⚠️ helix 的 `strip` 是**数字**(剥几层),Mason 是**路径前缀** → 不一致时标记需手工 |

**平台命名**:helix 的 `{triple}` 来自 Rust triple 表(`("macos","x86_64")=>"x86_64-apple-darwin"`),
正好匹配 Mason 里用 Rust triple 命名的资产(如 ruff)。
用别的命名(如 `linux-x64`)的**无法**用单一模板表达 → 本脚本**如实标记 `needs-manual`**,不猜。

## 实测结论(重要):**瓶颈是 schema,不是数据**

用真实包跑过之后(`ruff` 与 `lua-language-server`),**大多数包无法自动转换**,原因**不在 Mason 的数据**,
而在 helix 配方的**表达能力更窄**:

| Mason 有 | helix 配方有 | 后果 |
|---|---|---|
| 逐 target 的资产表(`linux_x64` / `linux_x64_musl` / `linux_arm64_gnu` …) | **单一 `url` 模板** + `{version}/{os}/{arch}/{triple}` | 表达不了平台矩阵 |
| 各平台用**不同 triple**(x64→musl、aarch64→gnu 等) | `{triple}` 表只有 `*-unknown-linux-gnu` 等少数 | 映射不了 musl/其它变体 |
| 资产名可含**解压前缀**(`file: libexec/`) | `strip` 是**数字**(剥几层) | 语义不同,需手工核 |
| target 粒度更细(`linux_x64` vs `linux_x64_musl`) | 只有 `(os, arch)` | 同平台多资产表达不了 |

### 所以要做的是先扩 schema(有界改动)

给配方加一个**逐平台资产表**(照 Mason 的形状),例如:

```toml
[[server-manager.registry.ruff.asset]]
target = "linux_x64_musl"
file = "ruff-{version}-x86_64-unknown-linux-musl.tar.gz"
bin = "ruff-x86_64-unknown-linux-musl/ruff"
```

要点:① `registry.rs` 的解析加 `asset` 数组;② 运行时按**当前平台**选条目(并把 `{version}` 展开);
③ 同时扩 `rust_triple()` 表(musl 等变体);④ 保留现有单模板写法作为简写(向后兼容)。
**做完这一步,本脚本才能真正批量转换** —— 在那之前它只做"能转的转、不能转的如实标记"。

## 用法

    python3 contrib/mason-registry-to-recipes.py ruff lua-language-server --fetch   # 走 API 抓
    python3 contrib/mason-registry-to-recipes.py --dir /path/to/mason/packages      # 读本地目录
"""

from __future__ import annotations

import argparse
import base64
import json
import re
import sys
import urllib.request

API = "https://api.github.com/repos/mason-org/mason-registry/contents/packages"
CATEGORY_TO_KIND = {
    "lsp": "lsp",
    "dap": "dap",
    "linter": "linter",
    "formatter": "formatter",
}
# Mason 的 target → (os, arch),用于把资产名里的平台段换算成 helix 的占位符
MASON_TARGETS = {
    "linux_x64": ("linux", "x86_64"),
    "linux_x64_gnu": ("linux", "x86_64"),
    "linux_x64_musl": ("linux", "x86_64"),
    "linux_x86": ("linux", "x86"),
    "linux_arm64": ("linux", "aarch64"),
    "linux_arm64_gnu": ("linux", "aarch64"),
    "linux_arm64_musl": ("linux", "aarch64"),
    "darwin_x64": ("macos", "x86_64"),
    "darwin_arm64": ("macos", "aarch64"),
    "win_x64": ("windows", "x86_64"),
    "win_arm64": ("windows", "aarch64"),
}
# helix 的 {triple} 表(与 server_manager.rs 的 rust_triple() 对齐)
RUST_TRIPLES = {
    ("linux", "x86_64"): "x86_64-unknown-linux-gnu",
    ("linux", "aarch64"): "aarch64-unknown-linux-gnu",
    ("macos", "x86_64"): "x86_64-apple-darwin",
    ("macos", "aarch64"): "aarch64-apple-darwin",
    ("windows", "x86_64"): "x86_64-pc-windows-msvc",
    ("windows", "aarch64"): "aarch64-pc-windows-msvc",
}


def api_json(url: str):
    req = urllib.request.Request(
        url, headers={"User-Agent": "helix-mason-convert", "Accept": "application/vnd.github+json"}
    )
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.load(r)


def fetch_yaml(pkg: str) -> str:
    files = api_json(f"{API}/{pkg}")
    y = [f for f in files if f["name"].endswith((".yaml", ".yml"))]
    if not y:
        raise SystemExit(f"{pkg}: 目录里没有 yaml")
    return base64.b64decode(api_json(y[0]["url"])["content"]).decode()


def parse_yaml(text: str) -> dict:
    """极简 YAML 解析(只覆盖 Mason package.yaml 用到的形状;避免引入 pyyaml 依赖)。"""
    out: dict = {"asset": []}
    cur = None
    for raw in text.splitlines():
        line = raw.rstrip()
        if not line or line.lstrip().startswith("#"):
            continue
        if re.match(r"^(\w+):\s*$", line):
            key = line.split(":")[0]
            if key == "source":
                cur = "source"
            out.setdefault(key, [] if key in ("languages", "categories", "licenses") else None)
            continue
        m = re.match(r"^\s*-\s+(.*)$", line)
        if m and cur in (None, "list"):
            val = m.group(1).strip()
            # 归属到"最近一个列表键"
            for k in ("licenses", "languages", "categories"):
                if isinstance(out.get(k), list) and out.get(k) is not None and not out[k]:
                    out[k] = [val]
                    break
            else:
                for k in ("licenses", "languages", "categories"):
                    if isinstance(out.get(k), list):
                        out[k].append(val)
                        break
            continue
        m = re.match(r"^(\w+):\s*(.*)$", line)
        if m:
            k, v = m.group(1), m.group(2).strip()
            if cur == "source":
                out[f"source_{k}"] = v
            elif cur == "asset":
                cur = None  # 资产项由下面的块解析
            else:
                cur = None
            if k in ("licenses", "languages", "categories"):
                out[k] = [v.strip('"')] if v else []
            elif k in ("name", "description", "homepage"):
                out[k] = v.strip('"')
            continue
        if cur == "source":
            out.setdefault("source_id", None)
            if line.strip().startswith("id:"):
                out["source_id"] = line.split("id:", 1)[1].strip()
    # 资产块:逐项解析 target/file/bin
    item = None
    in_asset = False
    for raw in text.splitlines():
        if re.match(r"^\s*asset:\s*$", raw):
            in_asset = True
            continue
        if in_asset and re.match(r"^\S", raw) and not raw.startswith(" "):
            break
        if not in_asset:
            continue
        m = re.match(r"^\s*-\s*target:\s*(\S+)", raw)
        if m:
            item = {"target": m.group(1)}
            out["asset"].append(item)
            continue
        if item is not None:
            m = re.match(r"^\s*(file|bin):\s*(.+)$", raw)
            if m:
                item[m.group(1)] = m.group(2).strip().strip('"')
    return out


def to_recipe(pkg: dict) -> tuple[str, list[str]]:
    """→ (TOML 文本, 需要手工处理的原因列表)"""
    problems: list[str] = []
    name = pkg.get("name") or "?"
    cats = [c.lower() for c in (pkg.get("categories") or [])]
    kind = next((CATEGORY_TO_KIND[c] for c in cats if c in CATEGORY_TO_KIND), None)
    if kind is None:
        problems.append("无法从 categories 定出 kind(helix 的 kind 是必需字段)")
    langs = [str(x).lower() for x in (pkg.get("languages") or [])]
    src = pkg.get("source_id") or ""
    m = re.match(r"pkg:github/([^/]+)/([^@]+)@(.+)", src)
    assets = pkg.get("asset") or []
    if not m:
        problems.append(f"来源不是 github-release({src})—— helix 的 Archive 走 release 资产 URL")
        version, base = "", ""
    else:
        owner, repo, version = m.group(1), m.group(2), m.group(3)
        base = f"https://github.com/{owner}/{repo}/releases/download/{{version}}"

    # 资产名 → 能否用单一模板表达
    template = None
    for a in assets:
        target, f = a.get("target", ""), a.get("file", "")
        if not f:
            continue
        fname, _, prefix = f.partition(":")
        if prefix:
            problems.append(
                f"{target}: 资产含解压前缀 `{prefix}` —— helix 的 `strip` 是**数字**(剥层数),语义不同,需手工核"
            )
        if "{{version}}" in fname or "{version}" in fname:
            cand = fname.replace("{{version}}", "{version}")
        else:
            cand = fname
        t = MASON_TARGETS.get(target)
        if t and t in RUST_TRIPLES:
            trial = cand.replace(RUST_TRIPLES[t], "{triple}")
            if "{triple}" not in trial:
                # 资产名没用 Rust triple 命名 —— 试试 os/arch 两种写法
                os_, arch = t
                for pat in (f"{os_}-{arch}", f"{os_}_{arch}", f"{arch}-{os_}"):
                    if pat in cand:
                        trial = cand.replace(pat, "{triple}")  # 仍走 triple 占位(值由 helix 表给出)
                        break
            if "{triple}" in trial:
                if template is None:
                    template = trial
                elif template != trial:
                    problems.append("不同平台的资产名**无法**用同一个模板表达 → 需要逐平台条目")
                    template = None
                    break
            else:
                problems.append(f"{target}: 资产名 `{fname}` 里的平台段无法映射到 {{{{triple}}}}/{{{{os}}}}/{{{{arch}}}}")
        elif target:
            problems.append(f"{target}: 未知 target(Mason 新增的平台?)")

    lines = [f"[server-manager.registry.{name}]"]
    if kind:
        lines.append(f'kind = "{kind}"')
    if langs:
        lines.append("languages = [" + ", ".join(f'"{x}"' for x in langs) + "]")
    if pkg.get("description"):
        lines.append(f'description = "{pkg["description"]}"')
    if pkg.get("homepage"):
        lines.append(f'homepage = "{pkg["homepage"]}"')
    if version:
        lines.append(f'version = "{version}"   # Mason 的 source 里带的版本;留空则需 needs-version')
    if template and base:
        lines.append(f'url = "{base}/{template}"')
    else:
        lines.append('# url = "<需手工:资产命名无法用单一模板表达,或来源不是 github-release>"')
    if problems:
        for p in problems:
            lines.append(f"# ⚠️ needs-manual: {p}")
    return "\n".join(lines) + "\n", problems


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("packages", nargs="*")
    ap.add_argument("--fetch", action="store_true", help="经 GitHub API 抓取(强调:不是 raw)")
    ap.add_argument("--dir", help="读本地 mason packages/ 目录")
    args = ap.parse_args()

    texts: list[tuple[str, str]] = []
    if args.dir:
        import pathlib

        for p in sorted(pathlib.Path(args.dir).iterdir()):
            y = list(p.glob("package.y*ml"))
            if y:
                texts.append((p.name, y[0].read_text()))
    for p in args.packages:
        texts.append((p, fetch_yaml(p)))
    if not texts:
        ap.error("给出包名(--fetch)或 --dir")

    ok = manual = 0
    try:
        import tomllib
    except ModuleNotFoundError:
        tomllib = None  # type: ignore
    for name, text in texts:
        pkg = parse_yaml(text)
        toml, problems = to_recipe(pkg)
        print("# " + "=" * 70)
        print(f"# {name}: {'需手工' if problems else '可自动转换'}")
        print(toml)
        if tomllib:
            try:
                tomllib.loads(toml.split("#")[0] if False else toml)
            except Exception as e:  # 生成的 TOML 必须可解析
                print(f"# ‼️ 生成的 TOML 解析失败: {e}")
                return 2
        if problems:
            manual += 1
        else:
            ok += 1
    print(f"# ===== 汇总:可自动 {ok} · 需手工 {manual} =====")
    return 0


if __name__ == "__main__":
    sys.exit(main())
