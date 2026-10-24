//! 插件能力清单与检查点(C-1 字段扩展 / C-2 三个入口)。
//!
//! 由来与策略(2026-10-14 计划 §T3):`helix.plugin(name, { deps, version })` 里
//! 没有 `permissions`,于是"插件能不能起进程 / 读写任意文件"这件事**没有任何声明**;
//! 但一步到位地"拦截未声明的操作"会让现有一切插件当场全挂 ⇒ 必须**渐进**:
//!
//! 1. **C-1 只解析、只记录**:manifest 的新字段进 `PluginManifest`,不影响行为。
//! 2. **C-2 三个入口做检查**:`helix.run` / `read_file` / `write_plugin_file`。
//! 3. **fail-closed**(C-4,2026-10-21 定):
//!    - 内置插件(提供根是**用户插件目录以外**的**已知**根 ⇒ 随软件分发的 runtime)
//!      一律放行。判据是"已知根":绝对路径加载 / `load_script_named`(如
//!      `:plugin-load /abs/x.js`)没有提供根 ⇒ 按**第三方**对待(早先把这种情况判成
//!      内置 ⇒ 检查点对 `:plugin-load` 整体失效,见 `state::is_builtin_root`);
//!    - **没声明 `permissions`** 的 manifest = **空能力集** ⇒ 与"声明了但没要这项"同档:
//!      **问一次**(拒绝本次 + 记 pending)。这条替代了早期的 legacy 放行 —— 那个语义
//!      让"不写 manifest"成为绕过全部检查的办法。17 个内置插件因此都显式声明了
//!      (无受保护调用的写 `permissions: {}`);
//!    - **没有属主**(编辑器内部调用 / 不属于任何脚本的裸代码)⇒ 直接拒绝。
//!
//! "拒绝本次"而不是"静默放行"是刻意的:静默放行会让 `permissions` 变成装饰,
//! 而失败可见(错误消息 + pending 列表)才让人有机会授权。
//!
//! 端到端证据:`helix-term/tests/test/plugin_permissions.rs`(询问 → allow 放行 /
//! deny 长期生效 / 未声明 permissions 被拒 / onPress 越权被拒)。
//!
//! # 归属(哪个插件在调?)
//!
//! JS API 是全局的(`helix.run(...)`),运行时并不知道"这是谁在调"。归属按
//! **回调注册时**绑定(这是唯一可靠的时刻):
//!
//! * `helix.register_command` ⇒ 记下命令的属主;命令执行时用它;
//! * `helix.on(event, fn)` ⇒ 逐处理器记属主;事件分发时按处理器切换。
//!
//! 两者之外的路径(裸脚本顶层代码)归属为"最后声明的插件名"(加载期语义,
//! 与 `helix.plugin` 的调用顺序一致)。

use crate::host::HostState;
use std::cell::RefCell;
use std::collections::HashMap;

/// 把版本串拆成数字段(`"25.07.1 (c4831272)"` → `[25, 7, 1]`)。
///
/// 宽松但有边界:每段取**前导数字**(`"1-beta"` → 1),没有数字的段记 0;
/// 整串一个数字都没有 ⇒ `None`(调用方据此决定"拒绝"还是"跳过")。
fn version_parts(v: &str) -> Option<Vec<u64>> {
    let mut out = Vec::new();
    let mut any = false;
    for seg in v.split('.') {
        let digits: String = seg.chars().take_while(|c| c.is_ascii_digit()).collect();
        let n: u64 = digits.parse().unwrap_or(0);
        if !digits.is_empty() {
            any = true;
        }
        out.push(n);
    }
    any.then_some(out)
}

/// `min_editor` 检查:`declared`(插件要求)是否**高于** `actual`(编辑器版本)。
///
/// 返回 `Ok(())` = 可以加载;`Err(消息)` = 不该加载(缺段按 0 补,`25.7` == `25.7.0`)。
pub(crate) fn min_editor_ok(declared: &str, actual: &str) -> Result<(), String> {
    let (Some(d), Some(a)) = (version_parts(declared), version_parts(actual)) else {
        return Err(format!(
            "min_editor {declared:?} 不是版本号(应是如 \"25.7\" 的形态)"
        ));
    };
    let n = d.len().max(a.len());
    for i in 0..n {
        let dv = d.get(i).copied().unwrap_or(0);
        let av = a.get(i).copied().unwrap_or(0);
        if dv > av {
            return Err(format!(
                "需要编辑器 >= {declared},当前 {actual}(升级编辑器或换用旧版插件)"
            ));
        }
        if dv < av {
            return Ok(());
        }
    }
    Ok(())
}

