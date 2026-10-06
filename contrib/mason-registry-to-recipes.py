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

### 现状:**schema 扩好了**(此段保留作决策记录)

`[[…asset]]` 逐平台资产表已落地并有测试覆盖(解析 / 平台选择 / 消费点 / **端到端安装**)。
所以本脚本下一步可以真正批量转换;剩下的只有"`rust_triple()` 变体"与"libc 探测"这类
**锦上添花**(逐平台条目可以自己写精确文件名,本就不必依赖 `{triple}`)。

<details><summary>当时的改动方案(已执行,保留备查)</summary>

全部在 `helix-term/src/commands/server_manager.rs`(共 7 处生产点 + 3 处测试点):

| # | 位置 | 改什么 |
|---|---|---|
| 1 | `:898-910` `enum Install::Archive` | 加 `assets: Vec<ArchiveAsset>`(`{target, url, bin_rel}`);**空 = 沿用现有单模板简写**(向后兼容) |
| 2 | `:1058` 一带(`tbl.get("url")` 附近) | 解析 `[[server-manager.registry.<n>.asset]]` 数组 |
| 3 | `:1111-1122` 构造 `Install::Archive` | 带上 `assets` |
| 4 | `:1207`(安装时构造 `url`) | **按当前平台选 asset**(无匹配则回落单模板);命中后用条目自己的 `url`/`bin_rel` |
| 5 | `:493` `has_source` · `:930` `bin_rel` · `:948` `is_empty` · `:956` `needs_version` | 逐个处理新字段 —— **漏掉任一处,该工具在 arsenal 列表里就会显示错**(这是最易漏的部分) |
| 6 | `:1192` `expand_template` + `rust_triple()` | 扩 musl 等变体;`{triple}` 表需与 Mason 的命名对齐 |
| 7 | 测试 `:2395` / `:2531` / `:2556` | 补"逐平台资产"的解析 + 平台选择测试 |

**为什么没有直接做**:它同时动 **schema 与安装路径**,且消费点分散(第 5 项漏一处就静默显示错)。
这类改动必须**端到端验证**,预算不足时开它容易留下"半改的 schema" —— 那比没做更糟。

</details>

### 目标形状(照 Mason)

给配方加一个**逐平台资产表**,例如:

```toml
[[server-manager.registry.ruff.asset]]
target = "linux_x64_musl"
file = "ruff-{version}-x86_64-unknown-linux-musl.tar.gz"
bin = "ruff-x86_64-unknown-linux-musl/ruff"
```

要点:① `registry.rs` 的解析加 `asset` 数组;② 运行时按**当前平台**选条目(并把 `{version}` 展开);
③ 同时扩 `rust_triple()` 表(musl 等变体);④ 保留现有单模板写法作为简写(向后兼容)。
**做完这一步,本脚本才能真正批量转换** —— 在那之前它只做"能转的转、不能转的如实标记"。

## ⚠️ 批量跑之前先读这个(实测教训)

- **不要在重定向到文件时靠 stdout 看进度**:Python 对文件是块缓冲,卡住时**零线索**。
  现在进度打到 **stderr 并 flush**,且单个包失败**跳过而非中断整批**。
- **API 限额**:未认证 60 次/小时,而**每个包要 2 次**调用 → 一小时约 30 个包;
  600+ 包需要 **20+ 小时**(或带 token)。
- 上次实测一次 `--fetch` 挂了 31 分钟且**一次调用都没发出去**;`urlopen(timeout=)` 
  只覆盖已建立的连接,**DNS/建连阶段可能不受它约束**。

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
    """**带可见进度**的取数。

    实测教训:一次 `--fetch` 批量转换**卡了 31 分钟、日志 0 字节、且一次 API 调用都没发出去**
    (配额 59/60 未消耗)。两个原因值得记住:
      1. stdout 重定向到文件时**是块缓冲的** —— 卡住时你连"卡在哪一步"都看不到;
      2. `urlopen` 的 timeout 只覆盖已建立的连接,**DNS/建连阶段可能不生效**。
    所以:进度打到 **stderr 且 flush**,并给整个调用套一个**硬超时**。
    """
    print(f"  → GET {url}", file=sys.stderr, flush=True)
    req = urllib.request.Request(
        url, headers={"User-Agent": "helix-mason-convert", "Accept": "application/vnd.github+json"}
    )
    try:
        with urllib.request.urlopen(req, timeout=20) as r:
            return json.load(r)
    except Exception as e:
        raise SystemExit(f"取数失败 {url}: {type(e).__name__}: {e}") from e


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


def fix_block_scalars(pkg: dict, text: str) -> None:
    """修 YAML **块标量**:`description: |` 这类会被极简解析器读成字面量 `"|"`。

    这里按原文把后续**更深缩进**的行拼回来(只处理 description —— 实测里唯一的多行字段)。
    **正解是换真 YAML 解析器**;在此之前的取舍:不引入依赖,但只覆盖已知形状。
    """
    if pkg.get("description") not in ("|", ">", "|-", ">-"):
        return
    lines = text.splitlines()
    for i, ln in enumerate(lines):
        m = re.match(r"^(description):\s*[|>]-?\s*$", ln)
        if not m:
            continue
        buf = []
        for nxt in lines[i + 1 :]:
            if not nxt.strip():
                buf.append("")
                continue
            if not nxt.startswith((" ", "\t")):
                break
            buf.append(nxt.strip())
        pkg["description"] = " ".join(x for x in buf if x).strip()
        return


