use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use boa_engine::object::builtins::{JsArray, JsFunction};
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::NativeFunction;
use boa_engine::{Context, JsError, JsString, JsValue, Source};

/// 事件名白名单：helix.on 只接受这些事件。
/// 通知型（save/buffer-*/theme-* 等）用 emit_event；终端钩子（term-*）用 emit_hook，
/// 其中 term-key/term-close 的返回值参与决策（见 emit_term_key / emit_hook）。
const EVENT_WHITELIST: [&str; 18] = [
    "save",
    "mode-change",
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
];

use crate::state::{
    with_buffer_icon_hook, with_engine, with_event_handlers, with_last_export, with_popups,
    with_registry, with_script_exports, with_statusline_hook, MESSAGES, UI_REQUESTS,
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
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            format!("invalid command name: {name:?}"),
        ))));
    }
    // 先校验/存入 doc 再注册：第三参类型非法（如 42）时整体失败，不产生半注册
    if let Some(doc_arg) = args.get(2) {
        if !doc_arg.is_null_or_undefined() {
            let doc: String = doc_arg.try_js_into(context)?;
            crate::state::with_command_docs(|d| d.insert(name.clone(), doc));
        }
    }
    with_registry(|r| r.insert(name, func));
    Ok(JsValue::undefined())
}

/// 设置插件目录（js_load 相对名解析用）。OnceLock 只生效一次：helix-term 启动时调用。
pub fn set_plugins_dir(dir: PathBuf) {
    // 单一目录 = 只有一个根(保留这个入口:测试与旧调用点都用它)
    crate::state::set_plugin_roots(vec![dir]);
}

/// helix.load(name)：从插件目录加载脚本（相对名或绝对路径），返回其 helix.export 的值；
/// 重复加载返回缓存对象。`.js` 后缀强制（无则补）。文件缺失/语法错 → 抛错。
/// 嵌套加载：内层 load 消费 LAST_EXPORT（take 语义），外层脚本自己的 export 随后设置。
/// 直接用传入的 boa Context 调（不再借 CONTEXT 线程局部）——load 可能发生在命令运行中
/// （lazy 桩），外层 run_command 正持有 CONTEXT 的 RefCell 借用，再借会 panic。
pub(crate) fn js_load(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "helix.load: name must be a string",
            )))
        })?;
    if name.is_empty() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.load: name must not be empty",
        ))));
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

/// 已加载的跳过（with_script_exports 缓存），循环依赖报错。
/// 加载栈为跨脚本共享的 thread_local（LOAD_STACK）：js_load 不再每次新建空栈，
/// 嵌套 helix.load（运行时/依赖递归）都能看到祖先——循环依赖报错而非栈溢出崩溃。
fn load_script_checked(ctx: &mut Context, name: &str) -> boa_engine::JsResult<JsValue> {
    // 加载参数 → 候选 key:裸名先找 `<name>/plugin.js`(插件布局约定),再回退 `<name>.js`。
    // 选中"第一个**能解析到文件**的候选",以此作为缓存/依赖/循环检测的 key(必须稳定)。
    let candidates = crate::state::entry_keys(name);
    let roots = crate::state::plugin_roots();
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
    if crate::state::with_load_stack(|s| s.iter().any(|k| k == &key)) {
        let chain = crate::state::with_load_stack(|s| {
            s.iter()
                .chain([&key])
                .cloned()
                .collect::<Vec<_>>()
                .join(" -> ")
        });
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            format!("helix.load: circular dependency: {chain}"),
        ))));
    }
    // eval 前入栈（嵌套 load 期间本 key 保持可见）；借用即刻释放，不跨 eval 持有。
    crate::state::with_load_stack(|s| s.push(key.clone()));
    // 与 key **同一处** push/pop,两栈因此始终等长(见 state::LOAD_ROOTS 注释)
    crate::state::push_load_root(providing_root);
    let result = (|| -> boa_engine::JsResult<JsValue> {
        let path = if Path::new(&key).is_absolute() {
            PathBuf::from(&key)
        } else {
            crate::state::resolve_plugin_path(&key).ok_or_else(|| {
                JsError::from_opaque(JsValue::from(JsString::from(
                    "helix.load: plugins dir not set",
                )))
            })?
        };
        let src = std::fs::read_to_string(&path).map_err(|e| {
            JsError::from_opaque(JsValue::from(JsString::from(format!(
                "helix.load('{key}'): {e}"
            ))))
        })?;
        // eval 前清空依赖记录，脚本内 helix.plugin 累积；eval 后 take 递归加载
        crate::state::with_last_plugin_deps(|d| d.clear());
        eval_wrapped(ctx, &src).map_err(|e| {
            JsError::from_opaque(JsValue::from(JsString::from(format!(
                "helix.load('{key}') failed: {e}"
            ))))
        })?;
        let deps = crate::state::with_last_plugin_deps(std::mem::take);
        // 先取外层 export（递归依赖 eval 会覆盖 LAST_EXPORT，外层 export 必须提前保存）
        let export = with_last_export(|l| l.take()).unwrap_or(JsValue::undefined());
        for dep in &deps {
            load_script_checked(ctx, dep)?;
        }
        crate::state::with_loaded_scripts(|s| {
            if !s.iter().any(|(n, _)| n == &key) {
                s.push((key.clone(), src));
            }
        });
        with_script_exports(|m| m.insert(key.clone(), export.clone()));
        Ok(export)
    })();
    crate::state::with_load_stack(|s| {
        s.pop();
    });
    crate::state::pop_load_root();
    result
}