use crate::types::{PluginManifest, PluginPermissions};

/// 能力名:起子进程 / shell(`helix.run`、`helix.spawn`、`run_async`)
pub const CAP_PROCESS: &str = "process";
/// 能力名:读任意文件(`read_file`/`read_dir`/异步同族)
pub const CAP_FS_READ: &str = "fs_read";
/// 能力名:写文件(`write_file` 与插件自身文件的写入)
pub const CAP_FS_WRITE: &str = "fs_write";

/// 授权状态(C-2):记住的决定 + 待询问列表。按宿主隔离。
#[derive(Debug, Default)]
pub(crate) struct PermissionState {
    /// `(插件名, 能力名)` → 允许?
    pub(crate) decisions: HashMap<(String, String), bool>,
    /// 已问过、还没决定的(去重,保持发现顺序)
    pub(crate) pending: Vec<(String, String)>,
    /// 内置插件(来自用户插件目录以外的根)——永不询问
    pub(crate) builtin: HashMap<String, bool>,
    /// 加载期归属:最后一次 `helix.plugin(name, …)` 声明的名字
    pub(crate) last_declared: Option<String>,
    /// 运行时归属:当前正在执行的回调的属主
    pub(crate) current: Option<String>,
    /// 已声明过能力清单的插件(供 `:js-permission list` 与文档)
    pub(crate) manifests: HashMap<String, PluginManifest>,
    /// 注册期绑定:命令名 → 属主插件
    pub(crate) command_owners: HashMap<String, String>,
    /// 注册期绑定:事件名 → 各处理器的属主插件(下标与处理器列表对齐)
    pub(crate) handler_owners: HashMap<String, Vec<String>>,
    /// 注册期绑定:其余回调("谁注册的 onPress/render/preview/.then")→ 属主插件。
    /// 键见 [`CbKey`]。命令与事件各有自己的表(历史原因,不动它们)。
    pub(crate) callback_owners: HashMap<CbKey, String>,
    /// **仅测试**:本夹具的脚本一律视为已授权。
    ///
    /// 为什么需要:helix-js 的单元测试直接用 `eval` 跑一段脚本去验 API 机制
    /// (pty/fs/watch/server_*),它们不是"插件",也没有 manifest。集成测试
    /// (`--features integration`)走真实路径,仍然执行 fail-closed 判定。
    pub(crate) trust_all: bool,
}

/// 回调的注册期身份。
///
/// * 按 id 的回调(popup / 异步 promise / 终端 / watch / 一次性行回调)= `(kind, id)`,
///   构造**零分配**(每帧的 render 走这条);
/// * 按名字的回调(node / picker 源)= 名字进 `name`(按键路径,一次小分配可忽略);
/// * 单例钩子(statusline / bufferline 图标 …)= `hook` 常量 + 空 name/id。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CbKey {
    kind: u8,
    id: u64,
    name: String,
}

// 回调种类(纯内部;不用 enum 是为了让 `CbKey::popup(3)` 这种构造零成本)
pub(crate) const CB_POPUP: u8 = 1;
pub(crate) const CB_NODE: u8 = 2;
pub(crate) const CB_PICKER: u8 = 3;
pub(crate) const CB_ASYNC: u8 = 4;
pub(crate) const CB_LSP: u8 = 5;
pub(crate) const CB_TERM: u8 = 6;
pub(crate) const CB_SERVER_ROWS: u8 = 7;
pub(crate) const CB_SERVER_TASK: u8 = 8;
pub(crate) const CB_WATCH: u8 = 9;
pub(crate) const CB_HOOK_STATUSLINE: u8 = 10;
pub(crate) const CB_HOOK_BUFFER_ICON: u8 = 11;
pub(crate) const CB_HOOK_COMPLETION_ICON: u8 = 12;
pub(crate) const CB_HOOK_COMPLETION_RENDER: u8 = 13;
pub(crate) const CB_HOOK_KEYMAP_HINT: u8 = 14;

impl CbKey {
    pub(crate) fn kind(&self) -> u8 {
        self.kind
    }
    pub(crate) fn id(&self) -> u64 {
        self.id
    }
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
    fn bare(kind: u8, id: u64) -> Self {
        Self {
            kind,
            id,
            name: String::new(),
        }
    }

