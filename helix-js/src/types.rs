use boa_engine::JsValue;
use serde::Serialize;

use crate::draw::{BorderKind, DrawOp};
use crate::picker::RowSpec;

/// 一次文档变更的 (旧文本坐标, 新文本坐标)，均为 char 索引；doc-change 事件窗口内合并后序列化。
pub type DocChange = ((usize, usize), (usize, usize));
// ── UiRequest 的具名 payload(B-2)────────────────────────────────────────
//
// 每个变体的字段结构在此**各有一个名字**(`<变体>Req`),`UiRequest` 只负责"是哪件事"。
// 为什么:字段内联在变体上时,JS→Rust 这条边没有任何可指名的形状 —— 想写一个
// "构造 + 校验"的函数就得把 `UiRequest::X { a, b, c }` 抄一遍,抄错了没有编译错。
// 现在形状是类型,构造点与消费点共用同一个名字,字段改名会同时打在两边。
//
// 版本:同一份 `API_VERSION` 管两个方向(Rust→JS 的事件 payload 与 JS→Rust 的
// UiRequest);队列本身是同进程枚举、不过线,所以版本挂在常量与 `api_version()`
// 访问器上,而不是每个结构体里再塞一个字段(那只会让 150 个构造点全要改)。

#[derive(Debug)]
pub struct OpenPopupReq {
    pub id: u64,
    /// 弹窗宽高上限(px;v1 尺寸 hint;center 模式下为固定 px 尺寸)
    pub width: Option<u16>,
    pub height: Option<u16>,
    /// anchor 模式:参照点 (row, col);center 模式忽略(恒 None)
    pub position: Option<(u16, u16)>,
    /// 图层名:同层 open_popup 替换、不同层并存(JS 缺省 "plugin-popup" 保兼容)
    pub layer: String,
    /// 居中浮层(position:"center")
    pub center: bool,
    /// center 模式视口百分比 (width%, height%),两轴成对给定(78/75 → 78%×75%)
    pub size_pct: Option<(u16, u16)>,
}

#[derive(Debug)]
pub struct OpenPanelReq {
    pub id: u64,
    pub side: String,
    pub size: u16,
    /// 节点焦点路由开关（Tab 移动焦点/Esc 取消/节点按键直达，复用弹窗机制）
    pub focusable: bool,
    /// 注册为侧栏 rail(贴边全高,不参与 window mode 分裂;默认 false=普通叶)
    pub rail: bool,
}

#[derive(Debug)]
pub struct ClosePanelReq {
    pub id: u64,
}

/// `helix.close_popup(id)` —— 关掉一个 JS 弹层(触发 onClose 并移除宿主层)。
#[derive(Debug)]
pub struct ClosePopupReq {
    pub id: u64,
}

#[derive(Debug)]
pub struct OpenFileReq {
    pub path: String,
    pub row: Option<u16>,
    pub col: Option<u16>,
}

/// 打开文件但**不绑定路径**(`helix.open_file(p, { scratch: true })`)。
/// 用途:内置"内容型插件"打开随软件分发的内容(如 `runtime/tutor`)——
/// 用户 `:w` 不该覆盖原始文件。单独一个请求,是为了**不改既有 `OpenFile` 的形状**
/// (否则所有构造点都要跟着改)。
#[derive(Debug)]
pub struct OpenScratchFileReq {
    pub path: String,
}

#[derive(Debug)]
pub struct FocusBufferReq {
    pub id: u64,
}

#[derive(Debug)]
pub struct MovePanelReq {
    pub id: u64,
    pub side: String,
}

#[derive(Debug)]
pub struct MapKeyReq {
    pub mode: String,
    pub key: String,
    pub command: String,
    pub unbind: bool,
}

/// 打开原生终端面板：view_id 是面板 id（open_terminal 返回值，term_feed 按它路由），
/// pty_id 是内部 spawn 的 pty 进程 id（term_write/term_resize/关闭时 kill 用）。
#[derive(Debug)]
pub struct OpenTerminalReq {
    pub view_id: u64,
    pub pty_id: u64,
    pub cmd: String,
    pub side: String,
    pub size: u16,
}

/// 把 PTY 输出块喂给对应终端视图（按 view_id 找层，找不到丢弃）
#[derive(Debug)]
pub struct TermFeedReq {
    pub view_id: u64,
    pub chunk: String,
}

/// 终端显示模式：dock / fullscreen / floating / minimized
#[derive(Debug)]
pub struct TermModeReq {
    pub view_id: u64,
    pub mode: String,
}

/// 清空终端网格
#[derive(Debug)]
pub struct TermClearReq {
    pub view_id: u64,
}

/// 导出终端全部内容（scrollback + 屏幕）到文件（持久化保存）
#[derive(Debug)]
pub struct TermSaveReq {
    pub view_id: u64,
    pub path: String,
}

/// 运行中调整终端面板尺寸（列宽或行高）
#[derive(Debug)]
pub struct TermResizeReq {
    pub view_id: u64,
    pub size: u16,
}

/// 布局树：切分活动叶子（id 预分配；面板回调已注册在 POPUPS）
#[derive(Debug)]
pub struct SplitLeafReq {
    pub id: u64,
    pub dir: String,
    pub kind: String, // "terminal" | "panel"
    pub cmd: Option<String>,
    pub size: u16,
}

#[derive(Debug)]
pub struct CloseLeafReq {
    pub id: u64,
}

#[derive(Debug)]
pub struct ZoomLeafReq {
    pub id: u64,
}

/// 把叶子浮动起来(plan ③;词汇取自 zellij 插件 API 的 `float_multiple_panes`)
#[derive(Debug)]
pub struct PaneFloatReq {
    pub id: u64,
}

/// 把浮动的叶子收回平铺(对应 zellij 的 `embed_multiple_panes`)
#[derive(Debug)]
pub struct PaneEmbedReq {
    pub id: u64,
}

