use crate::host::HostState;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use boa_engine::object::builtins::{JsArray, JsFunction};
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::NativeFunction;
use boa_engine::{Context, JsString, JsValue, Source};

/// 事件名白名单：helix.on 只接受这些事件。
/// 通知型（save/buffer-*/theme-* 等）用 emit_event；终端钩子（term-*）用 emit_hook，
/// 其中 term-key/term-close 的返回值参与决策（见 emit_term_key / emit_hook）。
pub(crate) const EVENT_WHITELIST: [&str; 31] = [
    "save",
    "mode-change",
    "cwd-change", // 工作目录真的变了（:cd / :lcd / :pop-directory）——插件据此重取根目录
    "buffer-open",
    "buffer-close",
    "doc-change",
    "theme-change",
    "term-open",
    "term-mode-change",
    "term-exit",
    "term-close",
    "term-resize",
    "term-title",
    "term-key",
    "component-event",
    "lsp-diagnostics",
    "cursor-move",
    "selection-change",
    "pane-mode-change",
    "layout-change",
    "startup", // 启动完成（发在 application.rs 的初始文件处理之后；由插件自行判断“是否空白无名 buffer”）
    "dap-session",
    "dap-stopped",
    "dap-continued",
    "dap-exited",
    "dap-terminated",
    "dap-output",
    "dap-thread",
    "dap-breakpoint",
    "dap-breakpoints",
    "dap-progress",
];

/// 事件白名单（`api::describe()` 与对账脚本用；唯一事实仍是上面那张表）
pub(crate) fn event_whitelist() -> Vec<&'static str> {
    EVENT_WHITELIST.to_vec()
}

use crate::state::{
    with_engine, with_event_handlers, with_last_export, with_registry, with_script_exports,
    UI_REQUESTS,
};

use crate::types::*;

pub(crate) fn js_register_command(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let func = args.get(1).cloned().unwrap_or(JsValue::undefined());
    if name.is_empty() || name.chars().any(char::is_whitespace) {
        return Err(crate::js_err(format!("invalid command name: {name:?}")));
    }
    // 先校验/存入 doc 再注册：第三参类型非法（如 42）时整体失败，不产生半注册
    if let Some(doc_arg) = args.get(2) {
        if !doc_arg.is_null_or_undefined() {
            let doc: String = doc_arg.try_js_into(context)?;
            crate::state::with_command_docs(crate::host::host_state(context), |d| {
                d.insert(name.clone(), doc)
            });
        }
    }
    // C-2 归属:命令→属主插件(注册期绑定,这是唯一可靠的时刻)。
    // 优先**运行期属主**(回调里注册的命令),再退回"本次 eval 声明的名字"(加载期)。
    if let Some(owner) = crate::permissions::current_owner() {
        crate::permissions::bind_command_owner(&name, &owner);
    } else if let Some(owner) = crate::permissions::current_declared() {
        crate::permissions::bind_command_owner(&name, &owner);
    }
    with_registry(crate::host::host_state(context), |r| r.insert(name, func));
    Ok(JsValue::undefined())
}

/// 设置插件目录（js_load 相对名解析用）。OnceLock 只生效一次：helix-term 启动时调用。
pub fn set_plugins_dir(dir: PathBuf) {
    // 单一目录 = 只有一个根(保留这个入口:测试与旧调用点都用它)
    crate::host::with_state_or_init(|st| crate::state::set_plugin_roots(st, vec![dir]));
}

/// helix.load(name)：从插件目录加载脚本（相对名或绝对路径），返回其 helix.export 的值；
/// 重复加载返回缓存对象。`.js` 后缀强制（无则补）。文件缺失/语法错 → 抛错。
/// 嵌套加载：内层 load 消费 LAST_EXPORT（take 语义），外层脚本自己的 export 随后设置。
/// 直接用传入的 boa Context 调（不再借 CONTEXT 线程局部）——load 可能发生在命令运行中
/// （lazy 桩），外层 run_command 正持有 CONTEXT 的 RefCell 借用，再借会 panic。
/// `helix.remove_plugin_file(rel)` → 删掉**插件根内**的文件/目录(限域,见 state 里的说明)。
/// 返回 `true`(删了)/ `false`(本来不存在,幂等);非法路径**抛错**。
pub(crate) fn js_remove_plugin_file(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut boa_engine::Context,
) -> boa_engine::JsResult<JsValue> {
    // C-2 检查点:删文件是写操作(与 `write_plugin_file` 对称)⇒ `fs_write`。
    // 此前它与 remove 的"限域"性质一起被当成安全操作 ⇒ 文档的能力表写了、代码却没检查
    // (由 `check_capability_surface.mjs` 第一次跑就抓到)。
    let rel: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| crate::js_err("helix.remove_plugin_file: path must be a string"))?;
    match crate::state::remove_plugin_file(&rel) {
        Ok(b) => Ok(JsValue::from(b)),
        Err(e) => Err(crate::js_err(e)),
    }
}

/// `helix.write_plugin_file(rel, text)` → 写入**用户层插件根内**的文件(父目录按需创建)。
/// 限域同 `remove_plugin_file`:只相对路径 · 拒 `..`。返回写入的绝对路径;非法**抛错**。
pub(crate) fn js_write_plugin_file(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut boa_engine::Context,
) -> boa_engine::JsResult<JsValue> {
    let rel: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| crate::js_err("helix.write_plugin_file: path must be a string"))?;
    let text: String = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| crate::js_err("helix.write_plugin_file: text must be a string"))?;
    // C-2 检查点:写插件自身文件属于 `fs_write`
    match crate::state::write_plugin_file(&rel, &text) {
        Ok(p) => Ok(JsValue::from(JsString::from(
            p.to_string_lossy().to_string(),
        ))),
        Err(e) => Err(crate::js_err(e)),
    }
}

/// `helix.plugins_dir()` → 插件目录绝对路径(未推入 → null)。
/// `:plugin` 管理器用它定位 `manifest.json` 与插件文件。
/// `helix.picker_state() -> string` —— picker 的状态摘要(query/命中/总数/选中)✓
/// 由核心渲染路径每帧推送 ✓;picker 未开启时为空串 ✓
/// 签名照抄同文件 `js_viewport`(**boa 引擎** ✓)
pub(crate) fn js_picker_state(
    _this: &JsValue,
    _args: &[JsValue],
    _ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    Ok(JsValue::from(JsString::from(crate::state::picker_state())))
}

/// `helix.viewport() -> [width, height]` —— 视口列数/行数 ✓
///
/// 为什么需要:插件要"真正居中/铺满"就必须读得到列数,而此前**没有任何接口** ✗
/// (曾猜过 `helix.width`/`helix.rows` ⇒ 探针实测都是 undefined ✗)
/// 签名照抄同文件的 `js_plugins_dir`(**boa 引擎** ✓,不是 rquickjs ✗)
pub(crate) fn js_viewport(
    _this: &JsValue,
    _args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let (w, h) = crate::state::viewport();
    let arr = JsArray::new(ctx)?;
    arr.set(0, JsValue::from(w as i32), true, ctx)?;
    arr.set(1, JsValue::from(h as i32), true, ctx)?;
    Ok(arr.into())
}

/// `helix.quit()` —— 关闭当前视图(关掉最后一个视图即退出 ✓)
///
/// 为什么需要:实测 `helix.run_command("quit")` **永远失败** ✗
/// —— `js_run_command` 只查 **JS 侧注册表**(`helix.register_command` 填的那张 ✓),
/// 核心的 `:quit` 不在其中 ✓。故为此提供专用接口 ✓
pub(crate) fn js_quit(
    _this: &JsValue,
    _args: &[JsValue],
    _ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::Quit);
    Ok(JsValue::undefined())
}

/// `helix.plugin.unload(name)` —— 卸载**单个**插件(**入队**,命令边界执行)。
///
/// 为什么不能同步做:回收前要跑插件的 `onUnload`(再进 boa 引擎),而本函数本身正在
/// JS 回调里 ⇒ 同一个 `Context` 二次借用会 panic。走 `UiRequest::UnloadPlugin`,
/// 由 helix-term 在命令边界转成 `helix_js::unload_plugin`(与 `:plugin-unload` 同一实现)。
pub(crate) fn js_plugin_unload(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| crate::js_err("plugin.unload: name must be a string"))?;
    if name.is_empty() {
        return Err(crate::js_err("plugin.unload: name must not be empty"));
    }
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::UnloadPlugin(UnloadPluginReq { name }));
    Ok(JsValue::undefined())
}

/// `helix.reload_plugins()` —— 热重载**入队**（命令边界执行，见 `UiRequest::ReloadPlugins`）。
/// 语义与 `:plugin-reload` 完全一致（同一实现），返回 undefined。
pub(crate) fn js_reload_plugins(
    _this: &JsValue,
    _args: &[JsValue],
    _ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::ReloadPlugins);
    Ok(JsValue::undefined())
}

/// `helix.cwd()` → 当前工作目录的**绝对路径**（每次调用现取）。
///
/// 为什么需要：此前插件要拿到 cwd **只能** `helix.run("pwd")` —— 那需要 `process`
/// 能力，而"只想读文件"的插件为了一条 `pwd` 被迫申请起进程权限；且它只能缓存
/// 一次（见 `cwd-change` 事件）。签名照抄同文件的 `js_plugins_dir`。
/// 不检查任何能力：这是纯元信息（与 `helix.plugins_dir()` 同级），不是文件内容。
pub(crate) fn js_cwd(
    _this: &JsValue,
    _args: &[JsValue],
    _ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    Ok(JsValue::from(JsString::from(
        helix_stdx::env::current_working_dir()
            .to_string_lossy()
            .to_string(),
    )))
}

pub(crate) fn js_plugins_dir(
    _this: &JsValue,
    _args: &[JsValue],
    _ctx: &mut boa_engine::Context,
) -> boa_engine::JsResult<JsValue> {
    Ok(match crate::state::plugins_dir() {
        Some(p) => JsValue::from(JsString::from(p.to_string_lossy().to_string())),
        None => JsValue::null(),
    })
}

/// `helix.command_args()` → 当前键入命令的参数数组(如 `:layout save dev` → `["save","dev"]`)。
/// 由分派器在调用前设入、调用后清空(见 `state::{set,clear}_command_args`)。
pub(crate) fn js_command_args(
    _this: &JsValue,
    _args: &[JsValue],
    ctx: &mut boa_engine::Context,
) -> boa_engine::JsResult<JsValue> {
    let arr = boa_engine::object::builtins::JsArray::new(ctx)?;
    for (i, a) in crate::state::command_args(crate::host::host_state(ctx))
        .iter()
        .enumerate()
    {
        let _ = arr.set(i, JsValue::from(JsString::from(a.as_str())), false, ctx);
    }
    Ok(arr.into())
}

/// `helix.loaded_plugins()` → **已加载**插件名数组(与 Rust `:plugin list` 同一个数据源
/// `loaded_scripts()`,所以语义天然一致)。
/// 用途:让 JS 侧的插件管理器能给出与核心相同的 `list` 语义(此前只能给"已安装")。
pub(crate) fn js_loaded_plugins(
    _this: &JsValue,
    _args: &[JsValue],
    ctx: &mut boa_engine::Context,
) -> boa_engine::JsResult<JsValue> {
    let arr = boa_engine::object::builtins::JsArray::new(ctx)?;
    for (i, name) in loaded_scripts().iter().enumerate() {
        let _ = arr.set(i, JsValue::from(JsString::from(name.as_str())), false, ctx);
    }
    Ok(arr.into())
}

/// `helix.runtime_path(name)` → 自带 runtime 里的绝对路径(找不到 → null)。
/// 用途:让**内置内容型插件**能打开随软件分发的内容(如 `:tutor` 打开 `runtime/tutor`)。
pub(crate) fn js_runtime_path(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut boa_engine::Context,
) -> boa_engine::JsResult<JsValue> {
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| crate::js_err("helix.runtime_path: name must be a string"))?;
    Ok(match crate::state::runtime_path(&name) {
        Some(p) => JsValue::from(JsString::from(p.to_string_lossy().to_string())),
        None => JsValue::null(),
    })
}

pub(crate) fn js_load(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| crate::js_err("helix.load: name must be a string"))?;
    if name.is_empty() {
        return Err(crate::js_err("helix.load: name must not be empty"));
    }
    load_script_checked(ctx, &name)
}