    pub(crate) fn popup(id: u64) -> Self {
        Self::bare(CB_POPUP, id)
    }
    pub(crate) fn async_promise(id: u64) -> Self {
        Self::bare(CB_ASYNC, id)
    }
    pub(crate) fn lsp_promise(id: u64) -> Self {
        Self::bare(CB_LSP, id)
    }
    pub(crate) fn term(id: u64) -> Self {
        Self::bare(CB_TERM, id)
    }
    pub(crate) fn server_rows(id: u64) -> Self {
        Self::bare(CB_SERVER_ROWS, id)
    }
    pub(crate) fn server_task(id: u64) -> Self {
        Self::bare(CB_SERVER_TASK, id)
    }
    pub(crate) fn watch(id: u64) -> Self {
        Self::bare(CB_WATCH, id)
    }
    pub(crate) fn hook(kind: u8) -> Self {
        Self::bare(kind, 0)
    }
    pub(crate) fn node(view_id: u64, node_id: &str) -> Self {
        Self {
            kind: CB_NODE,
            id: view_id,
            name: node_id.to_string(),
        }
    }
    pub(crate) fn picker(source: &str) -> Self {
        Self {
            kind: CB_PICKER,
            id: 0,
            name: source.to_string(),
        }
    }
}

thread_local! {
    /// 宿主不可用时的兜底(纯单测路径):检查点在这些线程上直接放行,不建宿主。
    static FALLBACK: RefCell<PermissionState> = RefCell::new(PermissionState::default());
}

fn with_state<T>(f: impl FnOnce(&mut PermissionState) -> T) -> T {
    // 先试宿主里的那份(正常路径);没有宿主(纯单测/无 JS)才落到线程兜底表。
    // 用 Option<FnOnce> 让闭包只在一条分支上被消费。
    let mut f = Some(f);
    let host = crate::host::try_with_permission_state(|s| (f.take().expect("只调一次"))(s));
    match host {
        Some(v) => v,
        None => FALLBACK.with(|s| (f.take().expect("上面没消费过"))(&mut s.borrow_mut())),
    }
}

/// C-1:`helix.plugin(name, {...})` 声明的 manifest 记下来(只记录,不拦截)
pub(crate) fn record_manifest(manifest: PluginManifest) {
    with_state(|s| {
        s.last_declared = Some(manifest.name.clone());
        s.manifests.insert(manifest.name.clone(), manifest);
    });
}

/// 已声明过 manifest 的插件名(卸载时按它逐个回收)
pub(crate) fn manifest_names() -> Vec<String> {
    with_state(|s| s.manifests.keys().cloned().collect())
}

/// 加载完成后标注"这个插件是不是内置的"(由 `load_script_checked` 在知道
/// **提供根**时调用 —— 归属判据是根,不是名字)
pub(crate) fn mark_builtin(name: &str, builtin: bool) {
    with_state(|s| {
        s.builtin.insert(name.to_string(), builtin);
    });
}

/// 加载期归属:最后一次 `helix.plugin(name, …)` 声明的名字
pub(crate) fn current_declared() -> Option<String> {
    with_state(|s| s.last_declared.clone())
}

/// 进入新脚本 eval 前清标记(用来回答"**本次** eval 有没有声明 manifest")
pub(crate) fn begin_manifest() {
    with_state(|s| s.last_declared = None);
}

/// 设当前属主,返回上一个(供恢复)
pub(crate) fn set_current_owner(name: Option<&str>) -> Option<String> {
    with_state(|s| {
        let prev = s.current.clone();
        s.current = name.map(str::to_string);
        prev
    })
}

/// 恢复属主
pub(crate) fn restore_owner(prev: Option<String>) {
    with_state(|s| s.current = prev);
}

/// 注册期绑定:这个回调是谁注册的(不传 owner = 取当前属主)。
///
/// 为什么需要:权限检查点看的是"当前属主",而属主此前只在命令/事件两个入口设置;
/// render / onPress / preview / Promise 续体全都落空 ⇒ 判定会落到"加载期最后声明的
/// 那个插件"身上(等于拿 A 的权限判 B)。这里把归属钉在**注册**时刻 —— 那是唯一
/// 必然知道"谁在注册"的时刻。
pub(crate) fn bind_callback(key: CbKey) {
    let owner = current_owner();
    with_state(|s| {
        if let Some(owner) = owner {
            s.callback_owners.insert(key, owner);
        } else {
            s.callback_owners.remove(&key);
        }
    });
}

