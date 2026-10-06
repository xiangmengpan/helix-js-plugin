// helix.d.ts — Helix JS 插件 API 类型声明(ambient global)
//
// 用途:给 tsserver(typescript-language-server)补上 `helix.` 的补全/签名提示。
// 插件 .js 是全局脚本(无 import/export),本文件也不含 export → `helix` 声明为
// 全局符号,项目内所有 JS 文件可见。
//
// 接线方式(二选一):
//   1. 目录级:插件根目录放 jsconfig.json,include 覆盖 "**/*.js" 与本文件(见 plugins/jsconfig.json)
//   2. 文件级:每个插件文件头部加 `/// <reference path="helix.d.ts" />`(相对路径)
//
// 依据:helix-js/src/lib.rs 绑定注册表 + docs/plugin-api.md + docs/api/*.md。

declare const helix: Helix;
declare const TABBAR_ID: number;

// ────────────────────────── 基础类型 ──────────────────────────

type PanelSide = "right" | "left" | "bottom";
type SplitDir = "right" | "left" | "top" | "bottom";
type TerminalMode = "dock" | "fullscreen" | "floating" | "minimized";
type Mode = "normal" | "insert" | "select";
type KeyResult = "close" | "handled" | "ignore";

/** 0-based 行列坐标 */
interface Pos {
  row: number;
  col: number;
}

/** onKey 收到的按键对象 */
interface PluginKey {
  name: string; // 字符 | Enter|Esc|Tab|Backspace|Delete|Insert|方向键|Home|End|PageUp|PageDown|F1..F12
  shift: boolean;
  ctrl: boolean;
  alt: boolean;
}

/** 可编辑文档快照:坐标为命令开始时原始快照,越界自动 clamp,一次命令 = 一次撤销 */
interface Doc {
  path: string | null;
  text: string;
  cursor: Pos;
  selection: { anchor: Pos; head: Pos };
  insert(row: number, col: number, text: string): void;
  replace(sr: number, sc: number, er: number, ec: number, text: string): void;
  delete(sr: number, sc: number, er: number, ec: number): void;
}

/** 命令 ctx = 只读快照 + 编辑队列 */
interface CommandContext {
  path: string | null;
  text: string;
  cursor: Pos;
  selection: { anchor: Pos; head: Pos };
  doc: Doc;
}

/** render 返回值:行数组(string 或 {text,style})或 el 组件树 */
interface StyledText {
  text: string;
  style?: string | null;
}
type RenderLine = string | StyledText;
type RenderContent = RenderLine[] | ElNode;
type RenderFn = (focus: string | null, ctx: { width: number; height: number }) => RenderContent;

type KeyHandler = (key: PluginKey, doc: Doc) => KeyResult;

// ────────────────────────── el 组件树 ──────────────────────────

type ElNode =
  | {
      type: "text";
      text: string | StyledText[]; // 字符串或富文本段(一行多色)
      style?: string | null;
      width?: number; // 截断上限
      flex?: number;
      wrap?: boolean;
    }
  | {
      type: "row" | "col"; // 水平并排 / 垂直堆叠
      children: ElNode[];
      gap?: number;
    }
  | {
      type: "scroll"; // 高度裁剪容器(保留最后 height 行)
      children: ElNode[];
      height?: number;
      offset?: number;
    }
  | {
      type: "button";
      label: string;
      id?: string; // 有 id → 可聚焦,Enter/Space 触发 onPress
      onPress?: () => void;
      style?: string | null;
    }
  | {
      type: "input";
      id: string; // 可聚焦;value 由引擎权威维护(JS 传值仅初始化)
      value?: string;
      width?: number;
      multiline?: boolean; // true → Enter 换行/Up-Down 行间移动
      onChange?: (v: string) => void;
      onKey?: (k: PluginKey) => void;
    };

// ────────────────────────── 弹窗 / 面板 ──────────────────────────

interface PopupOptions {
  render: RenderFn;
  onKey?: KeyHandler;
  onClose?: () => void;
  width?: number; // 尺寸上限(clamp)
  height?: number;
  position?: Pos; // 屏幕锚点;默认向下展开,底部空间不足自动翻到上方(非绝对左上角)
}
interface PanelOptions {
  side: PanelSide; // 白名单 right|left|bottom(无 top)
  size: number;
  render: RenderFn; // 面板 render 固定收到 focus = null
  onKey?: KeyHandler;
  onClose?: () => void;
  focusable?: boolean; // 参与焦点路由(Tab/Esc)
}