/// 加载脚本（含依赖递归）。依赖在脚本内 helix.plugin(name, { deps }) 声明：
/// deps 是**加载参数数组** —— 写**插件名**（如 `["icons"]` → `icons/plugin.js`，见 docs/plugin-layout.md §3.4）
/// 或文件 key（如 `["lib/x.js"]`）。两者都经 `entry_keys` 解析，所以规则一致。
///
/// **注意顺序**(现场核对代码,而非照抄旧注释):先 `eval_wrapped(脚本)` 收走它声明的 deps,
/// **然后**才递归加载依赖。所以脚本**顶层的代码不能假定依赖已经执行**(要延迟到
/// 回调/命令里用)。旧注释写的是"加载目标前先递归加载依赖",与代码不符,已改。
///
/// 已加载的跳过（with_script_exports 缓存），循环依赖报错。
/// 加载栈为跨脚本共享的 thread_local（LOAD_STACK）：js_load 不再每次新建空栈，
/// 嵌套 helix.load（运行时/依赖递归）都能看到祖先——循环依赖报错而非栈溢出崩溃。
fn load_script_checked(ctx: &mut Context, name: &str) -> boa_engine::JsResult<JsValue> {
    // 加载参数 → 候选 key:裸名先找 `<name>/plugin.js`(插件布局约定),再回退 `<name>.js`。
    // 选中"第一个**能解析到文件**的候选",以此作为缓存/依赖/循环检测的 key(必须稳定)。
    let candidates = crate::state::entry_keys(name);
    let roots = crate::state::plugin_roots(crate::host::host_state(ctx));
    // 选中候选的**同时**记下"由哪个根提供"(§4-2 目录级覆盖:该插件自身的相对加载
    // 会优先在这个根内解析,避免造出"用户的 plugin.js + 内置的同名 sibling"的混合体)
    let (key, providing_root) = candidates
        .iter()
        .find_map(|k| {
            crate::state::resolve_in_with_root(roots, k)
                .filter(|(p, _)| p.exists())
                .map(|(_, r)| (k.clone(), r))
        })
        .unwrap_or_else(|| (candidates[0].clone(), std::path::PathBuf::new()));
    if let Some(cached) = with_script_exports(|m| m.get(&key).cloned()) {
        return Ok(cached);
    }
    if crate::state::with_load_stack(crate::host::host_state(ctx), |s| {
        s.iter().any(|k| k == &key)
    }) {
        let chain = crate::state::with_load_stack(crate::host::host_state(ctx), |s| {
            s.iter()
                .chain([&key])
                .cloned()
                .collect::<Vec<_>>()
                .join(" -> ")
        });
        return Err(crate::js_err(format!(
            "helix.load: circular dependency: {chain}"
        )));
    }
    // eval 前入栈（嵌套 load 期间本 key 保持可见）；借用即刻释放，不跨 eval 持有。
    // §4-2 的关键一步:prefer 的对象是**父插件的根**,不是本文件自己的根
    // (自己的根是"这次加载的结果",拿它做 prefer 只会查回同一个文件,等于没生效 ——
    //  这正是上一版没起作用的原因)。所以**先取父根**,再 push 自己。
    let parent_root = crate::state::current_load_root(crate::host::host_state(ctx));
    crate::state::with_load_stack(crate::host::host_state(ctx), |s| s.push(key.clone()));
    // 与 key **同一处** push/pop,两栈因此始终等长(见 state::LOAD_ROOTS 注释)
    crate::state::push_load_root(crate::host::host_state(ctx), providing_root.clone());
    let result = (|| -> boa_engine::JsResult<JsValue> {
        let path = if Path::new(&key).is_absolute() {
            PathBuf::from(&key)
        } else {
            crate::state::resolve_in_pref(
                crate::state::plugin_roots(crate::host::host_state(ctx)),
                &key,
                parent_root.as_deref(),
            )
            .ok_or_else(|| crate::js_err("helix.load: plugins dir not set"))?
        };
        let src = std::fs::read_to_string(&path)
            .map_err(|e| crate::js_err(format!("helix.load('{key}'): {e}")))?;
        // eval 前清空依赖记录，脚本内 helix.plugin 累积；eval 后 take 递归加载
        crate::state::with_last_plugin_deps(crate::host::host_state(ctx), |d: &mut Vec<String>| {
            d.clear()
        });
        // C-1/C-2:同一时刻清空"本次 eval 声明过 manifest 吗"的标记
        crate::permissions::begin_manifest();
        // C-3 加载期属主:<name>/plugin.js ⇒ name;其它脚本(init.js / deps 的 lib)
        // 继承父属主 —— 库是引用它的那个插件的一部分,不该自成一个插件。
        // 脚本内 `helix.plugin("x")` 会再把属主升级成 x(见 js_plugin)。
        let mut prev_owner = crate::permissions::plugin_name_from_key(&key)
            .map(|name| crate::permissions::set_current_owner(Some(&name)));
        // ⚠ 属主要一直保持到**依赖也加载完**:`deps: ["lib/x.js"]` 的库是"这个插件的一部分",
        // 它的顶层代码必须按本插件判定(否则库一碰 fs 就被拒)。restore 在闭包末尾(见下)。
        let eval_result = eval_wrapped(ctx, &src)
            .map_err(|e| crate::js_err(format!("helix.load('{key}') failed: {e}")));
        if eval_result.is_err() {
            if let Some(prev) = prev_owner.take() {
                crate::permissions::restore_owner(prev);
            }
        }
        eval_result?;
        // C-2 的"内置"判据 = **提供根**是不是用户插件目录(用户层 = 最后一个根)。
        // 内置插件随软件分发(runtime/plugins,13 个)⇒ 永不询问;用户同名覆盖
        // (config/plugins 里那份)则按第三方对待。
        if let Some(name) = crate::permissions::last_declared() {
            crate::permissions::mark_builtin(&name, crate::state::is_builtin_root(&providing_root));
        }
        let deps =
            crate::state::with_last_plugin_deps(crate::host::host_state(ctx), std::mem::take);
        // 先取外层 export（递归依赖 eval 会覆盖 LAST_EXPORT，外层 export 必须提前保存）
        let export = with_last_export(crate::host::host_state(ctx), |l| l.take())
            .unwrap_or(JsValue::undefined());
        for dep in &deps {
            load_script_checked(ctx, dep)?;
        }
        crate::state::with_loaded_scripts(|s| {
            if !s.iter().any(|(n, _)| n == &key) {
                s.push((key.clone(), src));
            }
        });
        with_script_exports(|m| m.insert(key.clone(), export.clone()));
        // 依赖都加载完了 ⇒ 恢复外层属主
        if let Some(prev) = prev_owner.take() {
            crate::permissions::restore_owner(prev);
        }
        Ok(export)
    })();
    crate::state::with_load_stack(crate::host::host_state(ctx), |s| {
        s.pop();
    });
    crate::state::pop_load_root(crate::host::host_state(ctx));
    // 加载树结束(最外层)⇒ 脚本里的 define_config 已注册 schema,重建配置缓存。
    // 嵌套依赖不重复重建:外层结束前它们的 define_config 都跑完了。
    if result.is_ok()
        && crate::state::with_load_stack(crate::host::host_state(ctx), |s| s.is_empty())
    {
        refresh_configs_after_load();
    }
    result
}

/// `helix.plugin(name, { schema_version, min_editor, deps, permissions, contributes })`
///：声明当前脚本的 manifest(C-1:解析 + 记录,不拦截)。
/// deps 是**加载参数数组**（插件名如 ["icons"]，或文件 key 如 ["lib/x.js"]），helix.load 自动拓扑加载。
/// permissions 是**能力清单**（C-2 的判据）:
/// `{ process: bool, fs_read: bool, fs_write: bool }` —— **不写 = 空能力集**
/// （fail-closed:受保护调用会被拒一次并进入 `:js-permission` 待授权;见 `crate::permissions`）。
pub(crate) fn js_plugin(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    // name 仅校验类型（API 形状：helix.plugin(name, { … })）
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| crate::js_err("helix.plugin: name must be a string"))?;
    // C-3:manifest 里声明名字 = 宣称"本脚本就是它" ⇒ 属主就地升级成它。
    // 这是加载期归属的权威来源(脚本名只是兜底:裸脚本 / `:plugin-load /abs/x.js`)。
    crate::permissions::set_current_owner(Some(&name));
    let mut manifest = crate::types::PluginManifest {
        name: name.clone(),
        ..Default::default()
    };
    if let Some(meta) = args.get(1).unwrap_or(&JsValue::undefined()).as_object() {
        if let Ok(v) = meta
            .get(JsString::from("deps"), ctx)
            .and_then(|v| v.try_js_into::<Vec<String>>(ctx))
        {
            manifest.deps = v;
        }
        if let Ok(v) = meta
            .get(JsString::from("schema_version"), ctx)
            .and_then(|v| v.try_js_into::<u32>(ctx))
        {
            // 声明面版本**要比一比**:高于编辑器支持的版本 ⇒ 直接报错，不默默降级
            // (插件按它不认识的语义跑，比"加载失败"更糟)。
            if v > u32::from(crate::types::API_VERSION) {
                return Err(crate::js_err(format!(
                    "helix.plugin('{name}'): schema_version {v} 高于本编辑器支持的 {}（升级编辑器或降级插件）",
                    crate::types::API_VERSION
                )));
            }
            manifest.schema_version = Some(v);
        }
        if let Ok(v) = meta
            .get(JsString::from("min_editor"), ctx)
            .and_then(|v| v.try_js_into::<String>(ctx))
        {
            // 声明了就比一比 —— 但**只警告,不拒载**(与 `schema_version` 的区别在此:
            // 版本不匹配只说明"插件可能用到本编辑器没有的 API",代码本身仍可能正常工作;
            // 而 `schema_version` 高于当前 = 声明的**字段语义**都可能不同 ⇒ 必须拒)。
            // 编辑器版本未知(库用法/单测未 set_editor_version)⇒ 跳过,不误伤。
            if let Some(actual) = crate::host::editor_version() {
                if let Err(msg) = crate::permissions::min_editor_ok(&v, &actual) {
                    // 让失败可见:状态栏(与能力询问同一队列;helix-js 不依赖 `log`)
                    crate::host::push_message(format!("helix.plugin('{name}'): {msg}(已继续加载)"));
                }
            }
            manifest.min_editor = Some(v);
        }
        if let Ok(v) = meta
            .get(JsString::from("contributes"), ctx)
            .and_then(|v| v.try_js_into::<Vec<String>>(ctx))
        {
            manifest.contributes = v;
        }
        // 只看**是不是对象**:写 `permissions: {…}` 才进入检查;不写/写 null 都是 legacy
        if let Some(perms) = meta
            .get(JsString::from("permissions"), ctx)
            .ok()
            .and_then(|v| v.as_object())
        {
            let mut flag = |key: &str| {
                perms
                    .get(JsString::from(key), ctx)
                    .map(|v| v.to_boolean())
                    .unwrap_or(false)
            };
            manifest.permissions = Some(crate::types::PluginPermissions {
                process: flag("process"),
                fs_read: flag("fs_read"),
                fs_write: flag("fs_write"),
            });
        }
    }
    // `onUnload`:卸载**前**跑一次(不能阻止卸载;错误只进消息队列)。
    // 不放 manifest:`PluginManifest` 派生了 `PartialEq/Eq`,塞 `JsValue` 会破掉。
    if let Some(f) = args
        .get(1)
        .and_then(|v| v.as_object())
        .and_then(|o| o.get(JsString::from("onUnload"), ctx).ok())
        .filter(|v| v.as_callable().is_some())
    {
        crate::state::set_unload_hook(crate::host::host_state(ctx), &name, f);
    }
    crate::state::with_last_plugin_deps(crate::host::host_state(ctx), |d| {
        d.extend(manifest.deps.clone())
    });
    crate::permissions::record_manifest(manifest);
    Ok(JsValue::undefined())
}

// helix.plugin.install(arg)：镜像 :plugin install(path 或 git-url)。结果经状态栏显示。

// helix.plugin.update([name])：镜像 :plugin update(name 可选 = all)

// helix.plugin.remove(name)：镜像 :plugin remove

/// `helix.server.<op>`：镜像 :server(list/search/install/update/remove/status)。
/// 单向请求(与 PluginOp 同构)：结果经 editor 状态栏/错误显示,不回传 JS。
pub(crate) fn js_server_list(
    _this: &JsValue,
    args: &[JsValue],
    _context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    // B1:server.* 会触发下载/安装/卸载并改写 languages.toml(并且探测版本要起子进程)
    // ⇒ 与 `helix.run` 同一能力。内置插件 grandfathered;第三方 manifest 需声明 process。
    let arg = args
        .first()
        .and_then(|v| v.as_string())
        .map(|s| s.to_std_string_escaped());
    push_server_op("list", arg);
    Ok(JsValue::undefined())
}

pub(crate) fn js_server_search(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    // B1:server.* 会触发下载/安装/卸载并改写 languages.toml(并且探测版本要起子进程)
    // ⇒ 与 `helix.run` 同一能力。内置插件 grandfathered;第三方 manifest 需声明 process。
    let kw: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)
        .map_err(|_| crate::js_err("helix.server.search: keyword required"))?;
    push_server_op("search", Some(kw));
    Ok(JsValue::undefined())
}

pub(crate) fn js_server_install(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    // B1:server.* 会触发下载/安装/卸载并改写 languages.toml(并且探测版本要起子进程)
    // ⇒ 与 `helix.run` 同一能力。内置插件 grandfathered;第三方 manifest 需声明 process。
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)
        .map_err(|_| crate::js_err("helix.server.install: name required"))?;
    push_server_op("install", Some(name));
    Ok(JsValue::undefined())
}

pub(crate) fn js_server_update(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    // B1:server.* 会触发下载/安装/卸载并改写 languages.toml(并且探测版本要起子进程)
    // ⇒ 与 `helix.run` 同一能力。内置插件 grandfathered;第三方 manifest 需声明 process。
    // 无参 = 更新全部(与 :server update 一致)
    let name: Option<String> = match args.first() {
        Some(v) if !v.is_undefined() && !v.is_null() => Some(
            v.clone()
                .try_js_into(context)
                .map_err(|_| crate::js_err("helix.server.update: name must be a string"))?,
        ),
        _ => None,
    };
    push_server_op("update", name);
    Ok(JsValue::undefined())
}

pub(crate) fn js_server_remove(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    // B1:server.* 会触发下载/安装/卸载并改写 languages.toml(并且探测版本要起子进程)
    // ⇒ 与 `helix.run` 同一能力。内置插件 grandfathered;第三方 manifest 需声明 process。
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)
        .map_err(|_| crate::js_err("helix.server.remove: name required"))?;
    push_server_op("remove", Some(name));
    Ok(JsValue::undefined())
}

pub(crate) fn js_server_status(
    _this: &JsValue,
    args: &[JsValue],
    _context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    // B1:server.* 会触发下载/安装/卸载并改写 languages.toml(并且探测版本要起子进程)
    // ⇒ 与 `helix.run` 同一能力。内置插件 grandfathered;第三方 manifest 需声明 process。
    let arg = args
        .first()
        .and_then(|v| v.as_string())
        .map(|s| s.to_std_string_escaped());
    push_server_op("status", arg);
    Ok(JsValue::undefined())
}

fn push_server_op(op: &str, arg: Option<String>) {
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::ServerOp(ServerOpReq {
            op: op.to_string(),
            arg,
        }));
}

/// helix.export(obj)：声明当前脚本的导出（被 helix.load 的返回值拿到）。undefined/null 清空。
pub(crate) fn js_export(
    _this: &JsValue,
    args: &[JsValue],
    _ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let arg = args.first().cloned().unwrap_or(JsValue::undefined());
    crate::state::with_last_export_current(|l| {
        *l = if arg.is_null_or_undefined() {
            None
        } else {
            Some(arg)
        }
    });
    Ok(JsValue::undefined())
}