/// helix.plugin(name, { deps, version })：声明当前脚本的依赖清单（方案 2）。
/// deps 是**加载参数数组**（插件名如 ["icons"]，或文件 key 如 ["lib/x.js"]），helix.load 自动拓扑加载。
pub(crate) fn js_plugin(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    // name 仅校验类型（API 形状：helix.plugin(name, { deps, version })）；
    // 依赖清单只取 deps（文件 key），version 为元数据保留位暂不消费。
    let _name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "helix.plugin: name must be a string",
            )))
        })?;
    let mut deps: Vec<String> = Vec::new();
    if let Some(meta) = args.get(1).unwrap_or(&JsValue::undefined()).as_object() {
        if let Ok(v) = meta
            .get(JsString::from("deps"), ctx)
            .and_then(|v| v.try_js_into::<Vec<String>>(ctx))
        {
            deps = v;
        }
    }
    crate::state::with_last_plugin_deps(|d| d.extend(deps));
    Ok(JsValue::undefined())
}

/// helix.plugin.install(arg)：镜像 :plugin install(path 或 git-url)。结果经状态栏显示。
pub(crate) fn js_plugin_install(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let arg: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "helix.plugin.install: arg (path or git url) required",
            )))
        })?;
    push_plugin_op("install", Some(arg));
    Ok(JsValue::undefined())
}

/// helix.plugin.update([name])：镜像 :plugin update(name 可选 = all)
pub(crate) fn js_plugin_update(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let arg: Option<String> = args
        .first()
        .filter(|v| !v.is_null_or_undefined())
        .map(|v| {
            v.try_js_into(context).map_err(|_| {
                JsError::from_opaque(JsValue::from(JsString::from(
                    "helix.plugin.update: arg must be a string",
                )))
            })
        })
        .transpose()?;
    push_plugin_op("update", arg);
    Ok(JsValue::undefined())
}

/// helix.plugin.remove(name)：镜像 :plugin remove
pub(crate) fn js_plugin_remove(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let arg: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "helix.plugin.remove: name required",
            )))
        })?;
    push_plugin_op("remove", Some(arg));
    Ok(JsValue::undefined())
}

fn push_plugin_op(op: &str, arg: Option<String>) {
    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::PluginOp {
            op: op.to_string(),
            arg,
        });
}

/// helix.server.<op>：镜像 :server(list/search/install/update/remove/status)。
/// 单向请求(与 PluginOp 同构)：结果经 editor 状态栏/错误显示,不回传 JS。
pub(crate) fn js_server_list(
    _this: &JsValue,
    args: &[JsValue],
    _context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
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
    let kw: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "helix.server.search: keyword required",
            )))
        })?;
    push_server_op("search", Some(kw));
    Ok(JsValue::undefined())
}

pub(crate) fn js_server_install(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "helix.server.install: name required",
            )))
        })?;
    push_server_op("install", Some(name));
    Ok(JsValue::undefined())
}