/// 把浮动 pane 提到最上层(浮窗 z 序;`LayoutTree::float_raise`)
#[derive(Debug)]
pub struct PaneRaiseReq {
    pub id: u64,
}

/// pin / 取消 pin 浮动 pane(`on` 显式给出,不做隐式取反 —— 避免与快照不同步)
#[derive(Debug)]
pub struct PanePinReq {
    pub id: u64,
    pub on: bool,
}

/// 与**兄弟窗**建堆叠组(锚 = `id`)。规格 A.7:只有兄弟能堆叠,否则是无效操作
#[derive(Debug)]
pub struct StackCreateReq {
    pub id: u64,
}

/// 轮转到指定成员成为"当前显示的那个"。`member` 必须在同一组内
#[derive(Debug)]
pub struct StackActivateReq {
    pub id: u64,
    pub member: u64,
}

/// 把叶子摘出堆叠组(组剩 1 个则解散)
#[derive(Debug)]
pub struct StackRemoveReq {
    pub id: u64,
}

#[derive(Debug)]
pub struct ResizeLeafReq {
    pub id: u64,
    pub ratio: f32,
}

/// 按方向调整叶子份额（dir: h/v；delta>0 增大该叶子）
#[derive(Debug)]
pub struct ResizeLeafDirReq {
    pub id: u64,
    pub dir: String,
    pub delta: f32,
}

/// 聚焦方向邻居（dir: left/right/up/down）
#[derive(Debug)]
pub struct FocusLeafDirReq {
    pub id: u64,
    pub dir: String,
}

/// 与方向邻居交换内容（dir: left/right/up/down）
#[derive(Debug)]
pub struct SwapLeafDirReq {
    pub id: u64,
    pub dir: String,
}

/// 叶子所在 Split 恢复 50/50
#[derive(Debug)]
pub struct EqualizeLeafReq {
    pub id: u64,
}

/// 交换两个叶子的内容（组件引用互换）
#[derive(Debug)]
pub struct SwapLeavesReq {
    pub id1: u64,
    pub id2: u64,
}

/// 最小化/恢复叶子（渲染为底部标题横条，不占布局）
#[derive(Debug)]
pub struct MinimizeLeafReq {
    pub id: u64,
    pub minimized: bool,
}

/// 设置/取消叶子 fixed 标记（fixed 叶子不被 swap/resize/close/minimize/equalize）
#[derive(Debug)]
pub struct LayoutFixReq {
    pub id: u64,
    pub fixed: bool,
}

#[derive(Debug)]
pub struct FocusLeafReq {
    pub id: u64,
}

/// 把布局树序列化结果缓存到 helix-js（get_layout 读取）
#[derive(Debug)]
pub struct CacheLayoutReq {
    pub text: String,
}

/// 切换基准主题（set_theme_name；helix-term 加载 + set_theme + 清覆盖）
#[derive(Debug)]
pub struct SetThemeReq {
    pub name: String,
}

/// 按 view_id 关闭指定终端（term_close；解决 remove_type 关全部的问题）
#[derive(Debug)]
pub struct TermCloseReq {
    pub view_id: u64,
}

/// 打开文件为新的 BufferLeaf 叶子（split: "h" 水平在右 / 其余垂直在下）
#[derive(Debug)]
pub struct OpenBufferLeafReq {
    pub path: String,
    pub split: Option<String>,
}

/// 文件系统 watcher：id 由 JS 侧分配（helix.watch 返回值）；helix-term 据此建 notify watcher
#[derive(Debug)]
pub struct WatchReq {
    pub id: u64,
    pub path: String,
}

/// 停止 watcher：按 id 移除 notify watcher（JS 侧回调注册表同步移除）
#[derive(Debug)]
pub struct UnwatchReq {
    pub id: u64,
}

/// 打开插件 picker：source = 源名；columns = define 声明的列名（term 侧建表头/行宽校验）；
/// rows = define 后 run 取到的候选行（同步 items 或 Promise 续体）
#[derive(Debug)]
pub struct OpenPickerReq {
    pub source: String,
    pub columns: Vec<String>,
    pub rows: Vec<RowSpec>,
}

/// server 管理操作(JS API helix.server.* 镜像 :server;UI 面板用)
#[derive(Debug)]
pub struct ServerOpReq {
    pub op: String,
    pub arg: Option<String>,
}

/// server 面板行数据请求(JS helix.server.rows(cb) → term 算好回投到 cb)
#[derive(Debug)]
pub struct ServerListRowsReq {
    pub id: u64,
}

/// 后台 server 任务批次:一组 items 由 term 单 worker 串行执行。
/// task_id 由 JS 分配(helix.server.task 返回值);term 回传事件沿用该 id。
#[derive(Debug)]
pub struct ServerTaskReq {
    pub task_id: u64,
    pub items: Vec<ServerTaskItem>,
}