// ────────────────────────── 进程 / fs ──────────────────────────

interface SpawnOptions {
  cmd: string;
  pty?: boolean;
  onChunk?: (chunk: string) => void;
  onExit?: () => void;
}
interface DirEntry {
  name: string;
  is_dir: boolean;
  path: string;
}
interface Stat {
  is_dir: boolean;
  size: number;
  mtime: number;
}

// ────────────────────────── 终端 / 布局 ──────────────────────────

interface TerminalOptions {
  cmd: string;
  side?: PanelSide;
  size?: number;
  onExit?: () => void;
}
interface TermInfo {
  view_id: number;
  cmd: string;
}
interface SplitLeafOptions {
  terminal?: { cmd: string; size?: number };
  panel?: { render: RenderFn; onKey?: KeyHandler; size?: number };
}
interface BufferInfo {
  id: number;
  name?: string;
  path?: string | null;
}

// ────────────────────────── picker ──────────────────────────

interface PickerOptions {
  columns: string[];
  items: () => PickerRow[] | Promise<PickerRow[]>;
  preview?: (...payload: unknown[]) => unknown;
  action?: (...payload: unknown[]) => unknown;
}
type PickerRow = unknown[] | { cells: unknown[]; payload?: unknown[] };

// ────────────────────────── 主题 / 装饰 ──────────────────────────

type ThemeColor =
  | string
  | { fg?: string; bg?: string; modifiers?: string[] };
interface CompletionCtx {
  label: string;
  kind: string;
  kindNum: number;
  provider: string;
  detail: string;
  deprecated: boolean;
  matchIndices: number[];
}
interface StatuslineCtx {
  path: string | null;
  mode: string;
  cursor: Pos;
}

// ────────────────────────── LSP(原始协议 JSON) ──────────────────────────

interface LspPos {
  row: number;
  col: number;
}
interface LspRange {
  start: LspPos;
  end: LspPos;
}
interface LspHover {
  contents: unknown;
  range?: LspRange;
}
interface LspCompletionResult {
  isIncomplete: boolean;
  items: unknown[];
}
interface LspLocation {
  path?: string; // 便利字段
  range?: LspRange;
}

// ────────────────────────── 事件 ──────────────────────────

type EventName =
  | "save"
  | "mode-change"
  | "buffer-open"
  | "buffer-close"
  | "doc-change"
  | "theme-change"
  | "lsp-diagnostics"
  | "cursor-move"
  | "selection-change"
  | "term-open"
  | "term-mode-change"
  | "term-exit"
  | "term-close"
  | "term-resize"
  | "term-title"
  | "term-key"
  | "component-event";

// ────────────────────────── 主接口 ──────────────────────────

interface HelixPlugin {
  /**
   * 安装插件(本地路径或 git URL):复制/clone → 写 manifest。
   * 结果经状态栏反馈;失败抛错。
   * 与 `:plugin install <path|git-url>` 同一实现。
   */
  install(arg: string): void;
  /**
   * 更新 git 源插件(`fetch` + `ff-only` 合并,记录新 HEAD)。
   * 省略 `name` = 更新全部;**`pinned` 的会被跳过**。
   * 与 `:plugin update [name]` 同一实现。
   */
  update(name?: string): void;
  /**
   * 移除插件:删 manifest 记录的文件与目录,并更新 manifest。
   * 与 `:plugin remove <name>` 同一实现。
   */
  remove(name: string): void;
  /** 声明加载依赖(文件 key,拓扑加载) */
  (name: string, opts: { deps: string[]; version?: string }): void;
  /** 安装插件(本地路径或 git-url),结果经状态栏显示 */
  install(arg: string): void;
  /** 更新插件(不传 = 全部) */
  update(name?: string): void;
  /** 删除插件 */
  remove(name: string): void;
}

interface Helix {
  // ── 命令 / 消息 / 键位 / 加载 ──
  echo(text: string): void;
  register_command(name: string, fn: (ctx: CommandContext) => void, doc?: string): void;
  run_command(name: string, ctx?: CommandContext): boolean;
  map(mode: Mode, key: string, commandOrFn: string | (() => void)): void;
  load(name: string): unknown; // 缓存加载,返回 exports
  export(obj: Record<string, unknown>): void;
  lazy(name: string, ...commands: string[]): void;
  on(event: EventName, fn: (...args: any[]) => void): void;