def extract_bins(pkg: dict, text: str) -> None:
    """抽取顶层 `bins:` 列表(helix 的配方**必需** `bin`:解析处是 `need("bin")?`,
    缺它整条配方会被拒绝 —— 不是可选字段)。用正则直接扫原文,避开极简 list 解析。
    """
    m = re.search(r"^bins:\s*\n((?:[ \t]*-[ \t]*\S+\n?)+)", text, re.M)
    if not m:
        return
    bins = re.findall(r"-[ \t]*(\S+)", m.group(1))
    if bins:
        pkg["bins"] = bins


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
    m_tool = re.match(r"pkg:(pypi|npm)/([^@]+)@(.+)", src)
    assets = pkg.get("asset") or []
    if not m:
        problems.append(f"来源既不是 github-release 也不是 pypi/npm({src})—— 需人工判断安装方式")
        version, base = "", ""
    else:
        owner, repo, version = m.group(1), m.group(2), m.group(3)
        base = f"https://github.com/{owner}/{repo}/releases/download/{{version}}"

    # ── 逐平台资产(现在 schema 支持了,所以**不再需要**"映射成统一模板")──
    # 关键:文件名**原样照抄**,只把 Mason 的 `{{version}}` 换成 helix 的 `{version}`。
    # 于是 Go 风格(`linux_amd64`)、clangd 那种不带 arch 的命名、甚至 Rust triple 的 musl 变体
    # —— 全都不需要映射(这正是逐平台资产表存在的意义)。
    asset_rows: list[tuple[str, str, str]] = []
    if m_tool:
        # 包管理器源 → helix 的 **Tool** 型(cmd 在受管目录内跑,产物须落 <prefix>/bin/<bin>)
        # 注意:这里**不能**再走"非 github-release = needs-manual"那条判断(那是 Archive 路的判据)
        eco, pkgname, ver = m_tool.group(1), m_tool.group(2), m_tool.group(3)
        if eco == "npm":
            cmd, args = "npm", ["install", "--prefix", "{prefix}", f"{pkgname}@{ver}"]
        else:  # pypi
            cmd, args = "pip3", ["install", "--target", "{prefix}", f"{pkgname}=={ver}"]
        tool_cmd, tool_args = cmd, args
    elif not m:
        problems.append(f"来源既不是 github-release 也不是 pypi/npm({src})—— 需人工判断用哪种安装方式")
    else:
        for a in assets:
            target, f = a.get("target", ""), a.get("file", "")
            if not target or not f:
                continue
            fname, _, prefix = f.partition(":")
            if prefix:
                problems.append(
                    f"{target}: 资产含解压前缀 `{prefix}` —— helix 的 `strip` 是**数字**(剥层数),"
                    "语义不同,需手工核"
                )
            if target not in MASON_TARGETS:
                problems.append(f"{target}: 未知 target(helix 侧认不出该平台,该条会被忽略)")
            fname = fname.replace("{{version}}", "{version}")
            bin_ = a.get("bin", "").removeprefix("exec:")
            asset_rows.append((target, f"{base}/{fname}", bin_))
        if not asset_rows:
            problems.append("没有任何可用的 asset 条目")

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
        lines.append(f'version = "{version}"')
    if pkg.get("bins"):
        lines.append(f'bin = "{pkg["bins"][0]}"   # 来自 Mason 的 bins[0](helix 的 bin 是必需字段)')
    if m_tool:
        lines.append(f'cmd = "{tool_cmd}"')
        lines.append("args = [" + ", ".join(f'"{a}"' for a in tool_args) + "]")
        lines.append("# 注:helix 的 Tool 型要求产物落在 <prefix>/bin/<bin>(见 server_manager.rs 注释)")
    elif assets:
        lines.append('# 逐平台资产(文件名原样;{version} 由运行时展开):')
        for target, url, bin_ in asset_rows:
            lines.append("")
            lines.append(f"[[server-manager.registry.{name}.asset]]")
            lines.append(f'target = "{target}"')
            lines.append(f'url = "{url}"')
            if bin_:
                lines.append(f'bin = "{bin_}"')
    else:
        lines.append('# 无资产条目(该包可能只有 npm/pypi 源 —— 需手工改为 cmd/args 的 Tool 型)')
    if m_tool:
        # Tool 路的安装语义**已经确定**(cmd/args),不该再带 Archive 路的判据提示。
        # 之所以要"过滤"而不是"改那一处 append":产生该提示的分支在文件里有**两处**,
        # 且措辞不同、生成同一句话 —— 上一版就因此漏改了一处(验证当场打脸)。
        problems = [x for x in problems if "github-release" not in x]
    for pr in problems:
        lines.append(f"# ⚠️ needs-manual: {pr}")
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
        try:
            pkg, text = p, fetch_yaml(p)
        except SystemExit as e:
            print(f"# !! 跳过 {p}: {e}", file=sys.stderr, flush=True)
            continue
        print(f"# 已取 {pkg}", file=sys.stderr, flush=True)
        texts.append((pkg, text))
    if not texts:
        ap.error("给出包名(--fetch)或 --dir")

    ok = manual = 0
    try:
        import tomllib
    except ModuleNotFoundError:
        tomllib = None  # type: ignore
    for name, text in texts:
        pkg = parse_yaml(text)
        fix_block_scalars(pkg, text)
        extract_bins(pkg, text)
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