/// 插件向编辑器发起的 UI 请求(编辑器主线程取走后执行)
#[derive(Debug)]
pub enum UiRequest {
    OpenPopup(OpenPopupReq),
    OpenPanel(OpenPanelReq),
    /// 关闭当前视图(关掉最后一个视图即退出程序 ✓)—— dashboard 的 q 用 ✓
    Quit,
    /// `helix.reload_plugins()` —— **热重载插件**。为什么走队列而不是直接调:
    /// 重载会在 boa 引擎上重跑所有脚本,若发生在命令自己的 JS 栈里 ⇒ 二次借用
    /// `CONTEXT` panic(`commands.rs` 的 lazy 注释记过这个风险)。入队后由
    /// `apply_ui_requests` 在**命令边界**用 job 回调执行。
    ReloadPlugins,
    ClosePanel(ClosePanelReq),
    /// `helix.close_popup(id)`
    ClosePopup(ClosePopupReq),
    OpenFile(OpenFileReq),
    /// 打开文件但**不绑定路径**(`helix.open_file(p, { scratch: true })`)。
    /// 用途:内置"内容型插件"打开随软件分发的内容(如 `runtime/tutor`)——
    /// 用户 `:w` 不该覆盖原始文件。单独一个请求,是为了**不改既有 `OpenFile` 的形状**
    /// (否则所有构造点都要跟着改)。
    OpenScratchFile(OpenScratchFileReq),
    FocusBuffer(FocusBufferReq),
    MovePanel(MovePanelReq),
    MapKey(MapKeyReq),
    /// 打开原生终端面板：view_id 是面板 id（open_terminal 返回值，term_feed 按它路由），
    /// pty_id 是内部 spawn 的 pty 进程 id（term_write/term_resize/关闭时 kill 用）。
    OpenTerminal(OpenTerminalReq),
    /// 把 PTY 输出块喂给对应终端视图（按 view_id 找层，找不到丢弃）
    TermFeed(TermFeedReq),
    /// 终端显示模式：dock / fullscreen / floating / minimized
    TermMode(TermModeReq),
    /// 清空终端网格
    TermClear(TermClearReq),
    /// 导出终端全部内容（scrollback + 屏幕）到文件（持久化保存）
    TermSave(TermSaveReq),
    /// 运行中调整终端面板尺寸（列宽或行高）
    TermResize(TermResizeReq),
    /// 布局树：切分活动叶子（id 预分配；面板回调已注册在 POPUPS）
    SplitLeaf(SplitLeafReq),
    CloseLeaf(CloseLeafReq),
    ZoomLeaf(ZoomLeafReq),
    /// 把叶子浮动起来(plan ③;词汇取自 zellij 插件 API 的 `float_multiple_panes`)
    PaneFloat(PaneFloatReq),
    /// 把浮动的叶子收回平铺(对应 zellij 的 `embed_multiple_panes`)
    PaneEmbed(PaneEmbedReq),
    /// 把浮动 pane 提到最上层(浮窗 z 序;`LayoutTree::float_raise`)
    PaneRaise(PaneRaiseReq),
    /// pin / 取消 pin 浮动 pane(`on` 显式给出,不做隐式取反 —— 避免与快照不同步)
    PanePin(PanePinReq),
    /// 与**兄弟窗**建堆叠组(锚 = `id`)。规格 A.7:只有兄弟能堆叠,否则是无效操作
    StackCreate(StackCreateReq),
    /// 轮转到指定成员成为"当前显示的那个"。`member` 必须在同一组内
    StackActivate(StackActivateReq),
    /// 把叶子摘出堆叠组(组剩 1 个则解散)
    StackRemove(StackRemoveReq),
    Unzoom,
    ResizeLeaf(ResizeLeafReq),
    /// 按方向调整叶子份额（dir: h/v；delta>0 增大该叶子）
    ResizeLeafDir(ResizeLeafDirReq),
    /// 聚焦方向邻居（dir: left/right/up/down）
    FocusLeafDir(FocusLeafDirReq),
    /// 与方向邻居交换内容（dir: left/right/up/down）
    SwapLeafDir(SwapLeafDirReq),
    /// 叶子所在 Split 恢复 50/50
    EqualizeLeaf(EqualizeLeafReq),
    /// 交换两个叶子的内容（组件引用互换）
    SwapLeaves(SwapLeavesReq),
    /// 最小化/恢复叶子（渲染为底部标题横条，不占布局）
    MinimizeLeaf(MinimizeLeafReq),
    /// 设置/取消叶子 fixed 标记（fixed 叶子不被 swap/resize/close/minimize/equalize）
    LayoutFix(LayoutFixReq),
    FocusLeaf(FocusLeafReq),
    /// 把布局树序列化结果缓存到 helix-js（get_layout 读取）
    CacheLayout(CacheLayoutReq),
    /// 切换基准主题（set_theme_name；helix-term 加载 + set_theme + 清覆盖）
    SetTheme(SetThemeReq),
    /// 按 view_id 关闭指定终端（term_close；解决 remove_type 关全部的问题）
    TermClose(TermCloseReq),
    /// 打开文件为新的 BufferLeaf 叶子（split: "h" 水平在右 / 其余垂直在下）
    OpenBufferLeaf(OpenBufferLeafReq),
    /// 文件系统 watcher：id 由 JS 侧分配（helix.watch 返回值）；helix-term 据此建 notify watcher
    Watch(WatchReq),
    /// 停止 watcher：按 id 移除 notify watcher（JS 侧回调注册表同步移除）
    Unwatch(UnwatchReq),
    /// 打开插件 picker：source = 源名；columns = define 声明的列名（term 侧建表头/行宽校验）；
    /// rows = define 后 run 取到的候选行（同步 items 或 Promise 续体）
    OpenPicker(OpenPickerReq),
    /// server 管理操作(JS API helix.server.* 镜像 :server;UI 面板用)
    ServerOp(ServerOpReq),
    /// server 面板行数据请求(JS helix.server.rows(cb) → term 算好回投到 cb)
    ServerListRows(ServerListRowsReq),
    /// 后台 server 任务批次:一组 items 由 term 单 worker 串行执行。
    /// task_id 由 JS 分配(helix.server.task 返回值);term 回传事件沿用该 id。
    ServerTask(ServerTaskReq),
}

impl UiRequest {
    /// 本请求所属的协议版本(见 [`API_VERSION`])
    pub fn api_version(&self) -> u16 {
        API_VERSION
    }
}

/// server manager 一行(term → JS 回传)
#[derive(Debug, Clone)]
pub struct ServerRow {
    pub name: String,
    /// "lsp" | "dap" | "linter" | "formatter"
    pub kind: String,
    pub languages: Vec<String>,
    pub installed: bool,
    /// true = 本地 PATH 已有(非受管)
    pub local: bool,
    pub version: Option<String>,
    pub description: String,
    pub homepage: Option<String>,
    pub installable: bool,
    /// 受管已装且 detected 版本 != 配方 version → 可升级(行 ▲ 标记)
    pub upgradable: bool,
    /// 下载源含 {version} 占位且未固定 version → 安装/升级需用户显式版本
    pub needs_version: bool,
    /// 可执行路径:受管 = managed/bin 软链;本机 = PATH 绝对路径;缺失 = None
    pub bin: Option<String>,
    /// 下载源(archive url_template 原文;Tool/惰性 = None;仅显示用)
    pub source: Option<String>,
}

