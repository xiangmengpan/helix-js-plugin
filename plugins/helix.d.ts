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
/** `term-key` 钩子的返回值:`pass` 直通 pty / `consume` 消费 / `minimize` 最小化 / `close` 关闭终端 */
type TermKeyResult = "pass" | "consume" | "minimize" | "close" | void;

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
  name: string; // 无名 buffer 的占位名,如 "[scratch]"
  path: string | null; // 未绑定路径(scratch)为 null
  dirty: boolean;
  language: string | null; // 未识别语言为 null
}

/** `helix.server.rows(cb)` 回调收到的行(字段来自 Rust 侧 `ServerRow`) */
interface ServerRow {
  name: string;
  kind: string;
  languages: string[];
  installed: boolean;
  local: boolean;
  version: string | null;
  description: string;
  homepage: string | null;
  installable: boolean;
  upgradable: boolean;
  needs_version: boolean;
  bin: string | null;
  source: string | null;
}

/** `helix.server.task(items, cb)` 的入参项 */
interface ServerTaskItem {
  op: string; // 批处理动作;具体取值由 server 管理器决定
  name: string;
  version?: string; // 配方 needs_version 时必填(给非字符串会抛错,不静默丢弃)
}

/** `helix.server.task(items, cb)` 回调收到的事件 */
interface ServerTaskEvent {
  task_id: number;
  seq: number; // 同批次内从 0 递增
  name: string;
  kind: "phase" | "progress" | "done" | "error";
  msg?: string | null;
  bytes?: number | null;
  total?: number | null;
}

// ────────────────────────── picker ──────────────────────────