pub(crate) fn js_server_update(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    // 无参 = 更新全部(与 :server update 一致)
    let name: Option<String> = match args.first() {
        Some(v) if !v.is_undefined() && !v.is_null() => {
            Some(v.clone().try_js_into(context).map_err(|_| {
                JsError::from_opaque(JsValue::from(JsString::from(
                    "helix.server.update: name must be a string",
                )))
            })?)
        }
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
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "helix.server.remove: name required",
            )))
        })?;
    push_server_op("remove", Some(name));
    Ok(JsValue::undefined())
}

pub(crate) fn js_server_status(
    _this: &JsValue,
    args: &[JsValue],
    _context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
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
        .push(UiRequest::ServerOp {
            op: op.to_string(),
            arg,
        });
}

/// helix.export(obj)：声明当前脚本的导出（被 helix.load 的返回值拿到）。undefined/null 清空。
pub(crate) fn js_export(
    _this: &JsValue,
    args: &[JsValue],
    _ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let arg = args.first().cloned().unwrap_or(JsValue::undefined());
    with_last_export(|l| {
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
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.lazy: name and at least one command required",
        ))));
    }
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "helix.lazy: name must be a string",
            )))
        })?;
    if name.is_empty() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.lazy: name must not be empty",
        ))));
    }
    let factory = ctx
        .eval(Source::from_bytes(
            "(function(n, c) { return function(ctx) { helix.load(n); helix.run_command(c, ctx); }; })",
        ))
        .map_err(|e| JsError::from_opaque(JsValue::from(JsString::from(format!("helix.lazy: {e}")))))?;
    let factory = factory
        .as_callable()
        .and_then(JsFunction::from_object)
        .ok_or_else(|| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "helix.lazy: internal factory error",
            )))
        })?;
    let undefined = JsValue::undefined();
    for cmd in &args[1..] {
        let cmd: String = cmd.try_js_into(ctx).map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "helix.lazy: command names must be strings",
            )))
        })?;
        if cmd.is_empty() || cmd.chars().any(char::is_whitespace) {
            return Err(JsError::from_opaque(JsValue::from(JsString::from(
                format!("invalid command name: {cmd:?}"),
            ))));
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
            .map_err(|e| {
                JsError::from_opaque(JsValue::from(JsString::from(format!("helix.lazy: {e}"))))
            })?;
        with_registry(|r| r.insert(cmd, closure));
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
                JsError::from_opaque(JsValue::from(JsString::from(
                    "helix.run_command: 'selection' must be an object with anchor/head",
                )))
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
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "helix.run_command: name must be a string",
            )))
        })?;
    let command_ctx = parse_command_ctx(args.get(1).unwrap_or(&JsValue::undefined()), ctx)?;
    let func = with_registry(|r| r.get(&name).cloned()).ok_or_else(|| {
        JsError::from_opaque(JsValue::from(JsString::from(format!(
            "helix.run_command: '{name}' is not registered"
        ))))
    })?;
    let func = func
        .as_callable()
        .and_then(JsFunction::from_object)
        .ok_or_else(|| {
            JsError::from_opaque(JsValue::from(JsString::from(format!(
                "registered value for '{name}' is not a function"
            ))))
        })?;
    let arg = ctx_to_js(&command_ctx, ctx).map_err(|e| {
        JsError::from_opaque(JsValue::from(JsString::from(format!(
            "helix.run_command: {e}"
        ))))
    })?;
    let undefined = JsValue::undefined();
    func.call(&undefined, &[arg], ctx)
        .map(|_| JsValue::undefined())
        .map_err(|e| {
            JsError::from_opaque(JsValue::from(JsString::from(format!(
                "helix.run_command('{name}') failed: {e}"
            ))))
        })
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
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            format!("helix.map: unknown mode '{mode}'"),
        ))));
    }
    let key: String = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    if key.is_empty() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.map: key must not be empty",
        ))));
    }
    let command_arg = args.get(2).cloned().unwrap_or(JsValue::undefined());

    // 函数 → 注册为隐藏插件命令 __mapped_N
    let command: String = if command_arg.as_callable().is_some() {
        let id = crate::state::next_map_id();
        let name = format!("__mapped_{id}");
        with_registry(|r| r.insert(name.clone(), command_arg));
        name
    } else {
        command_arg.try_js_into(context)?
    };

    UI_REQUESTS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .push(UiRequest::MapKey { mode, key, command });
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
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "helix.by_path: path must be a string",
            )))
        })?;
    let canonical = helix_stdx::path::canonicalize(&path)
        .to_string_lossy()
        .into_owned();
    let hit = crate::state::with_doc_snapshots(|s| s.iter().find(|d| d.path == canonical).cloned());
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
pub fn run_command(name: &str, ctx: &CommandContext) -> Result<bool> {
    crate::init();
    // 命令开始时清空编辑队列，避免跨命令残留
    crate::state::with_edits(|c| c.clear());
    crate::state::with_decoration_requests(|c| c.clear());
    crate::state::with_cursor_requests(|c| c.clear());
    // 复位事务深度：上一命令未配对的 begin 在此丢弃（积压编辑随队列清空）
    crate::state::with_txn_depth(|d| *d = 0);
    // 携带其它已打开 buffer 快照（by_path 读取；本次命令生命周期）
    crate::state::set_doc_snapshots(ctx.docs.clone());
    let func = with_registry(|r| r.get(name).cloned());
    let Some(func) = func else { return Ok(false) };

    let func = func
        .as_callable()
        .and_then(JsFunction::from_object)
        .ok_or_else(|| anyhow!("registered value for '{name}' is not a function"))?;

    crate::state::with_engine(|engine| {
        let arg =
            ctx_to_js(ctx, engine).map_err(|e| anyhow!("failed to build command context: {e}"))?;
        let undefined = JsValue::undefined();
        func.call(&undefined, &[arg], engine)
            .map(|_| true)
            .map_err(|e| anyhow!("plugin command '{name}' failed: {e}"))
    })
}

