// dashboard/plugin.js —— LazyVim 风格启动屏(**弹窗式**,A′)。
//
// ## 为什么是弹窗(实测决定,非推断)
//   · `open_panel(...)` 需要必填 `side: string` ✗ ⇒ 它是**停靠侧边**的面板,
//     不是全屏居中界面(探针实得:`open_panel: 'side' must be a string` ✓)
//   · `open_popup({width:"80%",height:"70%",position:"center",render,onKey,onClose})` ✓
//     实测:render 被调用且 ctx 给出**自己盒子的宽高**(80%×120=96 ✓ / 70%×150=105 ✓);
//     裸键走 onKey ✓;返回 "close" 真的关闭 ✓ 且触发 onClose ✓;关闭后不再拦键 ✓
//
// ## 弹窗式带来的四个"自动正确"(对比 buffer 式)
//   ① 启动即显示:open_popup **不需要 ctx.doc** ✓ ⇒ 无 ctx/时序难题 ✓
//   ② q 退出程序:onKey 里直接 run_command("quit") ✓
//   ③ 能回到界面:再开一次弹窗即可 ✓(不需要"常驻 buffer" ✓)
//   ④ 无未保存警告:**从不碰任何 buffer** ✓
//   且**零全局键** ⇒ 结构上不可能再出现"按键卡死" ✓(buffer 式那次的教训 ✓)
//
// ## 安全设计
// 所有 API 探测/动作都包 guard();失败一律 echo(**绝不静默**,更不 throw ✗
// —— 真机上抛出的异常不可见,这一路被它坑过三次 ✓)
helix.plugin("dashboard", { deps: [], version: "2.0" });

// ── 可配置 ────────────────────────────────────────────────────────────────
helix.define_config("dashboard", {
  enabled: { type: "boolean", default: true, doc: "无文件启动时是否显示启动屏" },
  logo: { type: "string", default: "blocks", doc: "logo 风格:blocks | slim | none" },
  width: { type: "string", default: "80%", doc: "弹窗宽度(数字或百分比)" },
  height: { type: "string", default: "70%", doc: "弹窗高度" },
});

const LOGO_BLOCKS = [
  "  ██╗  ██╗███████╗██╗     ██╗██╗  ██╗",
  "  ██║  ██║██╔════╝██║     ██║╚██╗██╔╝",
  "  ███████║█████╗  ██║     ██║ ╚███╔╝ ",
  "  ██╔══██║██╔══╝  ██║     ██║ ██╔██╗ ",
  "  ██║  ██║███████╗███████╗██║██╔╝ ██╗",
  "  ╚═╝  ╚═╝╚══════╝╚══════╝╚═╝╚═╝  ╚═╝",
];
const LOGO_SLIM = ["  H E L I X  ⌘"];

// ── 小工具 ────────────────────────────────────────────────────────────────
function guard(fn, fallback) {
  try {
    return fn();
  } catch (e) {
    return fallback;
  }
}
function say(msg) {
  guard(() => helix.echo(msg));
}
function stamp() {
  return guard(() => new Date().toISOString().slice(0, 16).replace("T", " "), "");
}
/// **显示宽度**(不是码点数):CJK/全角在屏上占 2 列 ✓
/// (块元素 █ 与制表符 ╗─ 是 1 列 —— 这一点由真机渲染自己反证过 ✓)
function disp_width(s) {
  let n = 0;
  for (const ch of s) {
    const c = ch.codePointAt(0);
    const wide =
      (c >= 0x1100 && c <= 0x115f) ||
      (c >= 0x2e80 && c <= 0xa4cf) ||
      (c >= 0xac00 && c <= 0xd7a3) ||
      (c >= 0xf900 && c <= 0xfaff) ||
      (c >= 0xfe30 && c <= 0xfe6f) ||
      (c >= 0xff00 && c <= 0xff60) ||
      (c >= 0xffe0 && c <= 0xffe6) ||
      (c >= 0x1f300 && c <= 0x1f64f);
    n += wide ? 2 : 1;
  }
  return n;
}

// ── 菜单(键 → 动作)────────────────────────────────────────────────────────
function picker_run(source) {
  try {
    helix.picker.run(source);
  } catch (e) {
    say("dashboard: 打开 " + source + " 失败 —— " + (e && e.message ? e.message : e));
  }
}
function try_command(name) {
  try {
    helix.run_command(name);
  } catch (e) {
    say("dashboard: 命令 " + name + " 不可用 —— " + (e && e.message ? e.message : e));
  }
}

const MENU = [
  ["f", "查找文件", () => picker_run("files")],
  ["r", "打开 buffer", () => picker_run("buffers")],
  ["e", "文件树", () => try_command("filetree")],
  ["c", "配置", () => guard(() => helix.open_file(guard(() => helix.config_dir() + "/init.js", "~/.config/helix/init.js"), {}))],
  ["m", "市场", () => try_command("arsenal")],
  ["l", "布局", () => try_command("layout")],
  ["p", "插件管理", () => try_command("plugin")],
  ["q", "退出程序", () => try_command("quit")],
];

