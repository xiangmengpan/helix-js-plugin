// server-manager.js — :server 管理面板(rail 骨架;数据经 helix.server.rows term→JS 回传)
// 用法:init.js 里 helix.load("features/server-manager/index.js"),:server-manager 开关。
// 键位:j/k 或 Up/Down 导航;Enter 未装→install / 已装→update;x 移除;r 刷新;q/Esc 关闭。
// 说明:配方无下载源(installable=false)时 Enter 报错提示;移除无二次确认(先 r 看版本)。

helix.plugin("server-manager", { deps: [] });

const S = {
  panel_id: null,
  rows: [],
  sel: 0,
};

function fetch_rows() {
  helix.server.rows((rows) => {
    S.rows = rows || [];
    if (S.sel >= S.rows.length) S.sel = Math.max(0, S.rows.length - 1);
  });
}

function status_of(r) {
  if (r.installed) return r.version ? "installed " + r.version : "installed";
  if (r.installable) return "ready";
  return "no source";
}

function row_el(r, selected) {
  const mark = r.installed ? "\u2713" : " ";
  const glyph = r.kind === "lsp" ? "L" : r.kind === "dap" ? "D" : r.kind === "linter" ? "!" : "F";
  const langs = r.languages && r.languages.length ? " [" + r.languages.join(",") + "]" : "";
  const status = status_of(r);
  const style = selected ? "ui.selection" : null;
  return helix.el("text", mark + " " + glyph + " " + r.name + langs + "  " + status, { style });
}

function render(focus, ctx) {
  try {
    const h = Math.max(1, (ctx ? ctx.height : 20) - 1);
    const start = Math.max(0, Math.min(S.sel - Math.floor(h / 2), S.rows.length - h));
    const window = S.rows.slice(start, start + h);
    const lines = window.map((r, i) => row_el(r, start + i === S.sel));
    if (S.rows.length === 0) {
      return helix.el("col", [
        helix.el("text", "server registry 为空;回车/输入或 :server list 查看", {
          style: "ui.virtual",
        }),
      ]);
    }
    return helix.el("scroll", lines, { height: h });
  } catch (e) {
    return helix.el("col", [
      helix.el("text", "server-manager 渲染错误: " + (e && e.message || e), {
        style: "ui.popup",
      }),
    ]);
  }
}

function move(d) {
  if (S.rows.length === 0) return;
  S.sel = Math.max(0, Math.min(S.rows.length - 1, S.sel + d));
}

function selected() {
  return S.rows[S.sel] || null;
}

function act_toggle() {
  const r = selected();
  if (!r) return;
  if (!r.installable) {
    helix.echo("server '" + r.name + "': 下载源未配置;配 [server-manager.registry." + r.name + "] url/version");
    return;
  }
  if (r.installed) helix.server.update(r.name);
  else helix.server.install(r.name);
  fetch_rows(); // op 应用后行数据会更新;此处先请求(term 顺序处理 op→rows)
}

function act_remove() {
  const r = selected();
  if (!r || !r.installed) {
    helix.echo("server '" + (r ? r.name : "?") + "' 未安装");
    return;
  }
  helix.server.remove(r.name);
  fetch_rows();
}

function handle_key(key) {
  if (key.ctrl || key.alt) {
    if (key.ctrl && key.name === "\\") {
      helix.focus(0);
      return "handled";
    }
    return "handled"; // C-x/M-x 组合穿透(与 filetree 一致)
  }
  switch (key.name) {
    case "Esc":
    case "q": close_panel(); return "handled";
    case "Up":
    case "k": move(-1); return "handled";
    case "Down":
    case "j": move(1); return "handled";
    case "PageUp": move(-10); return "handled";
    case "PageDown": move(10); return "handled";
    case "Home": S.sel = 0; return "handled";
    case "End": S.sel = Math.max(0, S.rows.length - 1); return "handled";
    case "Enter": act_toggle(); return "handled";
    case "x": act_remove(); return "handled";
    case "r": fetch_rows(); return "handled";
    case "?": show_help(); return "handled";
    default: return "ignore";
  }
}

function show_help() {
  helix.echo("server-manager: j/k 或 Up/Down 导航 | Enter 安装/升级 | x 移除 | r 刷新 | q/Esc 关闭");
}

function open_panel() {
  if (S.panel_id !== null) return;
  S.panel_id = helix.open_panel({ side: "left", size: 34, render, onKey: handle_key });
  fetch_rows();
}

function close_panel() {
  if (S.panel_id !== null) {
    helix.close_panel(S.panel_id);
    S.panel_id = null;
  }
}

function toggle_panel() {
  if (S.panel_id !== null) close_panel();
  else open_panel();
}

helix.register_command("server-manager", toggle_panel, "切换 :server 管理面板");

if (typeof module !== "undefined") module.exports = { S, fetch_rows, render, open_panel, close_panel, toggle_panel };