/// 后台任务的一项(JS → term):op 为 server_manager 任务名(install/update/remove/unmanage),name 为配方名
#[derive(Debug, Clone)]
pub struct ServerTaskItem {
    pub op: String,
    pub name: String,
    pub version: Option<String>,
}

/// 后台任务事件(term → JS;helix.server.task 回调收到单对象 {task_id, seq, name, kind, msg, bytes, total})
#[derive(Debug, Clone)]
pub struct ServerTaskEvent {
    /// JS 分配的 taskId(见 JS API)
    pub task_id: u64,
    /// 同一批次内从 0 递增
    pub seq: u32,
    /// 任务名(配方名)
    pub name: String,
    /// "phase" | "progress" | "done" | "error"
    pub kind: String,
    pub msg: Option<String>,
    pub bytes: Option<u64>,
    pub total: Option<u64>,
}

/// 文件系统 watcher 变更条目（JS 回调收到 [{kind, path}] 数组）
#[derive(Debug, Clone)]
pub struct WatchChange {
    /// "create" | "modify" | "delete" | "rename"
    pub kind: String,
    /// 绝对路径（rename 取目标路径）
    pub path: String,
}

/// watcher 事件通道消息：watcher id + 一批变更（notify 防抖合并后投递）
#[derive(Debug)]
pub enum WatchEvent {
    Changed(u64, Vec<WatchChange>),
}

/// 带唤醒的发送端：worker 发事件时触发宿主注册的唤醒回调（即时重绘），
pub enum TermEvent {
    Chunk(u64, String),
    /// run_async 的 Exit 携带聚合后的完整 stdout（Option）；spawn 的 Exit 为 None
    Exit(u64, i32, Option<String>),
}

/// 异步 fs 操作结果（stat 快照）
#[derive(Debug, Clone)]
pub struct FsStat {
    pub size: u64,
    pub is_dir: bool,
    /// 修改时间（Unix 秒）
    pub mtime: u64,
}

/// 异步 fs 事件（一次性 worker → 主线程；drain 后 resolve 到 JS 回调）。
/// Result 的 Err 分支携带错误字符串（回调的 err 参数）。
#[derive(Debug)]
pub enum AsyncEvent {
    FsRead(u64, std::result::Result<String, String>),
    FsWrite(u64, std::result::Result<(), String>),
    FsStat(u64, std::result::Result<FsStat, String>),
    FsGlob(u64, std::result::Result<Vec<String>, String>),
    /// read_tree 递归列表结果:(name, is_dir, path) 树序
    FsTree(
        u64,
        std::result::Result<Vec<(String, bool, String)>, String>,
    ),
}

/// 主线程 → worker 线程的控制指令（stdin 写入 / 杀进程）
pub(crate) enum TermCtrl {
    Write(String),
    Kill,
}

/// 一个进程 id 的 JS 回调集（resolve 时需要克隆出容器外调用）
#[derive(Clone)]
pub(crate) struct NodeHandlers {
    pub(crate) on_press: Option<JsValue>,
    pub(crate) on_key: Option<JsValue>,
    pub(crate) on_change: Option<JsValue>,
}
pub(crate) struct TermCallbacks {
    pub(crate) on_chunk: JsValue,
    pub(crate) on_exit: Option<JsValue>,
}

/// 按键事件的只读快照，传给 JS onKey 回调
pub struct PluginKey {
    pub name: String,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// popup_key 的返回：关闭弹窗 / 已消费按键 / 穿透给编辑器
#[derive(Debug, PartialEq, Eq)]
pub enum PopupKeyResult {
    Close,
    Handled,
    Ignored,
}

/// 弹窗回调注册表项（JsValue 是 Clone 的，popup_key 需要克隆出来调）
#[derive(Clone)]
pub(crate) struct PopupCallbacks {
    pub(crate) render: JsValue,
    pub(crate) on_key: Option<JsValue>,
    pub(crate) on_close: Option<JsValue>,
    /// 脏渲染协议的**上次内容**:回调返回 null/undefined 表示"无变化"⇒ 复用这份,不再解析。
    /// 生命周期天然安全:注销(id 移除)与 `reset_plugin_state`(整表 clear)都会丢掉它。
    pub(crate) last: Option<Content>,
    /// 上次的**输入签名**(只对 `pure` 渲染器有意义):命中则**连 JS 都不调**。
    pub(crate) last_key: Option<String>,
    /// 插件声明:渲染**只依赖宿主传入的输入**(focus + ctx{width,height} + 调用前宿主 `set_*`
    /// 喂进去的快照),不含主题/缓冲区/时间等外部状态 ⇒ 允许按输入签名缓存。见
    /// `set_component_render` 的第三个参数与 `render_component_cached`。
    pub(crate) pure: bool,
}

/// 一段带样式的文本（多 span 行模型：一行可有多段不同样式）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSpan {
    pub text: String,
    pub style: Option<String>,
}

/// render 回调返回的一行：多段样式文本（"error" 等主题 scope 名）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledLine {
    pub spans: Vec<TextSpan>,
}

impl StyledLine {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            spans: vec![TextSpan {
                text: text.into(),
                style: None,
            }],
        }
    }

    pub fn styled(text: impl Into<String>, style: impl Into<String>) -> Self {
        Self {
            spans: vec![TextSpan {
                text: text.into(),
                style: Some(style.into()),
            }],
        }
    }

    pub fn width(&self) -> usize {
        self.spans.iter().map(|s| s.text.chars().count()).sum()
    }
}