/// 已注册的插件命令名（供命令行补全）
pub fn command_names() -> Vec<String> {
    crate::init();
    with_registry(|r| r.keys().cloned().collect())
}

/// 插件命令说明（未注册返回 None）
pub fn command_doc(name: &str) -> Option<String> {
    crate::init();
    crate::state::with_command_docs(|d| d.get(name).cloned())
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
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            format!("helix.on: unknown event '{name}'"),
        ))));
    }
    if handler.as_callable().is_none() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "helix.on: handler must be a function",
        ))));
    }
    with_event_handlers(|h| h.entry(name).or_default().push(handler));
    Ok(JsValue::undefined())
}

/// 是否有注册的事件处理器（挂点快速跳过）
pub fn has_handlers(name: &str) -> bool {
    crate::init();
    with_event_handlers(|h| h.get(name).is_some_and(|v| !v.is_empty()))
}

/// 触发事件：按注册顺序调用处理器；开始时清空编辑队列（防残留）。
/// extra 用于 mode-change 的 mode 字符串参数。
pub fn emit_event(name: &str, ctx: &CommandContext, extra: Option<&str>) -> Result<()> {
    emit_event_impl(name, ctx, extra, None)
}

/// doc-change 事件入口：参数附带防抖窗口内合并的变更范围（doc.changes）。
pub fn emit_doc_change(ctx: &CommandContext, changes: &[DocChange]) -> Result<()> {
    emit_event_impl("doc-change", ctx, None, Some(changes))
}