/// 显式绑定到某个属主(加载期按脚本名归属用)
pub(crate) fn bind_callback_to(key: CbKey, owner: &str) {
    let owner = owner.to_string();
    with_state(|s| {
        s.callback_owners.insert(key, owner);
    });
}

/// 该回调的属主(注册期记录的;没有 ⇒ None,权限检查会按"无上下文"拒绝)
pub(crate) fn callback_owner(key: &CbKey) -> Option<String> {
    with_state(|s| s.callback_owners.get(key).cloned())
}

/// 注销一条绑定(回调随组件/进程一起消失时调;不清会让 id 复用带错属主)
pub(crate) fn forget_callback(key: &CbKey) {
    with_state(|s| {
        s.callback_owners.remove(key);
    });
}

/// 事件处理器属主表长度(测试断言"两张表等长"用)
#[cfg(test)]
pub(crate) fn handler_owner_len(event: &str) -> usize {
    with_state(|s| s.handler_owners.get(event).map(|v| v.len()).unwrap_or(0))
}

/// 在"该回调的属主"作用域里执行 `f`。
///
/// 用 drop 守卫恢复:回调 panic 也不能把属主留在原地(否则后续检查全按它判)。
/// 嵌套安全:`f` 里再进一层会把属主换掉、退出时恢复成外层。
pub(crate) fn with_callback_owner<T>(key: &CbKey, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<String>);
    impl Drop for Restore {
        fn drop(&mut self) {
            restore_owner(self.0.take());
        }
    }
    let found = callback_owner(key);
    let prev = set_current_owner(found.as_deref());
    let _guard = Restore(prev);
    f()
}

/// 加载期归属:该脚本声明了哪个插件名(供 `load_script_checked` 打内置标记)
pub(crate) fn last_declared() -> Option<String> {
    with_state(|s| s.last_declared.clone())
}

/// 当前属主(**只看** `current`,不看 `last_declared` —— 两者的语义在 C-3 分开)。
pub(crate) fn current_owner() -> Option<String> {
    with_state(|s| s.current.clone())
}

/// 注册期绑定:命令名 → 属主插件
pub(crate) fn bind_command_owner(command: &str, owner: &str) {
    with_state(|s| {
        s.command_owners
            .insert(command.to_string(), owner.to_string());
    })
}

/// 注册期绑定:事件处理器(按注册序号)→ 属主插件
pub(crate) fn bind_handler_owner(event: &str, owner: &str) {
    with_state(|s| {
        s.handler_owners
            .entry(event.to_string())
            .or_default()
            .push(owner.to_string());
    })
}

/// 命令的属主(运行时归属用)
pub(crate) fn command_owner(command: &str) -> Option<String> {
    with_state(|s| s.command_owners.get(command).cloned())
}

/// 事件第 `idx` 个处理器的属主
pub(crate) fn handler_owner(event: &str, idx: usize) -> Option<String> {
    with_state(|s| {
        s.handler_owners
            .get(event)
            .and_then(|v| v.get(idx).cloned())
    })
}

impl PermissionState {
    // (无方法:提供者是 `with_state` — 内部可变性收敛在一处)
}

/// 当前归属插件(运行时优先,其次加载期)
fn current_plugin() -> Option<String> {
    // C-3:**只看执行期属主**。早先这里回退 `last_declared`（加载期最后一次
    // `helix.plugin()` 的名字）—— 于是所有"没设属主"的入口（render/onPress/preview/
    // promise 续体）都被按那个插件判定：最后加载的是内置插件时 = 直接放行。
    // 加载期归属现在由 `load_script_checked` 按**脚本名**显式设置（见 `plugin_name_from_key`）。
    with_state(|s| s.current.clone())
}

/// 脚本 key → 插件名（加载期属主）。
///
/// 规则（与 `docs/plugin-layout.md` §3.3 的布局约定一致）：
/// * `<name>/plugin.js` ⇒ `name`；
/// * 绝对路径且末段是 `plugin.js` ⇒ 其目录名；
/// * 其它（`init.js`、被 `deps` 加载的 `lib/x.js`、单文件脚本）⇒ `None`，
///   调用方**继承父属主**（库是引用它的那个插件的一部分，不该自成一个插件）。
pub(crate) fn plugin_name_from_key(key: &str) -> Option<String> {
    let p = std::path::Path::new(key);
    let is_plugin_entry = p.file_name().map(|f| f == "plugin.js").unwrap_or(false);
    if !is_plugin_entry {
        return None;
    }
    p.parent()
        .and_then(|d| d.file_name())
        .map(|n| n.to_string_lossy().into_owned())
}

