// picker.js — helix.picker 内置源（files/grep/buffers/symbols）
// 依赖清单：无（只用 helix 核心 API）
helix.plugin("picker", { deps: [] });
// 用法：init.js 里 helix.load("features/picker.js")；然后用 helix.map 绑键位：
//   helix.map("normal", "space-f", () => helix.picker.run("files"));
//   helix.map("normal", "space-g", () => helix.picker.run("grep"));
// 复制本文件即可自定义源（改 items/preview/action；行格式见 plugin-api §10）。

// files：当前目录文件树（cwd = hx 启动目录；行 = [name, path]）
helix.picker.define("files", {
  columns: ["name", "path"],
  items: () => helix.read_tree(".").then((entries) =>
    entries
      .filter((e) => !e.is_dir)
      .map((e) => [e.name, e.path.replace(/^\.\//, "")])),
  preview: (row) => ({ path: row[1], line: 0 }),
  action: (row) => helix.open_file(row[1]),
});

// grep：rg 全文搜索（排除 target/.git；payload = [path, 行号 0-based]）
// rg 无匹配 = exit 1（常规退出，非错误）→ 返回空列表；未安装/参数错等真实错误仍抛
helix.picker.define("grep", {
  columns: ["file:line", "text"],
  items: () =>
    helix.run_async("rg -n --no-heading -g '!target' -g '!.git' .")
      .then((out) =>
        out.split("\n").filter(Boolean).map((line) => {
          const [path, ln, ...text] = line.split(":");
          const p = path.replace(/^\.\//, "");
          return { cells: [p + ":" + ln, text.join(":")], payload: [p, String(Number(ln) - 1)] };
        }))
      .catch((e) => {
        if (e && /^exit 1/.test(e.message)) return [];
        throw e;
      }),
  preview: (row) => ({ path: row[0], line: Number(row[1]) }),
  action: (row) => helix.open_file(row[0], { row: Number(row[1]), col: 0 }),
});

// buffers：打开缓冲切换（payload = [id]）
helix.picker.define("buffers", {
  columns: ["name", "path"],
  items: () => helix.buffers().map((b) => ({ cells: [b.name, b.path || ""], payload: [String(b.id)] })),
  action: (row) => helix.focus_buffer(Number(row[0])),
});

// symbols：文档符号跳转（需 LSP；payload = [name, 起始行 0-based]）
helix.picker.define("symbols", {
  columns: ["name", "kind"],
  items: async () => {
    const syms = (await helix.lsp.document_symbols()) || [];
    const flat = [];
    (function walk(s) {
      for (const x of s) {
        // SymbolInformation 形状无 range → 回退行 0（顶部）
        const line = x.range ? x.range.start.line : 0;
        flat.push({ cells: [x.name, String(x.kind)], payload: [x.name, String(line)] });
        if (x.children) walk(x.children);
      }
    })(syms);
    return flat;
  },
  action: (row) => helix.set_cursor(Number(row[1]), 0),
});