interface PickerOptions {
  columns: string[];
  items: () => PickerRow[] | Promise<PickerRow[]>;
  preview?: (...payload: unknown[]) => unknown;
  action?: (...payload: unknown[]) => unknown;
  /** 可选键位钩子:核心**不认识**的键会先问它。
   *  仅返回字符串 "handled" 才算消费;抛错/返回其它 => 未处理。
   *  核心键(Esc/Enter/Tab/翻页…)由核心先处理 => 插件拿不到,无法重绑;
   *  纯字符输入永不交给插件。仅在按键时调用(不在每帧路径)。 */
  onKey?: (e: PickerKeyEvent) => "handled" | undefined;
}
/** onKey 收到的事实(只读) */
interface PickerKeyEvent {
  key: string;
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
  query: string;
  selected: number;
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

// 边界协议(B-1):事件 payload 具名化 + 版本化。
//
// 每个事件的 handler 在**位置参数之后**多收一个 payload 对象:它是 `api_version`
// 与具名字段的唯一真相,位置参数只是历史形状(向后兼容,不再扩展)。
// 也可用 `helix.api_version` 在运行时做版本分支。

interface EvPos {
  row: number;
  col: number;
}
interface EvRange {
  start: EvPos;
  end: EvPos;
}
/** 与 `doc.changes[i]` 同形(同一份计算)
 */
interface EvChange {
  oldRange: EvRange;
  newRange: EvRange;
}
/** 所有事件 payload 的公共头 */
interface EventHeader {
  /** 边界协议版本(== `helix.api_version`) */
  api_version: number;
  /** 事件名(= `helix.on` 的第一参) */
  type: EventName;
}
interface EventStartup extends EventHeader {
  /**
   * 启动时命令行让打开的东西数量:**文件/目录参数 + `--tutor` + 管道 stdin(非 tty)**。
   *
   * 为什么它不是"当前 buffer 有没有 path":`hx <目录>` 会先弹文件选择器再建无名 buffer、
   * `--tutor` 的 buffer 被 `set_path(None)`、管道 stdin 也没有路径 —— 这三种都是
   * "用户已经指定了要打开什么",只看 `doc.path` 会把它们判成空启动。
   * `0` 才是真正的"空启动"(该弹启动屏)。
   */
  open_paths: number;
}
/** `kind` 缺种类时是 **null**(不是 undefined) */
interface EventLayoutChange extends EventHeader {
  kind: string | null;
}
/** `save` / `buffer-open` / `buffer-close` / `theme-change` 的 payload */
interface EventDoc extends EventHeader {
  path: string | null;
  cursor: EvPos;
  selection: { anchor: EvPos; head: EvPos };
}
interface EventModeChange extends EventDoc {
  mode: string;
}
interface EventDocChange extends EventDoc {
  changes: EvChange[];
}
interface EventPaneModeChange extends EventHeader {
  mode: string;
}
interface EventCursorMove extends EventHeader {
  doc_id: number;
  row: number;
  col: number;
  mode: string;
}
interface EventSelectionChange extends EventHeader {
  doc_id: number;
  count: number;
  primary: EvPos;
}
interface EventLspDiagnostics extends EventHeader {
  doc_id: number;
  diagnostics: unknown[];
}
interface EventTermOpen extends EventHeader {
  pty_id: number;
  cmd: string;
}
interface EventTermModeChange extends EventHeader {
  pty_id: number;
  mode: string;
}
interface EventTermExit extends EventHeader {
  pty_id: number;
  code: number;
}
interface EventTermResize extends EventHeader {
  pty_id: number;
  rows: number;
  cols: number;
}
interface EventTermTitle extends EventHeader {
  pty_id: number;
  title: string;
}
interface EventTermClose extends EventHeader {
  pty_id: number;
  reason: string;
}
interface EventTermKey extends EventHeader {
  pty_id: number;
  code: string;
  shift: boolean;
  ctrl: boolean;
  alt: boolean;
}
interface EventComponentEvent extends EventHeader {
  id: number;
  kind: string;
  x: number;
  y: number;
}
/** 全部事件 payload 的并集(按 `type` 区分) */
type EventPayload =
  | EventStartup
  | EventLayoutChange
  | EventDoc
  | EventModeChange
  | EventDocChange
  | EventPaneModeChange
  | EventCursorMove
  | EventSelectionChange
  | EventLspDiagnostics
  | EventTermOpen
  | EventTermModeChange
  | EventTermExit
  | EventTermResize
  | EventTermTitle
  | EventTermClose
  | EventTermKey
  | EventComponentEvent;

type EventName =
  | "startup"
  | "layout-change"
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
  | "pane-mode-change"
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
  /**
   * 当前插件**现在**能做什么(能力清单的查询面)。
   *
   * 语义 = "不问就放行吗":没声明 `permissions` / 内置插件 ⇒ 全 `true`(legacy 放行);
   * 声明了则看声明或已记住的决定(未决定 ⇒ `false`,因为那次调用会被问一次)。
   * 用途:插件据此**降级**(例如没有 `process` 就不去调 git,而不是先失败一次)。
   */
  /** 声明加载依赖(拓扑加载)。`deps` 用**插件名**(如 `"icons"`)或文件 key;
   *  ⚠️ 依赖在**本脚本 eval 之后**才加载 ⇒ 顶层代码不能假定依赖已执行(要放到回调/命令里用)
   *
   *  `permissions`(C-1/C-2)是**能力清单**:
   *  * **不写**该字段 = legacy 语义(不拦截,存量插件零影响);
   *  * **写了**之后,未列出的能力首次使用时会被**询问**(本次拒绝 +
   *    记入待授权),用 `:js-permission allow <plugin> <cap>` 授权。
   *  * 随软件分发的内置插件(用户插件目录以外)永不询问。 */
  (
    name: string,
    opts: {
      deps: string[];
      version?: string;
      /** 声明面版本;高于编辑器支持的版本(`helix.api_version`)⇒ `helix.plugin` 抛错 */
      schema_version?: number;
      /**
       * 要求的最低编辑器版本(如 `"25.7"`)。
       * 声明后会与**真实编辑器版本**比较:高于当前版本 ⇒ **状态栏警告并继续加载**
       * (不拒载 —— 与 `schema_version` 的区别:版本不匹配只说明插件可能用到本编辑器
       * 没有的 API,代码本身仍可能正常;而 `schema_version` 更高 = 字段语义都可能变)。
       * 编辑器版本未知(库/单测场景)时跳过检查。
       */
      min_editor?: string;
      /**
       * 能力清单;不写 = legacy 放行(存量插件零影响)。
       * 写了之后,未列出的能力**首次使用时会被询问**(失败一次 + 记入待授权,
       * 用 `:js-permission allow <plugin> <cap>` 授权)。内置插件永不询问。
       */
      permissions?: {
        /** `helix.run` / `helix.run_async` / `helix.spawn` */
        process?: boolean;
        /** `helix.read_file` / `helix.read_file_async` / `helix.read_dir` /
         *  `helix.stat_async` / `helix.glob_async` / `helix.read_tree` / `helix.watch` */
        fs_read?: boolean;
        /** `helix.write_file` / `helix.write_file_async` / `helix.write_plugin_file` /
         *  `helix.remove_plugin_file` */
        fs_write?: boolean;
      };
      /** 贡献点(命令/事件/配置名;只记录,供 `:js-permission list` 展示) */
      contributes?: string[];
    },
  ): void;
}