/// `unload_plugin` 从属主表里摘下来的东西(**不含** JS 注册表本身 —— 那在 `commands`/`state`)
pub(crate) struct OwnerCleanup {
    /// 属于该插件的命令名(调用方负责从 registry / command_docs 删)
    pub(crate) commands: Vec<String>,
    /// 属于该插件的回调键(弹窗/面板/节点/picker/钩子)
    pub(crate) callbacks: Vec<CbKey>,
    /// 被摘掉的事件处理器条数
    pub(crate) handlers: usize,
}

/// 摘掉某个属主的全部注册期绑定(命令 / 事件处理器 / 其余回调 / 待授权 / manifest)。
///
/// **事件处理器的属主表按下标对齐** ⇒ 这里先把要删的下标算出来,再对
/// `handler_owners` 与 `event_handlers` **同一组下标、倒序**删(绝不按值 retain 单边,
/// 那会让后续下标错位 —— 卸 A 删掉 B)。
pub(crate) fn take_owner_registrations(st: &HostState, owner: &str) -> OwnerCleanup {
    with_state(|p| {
        let commands: Vec<String> = p
            .command_owners
            .iter()
            .filter(|(_, o)| o.as_str() == owner)
            .map(|(c, _)| c.clone())
            .collect();
        for c in &commands {
            p.command_owners.remove(c);
        }
        let callbacks: Vec<CbKey> = p
            .callback_owners
            .iter()
            .filter(|(_, o)| o.as_str() == owner)
            .map(|(k, _)| k.clone())
            .collect();
        for k in &callbacks {
            p.callback_owners.remove(k);
        }
        // 事件:先算下标,双侧同步
        let mut per_event: Vec<(String, Vec<usize>)> = Vec::new();
        for (event, owners) in p.handler_owners.iter() {
            let idxs: Vec<usize> = owners
                .iter()
                .enumerate()
                .filter(|(_, o)| o.as_str() == owner)
                .map(|(i, _)| i)
                .collect();
            if !idxs.is_empty() {
                per_event.push((event.clone(), idxs));
            }
        }
        let mut handlers = 0usize;
        for (event, idxs) in &per_event {
            handlers += idxs.len();
            crate::state::with_event_handlers(st, |m| {
                if let Some(list) = m.get_mut(event) {
                    for &i in idxs.iter().rev() {
                        if i < list.len() {
                            list.remove(i);
                        }
                    }
                }
            });
            if let Some(owners) = p.handler_owners.get_mut(event) {
                for &i in idxs.iter().rev() {
                    if i < owners.len() {
                        owners.remove(i);
                    }
                }
            }
        }
        // 元数据:manifest / 待授权 / 加载期与运行期归属
        p.manifests.remove(owner);
        p.pending.retain(|(plugin, _)| plugin != owner);
        if p.last_declared.as_deref() == Some(owner) {
            p.last_declared = None;
        }
        if p.current.as_deref() == Some(owner) {
            p.current = None;
        }
        OwnerCleanup {
            commands,
            callbacks,
            handlers,
        }
    })
}

/// 放行/拒绝一次的判定(纯函数,便于单测直接打表)
///
/// 返回:`None` = 不需要询问(放行或不需要问);`Some(plugin)` = 需要问该插件
fn verdict(ctx: &RequestCtx, cap: &str) -> Verdict {
    if ctx.builtin {
        return Verdict::Allow;
    }
    // fail-closed:没声明 permissions = **空能力集**(早先是 legacy 放行 —— 那让
    // "不写 manifest"成为绕过全部检查的办法)。
    let declared = ctx.permissions.is_some_and(|p| p.allows(cap));
    if declared {
        return Verdict::Allow;
    }
    // 用户显式记住的决定覆盖"没声明"(老插件授权一次后继续可用)
    match ctx.decision {
        Some(true) => Verdict::Allow,
        Some(false) => Verdict::Denied,
        None => Verdict::Ask,
    }
}

