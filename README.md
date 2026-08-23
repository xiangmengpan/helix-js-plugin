[English](README.md) · [中文](README.zh-CN.md)

<div align="center">

<h1>
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="logo_dark.svg">
  <source media="(prefers-color-scheme: light)" srcset="logo_light.svg">
  <img alt="Helix" height="128" src="logo_light.svg">
</picture>
</h1>

**Helix + JS plugin system + window mode (zellij-style)**

</div>

## 📌 What is this

A deeply modified fork of [Helix editor](https://github.com/helix-editor/helix) (a Kakoune/Neovim-inspired editor written in Rust). Core changes:

1. **JS plugin system**: embedded boa JavaScript engine — plugins can register commands, render UI components, listen to events, and take over keymap hints
2. **zellij-style window mode**: global `C-w` window management (focus/swap/resize/minimize/close) available from any leaf focus
3. **Terminal enhancements**: native terminal panels (Esc → scroll mode, scrollback buffer, yank, hook APIs, state persistence)
4. **Pluginized keymap hints**: which-key Chinese hints, configurable position, replacing the built-in Info

Upstream Helix docs: [Website](https://helix-editor.com) · [Documentation](https://docs.helix-editor.com/) · [Keymap](https://docs.helix-editor.com/keymap.html)

## ✨ New features

### Global window mode (compositor layer, Rust)

Press `C-w` from any focus (editor/terminal/panel) in normal mode:

| Key | Action |
|---|---|
| `h` `j` `k` `l` | Focus direction (left/down/up/right) |
| `H` `J` `K` `L` | Swap windows |
| `C-h` `C-j` `C-k` `C-l` | Resize ∓5% |
| `x` | Close window |
| `z` | Minimize/restore |
| `f` | Maximize/restore |
| `Enter` | Confirm current window & exit |
| `Esc` / `C-w` | Exit |

- `C-w` in insert mode keeps delete-word; terminal Insert passthrough lets `C-w` through to the pty (vim/emacs inside terminal)
- Closing a terminal goes through window-mode `x` (Esc switches to normal scroll mode, `i` back to insert)

### JS plugin system

Rust provides low-level services (layout tree, pty, grid, draw primitives, state machines); JS owns views and behavior:

- **Component state**: `helix.get_component_state(id)` → terminal mode/title/minimized, etc.
- **Component view**: `helix.set_component_render(id, fn)` → any component's chrome drawn by JS (title bar / tab bar)
- **Keymap hints**: `helix.set_keymap_hint(fn)` → which-key Chinese hints, return `{text, position}` to pick position
- **Mouse interaction**: `helix.on("component-event", ...)` → clicks delegated to JS
- **Terminal hooks**: term-key/close/open/exit/title/resize/mode-change + `term_state` persistence

### Terminal enhancements

- `Esc` → normal scroll mode (scrollback, `y` yank, `g`/`G` jump), `i` → insert passthrough
- Terminal title bar / minimized bar drawable via JS view (grid rendering stays in Rust — the hot path)
- OSC title, scroll offset, mode exposed via `get_component_state`

## 🔌 JS API usage

### Commands & events

```js
// Register a command (:my-cmd; 3rd arg is the doc shown in hints)
helix.register_command("my-cmd", (ctx) => { ... }, "My command");

// Listen to events (save / buffer-open / mode-change / doc-change / theme-change ...)
helix.on("save", (doc) => { ... });

// Load / export modules
helix.load("lib/icons.js");                       // relative to plugin dir
helix.plugin("my-plugin", { deps: ["lib/icons.js"] });
helix.export({ run: myFn });                      // callable after other plugins load it
```

### Layout & windows

```js
const id = helix.split("right", { terminal: { cmd: "bash", size: 40 } }); // split
const id = helix.buffer_open("/path/file.js", { split: "right" });        // buffer leaf
helix.focus(id);              // focus leaf
helix.layout_fix(id, true);   // fix leaf (immune to swap/close/minimize)
const layout = helix.get_layout(); // { tree, active, leafs:[{id, fixed}], zoomed, minimized }
helix.zoom(id); helix.unzoom();
helix.layout_minimize(id, true);    // minimize
helix.layout_resize(id, "h", 0.05); // resize
```

### UI rendering

```js
// Statusline
helix.set_statusline((ctx) => [ { text: " N ", style: "ui.statusline.normal" }, ctx.path, ... ]);

// Popup / panel (component tree: type text/row/col/scroll/button/input)
const popupId = helix.open_popup({
  width: 40, height: 10,
  render: () => helix.el("col", [
    helix.el("text", "Title", { style: "ui.popup" }),
    helix.el("text", "Content"),
  ]),
  onKey: (key) => { ... },
  onClose: () => { ... },
});
```

### Component view layer (JS-rendered component chrome)

```js
// Terminal title bar: JS draws the top row, grid stays in Rust
helix.set_component_render(tid, (ctx) => [
  { type: "text", text: " " + (helix.get_component_state(tid).title || "terminal") },
]);

// Layout tab bar (top slot, helix.TABBAR_ID constant)
helix.set_component_render(TABBAR_ID, () => {
  const layout = helix.get_layout();
  return layout.leafs.map((leaf) => ({
    type: "text", text: " " + leaf.id + " ", style: leaf.id === layout.active ? "ui.selection" : "ui.statusline.inactive",
  }));
});

// Mouse click on a component
helix.on("component-event", (id, { kind, x, y }) => {
  if (kind === "click") { ...; return true; } // return true to consume
});
```

### Keymap hints (which-key)

```js
helix.set_keymap_hint((ctx) => ({
  text: ctx.entries.map((e) => e.keys + "  " + e.doc).join("\n"),
  position: "bottom-right", // bottom-right|bottom-left|top-right|top-left|center
}));
// Return null to hide; no callback registered → built-in Info fallback
```

### Terminal hooks

```js
helix.on("term-key", (ptyId, { code, shift, ctrl, alt }) => {
  if (code === "esc") return "normal";  // or pass/consume/minimize/close
});
helix.on("term-close", (ptyId, reason) => false);   // return false to block close
helix.on("term-open", (ptyId, cmd) => { ... });
helix.on("term-exit", (ptyId, code) => { ... });    // process exit
helix.on("term-title", (ptyId, title) => { ... });
helix.term_state(ptyId, "cwd", "/path");            // persistent state (cross-session)
helix.term_state(ptyId, "cwd");                     // read
```

### Full API list

`echo` `register_command` `run_command` `on` `map` `el` `export` `plugin` `load` `lazy`
`open_popup` `open_panel` `close_panel` `move_panel` `read_dir` `open_file` `set_buffer_icon` `set_statusline`
`open_terminal` `term_write` `term_feed` `term_kill` `term_list` `term_close` `term_resize` `term_clear` `term_save` `set_terminal_mode`
`split` `buffer_open` `close_leaf` `zoom` `unzoom` `resize_leaf` `layout_resize` `layout_swap` `layout_minimize` `layout_focus` `layout_swap_dir` `layout_equalize` `layout_fix` `focus` `get_layout` `restore_layout`
`set_cursor` `set_selection` `get_str`
`set_theme` `reset_theme` `get_style` `theme_info` `set_theme_name` `set_diagnostic_icons`
`run` `run_async` `spawn` `read_file_async` `write_file_async` `stat_async` `glob_async`
`set_component_render` `get_component_state` `set_keymap_hint` `term_state`

**Event whitelist**: `save` `mode-change` `buffer-open` `buffer-close` `doc-change` `theme-change`
`term-open` `term-mode-change` `term-exit` `term-close` `term-resize` `term-title` `term-key` `component-event`

## 📦 Bundled plugins

Located in `~/.config/helix/plugins/` (auto-loaded by `init.js`; repo mirror under `plugins/`):

| Plugin | Description |
|---|---|
| `features/filetree/` | Side file tree panel (icons, expand/collapse, Enter opens & focuses) |
| `features/terminal.js` | Terminal commands (:term/:vterm/:hterm), panel management |
| `features/statusline.js` | Statusline (mode icon, file-type icon, git branch, diagnostics) |
| `features/which-key.js` | Keymap Chinese hints (full keymap coverage, configurable position) |
| `features/tabbar.js` | Layout tab bar demo (top slot, click to focus) |
| `lib/icons.js` | Unified icon map (file type / dir / mode / diagnostic / git) |
| `lib/layout.js` | Layout command wrappers (:layout-focus/swap/resize/minimize ...) |

Install: copy into `~/.config/helix/plugins/`, add `helix.load("features/xxx.js")` to `~/.config/helix/init.js`; changes take effect with `:plugin-reload`.

## 🛠 Build

```bash
cargo build --release
# a nerd-font terminal is required for icons
```

## 📄 Docs

- JS view-layer design: `docs/superpowers/specs/2026-08-15-js-ui-rendering-design.md`
- Handoff log: `docs/handoff-2026-08-14.md` (window mode / terminal / plugin evolution)
- Upstream Helix docs: [Website](https://helix-editor.com) · [Documentation](https://docs.helix-editor.com/) · [Keymap](https://docs.helix-editor.com/keymap.html)

## 🙏 Credits

Upstream [Helix editor](https://github.com/helix-editor/helix) (Kakoune/Neovim-inspired, written in Rust) — all base capability comes from it; this fork only adds the JS plugin system, window mode and other features on top.