/// 组件树节点：render 返回单节点对象（含 type 字段）时解析出的布局树。
/// Text 单行（style/width 可选，flex 弹性宽占比、wrap 超宽换行）；Row 水平并排（gap 列间距，flex 弹性宽占比）；
/// Col 垂直堆叠（gap 行间距，flex 弹性高占比）；Scroll 高度裁剪容器（保留最后 height 行，offset 指定起始行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompNode {
    Text {
        spans: Vec<TextSpan>,
        width: Option<u16>,
        id: Option<String>,
        flex: Option<u16>,
        wrap: bool,
    },
    Row {
        children: Vec<CompNode>,
        gap: u16,
        flex: Option<u16>,
    },
    Col {
        children: Vec<CompNode>,
        gap: u16,
        flex: Option<u16>,
    },
    Scroll {
        children: Vec<CompNode>,
        height: u16,
        offset: Option<u16>,
    },
    /// 可聚焦按钮：Enter/Space 触发 onPress（id 必填）
    Button {
        label: Vec<TextSpan>,
        width: Option<u16>,
        id: String,
        flex: Option<u16>,
    },
    /// 带边框的容器：框是绘制原语，`children` 在**框内**按 col 布局（可含 button/input ⇒
    /// 节点焦点照旧工作）。`width`/`height` 是**外框**尺寸（含边框）。
    Box {
        children: Vec<CompNode>,
        border: BorderKind,
        /// 标题：画在上边框上（`╭ ti ─╮`）
        title: Vec<TextSpan>,
        style: Option<String>,
        width: Option<u16>,
        height: Option<u16>,
        flex: Option<u16>,
    },
    /// 可聚焦输入框：按键路由到 onKey（id 必填，value/cursor 由引擎状态渲染）
    /// multiline：true 时 Enter 换行、Up/Down 行间移动、Home/End 行级、多行渲染（单行不变）
    Input {
        value: String,
        cursor: usize,
        width: Option<u16>,
        id: String,
        flex: Option<u16>,
        multiline: bool,
    },
}

/// render 回调的返回：数组（字符串/样式对象）→ 旧行 API；单节点对象（含 type）→ 组件树
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    Lines(Vec<StyledLine>),
    Tree(CompNode),
    /// 显示列表(`helix.draw.*`):按绝对坐标发几条 op,由 `draw::rasterize` 光栅化成行。
    /// 与 `Lines`/`Tree` **并存且互不影响**(既有插件零改动)。
    Ops(Vec<DrawOp>),
}

/// 插件命令收到的只读上下文快照（由 helix-term 序列化编辑器状态得到）
pub struct CommandContext {
    pub path: Option<String>,
    pub text: String,
    pub cursor: (usize, usize),
    /// 主选区（anchor, head）行列对
    pub selection: ((usize, usize), (usize, usize)),
    /// 其它已打开 buffer 快照；渲染入口（panel/popup）显式传空
    pub docs: Vec<DocSnapshot>,
}

/// 命令/事件入口携带的其它已打开 buffer 快照（只读；路径已 canonicalize）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocSnapshot {
    pub path: String,
    pub text: String,
}

/// 状态栏钩子收到的轻量上下文（不含 doc.text，避免每帧克隆全文）
pub struct StatuslineCtx {
    pub path: Option<String>,
    pub mode: String,
    pub cursor: (usize, usize),
    pub total_lines: usize,
    pub diagnostics_error: usize,
    pub diagnostics_warning: usize,
    pub window_mode: bool,
    /// 活动窗口类型(editor/buffer/terminal/panel)——窗口模式图标用
    pub active_leaf_type: String,
    /// 活动窗口路径(editor/buffer 叶子时)
    pub active_leaf_path: Option<String>,

    /// 文档是否已修改(核心 `FileModificationIndicator` 语义:未改时也占位)
    pub modified: bool,
    /// 选区数(核心 `Selections` 的语义:多光标时显示数量)
    pub selections: usize,
    /// 主选区序号(从 0 起;核心渲染成 `{primary+1}/{count} sels`)
    pub selections_primary: usize,
    /// 是否只读(核心用**字段** `doc.readonly`)
    pub read_only: bool,
    /// 当前编码名(如 `utf-8`;UTF-8 时 JS 侧不显示,对齐核心 `enc != UTF_8`)
    pub encoding: String,
    /// 行尾符名(核心 `render_file_line_ending` 的映射:CRLF/LF/…)
    pub line_ending: String,
    /// 当前寄存器(未选中为 None;核心 `render_register` 语义)
    pub register: Option<String>,
    /// 主选区长度(字符数;核心 `render_primary_selection_length`)
    pub primary_selection_length: usize,
    /// 缩进风格文案(核心原文案: `tabs` / `4 spaces`)
    pub indent_style: String,
    /// 语言名(核心: `language_name().unwrap_or(DEFAULT_LANGUAGE_NAME)`)
    pub file_type: Option<String>,
    /// 当前工作目录的**末段名**(核心 `render_cwd`:`current_working_dir().file_name()`)
    pub cwd: String,

    /// 是否有代码动作提示(核心 `render_code_action_hint`:`focused && doc.code_action_hints(view)`)
    pub code_action_hint: bool,

    /// 工作区(全部文档)诊断计数(核心 `render_workspace_diagnostics` 按 severity 折叠)
    pub workspace_diagnostics_error: usize,
    pub workspace_diagnostics_warning: usize,
    pub workspace_diagnostics_info: usize,
    pub workspace_diagnostics_hint: usize,

    /// LSP 进度帧(spinner);无进行中任务时为 None(核心 `render_lsp_spinner`)
    pub spinner: Option<String>,
}

/// 状态栏分段：text + 可选 theme scope（null = 跟随状态栏基样式）。
/// zone 决定 replace 模式下的区域："left"（默认）| "center" | "right"（右对齐）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatuslinePart {
    pub text: String,
    pub style: Option<String>,
    pub zone: Option<String>,
}