/// 判定所需的一切(把"读状态"与"判定"分开 ⇒ 判定可单测,不必搭引擎)
pub(crate) struct RequestCtx {
    pub(crate) plugin: String,
    pub(crate) builtin: bool,
    pub(crate) permissions: Option<PluginPermissions>,
    pub(crate) decision: Option<bool>,
}

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Allow,
    Denied,
    Ask,
}

/// **检查点**(C-2):几个入口在真正做事之前调它。
///
/// 现在接上的入口(**同一能力覆盖同族 API**,否则"声明了 process"只拦 `run` 就是误导):
/// * `process` —— `helix.run` / `helix.run_async` / `helix.spawn`;
/// * `fs_read` —— `helix.read_file` / `helix.read_file_async` / `helix.read_dir` /
///   `helix.stat_async` / `helix.glob_async` / `helix.read_tree` / `helix.watch`;
/// * `fs_write` —— `helix.write_file` / `helix.write_file_async` / `helix.write_plugin_file` /
///   `helix.remove_plugin_file`。
///
/// * `Ok(())` = 放行;
/// * `Err(_)` = 拒绝,且消息足够让人知道下一步怎么做(谁、哪个能力、怎么授权)。
pub(crate) fn check(cap: &str) -> anyhow::Result<()> {
    // 没有宿主(纯单测 / 无 JS 场景)⇒ 不建宿主、不拦
    if !crate::host::engine_exists() {
        return Ok(());
    }
    if cfg!(test) && with_state(|s| s.trust_all) {
        return Ok(());
    }
    let Some(plugin) = current_plugin() else {
        // 归属未知 = 没有插件上下文(编辑器内部调用 / 未归到任何脚本的裸代码)。
        // fail-closed:拒绝并说清原因,而不是"猜一个插件"或"静默放行"。
        return Err(anyhow::anyhow!(
            "没有插件上下文,拒绝 '{cap}':受保护 API 只能在插件回调/命令/加载期脚本里调用"
        ));
    };
    let (builtin, permissions, decision) = with_state(|s| {
        (
            *s.builtin.get(&plugin).unwrap_or(&false),
            // 没有 manifest 或 manifest 里没写 `permissions` ⇒ legacy(grandfathered)
            s.manifests.get(&plugin).and_then(|m| m.permissions),
            s.decisions.get(&(plugin.clone(), cap.to_string())).copied(),
        )
    });
    let ctx = RequestCtx {
        plugin: plugin.clone(),
        builtin,
        permissions,
        decision,
    };
    match verdict(&ctx, cap) {
        Verdict::Allow => Ok(()),
        Verdict::Denied => Err(anyhow::anyhow!(
            "插件 '{}' 的能力 '{cap}' 已被拒绝(:js-permission allow {} {cap} 可改)",
            ctx.plugin,
            ctx.plugin
        )),
        Verdict::Ask => {
            let first = with_state(|s| {
                let kv = (ctx.plugin.clone(), cap.to_string());
                if s.pending.contains(&kv) {
                    false
                } else {
                    s.pending.push(kv);
                    true
                }
            });
            if first {
                // 让失败可见:状态栏也会看到这条(与 check 的 Err 一起)
                crate::host::with_messages(|m| {
                    m.push(format!(
                        "插件 '{}' 请求能力 '{cap}'(未在 manifest 声明):\
                         用 :js-permission allow {} {cap} 授权,或 deny 拒绝",
                        ctx.plugin, ctx.plugin
                    ))
                });
            }
            Err(anyhow::anyhow!(
                "插件 '{}' 未声明能力 '{cap}' —— 已记入待授权列表(首次询问)",
                ctx.plugin
            ))
        }
    }
}

// 检查点的 JS 绑定侧包装(`check_js`)已删除(2026-10-22):
// 能力现在是 `api::API` 的字段,由 `api::native()` 在**派发点**统一检查 —— 实现里再调一次
// 属于重复,而"忘记调用"这个失败模式已经不存在了。非表入口(如果有)请直接用 `check()`。

// ── helix-term 侧接口(:js-permission 命令)─────────────────────────────

/// 记住一个决定(允许/拒绝)。返回是否真的改变了什么。
pub fn decide(plugin: &str, cap: &str, allow: bool) -> bool {
    with_state(|s| {
        s.pending.retain(|(p, c)| !(p == plugin && c == cap));
        s.decisions
            .insert((plugin.to_string(), cap.to_string()), allow)
            .is_none()
    })
}