/// helix.lazy(name, ...cmds)：为每个 cmd 注册桩闭包——首次调用时加载 name 再转执行。
/// 桩经 eval 工厂构造闭包（不经 REGISTRY 捕获 JsValue，避免闭包环境问题）。
pub(crate) fn js_lazy(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    if args.len() < 2 {
        return Err(crate::js_err(
            "helix.lazy: name and at least one command required",
        ));
    }
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| crate::js_err("helix.lazy: name must be a string"))?;
    if name.is_empty() {
        return Err(crate::js_err("helix.lazy: name must not be empty"));
    }
    let factory = ctx
        .eval(Source::from_bytes(
            "(function(n, c) { return function(ctx) { helix.load(n); helix.run_command(c, ctx); }; })",
        ))
        .map_err(|e| crate::js_err(format!("helix.lazy: {e}")))?;
    let factory = factory
        .as_callable()
        .and_then(JsFunction::from_object)
        .ok_or_else(|| crate::js_err("helix.lazy: internal factory error"))?;
    let undefined = JsValue::undefined();
    for cmd in &args[1..] {
        let cmd: String = cmd
            .try_js_into(ctx)
            .map_err(|_| crate::js_err("helix.lazy: command names must be strings"))?;
        if cmd.is_empty() || cmd.chars().any(char::is_whitespace) {
            return Err(crate::js_err(format!("invalid command name: {cmd:?}")));
        }
        let closure = factory
            .call(
                &undefined,
                &[
                    JsValue::from(JsString::from(name.clone())),
                    JsValue::from(JsString::from(cmd.clone())),
                ],
                ctx,
            )
            .map_err(|e| crate::js_err(format!("helix.lazy: {e}")))?;
        with_registry(crate::host::host_state(ctx), |r| r.insert(cmd, closure));
    }
    Ok(JsValue::undefined())
}

/// 解析 JS { row, col } → 坐标（字段缺失/非对象用缺省值）
fn parse_pos(
    v: &JsValue,
    ctx: &mut Context,
    dflt: (usize, usize),
) -> boa_engine::JsResult<(usize, usize)> {
    let Some(obj) = v.as_object() else {
        return Ok(dflt);
    };
    let row = obj.get(JsString::from("row"), ctx)?;
    let col = obj.get(JsString::from("col"), ctx)?;
    Ok((
        if row.is_null_or_undefined() {
            dflt.0
        } else {
            row.try_js_into::<usize>(ctx)?
        },
        if col.is_null_or_undefined() {
            dflt.1
        } else {
            col.try_js_into::<usize>(ctx)?
        },
    ))
}

/// 解析 JS ctx 对象 → CommandContext（缺省：path=None/text=""/cursor=(0,0)/selection 全 0）
/// 从 JS 对象读可选字符串字段
pub(crate) fn js_get_str(
    obj: &boa_engine::JsObject,
    key: &str,
    ctx: &mut Context,
) -> boa_engine::JsResult<Option<String>> {
    let v = obj.get(JsString::from(key), ctx)?;
    if v.is_null_or_undefined() {
        Ok(None)
    } else {
        Ok(Some(v.try_js_into::<String>(ctx)?))
    }
}

fn parse_command_ctx(v: &JsValue, ctx: &mut Context) -> boa_engine::JsResult<CommandContext> {
    let dflt = CommandContext {
        docs: vec![],
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
    };
    let Some(obj) = v.as_object() else {
        return Ok(dflt);
    };
    // 命令实际收到的 ctx 形状是 { doc: { path, text, ... }, cursor, selection }——
    // 兼容两层（doc.path ?? path），保证 lazy 转发不丢 path/text
    let doc_val = obj.get(JsString::from("doc"), ctx)?;
    let doc_obj = doc_val.as_object();
    let path = match &doc_obj {
        Some(doc) => js_get_str(doc, "path", ctx)?.or(js_get_str(&obj, "path", ctx)?),
        None => js_get_str(&obj, "path", ctx)?,
    };
    let text = match &doc_obj {
        Some(doc) => js_get_str(doc, "text", ctx)?.unwrap_or_else(|| {
            js_get_str(&obj, "text", ctx)
                .ok()
                .flatten()
                .unwrap_or_default()
        }),
        None => js_get_str(&obj, "text", ctx)?.unwrap_or_default(),
    };
    let cursor = parse_pos(&obj.get(JsString::from("cursor"), ctx)?, ctx, (0, 0))?;
    let selection = {
        let sel = obj.get(JsString::from("selection"), ctx)?;
        if sel.is_null_or_undefined() {
            ((0, 0), (0, 0))
        } else {
            let sel_obj = sel.as_object().ok_or_else(|| {
                crate::js_err("helix.run_command: 'selection' must be an object with anchor/head")
            })?;
            let anchor = parse_pos(&sel_obj.get(JsString::from("anchor"), ctx)?, ctx, (0, 0))?;
            let head = parse_pos(&sel_obj.get(JsString::from("head"), ctx)?, ctx, (0, 0))?;
            (anchor, head)
        }
    };
    Ok(CommandContext {
        docs: vec![],
        path,
        text,
        cursor,
        selection,
    })
}

/// helix.run_command(name, ctx?)：程序化调用插件命令。ctx 缺省空快照。
/// 嵌套调用合法：直接用传入的 boa Context 调（不再借 CONTEXT，避开 RefCell 重入 panic）；
/// 编辑/光标请求排队，由外层命令的 drain 应用。
pub(crate) fn js_run_command(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| crate::js_err("helix.run_command: name must be a string"))?;
    let command_ctx = parse_command_ctx(args.get(1).unwrap_or(&JsValue::undefined()), ctx)?;
    let func = with_registry(crate::host::host_state(ctx), |r| r.get(&name).cloned())
        .ok_or_else(|| crate::js_err(format!("helix.run_command: '{name}' is not registered")))?;
    let func = func
        .as_callable()
        .and_then(JsFunction::from_object)
        .ok_or_else(|| crate::js_err(format!("registered value for '{name}' is not a function")))?;
    let arg = ctx_to_js(&command_ctx, ctx)
        .map_err(|e| crate::js_err(format!("helix.run_command: {e}")))?;
    let undefined = JsValue::undefined();
    func.call(&undefined, &[arg], ctx)
        .map(|_| JsValue::undefined())
        .map_err(|e| crate::js_err(format!("helix.run_command('{name}') failed: {e}")))
}

/// helix.map 允许的 mode 白名单
const KEYMAP_MODES: [&str; 3] = ["normal", "insert", "select"];

/// 注册键位绑定：字符串命令直接入队，函数注册为隐藏插件命令 __mapped_N
pub(crate) fn js_map(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let mode: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    if !KEYMAP_MODES.contains(&mode.as_str()) {
        return Err(crate::js_err(format!("helix.map: unknown mode '{mode}'")));
    }
    let key: String = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    if key.is_empty() {
        return Err(crate::js_err("helix.map: key must not be empty"));
    }
    let command_arg = args.get(2).cloned().unwrap_or(JsValue::undefined());

    // 函数 → 注册为隐藏插件命令 __mapped_N
    let command: String = if command_arg.as_callable().is_some() {
        // 4a L1：宿主状态从**引擎**取回（原生回调拿不到 &mut JsHost）
        let id = crate::state::next_map_id(crate::host::host_state(context));
        let name = format!("__mapped_{id}");
        with_registry(crate::host::host_state(context), |r| {
            r.insert(name.clone(), command_arg)
        });
        // C-2 归属:映射函数是**命令**形态执行的(`run_command`),所以按命令绑定属主。
        // 不绑 ⇒ 这个隐藏命令没有属主 ⇒ 权限判定退到"加载期最后声明的插件"。
        if let Some(owner) = crate::permissions::current_owner() {
            crate::permissions::bind_command_owner(&name, &owner);
        }
        name
    } else {
        command_arg.try_js_into(context)?
    };

    // 第 4 个可选参数:{ unbind: true } ⇒ 摘掉这条绑定(dashboard 关屏时用)✓
    let unbind = args
        .get(3)
        .and_then(|v| v.as_object())
        .and_then(|o| o.get(JsString::from("unbind"), context).ok())
        .and_then(|v| v.as_boolean())
        .unwrap_or(false);

    // 键位属主:按 (mode, key) 记,`:plugin-unload` 靠它对每条绑定入队解绑
    let owner = crate::permissions::current_owner().or_else(crate::permissions::current_declared);
    let st = crate::host::host_state(context);
    let slot = (mode.clone(), key.clone());
    crate::state::with_keymap_owners(st, |m| match (&owner, unbind) {
        (Some(o), false) => {
            m.insert(slot, o.clone());
        }
        (_, true) => {
            m.remove(&slot);
        }
        _ => {}
    });
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::MapKey(MapKeyReq {
            mode,
            key,
            command,
            unbind,
        }));
    Ok(JsValue::undefined())
}

/// 把 CommandContext 转成 doc 对象 { path, text, cursor, _target } + 编辑方法。
/// _target = 编辑目标 path（by_path 的 doc）；ctx.doc 为 null → 编辑推 Edit.doc = None。
fn build_doc_object(
    path: Option<&str>,
    text: &str,
    cursor: (usize, usize),
    target: Option<&str>,
    engine: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let cursor_obj = ObjectInitializer::new(engine)
        .property(
            JsString::from("row"),
            JsValue::from(cursor.0 as f64),
            Attribute::all(),
        )
        .property(
            JsString::from("col"),
            JsValue::from(cursor.1 as f64),
            Attribute::all(),
        )
        .build();
    Ok(JsValue::from(
        ObjectInitializer::new(engine)
            .property(
                JsString::from("path"),
                match path {
                    Some(p) => JsValue::from(JsString::from(p)),
                    None => JsValue::null(),
                },
                Attribute::all(),
            )
            .property(
                JsString::from("text"),
                JsValue::from(JsString::from(text)),
                Attribute::all(),
            )
            .property(JsString::from("cursor"), cursor_obj, Attribute::all())
            // 内部目标:by_path 的 doc 用它路由编辑;ctx.doc 为 null。插件可见但无害。
            .property(
                JsString::from("_target"),
                match target {
                    Some(t) => JsValue::from(JsString::from(t)),
                    None => JsValue::null(),
                },
                Attribute::all(),
            )
            .function(
                NativeFunction::from_fn_ptr(js_doc_insert),
                JsString::from("insert"),
                3,
            )
            .function(
                NativeFunction::from_fn_ptr(js_doc_replace),
                JsString::from("replace"),
                5,
            )
            .function(
                NativeFunction::from_fn_ptr(js_doc_delete),
                JsString::from("delete"),
                4,
            )
            .build(),
    ))
}

/// 把 CommandContext 转成 doc 对象（当前 buffer 编辑目标）
pub(crate) fn doc_to_js(
    ctx: &CommandContext,
    engine: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    build_doc_object(ctx.path.as_deref(), &ctx.text, ctx.cursor, None, engine)
}

/// 按路径查已打开 buffer 快照，返回与 ctx.doc 同构的 doc 对象（编辑路由到目标 buffer）。
/// 未找到 → null（不自动打开文件）。路径经 canonicalize 规范化后精确匹配。
pub(crate) fn js_by_path(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let path: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)
        .map_err(|_| crate::js_err("helix.by_path: path must be a string"))?;
    let canonical = helix_stdx::path::canonicalize(&path)
        .to_string_lossy()
        .into_owned();
    let hit = crate::state::with_doc_snapshots(crate::host::host_state(context), |s| {
        s.iter().find(|d| d.path == canonical).cloned()
    });
    let Some(hit) = hit else {
        return Ok(JsValue::null());
    };
    build_doc_object(Some(&hit.path), &hit.text, (0, 0), Some(&hit.path), context)
}

/// 把 CommandContext 转成 JS 对象 { doc: { path, text }, cursor: { row, col }, selection: { anchor: {row,col}, head: {row,col} } }
fn ctx_to_js(ctx: &CommandContext, engine: &mut Context) -> boa_engine::JsResult<JsValue> {
    let doc = doc_to_js(ctx, engine)?;
    let anchor = ObjectInitializer::new(engine)
        .property(
            JsString::from("row"),
            JsValue::from(ctx.selection.0 .0 as f64),
            Attribute::all(),
        )
        .property(
            JsString::from("col"),
            JsValue::from(ctx.selection.0 .1 as f64),
            Attribute::all(),
        )
        .build();
    let head = ObjectInitializer::new(engine)
        .property(
            JsString::from("row"),
            JsValue::from(ctx.selection.1 .0 as f64),
            Attribute::all(),
        )
        .property(
            JsString::from("col"),
            JsValue::from(ctx.selection.1 .1 as f64),
            Attribute::all(),
        )
        .build();
    let selection = ObjectInitializer::new(engine)
        .property(JsString::from("anchor"), anchor, Attribute::all())
        .property(JsString::from("head"), head, Attribute::all())
        .build();
    let cursor = ObjectInitializer::new(engine)
        .property(
            JsString::from("row"),
            JsValue::from(ctx.cursor.0 as f64),
            Attribute::all(),
        )
        .property(
            JsString::from("col"),
            JsValue::from(ctx.cursor.1 as f64),
            Attribute::all(),
        )
        .build();
    Ok(JsValue::from(
        ObjectInitializer::new(engine)
            .property(JsString::from("doc"), doc, Attribute::all())
            .property(JsString::from("cursor"), cursor, Attribute::all())
            .property(JsString::from("selection"), selection, Attribute::all())
            .build(),
    ))
}

/// 运行插件命令。返回 Ok(true) 表示已运行，Ok(false) 表示未注册
/// 运行插件命令。返回 Ok(true) 表示已运行，Ok(false) 表示未注册。
///
/// **L4-④**:状态由调用方给出(`JsHost::run_command` 经 `pump` 传入),本函数不再摸隐式宿主;
/// 只有 `with_engine` 仍走隐式 —— 那是"引擎已借出"的那一类(原生回调),不在本档范围。
pub fn run_command(st: &HostState, name: &str, ctx: &CommandContext) -> Result<bool> {
    // C-2 归属:命令的属主(注册期绑定);执行完恢复上一个(嵌套 run_command 也正确)
    let prev_owner =
        crate::permissions::set_current_owner(crate::permissions::command_owner(name).as_deref());
    // 命令开始时清空编辑队列，避免跨命令残留
    crate::state::with_edits(st, |c| c.clear());
    crate::state::with_decoration_requests(st, |c| c.clear());
    crate::state::with_cursor_requests(st, |c| c.clear());
    // 复位事务深度：上一命令未配对的 begin 在此丢弃（积压编辑随队列清空）
    crate::state::with_txn_depth(st, |d| *d = 0);
    // 携带其它已打开 buffer 快照（by_path 读取；本次命令生命周期）
    crate::state::set_doc_snapshots(st, ctx.docs.clone());
    let func = crate::state::with_registry(st, |r| r.get(name).cloned());
    let Some(func) = func else { return Ok(false) };

    let func = func
        .as_callable()
        .and_then(JsFunction::from_object)
        .ok_or_else(|| anyhow!("registered value for '{name}' is not a function"))?;

    let res = crate::state::with_engine(|engine| {
        let arg =
            ctx_to_js(ctx, engine).map_err(|e| anyhow!("failed to build command context: {e}"))?;
        let undefined = JsValue::undefined();
        func.call(&undefined, &[arg], engine)
            .map(|_| true)
            .map_err(|e| anyhow!("plugin command '{name}' failed: {e}"))
    });
    crate::permissions::restore_owner(prev_owner);
    res
}