/// 一次文档编辑请求（坐标基于命令开始时的原始快照，0-based 行列）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    /// Some(path) = 目标其它 buffer；None = 当前 buffer（现状）
    pub doc: Option<String>,
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub insert: String,
}

/// 插件装饰请求:set_virtual_text / set_highlight 入队,应用时按 doc 整体替换
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecorationKind {
    VirtualText {
        row: usize,
        col: usize,
        text: String,
        style: Option<String>,
    },
    Highlight {
        sr: usize,
        sc: usize,
        er: usize,
        ec: usize,
        style: Option<String>,
    },
    /// 清除该 doc 全部插件装饰(set_virtual_text 只传 path 时产生)
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecorationRequest {
    /// Some(canonical path) = 目标 doc;None = 清空所有 doc(仅脚本重载产生)
    pub doc: Option<String>,
    pub kind: DecorationKind,
}

/// 光标/选区请求（命令返回后由 helix-term 应用）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorRequest {
    SetCursor {
        row: usize,
        col: usize,
    },
    SetSelection {
        anchor: (usize, usize),
        head: (usize, usize),
    },
    SetSelections(Vec<((usize, usize), (usize, usize))>),
}

// boa 的 Context/JsValue 是 !Send（Rc GC 堆），不能用 static 全局共享，
// 所以引擎按线程存放（编辑器主线程是唯一调用者）；消息队列随宿主隔离（T1 步骤 2 起）。
// 进程退出阶段曾约 50% 概率 SIGABRT（glibc tcache corruption）：线程退出时
// 容器 drop → JsValue drop → boa Gc 堆对象被释放，与 teardown 时序冲突。
// 故 CONTEXT 及所有持有 JsValue 的容器（REGISTRY/POPUPS/EVENT_HANDLERS/
// BUFFER_ICON_HOOK/STATUSLINE_HOOK）一律 Box::leak 到 'static——线程退出时
// 只 drop 引用（指针），内容永不 drop，由 OS 在进程退出时回收。对 PoC 无
// 实际代价；升级 boa 后可改回正常持有。

// ══════════════════════════════════════════════════════════════════════════
// 边界协议(B-1):事件 payload 具名化 + 版本化
// ══════════════════════════════════════════════════════════════════════════
//
// 由来(2026-10-14 现状核对):事件到 JS 的"形状"此前**只存在于各 emit 点的
// `ObjectInitializer` 调用里** —— 没有一处具名声明,所以:① 改字段没有编译错,
// ② 没有任何"协议版本"可供 JS 侧做兼容分支,③ 单测只能断言"某个字段在不在"。
// 这里给**20 个白名单事件**各一个具名结构体(serde 派生 = 唯一真相),并统一带
// `api_version`。序列化后是**一层扁平对象**:`{ api_version, type, ...字段 }`。
//
// 为什么不引 protobuf/WIT:收益(跨语言 IDL)在当前规模下不抵依赖与迁移成本;
// serde 具名结构已经给出"形状有名字、改动有编译错、版本可声明"这三样。
// 见 docs/superpowers/plans/2026-10-14-js-host-phaseD-and-boundary.md §T2。

/// 边界协议的**线格式版本**。
///
/// 递增规则:事件 payload 或 `UiRequest` 的字段**增删/语义变更** ⇒ `+1`。
/// 加字段(向后兼容)也要递增 —— 版本号回答的是"对端能不能假定这份形状",不是
/// "旧端会不会崩"。JS 侧:每次事件 payload 里都带 `api_version`,插件可据此分支。
pub const API_VERSION: u16 = 1;

/// 事件 payload 的**公共头**:`{ api_version, type }`。
///
/// 每个具名 payload 都用 `#[serde(flatten)] pub hdr: BusHeader` 引入它 ⇒
/// 序列化结果里 `api_version`/`type` 与其余字段**同层**(不是嵌套子对象)。
#[derive(Debug, Clone, Serialize)]
pub struct BusHeader {
    /// 见 [`API_VERSION`]
    pub api_version: u16,
    /// 事件名(与 `commands.rs` 的 `EVENT_WHITELIST` 同名;`helix.on` 收到的第一参)
    #[serde(rename = "type")]
    pub event_type: String,
}

impl BusHeader {
    pub fn new(event_type: &str) -> Self {
        Self {
            api_version: API_VERSION,
            event_type: event_type.to_string(),
        }
    }
}

/// 0-based 行列坐标(事件 payload 里的点)
#[derive(Debug, Clone, Copy, Serialize)]
pub struct EvPos {
    pub row: usize,
    pub col: usize,
}

/// 选区(anchor/head 两点)
#[derive(Debug, Clone, Copy, Serialize)]
pub struct EvSelection {
    pub anchor: EvPos,
    pub head: EvPos,
}

/// 半开区间 `[start, end)`(行列)
#[derive(Debug, Clone, Copy, Serialize)]
pub struct EvRange {
    pub start: EvPos,
    pub end: EvPos,
}

/// 一次合并后的文档变更(与 `doc.changes` 的 `{oldRange, newRange}` **同源同形**)
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvChange {
    pub old_range: EvRange,
    pub new_range: EvRange,
}

/// `startup` —— 启动完成(发在初始文件处理之后)
#[derive(Debug, Clone, Serialize)]
pub struct StartupPayload {
    #[serde(flatten)]
    pub hdr: BusHeader,
    /// **启动时命令行传入的"要打开的东西"数量**(文件 / 目录 / `--tutor` / 管道 stdin)。
    ///
    /// 为什么需要:此时"当前 buffer"可能**没有路径**却是"用户已经指定了路径"——
    /// `hx <目录>` 会先弹文件选择器、再建一个无名空 buffer;`hx --tutor` 的 tutor buffer
    /// 被刻意 `set_path(None)`;`cat x | hx` 的 buffer 也没有路径。
    /// 只靠 `doc.path` 判断"启动时有没有打开文件"会把这三类都判错
    /// (内建 dashboard 因此在 `hx <目录>` 时弹在文件选择器上面 —— 实测 bug)。
    pub open_paths: usize,
}