interface Helix {
  /**
   * 边界协议的线格式版本(B-1/B-2 同一份)。
   * 改字段/改语义 ⇒ 递增;插件可据此分支(`if (helix.api_version >= 2) …`)。
   */
  api_version: number;
  /**
   * 当前插件**现在**能做什么(能力清单的查询面)。
   *
   * 语义 = "不问就放行吗":没声明 `permissions` / 内置插件 ⇒ 全 `true`(legacy 放行);
   * 声明了则看声明或已记住的决定(未决定 ⇒ `false`,因为那次调用会被问一次)。
   * 用途:插件据此**降级**(例如没有 `process` 就不去调 git,而不是先失败一次)。
   */
  permissions(): { process: boolean; fs_read: boolean; fs_write: boolean };
  // ── 命令 / 消息 / 键位 / 加载 ──
  echo(text: string): void;
  register_command(name: string, fn: (ctx: CommandContext) => void, doc?: string): void;
  run_command(name: string, ctx?: CommandContext): boolean;
  map(mode: Mode, key: string, commandOrFn: string | (() => void)): void;
  load(name: string): unknown; // 缓存加载,返回 exports
  export(obj: Record<string, unknown>): void;
  lazy(name: string, ...commands: string[]): void;
  // ── 事件订阅(B-1:每个事件的手势与 payload 都**逐字声明**)──
  //
  // 形状来自实现(`helix-js/src/commands.rs` 的 emit 点),不是凭印象写的:
  // **位置参数**是历史形状(向后兼容、不再扩展),**最后一个参数**恒为具名 payload
  // (`{ api_version, type, … }`,见 `EventPayload`)。位置参数省略尾参在 TS 里合法
  // ⇒ 写 `() => {}` / `(doc) => {}` 的老插件依旧通过类型检查。
  /**
   * `startup`:初始文件处理之后(启动完成)。
   * `doc` 是**启动后的当前 buffer**:`doc.path` 为空 = 启动时没打开文件
   * (内建 dashboard 就靠这一条决定"要不要自动显示启动屏")。
   */
  on(event: "startup", fn: (doc: Doc, payload: EventStartup) => void): void;
  /** `layout-change` 的形状恒定:`kind` 缺省时是 **null**(不是"没有第一参"),`doc` 恒在第二参;第三参是具名 payload。 */
  on(event: "layout-change", fn: (kind: string | null, doc: Doc, payload: EventLayoutChange) => void): void;
  /** `save`:文档已保存(`:w`) */
  on(event: "save", fn: (doc: Doc, payload: EventDoc) => void): void;
  /** `mode-change`:`mode` = normal | insert | select */
  on(event: "mode-change", fn: (mode: string, doc: Doc, payload: EventModeChange) => void): void;
  /** `buffer-open`:新 buffer 打开 */
  on(event: "buffer-open", fn: (doc: Doc, payload: EventDoc) => void): void;
  /** `buffer-close`:buffer 关闭 */
  on(event: "buffer-close", fn: (doc: Doc, payload: EventDoc) => void): void;
  /** `doc-change`:防抖窗口内合并的变更范围在 `doc.changes` / `payload.changes` */
  on(event: "doc-change", fn: (doc: Doc, payload: EventDocChange) => void): void;
  /** `theme-change`:主题切换 */
  on(event: "theme-change", fn: (doc: Doc, payload: EventDoc) => void): void;
  /** `lsp-diagnostics`:诊断更新;`diags` 是原样透传的 JSON 数组 */
  on(event: "lsp-diagnostics", fn: (docId: number, diags: unknown[], payload: EventLspDiagnostics) => void): void;
  /** `cursor-move`:光标移动 */
  on(event: "cursor-move", fn: (docId: number, ev: { row: number; col: number; mode: string }, payload: EventCursorMove) => void): void;
  /** `selection-change`:选区数量/主选区 */
  on(event: "selection-change", fn: (docId: number, ev: { count: number; primary: EvPos }, payload: EventSelectionChange) => void): void;
  /** `term-open`:终端面板打开(`id` = pty id) */
  on(event: "term-open", fn: (id: number, cmd: string, payload: EventTermOpen) => void): void;
  /** `term-mode-change`:终端显示模式 */
  on(event: "term-mode-change", fn: (id: number, mode: string, payload: EventTermModeChange) => void): void;
  /** `term-exit`:终端进程退出 */
  on(event: "term-exit", fn: (id: number, code: number, payload: EventTermExit) => void): void;
  /** `term-close`:终端关闭请求;返回 **false** 阻止关闭 */
  on(event: "term-close", fn: (id: number, reason: string, payload: EventTermClose) => boolean | void): void;
  /** `term-resize`:终端尺寸变更 */
  on(event: "term-resize", fn: (id: number, rows: number, cols: number, payload: EventTermResize) => void): void;
  /** `term-title`:终端标题(OSC) */
  on(event: "term-title", fn: (id: number, title: string, payload: EventTermTitle) => void): void;
  /** `term-key`:终端按键;返回 `TermKeyResult` 决定这次按键怎么处理 */
  on(event: "term-key", fn: (id: number, key: PluginKey, payload: EventTermKey) => TermKeyResult): void;
  /** `pane-mode-change`:平级 pane 模式切换 */
  on(event: "pane-mode-change", fn: (mode: string, payload: EventPaneModeChange) => void): void;
  /** `component-event`:组件节点上的鼠标/点击;返回 **true** 消费该事件 */
  on(event: "component-event", fn: (id: number, ev: { kind: string; x: number; y: number }, payload: EventComponentEvent) => boolean | void): void;
  /** 兜底:事件名 / 参数类型未知时(上面每个具体事件都有**更精确的**重载) */
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
  /** 同步读整个文件(UTF-8);读不到 **throw**(不返回 null)。同族异步:`read_file_async` */
  read_file(path: string): string;
  /** 同步写文件(父目录按需创建);失败 **throw** */
  write_file(path: string, text: string): void;
  read_file_async(path: string): Promise<string>; // UTF-8 lossy
  write_file_async(path: string, content: string): Promise<void>;
  stat_async(path: string): Promise<Stat>;
  glob_async(pattern: string): Promise<string[]>;
  read_tree(path: string, opts?: { depth?: number }): Promise<DirEntry[]>; // 递归+排序