/// 已注册的插件命令名（供命令行补全）
pub fn command_names() -> Vec<String> {
    // 查询:状态在宿主里;无宿主 ⇒ 空列表(**不得**建宿主)
    crate::state::try_with_registry(|r| r.keys().cloned().collect()).unwrap_or_default()
}

/// 插件命令说明（未注册返回 None）
pub fn command_doc(name: &str) -> Option<String> {
    // 查询:不需要宿主(表在 thread_local 里)⇒ **不得** init()(那会建整个引擎)
    crate::state::with_command_docs_current(|d| d.get(name).cloned())
}

pub(crate) fn js_on(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let handler = args.get(1).cloned().unwrap_or(JsValue::undefined());
    if !EVENT_WHITELIST.contains(&name.as_str()) {
        return Err(crate::js_err(format!("helix.on: unknown event '{name}'")));
    }
    if handler.as_callable().is_none() {
        return Err(crate::js_err("helix.on: handler must be a function"));
    }
    // 同 register_command:运行期属主优先(回调里注册的事件处理器属于那个回调的插件)
    if let Some(owner) = crate::permissions::current_owner() {
        crate::permissions::bind_handler_owner(&name, &owner);
    } else if let Some(owner) = crate::permissions::current_declared() {
        crate::permissions::bind_handler_owner(&name, &owner);
    }
    with_event_handlers(crate::host::host_state(context), |h| {
        h.entry(name).or_default().push(handler)
    });
    Ok(JsValue::undefined())
}

/// 是否有注册的事件处理器（挂点快速跳过）
pub fn has_handlers(name: &str) -> bool {
    // 查询:同上,无宿主 ⇒ false
    crate::state::try_with_event_handlers(|h| h.get(name).is_some_and(|v| !v.is_empty()))
        .unwrap_or(false)
}

/// 触发事件：按注册顺序调用处理器；开始时清空编辑队列（防残留）。
/// extra 用于 mode-change 的 mode 字符串参数。
pub fn emit_event(name: &str, ctx: &CommandContext, extra: Option<&str>) -> Result<()> {
    emit_event_impl(name, ctx, extra, None, 0, None, None)
}

/// `cwd-change` 事件入口：**只有真正 chdir 成功的地方**可以调（`helix-term` 的
/// `apply_directory_change`，`:cd` / `:lcd` / `:pop-directory` 三条命令的公共出口）。
///
/// 为什么不用现成的 `extra`：本事件要同时给**新/旧**两个路径，而 `extra` 只能带一个
/// 字符串；"再多一个隐式参数" 的替代方案（宿主字段暂存）本项目已明确废弃
/// （见 `emit_layout_change_kind` 上方的注释）⇒ 和 `emit_startup` 同款：走显式参数。
pub fn emit_cwd_change(ctx: &CommandContext, prev: Option<&Path>, cwd: &Path) -> Result<()> {
    match emit_event_impl("cwd-change", ctx, None, None, 0, Some((cwd, prev)), None) {
        Ok(()) => Ok(()),
        Err(e) => {
            // 与 layout-change/startup 同款：**让失败可见**（静默 log 曾让人误判多轮 ✗）
            crate::host::with_messages(|v| v.push(format!("cwd-change emit failed: {e}")));
            Err(e)
        }
    }
}

/// `layout-change` 事件入口(**无需命令上下文**)。
///
/// 为什么需要它:布局变更发生在 `LayoutTree`/`Compositor` 内部(§47 勘明),
/// 那里**没有** `CommandContext` ✗;而 `emit_event_impl` 只用到 `ctx.docs`
/// (其它已打开 buffer 的快照)⇒ 这里给一个**最小上下文**(与测试构造同款)即可 ✓
/// `startup` 事件入口（**无需命令上下文**，与 `emit_layout_change` 同款）。
///
/// 发在 `application.rs` 处理完初始文件**之后** ⇒ “要不要显示启动屏”由**插件**判断
/// （它自己看当前 buffer 是否空白无名）✓ —— 核心只负责“启动完成了”这一事实 ✓
/// `startup` 事件入口。
///
/// ⚠️ 这里**必须**由调用方给出真实的 `ctx`(启动后的当前 buffer):早先它写死成
/// "path=None / text=""" ⇒ 处理器**无法**判断"启动时到底有没有打开文件",
/// 而内建 dashboard 正是靠这个判断"要不要显示启动屏"的(实测 bug:带路径打开文件时
/// dashboard 照样弹出,和文件选择器叠在一起)。
pub fn emit_startup(ctx: &CommandContext, open_paths: usize) -> Result<()> {
    match emit_event_impl("startup", ctx, None, None, open_paths, None, None) {
        Ok(()) => Ok(()),
        Err(e) => {
            // 与 layout-change 同款：让失败**可见**（静默的 log::warn 曾让我误判多轮 ✗）
            crate::host::with_messages(|v| v.push(format!("startup emit failed: {e}")));
            Err(e)
        }
    }
}

// `layout-change` 的**变更种类**现在是**显式参数**(原为宿主字段 `layout_change_kind` 的"暂存"):
//
// 为什么改:暂存机制是为"发射点拿不到种类"设计的 —— 发射点在
// `Compositor::sync_layout_cache` 里,而知道种类的是它**前面**那一行。两个调用方隔着一层
// 共享善变的宿主字段,谁都能写、谁都忘了读。现在种类存**在变更发生的那一层**
// (`Compositor` 的意图队列),由边界处(`flush_layout_change`)原样传进来,
// 宿主不再承担这个暂存任务 ⇒ 少一个字段、少一处隐式耦合。

/// 发 `layout-change`;`kind` 为 `Some` 时作为事件的第一个参数(与 `mode-change` 传模式串同一机制)。
pub fn emit_layout_change_kind(kind: Option<&'static str>) -> Result<()> {
    let ctx = CommandContext {
        path: None,
        text: String::new(),
        cursor: (0, 0),
        selection: ((0, 0), (0, 0)),
        docs: Vec::new(),
    };
    match emit_event_impl("layout-change", &ctx, kind, None, 0, None, None) {
        Ok(()) => Ok(()),
        Err(e) => {
            // **让失败可见**：只 log::warn 会静默（我曾因此误判多轮 ✗）
            crate::host::with_messages(|v| v.push(format!("layout-change emit failed: {e}")));
            Err(e)
        }
    }
}

/// doc-change 事件入口：参数附带防抖窗口内合并的变更范围（doc.changes）。
///
/// `doc_id` 记 0 = "不知道/当前文档"（老调用点保持原形状）。
pub fn emit_doc_change(ctx: &CommandContext, changes: &[DocChange]) -> Result<()> {
    emit_doc_change_for(ctx, 0, changes)
}

/// 同 [`emit_doc_change`]，但**显式带文档 id**（后台 buffer 的变更靠它分辨）。
pub fn emit_doc_change_for(ctx: &CommandContext, doc_id: u64, changes: &[DocChange]) -> Result<()> {
    emit_event_impl(
        "doc-change",
        ctx,
        None,
        Some(changes),
        0,
        None,
        Some(doc_id),
    )
}

/// 事件触发公共实现；changes 为 Some 时序列化合并范围到 doc.changes。
fn emit_event_impl(
    name: &str,
    ctx: &CommandContext,
    extra: Option<&str>,
    changes: Option<&[DocChange]>,
    // 只有 `startup` 用:命令行传入的"要打开的东西"数量(见 `StartupPayload::open_paths`)
    startup_open_paths: usize,
    // 只有 `cwd-change` 用:(新 cwd, 旧 cwd) —— 两个值都在**发射点**成立,别处拿不到
    cwd: Option<(&Path, Option<&Path>)>,
    // 只有 `doc-change` 用:被改文档的 id(逐文档派发时区分;None = 调用点没给 ⇒ 记 0)
    doc_id: Option<u64>,
) -> Result<()> {
    crate::init();
    crate::state::with_edits_current(|c| c.clear());
    crate::state::with_decoration_requests_current(|c| c.clear());
    crate::state::with_cursor_requests_current(|c| c.clear());
    // 复位事务深度：上一命令/事件未配对的 begin 在此丢弃（积压编辑随队列清空）
    crate::state::with_txn_depth_current(|d| *d = 0);
    // 携带其它已打开 buffer 快照（by_path 读取；本次事件生命周期）
    crate::host::with_state_or_init(|st| crate::state::set_doc_snapshots(st, ctx.docs.clone()));
    let handlers = crate::state::with_event_handlers_current(|h| h.get(name).cloned());
    let Some(handlers) = handlers else {
        return Ok(());
    };
    if handlers.is_empty() {
        return Ok(());
    }
    crate::state::with_engine(|engine| {
        let doc = doc_to_js(ctx, engine).map_err(|e| anyhow!("failed to build event doc: {e}"))?;
        // B-1:doc 对象也带 `api_version`/`type` —— 单参形状的订阅者(`(doc) => …`)
        // 不必改签名就能判断协议版本。两个字段都是**追加**,不动既有字段。
        {
            let obj = doc
                .as_object()
                .ok_or_else(|| anyhow!("event doc is not an object"))?;
            obj.set(
                JsString::from("api_version"),
                JsValue::from(API_VERSION),
                false,
                engine,
            )
            .map_err(|e| anyhow!("failed to attach api_version: {e}"))?;
            obj.set(
                JsString::from("type"),
                JsValue::from(JsString::from(name.to_string())),
                false,
                engine,
            )
            .map_err(|e| anyhow!("failed to attach event type: {e}"))?;
        }
        if let Some(changes) = changes {
            let arr = build_changes_array(changes, ctx, engine)
                .map_err(|e| anyhow!("failed to build event changes: {e}"))?;
            doc.as_object()
                .ok_or_else(|| anyhow!("event doc is not an object"))?
                .set(JsString::from("changes"), arr, false, engine)
                .map_err(|e| anyhow!("failed to attach event changes: {e}"))?;
        }
        // B-1:每次事件都附一份**具名 + 版本化**的 payload,作为**最后一个参数**。
        // 位置参数保持原样(既有订阅者零迁移);payload 是往前兼容的那一面 ——
        // 它回答"这是哪个事件、协议第几版、字段叫什么",而位置参数回答不了。
        let ev_changes = changes.map(|c| changes_to_ev(c, ctx)).unwrap_or_default();
        let payload = payload_js(
            engine,
            &build_event_payload(
                name,
                ctx,
                extra,
                &ev_changes,
                startup_open_paths,
                cwd,
                doc_id,
            ),
        )
        .map_err(|e| anyhow!("failed to build event payload: {e}"))?;
        let undefined = JsValue::undefined();
        for (idx, handler) in handlers.iter().enumerate() {
            // C-2 归属:事件处理器的属主(注册期按下标绑定)
            let prev_owner = crate::permissions::set_current_owner(
                crate::permissions::handler_owner(name, idx).as_deref(),
            );
            let func = handler
                .as_callable()
                .and_then(JsFunction::from_object)
                .ok_or_else(|| anyhow!("event '{name}' handler not callable"))?;
            // 形状统一(**仅 `layout-change`** —— 它是唯一"有种类概念"的事件):永远是
            // `(kind | null, doc)`。缺种类时第一参是**显式 null**,绝不把 doc 挪到第一参。
            // 旧实现 `None ⇒ vec![doc]` 会让写 `(kind) => …` 的插件**静默**拿到事件对象:
            // `sync_layout_cache` 的 40 个调用点里只有 10 处设种类(typed.rs),多数路径命中该坑。
            // 其它事件没有种类概念 ⇒ 单参形状**保持不变**(改了会打断全部既有订阅者)。
            let mut args: Vec<JsValue> = if name == "layout-change" {
                let kind = match extra {
                    Some(mode) => JsValue::from(JsString::from(mode.to_string())),
                    None => JsValue::null(),
                };
                vec![kind, doc.clone()]
            } else {
                match extra {
                    Some(mode) => {
                        vec![JsValue::from(JsString::from(mode.to_string())), doc.clone()]
                    }
                    None => vec![doc.clone()],
                }
            };
            args.push(payload.clone());
            let _: JsValue = func.call(&undefined, &args, engine).map_err(|e| {
                crate::state::with_edits_current(|c: &mut Vec<crate::types::Edit>| c.clear());
                crate::state::with_decoration_requests_current(|c| c.clear());
                crate::state::with_cursor_requests_current(|c| c.clear());
                crate::host::with_state_or_init(|st| crate::state::with_txn_depth(st, |d| *d = 0)); // 与正常入口复位一致:错误路径也恢复事务深度
                anyhow!("event '{name}' handler failed: {e}")
            })?;
            crate::permissions::restore_owner(prev_owner);
        }
        Ok(())
    })
}

/// 防抖窗口内变更合并为包围范围：old = (min old_start, max old_end)，new = (min new_start, max new_end)。
/// 窗口无变更 → 空数组。坐标行列按当前文本换算（old 坐标相对变更时刻文本，窗口语义由插件处理）。
fn build_changes_array(
    changes: &[DocChange],
    ctx: &CommandContext,
    engine: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    if changes.is_empty() {
        return Ok(JsValue::from(JsArray::new(engine)?));
    }
    let ev = changes_to_ev(changes, ctx);
    let mut items = Vec::with_capacity(ev.len());
    for c in &ev {
        items.push(change_js(engine, c));
    }
    Ok(JsValue::from(JsArray::from_iter(items, engine)))
}