/// `layout-change` —— 布局变动;`kind` 是种类名,缺种类时是 **null**(不是"没有这个字段")
#[derive(Debug, Clone, Serialize)]
pub struct LayoutChangePayload {
    #[serde(flatten)]
    pub hdr: BusHeader,
    pub kind: Option<String>,
}

/// 文档类事件(`save`/`buffer-open`/`buffer-close`/`theme-change`)——
/// 只带轻量定位信息;全文仍在位置参数里的 doc 对象上(不重复序列化正文)
#[derive(Debug, Clone, Serialize)]
pub struct DocEventPayload {
    #[serde(flatten)]
    pub hdr: BusHeader,
    pub path: Option<String>,
    pub cursor: EvPos,
    pub selection: EvSelection,
}

/// `mode-change` —— 模式切换(`mode`: normal/insert/select)
#[derive(Debug, Clone, Serialize)]
pub struct ModeChangePayload {
    #[serde(flatten)]
    pub hdr: BusHeader,
    pub mode: String,
    pub path: Option<String>,
    pub cursor: EvPos,
    pub selection: EvSelection,
}

/// `cwd-change` —— 工作目录真的变了(`:cd` / `:lcd` / `:pop-directory` 成功后发)。
///
/// 为什么需要这个事件:插件此前**只能**在加载时 `helix.run("pwd")` 问一次并缓存
/// (内建 filetree 就是 `S.root === null` 时取一次)⇒ `:cd` 之后面板永远停在启动目录。
/// `prev_cwd` 让插件能区分"同一个项目里换目录"与"整个 workspace 换了"。
#[derive(Debug, Clone, Serialize)]
pub struct CwdChangePayload {
    #[serde(flatten)]
    pub hdr: BusHeader,
    /// 变更后的工作目录(绝对路径,已 canonicalize)
    pub cwd: String,
    /// 变更前的工作目录;未知时为 null
    pub prev_cwd: Option<String>,
}

/// `doc-change` —— 防抖窗口内合并后的变更范围(`changes` 与 `doc.changes` 同源)
#[derive(Debug, Clone, Serialize)]
pub struct DocChangePayload {
    #[serde(flatten)]
    pub hdr: BusHeader,
    pub path: Option<String>,
    pub cursor: EvPos,
    pub selection: EvSelection,
    pub changes: Vec<EvChange>,
}

/// `pane-mode-change` —— 平级 pane 模式(`mode`: tabs/stack/…)
#[derive(Debug, Clone, Serialize)]
pub struct PaneModeChangePayload {
    #[serde(flatten)]
    pub hdr: BusHeader,
    pub mode: String,
}

/// `cursor-move` —— 光标移动(`doc_id` 是 helix-term 侧的 doc id)
#[derive(Debug, Clone, Serialize)]
pub struct CursorMovePayload {
    #[serde(flatten)]
    pub hdr: BusHeader,
    pub doc_id: u64,
    pub row: usize,
    pub col: usize,
    pub mode: String,
}

/// `selection-change` —— 选区数量/主选区
#[derive(Debug, Clone, Serialize)]
pub struct SelectionChangePayload {
    #[serde(flatten)]
    pub hdr: BusHeader,
    pub doc_id: u64,
    pub count: usize,
    pub primary: EvPos,
}

/// `lsp-diagnostics` —— 当前文档诊断详情(`diagnostics` 为原样透传的 JSON 数组)
#[derive(Debug, Clone, Serialize)]
pub struct LspDiagnosticsPayload {
    #[serde(flatten)]
    pub hdr: BusHeader,
    pub doc_id: u64,
    pub diagnostics: serde_json::Value,
}

/// `term-open` —— 终端面板打开
#[derive(Debug, Clone, Serialize)]
pub struct TermOpenPayload {
    #[serde(flatten)]
    pub hdr: BusHeader,
    pub pty_id: u64,
    pub cmd: String,
}

/// `term-mode-change` —— 终端显示模式(dock/fullscreen/floating/minimized)
#[derive(Debug, Clone, Serialize)]
pub struct TermModeChangePayload {
    #[serde(flatten)]
    pub hdr: BusHeader,
    pub pty_id: u64,
    pub mode: String,
}

/// `term-exit` —— 终端进程退出(`code` 为退出码)
#[derive(Debug, Clone, Serialize)]
pub struct TermExitPayload {
    #[serde(flatten)]
    pub hdr: BusHeader,
    pub pty_id: u64,
    pub code: i32,
}

/// `term-resize` —— 终端尺寸变更
#[derive(Debug, Clone, Serialize)]
pub struct TermResizePayload {
    #[serde(flatten)]
    pub hdr: BusHeader,
    pub pty_id: u64,
    pub rows: u16,
    pub cols: u16,
}

/// `term-title` —— 终端标题(OSC 标题序列)
#[derive(Debug, Clone, Serialize)]
pub struct TermTitlePayload {
    #[serde(flatten)]
    pub hdr: BusHeader,
    pub pty_id: u64,
    pub title: String,
}

/// `term-close` —— 终端关闭请求(处理器返回 false 可阻止)
#[derive(Debug, Clone, Serialize)]
pub struct TermClosePayload {
    #[serde(flatten)]
    pub hdr: BusHeader,
    pub pty_id: u64,
    pub reason: String,
}