/// **已记住的决定**(插件, 能力, 是否允许),按插件名+能力名排序 ⇒ `:js-permission list`
/// 的输出稳定可断言。
pub fn decisions() -> Vec<(String, String, bool)> {
    with_state(|s| {
        let mut v: Vec<(String, String, bool)> = s
            .decisions
            .iter()
            .map(|((p, c), allow)| (p.clone(), c.clone(), *allow))
            .collect();
        v.sort();
        v
    })
}

/// 待询问列表(供状态栏/`:js-permission list` 展示)
pub fn pending() -> Vec<(String, String)> {
    with_state(|s| s.pending.clone())
}

/// 已知的 manifest(名 → manifest);供 `:js-permission list` 展示声明面
pub fn manifests() -> Vec<PluginManifest> {
    with_state(|s| {
        let mut v: Vec<_> = s.manifests.values().cloned().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    })
}

/// 某个能力**现在**会不会被放行(不产生副作用:不问、不记 pending)。
///
/// 语义 = "不问就放行吗":内置插件 ⇒ true;其余看声明或已记住的决定
/// (未声明 / 未决定 ⇒ false —— 调用会被问一次)。无归属 ⇒ false(fail-closed)。
pub(crate) fn allowed_now(cap: &str) -> bool {
    if !crate::host::engine_exists() {
        return true;
    }
    if cfg!(test) && with_state(|s| s.trust_all) {
        return true;
    }
    let Some(plugin) = current_plugin() else {
        return false;
    };
    let (builtin, permissions, decision) = with_state(|s| {
        (
            *s.builtin.get(&plugin).unwrap_or(&false),
            s.manifests.get(&plugin).and_then(|m| m.permissions),
            s.decisions.get(&(plugin.clone(), cap.to_string())).copied(),
        )
    });
    match verdict(
        &RequestCtx {
            plugin,
            builtin,
            permissions,
            decision,
        },
        cap,
    ) {
        Verdict::Allow => true,
        Verdict::Denied | Verdict::Ask => false,
    }
}

/// `helix.permissions()`(JS API):当前插件**现在**能做什么。
///
/// 用途:插件据此降级(例如没有 `process` 就不去调 git,而不是先失败一次)。
/// 返回值:`{ process, fs_read, fs_write }`,语义见 [`allowed_now`]。
pub(crate) fn js_permissions(
    _this: &boa_engine::JsValue,
    _args: &[boa_engine::JsValue],
    ctx: &mut boa_engine::Context,
) -> boa_engine::JsResult<boa_engine::JsValue> {
    use boa_engine::object::ObjectInitializer;
    use boa_engine::property::Attribute;
    use boa_engine::{JsString, JsValue};
    let flag = |cap: &str| JsValue::from(allowed_now(cap));
    Ok(JsValue::from(
        ObjectInitializer::new(ctx)
            .property(
                JsString::from(CAP_PROCESS),
                flag(CAP_PROCESS),
                Attribute::all(),
            )
            .property(
                JsString::from(CAP_FS_READ),
                flag(CAP_FS_READ),
                Attribute::all(),
            )
            .property(
                JsString::from(CAP_FS_WRITE),
                flag(CAP_FS_WRITE),
                Attribute::all(),
            )
            .build(),
    ))
}