/// 合并窗口 → **具名**变更范围(包围范围;行列,越界 clamp)。
///
/// 单一真相:数字在这里算一次,
/// ① `doc.changes` 的 JS 数组 ② 事件 payload 里的 `changes` —— 都由它派生。
fn changes_to_ev(changes: &[DocChange], ctx: &CommandContext) -> Vec<EvChange> {
    if changes.is_empty() {
        return Vec::new();
    }
    let old_start = changes.iter().map(|c| c.0 .0).min().unwrap();
    let old_end = changes.iter().map(|c| c.0 .1).max().unwrap();
    let new_start = changes.iter().map(|c| c.1 .0).min().unwrap();
    let new_end = changes.iter().map(|c| c.1 .1).max().unwrap();
    vec![EvChange {
        old_range: EvRange {
            start: pos_to_row_col(&ctx.text, old_start).into(),
            end: pos_to_row_col(&ctx.text, old_end).into(),
        },
        new_range: EvRange {
            start: pos_to_row_col(&ctx.text, new_start).into(),
            end: pos_to_row_col(&ctx.text, new_end).into(),
        },
    }]
}

/// 具名点 → JS `{row, col}`
fn ev_pos_js(engine: &mut Context, p: EvPos) -> JsValue {
    JsValue::from(
        ObjectInitializer::new(engine)
            .property(
                JsString::from("row"),
                JsValue::from(p.row),
                Attribute::all(),
            )
            .property(
                JsString::from("col"),
                JsValue::from(p.col),
                Attribute::all(),
            )
            .build(),
    )
}

/// 具名范围 → JS `{start, end}`
fn ev_range_js(engine: &mut Context, r: EvRange) -> JsValue {
    let start = ev_pos_js(engine, r.start);
    let end = ev_pos_js(engine, r.end);
    JsValue::from(
        ObjectInitializer::new(engine)
            .property(JsString::from("start"), start, Attribute::all())
            .property(JsString::from("end"), end, Attribute::all())
            .build(),
    )
}

/// 具名变更 → JS `{oldRange, newRange}`(与 payload 的 `EvChange` **同形同源**)
fn change_js(engine: &mut Context, c: &EvChange) -> JsValue {
    let old_range = ev_range_js(engine, c.old_range);
    let new_range = ev_range_js(engine, c.new_range);
    JsValue::from(
        ObjectInitializer::new(engine)
            .property(JsString::from("oldRange"), old_range, Attribute::all())
            .property(JsString::from("newRange"), new_range, Attribute::all())
            .build(),
    )
}

/// 事件名 + 上下文 → 具名 payload(B-1)。
///
/// 名字只能来自 `EVENT_WHITELIST`(不在表里的名字早在 `helix.on` 就被拒),所以这里
/// 只覆盖白名单里的**文档类**事件;变体选择**不影响线格式** —— 线上的 `type` 来自 `hdr`。
fn build_event_payload(
    name: &str,
    ctx: &CommandContext,
    extra: Option<&str>,
    changes: &[EvChange],
    startup_open_paths: usize,
    cwd: Option<(&Path, Option<&Path>)>,
    doc_id: Option<u64>,
) -> EventPayload {
    let hdr = BusHeader::new(name);
    let cursor = ctx.cursor.into();
    let selection = ctx.selection.into();
    let path = || ctx.path.clone();
    match name {
        "startup" => EventPayload::Startup(StartupPayload {
            hdr,
            open_paths: startup_open_paths,
        }),
        "layout-change" => EventPayload::LayoutChange(LayoutChangePayload {
            hdr,
            kind: extra.map(|k| k.to_string()),
        }),
        "mode-change" => EventPayload::ModeChange(ModeChangePayload {
            hdr,
            mode: extra.unwrap_or("normal").to_string(),
            path: path(),
            cursor,
            selection,
        }),
        "cwd-change" => EventPayload::CwdChange(CwdChangePayload {
            hdr,
            // 缺 cwd 参数 = 发射点写错了；空串比 panic 好（插件能看到"没拿到"）
            cwd: cwd
                .map(|(cwd, _)| cwd.to_string_lossy().to_string())
                .unwrap_or_default(),
            prev_cwd: cwd
                .and_then(|(_, prev)| prev)
                .map(|p| p.to_string_lossy().to_string()),
        }),
        "doc-change" => EventPayload::DocChange(DocChangePayload {
            hdr,
            doc_id: doc_id.unwrap_or(0),
            path: path(),
            cursor,
            selection,
            changes: changes.to_vec(),
        }),
        "buffer-open" => EventPayload::BufferOpen(DocEventPayload {
            hdr,
            path: path(),
            cursor,
            selection,
        }),
        "buffer-close" => EventPayload::BufferClose(DocEventPayload {
            hdr,
            path: path(),
            cursor,
            selection,
        }),
        "theme-change" => EventPayload::ThemeChange(DocEventPayload {
            hdr,
            path: path(),
            cursor,
            selection,
        }),
        _ => EventPayload::Save(DocEventPayload {
            hdr,
            path: path(),
            cursor,
            selection,
        }),
    }
}

/// 具名 payload → JS 对象(`serde` → JSON → boa)。
///
/// 走 JSON 而不是手写 `ObjectInitializer`:线格式**只在一处**定义(types.rs 的结构体),
/// 这里不重复描述字段名 —— 否则"具名"会退化成两份会漂移的形状。
pub(crate) fn payload_js(
    engine: &mut Context,
    payload: &EventPayload,
) -> boa_engine::JsResult<JsValue> {
    JsValue::from_json(&payload.to_json(), engine)
}

/// char 索引 → 0-based (row, col)；越界 clamp 到文本末尾。
fn pos_to_row_col(text: &str, pos: usize) -> (usize, usize) {
    let mut row = 0;
    let mut col = 0;
    for (n, ch) in text.chars().enumerate() {
        if n >= pos {
            break;
        }
        if ch == '\n' {
            row += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (row, col)
}

/// 钩子调用：按注册顺序调用 name 的 handler，返回第一个非 undefined 返回值。
/// 通知型钩子（term-open/mode-change/exit/resize/title）忽略返回值；
/// term-close 返回 false 表示阻止关闭。参数由调用方构造（原生 JsValue 无需 engine）。
pub fn emit_hook(name: &str, args: &[JsValue]) -> Option<JsValue> {
    emit_hook_args(name, args, None)
}

/// 钩子调用的公共实现(B-1 后):`payload` 为 Some 时,它就是**追加在位置参数后的最后一个参数**。
fn emit_hook_args(name: &str, args: &[JsValue], payload: Option<&EventPayload>) -> Option<JsValue> {
    crate::init();
    let handlers = crate::state::with_event_handlers_current(|h| h.get(name).cloned())?;
    if handlers.is_empty() {
        return None;
    }
    with_engine(|engine| {
        let mut args: Vec<JsValue> = args.to_vec();
        if let Some(payload) = payload {
            match payload_js(engine, payload) {
                Ok(p) => args.push(p),
                // 唯一可能:`serde_json` 序列化失败(字段全是基础类型 ⇒ 实际不会发生)。
                // 不静默发半个 payload(那会让插件收到形状不全的事件),直接不发。
                Err(_) => return None,
            }
        }
        let undefined = JsValue::undefined();
        for handler in &handlers {
            let Some(func) = handler.as_callable().and_then(JsFunction::from_object) else {
                continue;
            };
            if let Ok(ret) = func.call(&undefined, &args, engine) {
                if !ret.is_undefined() {
                    return Some(ret);
                }
            }
        }
        None
    })
}

/// term-key 钩子的决策结果（插件返回值映射）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TermKeyDecision {
    /// 直通 pty
    Pass,
    /// 消费按键（不直通）
    Consume,
    /// 最小化终端叶子
    Minimize,
    /// 关闭终端（杀 pty）
    Close,
}

/// term-key 钩子：构造 { code, shift, ctrl, alt } 对象传给 handler；
/// 返回 "pass" | "consume" | "minimize" | "close"（字符串）→ 对应决策；其他/无 handler → None（走默认）。
pub fn emit_term_key(
    st: &HostState,
    pty_id: u64,
    code: &str,
    shift: bool,
    ctrl: bool,
    alt: bool,
) -> Option<TermKeyDecision> {
    crate::init();
    let handlers = crate::state::with_event_handlers(st, |h| h.get("term-key").cloned())?;
    if handlers.is_empty() {
        return None;
    }
    with_engine(|engine| {
        let key = ObjectInitializer::new(engine)
            .property(
                JsString::from("code"),
                JsValue::from(JsString::from(code)),
                Attribute::all(),
            )
            .property(
                JsString::from("shift"),
                JsValue::from(shift),
                Attribute::all(),
            )
            .property(
                JsString::from("ctrl"),
                JsValue::from(ctrl),
                Attribute::all(),
            )
            .property(JsString::from("alt"), JsValue::from(alt), Attribute::all())
            .build();
        let mut args = vec![JsValue::from(pty_id), JsValue::from(key)];
        // B-1:具名 payload 追加为最后一个参数(位置参数不变)
        if let Ok(p) = payload_js(
            engine,
            &EventPayload::TermKey(TermKeyPayload {
                hdr: BusHeader::new("term-key"),
                pty_id,
                code: code.to_string(),
                shift,
                ctrl,
                alt,
            }),
        ) {
            args.push(p);
        }
        let undefined = JsValue::undefined();
        for handler in &handlers {
            let Some(func) = handler.as_callable().and_then(JsFunction::from_object) else {
                continue;
            };
            let Ok(ret) = func.call(&undefined, &args, engine) else {
                continue;
            };
            let s = ret.as_string().map(|s| s.to_std_string_escaped());
            let decision = match s.as_deref() {
                Some("pass") => Some(TermKeyDecision::Pass),
                Some("consume") => Some(TermKeyDecision::Consume),
                Some("minimize") => Some(TermKeyDecision::Minimize),
                Some("close") => Some(TermKeyDecision::Close),
                _ => None,
            };
            if let Some(d) = decision {
                return Some(d);
            }
        }
        None
    })
}

/// 平级模式切换通知(阶段①/②):插件据此显示模式指示/键位表
pub fn emit_pane_mode_change(mode: &str) {
    emit_hook_args(
        "pane-mode-change",
        &[JsValue::from(JsString::from(mode))],
        Some(&EventPayload::PaneModeChange(PaneModeChangePayload {
            hdr: BusHeader::new("pane-mode-change"),
            mode: mode.to_string(),
        })),
    );
}

/// 通知型终端钩子封装（helix-term 侧不依赖 boa，统一走这里）：
/// term-open / term-mode-change / term-exit / term-resize / term-title 通知；
/// term-close 返回 true=放行关闭 / false=阻止。
///
/// B-1 起：位置参数不变，**末尾**追加一个具名 payload（`{api_version, type, …}`）。
pub fn emit_term_open(pty_id: u64, cmd: &str) {
    emit_hook_args(
        "term-open",
        &[JsValue::from(pty_id), JsValue::from(JsString::from(cmd))],
        Some(&EventPayload::TermOpen(TermOpenPayload {
            hdr: BusHeader::new("term-open"),
            pty_id,
            cmd: cmd.to_string(),
        })),
    );
}
pub fn emit_term_mode(pty_id: u64, mode: &str) {
    emit_hook_args(
        "term-mode-change",
        &[JsValue::from(pty_id), JsValue::from(JsString::from(mode))],
        Some(&EventPayload::TermModeChange(TermModeChangePayload {
            hdr: BusHeader::new("term-mode-change"),
            pty_id,
            mode: mode.to_string(),
        })),
    );
}
pub(crate) fn emit_term_exit(pty_id: u64, code: i32) {
    emit_hook_args(
        "term-exit",
        &[JsValue::from(pty_id), JsValue::from(code)],
        Some(&EventPayload::TermExit(TermExitPayload {
            hdr: BusHeader::new("term-exit"),
            pty_id,
            code,
        })),
    );
}
pub fn emit_term_resize(pty_id: u64, rows: u16, cols: u16) {
    emit_hook_args(
        "term-resize",
        &[
            JsValue::from(pty_id),
            JsValue::from(rows),
            JsValue::from(cols),
        ],
        Some(&EventPayload::TermResize(TermResizePayload {
            hdr: BusHeader::new("term-resize"),
            pty_id,
            rows,
            cols,
        })),
    );
}
pub fn emit_term_title(pty_id: u64, title: &str) {
    emit_hook_args(
        "term-title",
        &[JsValue::from(pty_id), JsValue::from(JsString::from(title))],
        Some(&EventPayload::TermTitle(TermTitlePayload {
            hdr: BusHeader::new("term-title"),
            pty_id,
            title: title.to_string(),
        })),
    );
}
/// 组件事件(鼠标点击/滚动等):构造 {kind, x, y} 对象;插件返回 true → 消费该事件。
pub fn emit_component_event(st: &HostState, id: u64, kind: &str, x: u16, y: u16) -> bool {
    crate::init();
    let handlers = crate::state::with_event_handlers(st, |h| h.get("component-event").cloned());
    let Some(handlers) = handlers else {
        return false;
    };
    if handlers.is_empty() {
        return false;
    }
    with_engine(|engine| {
        let ev = ObjectInitializer::new(engine)
            .property(
                JsString::from("kind"),
                JsValue::from(JsString::from(kind)),
                Attribute::all(),
            )
            .property(JsString::from("x"), JsValue::from(x), Attribute::all())
            .property(JsString::from("y"), JsValue::from(y), Attribute::all())
            .build();
        let mut args = vec![JsValue::from(id), JsValue::from(ev)];
        // B-1:具名 payload 追加为最后一个参数(位置参数不变)
        if let Ok(p) = payload_js(
            engine,
            &EventPayload::ComponentEvent(ComponentEventPayload {
                hdr: BusHeader::new("component-event"),
                id,
                kind: kind.to_string(),
                x,
                y,
            }),
        ) {
            args.push(p);
        }
        let undefined = JsValue::undefined();
        for handler in &handlers {
            let Some(func) = handler.as_callable().and_then(JsFunction::from_object) else {
                continue;
            };
            if let Ok(ret) = func.call(&undefined, &args, engine) {
                if ret.as_boolean() == Some(true) {
                    return true;
                }
            }
        }
        false
    })
}

/// term-close 钩子：插件返回 false → 阻止关闭（返回 true=阻止）
pub fn emit_term_close(pty_id: u64, reason: &str) -> bool {
    emit_hook_args(
        "term-close",
        &[JsValue::from(pty_id), JsValue::from(JsString::from(reason))],
        Some(&EventPayload::TermClose(TermClosePayload {
            hdr: BusHeader::new("term-close"),
            pty_id,
            reason: reason.to_string(),
        })),
    )
    .is_some_and(|v| v.as_boolean() == Some(false))
}

/// term_state(ptyId, key[, value])：终端任意状态持久化（跨会话存盘）。
/// 读：值不存在返回 undefined；写：value 必须是字符串（覆盖）。
/// 存储：~/.local/state/helix/term-state.json（`{"<ptyId>": {"<key>": "<value>"}}`）。
pub(crate) fn js_term_state(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let pty_id: u64 = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)
        .map_err(|_| crate::js_err("term_state: ptyId must be a number"))?;
    let key: String = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)
        .map_err(|_| crate::js_err("term_state: key must be a string"))?;
    let value = match args.get(2) {
        Some(v) => v,
        None => &JsValue::undefined(),
    };
    if value.is_undefined() {
        // 读
        return Ok(read_term_state(pty_id, &key)
            .map(|v| JsValue::from(JsString::from(v)))
            .unwrap_or(JsValue::undefined()));
    }
    let v: String = value
        .try_js_into(context)
        .map_err(|_| crate::js_err("term_state: value must be a string"))?;
    write_term_state(pty_id, &key, &v);
    Ok(JsValue::undefined())
}