/// `term-key` —— 终端按键(处理器返回值决定 pass/consume/minimize/close)
#[derive(Debug, Clone, Serialize)]
pub struct TermKeyPayload {
    #[serde(flatten)]
    pub hdr: BusHeader,
    pub pty_id: u64,
    pub code: String,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// `component-event` —— 组件节点上的鼠标/输入事件
#[derive(Debug, Clone, Serialize)]
pub struct ComponentEventPayload {
    #[serde(flatten)]
    pub hdr: BusHeader,
    pub id: u64,
    pub kind: String,
    pub x: u16,
    pub y: u16,
}

/// 边界总线上**全部**事件 payload 的清单(与 `EVENT_WHITELIST` 一一对应)。
///
/// `untagged`:每个变体的结构体**自带** `hdr.type`,不需要 serde 再打一层 tag。
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum EventPayload {
    Startup(StartupPayload),
    LayoutChange(LayoutChangePayload),
    Save(DocEventPayload),
    BufferOpen(DocEventPayload),
    BufferClose(DocEventPayload),
    ThemeChange(DocEventPayload),
    ModeChange(ModeChangePayload),
    CwdChange(CwdChangePayload),
    DocChange(DocChangePayload),
    PaneModeChange(PaneModeChangePayload),
    CursorMove(CursorMovePayload),
    SelectionChange(SelectionChangePayload),
    LspDiagnostics(LspDiagnosticsPayload),
    TermOpen(TermOpenPayload),
    TermModeChange(TermModeChangePayload),
    TermExit(TermExitPayload),
    TermResize(TermResizePayload),
    TermTitle(TermTitlePayload),
    TermClose(TermClosePayload),
    TermKey(TermKeyPayload),
    ComponentEvent(ComponentEventPayload),
}

impl EventPayload {
    pub fn api_version(&self) -> u16 {
        API_VERSION
    }

    /// 该 payload 在总线上的**事件名**(= `hdr.type`)
    pub fn event_name(&self) -> &str {
        match self {
            EventPayload::Startup(p) => &p.hdr.event_type,
            EventPayload::LayoutChange(p) => &p.hdr.event_type,
            EventPayload::Save(p)
            | EventPayload::BufferOpen(p)
            | EventPayload::BufferClose(p)
            | EventPayload::ThemeChange(p) => &p.hdr.event_type,
            EventPayload::ModeChange(p) => &p.hdr.event_type,
            EventPayload::CwdChange(p) => &p.hdr.event_type,
            EventPayload::DocChange(p) => &p.hdr.event_type,
            EventPayload::PaneModeChange(p) => &p.hdr.event_type,
            EventPayload::CursorMove(p) => &p.hdr.event_type,
            EventPayload::SelectionChange(p) => &p.hdr.event_type,
            EventPayload::LspDiagnostics(p) => &p.hdr.event_type,
            EventPayload::TermOpen(p) => &p.hdr.event_type,
            EventPayload::TermModeChange(p) => &p.hdr.event_type,
            EventPayload::TermExit(p) => &p.hdr.event_type,
            EventPayload::TermResize(p) => &p.hdr.event_type,
            EventPayload::TermTitle(p) => &p.hdr.event_type,
            EventPayload::TermClose(p) => &p.hdr.event_type,
            EventPayload::TermKey(p) => &p.hdr.event_type,
            EventPayload::ComponentEvent(p) => &p.hdr.event_type,
        }
    }

    /// 序列化为 JSON 对象(`{ api_version, type, ... }`)—— 发往 JS 前的线格式
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("事件 payload 必须可序列化(字段全是基础类型)")
    }
}

/// `CommandContext` → 具名 payload 的公共点/选区转换(给各 emit 点复用)
impl From<(usize, usize)> for EvPos {
    fn from((row, col): (usize, usize)) -> Self {
        Self { row, col }
    }
}

impl From<((usize, usize), (usize, usize))> for EvSelection {
    fn from((anchor, head): ((usize, usize), (usize, usize))) -> Self {
        Self {
            anchor: anchor.into(),
            head: head.into(),
        }
    }
}

// ── 插件 manifest(C-1)──────────────────────────────────────────────────
//
// `helix.plugin(name, {...})` 的**声明面**。现状它只有 `deps`/`version`,于是
// "这个插件能不能起进程 / 读写任意文件"没有任何地方写下来。C-1 只**解析 + 记录**,
// C-2 才在三个入口据它检查(检查点见 `crate::permissions`)。

/// 插件声明的能力清单(C-2 的判据)。
///
/// **未声明** = `PluginManifest::permissions == None` ⇒ legacy 语义(不拦截);
/// **声明了但没列某项** ⇒ 那项要"问一次"(见 `crate::permissions::check`)。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PluginPermissions {
    /// 起子进程 / shell(`helix.run`、`helix.spawn`、`run_async`)
    pub process: bool,
    /// 读任意文件(`read_file`/`read_dir` 及异步同族)
    pub fs_read: bool,
    /// 写文件(`write_file` 与插件自身文件的写入)
    pub fs_write: bool,
}

impl PluginPermissions {
    /// 三项全假 = 声明了"什么能力都不要"(于是任何能力都要问一次)
    pub fn none() -> Self {
        Self::default()
    }

    /// 能力名 → 是否已声明(与 `crate::permissions::CAP_*` 对应)
    pub fn allows(&self, cap: &str) -> bool {
        match cap {
            "process" => self.process,
            "fs_read" => self.fs_read,
            "fs_write" => self.fs_write,
            _ => false,
        }
    }
}

/// 一个插件在 `helix.plugin(name, {...})` 里声明的 manifest
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginManifest {
    /// 插件名(`helix.plugin` 第一参)
    pub name: String,
    /// 声明面的版本(将来字段语义变更时用它分支)
    pub schema_version: Option<u32>,
    /// 要求的最低编辑器版本(字符串比较不做,只记录 + 展示)
    pub min_editor: Option<String>,
    /// 依赖(加载参数数组;语义见 `load_script_checked`)
    pub deps: Vec<String>,
    /// 能力清单;`None` = 未声明(legacy/grandfathered)
    pub permissions: Option<PluginPermissions>,
    /// 贡献点(命令/事件/配置/schema 的名字;只记录,供 `:js-permission list` 展示)
    pub contributes: Vec<String>,
}