  // ── 文档编辑与选区 ──
  begin_edit(): void;
  end_edit(): void;
  by_path(path: string): Doc | null; // 只查已打开 buffer
  set_cursor(row: number, col: number): void;
  set_selection(
    ar: number, ac: number, hr: number, hc: number,
  ): void;
  set_selection(
    ranges: { anchor: Pos; head: Pos }[],
  ): void;
  /** 行内装饰:只传 path 清除该 buffer 全部装饰 */
  set_virtual_text(path: string, row?: number, col?: number, text?: string, style?: string | null): void;
  /** 区域高亮:半开区间 [start, end);style 可省略 */
  set_highlight(path: string, sr: number, sc: number, er: number, ec: number, style?: string | null): void;

  // ── 进程与异步 fs ──
  run(cmd: string): string; // 同步,阻塞主线程
  run_async(cmd: string): Promise<string>; // 失败 reject Error
  spawn(opts: SpawnOptions): number; // 返回 id
  read_dir(path: string): DirEntry[]; // 同步
  read_file_async(path: string): Promise<string>; // UTF-8 lossy
  write_file_async(path: string, content: string): Promise<void>;
  stat_async(path: string): Promise<Stat>;
  glob_async(pattern: string): Promise<string[]>;
  read_tree(path: string, opts?: { depth?: number }): Promise<DirEntry[]>; // 递归+排序

  // ── 弹窗 / 组件 / 面板 ──
  open_popup(opts: PopupOptions): number; // 返回 id;重复打开替换前一个
  close_popup(id: number): void;
  open_panel(opts: PanelOptions): number;
  close_panel(id: number): void;
  move_panel(id: number, side: PanelSide): void;
  open_file(path: string, opts?: { row?: number; col?: number }): void;
  el(type: string, arg: unknown, opts?: Record<string, unknown>): ElNode;
  set_input_value(popup_id: number, node_id: string, value: string): void;
  get_component_state(id: number): unknown;
  set_component_render(id: number, fn: (ctx: unknown) => (string | StyledText)[]): void;

  // ── 界面定制 ──
  set_statusline(fn: ((ctx: StatuslineCtx) => string | null) | null): void; // null 隐藏
  set_buffer_icon(fn: (path: string) => string | null): void;
  set_completion_icon(fn: (kindNum: number) => string): void;
  set_completion_render(fn: (ctx: CompletionCtx) => (string | StyledText)[] | ElNode): void;
  set_diagnostic_icons(icons: { error?: string; warning?: string; info?: string; hint?: string }): void;
  set_keymap_hint(fn: ((ctx: unknown) => { text: string; position?: string } | null) | null): void;

  // ── 终端 ──
  open_terminal(opts: TerminalOptions): number; // 返回 view_id
  term_write(id: number, text: string): void;
  term_feed(id: number, text: string): void;
  term_kill(id: number): void;
  term_list(): TermInfo[];
  term_close(view_id: number): void;
  term_resize(id: number, rows: number, cols: number): void; // Unix
  term_clear(id: number): void;
  term_save(id: number, path?: string): void;
  term_state(id: number, action: string, arg?: unknown): unknown;
  set_terminal_mode(view_id: number, mode: TerminalMode): void;
  resize_term(id: number, size: number): void;