fn term_state_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".local/state/helix")
        .join("term-state.json")
}

fn load_term_state() -> serde_json::Map<String, serde_json::Value> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = LOCK.lock().unwrap();
    let path = term_state_path();
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    serde_json::from_str::<serde_json::Value>(&content)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn save_term_state(map: &serde_json::Map<String, serde_json::Value>) {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = LOCK.lock().unwrap();
    let path = term_state_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, serde_json::to_string_pretty(map).unwrap_or_default());
}

fn read_term_state(pty_id: u64, key: &str) -> Option<String> {
    load_term_state()
        .get(&pty_id.to_string())
        .and_then(|m| m.get(key))
        .and_then(|v| v.as_str().map(|s| s.to_string()))
}

fn write_term_state(pty_id: u64, key: &str, value: &str) {
    let mut map = load_term_state();
    let entry = map
        .entry(pty_id.to_string())
        .or_insert_with(|| serde_json::Value::Object(Default::default()));
    if let Some(obj) = entry.as_object_mut() {
        obj.insert(
            key.to_string(),
            serde_json::Value::String(value.to_string()),
        );
    }
    save_term_state(&map);
}

pub(crate) fn js_set_cursor(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let row: usize = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let col: usize = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    crate::state::with_cursor_requests(crate::host::host_state(context), |c| {
        c.push(CursorRequest::SetCursor { row, col })
    });
    Ok(JsValue::undefined())
}

pub(crate) fn js_set_selection(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    // 第一参是数组 → 多选区形态；否则走 4 参单选区兼容逻辑
    if let Some(obj) = args.first().unwrap_or(&JsValue::undefined()).as_object() {
        if obj.is_array() {
            let arr = JsArray::from_object(obj.clone()).expect("is_array checked");
            let len = arr.length(context)?;
            if len == 0 {
                return Err(crate::js_err("set_selection: empty selection array"));
            }
            let mut selections = Vec::with_capacity(len as usize);
            for i in 0..len {
                let elem = arr.get(i, context)?;
                let el = elem.as_object().ok_or_else(|| {
                    crate::js_err("set_selection: each array element must be { anchor, head }")
                })?;
                let anchor = parse_sel_pos(&el.get(JsString::from("anchor"), context)?, context)?;
                let head = parse_sel_pos(&el.get(JsString::from("head"), context)?, context)?;
                selections.push((anchor, head));
            }
            crate::state::with_cursor_requests(crate::host::host_state(context), |c| {
                c.push(CursorRequest::SetSelections(selections))
            });
            return Ok(JsValue::undefined());
        }
    }
    let ar: usize = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let ac: usize = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let hr: usize = args
        .get(2)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let hc: usize = args
        .get(3)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    crate::state::with_cursor_requests(crate::host::host_state(context), |c| {
        c.push(CursorRequest::SetSelection {
            anchor: (ar, ac),
            head: (hr, hc),
        })
    });
    Ok(JsValue::undefined())
}

/// 解析多选区的 { row, col }——字段缺失/非对象 → Err（与 parse_pos 的缺省语义不同，
/// 多选区坐标必须完整显式）
fn parse_sel_pos(v: &JsValue, ctx: &mut Context) -> boa_engine::JsResult<(usize, usize)> {
    let obj = v
        .as_object()
        .ok_or_else(|| crate::js_err("set_selection: anchor/head must be { row, col }"))?;
    let row = obj.get(JsString::from("row"), ctx)?;
    let col = obj.get(JsString::from("col"), ctx)?;
    if row.is_null_or_undefined() || col.is_null_or_undefined() {
        return Err(crate::js_err("set_selection: anchor/head missing row/col"));
    }
    Ok((
        row.try_js_into::<usize>(ctx)?,
        col.try_js_into::<usize>(ctx)?,
    ))
}

/// shell 执行输出截断上限（防失控输出冻结状态栏）
const RUN_OUTPUT_LIMIT: usize = 65536;

