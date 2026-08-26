// lsp-hover — LSP 主动请求 demo：:lsp-hover 光标处 hover 弹窗；:lsp-goto 跳转定义。
// 依赖 helix.lsp.*（4 方法均返回 Promise，透传 LSP 原始 JSON；goto_definition 附 path）。
// 加载：helix.load("features/lsp-hover/index.js") 或 :plugin-load plugins/features/lsp-hover/index.js
// 无 server / 不支持该功能时 resolve null → echo 提示；调用出错 reject → catch 内 echo 错误。

// :lsp-hover 光标处 hover 弹窗
helix.register_command("lsp-hover", async () => {
  const hover = await helix.lsp.hover().catch((e) => {
    helix.echo("lsp-hover error: " + e.message);
    return null;
  });
  if (!hover) return helix.echo("no hover");
  // Hover.contents：字符串 | { value } | 两者数组 → 归一为文本行
  const text = Array.isArray(hover.contents)
    ? hover.contents.map((c) => c.value ?? c).join("\n")
    : hover.contents.value ?? hover.contents;
  const lines = String(text).split("\n");
  helix.open_popup({
    render: () => helix.el("col", lines.map((line) => helix.el("text", line))),
  });
});

// :lsp-goto 跳转到定义（首个结果；path 由 uri/targetUri 注入，无需自行解析）
helix.register_command("lsp-goto", async () => {
  const locs = await helix.lsp.goto_definition().catch((e) => {
    helix.echo("lsp-goto error: " + e.message);
    return null;
  });
  if (!locs) return helix.echo("no definition");
  // Scalar(Location) | Location[] | LocationLink[]，统一成数组
  const arr = Array.isArray(locs) ? locs : [locs];
  // 无 path 的项跳过（非 file:// uri 等）；Location 用 range，LocationLink 用 targetRange
  const l = arr.find((x) => x.path && (x.range ?? x.targetRange));
  if (!l) return helix.echo("no definition");
  const r = l.range ?? l.targetRange;
  helix.open_file(l.path, { row: r.start.line, col: r.start.character });
});