  // ── 弹窗 / 组件 / 面板 ──
  open_popup(opts: PopupOptions): number; // 返回 id;重复打开替换前一个
  // 注:`helix.close_popup(id)` **不存在**(核心未注册;2026-10 对账核实)。
  //     关面板用 `close_panel(id)` / `pane.close(id)`;弹窗目前只能由自身键位(如 Esc)关。
  open_panel(opts: PanelOptions): number;
  close_panel(id: number): void;
  move_panel(id: number, side: PanelSide): void;
  open_file(path: string, opts?: { row?: number; col?: number }): void;
  el(type: string, arg: unknown, opts?: Record<string, unknown>): ElNode;
  set_input_value(popup_id: number, node_id: string, value: string): void;
  get_component_state(id: number): unknown;
  /**
   * 注册组件渲染回调。
   *
   * **脏渲染**:返回**显式 `null`** 表示"自上次以来无变化" —— 宿主复用上次内容、不再解析
   * (何时脏由你决定,故不会陈旧);首次调用必须真的给出内容。`undefined`(忘了 return)仍是错误。
   *
   * **`opts.pure: true`** 声明"渲染**只依赖宿主传入的输入**"(focus + ctx 尺寸 + 调用前宿主喂的
   * 快照),不含主题/缓冲区/配置/时间等外部状态 —— 只有这样声明过,宿主才允许按输入签名缓存并
   * **跳过整次 JS 调用**(每帧渲染的组件如 picker 受益最大)。声明错(实际读了外部状态)会导致
   * UI 不更新;不声明就永远是安全的逐帧真渲染。
   */
  set_component_render(
    id: number,
    fn: (ctx: unknown) => (string | StyledText)[] | null,
    opts?: { pure?: boolean },
  ): void;

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

