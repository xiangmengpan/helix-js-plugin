use boa_engine::JsValue;
/// 插件向编辑器发起的 UI 请求（编辑器主线程取走后执行）
#[derive(Debug)]
pub enum UiRequest {
    OpenPopup {
        id: u64,
        width: Option<u16>,
        height: Option<u16>,
        position: Option<(u16, u16)>,
    },
    OpenPanel {
        id: u64,
        side: String,
        size: u16,
    },
    ClosePanel {
        id: u64,
    },
    OpenFile {
        path: String,
    },
    MovePanel {
        id: u64,
        side: String,
    },
    MapKey { mode: String, key: String, command: String },
    /// 打开原生终端面板：view_id 是面板 id（open_terminal 返回值，term_feed 按它路由），
    /// pty_id 是内部 spawn 的 pty 进程 id（term_write/term_resize/关闭时 kill 用）。
    OpenTerminal {
        view_id: u64,
        pty_id: u64,
        cmd: String,
        side: String,
        size: u16,
    },
    /// 把 PTY 输出块喂给对应终端视图（按 view_id 找层，找不到丢弃）
    TermFeed {
        view_id: u64,
        chunk: String,
    },
    /// 终端显示模式：dock / fullscreen / floating / minimized
    TermMode {
        view_id: u64,
        mode: String,
    },
    /// 清空终端网格
    TermClear {
        view_id: u64,
    },
    /// 运行中调整终端面板尺寸（列宽或行高）
    TermResize {
        view_id: u64,
        size: u16,
    },
    /// 布局树：切分活动叶子（id 预分配；面板回调已注册在 POPUPS）
    SplitLeaf {
        id: u64,
        dir: String,
        kind: String, // "terminal" | "panel"
        cmd: Option<String>,
        size: u16,
    },
    CloseLeaf { id: u64 },
    ZoomLeaf { id: u64 },
    Unzoom,
    ResizeLeaf { id: u64, ratio: f32 },
    FocusLeaf { id: u64 },
    /// 把布局树序列化结果缓存到 helix-js（get_layout 读取）
    CacheLayout(String),
    /// 切换基准主题（set_theme_name；helix-term 加载 + set_theme + 清覆盖）
    SetTheme { name: String },
    /// 按 view_id 关闭指定终端（term_close；解决 remove_type 关全部的问题）
    TermClose { view_id: u64 },
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
}
pub(crate) struct TermCallbacks {
    pub(crate) on_chunk: JsValue,
    pub(crate) on_exit: Option<JsValue>,
    /// true = run_async（Exit 回调签名为 (err, out)）；false = spawn（签名为 (code)）
    pub(crate) is_run_async: bool,
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
        Self { spans: vec![TextSpan { text: text.into(), style: None }] }
    }

    pub fn styled(text: impl Into<String>, style: impl Into<String>) -> Self {
        Self { spans: vec![TextSpan { text: text.into(), style: Some(style.into()) }] }
    }

    pub fn width(&self) -> usize {
        self.spans.iter().map(|s| s.text.chars().count()).sum()
    }
}

/// 组件树节点：render 返回单节点对象（含 type 字段）时解析出的布局树。
/// Text 单行（style/width 可选）；Row 水平并排（gap 列间距）；Col 垂直堆叠（gap 行间距）；
/// Scroll 高度裁剪容器（保留最后 height 行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompNode {
    Text { spans: Vec<TextSpan>, width: Option<u16>, id: Option<String> },
    Row { children: Vec<CompNode>, gap: u16 },
    Col { children: Vec<CompNode>, gap: u16 },
    Scroll { children: Vec<CompNode>, height: u16 },
    /// 可聚焦按钮：Enter/Space 触发 onPress（id 必填）
    Button { label: Vec<TextSpan>, width: Option<u16>, id: String },
    /// 可聚焦输入框：按键路由到 onKey（id 必填，value 由 JS 侧状态渲染）
    Input { value: String, width: Option<u16>, id: String },
}

/// render 回调的返回：数组（字符串/样式对象）→ 旧行 API；单节点对象（含 type）→ 组件树
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    Lines(Vec<StyledLine>),
    Tree(CompNode),
}

/// 插件命令收到的只读上下文快照（由 helix-term 序列化编辑器状态得到）
pub struct CommandContext {
    pub path: Option<String>,
    pub text: String,
    pub cursor: (usize, usize),
    /// 主选区（anchor, head）行列对
    pub selection: ((usize, usize), (usize, usize)),
}

/// 状态栏钩子收到的轻量上下文（不含 doc.text，避免每帧克隆全文）
pub struct StatuslineCtx {
    pub path: Option<String>,
    pub mode: String,
    pub cursor: (usize, usize),
    pub total_lines: usize,
    pub diagnostics_error: usize,
    pub diagnostics_warning: usize,
}

/// 状态栏分段：text + 可选 theme scope（null = 跟随状态栏基样式）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatuslinePart {
    pub text: String,
    pub style: Option<String>,
}

/// 一次文档编辑请求（坐标基于命令开始时的原始快照，0-based 行列）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub insert: String,
}

/// 光标/选区请求（命令返回后由 helix-term 应用）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorRequest {
    SetCursor { row: usize, col: usize },
    SetSelection { anchor: (usize, usize), head: (usize, usize) },
}

// boa 的 Context/JsValue 是 !Send（Rc GC 堆），不能用 static 全局共享，
// 所以引擎按线程存放（编辑器主线程是唯一调用者）；MESSAGES 跨线程共享。
// 进程退出阶段曾约 50% 概率 SIGABRT（glibc tcache corruption）：线程退出时
// 容器 drop → JsValue drop → boa Gc 堆对象被释放，与 teardown 时序冲突。
// 故 CONTEXT 及所有持有 JsValue 的容器（REGISTRY/POPUPS/EVENT_HANDLERS/
// BUFFER_ICON_HOOK/STATUSLINE_HOOK）一律 Box::leak 到 'static——线程退出时
// 只 drop 引用（指针），内容永不 drop，由 OS 在进程退出时回收。对 PoC 无
// 实际代价；升级 boa 后可改回正常持有。