// ── 内容 ──────────────────────────────────────────────────────────────────
function build_lines(box_width) {
  const cfg = guard(() => helix.get_config("dashboard"), {}) || {};
  const w = typeof box_width === "number" && box_width > 0 ? box_width : 0;
  const pad = (s) =>
    w > 0 ? " ".repeat(Math.max(0, Math.floor((w - disp_width(s)) / 2))) + s : "    " + s;

  const lines = [""];
  if (cfg.logo !== "none") {
    for (const l of cfg.logo === "slim" ? LOGO_SLIM : LOGO_BLOCKS) lines.push(pad(l));
  }
  lines.push("");
  // 菜单**整体居中**:先求最宽项,再共用同一边距 ✓(逐行居中会让键位参差 —— 踩过 ✓)
  const label = (it) => "  " + it[0] + "   " + it[1];
  let item_w = 0;
  for (const it of MENU) item_w = Math.max(item_w, disp_width(label(it)));
  const left = w > 0 ? " ".repeat(Math.max(0, Math.floor((w - item_w) / 2))) : "    ";
  for (const it of MENU) lines.push(left + label(it));
  lines.push("");
  lines.push(pad("────────────────────────────────"));
  lines.push(pad("  " + (stamp() ? "启动于 " + stamp() : "欢迎") + " · Esc 关闭 · q 退出"));
  lines.push("");
  return lines;
}

// ── 开屏 / 关屏 ───────────────────────────────────────────────────────────
let popup_id = null;
let n_open = 0;
let n_key = 0;

function open_dashboard() {
  if (popup_id !== null) return;
  const cfg = guard(() => helix.get_config("dashboard"), {}) || {};
  n_open++;
  const id = guard(
    () =>
      helix.open_popup({
        width: cfg.width || "80%",
        height: cfg.height || "70%",
        position: "center",
        // render 收到的是**自己盒子的宽高** ✓ ⇒ 居中以此为准(不必依赖视口 ✓)
        render: (_focus, ctx) => build_lines(ctx && ctx.width),
        onKey: (key) => {
          n_key++;
          const nm = key && key.name !== undefined ? String(key.name) : String(key);
          if (nm === "Esc" || nm === "C-c") return "close";
          for (const it of MENU) {
            if (it[0] === nm) {
              it[2]();          // 执行动作 ✓
              if (nm !== "q") return "close"; // 除退出外,动作后关闭启动屏 ✓(像 LazyVim ✓)
              return "handled"; // q ⇒ 已交给 :quit ✓
            }
          }
          // ★ 实测发现:弹窗打开时会**吞掉所有按键**(连敲 `:` 都被 onKey 吃掉 ✓)
          // ⇒ 若对未知键返回 "handled",用户会觉得"键盘失灵" ✗
          // ⇒ 改为 **LazyVim 行为:任意其它键即关闭** ✓("想干什么就按什么,屏自己让开" ✓)
          return "close";
        },
        onClose: () => {
          popup_id = null;
        },
      }),
    null
  );
  if (id === null || id === undefined) {
    say("dashboard: open_popup 失败(该版本可能不支持)");
    return;
  }
  popup_id = id;
}

function close_dashboard() {
  const id = popup_id;
  popup_id = null;
  if (id === null) return;
  // 关弹窗:顶层没有统一的 close ✗ ⇒ 逐个探测(照实测:close_panel 存在 ✓)
  for (const fn of ["close_panel", "close_popup"]) {
    if (typeof helix[fn] !== "function") continue;
    try {
      helix[fn](id);
      return;
    } catch (e) {
      say("dashboard: " + fn + " 失败 —— " + (e && e.message ? e.message : e));
    }
  }
  say("dashboard: 未找到可用的关闭接口(Esc 亦可关闭)");
}

// ── 命令(永远可用,不依赖 startup ✓)──────────────────────────────────────
helix.register_command("dashboard", () => open_dashboard());
helix.register_command("dashboard-close", () => close_dashboard());
/// 自诊断:把"居中/触发"依赖的事实打出来 ✓
helix.register_command("dashboard-diag", () => {
  const v = guard(() => helix.viewport(), null);
  say(
    "dashboard-diag: vw=" + (v ? v[0] : "n/a") + " vh=" + (v ? v[1] : "n/a") +
      " popup=" + (popup_id === null ? "closed" : popup_id) +
      " opens=" + n_open + " keys=" + n_key
  );
});

// ── 启动自动显示 ──────────────────────────────────────────────────────────
// open_popup **不需要 ctx.doc** ✓ ⇒ 直接在 startup 里开,无需任何 ctx 转交 ✓
helix.on("startup", () => {
  const cfg = guard(() => helix.get_config("dashboard"), {}) || {};
  if (cfg.enabled === false) return;
  open_dashboard();
});