  // ── 布局(扁平名只剩没有新对应物的这三个)──
  split(dir: SplitDir, leaf: SplitLeafOptions): number; // 返回 leaf_id
  buffer_open(path: string, opts?: unknown): number;
  layout_resize(dir: SplitDir, amount: number, id?: number): void;
  layout_swap(a: number, b: number): void;
  /** 视口尺寸 `[width, height]` ✓ —— 由核心在 `Compositor::new`/`resize` 推送 ✓
   *  **真机实测可用** ✓(`vw=120` ✓);测试夹具里为 `[0,0]`(无真实终端 ✓) */
  viewport(): [number, number];
  /** ⚠️ 实测:**不是** `helix.current_buffer` ✗(探针:`typeof helix.current_buffer === "undefined"` ✓) */
  buffer: {
    /** 已打开文档快照(核心每帧推来);**一帧都没渲染过时为 null** */
    list(): BufferInfo[] | null;
    /** 当前文档的 **id**(不是对象!);拿不到快照时为 null。要详情请用 `list()` 里同 id 那项 */
    current(): number | null;
    /** 把当前视图切到该文档 */
    focus(id: number): void;
  };

  // ── 已删除的旧扁平根名(@ 2026-10 对账核实:核心**未注册**这些名字)──
  // 照旧名调用 = 运行期 undefined。迁移对照(注意两条参数顺序相反):
  //   focus(id)          → helix.pane.focus(id)
  //   zoom(id)/unzoom()  → helix.pane.zoom(id)/helix.pane.unzoom()
  //   close_leaf(id)     → helix.pane.close(id)
  //   resize_leaf(id,r)  → helix.pane.resize(id,r)
  //   layout_minimize(id, side?)  → helix.pane.minimize(id, on?)   ← 语义变了:bool 不是方向
  //   layout_focus(dir, id?)      → helix.pane.focus_dir(id, dir)  ← **参数顺序相反**
  //   layout_swap_dir(dir, id?)   → helix.pane.move(id, dir)       ← **参数顺序相反**
  //   layout_equalize(id?)        → helix.pane.equalize(id)
  //   layout_fix(dir, id?)        → helix.pane.fix(id, fixed)
  //   get_layout()       → helix.layout.get()
  //   restore_layout(x)  → helix.layout.restore(x)
  //   buffers()          → helix.buffer.list()
  //   focus_buffer(id)   → helix.buffer.focus(id)
  //   close_popup(id)    → ✗ **无对应**:核心没注册这个名,内置插件自己也在探测它
  //                        (plugins/dashboard/plugin.js:210 先 `typeof helix.close_popup`
  //                         再退到 close_panel)。面板用 close_panel(id) / pane.close(id)。