/// 事件触发公共实现；changes 为 Some 时序列化合并范围到 doc.changes。
fn emit_event_impl(
    name: &str,
    ctx: &CommandContext,
    extra: Option<&str>,
    changes: Option<&[DocChange]>,
) -> Result<()> {
    crate::init();
    crate::state::with_edits(|c| c.clear());
    crate::state::with_decoration_requests(|c| c.clear());
    crate::state::with_cursor_requests(|c| c.clear());
    // 复位事务深度：上一命令/事件未配对的 begin 在此丢弃（积压编辑随队列清空）
    crate::state::with_txn_depth(|d| *d = 0);
    // 携带其它已打开 buffer 快照（by_path 读取；本次事件生命周期）
    crate::state::set_doc_snapshots(ctx.docs.clone());
    let handlers = with_event_handlers(|h| h.get(name).cloned());
    let Some(handlers) = handlers else {
        return Ok(());
    };
    if handlers.is_empty() {
        return Ok(());
    }
    crate::state::with_engine(|engine| {
        let doc = doc_to_js(ctx, engine).map_err(|e| anyhow!("failed to build event doc: {e}"))?;
        if let Some(changes) = changes {
            let arr = build_changes_array(changes, ctx, engine)
                .map_err(|e| anyhow!("failed to build event changes: {e}"))?;
            doc.as_object()
                .ok_or_else(|| anyhow!("event doc is not an object"))?
                .set(JsString::from("changes"), arr, false, engine)
                .map_err(|e| anyhow!("failed to attach event changes: {e}"))?;
        }
        let undefined = JsValue::undefined();
        for handler in &handlers {
            let func = handler
                .as_callable()
                .and_then(JsFunction::from_object)
                .ok_or_else(|| anyhow!("event '{name}' handler not callable"))?;
            let args: Vec<JsValue> = match extra {
                Some(mode) => vec![JsValue::from(JsString::from(mode.to_string())), doc.clone()],
                None => vec![doc.clone()],
            };
            let _: JsValue = func.call(&undefined, &args, engine).map_err(|e| {
                crate::state::with_edits(|c| c.clear());
                crate::state::with_decoration_requests(|c| c.clear());
                crate::state::with_cursor_requests(|c| c.clear());
                crate::state::with_txn_depth(|d| *d = 0); // 与正常入口复位一致:错误路径也恢复事务深度
                anyhow!("event '{name}' handler failed: {e}")
            })?;
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
    let old_start = changes.iter().map(|c| c.0 .0).min().unwrap();
    let old_end = changes.iter().map(|c| c.0 .1).max().unwrap();
    let new_start = changes.iter().map(|c| c.1 .0).min().unwrap();
    let new_end = changes.iter().map(|c| c.1 .1).max().unwrap();
    let old_range = range_point(engine, &ctx.text, old_start, old_end)?;
    let new_range = range_point(engine, &ctx.text, new_start, new_end)?;
    let entry = JsValue::from(
        ObjectInitializer::new(engine)
            .property(JsString::from("oldRange"), old_range, Attribute::all())
            .property(JsString::from("newRange"), new_range, Attribute::all())
            .build(),
    );
    Ok(JsValue::from(JsArray::from_iter([entry], engine)))
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

/// { start: {row, col}, end: {row, col} } 范围对象。
fn range_point(
    engine: &mut Context,
    text: &str,
    start: usize,
    end: usize,
) -> boa_engine::JsResult<JsValue> {
    let start_pt = point_js(engine, text, start)?;
    let end_pt = point_js(engine, text, end)?;
    Ok(JsValue::from(
        ObjectInitializer::new(engine)
            .property(JsString::from("start"), start_pt, Attribute::all())
            .property(JsString::from("end"), end_pt, Attribute::all())
            .build(),
    ))
}

/// { row, col } 坐标对象。
fn point_js(engine: &mut Context, text: &str, pos: usize) -> boa_engine::JsResult<JsValue> {
    let (row, col) = pos_to_row_col(text, pos);
    Ok(JsValue::from(
        ObjectInitializer::new(engine)
            .property(
                JsString::from("row"),
                JsValue::from(row as f64),
                Attribute::all(),
            )
            .property(
                JsString::from("col"),
                JsValue::from(col as f64),
                Attribute::all(),
            )
            .build(),
    ))
}

/// 钩子调用：按注册顺序调用 name 的 handler，返回第一个非 undefined 返回值。
/// 通知型钩子（term-open/mode-change/exit/resize/title）忽略返回值；
/// term-close 返回 false 表示阻止关闭。参数由调用方构造（原生 JsValue 无需 engine）。
pub fn emit_hook(name: &str, args: &[JsValue]) -> Option<JsValue> {
    crate::init();
    let handlers = with_event_handlers(|h| h.get(name).cloned())?;
    if handlers.is_empty() {
        return None;
    }
    with_engine(|engine| {
        let undefined = JsValue::undefined();
        for handler in &handlers {
            let Some(func) = handler.as_callable().and_then(JsFunction::from_object) else {
                continue;
            };
            if let Ok(ret) = func.call(&undefined, args, engine) {
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
    pty_id: u64,
    code: &str,
    shift: bool,
    ctrl: bool,
    alt: bool,
) -> Option<TermKeyDecision> {
    crate::init();
    let handlers = with_event_handlers(|h| h.get("term-key").cloned())?;
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
        let args = [JsValue::from(pty_id), JsValue::from(key)];
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
    emit_hook("pane-mode-change", &[JsValue::from(JsString::from(mode))]);
}

/// 通知型终端钩子封装（helix-term 侧不依赖 boa，统一走这里）：
/// term-open / term-mode-change / term-exit / term-resize / term-title 通知；
/// term-close 返回 true=放行关闭 / false=阻止。
pub fn emit_term_open(pty_id: u64, cmd: &str) {
    emit_hook(
        "term-open",
        &[JsValue::from(pty_id), JsValue::from(JsString::from(cmd))],
    );
}
pub fn emit_term_mode(pty_id: u64, mode: &str) {
    emit_hook(
        "term-mode-change",
        &[JsValue::from(pty_id), JsValue::from(JsString::from(mode))],
    );
}
pub fn emit_term_exit(pty_id: u64, code: i32) {
    emit_hook("term-exit", &[JsValue::from(pty_id), JsValue::from(code)]);
}
pub fn emit_term_resize(pty_id: u64, rows: u16, cols: u16) {
    emit_hook(
        "term-resize",
        &[
            JsValue::from(pty_id),
            JsValue::from(rows),
            JsValue::from(cols),
        ],
    );
}
pub fn emit_term_title(pty_id: u64, title: &str) {
    emit_hook(
        "term-title",
        &[JsValue::from(pty_id), JsValue::from(JsString::from(title))],
    );
}
/// 组件事件(鼠标点击/滚动等):构造 {kind, x, y} 对象;插件返回 true → 消费该事件。
pub fn emit_component_event(id: u64, kind: &str, x: u16, y: u16) -> bool {
    crate::init();
    let handlers = with_event_handlers(|h| h.get("component-event").cloned());
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
        let args = [JsValue::from(id), JsValue::from(ev)];
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
    emit_hook(
        "term-close",
        &[JsValue::from(pty_id), JsValue::from(JsString::from(reason))],
    )
    .is_some_and(|v| v.as_boolean() == Some(false))
}

/// term_state(ptyId, key[, value])：终端任意状态持久化（跨会话存盘）。
/// 读：值不存在返回 undefined；写：value 必须是字符串（覆盖）。
/// 存储：~/.local/state/helix/term-state.json（{"<ptyId>": {"<key>": "<value>"}}）。
pub(crate) fn js_term_state(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let pty_id: u64 = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "term_state: ptyId must be a number",
            )))
        })?;
    let key: String = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "term_state: key must be a string",
            )))
        })?;
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
    let v: String = value.try_js_into(context).map_err(|_| {
        JsError::from_opaque(JsValue::from(JsString::from(
            "term_state: value must be a string",
        )))
    })?;
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
    crate::state::with_cursor_requests(|c| c.push(CursorRequest::SetCursor { row, col }));
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
                return Err(JsError::from_opaque(JsValue::from(JsString::from(
                    "set_selection: empty selection array",
                ))));
            }
            let mut selections = Vec::with_capacity(len as usize);
            for i in 0..len {
                let elem = arr.get(i, context)?;
                let el = elem.as_object().ok_or_else(|| {
                    JsError::from_opaque(JsValue::from(JsString::from(
                        "set_selection: each array element must be { anchor, head }",
                    )))
                })?;
                let anchor = parse_sel_pos(&el.get(JsString::from("anchor"), context)?, context)?;
                let head = parse_sel_pos(&el.get(JsString::from("head"), context)?, context)?;
                selections.push((anchor, head));
            }
            crate::state::with_cursor_requests(|c| {
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
    crate::state::with_cursor_requests(|c| {
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
    let obj = v.as_object().ok_or_else(|| {
        JsError::from_opaque(JsValue::from(JsString::from(
            "set_selection: anchor/head must be { row, col }",
        )))
    })?;
    let row = obj.get(JsString::from("row"), ctx)?;
    let col = obj.get(JsString::from("col"), ctx)?;
    if row.is_null_or_undefined() || col.is_null_or_undefined() {
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            "set_selection: anchor/head missing row/col",
        ))));
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
    let cmd: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(context)?;
    let output = Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .output()
        .map_err(|e| {
            JsError::from_opaque(JsValue::from(JsString::from(format!(
                "helix.run: failed to spawn: {e}"
            ))))
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr: String = stderr.chars().take(RUN_OUTPUT_LIMIT).collect();
        let code = output
            .status
            .code()
            .map_or_else(|| "signal".to_string(), |c| c.to_string());
        return Err(JsError::from_opaque(JsValue::from(JsString::from(
            format!("helix.run: command failed ({code}): {stderr}"),
        ))));
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
    MESSAGES
        .get()
        .expect("MESSAGES initialized")
        .lock()
        .expect("messages lock")
        .push(text);
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
    crate::state::with_edits(|c| {
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
    crate::state::with_edits(|c| {
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
    crate::state::with_edits(|c| {
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
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "helix.set_virtual_text: path must be a string",
            )))
        })?;
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
    crate::state::with_decoration_requests(|c| {
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
        .map_err(|_| {
            JsError::from_opaque(JsValue::from(JsString::from(
                "helix.set_highlight: path must be a string",
            )))
        })?;
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
    crate::state::with_decoration_requests(|c| {
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
    if crate::state::with_txn_depth(|d| *d > 0) {
        // 事务打开:积压不消费。未配对的 begin(不 end)在下一命令/事件入口
        // 随深度一起清队列,积压编辑丢弃(不崩),见规格 §4
        return Vec::new();
    }
    crate::state::with_edits(std::mem::take)
}

/// 取走并清空装饰请求队列（helix-term 在命令返回后消费）；事务开启时积压不消费。
/// 与 take_edits 同构：未配对的 begin 在下一命令/事件入口随队列清空丢弃。
pub fn take_decorations() -> Vec<DecorationRequest> {
    crate::init();
    if crate::state::with_txn_depth(|d| *d > 0) {
        return Vec::new();
    }
    crate::state::with_decoration_requests(std::mem::take)
}

/// 声明批量编辑事务开始:期间编辑积压,end 后合并取走(一次撤销)
pub(crate) fn js_begin_edit(
    _this: &JsValue,
    _args: &[JsValue],
    _ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    crate::state::with_txn_depth(|d| *d += 1);
    Ok(JsValue::undefined())
}

/// 声明批量编辑事务结束:积压编辑可被 take_edits 取走
pub(crate) fn js_end_edit(
    _this: &JsValue,
    _args: &[JsValue],
    _ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    crate::state::with_txn_depth(|d| *d = d.saturating_sub(1));
    Ok(JsValue::undefined())
}

/// 触发节点事件（焦点路由）：onPress → 无参调用；onKey → 传键名
pub fn dispatch_node_event(view_id: u64, node_id: &str, key: Option<&str>) -> Result<()> {
    crate::init();
    crate::state::with_engine(|engine| {
        let handlers = crate::state::with_node_handlers(|h| {
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
        let undefined = JsValue::undefined();
        match key {
            None => {
                // button 按下（Enter/Space）
                if let Some(f) = handlers.on_press {
                    let func = f
                        .as_callable()
                        .and_then(JsFunction::from_object)
                        .ok_or_else(|| anyhow!("node {node_id} onPress not callable"))?;
                    let _: JsValue = func
                        .call(&undefined, &[], engine)
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
                    let _: JsValue = func
                        .call(
                            &undefined,
                            &[JsValue::from(JsString::from(k.to_string()))],
                            engine,
                        )
                        .map_err(|e| anyhow!("node {node_id} onKey failed: {e}"))?;
                }
            }
        }
        Ok(())
    })
}

/// 收集组件树中的可聚焦节点 id（button/input，树序）
pub fn focusable_node_ids(node: &CompNode, out: &mut Vec<String>) {
    match node {
        CompNode::Button { id, .. } => out.push(id.clone()),
        CompNode::Input { id, .. } => out.push(id.clone()),
        CompNode::Row { children, .. }
        | CompNode::Col { children, .. }
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
    crate::state::with_cursor_requests(std::mem::take)
}

/// 求值一段插件脚本（无名字，用于测试/内联）；脚本里可调用 `helix.register_command` / `helix.echo`
pub fn load_script(src: &str) -> Result<()> {
    load_script_named("<anon>", src)
}

/// IIFE 包裹求值：每次求值（含 reload 重跑）都得到全新词法作用域，
/// 顶层 const/let 不再与上一次求值冲突；顺带隔离脚本间全局污染。
/// `helix` 是全局属性，IIFE 内可正常访问；错误消息定位脚本名。
fn eval_wrapped(engine: &mut boa_engine::Context, src: &str) -> boa_engine::JsResult<JsValue> {
    let wrapped = format!("(function() {{\n{src}\n}})()");
    engine.eval(Source::from_bytes(&wrapped))
}

/// 求值并记录脚本（名字用于报错定位与热重载）
pub fn load_script_named(name: &str, src: &str) -> Result<()> {
    crate::init();
    // 脚本(重)加载不继承旧装饰:全局 Clear(term 侧 doc: None = 清空所有 doc)
    crate::state::with_decoration_requests(|c| {
        c.push(DecorationRequest {
            doc: None,
            kind: DecorationKind::Clear,
        })
    });
    crate::state::with_engine(|engine| {
        eval_wrapped(engine, src)
            .map(|_| ())
            .map_err(|e| anyhow!("plugin script '{name}' error: {e}"))?;
        crate::state::with_loaded_scripts(|s| s.push((name.to_string(), src.to_string())));
        Ok(())
    })
}

/// 已加载脚本名（按加载顺序；供 :plugin list / status）
pub fn loaded_scripts() -> Vec<String> {
    crate::init();
    crate::state::with_loaded_scripts(|s| s.iter().map(|(name, _)| name.clone()).collect())
}

/// 清空所有插件状态（热重载用；id 计数器保留保证唯一性）。
/// 注意：UI_REQUESTS/MESSAGES 不清——它们是命令边界 drain 的队列。
fn reset_plugin_state() {
    with_registry(|r| r.clear());
    with_event_handlers(|h| h.clear());
    with_popups(|p| p.clear());
    crate::state::with_edits(|c| c.clear());
    crate::state::with_decoration_requests(|c| c.clear());
    crate::state::with_cursor_requests(|c| c.clear());
    crate::state::with_txn_depth(|d| *d = 0);
    with_buffer_icon_hook(|h| *h = None);
    with_statusline_hook(|h| *h = None);
    crate::state::set_last_panel_id(None);
    crate::state::with_command_docs(|d| d.clear());
    // 导出缓存与 pending 导出随插件状态重置（reload 后按名重读磁盘重跑）
    with_script_exports(|m| m.clear());
    // 加载栈防御性清空（正常路径已 pop；防止异常残留影响后续 reload）
    crate::state::with_load_stack(|s| s.clear());
    crate::state::clear_load_roots();
    with_last_export(|l| *l = None);
    // 主题覆盖随插件状态重置：清空并置脏（下次 drain 还原基准主题）
    crate::state::with_theme_overrides(|o| o.clear());
    crate::state::mark_theme_dirty();
}

/// 热重载：清空状态后按加载顺序重跑全部脚本。
/// 脚本以 IIFE 包裹求值，每次重跑都是全新词法作用域——顶层 const/let/var 都不冲突，
/// reload 必然成功（脚本不依赖其他脚本的状态，reload 会重新注册全部命令/处理器），
/// 因此 Err 分支仅作防御（如脚本依赖被清空的全局状态），单测只覆盖成功路径。
pub fn reload_all() -> Result<()> {
    crate::init();
    // 面板层还挂在 compositor 上——入队 ClosePanel 让 plugin_reload 的 apply_ui_requests 移除它们，
    // 否则 reset_plugin_state 清掉 JS 注册表后这些层变成无法关闭的僵尸层（渲染报错 + 按键穿透）。
    // 只关 last_panel_id 会漏掉多面板场景，遍历全部。
    let open_panels = crate::state::with_open_panels(std::mem::take);
    for panel_id in open_panels {
        UI_REQUESTS
            .get()
            .unwrap()
            .lock()
            .unwrap()
            .push(UiRequest::ClosePanel { id: panel_id });
    }
    crate::state::set_last_panel_id(None);
    let scripts = crate::state::with_loaded_scripts(|s| s.clone());
    reset_plugin_state();
    // 装饰不随 reload 继承:全局 Clear(term 侧 doc: None = 清空所有 doc;泵循环下一帧应用)。
    // 必须放在 reset 之后(reset 清空队列);脚本重跑 eval 不 push 装饰,队列里只有这一个 Clear。
    crate::state::with_decoration_requests(|c| {
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
        let result = crate::state::with_engine(|engine| -> Result<()> {
            eval_wrapped(engine, src)
                .map(|_| ())
                .map_err(|e| anyhow!("plugin reload '{name}' failed: {e}"))?;
            Ok(())
        });
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