// ponytail: 同步阻塞 + sh -c，仅 Unix；未来要 Windows 支持需改 cmd.exe /C。
pub(crate) fn js_run(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    use std::process::Command;
    // C-2 检查点:起进程需要 `process` 能力
    let cmd: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let output = Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .output()
        .map_err(|e| crate::js_err(format!("helix.run: failed to spawn: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr: String = stderr.chars().take(RUN_OUTPUT_LIMIT).collect();
        let code = output
            .status
            .code()
            .map_or_else(|| "signal".to_string(), |c| c.to_string());
        return Err(crate::js_err(format!(
            "helix.run: command failed ({code}): {stderr}"
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut stdout: String = stdout.chars().take(RUN_OUTPUT_LIMIT).collect();
    if output.stdout.len() > RUN_OUTPUT_LIMIT {
        stdout.push_str("(truncated)");
    }
    Ok(JsValue::from(JsString::from(stdout)))
}

pub(crate) fn js_echo(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let text: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    crate::host::with_messages(|v| v.push(text));
    Ok(JsValue::undefined())
}

/// 读线程主循环（stdout/stderr 共用）。
/// aggregate=true：原始字节累积进 output，EOF 后统一 lossy 解码（跨块多字节字符不被切开）；
/// aggregate=false：流式增量解码——拼上跨块的残留尾部（最多 3 字节，UTF-8 最长序列），
/// 从 doc 对象读编辑目标：this._target 为字符串 → Some(path)；null/非对象 → None（当前 buffer）
fn edit_target(_this: &JsValue, context: &mut Context) -> boa_engine::JsResult<Option<String>> {
    let Some(obj) = _this.as_object() else {
        return Ok(None);
    };
    let v = obj.get(JsString::from("_target"), context)?;
    if v.is_null_or_undefined() {
        Ok(None)
    } else {
        v.try_js_into(context).map(Some)
    }
}

pub(crate) fn js_doc_insert(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let row: usize = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let col: usize = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let insert: String = args
        .get(2)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let doc = edit_target(_this, context)?;
    crate::state::with_edits(crate::host::host_state(context), |c| {
        c.push(Edit {
            doc,
            start: (row, col),
            end: (row, col),
            insert,
        })
    });
    Ok(JsValue::undefined())
}

pub(crate) fn js_doc_replace(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let sr: usize = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let sc: usize = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let er: usize = args
        .get(2)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let ec: usize = args
        .get(3)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let insert: String = args
        .get(4)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let doc = edit_target(_this, context)?;
    crate::state::with_edits(crate::host::host_state(context), |c| {
        c.push(Edit {
            doc,
            start: (sr, sc),
            end: (er, ec),
            insert,
        })
    });
    Ok(JsValue::undefined())
}

pub(crate) fn js_doc_delete(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let sr: usize = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let sc: usize = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let er: usize = args
        .get(2)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let ec: usize = args
        .get(3)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let doc = edit_target(_this, context)?;
    crate::state::with_edits(crate::host::host_state(context), |c| {
        c.push(Edit {
            doc,
            start: (sr, sc),
            end: (er, ec),
            insert: String::new(),
        })
    });
    Ok(JsValue::undefined())
}

/// helix.set_virtual_text(path[, row, col, text, style])：行内文本装饰入队。
/// 只传 path（text 省略/undefined）→ Clear：清除该 doc 全部插件装饰。
/// 路径 canonicalize（与 js_by_path / Edit.doc 同款）；未打开 doc 应用时静默忽略（term 侧）。
pub(crate) fn js_set_virtual_text(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let path: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)
        .map_err(|_| crate::js_err("helix.set_virtual_text: path must be a string"))?;
    let canonical = helix_stdx::path::canonicalize(&path)
        .to_string_lossy()
        .into_owned();
    // 清除语义:只传 path(row/col/text 均省略)= Clear;给了坐标却缺 text → 走类型解析报 TypeError
    let is_clear = match args.get(3) {
        None => args.get(1).is_none() && args.get(2).is_none(),
        Some(v) if v.is_null_or_undefined() => args.get(1).is_none() && args.get(2).is_none(),
        _ => false,
    };
    let kind = if is_clear {
        DecorationKind::Clear
    } else {
        let row: usize = args
            .get(1)
            .unwrap_or(&JsValue::undefined())
            .try_js_into(context)?;
        let col: usize = args
            .get(2)
            .unwrap_or(&JsValue::undefined())
            .try_js_into(context)?;
        let text: String = args
            .get(3)
            .unwrap_or(&JsValue::undefined())
            .try_js_into(context)?;
        let style: Option<String> = match args.get(4) {
            Some(v) if !v.is_null_or_undefined() => Some(v.try_js_into(context)?),
            _ => None,
        };
        DecorationKind::VirtualText {
            row,
            col,
            text,
            style,
        }
    };
    crate::state::with_decoration_requests(crate::host::host_state(context), |c| {
        c.push(DecorationRequest {
            doc: Some(canonical),
            kind,
        })
    });
    Ok(JsValue::undefined())
}

/// helix.set_highlight(path, sr, sc, er, ec[, style])：区域高亮装饰入队。
/// 路径 canonicalize（与 js_by_path / Edit.doc 同款）；style 省略/undefined → None。
pub(crate) fn js_set_highlight(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let path: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)
        .map_err(|_| crate::js_err("helix.set_highlight: path must be a string"))?;
    let canonical = helix_stdx::path::canonicalize(&path)
        .to_string_lossy()
        .into_owned();
    let sr: usize = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let sc: usize = args
        .get(2)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let er: usize = args
        .get(3)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let ec: usize = args
        .get(4)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let style: Option<String> = match args.get(5) {
        Some(v) if !v.is_null_or_undefined() => Some(v.try_js_into(context)?),
        _ => None,
    };
    crate::state::with_decoration_requests(crate::host::host_state(context), |c| {
        c.push(DecorationRequest {
            doc: Some(canonical),
            kind: DecorationKind::Highlight {
                sr,
                sc,
                er,
                ec,
                style,
            },
        })
    });
    Ok(JsValue::undefined())
}

/// 取走并清空编辑队列（helix-term 在命令返回后消费）；事务开启时积压不消费
pub fn take_edits() -> Vec<Edit> {
    crate::init();
    if crate::state::with_txn_depth_current(|d| *d > 0) {
        // 事务打开:积压不消费。未配对的 begin(不 end)在下一命令/事件入口
        // 随深度一起清队列,积压编辑丢弃(不崩),见规格 §4
        return Vec::new();
    }
    crate::state::with_edits_current(std::mem::take)
}

/// 取走并清空装饰请求队列（helix-term 在命令返回后消费）；事务开启时积压不消费。
/// 与 take_edits 同构：未配对的 begin 在下一命令/事件入口随队列清空丢弃。
pub fn take_decorations() -> Vec<DecorationRequest> {
    crate::init();
    if crate::state::with_txn_depth_current(|d| *d > 0) {
        return Vec::new();
    }
    crate::state::with_decoration_requests_current(std::mem::take)
}

/// 声明批量编辑事务开始:期间编辑积压,end 后合并取走(一次撤销)
pub(crate) fn js_begin_edit(
    _this: &JsValue,
    _args: &[JsValue],
    _ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    crate::state::with_txn_depth_current(|d| *d += 1);
    Ok(JsValue::undefined())
}

/// 声明批量编辑事务结束:积压编辑可被 take_edits 取走
pub(crate) fn js_end_edit(
    _this: &JsValue,
    _args: &[JsValue],
    _ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    crate::state::with_txn_depth_current(|d| *d = d.saturating_sub(1));
    Ok(JsValue::undefined())
}

/// 触发节点事件（焦点路由）：onPress → 无参调用；onKey → 传键名
pub fn dispatch_node_event(
    st: &HostState,
    view_id: u64,
    node_id: &str,
    key: Option<&str>,
) -> Result<()> {
    crate::init();
    let result = crate::state::with_engine(|engine| {
        let handlers = crate::state::with_node_handlers(st, |h| {
            h.get(&(view_id, node_id.to_string()))
                .cloned()
                .map(|hd| NodeHandlers {
                    on_press: hd.on_press.clone(),
                    on_key: hd.on_key.clone(),
                    on_change: hd.on_change.clone(),
                })
        });
        let Some(handlers) = handlers else {
            return Ok(());
        };
        // C-2 归属:节点回调的属主来自**注册期**(`parse_node` 时绑定),不再回退
        // "加载期最后声明的插件"。早先这里没有属主 ⇒ onPress 里调 `helix.run` 会被
        // 按别人的权限判定(最后加载的是内置插件时 = 直接放行)。
        let cb_key = crate::permissions::CbKey::node(view_id, node_id);
        let undefined = JsValue::undefined();
        match key {
            None => {
                // button 按下（Enter/Space）
                if let Some(f) = handlers.on_press {
                    let func = f
                        .as_callable()
                        .and_then(JsFunction::from_object)
                        .ok_or_else(|| anyhow!("node {node_id} onPress not callable"))?;
                    let _: JsValue = crate::permissions::with_callback_owner(&cb_key, || {
                        func.call(&undefined, &[], engine)
                    })
                    .map_err(|e| anyhow!("node {node_id} onPress failed: {e}"))?;
                }
            }
            Some(k) => {
                // input 按键
                if let Some(f) = handlers.on_key {
                    let func = f
                        .as_callable()
                        .and_then(JsFunction::from_object)
                        .ok_or_else(|| anyhow!("node {node_id} onKey not callable"))?;
                    let _: JsValue = crate::permissions::with_callback_owner(&cb_key, || {
                        func.call(
                            &undefined,
                            &[JsValue::from(JsString::from(k.to_string()))],
                            engine,
                        )
                    })
                    .map_err(|e| anyhow!("node {node_id} onKey failed: {e}"))?;
                }
            }
        }
        Ok(())
    });
    // 失败可见:helix-term 的调用点用的是 `.is_ok()`(失败 = 不消费按键),错误若只
    // 往上传就会被**静默吞掉** —— 权限拒绝、插件抛错都会表现成"按键没反应"。
    // 这里统一投到消息队列(下一帧/命令边界上状态栏可见),返回值保持原样。
    if let Err(err) = &result {
        crate::host::with_messages(|m| m.push(format!("node '{node_id}' 事件失败: {err}")));
    }
    result
}

/// 收集组件树中的可聚焦节点 id（button/input，树序）
pub fn focusable_node_ids(node: &CompNode, out: &mut Vec<String>) {
    match node {
        CompNode::Button { id, .. } => out.push(id.clone()),
        CompNode::Input { id, .. } => out.push(id.clone()),
        CompNode::Row { children, .. }
        | CompNode::Col { children, .. }
        | CompNode::Box { children, .. }
        | CompNode::Scroll { children, .. } => {
            for c in children {
                focusable_node_ids(c, out);
            }
        }
        CompNode::Text { .. } => {}
    }
}

/// 取走并清空光标/选区请求队列（helix-term 消费）
pub fn take_cursor_requests() -> Vec<CursorRequest> {
    crate::init();
    crate::state::with_cursor_requests_current(std::mem::take)
}

/// 求值一段插件脚本（无名字）。**测试/内联专用** —— 没有宿主句柄 ⇒ 经**捕获入口**取态。
/// 生产路径请用 [`load_script_named`](fn.load_script_named.html)。
pub fn load_script(src: &str) -> Result<()> {
    let st = crate::host::state_handle_or_init();
    load_script_named(&st, "<anon>", src)
}

/// IIFE 包裹求值：每次求值（含 reload 重跑）都得到全新词法作用域，
/// 顶层 const/let 不再与上一次求值冲突；顺带隔离脚本间全局污染。
/// `helix` 是全局属性，IIFE 内可正常访问；错误消息定位脚本名。
fn eval_wrapped(engine: &mut boa_engine::Context, src: &str) -> boa_engine::JsResult<JsValue> {
    let wrapped = format!("(function() {{\n{src}\n}})()");
    engine.eval(Source::from_bytes(&wrapped))
}

/// 求值并记录脚本（名字用于报错定位与热重载）
pub fn load_script_named(st: &HostState, name: &str, src: &str) -> Result<()> {
    crate::init();
    // 脚本(重)加载不继承旧装饰:全局 Clear(term 侧 doc: None = 清空所有 doc)
    crate::state::with_decoration_requests(st, |c| {
        c.push(DecorationRequest {
            doc: None,
            kind: DecorationKind::Clear,
        })
    });
    // C-3 加载期属主:由文件名兜底(`:plugin-load /abs/x.js` ⇒ "x")。
    // 脚本内 `helix.plugin("y")` 会再升级成 y。
    let owner = crate::permissions::plugin_name_from_key(name).or_else(|| {
        std::path::Path::new(name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
    });
    let prev_owner = owner
        .as_deref()
        .map(|o| crate::permissions::set_current_owner(Some(o)));
    let result = crate::state::with_engine(|engine| {
        eval_wrapped(engine, src)
            .map(|_| ())
            .map_err(|e| anyhow!("plugin script '{name}' error: {e}"))?;
        crate::state::with_loaded_scripts(|s| s.push((name.to_string(), src.to_string())));
        Ok(())
    });
    if let Some(prev) = prev_owner {
        crate::permissions::restore_owner(prev);
    }
    result
}

/// 脚本(重)执行后重建插件配置:define_config 只在脚本执行时才注册 schema,而 CONFIGS
/// 由启动 / `:config-reload` 构建 ⇒ `helix.load` 懒加载的插件本来一直拿到 null(默认值全丢)。
///
/// 为什么在 helix-js 侧:懒加载走 `load_script_checked`,不经过 helix-term 的 `:plugin-load`,
/// 那边没有可挂的边界。校验错误进 echo 队列(下个命令/事件边界上状态栏可见,不静默丢)。
pub(crate) fn refresh_configs_after_load() {
    let st = crate::host::state_handle_or_init();
    let (json, errors) = crate::config::rebuild_configs(&st);
    crate::state::cache_configs(&st, &json);
    for e in errors {
        crate::host::push_message(format!("plugin config: {e}"));
    }
}

/// 已加载脚本名（按加载顺序；供 :plugin list / status）
pub fn loaded_scripts() -> Vec<String> {
    // ⚠️ **不要在这里调 `crate::init()`** ——(2026-10-06 定位到的真因)
    // 读"已加载脚本"只是读一个**线程局部**;而 `init()` 每次都会走
    // `state::with_engine_slot(|slot| …)` ⇒ `CONTEXT.borrow_mut()`。
    // 本函数会被**命令执行路径**调用(`:plugin list` · `helix.loaded_plugins()`),
    // 而外层 `run_command` 正持有 CONTEXT 的借用 ⇒ **再借即 panic**
    // (panic 点:`state.rs:138`,与 `helix.load` 注释里警告过的是同一类)。
    crate::state::with_loaded_scripts(|s| s.iter().map(|(name, _)| name.clone()).collect())
}

/// 清空所有插件状态（热重载用；id 计数器保留保证唯一性）。
/// 注意：UI_REQUESTS/MESSAGES 不清——它们是命令边界 drain 的队列。
/// 回收**没有属主**的注册(裸脚本 `:plugin-load x.js` / 单测里直接 `eval` 的脚本)。
///
/// 属主判据与 `unload_plugin` 同一套:注册期绑定的插件名。区别只是这里找的是
/// "注册表里有它、属主表里没有它"的条目 —— 它们没有名字可供 `:plugin-unload`,只在
/// 全部重载时由这条兜底清掉。
///
/// 返回清掉的注册条数(relabel 用;不计入 `UnloadReport`)。
pub fn unload_orphans(st: &HostState) -> usize {
    orphan_commands(st)
        + orphan_handlers(st)
        + orphan_popups(st)
        + orphan_hooks(st)
        + orphan_sources_and_nodes(st)
        + orphan_theme_scopes(st)
}

/// 无主命令(注册表里有、`command_owners` 里没有)+ 它们的命令说明
fn orphan_commands(st: &HostState) -> usize {
    let orphans: Vec<String> =
        crate::state::with_registry(st, |r| r.keys().cloned().collect::<Vec<_>>())
            .into_iter()
            .filter(|c| crate::permissions::command_owner(c).is_none())
            .collect();
    if orphans.is_empty() {
        return 0;
    }
    crate::state::with_registry(st, |r| {
        for c in &orphans {
            r.remove(c);
        }
    });
    crate::state::with_command_docs(st, |d| {
        for c in &orphans {
            d.remove(c);
        }
    });
    orphans.len()
}

/// 无主事件处理器:逐事件按"属主缺项"重建两侧(下标先算,双侧同步 —— 与 `unload_plugin` 同款)
fn orphan_handlers(st: &HostState) -> usize {
    let mut removed = 0usize;
    let events: Vec<String> =
        crate::state::with_event_handlers(st, |m| m.keys().cloned().collect::<Vec<_>>());
    for event in &events {
        let len =
            crate::state::with_event_handlers(st, |m| m.get(event).map(|v| v.len()).unwrap_or(0));
        if len == 0 {
            continue;
        }
        let keep: Vec<usize> = (0..len)
            .filter(|i| crate::permissions::handler_owner(event, *i).is_some())
            .collect();
        if keep.len() == len {
            continue;
        }
        removed += len - keep.len();
        crate::state::with_event_handlers(st, |m| {
            if let Some(list) = m.get_mut(event) {
                let mut idx = 0usize;
                list.retain(|_| {
                    let k = keep.contains(&idx);
                    idx += 1;
                    k
                });
            }
        });
        let owners: Vec<String> = keep
            .iter()
            .filter_map(|i| crate::permissions::handler_owner(event, *i))
            .collect();
        crate::permissions::set_handler_owners(event, owners);
    }
    removed
}

/// 无主弹窗/面板:入队关闭并摘表
fn orphan_popups(st: &HostState) -> usize {
    let open_panels = crate::state::with_open_panels(st, |p| p.clone());
    let popup_ids = crate::state::with_popups(st, |m| m.keys().cloned().collect::<Vec<_>>());
    let mut removed = 0usize;
    for id in popup_ids {
        if crate::permissions::callback_owner(&crate::permissions::CbKey::popup(id)).is_some() {
            continue;
        }
        if open_panels.contains(&id) {
            crate::state::with_open_panels(st, |p| p.retain(|x| *x != id));
            if crate::state::last_panel_id(st) == Some(id) {
                crate::state::set_last_panel_id(st, None);
            }
            UI_REQUESTS
                .get()
                .unwrap()
                .lock()
                .unwrap()
                .push(UiRequest::ClosePanel(ClosePanelReq { id }));
        } else {
            UI_REQUESTS
                .get()
                .unwrap()
                .lock()
                .unwrap()
                .push(UiRequest::ClosePopup(ClosePopupReq { id }));
        }
        crate::state::with_popups(st, |m| m.remove(&id));
        removed += 1;
    }
    removed
}

/// 无主单例钩子:槽里有值但属主表里没有
fn orphan_hooks(st: &HostState) -> usize {
    use crate::permissions::CbKey as K;
    let mut removed = 0usize;
    if crate::state::with_statusline_hook(st, |h| h.is_some())
        && crate::permissions::callback_owner(&K::hook(crate::permissions::CB_HOOK_STATUSLINE))
            .is_none()
    {
        crate::state::with_statusline_hook(st, |h| *h = None);
        crate::state::set_statusline_replace(st, false);
        removed += 1;
    }
    if crate::state::with_buffer_icon_hook(st, |h| h.is_some())
        && crate::permissions::callback_owner(&K::hook(crate::permissions::CB_HOOK_BUFFER_ICON))
            .is_none()
    {
        crate::state::with_buffer_icon_hook(st, |h| *h = None);
        removed += 1;
    }
    if crate::state::with_completion_icon_hook_current(|h| h.is_some())
        && crate::permissions::callback_owner(&K::hook(crate::permissions::CB_HOOK_COMPLETION_ICON))
            .is_none()
    {
        crate::state::with_completion_icon_hook_current(|h| *h = None);
        removed += 1;
    }
    if crate::state::with_completion_render_hook_current(|h| h.is_some())
        && crate::permissions::callback_owner(&K::hook(
            crate::permissions::CB_HOOK_COMPLETION_RENDER,
        ))
        .is_none()
    {
        crate::state::with_completion_render_hook_current(|h| *h = None);
        removed += 1;
    }
    if crate::state::with_keymap_hint_hook_current(|h| h.is_some())
        && crate::permissions::callback_owner(&K::hook(crate::permissions::CB_HOOK_KEYMAP_HINT))
            .is_none()
    {
        crate::state::with_keymap_hint_hook_current(|h| *h = None);
        removed += 1;
    }
    // 外部状态栏段是**按名字**的回调(同 picker 源),属主走了就删 —— 否则卸载后的插件
    // 还能继续往状态栏上画(每帧跑着已死插件的代码)。
    let segments: Vec<String> =
        crate::state::with_statusline_segments(st, |m| m.keys().cloned().collect());
    for name in segments {
        if crate::permissions::callback_owner(&K::statusline_segment(&name)).is_none() {
            crate::state::with_statusline_segments(st, |m| {
                m.remove(&name);
            });
            removed += 1;
        }
    }
    removed
}

/// 无主的 picker 源与节点回调
fn orphan_sources_and_nodes(st: &HostState) -> usize {
    use crate::permissions::CbKey as K;
    let mut removed = 0usize;
    let sources: Vec<String> =
        crate::state::with_picker_sources(st, |m| m.keys().cloned().collect::<Vec<_>>());
    for src in sources {
        if crate::permissions::callback_owner(&K::picker(&src)).is_none() {
            crate::state::with_picker_sources(st, |m| {
                m.remove(&src);
            });
            removed += 1;
        }
    }
    let nodes: Vec<(u64, String)> =
        crate::state::with_node_handlers(st, |m| m.keys().cloned().collect::<Vec<_>>());
    for (view, node) in nodes {
        if crate::permissions::callback_owner(&K::node(view, &node)).is_none() {
            crate::state::with_node_handlers(st, |m| {
                m.remove(&(view, node.clone()));
            });
            removed += 1;
        }
    }
    removed
}

/// 无主主题覆盖:摘掉并置脏
fn orphan_theme_scopes(st: &HostState) -> usize {
    let scopes: Vec<String> =
        crate::state::with_theme_overrides(st, |o| o.keys().cloned().collect::<Vec<_>>());
    let unowned: Vec<String> = scopes
        .into_iter()
        .filter(|sc| !crate::state::with_theme_owners(st, |m| m.contains_key(sc)))
        .collect();
    if unowned.is_empty() {
        return 0;
    }
    crate::state::with_theme_overrides(st, |o| {
        for sc in &unowned {
            o.remove(sc);
        }
    });
    crate::state::mark_theme_dirty(st);
    unowned.len()
}

/// 重载时的**加载期瞬态**(不属于任何插件的产物):编辑/装饰/光标队列、事务深度、
/// 主题脏位、加载栈、`last_export`。所有权归"本次加载",重跑脚本前必须清干净。
fn reset_transient_state(st: &HostState) {
    crate::state::with_edits(st, |c| c.clear());
    crate::state::with_decoration_requests(st, |c| c.clear());
    crate::state::with_cursor_requests(st, |c| c.clear());
    crate::state::with_txn_depth(st, |d| *d = 0);
    crate::state::set_last_panel_id(st, None);
    // 导出缓存与 pending 导出随加载重置（reload 后按名重读磁盘重跑）
    with_script_exports(|m| m.clear());
    // 加载栈防御性清空（正常路径已 pop；防止异常残留影响后续 reload）
    crate::state::with_load_stack(st, |s: &mut Vec<String>| s.clear());
    crate::state::clear_load_roots(st);
    crate::state::with_last_export(st, |l| *l = None);
}

/// `:plugin-unload <name>` 的回收结果(状态栏显示用)
#[derive(Debug, Default, Clone)]
pub struct UnloadReport {
    pub name: String,
    pub commands: usize,
    pub handlers: usize,
    pub hooks: usize,
    pub popups: usize,
    pub panels: usize,
    pub keymaps: usize,
    pub themes: usize,
    pub scripts: usize,
}

/// 由脚本 key 推出属主名(与 `reload_all` 同一规则:`a/plugin.js` → `a`,`x/y.js` → `y`)
fn owner_of_script_key(key: &str) -> Option<String> {
    crate::permissions::plugin_name_from_key(key).or_else(|| {
        std::path::Path::new(key)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
    })
}

/// 卸载一个插件:先跑它的 `onUnload`,再**按属主**回收它注册的一切。
///
/// 清理面:命令(含 `helix.map` 生成的 `__mapped_N`)、事件处理器、单例钩子、
/// 弹窗/面板、键位绑定、主题覆盖、picker 源、节点回调、watch、导出与加载清单。
///
/// **不做**的(写进文档,不假装做到):JS 全局变量/闭包(同一 Realm 无法回收)、插件自己
/// 起的 pty 与外部资源 —— 那些请插件在 `onUnload` 里自己关。
///
/// 允许在**该插件自己的命令里**调用:命令是从注册表**克隆**出来执行的
/// (`run_command` 先 `get().cloned()`),回收注册表不影响正在跑的那个函数。
pub fn unload_plugin(st: &HostState, name: &str) -> UnloadReport {
    crate::init();
    let mut report = UnloadReport {
        name: name.to_string(),
        ..Default::default()
    };

    // 1) onUnload **先**跑:错误只记录,绝不中断后面的回收(半卸载比不卸载更难查)
    run_unload_hook(st, name);

    // 2) 属主表:命令 / 事件处理器 / 其余回调
    let cleanup = crate::permissions::take_owner_registrations(st, name);
    report.commands = cleanup.commands.len();
    report.handlers = cleanup.handlers;
    remove_commands(st, &cleanup.commands);

    // 3) 回调:按种类分流(弹窗/面板要**入队关闭**,钩子清槽,源/节点表删除)
    reclaim_owned_callbacks(st, &cleanup.callbacks, &mut report);

    // 4) 键位 / 5) 主题覆盖 / 6) 导出与加载清单
    report.keymaps = unbind_owned_keymaps(st, name);
    report.themes = drop_owned_theme_scopes(st, name);
    report.scripts = forget_owned_scripts(name);

    report
}

/// 1) 卸载前跑 manifest 的 `onUnload`(错误只进消息队列,**不中断**回收)
fn run_unload_hook(st: &HostState, name: &str) {
    let Some(hook) = crate::state::take_unload_hook(st, name) else {
        return;
    };
    if let Some(func) = hook.as_callable().and_then(JsFunction::from_object) {
        let res = crate::state::with_engine(|engine| {
            func.call(&JsValue::undefined(), &[], engine).map(|_| ())
        });
        if let Err(e) = res {
            crate::host::push_message(format!("{name}: onUnload failed: {e}"));
        }
    }
}

/// 2) 从注册表与命令说明里删掉这些命令
fn remove_commands(st: &HostState, commands: &[String]) {
    if commands.is_empty() {
        return;
    }
    crate::state::with_registry(st, |r| {
        for c in commands {
            r.remove(c);
        }
    });
    crate::state::with_command_docs(st, |d| {
        for c in commands {
            d.remove(c);
        }
    });
}

/// 3) 回调按种类分流:弹窗/面板入队关闭、单例钩子清槽、picker 源/节点表删、watch 退订
fn reclaim_owned_callbacks(
    st: &HostState,
    callbacks: &[crate::permissions::CbKey],
    report: &mut UnloadReport,
) {
    // 3) 回调:按种类分流(弹窗/面板要**入队关闭**,钩子清槽,源/节点表删除)
    let open_panels = crate::state::with_open_panels(st, |p| p.clone());
    for key in callbacks {
        match key.kind() {
            crate::permissions::CB_POPUP => {
                let id = key.id();
                let is_panel = open_panels.contains(&id);
                if is_panel {
                    crate::state::with_open_panels(st, |p| p.retain(|x| *x != id));
                    if crate::state::last_panel_id(st) == Some(id) {
                        crate::state::set_last_panel_id(st, None);
                    }
                    UI_REQUESTS
                        .get()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .push(UiRequest::ClosePanel(ClosePanelReq { id }));
                    report.panels += 1;
                } else {
                    UI_REQUESTS
                        .get()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .push(UiRequest::ClosePopup(ClosePopupReq { id }));
                    report.popups += 1;
                }
                crate::state::with_popups(st, |m| m.remove(&id));
            }
            crate::permissions::CB_HOOK_STATUSLINE => {
                crate::state::with_statusline_hook(st, |h| *h = None);
                crate::state::set_statusline_replace(st, false);
                report.hooks += 1;
            }
            crate::permissions::CB_HOOK_BUFFER_ICON => {
                crate::state::with_buffer_icon_hook(st, |h| *h = None);
                report.hooks += 1;
            }
            crate::permissions::CB_HOOK_COMPLETION_ICON => {
                crate::state::with_completion_icon_hook_current(|h| *h = None);
                report.hooks += 1;
            }
            crate::permissions::CB_HOOK_COMPLETION_RENDER => {
                crate::state::with_completion_render_hook_current(|h| *h = None);
                report.hooks += 1;
            }
            crate::permissions::CB_HOOK_KEYMAP_HINT => {
                crate::state::with_keymap_hint_hook_current(|h| *h = None);
                report.hooks += 1;
            }
            crate::permissions::CB_NODE => {
                crate::state::with_node_handlers(st, |m| {
                    m.remove(&(key.id(), key.name().to_string()));
                });
            }
            crate::permissions::CB_PICKER => {
                crate::state::with_picker_sources(st, |m| {
                    m.remove(key.name());
                });
            }
            crate::permissions::CB_WATCH => {
                UI_REQUESTS
                    .get()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .push(UiRequest::Unwatch(UnwatchReq { id: key.id() }));
            }
            // 异步/LSP/终端/一次性行回调:只摘属主。资源(pty、promise)由插件在
            // `onUnload` 里自己收;这里不猜。
            _ => {}
        }
    }
}

/// 4) 该插件的每条键位入队解绑(term 侧 `remove_binding` 是真摘叶),返回条数
fn unbind_owned_keymaps(st: &HostState, name: &str) -> usize {
    // 4) 键位:该插件的每条绑定都入队解绑(term 侧 remove_binding 是真摘叶)
    let keys: Vec<(String, String)> = crate::state::with_keymap_owners(st, |m| {
        let mine: Vec<(String, String)> = m
            .iter()
            .filter(|(_, owner)| owner.as_str() == name)
            .map(|(k, _)| k.clone())
            .collect();
        for k in &mine {
            m.remove(k);
        }
        mine
    });
    for (mode, key) in &keys {
        UI_REQUESTS
            .get()
            .unwrap()
            .lock()
            .unwrap()
            .push(UiRequest::MapKey(MapKeyReq {
                mode: mode.clone(),
                key: key.clone(),
                command: String::new(),
                unbind: true,
            }));
    }
    keys.len()
}

/// 5) 摘掉该插件的主题 scope 并置脏(下次 drain 还原基准主题),返回 scope 数
fn drop_owned_theme_scopes(st: &HostState, name: &str) -> usize {
    // 5) 主题覆盖:摘掉该插件的 scope 并置脏(下次 drain 还原基准主题)
    let scopes: Vec<String> = crate::state::with_theme_owners(st, |m| {
        let mine: Vec<String> = m
            .iter()
            .filter(|(_, owner)| owner.as_str() == name)
            .map(|(s, _)| s.clone())
            .collect();
        for s in &mine {
            m.remove(s);
        }
        mine
    });
    if !scopes.is_empty() {
        crate::state::with_theme_overrides(st, |o| {
            for s in &scopes {
                o.remove(s);
            }
        });
        crate::state::mark_theme_dirty(st);
    }
    scopes.len()
}

/// 6) 从导出缓存与加载清单里摘掉该插件的脚本,返回脚本数
fn forget_owned_scripts(name: &str) -> usize {
    // 6) 导出与加载清单:按脚本 key 的派生名回收(manifest 名可能不同,两边都试)
    let stale: Vec<String> = crate::host::with_loaded_scripts(|s| {
        s.iter()
            .filter(|(k, _)| {
                let derived = owner_of_script_key(k);
                derived.as_deref() == Some(name)
            })
            .map(|(k, _)| k.clone())
            .collect()
    });
    for k in &stale {
        crate::host::with_loaded_scripts(|s| s.retain(|(n, _)| n != k));
        crate::host::with_script_exports(|m| {
            m.remove(k);
        });
    }
    stale.len()
}

/// 对**当前已知的所有属主**逐个 `unload_plugin`(`reload_all` 复用同一份清理逻辑)。
pub fn unload_all(st: &HostState) -> Vec<UnloadReport> {
    let mut owners: Vec<String> = Vec::new();
    for (key, _) in crate::host::with_loaded_scripts(|s| s.clone()) {
        if let Some(o) = owner_of_script_key(&key) {
            if !owners.contains(&o) {
                owners.push(o);
            }
        }
    }
    for o in crate::permissions::manifest_names() {
        if !owners.contains(&o) {
            owners.push(o);
        }
    }
    owners.into_iter().map(|o| unload_plugin(st, &o)).collect()
}

/// 热重载：清空状态后按加载顺序重跑全部脚本。
/// 脚本以 IIFE 包裹求值，每次重跑都是全新词法作用域——顶层 const/let/var 都不冲突，
/// reload 必然成功（脚本不依赖其他脚本的状态，reload 会重新注册全部命令/处理器），
/// 因此 Err 分支仅作防御（如脚本依赖被清空的全局状态），单测只覆盖成功路径。
/// **L4-④**:状态由调用方给出(`JsHost::reload_all` 经 `pump` 传入)。
pub fn reload_all(st: &HostState) -> Result<()> {
    crate::init();
    // 面板层还挂在 compositor 上——入队 ClosePanel 让 plugin_reload 的 apply_ui_requests 移除它们，
    // 否则 unload_all/unload_orphans 清掉 JS 注册表后这些层变成无法关闭的僵尸层（渲染报错 + 按键穿透）。
    // 只关 last_panel_id 会漏掉多面板场景，遍历全部。
    let open_panels = crate::state::with_open_panels(st, std::mem::take);
    for panel_id in open_panels {
        UI_REQUESTS
            .get()
            .unwrap()
            .lock()
            .unwrap()
            .push(UiRequest::ClosePanel(ClosePanelReq { id: panel_id }));
    }
    crate::host::with_state_or_init(|st| crate::state::set_last_panel_id(st, None));
    let scripts = crate::state::with_loaded_scripts(|s| s.clone());
    // ① 逐个属主走 unload(onUnload 在 reload 时也要跑;清理逻辑与 :plugin-unload 同一份)
    unload_all(st);
    // ② 没有属主的注册(裸脚本 / 单测里直接 eval)没人能认领 ⇒ 单独兜底回收
    unload_orphans(st);
    // ③ 加载期瞬态(编辑/装饰队列、加载栈…)
    reset_transient_state(st);
    // reload 语义 = 全部重新加载 ⇒ 把加载清单恢复(unload 会按属主摘掉条目,下面要按它重跑)
    crate::state::with_loaded_scripts(|s| *s = scripts.clone());
    // 装饰不随 reload 继承:全局 Clear(term 侧 doc: None = 清空所有 doc;泵循环下一帧应用)。
    // 必须放在 reset 之后(reset 清空队列);脚本重跑 eval 不 push 装饰,队列里只有这一个 Clear。
    crate::state::with_decoration_requests_current(|c| {
        c.push(DecorationRequest {
            doc: None,
            kind: DecorationKind::Clear,
        })
    });
    // 逐个重跑,单脚本失败不中止后续(否则一个插件挂→ init.js 断 → 其余全部失效)。
    // 错误汇总返回;成功脚本照常注册(命令/钩子/状态栏/键位都恢复)。
    let mut errors: Vec<String> = Vec::new();
    for (name, src) in &scripts {
        // 按名重读磁盘：js_load 记录的模块文件更新生效（相对名走多根解析，
        // 绝对路径直接用）；读不到（load_script_named 的字符串脚本/目录已删）用记录 src 兜底
        let disk_src = if Path::new(name).is_absolute() {
            std::fs::read_to_string(name).ok()
        } else {
            // 多根解析:后加的根覆盖先加的(用户覆盖内置)
            crate::state::resolve_plugin_path(name)
                .filter(|p| p.is_file())
                .and_then(|p| std::fs::read_to_string(p).ok())
        };
        let src = disk_src.as_deref().unwrap_or(src);
        // C-3:重跑也要有加载期属主(与首次加载同一条规则)
        let owner = crate::permissions::plugin_name_from_key(name).or_else(|| {
            std::path::Path::new(name)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
        });
        let prev_owner = owner
            .as_deref()
            .map(|o| crate::permissions::set_current_owner(Some(o)));
        let result = crate::state::with_engine(|engine| -> Result<()> {
            eval_wrapped(engine, src)
                .map(|_| ())
                .map_err(|e| anyhow!("plugin reload '{name}' failed: {e}"))?;
            Ok(())
        });
        if let Some(prev) = prev_owner {
            crate::permissions::restore_owner(prev);
        }
        if let Err(e) = result {
            errors.push(format!("{e}"));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(
            "{} plugin(s) failed: {}",
            errors.len(),
            errors.join("; ")
        ))
    }
}