  // ── 命名空间(③ 的最终命名;与旧扁平 API 是同一批原生函数) ──
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
    /** ⚠️ 实测:顶层 `helix.close` 是 **undefined** ✗(探针 ✓)。
   *  关 buffer 请用 `helix.open_file()` 切走 / 或命令 `:buffer-close`;
   *  关**面板**用 `helix.close_panel(id)` ✓ */
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
    /** 新建 pane 并**直接带内容**（统一入口；由内置插件 `pane-open` 提供） */
    open(opts: {
      /** 方向；缺省 "right"。也接受别名 below/above */
      place?: SplitDir | "below" | "above";
      /** 透传给底层 split 的内容：`{terminal: {cmd, size}}` 或 `{panel: {...}}` */
      content?: { terminal?: { cmd: string; size?: number }; panel?: PanelOptions };
    }): void;
    // 注:`pane.split` **不存在**(核心未注册;2026-10 对账核实)。按方向分裂用顶层
    //     `helix.split(dir, {terminal|panel})`;要统一创建入口用上面 `open({place, content})`。
    /** 该 pane 的详情（与 `list()` 的单项同构） */
    info(id: number): { id: number; kind: string; place: string; focused: boolean } | null;
    /** 提到最前（浮窗层序 / 堆叠可见位） */
    raise(id: number): void;
    /** 固定：更新时跳过（堆叠成员用） */
    pin(id: number, on?: boolean): void;
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
    /** 把当前布局存成命名会话(还没渲染过一帧 → 抛错) */
    save(name: string): boolean;
    /** 按名字重建布局(不存在 → 抛错) */
    load(name: string): boolean;
    /** 已存会话名列表 */
    list(): string[];
    /** `true`=删了,`false`=本来就不存在(幂等) */
    delete(name: string): boolean;
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

  /** server 管理器(LSP/DAP/linter/formatter)—— 镜像 `:server` 命令族。
   *  **单向请求**:调用即入队,结果经**状态栏**显示,不回传 JS(要结构化数据请用 `rows`)。 */
  server: {
    list(filter?: string): void;
    search(keyword: string): void;
    install(name: string): void;
    update(name?: string): void; // 省略 = 全部(pinned 的会跳过)
    remove(name: string): void;
    status(filter?: string): void;
    /** 请求当前行数据 → **回调一次**(随后接口失效);返回请求 id */
    rows(cb: (rows: ServerRow[]) => void): number;
    /** 批量安装/升级/卸载:`items` 必须非空,每项必须有 op/name;返回任务 id */
    task(items: ServerTaskItem[], cb: (event: ServerTaskEvent) => void): number;
  };
  /** 堆叠组(与 `C-p s` 同一机制;**每组恒为两个 pane**) */
  stack: {
    list(): { anchor: number; members: number[] }[];
    /** 与**兄弟** pane 组堆叠(非兄弟 = 不做事) */
    create(id: number): void;
    /** 把 `member` 设为可见的那个(参数不合法抛错) */
    activate(id: number, member: number): void;
    remove(id: number): void;
  };

  // ── 其他 ──
  /** 自带 runtime 目录内的文件路径(内容型插件打开随软件分发的资源,如 tutor);找不到 null */
  runtime_path(name: string): string | null;
  /** 本次命令的**键入参数**(`:foo a b` → `["a","b"]`);命令之外为空数组 */
  command_args(): string[];
  /** picker 状态摘要(核心渲染 picker 时推来:首行摘要 + 后续可见行,`\t` 分隔) */
  picker_state(): string;
  /** 退出编辑器(入队 Quit;关掉最后一个视图即退出) */
  quit(): void;
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
  /** 用户层插件根目录;未推入根时 null。注意写入**只落最后一个根**(用户层) */
  plugins_dir(): string | null;
  /** 已加载脚本名(按加载顺序;与 `:plugin list` 同一数据源) */
  loaded_plugins(): string[];
  /** 往插件根写文件(父目录按需创建),返回**写入的绝对路径**;
   *  只收相对路径、拒 `..`、越出插件根 → 抛错 */
  write_plugin_file(rel: string, text: string): string;
  /** 删插件根内文件或目录:`true`=删了,`false`=本来不存在(幂等);非法路径 → 抛错 */
  remove_plugin_file(rel: string): boolean;
  plugin: HelixPlugin;
}