  // ── 布局 ──
  split(dir: SplitDir, leaf: SplitLeafOptions): number; // 返回 leaf_id
  buffer_open(path: string, opts?: unknown): number;
  close_leaf(id: number): void;
  zoom(id: number): void;
  unzoom(): void;
  resize_leaf(id: number, ratio: number): void;
  layout_resize(dir: SplitDir, amount: number, id?: number): void;
  layout_swap(a: number, b: number): void;
  layout_minimize(id: number, side?: PanelSide): void;
  layout_focus(dir: SplitDir, id?: number): void;
  layout_swap_dir(dir: SplitDir, id?: number): void;
  layout_equalize(id?: number): void;
  layout_fix(dir: SplitDir, id?: number): void;
  focus(id: number): void;
  get_layout(): {
    tree: unknown; active: unknown; zoomed: unknown; minimized: unknown;
    floats: unknown[]; leafs: unknown[];
  } | null;
  restore_layout(layout: unknown): void; // 真重建:按 dump 重塑布局树 + 浮窗槽位
  buffers(): BufferInfo[];
  current_buffer(): BufferInfo | null;
  focus_buffer(id: number): void;
  // ── ③ 新命名空间(与上面扁平 API 等价;扁平名待删) ──
  /** pane 操作 —— 词汇对齐 zellij 插件 API(`float` / `embed`) */
  pane: {
    /** 统一 pane 清单(含浮窗;字段取自 Rust 侧推来的快照) */
    list(): {
      id: number;
      kind: string; // 类型名末段:"EditorView" / "PluginTerminal" / "PluginPanel" / …
      place: "tiled" | "float" | "minimized";
      focused: boolean;
      fixed: boolean;
      pinned: boolean;
      z?: number; // 浮窗层序
      rect?: { x: number; y: number; w: number; h: number }; // 浮窗比例几何
    }[];
    /** 平铺 → 浮窗 */
    float(id: number): void;
    /** 浮窗 → 平铺 */
    embed(id: number): void;
    close(id: number): void;
    focus(id: number): void;
    focus_dir(id: number, dir: SplitDir): void;
    /** 与方向邻居交换(浮窗则搬位置) */
    move(id: number, dir: SplitDir): void;
    resize(id: number, ratio: number): void;
    zoom(id: number): void;
    unzoom(): void;
    minimize(id: number, on?: boolean): void;
    equalize(id: number): void;
    fix(id: number, fixed: boolean): void;
  };
  /** 图标表 —— **核心单一来源**(helix-js/src/icons.rs)。
   *  所有插件**零依赖**可用,不需要再声明 `deps: ["lib/icons.js"]`。
   *  字体不支持 nerd font 时用 `enabled(false)` 一处降级(文件类图标变空、目录回退 ▸/▾),
   *  或在 config.toml 写 `[icons] nerd_font = false`。 */
  icons: {
    /** 文件类型图标。优先级:特殊文件名(readme/makefile/.gitignore…)→ 扩展名 → 默认 */
    file(path: string): string;
    /** 目录图标(展开/折叠);降级时为 ▾/▸ */
    dir(expanded: boolean): string;
    /** 模式图标(normal/insert/select) */
    mode(mode: string): string;
    /** 诊断标记的**默认**字形(与 set_diagnostic_icons 的覆盖集是两条通道) */
    diagnostic(severity: string): string;
    /** git 状态标记 */
    git(status: string): string;
    /** 补全类型图标(LSP kind;传数字会被转成字符串) */
    completion(kind: string | number): string;
    /** 总开关:无参时返回当前状态,便于插件自检 */
    enabled(on?: boolean): boolean;
  };
  /** 布局序列化 */
  layout: {
    get(): unknown;
    restore(layout: unknown): void;
  };
  /** compositor 的平级模式 —— 键位表的**唯一来源**(插件不要自建第二份) */
  pane_mode: {
    /** 当前模式,如 "C-p";Normal 时 null */
    current(): string | null;
    /** 当前模式的键位表(中文说明 + enabled/reason) */
    keymap(): {
      keys: { key: string; desc: string; enabled: boolean; reason?: string }[];
    } | null;
  };

  // ── 其他 ──
  watch(path: string, fn: (event: unknown) => void): void;
  unwatch(id: unknown): void;
  diagnostics(): unknown;
  define_config(name: string, schema: unknown): void;
  get_config(name: string): unknown;
  get_config_docs(name: string): unknown;

  // ── 主题 ──
  set_theme(theme: Record<string, ThemeColor>): void; // scope 级实时覆盖
  reset_theme(): void;
  get_style(scope: string): ThemeColor | null;
  theme_info(): unknown;
  set_theme_name(name: string): void;

  // ── LSP ──
  lsp: {
    hover(pos: { row: number; col: number }[]): Promise<LspHover | null>;
    completion(pos: { row: number; col: number }[]): Promise<unknown[] | LspCompletionResult | null>;
    goto_definition(pos: { row: number; col: number }[]): Promise<LspLocation | LspLocation[] | null>;
    document_symbols(): Promise<unknown[] | null>;
    format(): Promise<{ applied: true } | null>; // 自动应用
    rename(newName: string): Promise<{ applied: true; files: number } | null>; // 自动应用
    code_actions(pos: { row: number; col: number }[]): Promise<unknown[] | null>;
    execute_code_action(action: unknown): Promise<{ applied: true } | null>; // 自动应用
  };

  // ── Picker ──
  picker: {
    define(name: string, opts: PickerOptions): void;
    run(name: string): void;
  };

  // ── 插件管理 ──
  plugin: HelixPlugin;
}
