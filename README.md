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
2. **zellij-style flat modes**: five sibling modes (`C-g`/`C-p`/`C-n`/`C-h`/`C-y`) available from any leaf focus — replaces the old single global `C-w` mode
3. **Terminal enhancements**: native terminal panels (engine is now `alacritty_terminal`: real reflow, mouse reporting, bracketed paste, CJK/combining chars; Esc → scroll mode, scrollback buffer, yank, hook APIs, state persistence)
4. **Pluginized keymap hints**: which-key Chinese hints, configurable position, replacing the built-in Info

Upstream Helix docs: [Website](https://helix-editor.com) · [Documentation](https://docs.helix-editor.com/) · [Keymap](https://docs.helix-editor.com/keymap.html)

## ✨ New features

### zellij-style flat modes (compositor layer, Rust)

From any focus (editor/terminal/panel) in normal mode, press the prefix of a mode.
**Inside any mode, pressing another prefix switches straight to that mode; pressing the
mode's own prefix returns to Normal.**

| Mode | Prefix | Keys inside |
|---|---|---|
| **Pane** | `C-p` | `h j k l` focus · `H J K L` swap · `p`/`P`/`Tab` next/prev pane · `n` new split, `d` down, `r` right · `x` close · `f` fullscreen · `z` minimize · `w`/`e` float/unfloat · `i` pin · `s` stack with sibling |
| **Resize** | `C-n` | `h j k l` grow that way · `H J K L` shrink · `=`/`-` width ∓5% (adjusts the float's size when the active pane floats) |
| **Move** | `C-h` | `h j k l` swap with the directional neighbour (moves the float when floating) |
| **Scroll** | `C-y` | `j`/`k` line · `d`/`u` half page · `C-f`/`C-b`, `h`/`l` page |
| **Locked** | `C-g` | every key except `C-g` goes straight to the focused pane |

- `Esc` exits a mode; the statusline shows `PANE`/`RESIZE`/`MOVE`/`SCROLL`/`LOCKED` plus a Chinese keymap hint
- Interception rule: prefixes **pass through** in insert mode / terminal passthrough (`C-h` is delete-word in insert).
  The one exception is `C-g` — always intercepted, otherwise you could never enter Locked from a terminal passthrough
- The old global `C-w` is **deleted** (`C-w` in insert mode is still the editor's delete-word);
  upstream's `Space w` window subtree remains
- Closing a terminal goes through Pane mode's `x` (Esc switches to normal scroll mode, `i` back to insert)

**Sidebar rail** — `open_panel({ side:"left"|"right", rail:true })` pins a panel (e.g. filetree) to the screen edge at full height; window-mode splits/swap/minimize/zoom never touch it (`C-p h/l` focuses it, browsing keys go to the panel, `Esc`/`C-\`/`C-p h/l` return to the editor area).

- **Window = leaf** (LayoutTree): every window is a leaf showing one view of a buffer; terminal/panel/BufferLeaf are all leaves under the same window mode. The leaf that owns the view routes editing to it (`tree.focus` follows leaf focus), so same-doc multi-leaf editing stays in sync.
- `:vsplit`/`:hsplit` (with or without a path), `gf`, `:vsplit-new`/`:hsplit-new` all create **new leaves** (the old helix view-tree splitting is retired; startup leftovers are auto-adopted into leaves).
- `C-w` in insert mode keeps delete-word; terminal Insert passthrough lets prefixes through to the pty (vim/emacs inside terminal)
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
// Creation: `split` keeps its old name (it has no new equivalent yet —
// the unified `pane.open({content, place})` entry is still to be designed).
const id = helix.split("right", { terminal: { cmd: "bash", size: 40 } });

// ── pane.* (16 ops; vocabulary follows zellij: float / embed) ──
helix.pane.list();                    // → [{id, kind, place, focused, fixed, pinned, z?, rect?}]
helix.pane.info(id);                  // one pane (null if absent); same snapshot as list()
helix.pane.focus(id); helix.pane.focus_dir(id, "left"); helix.pane.move(id, "down");
helix.pane.resize(id, 0.3); helix.pane.close(id);
helix.pane.zoom(id); helix.pane.unzoom();
helix.pane.minimize(id, true); helix.pane.equalize(id); helix.pane.fix(id, true);
helix.pane.float(id); helix.pane.embed(id); helix.pane.raise(id); helix.pane.pin(id, true);

// ── stack.* (programmatic side of `C-p s`; groups are always **two** panes) ──
helix.stack.list();                   // → [{anchor, members}]
helix.stack.create(id);               // stack with the **sibling** pane (non-sibling = no-op)
helix.stack.activate(id, member);     // make `member` the shown one
helix.stack.remove(id);

// ── layout.* (serialization + file persistence) ──
helix.layout.get(); helix.layout.restore(dump);
helix.layout.save("dev"); helix.layout.load("dev"); helix.layout.list(); helix.layout.delete("dev");
// Same thing from the command line: :layout save|load|list|delete <name>

// ── buffer.* ──
helix.buffer.list(); helix.buffer.current(); helix.buffer.focus(id);

// ── icons.* (core-owned single source; zero deps, no `deps` needed) ──
helix.icons.file("src/main.rs"); helix.icons.dir(true); helix.icons.mode("normal");
helix.icons.diagnostic("error"); helix.icons.git("M"); helix.icons.completion(3);
helix.icons.enabled(false);  // turn all nerd-font icons off in one place
                             // (or `[icons] nerd_font = false` in config.toml)
helix.pane_mode.current(); helix.pane_mode.keymap();  // mode + its key table (single source)
```

> **The old flat names are gone** (`focus`, `zoom`, `get_layout`, `buffers`, `layout_fix`,
> `layout_minimize`, `layout_resize`, …) — replaced by the namespaces above.
> Only `split` / `layout_swap` / `layout_resize` remain (no new equivalent yet).

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
  const layout = helix.layout.get();
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

### Buffer traversal, diagnostics & filesystem watch

```js
// Open documents snapshot (read-only; id stable within session)
helix.buffer.list();        // → [{id, path, name, dirty, language}]
helix.buffer.current();     // → id
helix.buffer.focus(id);     // switch current view to that document

// LSP diagnostics of the current document (data from Document::diagnostics)
helix.diagnostics();        // → [{line, message, severity, code, source}]
helix.on("lsp-diagnostics", (docId, diags) => { ... });

// Filesystem watcher (notify-backed, 500ms debounce)
const wid = helix.watch("/path/to/dir", (events) => {
  // events: [{kind: "create"|"modify"|"delete"|"rename", path}]
  filetree.refresh();
});
helix.unwatch(wid);

// Cursor / selection events (frame-throttled)
helix.on("cursor-move", (docId, { row, col, mode }) => { ... });
helix.on("selection-change", (docId, { count, primary }) => { ... });
```

### API overview (grouped, with pros & cons)

**Commands & messaging**

| API | What | Pros | Cons |
|---|---|---|---|
| `register_command(name, fn, doc?)` | register `:name` command | one command = one undo; UI requests applied at command boundary | synchronous on main thread; use async for heavy work |
| `run_command(name, ctx?)` | call plugin command programmatically | scripting / lazy-stub base | only registered plugin commands |
| `echo(text)` | status bar message | simple | no history (overwrite) |

**Document editing & selection**

| API | What | Pros | Cons |
|---|---|---|---|
| `begin_edit()/end_edit()` | batch edit transaction | refactor-style plugins get single undo | must be paired |
| `by_path(path, fn)` | cross-buffer access | edit any doc by path | read-only snapshot + queued edits |
| `set_virtual_text/set_highlight` | decorations/markers | per-doc whole replacement, cleared on reload | render layer, not text ops |
| `set_cursor/set_selection` | cursor/selection | multi-selection array form | — |

**Process execution**

| API | What | Pros | Cons |
|---|---|---|---|
| `run(cmd)` | sync execution | simple, immediate result | **blocks main thread**, short commands only |
| `run_async(cmd)` | async execution | non-blocking; promise resumes on main thread | no streaming output |
| `spawn({cmd, onChunk, onExit})` | streaming process | chunked output, pty support | caller joins chunks |

**Async filesystem** (all return Promise, worker thread)

| API | What | Pros | Cons |
|---|---|---|---|
| `read_dir(path)` | list dir (sync) | simple | sync-blocking; not recursive |
| `read_file_async/write_file_async` | read/write file | async, non-blocking | UTF-8 lossy |
| `stat_async(path)` | file metadata | `{is_dir, size, mtime}` | — |
| `glob_async(pattern)` | glob match | `*`/`**`/`?` | relative to CWD |
| `read_tree(path, {depth})` | recursive tree | dirs first, sorted; depth limit | one-shot, delay on huge trees |

**Event hooks** `on(event, fn)`: `save` `mode-change` `buffer-open` `buffer-close` `doc-change` (with merged ranges) `theme-change` `lsp-diagnostics` `cursor-move` `selection-change` + terminal family (`term-open/mode-change/exit/close/resize/title/key`) `component-event`. Pros: multiple handlers run in order; a throwing handler doesn't block the flow. Cons: `doc` is a read-only snapshot + edit queue.

**Keybindings** `map(mode, key, command|fn)`: rebinding overwrites; multi-key sequences & modifiers; callbacks auto-register as hidden commands. Cons: lost on restart (re-register at plugin startup).

**Popups/Panels/Component tree**: `open_popup` (overlay modal; onKey returns close/handled/ignore) `open_panel` (side panel, layout-tree leaf) `close_panel` `move_panel` `el` (row/col/scroll/button/input). Pros: render + layout engine separated, dirty-cell diff; input is engine-authoritative (onChange auto callback). Cons: repeated open_popup replaces the previous one.

**Picker** `helix.picker.define/run`: define a data source, open the **native Picker** (nucleo fuzzy match / scroll / preview / keys all core). Pros: native performance; arbitrary sources (files/grep/buffers/symbols); rows as array or `{cells, payload}`. Cons: one-shot candidates (no streaming); no per-row component rendering.

**Terminal**: `open_terminal` `term_write/feed/kill/list/close/resize/clear/save` `set_terminal_mode` `term_state` (session persistence) + terminal hooks (`term-key` returns normal/pass/consume/minimize/close). Pros: native pty panels, four display modes, scrollback. Cons: passthrough key handling is manual.

**Layout tree**: `split` `buffer_open` `close_leaf` `zoom/unzoom` `resize_leaf` `layout_resize/swap/minimize/focus/swap_dir/equalize/fix` `focus` `get_layout` `restore_layout`. Pros: zellij-style window management, any-leaf focus; layout_fix immune to swap/close. Cons: restore_layout serialization is half-baked (see §14).

**Buffers/Diagnostics/Watch**: `buffers` `current_buffer` `focus_buffer` `diagnostics` `watch/unwatch` (notify-backed, 500ms debounce). Pros: read-only doc snapshots. Cons: ids valid within a session.

**UI customization**: `set_statusline` `set_buffer_icon` `set_completion_render` (JS-rendered candidate rows, falls back to native two columns) `set_completion_icon` (kind → icon char) `set_diagnostic_icons` `set_component_render` (JS-drawn component appearance) `get_component_state` `set_keymap_hint` (which-key). Pros: render layer fully customizable; fallback on unregistered/throw. Cons: completion row hook runs per row per frame — no heavy logic inside.

**Theme**: `set_theme` (live override, scope-level) `reset_theme` `get_style` `theme_info` `set_theme_name` (async switch). Pros: instant effect, inheritance handled; syntax scopes also overridable. Cons: colors can't reference another scope.

**Plugin management**: `helix.plugin(name, {deps})` declares load deps; `helix.plugin.install(path|git-url)` (manifest tracking + dep resolution + post-install load) `update` (git pull, keeps old on failure) `remove`. Pros: source tracking, uninstall, update, recursive deps, cycle detection; CLI via `:plugin install/update/pin/unpin/remove/status`. Cons: JS API results via status bar (fire-and-forget); pin is git-only.

**LSP**: `helix.lsp.hover/completion/document_symbols/workspace_symbols/format/rename/code_actions/execute_code_action` (see [api/lsp.md](docs/api/lsp.md)). Pros: full query/edit chain, resolves null on failure instead of hanging. Cons: block_on freezes main thread (execute_code_action).
`set_cursor` `set_selection` `get_str`
`set_theme` `reset_theme` `get_style` `theme_info` `set_theme_name` `set_diagnostic_icons`
`run` `run_async` `spawn` `read_file_async` `write_file_async` `stat_async` `glob_async`
`set_component_render` `get_component_state` `set_keymap_hint` `term_state`

**Event whitelist**: `save` `mode-change` `buffer-open` `buffer-close` `doc-change` `theme-change`
`term-open` `term-mode-change` `term-exit` `term-close` `term-resize` `term-title` `term-key` `component-event`
`lsp-diagnostics` `cursor-move` `selection-change`

## 📦 Bundled plugins & the two-layer model

**Layout convention** (full rules: [`docs/plugin-layout.md`](docs/plugin-layout.md)) — a plugin is
**one directory** with a fixed entry name:

```
<plugin root>/
├── init.js          # optional: this root's entry (user's wins over bundled)
├── <name>/plugin.js # ← a plugin. dir name = plugin name = helix.plugin("<name>")
├── lib/             # shared libraries (cross-plugin)
└── examples/        # examples/templates — NOT auto-loaded
```

`init.js` names plugins **by name** (no file paths): `helix.load("filetree")` → `<name>/plugin.js`.
`deps` take plugin names too: `helix.plugin("filetree", { deps: ["icons"] })`.

**Two layers, one rule set** (Neovim `runtimepath` semantics — bundled first, user last ⇒ **user overrides bundled**):

| Layer | Location | Role |
|---|---|---|
| **bundled** | `<runtime>/plugins/` (e.g. `~/.config/helix/runtime/plugins`) | ships with the software |
| **user** | `~/.config/helix/plugins/` | yours; a same-named **directory** wholly overrides the bundled one |

```bash
sh contrib/install-plugins.sh          # install the repo's plugins/ into the bundled layer
DRY_RUN=1 sh contrib/install-plugins.sh
```

| Plugin | Description |
|---|---|
| `filetree/` | Side file tree panel (icons, expand/collapse, Enter opens & focuses) |
| `terminal/` | Terminal commands (:term/:vterm/:hterm), panel management |
| `statusline/` | Statusline (mode icon, file-type icon, git branch, diagnostics) |
| `which-key/` | Keymap Chinese hints (configurable position) |
| `tabbar/` | Layout tab bar demo (top slot, click to focus) |
| `arsenal/` | Overlay marketplace window (`:arsenal`) — replaces the deprecated server-manager |
| `icons/` | `[icons]` config only — the icon table itself lives in core (`helix.icons.*`) |
| `tutor/` | `:tutor` — the tutorial (**the first feature moved out of core into a plugin**; content lives in `runtime/tutor`, the plugin opens it without binding a path) |
| `layout/` | `:layout save\|load\|list\|delete <name>` — named layout sessions (**moved out of core into a plugin**; data API `helix.layout.*` lives in `helix-js`) |
| `plugin/` | `:plugin list\|install\|remove\|update\|pin\|unpin\|reload\|status` — the plugin manager (**moved out of core**); it also provides the `helix.plugin.*` API |
| `pane-open/` | `helix.pane.open({place, content})` — unified pane-creation entry (a thin shape over the existing `helix.split`) |
| `examples/` | `picker.js` (defines the files/grep/buffers/symbols picker sources), `lsp-hover.js` |

Note: a failing plugin no longer takes down the whole entry — `init.js` wraps loads so one bad
plugin reports an error and the rest still load.

## 🛠 Build

```bash
cargo build --release
# a nerd-font terminal is required for icons
```

## 🧩 Key dependencies

- [boa](https://github.com/boa-dev/boa) — embedded JavaScript engine (plugin system)
- [vte](https://github.com/alacritty/vte) — terminal emulator escape-sequence parser (native terminal panels)
- [notify](https://github.com/notify-rs/notify) — filesystem event watching (helix.watch)

## 📄 Docs

- **All docs**: [`docs/README.md`](docs/README.md) (index: usage manual vs. historical archive)
- **Plugin layout & naming**: [`docs/plugin-layout.md`](docs/plugin-layout.md) (folders · entry point · deps · the two-layer override · distribution)
- **Plugin API**: [`docs/plugin-api.md`](docs/plugin-api.md) (overview & index) · [`docs/api/`](docs/api/) (per-domain detail: signatures/examples/pros & cons)
- **Type definitions**: [`plugins/helix.d.ts`](plugins/helix.d.ts) (for editor autocomplete)
- JS view-layer design: `docs/superpowers/specs/2026-08-15-js-ui-rendering-design.md`
- Handoff log: [`docs/superpowers/handoff/2026-08-14.md`](docs/superpowers/handoff/2026-08-14.md) (window mode / terminal / plugin evolution)
- Upstream Helix docs: [Website](https://helix-editor.com) · [Documentation](https://docs.helix-editor.com/) · [Keymap](https://docs.helix-editor.com/keymap.html)

## 🙏 Credits

Upstream [Helix editor](https://github.com/helix-editor/helix) (Kakoune/Neovim-inspired, written in Rust) — all base capability comes from it; this fork only adds the JS plugin system, window mode and other features on top.