/// 清空(测试用;宿主随线程丢弃时等价)
#[cfg(test)]
pub(crate) fn reset_for_test() {
    with_state(|s| {
        s.decisions.clear();
        s.pending.clear();
        s.builtin.clear();
        s.current = None;
        s.last_declared = None;
        s.manifests.clear();
        s.command_owners.clear();
        s.handler_owners.clear();
        s.callback_owners.clear();
        s.trust_all = false;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(process: bool, fs_read: bool, fs_write: bool) -> PluginPermissions {
        PluginPermissions {
            process,
            fs_read,
            fs_write,
        }
    }

    /// C-2 判定表(纯函数):内置 / 未声明 / 已声明+已授权 / 已声明+未授权 / 已拒绝
    #[test]
    fn verdict_table() {
        let mk = |builtin, permissions, decision| RequestCtx {
            plugin: "x".into(),
            builtin,
            permissions,
            decision,
        };
        // 内置插件永不询问(17 个内置插件 grandfathered 的核心)
        assert_eq!(
            verdict(&mk(true, Some(p(false, false, false)), None), CAP_PROCESS),
            Verdict::Allow
        );
        // 没声明 permissions ⇒ 空能力集 ⇒ 问一次(fail-closed)
        assert_eq!(verdict(&mk(false, None, None), CAP_PROCESS), Verdict::Ask);
        // 但已记住的 allow 仍然覆盖它(用户显式授权过的老插件继续可用)
        assert_eq!(
            verdict(&mk(false, None, Some(true)), CAP_PROCESS),
            Verdict::Allow
        );
        // 声明了 process 但没声明 fs_write
        assert_eq!(
            verdict(&mk(false, Some(p(true, false, false)), None), CAP_PROCESS),
            Verdict::Allow
        );
        assert_eq!(
            verdict(&mk(false, Some(p(true, false, false)), None), CAP_FS_WRITE),
            Verdict::Ask
        );
        // 记住的决定覆盖"没声明"
        assert_eq!(
            verdict(
                &mk(false, Some(p(true, false, false)), Some(true)),
                CAP_FS_WRITE
            ),
            Verdict::Allow
        );
        assert_eq!(
            verdict(
                &mk(false, Some(p(true, false, false)), Some(false)),
                CAP_FS_WRITE
            ),
            Verdict::Denied
        );
    }

    /// `min_editor` 比较(纯函数):缺段按 0 补、宽松取前导数字、非法版本号**拒绝**
    #[test]
    fn min_editor_comparison() {
        // 编辑器 25.07.1(= [25,7,1];真实版本串带 git hash)
        assert!(min_editor_ok("25.7", "25.07.1 (c4831272)").is_ok());
        assert!(min_editor_ok("25", "25.0.0").is_ok());
        assert!(min_editor_ok("25.7.1", "25.7.1").is_ok());
        assert!(min_editor_ok("25.7.0", "25.7").is_ok(), "缺段按 0 补");
        assert!(min_editor_ok("25.8", "25.7.1").is_err(), "高于当前 ⇒ 拒");
        assert!(min_editor_ok("26", "25.99.99").is_err());
        assert!(
            min_editor_ok("25.7.1", "25.7").is_err(),
            "位数多且更高 ⇒ 拒"
        );
        // 非法版本号:报错(而不是静默跳过 —— 静默会让字段变装饰)
        let e = min_editor_ok("second", "25.7.1").unwrap_err();
        assert!(e.contains("不是版本号"), "{e}");
    }

    /// `PluginPermissions::allows` 与能力名一一对应
    #[test]
    fn permissions_allows_named_caps() {
        let only_read = PluginPermissions {
            process: false,
            fs_read: true,
            fs_write: false,
        };
        assert!(only_read.allows(CAP_FS_READ));
        assert!(!only_read.allows(CAP_FS_WRITE));
        assert!(!only_read.allows(CAP_PROCESS));
        assert!(!PluginPermissions::none().allows(CAP_FS_READ));
    }

    /// 注册期绑定的属主在调用期生效，退出后恢复外层（嵌套安全）
    #[test]
    fn callback_owner_is_bound_at_registration_and_restored() {
        reset_for_test();
        set_current_owner(Some("filetree"));
        bind_callback(CbKey::popup(7));
        set_current_owner(Some("evil"));

        let seen = with_callback_owner(&CbKey::popup(7), current_owner);
        assert_eq!(seen.as_deref(), Some("filetree"), "回调内应是注册期属主");
        assert_eq!(current_owner().as_deref(), Some("evil"), "退出后恢复外层");
    }

    /// 没绑定过的回调 = 无属主（权限检查据此 fail-closed，而不是猜一个）
    #[test]
    fn unbound_callback_has_no_owner() {
        reset_for_test();
        assert!(callback_owner(&CbKey::node(1, "b")).is_none());
        assert!(with_callback_owner(&CbKey::node(1, "b"), current_owner).is_none());
    }

    /// panic 也不能把属主留在原地（drop 守卫）
    #[test]
    fn callback_owner_restored_after_panic() {
        reset_for_test();
        set_current_owner(Some("filetree"));
        bind_callback(CbKey::popup(9));
        set_current_owner(Some("evil"));
        let _ = std::panic::catch_unwind(|| {
            with_callback_owner(&CbKey::popup(9), || panic!("boom"));
        });
        assert_eq!(current_owner().as_deref(), Some("evil"));
    }
}
