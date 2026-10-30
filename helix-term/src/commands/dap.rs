use super::{Context, Editor};
use crate::{
    compositor::{self, Compositor},
    job::{Callback, Jobs},
    ui::{self, overlay::overlaid, Picker, Prompt, PromptEvent},
};
use dap::{StackFrame, Thread, ThreadStates};
use helix_core::syntax::config::{DebugConfigCompletion, DebugTemplate};
use helix_dap::{self as dap, requests::TerminateArguments};
use helix_view::editor::Breakpoint;

use serde_json::{json, to_value, Value};

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;

use anyhow::{anyhow, bail};

use crate::dap_config;
use helix_dap::registry::DebugAdapterId;
use helix_view::editor::DapIntent;
use helix_view::handlers::dap::{check_breakpoint_caps, jump_to_stack_frame};

fn thread_picker(
    cx: &mut Context,
    callback_fn: impl Fn(&mut Editor, &dap::Thread) + Send + 'static,
) {
    let debugger = debugger!(cx.editor);

    let future = debugger.threads();
    dap_callback(
        cx.jobs,
        future,
        move |editor, compositor, response: dap::requests::ThreadsResponse| {
            let threads = response.threads;
            if threads.len() == 1 {
                callback_fn(editor, &threads[0]);
                return;
            }
            let debugger = debugger!(editor);

            let thread_states = debugger.thread_states.clone();
            let columns = [
                ui::PickerColumn::new("name", |item: &Thread, _| item.name.as_str().into()),
                ui::PickerColumn::new("state", |item: &Thread, thread_states: &ThreadStates| {
                    thread_states
                        .get(&item.id)
                        .map(|state| state.as_str())
                        .unwrap_or("unknown")
                        .into()
                }),
            ];
            let picker = Picker::new(
                columns,
                0,
                threads,
                thread_states,
                move |cx, thread, _action| callback_fn(cx.editor, thread),
            )
            .with_preview(move |editor, _js, thread| {
                let frames = editor
                    .debug_adapters
                    .get_active_client()
                    .as_ref()?
                    .stack_frames
                    .get(&thread.id)?;
                let frame = frames.first()?;
                let path = frame.source.as_ref()?.path.as_ref()?.as_path();
                let pos = Some((
                    frame.line.saturating_sub(1),
                    frame.end_line.unwrap_or(frame.line).saturating_sub(1),
                ));
                Some((path.into(), pos))
            });
            compositor.push(Box::new(picker));
        },
    );
}

/// 当前行的断点：`(path, line, 断点)`。
///
/// 以前返回的是"该文件断点 Vec 里的下标" —— 那个下标在 `BreakpointStore` 里没有意义
/// （表是 `path → line → bp`，行的顺序由 BTreeMap 保证）。用 `(path, line)` 定位更稳：
/// adapter 回填时可能把行号挪走，下标立刻失效。
fn get_breakpoint_at_current_line(editor: &mut Editor) -> Option<(PathBuf, usize, Breakpoint)> {
    let (view, doc) = current!(editor);
    let text = doc.text().slice(..);

    let line = doc.selection(view.id).primary().cursor_line(text);
    let path = doc.path()?;
    let bp = editor.breakpoints.get(path, line)?.clone();
    Some((path.to_path_buf(), line, bp))
}

/// 把某个文件的断点推给 adapter（**不阻塞 UI**）。
///
/// 没有活动会话时**什么都不做**：断点已经进表，launch 之后 adapter 的 `initialized` 事件会
/// 全量重放（见 `helix-view/src/handlers/dap.rs` 的 `Event::Initialized`）。
/// 断点表**本地**变了（切花/删除/条件/日志/编辑重映射）⇒ 通知插件刷新。
///
/// 为什么单独发一个事件:adapter 的 `dap-breakpoint` 只在 `setBreakpoints` **回包**时发，
/// 而"还没启动会话就打断点"根本没有回包 ⇒ dapui 的行首 `●/◯` 会一直是旧的。
fn emit_breakpoints_changed(editor: &mut Editor) {
    // **先刷快照再发事件**：`helix.dap.list_breakpoints()` 读的是缓存快照，
    // 而快照平时只在"有 DAP 请求完成"时才刷新 —— 没会话时打断点就没人刷，
    // 插件拿到的还是旧的空表（现象：按 F9 打了断点，行首标记/断点页都不动）。
    refresh_dap_snapshots(editor, None);
    editor.emit_dap_event("dap-breakpoints", serde_json::json!({}));
}

fn push_breakpoints(editor: &mut Editor, path: PathBuf) {
    emit_breakpoints_changed(editor);
    editor.dap_intents.push(DapIntent::SyncBreakpoints(path));
}

/// 冲刷 [`Editor::dap_intents`]：把每个意图变成"异步请求 + 主线程回填"的 job。
///
/// **事件循环专用**（`application.rs::pump_term_events`）—— 那是唯一同时握着 `&mut Editor`
/// 与 `&mut Jobs` 的地方。命令与 Picker 回调只负责 `push`，一次都不等 adapter。
pub fn dispatch_dap_intents(editor: &mut Editor, jobs: &mut Jobs) {
    // 先回放"编辑导致的断点行号漂移"（钩子里没有 `&mut Editor`，只能排到这一拍）。
    // 有漂移的文件要重推一次 setBreakpoints；没有会话时 SyncBreakpoints 自己会跳过。
    for path in helix_view::breakpoint_remap::apply_pending(&mut editor.breakpoints) {
        editor.dap_intents.push(DapIntent::SyncBreakpoints(path));
    }

    // 构建完成 → 起会话（`dap_build_run` 只负责排队，这里才有 jobs）
    for pending in take_pending_launches() {
        let started = start_session_with(
            editor,
            jobs,
            pending.adapter,
            pending.request,
            pending.args,
            pending.socket,
            pending.build,
        );
        if let Err(err) = started {
            editor.set_error(format!("启动失败: {err}"));
        }
    }

    for intent in std::mem::take(&mut editor.dap_intents) {
        dispatch_one(editor, jobs, intent);
    }
}

fn dispatch_one(editor: &mut Editor, jobs: &mut Jobs, intent: DapIntent) {
    match intent {
        DapIntent::StartRequest { request, args } => {
            let Some(debugger) = editor.debug_adapters.get_active_client() else {
                editor.set_error("调试会话还没插入注册表，launch 已跳过");
                return;
            };
            let handle = debugger.handle();
            let is_launch = request == "launch";
            tokio::spawn(async move {
                let res = if is_launch {
                    handle.call::<dap::requests::Launch>(args).await.map(|_| ())
                } else {
                    handle.call::<dap::requests::Attach>(args).await.map(|_| ())
                };
                match res {
                    Ok(()) => log::info!("dap: {request} 已确认"),
                    Err(err) => log::error!("dap: {request} 失败: {err}"),
                }
            });
        }
        DapIntent::SyncBreakpoints(path) => {
            let Some(debugger) = editor.debug_adapters.get_active_client() else {
                // 没有会话：断点已在表里，launch 后由 `initialized` 事件全量重放
                return;
            };
            if let Err(err) =
                check_breakpoint_caps(debugger.caps.as_ref(), &editor.breakpoints.for_path(&path))
            {
                editor.set_error(format!("Failed to set breakpoints: {}", err));
                return;
            }
            let handle = debugger.handle();
            let source_breakpoints = editor.breakpoints.source_breakpoints(&path);
            let job = Box::pin(async move {
                let response = handle
                    .set_breakpoints(path.clone(), source_breakpoints)
                    .await;
                let call: Callback =
                    Callback::EditorCompositor(Box::new(move |editor, _compositor, mut _js| {
                        match response {
                            Ok(Some(breakpoints)) => {
                                // 回填 verified/id/message（异步结果 ⇒ 必须回主线程改状态）
                                editor.breakpoints.apply_response(&path, &breakpoints);
                            }
                            Ok(None) => {}
                            Err(err) => {
                                editor.set_error(format!("Failed to set breakpoints: {}", err))
                            }
                        }
                    }));
                Ok(call)
            });
            jobs.callback(job);
        }
        DapIntent::Request { kind, params } => {
            // 复用 `helix.dap.*` 的派发路径（同一个 kind → 同一份实现）。
            // `id: 0` 是"没有 Promise"的哨兵：`resolve(0, …)` 是 no-op。
            let request = helix_js::dap::DapRequest {
                id: 0,
                kind,
                params,
            };
            dispatch_dap_request(editor, jobs, request);
        }
        DapIntent::SwitchThread(thread_id) => {
            let Some(handle) = editor
                .debug_adapters
                .get_active_client()
                .map(|debugger| debugger.handle())
            else {
                editor.set_error("没有活动调试会话（先 :debug-start）");
                return;
            };
            let job = Box::pin(async move {
                let frames = handle.stack_trace(thread_id).await.map(|(f, _)| f);
                let call: Callback =
                    Callback::EditorCompositor(Box::new(move |editor, _compositor, mut _js| {
                        match frames {
                            Ok(frames) => {
                                if let Some(debugger) =
                                    editor.debug_adapters.get_active_client_mut()
                                {
                                    debugger.thread_id = Some(thread_id);
                                    debugger.stack_frames.insert(thread_id, frames.clone());
                                    debugger.active_frame = Some(0);
                                }
                                if let Some(frame) = frames.first() {
                                    jump_to_stack_frame(editor, frame);
                                }
                            }
                            Err(err) => {
                                editor.set_error(format!("Failed to get stack frames: {}", err))
                            }
                        }
                    }));
                Ok(call)
            });
            jobs.callback(job);
        }
        DapIntent::Variables { frame_id } => {
            let Some(handle) = editor
                .debug_adapters
                .get_active_client()
                .map(|debugger| debugger.handle())
            else {
                editor.set_error("没有活动调试会话（先 :debug-start）");
                return;
            };
            let job = Box::pin(async move {
                let scopes = handle.scopes(frame_id).await?;
                // 每个作用域 + 它的**直接**成员，摊平成一张表（真正的树在 dapui 面板里，
                // 这里只是"不开面板也能快速看一眼"）。表头用真列 ⇒ 名字/值/类型各自对齐。
                let mut rows: Vec<VarRow> = Vec::new();
                for scope in &scopes {
                    // 寄存器动不动几百项、对"看变量"没帮助 ⇒ 这个快速视图里跳过
                    // （需要看就去 dapui 面板里展开 `Registers`）
                    if scope.name.eq_ignore_ascii_case("registers") {
                        continue;
                    }
                    let vars = handle
                        .variables(scope.variables_reference)
                        .await
                        .map_err(|e| {
                            helix_dap::Error::Other(anyhow::anyhow!("{}: {e}", scope.name))
                        })?;
                    if vars.is_empty() {
                        continue; // 空作用域不占行（Globals/Registers 常常空）
                    }
                    for var in vars {
                        rows.push(VarRow {
                            scope: scope.name.clone(),
                            name: var.name,
                            ty: var.ty.unwrap_or_default(),
                            value: var.value,
                            label: String::new(),
                        });
                    }
                }
                let call: Callback = Callback::EditorCompositor(Box::new(
                    move |editor: &mut Editor, compositor: &mut Compositor, mut _js| {
                        if rows.is_empty() {
                            editor.set_status("当前帧没有可显示的变量");
                            return;
                        }
                        // **自己算列宽**拼一行（不用多列）：picker 的多列在窄终端会把
                        // 表头折成 `…`（列宽 = 数据宽度），反而更乱。
                        // 名字（非 Locals 时带作用域前缀）/ 值 / 类型 三段等宽对齐。
                        let name_of = |r: &VarRow| -> String {
                            if r.scope.eq_ignore_ascii_case("locals") {
                                r.name.clone()
                            } else {
                                format!("{}.{}", r.scope, r.name)
                            }
                        };
                        // 先按**全表最长**算出两列宽度，再把每行排版好塞进 `label`
                        // （`PickerColumn` 的格式化函数是 fn 指针，拿不到外部变量）
                        let mut w_name = 4usize;
                        let mut w_value = 5usize;
                        for r in &rows {
                            w_name = w_name.max(name_of(r).chars().count());
                            w_value = w_value.max(r.value.chars().count().min(40));
                        }
                        let w_name = w_name.min(28);
                        let w_value = w_value.min(40);
                        for r in &mut rows {
                            let ty = if r.ty.is_empty() {
                                "-".to_string()
                            } else {
                                r.ty.clone()
                            };
                            r.label = format!(
                                "{}  {}  {}",
                                pad_display(&name_of(r), w_name),
                                pad_display(&r.value, w_value),
                                ty
                            );
                        }
                        let columns = [ui::PickerColumn::new("变量", |r: &VarRow, _| {
                            r.label.as_str().into()
                        })];
                        let picker = Picker::new(columns, 0, rows, (), |cx, row, _action| {
                            // Enter = 复制值（与面板里 `y` 同款行为：`"` + 系统剪贴板），
                            // 列表留在原地好继续看其它行。Esc 不会走到这里（不触发回调）。
                            let val = row.value.clone();
                            let _ = cx.editor.registers.write('"', vec![val.clone()]);
                            let _ = cx.editor.registers.write('+', vec![val.clone()]);
                            cx.editor
                                .set_status(format!("已复制 {} = {}", row.name, val));
                        });
                        // `overlaid`：做成**浮层**（与配置 picker 同一种观感），
                        // 而不是塞满整个编辑器区域的面板。
                        compositor.replace_or_push("dap-variables", overlaid(picker));
                    },
                ));
                Ok(call)
            });
            jobs.callback(job);
        }
    }
}

/// 按**显示宽度**截断并补齐到 `w` 列（CJK 算 2 列；超出用 `…`）。
fn pad_display(s: &str, w: usize) -> String {
    use helix_core::unicode::width::UnicodeWidthStr;
    let mut out = String::new();
    let mut used = 0usize;
    let mut truncated = false;
    for ch in s.chars() {
        let cw = ch.to_string().width();
        if used + cw > w {
            truncated = true;
            break;
        }
        out.push(ch);
        used += cw;
    }
    if truncated {
        out.push('…');
        used += 1;
    }
    out.push_str(&" ".repeat(w.saturating_sub(used)));
    out
}

/// `v` 弹层（`:debug-variables`）里的一行：作用域 + 变量。
struct VarRow {
    scope: String,
    name: String,
    ty: String,
    value: String,
    /// 排版好的整行（名字 / 值 / 类型，列宽在拿到全部数据后算）
    label: String,
}

// -- DAP

fn dap_callback<T, F>(
    jobs: &mut Jobs,
    call: impl Future<Output = helix_dap::Result<serde_json::Value>> + 'static + Send,
    callback: F,
) where
    T: for<'de> serde::Deserialize<'de> + Send + 'static,
    F: FnOnce(&mut Editor, &mut Compositor, T) + Send + 'static,
{
    let callback = Box::pin(async move {
        let json = call.await?;
        let response = serde_json::from_value(json)?;
        let call: Callback = Callback::EditorCompositor(Box::new(
            move |editor: &mut Editor, compositor: &mut Compositor, mut _js| {
                callback(editor, compositor, response)
            },
        ));
        Ok(call)
    });

    jobs.callback(callback);
}

/// 起会话的**唯一**实现：给定 adapter 配置 + 请求 + 参数，后台 spawn + initialize + 请求，
/// 回主线程插入会话并回填 caps。
///
/// 三个入口共用它（`:debug-start` 模板、`launch.toml`/`launch.json` 配置、`helix.dap.launch`）。
/// 起会话（只依赖 `Editor + Jobs`，所以命令层/`commands::Context` 都能调）。
fn start_session_with(
    editor: &mut Editor,
    jobs: &mut Jobs,
    debugger_cfg: helix_core::syntax::config::DebugAdapterConfig,
    request: String,
    args: Value,
    socket: Option<std::net::SocketAddr>,
    build: Option<String>,
) -> Result<(), anyhow::Error> {
    // Refuse to spawn a debug adapter in workspace trust restricted mode.
    let workspace = doc!(editor).workspace_root().to_path_buf();
    if !editor
        .workspace_trust
        .query(&workspace, helix_loader::workspace_trust::TrustQuery::Dap)
        .is_trusted()
    {
        bail!("Workspace is not trusted. Run `:workspace-trust` to enable the debug adapter.");
    }
    // 不支持的请求要在**起进程之前**拒掉（别留一个孤儿 adapter）
    match request.as_str() {
        "launch" | "attach" => {}
        other => bail!("Unsupported request '{}'", other),
    }

    let id = editor.debug_adapters.reserve();
    let adapter_name = debugger_cfg.name.clone();
    let request_for_record = request.clone();
    let args_for_record = args.clone();
    let build_for_record = build.clone();
    let callback = Box::pin(async move {
        let (client, receiver) =
            helix_dap::registry::Registry::spawn_raw(id, socket, &debugger_cfg)
                .await
                .map_err(|e| anyhow!("Failed to start debug client: {}", e))?;
        let handle = client.handle();
        let caps = handle
            .request::<dap::requests::Initialize>(helix_dap::initialize_arguments(&adapter_name))
            .await
            .map_err(|e| anyhow!("Failed to initialize debug adapter: {}", e))?;
        // launch/attach 不在这里 await（见 `DapIntent::StartRequest`）：debugpy 之类
        // 要等 `configurationDone` 才回包，而 configurationDone 由 `initialized` 事件驱动 ——
        // 事件只有在 receiver 进了 registry 之后才泵得动。

        Ok(Callback::EditorCompositor(Box::new(
            move |editor: &mut Editor, _compositor: &mut Compositor, mut _js| {
                match editor
                    .debug_adapters
                    .insert_client(id, client, receiver, &debugger_cfg, true)
                {
                    Ok(_) => {
                        if let Some(client) = editor.debug_adapters.get_client_mut(id) {
                            client.caps = Some(caps);
                            // 记住启动参数：`r`（重启）要用它重放
                            client.remember_start(&request_for_record, &args_for_record);
                            // `r` 重启时用它先重建（否则改了代码跑的还是旧产物）
                            client.remember_build(build_for_record.clone());
                        }
                        // 与 API 路径一致：起好了就发 `dap-session`（插件据此更新会话列表/状态栏）
                        let public = editor.debug_adapters.public_id(id).unwrap_or(0);
                        editor.emit_dap_event(
                            "dap-session",
                            json!({
                                "id": public,
                                "kind": "created",
                                "name": adapter_name,
                                "state": "running",
                            }),
                        );
                        editor.dap_intents.push(DapIntent::StartRequest {
                            request: request_for_record.clone(),
                            args: args_for_record.clone(),
                        });
                    }
                    Err(err) => editor.set_error(format!("Failed to attach debugger: {err}")),
                }
            },
        )))
    });
    jobs.callback(callback);
    // 关于"要不要等 initialized 再放行命令"（老 TODO）：
    // **不缓冲**。① 未就绪时的每一条命令都已经有明确报错（`调试会话还没就绪（adapter 启动中）`），
    // 不会静默失败；② 缓冲的代价是"几秒前按的 next 突然执行"，比报错更难理解；
    // ③ 真正的初始化动作（断点重放 + `configurationDone`）已经在
    // `helix-view/src/handlers/dap.rs` 的 `Event::Initialized` 分支里做完了。
    Ok(())
}

/// `:debug-start [模板名] [参数…]`：走 `languages.toml` 的模板（保留原有的参数提示流程）。
pub fn dap_start_impl(
    cx: &mut compositor::Context,
    name: Option<&str>,
    socket: Option<std::net::SocketAddr>,
    params: Option<Vec<std::borrow::Cow<str>>>,
    release: bool,
) -> Result<(), anyhow::Error> {
    // 方案 A：`name` 命中三级配置（launch.toml / launch.json / 语言模板）
    let (configs, warnings) = load_launch_configs(cx.editor);
    if !warnings.is_empty() {
        cx.editor.set_error(warnings.join("; "));
    }
    if let Some(named) = name
        .and_then(|n| configs.iter().find(|c| c.name == n))
        .cloned()
    {
        if named.source != dap_config::Source::Lang {
            return start_with_launch_config(cx, named, socket);
        }
    }

    // 方案 B：语言模板（保留参数提示/占位替换）
    let doc = doc!(cx.editor);
    let config = doc
        .language_config()
        .and_then(|config| config.debugger.as_ref())
        .ok_or_else(|| anyhow!("No debug adapter available for language"))?;
    let config = config.clone();

    // 配置不是每帧读了：`load_launch_configs` 走的是宿主快照（8208d0225「配置快照不再每帧读盘」）。
    // 这里只剩"按名字从已加载的 templates 里挑一条"，O(模板数)。
    let template = match name {
        Some(name) => config.templates.iter().find(|t| t.name == name).cloned(),
        None => config.templates.first().cloned(),
    }
    .ok_or_else(|| anyhow!("No debug config with given name"))?;

    // `:debug-start binary`（不带参数）= 模板占位没填 ⇒ 先看能不能从项目检测里推出来。
    // 以前这里会把 `{0}` 原样当成 program（产物不存在 → 起不来），现在：
    //   · 单字段模板 + 检测到产物 ⇒ 直接用；
    //   · 其它情况 ⇒ 弹参数 Prompt（第一格预填检测到的路径）。
    let detected = doc_ctx(cx.editor).map(|(workspace, _)| {
        let file = current_doc_file(cx.editor);
        crate::dap_build::detect(file.as_deref(), &workspace, release)
    });
    let detected_program = detected
        .as_ref()
        .and_then(|d| d.program.as_ref())
        .map(|p| p.to_string_lossy().into_owned());
    let params: Option<Vec<std::borrow::Cow<'static, str>>> = match params {
        Some(p) if !p.is_empty() => Some(
            p.iter()
                .map(|x| std::borrow::Cow::Owned(x.to_string()))
                .collect(),
        ),
        _ if template.completion.is_empty() => None,
        _ => {
            let single_filename = template.completion.len() == 1
                && matches!(
                    template.completion.first(),
                    Some(DebugConfigCompletion::Advanced(cfg))
                        if matches!(cfg.completion.as_deref(), Some("filename" | "directory"))
                );
            if single_filename {
                match detected_program.clone() {
                    Some(program) => Some(vec![std::borrow::Cow::Owned(program)]),
                    None => {
                        // 推不出来 ⇒ 问用户（不要拿 `{0}` 硬起）
                        let completions = template.completion.clone();
                        let name = template.name.clone();
                        let callback = Box::pin(async move {
                            let call: Callback = Callback::EditorCompositor(Box::new(
                                move |_editor, compositor, mut _js| {
                                    let prompt =
                                        debug_parameter_prompt(completions, name, Vec::new());
                                    compositor.push(Box::new(prompt));
                                },
                            ));
                            Ok(call)
                        });
                        cx.jobs.callback(callback);
                        return Ok(());
                    }
                }
            } else {
                // 多字段模板（如 gdbserver attach）：弹 Prompt，第一格预填检测结果
                let completions = template.completion.clone();
                let name = template.name.clone();
                let prefill = detected_program.clone();
                let callback = Box::pin(async move {
                    let call: Callback =
                        Callback::EditorCompositor(Box::new(move |editor, compositor, mut _js| {
                            let mut prompt = debug_parameter_prompt(completions, name, Vec::new());
                            if let Some(p) = prefill {
                                prompt.insert_str(&p, editor);
                            }
                            compositor.push(Box::new(prompt));
                        }));
                    Ok(call)
                });
                cx.jobs.callback(callback);
                return Ok(());
            }
        }
    };

    let mut args: HashMap<&str, Value> = if let Some(params) = params.as_ref() {
        let preprocessed = prepare_dap_params(&template, params);
        template
            .args
            .iter()
            .map(|(k, v)| (k.as_str(), map_value(v, &preprocessed)))
            .collect()
    } else {
        template
            .args
            .iter()
            .map(|(k, v)| (k.as_str(), v.clone()))
            .collect()
    };
    args.insert("cwd", to_value(helix_stdx::env::current_working_dir())?);
    let args = to_value(args).unwrap();

    // 模板路径同样支持"产物不存在就先构建"（检测结果兜底；`launch.toml` 的 build 在上游已处理）
    let build = detected.and_then(|d| d.build);
    launch_or_build(
        cx.editor,
        cx.jobs,
        config,
        template.request.clone(),
        args,
        socket,
        build,
        BuildPolicy::IfMissing,
    )
}

/// 用一条 `launch.toml` / `launch.json` 配置起会话（参数已做过变量替换）。
fn start_with_launch_config(
    cx: &mut compositor::Context,
    cfg: dap_config::LaunchConfig,
    socket: Option<std::net::SocketAddr>,
) -> Result<(), anyhow::Error> {
    let Some(debugger_cfg) = debugger_for(cx.editor, cfg.adapter.as_deref()) else {
        bail!(
            "配置 {:?} 找不到可用的调试适配器（type={:?}）；先给该语言配 [language.debugger]",
            cfg.name,
            cfg.adapter
        );
    };
    // 显式配置（`.helix/launch.toml` / `.vscode/launch.json`）里写了 `build` ⇒ **每次都跑**，
    // 等价 VS Code 的 preLaunchTask：改了源码再按 `l`，不会拿旧二进制去单步
    // （旧 DWARF 行号表会让断点/单步落在旧行号上，看起来像"跳错文件"）。
    // cargo 增量时这一步通常 <0.3s，代价远小于"调试一个过期产物"。
    launch_or_build(
        cx.editor,
        cx.jobs,
        debugger_cfg,
        cfg.request,
        cfg.args,
        socket,
        cfg.build.clone(),
        BuildPolicy::Always,
    )
}

fn prepare_dap_params(template: &DebugTemplate, params: &[std::borrow::Cow<str>]) -> Vec<String> {
    params
        .iter()
        .enumerate()
        .map(|(i, x)| {
            let mut param = x.to_string();
            if let Some(DebugConfigCompletion::Advanced(cfg)) = template.completion.get(i) {
                if matches!(cfg.completion.as_deref(), Some("filename" | "directory")) {
                    param = std::fs::canonicalize(x.as_ref())
                        .ok()
                        .and_then(|pb| pb.into_os_string().into_string().ok())
                        .unwrap_or_else(|| x.to_string());
                }
            }
            param
        })
        .collect()
}

fn map_value(value: &Value, params: &[String]) -> Value {
    match value {
        Value::String(string) => {
            let mut string = string.clone();
            for (i, x) in params.iter().enumerate() {
                let pattern = format!("{{{}}}", i);
                string = string.replace(&pattern, x);
            }
            if let Ok(integer) = string.parse::<usize>() {
                to_value(integer).unwrap()
            } else {
                to_value(string).unwrap()
            }
        }
        Value::Array(array) => Value::Array(array.iter().map(|x| map_value(x, params)).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(k, v)| (k.clone(), map_value(v, params)))
                .collect(),
        ),

        _ => value.clone(),
    }
}

/// 一条 Picker 项：统一展示三级配置（`launch.toml` / `launch.json` / 语言模板）。
#[derive(Clone)]
struct LaunchEntry {
    /// `runInTerminal: true` 且没配 `[terminal]` ⇒ 选它必然失败，先在列表里标出来
    needs_terminal: bool,
    cfg: dap_config::LaunchConfig,
}

pub fn dap_launch(cx: &mut Context) {
    // 多会话：不再拒绝第二个调试器（新会话会成为 `active`；用 `:debug-sessions` 切换）。
    let (configs, warnings) = load_launch_configs(cx.editor);
    if configs.is_empty() {
        let msg = if warnings.is_empty() {
            "没有可用的调试配置：给这个语言配 [language.debugger]，或写 .helix/launch.toml"
                .to_string()
        } else {
            warnings.join("; ")
        };
        cx.editor.set_error(msg);
        return;
    }
    // 告警只在这里（用户主动开 Picker）显示一次
    if !warnings.is_empty() {
        cx.editor.set_error(warnings.join("; "));
    }

    let no_external_terminal = cx.editor.config().terminal.is_none();
    let entries: Vec<LaunchEntry> = configs
        .into_iter()
        .map(|cfg| {
            // `runInTerminal: true` 的模板（如 lldb-dap 的 `binary (terminal)`）要靠
            // `[terminal]` 外部终端；没配就标出来，别让人按了才看到 "No external terminal defined"。
            let needs_terminal = cfg
                .args
                .get("runInTerminal")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
                && no_external_terminal;
            LaunchEntry {
                cfg,
                needs_terminal,
            }
        })
        .collect();

    // 用**真列**：宽度由 picker 自己算并逐行对齐。
    // 早先手写 `{:<12} {:<7} …` 拼成一行 ⇒ 终端字体/中文字符宽度一变就"没对齐"。
    let columns = [
        ui::PickerColumn::new("配置", |item: &LaunchEntry, _| {
            item.cfg.name.as_str().into()
        }),
        ui::PickerColumn::new("请求", |item: &LaunchEntry, _| {
            item.cfg.request.as_str().into()
        }),
        ui::PickerColumn::new("来源", |item: &LaunchEntry, _| {
            item.cfg.source.as_str().into()
        }),
        // 语言 + 需要外部终端的告警合在一列（列太多时 picker 会把中间列压成 `…`）
        ui::PickerColumn::new("语言", |item: &LaunchEntry, _| {
            let lang = item.cfg.language.clone().unwrap_or_else(|| "-".into());
            if item.needs_terminal {
                format!("{lang} ⚠终端").into()
            } else {
                lang.into()
            }
        }),
    ];

    cx.push_layer(Box::new(overlaid(Picker::new(
        columns,
        0,
        entries,
        (),
        |cx, entry, _action| {
            let cfg = entry.cfg.clone();
            // 语言模板且带参数提示：保留原来的 Prompt 流程（`{0}` 占位要填）
            if cfg.source == dap_config::Source::Lang {
                let template = doc!(cx.editor)
                    .language_config()
                    .and_then(|c| c.debugger.as_ref())
                    .and_then(|d| d.templates.iter().find(|t| t.name == cfg.name).cloned());
                if let Some(template) = template {
                    if !template.completion.is_empty() {
                        // 单字段且是 filename 类 ⇒ 用检测结果直接启动（不再弹空白 Prompt）
                        let first_is_filename = template
                            .completion
                            .first()
                            .map(|c| {
                                matches!(
                                    c,
                                    DebugConfigCompletion::Advanced(cfg)
                                        if matches!(
                                            cfg.completion.as_deref(),
                                            Some("filename" | "directory")
                                        )
                                )
                            })
                            .unwrap_or(false);
                        if template.completion.len() == 1 && first_is_filename {
                            if let Some((workspace, _lang)) = doc_ctx(cx.editor) {
                                let file = current_doc_file(cx.editor);
                                let detected =
                                    crate::dap_build::detect(file.as_deref(), &workspace, false);
                                if let Some(program) = detected.program {
                                    let args = vec![std::borrow::Cow::Owned(
                                        program.to_string_lossy().into_owned(),
                                    )];
                                    if let Err(err) = dap_start_impl(
                                        cx,
                                        Some(&template.name),
                                        None,
                                        Some(args),
                                        false,
                                    ) {
                                        cx.editor.set_error(err.to_string());
                                    }
                                    return;
                                }
                            }
                        }
                        let completions = template.completion.clone();
                        let name = template.name.clone();
                        let callback = Box::pin(async move {
                            let call: Callback = Callback::EditorCompositor(Box::new(
                                move |_editor, compositor, mut _js| {
                                    let prompt =
                                        debug_parameter_prompt(completions, name, Vec::new());
                                    compositor.push(Box::new(prompt));
                                },
                            ));
                            Ok(call)
                        });
                        cx.jobs.callback(callback);
                        return;
                    }
                }
            }
            if let Err(err) = start_or_build(cx, cfg, None) {
                cx.editor.set_error(err.to_string());
            }
        },
    ))))
}

// ============================ 构建（`:debug-build` / 构建并启动） ============================

/// 构建完成之后要起的会话（构建是异步的，参数先排好队）。
///
/// `build` 一起排队：构建完起会话时得把它交给 `start_session_with` 记住，
/// 否则构建完按 `r`（重启）就拿不到构建命令，又变回“跑旧二进制”。
pub struct PendingLaunch {
    pub adapter: helix_core::syntax::config::DebugAdapterConfig,
    pub request: String,
    pub args: Value,
    pub socket: Option<std::net::SocketAddr>,
    pub build: Option<String>,
}

thread_local! {
    static PENDING_LAUNCH: std::cell::RefCell<Vec<PendingLaunch>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// 排空“构建完就该起的会话”。由 `dispatch_dap_intents`（事件循环，握有 jobs）调用。
pub fn take_pending_launches() -> Vec<PendingLaunch> {
    PENDING_LAUNCH.with(|q| std::mem::take(&mut *q.borrow_mut()))
}

/// 构建命令**首个可执行名**（`meson setup build && …` → `meson`）。
fn build_tool(cmd: &str) -> String {
    cmd.split_whitespace()
        .next()
        .unwrap_or(cmd)
        .to_string()
}

/// 起不来（spawn 失败）时的文案：模块缺失/权限这类要让用户知道**缺哪个程序**。
fn toolchain_hint(build: &str, err: &str) -> String {
    let tool = build_tool(build);
    format!("构建命令无法执行：需要工具链 `{tool}`（不在 PATH？）—— {err}")
}

/// 非 0 退出：127 视为"命令找不到"（= 缺工具链），其余照旧报 exit code。
fn build_failure_text(build: &str, status: std::process::ExitStatus) -> String {
    match status.code() {
        Some(127) => format!(
            "构建失败：找不到命令 `{}`（不在 PATH？装好工具链后重试）",
            build_tool(build)
        ),
        Some(code) => format!("构建失败（exit {code}）"),
        None => "构建失败（被信号终止）".to_string(),
    }
}

fn shell_command(cmd: &str) -> (String, Vec<String>) {
    if cfg!(windows) {
        ("cmd".to_string(), vec!["/C".to_string(), cmd.to_string()])
    } else {
        ("sh".to_string(), vec!["-c".to_string(), cmd.to_string()])
    }
}

/// 首次自动构建时把检测结果落成 `.helix/launch.toml`；已存在就返回 `None`（不动用户配置）。
///
/// 只在"知道要启动哪个产物"时写：没有 `program` 的配置写了也起不来，反而把人坑到一份
/// 错配置上。写失败不算致命（构建照跑），只报一条错。
fn write_launch_toml_once(
    editor: &mut Editor,
    workspace: &std::path::Path,
    language: Option<&str>,
    program: &Option<String>,
    detected: &crate::dap_build::Detected,
    build: &str,
) -> Option<PathBuf> {
    let program: PathBuf = program
        .as_deref()
        .map(PathBuf::from)
        .or_else(|| detected.program.clone())?;
    let name = program
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("debug");
    let doc = crate::dap_build::launch_toml_document(name, language, &program, workspace, build);
    match crate::dap_build::write_launch_toml_if_absent(workspace, &doc) {
        Ok(path) => path,
        Err(err) => {
            editor.set_error(format!("写 .helix/launch.toml 失败: {err}"));
            None
        }
    }
}

/// 跑一次构建（**不阻塞 UI**：整段丢给 job，回主线程后再把输出与结果一次性落地）。
///
/// 输出走 `dap-output` 事件（`category: "build"`）⇒ dapui 的 Output tab 直接显示，
/// 不需要新事件、也不需要新面板。
pub fn dap_build_run(
    editor: &mut Editor,
    jobs: &mut Jobs,
    release: bool,
    then: Option<PendingLaunch>,
    explicit_build: Option<String>,
) {
    let Some((workspace, lang)) = doc_ctx(editor) else {
        editor.set_error("构建：当前没有工作区");
        return;
    };
    let file = current_doc_file(editor);
    let detected = crate::dap_build::detect(file.as_deref(), &workspace, release);
    let build = explicit_build.or_else(|| detected.build.clone());
    let Some(build) = build else {
        editor.set_error(format!(
            "没认出构建方式（{}）；在 .helix/launch.toml 里写 build = \"…\"",
            detected.reason
        ));
        return;
    };
    let program = then.as_ref().and_then(|p| {
        p.args
            .get("program")
            .and_then(|v| v.as_str())
            .map(str::to_string)
    });
    // 首次自动构建 ⇒ 把检测结果落成 `.helix/launch.toml`（已存在就一个字都不动）。
    // 有了这份显式配置，下次按 `l` 走的是「显式 `build` ⇒ 每次启动前构建」那条路，
    // 不会再拿旧二进制去单步（旧 DWARF 行号表 = 断点/单步落在旧行号上）。
    let generated = write_launch_toml_once(
        editor,
        &workspace,
        lang.as_deref(),
        &program,
        &detected,
        &build,
    );
    editor.set_status(match &generated {
        Some(path) => format!(
            "已生成 {}（build 将随每次启动执行）；构建中: {build}",
            path.display()
        ),
        None => format!("构建中: {build}"),
    });
    let (shell, argv) = shell_command(&build);
    let ws = workspace.clone();
    let reason = detected.reason.clone();
    let job = Box::pin(async move {
        // **流式**：stdout/stderr 各起一个 reader 任务逐行 push 到全局队列，
        // 主线程在 `pump_term_events` 里 drain 成 `dap-output` 事件 ⇒ 构建期间
        // Output 面板就有内容（早先 `output()` 一次性收集 ⇒ 空白像卡死）。
        let spawned = tokio::process::Command::new(&shell)
            .args(&argv)
            .current_dir(&ws)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn();
        let out = match spawned {
            Err(err) => Err(err),
            Ok(mut child) => {
                let mut readers = Vec::new();
                if let Some(stdout) = child.stdout.take() {
                    readers.push(tokio::spawn(stream_lines(stdout, "build")));
                }
                if let Some(stderr) = child.stderr.take() {
                    readers.push(tokio::spawn(stream_lines(stderr, "stderr")));
                }
                let status = child.wait().await;
                for r in readers {
                    let _ = r.await;
                }
                status
            }
        };
        let call: Callback = Callback::EditorCompositor(Box::new(
            move |editor, _c, _js| match out {
                Err(err) => editor.set_error(toolchain_hint(&build, &err.to_string())),
                Ok(status) => {
                    if status.success() {
                        // 构建成功了，但"要启动什么"未必知道：没有 program 时给一条可执行的提示，
                        // 而不是「产物不存在: 」（空名字，看不出该做什么）。
                        let Some(program) = program.as_deref().filter(|p| !p.is_empty()) else {
                            editor.set_error(
                                "构建完成，但没检测到要启动的产物；用 `:debug-start <模板> <可执行文件>` \
                                 指定，或在 .helix/launch.toml 里写 program",
                            );
                            return;
                        };
                        if !std::path::Path::new(program).exists() {
                            editor.set_error(format!(
                                "构建完成但产物不存在: {program}（检测到: {reason}）"
                            ));
                            return;
                        }
                        editor.set_status("构建完成");
                        if let Some(pending) = then {
                            PENDING_LAUNCH.with(|q| q.borrow_mut().push(pending));
                            editor.set_status("构建完成，正在启动…");
                        }
                    } else {
                        editor.set_error(format!(
                            "{}；先看 Output（:dapui-output）",
                            build_failure_text(&build, status)
                        ));
                    }
                }
            },
        ));
        Ok(call)
    });
    jobs.callback(job);
}

/// 起会话前要不要先构建。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BuildPolicy {
    /// 显式配置里写了 `build`（`.helix/launch.toml` / `.vscode/launch.json`）⇒ 每次都跑。
    /// 语义就是 preLaunchTask：用户写了构建命令，就是要它每次都跑。
    Always,
    /// 检测出来的 `build` ⇒ 只在产物缺失时跑。
    /// 为什么不能 `Always`：模板/检测的 `build` 是猜的（`make all`、`cmake --build …` 可能很慢），
    /// 不能因为按一下 `l` 就无谓地跑一遍。
    IfMissing,
}

impl BuildPolicy {
    /// 这次要不要先构建（`attach` 没有产物、没 `build` 都不构建）。
    fn wants_build(self, request: &str, program: Option<&str>, build: Option<&str>) -> bool {
        if request == "attach" || build.is_none() {
            return false;
        }
        match self {
            BuildPolicy::Always => true,
            BuildPolicy::IfMissing => program
                .map(|p| !std::path::Path::new(p).exists())
                .unwrap_or(false),
        }
    }
}

/// 起会话的统一出口：按 `policy` 决定"先构建再启动"还是直接起。
#[allow(clippy::too_many_arguments)]
fn launch_or_build(
    editor: &mut Editor,
    jobs: &mut Jobs,
    adapter: helix_core::syntax::config::DebugAdapterConfig,
    request: String,
    args: Value,
    socket: Option<std::net::SocketAddr>,
    build: Option<String>,
    policy: BuildPolicy,
) -> Result<(), anyhow::Error> {
    let program = args
        .get("program")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    if policy.wants_build(&request, program.as_deref(), build.as_deref()) {
        dap_build_run(
            editor,
            jobs,
            false,
            Some(PendingLaunch {
                adapter,
                request,
                args,
                socket,
                build: build.clone(),
            }),
            build,
        );
        return Ok(());
    }
    start_session_with(editor, jobs, adapter, request, args, socket, build)
}

/// 启动一条配置；缺 `program` 时用检测结果补齐，产物不存在且有构建命令时**先构建再启动**。
///
/// 这就是"不用手填二进制"的主路径（设计 §3.3）。
pub fn start_or_build(
    cx: &mut compositor::Context,
    mut cfg: dap_config::LaunchConfig,
    socket: Option<std::net::SocketAddr>,
) -> Result<(), anyhow::Error> {
    let Some((workspace, _lang)) = doc_ctx(cx.editor) else {
        bail!("没有工作区");
    };
    let file = current_doc_file(cx.editor);
    let detected = crate::dap_build::detect(file.as_deref(), &workspace, false);

    // ① 缺 program ⇒ 检测结果兜底（只对 launch；attach 不需要 program）
    if cfg.request != "attach" {
        let has_program = cfg
            .args
            .get("program")
            .and_then(|v| v.as_str())
            .is_some_and(|p| !p.is_empty());
        if !has_program {
            if let Some(program) = &detected.program {
                match cfg.args.as_object_mut() {
                    Some(obj) => {
                        obj.insert("program".into(), json!(program.to_string_lossy()));
                    }
                    None => cfg.args = json!({ "program": program.to_string_lossy() }),
                }
            }
        }
    }

    let build = cfg.build.clone().or_else(|| detected.build.clone());
    // ② 统一出口：产物缺失 + 有 build ⇒ 构建并启动
    let Some(debugger_cfg) = debugger_for(cx.editor, cfg.adapter.as_deref()) else {
        bail!(
            "配置 {:?} 找不到可用的调试适配器（type={:?}）；先给该语言配 [language.debugger]",
            cfg.name,
            cfg.adapter
        );
    };
    launch_or_build(
        cx.editor,
        cx.jobs,
        debugger_cfg,
        cfg.request,
        cfg.args,
        socket,
        build,
        BuildPolicy::IfMissing,
    )
}

/// 重启调试会话。
///
/// 两条路（设计见 `docs/superpowers/specs/2026-10-26-dap-autobuild-and-restart-design.md` §4）：
///
/// 1. **adapter 支持 DAP `restart`** ⇒ 直接发请求（最快，进程不换、断点确认不丢）；
/// 2. **不支持**（lldb-dap 就是 `supportsRestartRequest: false`）⇒ **客户端重启**：
///    用本编辑器记住的 `(adapter 配置, launch/attach, 参数, socket)` 开一个新会话，
///    旧会话随后 disconnect（`terminate_debuggee` 视能力）。多会话内核允许两者短暂并存，
///    断点表由新会话的 `initialized` 自动重放。
pub fn dap_restart(cx: &mut Context) {
    let Some(debugger) = cx.editor.debug_adapters.get_active_client() else {
        cx.editor.set_error("Debugger is not running");
        return;
    };
    let args = debugger.starting_request_args().cloned();
    let socket = debugger.socket;
    let adapter = debugger.config.clone();
    let kind = debugger.connection_type();
    let dap_restart_ok = debugger
        .caps
        .as_ref()
        .and_then(|c| c.supports_restart_request)
        .unwrap_or(false);

    // ① adapter 自己会重启
    if dap_restart_ok && args.is_some() {
        let request = debugger.restart();
        dap_callback(cx.jobs, request, |editor, _compositor, _resp: Value| {
            editor.set_status("Debugging session restarted")
        });
        return;
    }

    // ①' 记住过构建命令 ⇒ **先停旧会话，再"构建 + 启动"**。
    // 否则改了代码按 `r` 跑的还是旧二进制（`cargo build` 的产物没更新）。
    if let Some(build) = debugger.starting_build() {
        let old = cx
            .editor
            .debug_adapters
            .get_active_client()
            .map(|c| c.handle());
        if let Some(handle) = old {
            let job = Box::pin(async move {
                let _ = handle
                    .call::<dap::requests::Disconnect>(Some(dap::requests::DisconnectArguments {
                        restart: Some(false),
                        terminate_debuggee: Some(true),
                        suspend_debuggee: None,
                    }))
                    .await;
                let call: Callback = Callback::EditorCompositor(Box::new(
                    |_editor: &mut Editor, _compositor: &mut Compositor, _js| {},
                ));
                Ok(call)
            });
            cx.jobs.callback(job);
        }
        if let (Some(a), Some(ad), Some(kind)) = (args.clone(), adapter.clone(), kind) {
            let request = match kind {
                dap::ConnectionType::Launch => "launch",
                dap::ConnectionType::Attach => "attach",
            };
            cx.editor.set_status(format!("重启：先构建（{build}）…"));
            dap_build_run(
                cx.editor,
                cx.jobs,
                false,
                Some(PendingLaunch {
                    adapter: ad,
                    request: request.to_string(),
                    args: a,
                    socket,
                    build: Some(build.clone()),
                }),
                Some(build),
            );
            return;
        }
    }

    // ② 客户端重启
    let (Some(args), Some(adapter), Some(kind)) = (args, adapter, kind) else {
        cx.editor.set_error(
            "无法重启：这条会话不是本编辑器起的（或没记住启动参数），用 :debug-start 重开",
        );
        return;
    };
    let request = match kind {
        dap::ConnectionType::Launch => "launch",
        dap::ConnectionType::Attach => "attach",
    };
    // 旧会话先留 handle：新会话起来之后再收尸，避免 terminate 的 terminated 事件抢在
    // 新会话的 stopped 之前把 dapui 的装饰清掉（插件侧也已按会话 id 过滤）。
    let old = cx
        .editor
        .debug_adapters
        .get_active_client()
        .map(|c| c.handle());
    match start_session_with(
        cx.editor,
        cx.jobs,
        adapter,
        request.to_string(),
        args,
        socket,
        None, // 重启路径不带构建（要重建走上面 ①' 的分支）
    ) {
        Ok(()) => {
            cx.editor
                .set_status("重启中：新会话已启动（旧会话正在关闭）");
            if let Some(handle) = old {
                let job = Box::pin(async move {
                    let _ = handle
                        .call::<dap::requests::Disconnect>(Some(
                            dap::requests::DisconnectArguments {
                                restart: Some(false),
                                terminate_debuggee: Some(true),
                                suspend_debuggee: None,
                            },
                        ))
                        .await;
                    let call: Callback = Callback::EditorCompositor(Box::new(
                        |_editor: &mut Editor, _compositor: &mut Compositor, _js| {},
                    ));
                    Ok(call)
                });
                cx.jobs.callback(job);
            }
        }
        Err(err) => cx.editor.set_error(format!("重启失败: {err}")),
    }
}

fn debug_parameter_prompt(
    completions: Vec<DebugConfigCompletion>,
    config_name: String,
    mut params: Vec<String>,
) -> Prompt {
    let completion = completions.get(params.len()).unwrap();
    let field_type = if let DebugConfigCompletion::Advanced(cfg) = completion {
        cfg.completion.as_deref().unwrap_or("")
    } else {
        ""
    };
    let name = match completion {
        DebugConfigCompletion::Advanced(cfg) => cfg.name.as_deref().unwrap_or(field_type),
        DebugConfigCompletion::Named(name) => name.as_str(),
    };
    let default_val = match completion {
        DebugConfigCompletion::Advanced(cfg) => cfg.default.as_deref().unwrap_or(""),
        _ => "",
    }
    .to_owned();

    let completer = match field_type {
        "filename" => |editor: &Editor, input: &str| {
            ui::completers::filename_with_git_ignore(editor, input, false)
        },
        "directory" => |editor: &Editor, input: &str| {
            ui::completers::directory_with_git_ignore(editor, input, false)
        },
        _ => ui::completers::none,
    };

    Prompt::new(
        format!("{}: ", name).into(),
        None,
        completer,
        move |cx, input: &str, event: PromptEvent| {
            if event != PromptEvent::Validate {
                return;
            }

            let mut value = input.to_owned();
            if value.is_empty() {
                value = default_val.clone();
            }
            params.push(value);

            if params.len() < completions.len() {
                let completions = completions.clone();
                let config_name = config_name.clone();
                let params = params.clone();
                let callback = Box::pin(async move {
                    let call: Callback = Callback::EditorCompositor(Box::new(
                        move |_editor, compositor, mut _js| {
                            let prompt = debug_parameter_prompt(completions, config_name, params);
                            compositor.push(Box::new(prompt));
                        },
                    ));
                    Ok(call)
                });
                cx.jobs.callback(callback);
            } else if let Err(err) = dap_start_impl(
                cx,
                Some(&config_name),
                None,
                Some(params.iter().map(|x| x.into()).collect()),
                false,
            ) {
                cx.editor.set_error(err.to_string());
            }
        },
    )
}

/// 当前 buffer 的 `(绝对路径, 光标行)`。断点相关命令（键位 + `:`）共用。
fn current_path_line(editor: &mut Editor) -> Option<(PathBuf, usize)> {
    let (view, doc) = current!(editor);
    let path = doc.path()?.to_owned();
    let text = doc.text().slice(..);
    let line = doc.selection(view.id).primary().cursor_line(text);
    Some((path, line))
}

/// 光标下的标识符（用于 `K` 求值）。取不到就返回 None。
fn word_under_cursor(editor: &mut Editor) -> Option<String> {
    let (view, doc) = current!(editor);
    let text = doc.text().slice(..);
    let pos = doc.selection(view.id).primary().cursor(text);
    let is_word =
        |c: char| c.is_alphanumeric() || c == '_' || c == '.' || c == ':' || c == '>' || c == '-';

    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let start_at = pos.min(chars.len() - 1);
    // 往回找起点
    let mut start = start_at;
    while start > 0 && is_word(chars[start - 1]) {
        start -= 1;
    }
    // 往后找终点
    let mut end = start_at;
    while end + 1 < chars.len() && is_word(chars[end + 1]) {
        end += 1;
    }
    let word: String = chars[start..=end].iter().collect();
    let word = word
        .trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
        .to_string();
    if word.is_empty() {
        None
    } else {
        Some(word)
    }
}

/// `R`（调试态）/ `:debug-run-to-cursor`：跑到光标所在行。
///
/// 实现 = 打一个**临时断点** + continue；命中后由 `Event::Stopped` 把它摘掉
/// （`editor.debug_temp_breakpoint`）。这样不需要 DAP 的 "run to" 扩展，任何 adapter 都能用。
pub fn dap_run_to_cursor(cx: &mut Context) {
    let Some((path, line)) = current_path_line(cx.editor) else {
        cx.editor.set_error("跑到光标：当前 buffer 没有路径");
        return;
    };
    if cx.editor.debug_adapters.get_active_client().is_none() {
        cx.editor
            .set_error("跑到光标：没有活动调试会话（先 :debug-start）");
        return;
    }
    let mut bp = Breakpoint::new(line);
    bp.condition = None;
    cx.editor.breakpoints.set(path.clone(), bp);
    cx.editor.debug_temp_breakpoint = Some((path.clone(), line));
    cx.editor
        .set_status(format!("跑到光标：{}:{}", path.display(), line + 1));
    push_breakpoints(cx.editor, path);
    dap_continue(cx);
}

/// `K`（调试态）/ `:debug-eval-under-cursor`：对光标下的标识符求值，结果显示在状态栏。
pub fn dap_eval_under_cursor(cx: &mut Context) {
    let Some(expr) = word_under_cursor(cx.editor) else {
        cx.editor.set_error("光标下没有可求值的标识符");
        return;
    };
    let debugger = match cx.editor.debug_adapters.get_active_client() {
        Some(d) => d,
        None => {
            cx.editor
                .set_error("求值：没有活动调试会话（先 :debug-start）");
            return;
        }
    };
    if debugger.caps.is_none() {
        cx.editor
            .set_error("求值：调试会话还没就绪（adapter 启动中）");
        return;
    }
    let handle = debugger.handle();
    // 带上**当前栈帧 id**：不带的话 lldb-dap 会拒绝（`context: hover` + 无帧 ⇒ success:false）
    let frame_id = debugger.current_stack_frame().map(|f| f.id);
    let args = dap::requests::EvaluateArguments {
        expression: expr.clone(),
        frame_id,
        context: Some("hover".to_string()),
        format: None,
    };
    let request = handle.call::<dap::requests::Evaluate>(args);
    // 不走 `dap_callback`：那个把失败吞进日志（表达式写错时"按了没反应"）。
    // 这里成功/失败都给状态栏一条消息。
    cx.jobs.callback(async move {
        let call: anyhow::Result<Callback> = match request.await {
            Ok(json) => {
                let resp: dap::requests::EvaluateResponse = serde_json::from_value(json)?;
                let ty = resp.ty.unwrap_or_default();
                let msg = if ty.is_empty() {
                    format!("{expr} = {}", resp.result)
                } else {
                    format!("{expr} = {} :{ty}", resp.result)
                };
                Ok(Callback::EditorCompositor(Box::new(
                    move |editor: &mut Editor, _c: &mut Compositor, _js| editor.set_status(msg),
                )))
            }
            Err(err) => Ok(Callback::EditorCompositor(Box::new(
                move |editor: &mut Editor, _c: &mut Compositor, _js| {
                    editor.set_error(format!("求值 {expr} 失败: {err:#}"))
                },
            ))),
        };
        call
    });
}

pub fn dap_toggle_breakpoint(cx: &mut Context) {
    let Some((path, line)) = current_path_line(cx.editor) else {
        cx.editor
            .set_error("Can't set breakpoint: document has no path");
        return;
    };
    dap_toggle_breakpoint_impl(cx, path, line);
}

pub fn dap_toggle_breakpoint_impl(cx: &mut Context, path: PathBuf, line: usize) {
    // TODO: need to map breakpoints over edits and update them?
    // we shouldn't really allow editing while debug is running though

    cx.editor.breakpoints.toggle(path.clone(), line);
    push_breakpoints(cx.editor, path);
    emit_breakpoints_changed(cx.editor);
}

pub fn dap_continue(cx: &mut Context) {
    let debugger = debugger!(cx.editor);

    if let Some(thread_id) = debugger.thread_id {
        let request = debugger.continue_thread(thread_id);

        dap_callback(
            cx.jobs,
            request,
            |editor, _compositor, _response: dap::requests::ContinueResponse| {
                debugger!(editor).resume_application();
            },
        );
    } else {
        cx.editor
            .set_error("Currently active thread is not stopped. Switch the thread.");
    }
}

pub fn dap_pause(cx: &mut Context) {
    thread_picker(cx, |editor, thread| {
        // 排队给事件循环发；不用设 thread_id：DAP 的 `stopped` 事件会告诉我们结果
        editor.dap_intents.push(DapIntent::Request {
            kind: "pause".into(),
            params: json!({ "thread": thread.id.to_string().parse::<i64>().unwrap_or(0) }),
        });
    })
}

pub fn dap_step_in(cx: &mut Context) {
    let debugger = debugger!(cx.editor);

    if let Some(thread_id) = debugger.thread_id {
        let request = debugger.step_in(thread_id);

        dap_callback(cx.jobs, request, |editor, _compositor, _response: ()| {
            debugger!(editor).resume_application();
        });
    } else {
        cx.editor
            .set_error("Currently active thread is not stopped. Switch the thread.");
    }
}

pub fn dap_step_out(cx: &mut Context) {
    let debugger = debugger!(cx.editor);

    if let Some(thread_id) = debugger.thread_id {
        let request = debugger.step_out(thread_id);
        dap_callback(cx.jobs, request, |editor, _compositor, _response: ()| {
            debugger!(editor).resume_application();
        });
    } else {
        cx.editor
            .set_error("Currently active thread is not stopped. Switch the thread.");
    }
}

pub fn dap_next(cx: &mut Context) {
    let debugger = debugger!(cx.editor);

    if let Some(thread_id) = debugger.thread_id {
        let request = debugger.next(thread_id);
        dap_callback(cx.jobs, request, |editor, _compositor, _response: ()| {
            debugger!(editor).resume_application();
        });
    } else {
        cx.editor
            .set_error("Currently active thread is not stopped. Switch the thread.");
    }
}

pub fn dap_variables(cx: &mut Context) {
    // 前置条件全在这里挡住（同步、错误明确）；真正拉数据交给 `dispatch_dap_intents`
    // —— 那是异步的，不能 `block_on` 卡 UI（一次变量视图要 1+2N 次往返）。
    let debugger = debugger!(cx.editor);

    if debugger.thread_id.is_none() {
        cx.editor
            .set_status("Cannot access variables while target is running.");
        return;
    }
    let (frame, thread_id) = match (debugger.active_frame, debugger.thread_id) {
        (Some(frame), Some(thread_id)) => (frame, thread_id),
        _ => {
            cx.editor
                .set_status("Cannot find current stack frame to access variables.");
            return;
        }
    };

    let frame_id = match debugger
        .stack_frames
        .get(&thread_id)
        .and_then(|frames| frames.get(frame))
    {
        Some(stack_frame) => stack_frame.id,
        None => {
            cx.editor.set_error(format!(
                "Failed to get stack frame for thread {thread_id} and frame {frame}."
            ));
            return;
        }
    };

    cx.editor
        .dap_intents
        .push(DapIntent::Variables { frame_id });
}

/// 切换活动调试会话（多会话时才有意义）。
///
/// 每个会话显示 `adapter 名 + 状态`；`▸` 标当前。选中后只改 `active`，不发任何请求
/// —— `stopped`/`continued` 事件仍会照常打到各自的会话上（事件本身带 id）。
pub fn dap_sessions(cx: &mut Context) {
    let ids = cx.editor.debug_adapters.session_ids();
    if ids.is_empty() {
        cx.editor.set_error("没有调试会话（先 :debug-start）");
        return;
    }
    let active = cx.editor.debug_adapters.active_id();
    let items: Vec<(DebugAdapterId, String)> = ids
        .iter()
        .map(|id| {
            let client = cx.editor.debug_adapters.get_client(*id);
            let name = client
                .and_then(|c| c.config.as_ref().map(|cfg| cfg.name.clone()))
                .unwrap_or_else(|| "adapter".to_string());
            let state = match client.map(|c| c.thread_id.is_some()) {
                Some(true) => "stopped",
                Some(false) => "running",
                None => "unknown",
            };
            (
                *id,
                format!(
                    "{} {name} ({state})",
                    if Some(*id) == active { "▸" } else { " " }
                ),
            )
        })
        .collect();

    let columns = [ui::PickerColumn::new(
        "session",
        |item: &(DebugAdapterId, String), _| item.1.as_str().into(),
    )];
    let picker = Picker::new(columns, 0, items, (), move |cx, (id, label), _action| {
        cx.editor.debug_adapters.set_active_client(*id);
        cx.editor
            .set_status(format!("Active debug session: {}", label.trim()));
    });
    cx.push_layer(Box::new(picker))
}

pub fn dap_terminate(cx: &mut Context) {
    cx.editor.set_status("Terminating debug session...");
    // 我们**主动**结束会话时，adapter 未必再回 `terminated` 事件（有的直接断连接）。
    // 所以这里自己发一条 `dap-terminated`：dapui 据此收面板、清只读/视角/标记，
    // 核心据此关掉"本次会话打开的文档"。早先漏了 ⇒ 按 `T` 之后面板/多余 buffer 一直留着。
    // 注意顺序：`debugger!` 会拿走 `&mut`，所有只读查询都要在它之前做完。
    let active = cx.editor.debug_adapters.active_id();
    if let Some(public) = active.and_then(|id| cx.editor.debug_adapters.public_id(id)) {
        cx.editor.emit_dap_event(
            "dap-terminated",
            serde_json::json!({ "id": public, "restart": serde_json::Value::Null }),
        );
    }
    cx.editor.close_debug_opened_docs();
    let debugger = debugger!(cx.editor);

    if debugger
        .caps
        .as_ref()
        .is_some_and(|c| c.supports_terminate_request.unwrap_or_default())
    {
        let terminate_arguments = Some(TerminateArguments {
            restart: Some(false),
        });

        let request = debugger.terminate(terminate_arguments);
        dap_callback(
            cx.jobs,
            request,
            move |editor, _compositor, _response: ()| {
                // **摘掉会话本身**（不能只 unset_active）：
                // 早先只清 active ⇒ registry 里还留着这个 client，`helix.dap.sessions()` 照样返回它
                // ⇒ 状态栏一直挂着 `#1 ⏸ t…`，插件也以为会话还在。
                teardown_session(editor, active);
            },
        );
    } else {
        teardown_session(cx.editor, active);
    }
}

/// 结束一条会话后的统一收尾（`T` 与 Terminated 两条路径一致）。
fn teardown_session(editor: &mut Editor, id: Option<DebugAdapterId>) {
    if let Some(id) = id {
        editor.debug_adapters.remove_client(id);
    }
    editor.debug_adapters.unset_active_client();
    // 断点全部标回"未确认"（● → ◯）
    editor.breakpoints.invalidate_all();
    emit_breakpoints_changed(editor);
}

pub fn dap_enable_exceptions(cx: &mut Context) {
    let debugger = debugger!(cx.editor);

    let filters = match &debugger.capabilities().exception_breakpoint_filters {
        Some(filters) => filters.iter().map(|f| f.filter.clone()).collect(),
        None => return,
    };

    let request = debugger.set_exception_breakpoints(filters);

    dap_callback(
        cx.jobs,
        request,
        |_editor, _compositor, _response: dap::requests::SetExceptionBreakpointsResponse| {
            // editor.set_error(format!("Failed to set up exception breakpoints: {}", e));
        },
    )
}

pub fn dap_disable_exceptions(cx: &mut Context) {
    let debugger = debugger!(cx.editor);

    let request = debugger.set_exception_breakpoints(Vec::new());

    dap_callback(
        cx.jobs,
        request,
        |_editor, _compositor, _response: dap::requests::SetExceptionBreakpointsResponse| {
            // editor.set_error(format!("Failed to set up exception breakpoints: {}", e));
        },
    )
}

/// 写回"某个字段"的公共路径：**优先按 adapter id 定位**。
///
/// 为什么不能只用 `(path, line)`：Prompt 打开期间 adapter 可能回填 `breakpoint` 事件、
/// 把这条断点挪到别的行（原 TODO 说的"offsets can change"）—— 按旧行号写回会改到**别人**身上，
/// 或者悄无声息地什么都不改。id 是 adapter 分配且稳定的；没有 id（还没起会话）才回退到行号。
fn edit_breakpoint_field(
    editor: &mut Editor,
    id: Option<usize>,
    path: &std::path::Path,
    line: usize,
    f: impl FnOnce(&mut Breakpoint),
) -> Option<PathBuf> {
    if let Some(id) = id {
        return editor.breakpoints.update_by_id(id, f).map(|(p, _)| p);
    }
    let mut bp = editor.breakpoints.get(path, line)?.clone();
    f(&mut bp);
    editor.breakpoints.set(path.to_path_buf(), bp);
    Some(path.to_path_buf())
}

pub fn dap_edit_condition(cx: &mut Context) {
    if let Some((path, line, breakpoint)) = get_breakpoint_at_current_line(cx.editor) {
        let bp_id = breakpoint.id;
        let callback = Box::pin(async move {
            let call: Callback =
                Callback::EditorCompositor(Box::new(move |editor, compositor, mut _js| {
                    let mut prompt = Prompt::new(
                        "condition:".into(),
                        None,
                        ui::completers::none,
                        move |cx, input: &str, event: PromptEvent| {
                            if event != PromptEvent::Validate {
                                return;
                            }
                            let value = match input {
                                "" => None,
                                input => Some(input.to_owned()),
                            };
                            match edit_breakpoint_field(cx.editor, bp_id, &path, line, |bp| {
                                bp.condition = value
                            }) {
                                Some(p) => push_breakpoints(cx.editor, p),
                                None => cx.editor.set_error("断点已不存在（可能被 adapter 移除）"),
                            }
                        },
                    );
                    if let Some(condition) = breakpoint.condition {
                        prompt.insert_str(&condition, editor)
                    }
                    compositor.push(Box::new(prompt));
                }));
            Ok(call)
        });
        cx.jobs.callback(callback);
    }
}

pub fn dap_edit_log(cx: &mut Context) {
    if let Some((path, line, breakpoint)) = get_breakpoint_at_current_line(cx.editor) {
        let bp_id = breakpoint.id;
        let callback = Box::pin(async move {
            let call: Callback =
                Callback::EditorCompositor(Box::new(move |editor, compositor, mut _js| {
                    let mut prompt = Prompt::new(
                        "log-message:".into(),
                        None,
                        ui::completers::none,
                        move |cx, input: &str, event: PromptEvent| {
                            if event != PromptEvent::Validate {
                                return;
                            }
                            let value = match input {
                                "" => None,
                                input => Some(input.to_owned()),
                            };
                            match edit_breakpoint_field(cx.editor, bp_id, &path, line, |bp| {
                                bp.log_message = value
                            }) {
                                Some(p) => push_breakpoints(cx.editor, p),
                                None => cx.editor.set_error("断点已不存在（可能被 adapter 移除）"),
                            }
                        },
                    );
                    if let Some(log_message) = breakpoint.log_message {
                        prompt.insert_str(&log_message, editor)
                    }
                    compositor.push(Box::new(prompt));
                }));
            Ok(call)
        });
        cx.jobs.callback(callback);
    }
}

pub fn dap_switch_thread(cx: &mut Context) {
    thread_picker(cx, |editor, thread| {
        // 拉栈帧要等 adapter 回包 ⇒ 排队，不做 `block_on`
        editor.dap_intents.push(DapIntent::SwitchThread(thread.id));
    })
}
pub fn dap_switch_stack_frame(cx: &mut Context) {
    let debugger = debugger!(cx.editor);

    let thread_id = match debugger.thread_id {
        Some(thread_id) => thread_id,
        None => {
            cx.editor.set_error("No thread is currently active");
            return;
        }
    };

    let frames = debugger.stack_frames[&thread_id]
        .iter()
        .cloned()
        .enumerate()
        .collect::<Vec<_>>();
    let thread_state = debugger
        .thread_states
        .get(&thread_id)
        .cloned()
        .unwrap_or_else(|| "unknown".to_string());

    let columns = [ui::PickerColumn::new(
        "frame",
        |item: &(usize, StackFrame), thread_state: &String| {
            format!("{} ({})", item.1.name, thread_state).into()
        },
    )];
    let picker = Picker::new(
        columns,
        0,
        frames,
        thread_state,
        move |cx, (index, frame), _action| {
            let debugger = debugger!(cx.editor);
            debugger.active_frame = Some(*index);
            jump_to_stack_frame(cx.editor, frame);
        },
    )
    .with_preview(move |_editor, _js, (_index, frame)| {
        frame.source.as_ref().and_then(|source| {
            source.path.as_ref().map(|path| {
                (
                    path.as_path().into(),
                    Some((
                        frame.line.saturating_sub(1),
                        frame.end_line.unwrap_or(frame.line).saturating_sub(1),
                    )),
                )
            })
        })
    });
    cx.push_layer(Box::new(picker))
}

// ============================ `helix.dap.*` 的派发面 ============================

/// 从参数里取会话：显式 `id`（对外数字 id）优先，否则活动会话。
fn dap_client_handle(editor: &Editor, params: &Value) -> Result<helix_dap::DapHandle, String> {
    let registry = &editor.debug_adapters;
    let id = match params.get("id").and_then(|v| v.as_u64()) {
        Some(public) => registry
            .by_public_id(public)
            .ok_or_else(|| format!("没有 id={public} 的调试会话")),
        None => registry
            .active_id()
            .ok_or_else(|| "没有活动调试会话（先 helix.dap.launch 或 :debug-start）".to_string()),
    }?;
    registry
        .get_client(id)
        .map(|c| c.handle())
        .ok_or_else(|| "调试会话还没就绪（adapter 启动中）".to_string())
}

/// 会话快照 → JSON（`sessions()` 的数据源）。
pub fn dap_sessions_json(editor: &Editor) -> String {
    let registry = &editor.debug_adapters;
    let active = registry.active_id();
    let items: Vec<Value> = registry
        .session_list()
        .into_iter()
        .map(|(public, id)| {
            let client = registry.get_client(id);
            let name = client
                .and_then(|c| c.config.as_ref().map(|cfg| cfg.name.clone()))
                .unwrap_or_else(|| "adapter".to_string());
            // 被调试的程序名（`/a/b/hello` → `hello`）：多会话时靠它区分"哪个进程"
            // 被调试的程序名（`/a/b/hello` → `hello`）：多会话时靠它区分"哪个进程"。
            // 从**启动参数**里取（adapter 配置里没有 program）。
            let program = client
                .and_then(|c| c.starting_request_args())
                .and_then(|args| args.get("program"))
                .and_then(|v| v.as_str())
                .and_then(|p| {
                    std::path::Path::new(p)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                });
            let thread = client.and_then(|c| c.thread_id);
            let state = if client.is_none() {
                "starting"
            } else if thread.is_some() {
                "stopped"
            } else {
                "running"
            };
            let frame = client.and_then(|client| {
                let thread = client.thread_id?;
                let frame = client.active_frame?;
                let f = client.stack_frames.get(&thread)?.get(frame)?;
                Some(json!({
                    "id": f.id,
                    "name": f.name,
                    // DAP 的 frame.line 是 1 起始 ⇒ 对外统一 0 起始
                    "line": f.line.saturating_sub(1),
                    "column": f.column.saturating_sub(1),
                }))
            });
            // 异常断点过滤器来自 adapter 的 capabilities（每个会话一份，UI 据此渲染勾选表）
            let filters: Vec<Value> = client
                .and_then(|c| c.caps.as_ref())
                .and_then(|caps| caps.exception_breakpoint_filters.as_ref())
                .map(|list| {
                    list.iter()
                        .map(|f| {
                            json!({
                                "filter": f.filter,
                                "label": f.label,
                                "default": f.default.unwrap_or(false),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            // 能力矩阵（`?` 帮助/文档里显示"这个 adapter 到底会什么"，别让人按了才知道）
            let caps = client.and_then(|c| c.caps.as_ref()).map(|c| {
                let flag = |v: Option<bool>| v.unwrap_or(false);
                json!({
                    "conditional": flag(c.supports_conditional_breakpoints),
                    "hitConditional": flag(c.supports_hit_conditional_breakpoints),
                    "logPoints": flag(c.supports_log_points),
                    "restart": flag(c.supports_restart_request),
                    "restartFrame": flag(c.supports_restart_frame),
                    "stepBack": flag(c.supports_step_back),
                    "setVariable": flag(c.supports_set_variable),
                    "terminate": flag(c.supports_terminate_request),
                    "modules": flag(c.supports_modules_request),
                    "memory": flag(c.supports_read_memory_request),
                    "loadedSources": flag(c.supports_loaded_sources_request),
                    "exceptionFilters": c
                        .exception_breakpoint_filters
                        .as_ref()
                        .map(|f| f.len())
                        .unwrap_or(0),
                })
            });
            json!({
                "id": public,
                "name": name,
                "program": program,
                "state": state,
                "active": active == Some(id),
                "thread": thread.and_then(|t| t.to_string().parse::<i64>().ok()),
                "frame": frame,
                "filters": filters,
                "caps": caps,
            })
        })
        .collect();
    Value::Array(items).to_string()
}

/// 断点快照 → JSON（`list_breakpoints()` 的数据源，顺序稳定）。
pub fn dap_breakpoints_json(editor: &Editor) -> String {
    let items: Vec<Value> = editor
        .breakpoints
        .list(None)
        .into_iter()
        .map(|(path, bp)| {
            json!({
                "path": path.to_string_lossy(),
                "line": bp.line,
                "condition": bp.condition,
                "hit": bp.hit_condition,
                "log": bp.log_message,
                "verified": bp.verified,
                "message": bp.message,
                "id": bp.id,
            })
        })
        .collect();
    Value::Array(items).to_string()
}

/// 当前文档上下文：`(workspace, language)`。没有文档/焦点时 `None`（快照退化成空列表）。
/// 当前缓冲的文件路径（检测项目类型用）。
fn current_doc_file(editor: &Editor) -> Option<PathBuf> {
    let view = editor.tree.try_get(editor.tree.focus)?;
    editor
        .documents
        .get(&view.doc)
        .and_then(|d| d.path().map(ToOwned::to_owned))
}

fn doc_ctx(editor: &Editor) -> Option<(PathBuf, Option<String>)> {
    let view = editor.tree.try_get(editor.tree.focus)?;
    let doc = editor.documents.get(&view.doc)?;
    Some((
        doc.workspace_root().to_path_buf(),
        doc.language_name().map(str::to_string),
    ))
}

/// `dap_configs_json` 的缓存指纹：工作区 + 语言 + 两个配置文件的 mtime。
///
/// 为什么需要缓存：`refresh_dap_snapshots` 在 `pump_term_events` 里**每帧**跑，而配置快照要
/// 读 + 解析两个磁盘文件。每帧两次 `open`+`parse` 是纯白烧（且每次都要重新分配一堆 String）。
/// 用 mtime 做钥匙：内容没变就复用（两次 `stat` 是微秒级，且不分配）。
#[derive(PartialEq, Eq, Clone)]
struct ConfigsFingerprint {
    workspace: PathBuf,
    language: Option<String>,
    toml: Option<std::time::SystemTime>,
    json: Option<std::time::SystemTime>,
}

thread_local! {
    static CONFIGS_CACHE: std::cell::RefCell<Option<(ConfigsFingerprint, String)>> =
        const { std::cell::RefCell::new(None) };
}

fn file_mtime(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

/// 清空配置快照缓存（配置层相关的集成测试用；生产路径靠 mtime 自动失效，不需要它）。
pub fn invalidate_dap_configs_cache() {
    CONFIGS_CACHE.with(|c| *c.borrow_mut() = None);
}

/// 读三级启动配置（`launch.toml` → `launch.json` → `languages.toml` 模板）。
///
/// 返回 `(配置列表, 告警)`。告警**只在用户主动开 Picker / 起会话时**显示 —— 这个函数也会被
/// 每帧的快照刷新调用，每帧弹告警等于刷屏。
pub fn load_launch_configs(editor: &Editor) -> (Vec<dap_config::LaunchConfig>, Vec<String>) {
    let Some((workspace, language)) = doc_ctx(editor) else {
        return (Vec::new(), Vec::new());
    };
    let Some(view) = editor.tree.try_get(editor.tree.focus) else {
        return (Vec::new(), Vec::new());
    };
    let Some(doc) = editor.documents.get(&view.doc) else {
        return (Vec::new(), Vec::new());
    };
    let file = doc.path().map(ToOwned::to_owned);
    let templates: Vec<dap_config::LangTemplate> = doc
        .language_config()
        .and_then(|c| c.debugger.as_ref())
        .map(|d| {
            d.templates
                .iter()
                .map(|t| dap_config::LangTemplate {
                    name: t.name.clone(),
                    request: t.request.clone(),
                    args: to_value(&t.args).unwrap_or(Value::Null),
                })
                .collect()
        })
        .unwrap_or_default();
    // 检测项目类型：`${cargoBin}` / `${cargoProfile}` 靠它，`build` 缺省也靠它
    let detected = crate::dap_build::detect(file.as_deref(), &workspace, false);
    let ctx = dap_config::VarCtx {
        workspace: workspace.clone(),
        file: file.clone(),
        cwd: std::env::current_dir().unwrap_or_else(|_| workspace.clone()),
        cargo_bin: detected
            .program
            .as_ref()
            .filter(|_| detected.reason.starts_with("Cargo.toml"))
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned()),
        cargo_profile: "debug".to_string(),
        program: None,
    };
    dap_config::load(&workspace, language.as_deref(), &templates, &ctx)
}

/// 按 `type`（= adapter 名）找 `[language.debugger]`：先看当前文档，再全语言搜。
///
/// `launch.json` 的 `type` 是 adapter 名（`lldb-dap` / `codelldb`），与"当前文档的语言"未必
/// 一致（比如在 `main.rs` 里跑一条 `type: "codelldb"` 的配置）。
/// 找 `dapDebugServer.js`（js-debug 的 DAP 入口）的常见安装位置。
///
/// 为什么要这个:javascript 的 `[language.debugger]` 模板**不带 command**（adapter 不随
/// 编辑器分发），早先报「Command not provided」。这里按常见位置探测，探测到就等于
/// node 也零配置了；探测不到仍旧给"怎么配"的提示。
fn find_js_debug_server() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from)?;
    find_js_debug_server_in(&home)
}

/// 同上，但显式给 home（单测用，避免改进程级 HOME）。
fn find_js_debug_server_in(home: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut roots = vec![
        home.join(".vscode/extensions"),
        home.join(".vscode-server/extensions"),
        home.join(".vscode-insiders/extensions"),
    ];
    roots.push(home.join("vscode-js-debug")); // 手动 git clone 的常见位置
    for root in roots {
        // `vscode-js-debug` checkout：直接 src/dapDebugServer.js
        let direct = root.join("src/dapDebugServer.js");
        if direct.is_file() {
            return Some(direct);
        }
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut best: Option<(std::path::PathBuf, std::time::SystemTime)> = None;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("ms-vscode.js-debug") {
                continue;
            }
            let candidate = entry.path().join("src/dapDebugServer.js");
            if !candidate.is_file() {
                continue;
            }
            // 多版本并存时取"最近修改"的那个
            let mtime = candidate
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            if best.as_ref().map(|(_, t)| mtime > *t).unwrap_or(true) {
                best = Some((candidate, mtime));
            }
        }
        if let Some((path, _)) = best {
            return Some(path);
        }
    }
    None
}

/// 给"没配 command"的 adapter 填上探测到的启动命令（目前只有 js-debug-dap）。
fn fill_missing_command(
    mut cfg: helix_core::syntax::config::DebugAdapterConfig,
) -> helix_core::syntax::config::DebugAdapterConfig {
    if !cfg.command.is_empty() {
        return cfg;
    }
    if cfg.name == "js-debug-dap" {
        if let Some(server) = find_js_debug_server() {
            cfg.command = "node".to_string();
            cfg.args = vec![server.to_string_lossy().into_owned()];
        }
    }
    cfg
}

fn debugger_for(
    editor: &Editor,
    adapter: Option<&str>,
) -> Option<helix_core::syntax::config::DebugAdapterConfig> {
    let current = editor
        .documents
        .get(&editor.tree.try_get(editor.tree.focus)?.doc)
        .and_then(|d| d.language_config())
        .and_then(|c| c.debugger.clone());
    let Some(want) = adapter else {
        return current.map(fill_missing_command);
    };
    if current.as_ref().is_some_and(|c| c.name == want) {
        return current.map(fill_missing_command);
    }
    let loader = editor.syn_loader.load();
    let found = loader
        .language_configs()
        .filter_map(|cfg| cfg.debugger.clone())
        .find(|d| d.name == want);
    found.or(current).map(fill_missing_command)
}

/// 可用调试配置 → JSON（`configs()` 的数据源）。
///
/// P1 只有 `source: "lang"`（`languages.toml` 的 `[language.debugger]` 模板）；
/// `.helix/launch.toml` 与 `.vscode/launch.json` 是 P2。
pub fn dap_configs_json(editor: &Editor) -> String {
    // 每帧跑（刷新快照）⇒ 必须 panic-free、不弹告警、**不重复读盘**。
    let Some((workspace, language)) = doc_ctx(editor) else {
        return "[]".to_string();
    };
    let toml_path = workspace.join(".helix").join("launch.toml");
    let json_path = workspace.join(".vscode").join("launch.json");
    let key = ConfigsFingerprint {
        workspace: workspace.clone(),
        language: language.clone(),
        toml: file_mtime(&toml_path),
        json: file_mtime(&json_path),
    };
    if let Some(cached) = CONFIGS_CACHE.with(|c| {
        c.borrow()
            .as_ref()
            .filter(|(k, _)| *k == key)
            .map(|(_, v)| v.clone())
    }) {
        return cached;
    }

    let (configs, _warnings) = load_launch_configs(editor);
    let adapter_name = editor
        .documents
        .get(
            &editor
                .tree
                .try_get(editor.tree.focus)
                .map(|v| v.doc)
                .unwrap_or_default(),
        )
        .and_then(|d| d.language_config())
        .and_then(|c| c.debugger.as_ref())
        .map(|d| d.name.clone());
    let items: Vec<Value> = configs
        .iter()
        .map(|c| {
            json!({
                "name": c.name,
                "request": c.request,
                "language": c.language,
                "source": c.source.as_str(),
                "adapter": c.adapter.clone().or_else(|| adapter_name.clone()),
            })
        })
        .collect();
    let out = Value::Array(items).to_string();
    CONFIGS_CACHE.with(|c| *c.borrow_mut() = Some((key, out.clone())));
    out
}

/// 刷新三份快照缓存（`pump_term_events` 每轮调一次；数据是几十条，成本可忽略）。
pub fn refresh_dap_snapshots(editor: &Editor, host: Option<&mut helix_js::host::JsHost>) {
    let sessions = dap_sessions_json(editor);
    let breakpoints = dap_breakpoints_json(editor);
    let configs = dap_configs_json(editor);
    match host {
        Some(host) => {
            let st = host.state();
            helix_js::dap::cache_snapshots(st, &sessions, &breakpoints, &configs);
        }
        None => {
            let st = helix_js::host::state_handle_or_init();
            helix_js::dap::cache_snapshots(&st, &sessions, &breakpoints, &configs);
        }
    }
}

fn thread_id_from(n: i64) -> Option<helix_dap::ThreadId> {
    serde_json::from_value(json!(n)).ok()
}

/// 派发一条 `helix.dap.*` 请求：请求在后台任务里跑，回主线程后**先改状态、再兑现 Promise**。
pub fn dispatch_dap_request(
    editor: &mut Editor,
    jobs: &mut Jobs,
    request: helix_js::dap::DapRequest,
) {
    use helix_js::dap::DapOutcome;
    let helix_js::dap::DapRequest { id, kind, params } = request;

    // 起会话没有"已有会话"可依赖：单独一条路
    if kind == "launch" || kind == "attach" {
        dispatch_dap_start(editor, jobs, id, kind, params);
        return;
    }

    // 切换 active 会话：纯本地状态变更（同步做、立刻刷新快照再兑现），不需要 adapter 往返
    if kind == "switch_session" {
        let public = params.get("id").and_then(|v| v.as_u64());
        let outcome = match public.and_then(|p| editor.debug_adapters.by_public_id(p)) {
            Some(sid) => {
                editor.debug_adapters.set_active_client(sid);
                DapOutcome::unit()
            }
            None => DapOutcome::err("switch_session: 没有这个会话".to_string()),
        };
        // 与异步路径同序：先改状态、刷新快照，再兑现 Promise（.then 里读快照必须已是新值）
        refresh_dap_snapshots(editor, None);
        let st = editor_state_handle();
        let _ = helix_js::dap::resolve(&st, id, outcome);
        helix_event::request_redraw();
        return;
    }

    // 断点三件套只改**本地**断点表 ⇒ 没有会话也要能改：先打点、后启动是常态，
    // 而且 `:dapui-breakpoints` 里的 d/c/l 在手写配置阶段最常用。
    // 有会话就顺带推给 adapter；没会话留给 `initialized` 事件全量重放
    // （`DapIntent::SyncBreakpoints` 已经这么处理了）。
    let local_only = matches!(
        kind.as_str(),
        "set_breakpoint" | "remove_breakpoint" | "clear_breakpoints"
    );
    let handle = match dap_client_handle(editor, &params) {
        Ok(handle) => Some(handle),
        Err(_) if local_only => None,
        Err(err) => {
            let st = editor_state_handle();
            let _ = helix_js::dap::resolve(&st, id, DapOutcome::err(err));
            return;
        }
    };

    // 会话上下文（同步取一次）：线程 id、重启参数、断点归属
    let session_id = match params.get("id").and_then(|v| v.as_u64()) {
        Some(public) => editor.debug_adapters.by_public_id(public),
        None => editor.debug_adapters.active_id(),
    };
    let (client_thread, restart_args) = session_id
        .and_then(|sid| editor.debug_adapters.get_client(sid))
        .map(|c| {
            (
                c.thread_id,
                c.starting_request_args().cloned().unwrap_or(Value::Null),
            )
        })
        .unwrap_or((None, Value::Null));

    let mut params = params;

    // 需要**先**改本地状态的（断点）：同步改完，把要推的载荷快照交给后台任务
    let mut breakpoint_push: Option<(PathBuf, Vec<dap::SourceBreakpoint>)> = None;
    match kind.as_str() {
        "set_breakpoint" => {
            let path = PathBuf::from(params["path"].as_str().unwrap_or_default());
            let row = params["row"].as_u64().unwrap_or(0) as usize;
            let mut bp = editor
                .breakpoints
                .get(&path, row)
                .cloned()
                .unwrap_or_else(|| Breakpoint::new(row));
            if let Some(opts) = params.get("opts") {
                bp.condition = opts
                    .get("condition")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                bp.hit_condition = opts.get("hit").and_then(|v| v.as_str()).map(String::from);
                bp.log_message = opts.get("log").and_then(|v| v.as_str()).map(String::from);
            }
            editor.breakpoints.set(path.clone(), bp);
            breakpoint_push = Some((path.clone(), editor.breakpoints.source_breakpoints(&path)));
        }
        "remove_breakpoint" => {
            let path = PathBuf::from(params["path"].as_str().unwrap_or_default());
            let row = params["row"].as_u64().unwrap_or(0) as usize;
            editor.breakpoints.remove(&path, row);
            emit_breakpoints_changed(editor);
            breakpoint_push = Some((path.clone(), editor.breakpoints.source_breakpoints(&path)));
        }
        "clear_breakpoints" => {
            match params.get("path").and_then(|v| v.as_str()) {
                Some(path) => {
                    let path = PathBuf::from(path);
                    editor.breakpoints.clear(Some(&path));
                    breakpoint_push =
                        Some((path.clone(), editor.breakpoints.source_breakpoints(&path)));
                }
                None => {
                    let paths = editor.breakpoints.paths();
                    editor.breakpoints.clear(None);
                    // 全部清空：每个曾有的文件都推一次空列表，否则 adapter 侧还留着旧断点
                    for path in paths {
                        editor.dap_intents.push(DapIntent::SyncBreakpoints(path));
                    }
                    breakpoint_push = None;
                }
            }
        }
        _ => {}
    }

    // 线程缺省值：走"当前停止的线程"
    let need_thread = matches!(
        kind.as_str(),
        "continue" | "pause" | "step_in" | "step_out" | "next"
    );
    if need_thread && params.get("thread").is_none() {
        if let Some(thread) = client_thread {
            params["thread"] = json!(thread.to_string().parse::<i64>().unwrap_or(0));
        }
    }

    let kind_for_job = kind.clone();
    let params_for_job = params.clone();
    let job = Box::pin(async move {
        let outcome = match kind_for_job.as_str() {
            // 断点：本地表已经改好；有会话才推，没会话就只兑现本地状态
            "set_breakpoint" | "remove_breakpoint" | "clear_breakpoints" => {
                match (handle, breakpoint_push) {
                    (Some(handle), Some((path, source_breakpoints))) => {
                        match handle.set_breakpoints(path, source_breakpoints).await {
                            Ok(_) => DapOutcome::unit(),
                            Err(e) => DapOutcome::err(format!("设置断点失败: {e}")),
                        }
                    }
                    _ => DapOutcome::unit(),
                }
            }
            kind => match handle {
                Some(handle) => match kind {
                    "restart" => match handle.call::<dap::requests::Restart>(restart_args).await {
                        Ok(_) => DapOutcome::unit(),
                        Err(e) => DapOutcome::err(format!("restart 失败: {e}")),
                    },
                    _ => run_dap_request(&handle, &kind_for_job, &params_for_job).await,
                },
                // 非断点类请求在上面已挡过“没会话”，这里只是防御
                None => DapOutcome::err("没有活动调试会话（先 :debug-start）".to_string()),
            },
        };
        let call: Callback = Callback::EditorCompositor(Box::new(
            move |editor: &mut Editor, _compositor: &mut Compositor, mut js| {
                // 副作用在**兑现之前**做：Promise 的 .then 里读快照必须已是新状态
                apply_dap_side_effects(editor, &kind_for_job, &outcome);
                refresh_dap_snapshots(editor, js.as_deref_mut());
                // `HostState` 是 Rc 内部的，clone 一次比借来借去省事（Promise 兑现要 &HostState）
                let st = match js.as_deref() {
                    Some(host) => std::rc::Rc::clone(host.state_rc()),
                    None => editor_state_handle(),
                };
                let _ = helix_js::dap::resolve(&st, id, outcome);
                // 兑现后**立刻**把 `.then` 续体泵掉（否则插件里 `.then(...)` 的渲染要等
                // 下一次按键）。注意 `pump_jobs` 自带重入保护：已经在泵里时只记标志，
                // 由外层那次 `run_jobs` 排空 —— 重入会挂死事件循环。
                let pumped = match js {
                    Some(host) => host.pump_jobs(),
                    None => helix_js::pump_jobs(),
                };
                if let Err(err) = pumped {
                    log::error!("dap resolve pump: {err}");
                }
                helix_event::request_redraw();
            },
        ));
        Ok(call)
    });
    jobs.callback(job);
}

fn editor_state_handle() -> std::rc::Rc<helix_js::host::HostState> {
    helix_js::host::state_handle_or_init()
}

/// 请求成功后的本地状态更新（恢复运行 / 终止后清理）。
fn apply_dap_side_effects(editor: &mut Editor, kind: &str, outcome: &helix_js::dap::DapOutcome) {
    if matches!(outcome, helix_js::dap::DapOutcome::Err(_)) {
        return;
    }
    match kind {
        "continue" | "step_in" | "step_out" | "next" => {
            if let Some(debugger) = editor.debug_adapters.get_active_client_mut() {
                debugger.resume_application();
            }
        }
        "terminate" => {
            if let Some(id) = editor.debug_adapters.active_id() {
                let public = editor.debug_adapters.public_id(id).unwrap_or(0);
                editor.debug_adapters.remove_client(id);
                editor.debug_adapters.unset_active_client();
                editor.emit_dap_event("dap-session", json!({ "id": public, "kind": "removed" }));
            }
            editor.breakpoints.invalidate_all();
            editor.set_status("调试会话已结束");
        }
        _ => {}
    }
}

/// 真正发 DAP 请求（后台任务里跑）。
async fn run_dap_request(
    handle: &helix_dap::DapHandle,
    kind: &str,
    params: &Value,
) -> helix_js::dap::DapOutcome {
    use helix_js::dap::DapOutcome;
    let thread = || {
        params
            .get("thread")
            .and_then(|v| v.as_i64())
            .and_then(thread_id_from)
    };
    match kind {
        "terminate" => match handle
            .call::<dap::requests::Terminate>(Some(dap::requests::TerminateArguments {
                restart: None,
            }))
            .await
        {
            Ok(_) => DapOutcome::unit(),
            Err(e) => DapOutcome::err(format!("terminate 失败: {e}")),
        },
        "continue" => match thread() {
            Some(tid) => match handle
                .call::<dap::requests::Continue>(dap::requests::ContinueArguments {
                    thread_id: tid,
                })
                .await
            {
                Ok(_) => DapOutcome::unit(),
                Err(e) => DapOutcome::err(format!("continue 失败: {e}")),
            },
            None => DapOutcome::err("continue: 当前没有停止的线程（先 :debug-sessions 切换会话）"),
        },
        "pause" => match thread() {
            Some(tid) => match handle
                .call::<dap::requests::Pause>(dap::requests::PauseArguments { thread_id: tid })
                .await
            {
                Ok(_) => DapOutcome::unit(),
                Err(e) => DapOutcome::err(format!("pause 失败: {e}")),
            },
            None => DapOutcome::err("pause: 需要 thread"),
        },
        "step_in" | "step_out" | "next" => {
            let Some(tid) = thread() else {
                return DapOutcome::err(format!("{kind}: 当前没有停止的线程"));
            };
            let sent = match kind {
                "step_in" => {
                    handle
                        .call::<dap::requests::StepIn>(dap::requests::StepInArguments {
                            thread_id: tid,
                            target_id: None,
                            granularity: None,
                        })
                        .await
                }
                "step_out" => {
                    handle
                        .call::<dap::requests::StepOut>(dap::requests::StepOutArguments {
                            thread_id: tid,
                            granularity: None,
                        })
                        .await
                }
                _ => {
                    handle
                        .call::<dap::requests::Next>(dap::requests::NextArguments {
                            thread_id: tid,
                            granularity: None,
                        })
                        .await
                }
            };
            match sent {
                Ok(_) => DapOutcome::unit(),
                Err(e) => DapOutcome::err(format!("{kind} 失败: {e}")),
            }
        }
        "threads" => {
            match handle
                .request::<dap::requests::Threads>(Some(dap::requests::ThreadsArguments {}))
                .await
            {
                Ok(resp) => DapOutcome::json(
                    resp.threads
                        .into_iter()
                        .map(|t| json!({"id": t.id.to_string().parse::<i64>().unwrap_or(0), "name": t.name}))
                        .collect::<Vec<_>>(),
                ),
                Err(e) => DapOutcome::err(format!("threads 失败: {e}")),
            }
        }
        "stack_frames" => {
            let Some(tid) = thread() else {
                return DapOutcome::err("stack_frames: 需要 thread");
            };
            match handle.stack_trace(tid).await {
                Ok((frames, _)) => DapOutcome::json(
                    frames
                        .into_iter()
                        .map(|f| {
                            json!({
                                "id": f.id,
                                "name": f.name,
                                "source": f.source.as_ref().and_then(|s| s.path.as_ref()).map(|p| p.to_string_lossy().to_string()),
                                // 0 起始（与 row 口径统一）
                                "line": f.line.saturating_sub(1),
                                "column": f.column.saturating_sub(1),
                                "end_line": f.end_line.map(|l| l.saturating_sub(1)),
                                "end_column": f.end_column.map(|c| c.saturating_sub(1)),
                            })
                        })
                        .collect::<Vec<_>>(),
                ),
                Err(e) => DapOutcome::err(format!("stack_frames 失败: {e}")),
            }
        }
        "scopes" => {
            let Some(frame) = params.get("frame").and_then(|v| v.as_u64()) else {
                return DapOutcome::err("scopes: 需要 frame");
            };
            match handle.scopes(frame as usize).await {
                Ok(scopes) => DapOutcome::json(
                    scopes
                        .into_iter()
                        .map(|s| {
                            json!({
                                "name": s.name,
                                "variablesReference": s.variables_reference,
                                "expensive": s.expensive,
                            })
                        })
                        .collect::<Vec<_>>(),
                ),
                Err(e) => DapOutcome::err(format!("scopes 失败: {e}")),
            }
        }
        "variables" => {
            let Some(reference) = params.get("reference").and_then(|v| v.as_u64()) else {
                return DapOutcome::err("variables: 需要 reference");
            };
            let start = params
                .get("start")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize);
            let count = params
                .get("count")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize);
            match handle
                .variables_paged(reference as usize, start, count)
                .await
            {
                Ok(vars) => DapOutcome::json(
                    vars.into_iter()
                        .map(|v| {
                            json!({
                                "name": v.name,
                                "value": v.value,
                                "type": v.ty,
                                "variablesReference": v.variables_reference,
                                "namedVariables": v.named_variables,
                                "indexedVariables": v.indexed_variables,
                            })
                        })
                        .collect::<Vec<_>>(),
                ),
                Err(e) => DapOutcome::err(format!("variables 失败: {e}")),
            }
        }
        "eval" => {
            let expr = params
                .get("expr")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let frame = params
                .get("frame")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize);
            let args = dap::requests::EvaluateArguments {
                expression: expr,
                frame_id: frame,
                context: None,
                format: None,
            };
            match handle.request::<dap::requests::Evaluate>(args).await {
                Ok(resp) => DapOutcome::json(json!({
                    "result": resp.result,
                    "type": resp.ty,
                    "variablesReference": resp.variables_reference,
                })),
                Err(e) => DapOutcome::err(format!("eval 失败: {e}")),
            }
        }
        "set_exception_breakpoints" => {
            let filters: Vec<String> = params
                .get("filters")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            match handle
                .call::<dap::requests::SetExceptionBreakpoints>(
                    dap::requests::SetExceptionBreakpointsArguments { filters },
                )
                .await
            {
                Ok(_) => DapOutcome::unit(),
                Err(e) => DapOutcome::err(format!("set_exception_breakpoints 失败: {e}")),
            }
        }
        "request" => {
            let name = params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let args = params.get("args").cloned();
            match handle.call_raw(name, args).await {
                Ok(v) => DapOutcome::json(v),
                Err(e) => DapOutcome::err(format!("request({name}) 失败: {e}")),
            }
        }
        other => DapOutcome::err(format!("helix.dap.{other} 还没实现")),
    }
}

/// `helix.dap.launch` / `attach`：与 `:debug-start` 同一条路（reserve → 后台 spawn → 回调插入）。
fn dispatch_dap_start(
    editor: &mut Editor,
    jobs: &mut Jobs,
    request_id: u64,
    kind: String,
    params: Value,
) {
    use helix_js::dap::DapOutcome;
    let doc = doc!(editor);
    let Some(config) = doc.language_config().and_then(|c| c.debugger.as_ref()) else {
        let st = editor_state_handle();
        let _ = helix_js::dap::resolve(
            &st,
            request_id,
            DapOutcome::err("这个语言没有配置调试适配器（[language.debugger]）"),
        );
        return;
    };
    let config = config.clone();
    let language = doc.language_name().unwrap_or_default().to_string();
    let template = match params.get("name").and_then(|v| v.as_str()) {
        Some(name) => config.templates.iter().find(|t| t.name == name).cloned(),
        None => config.templates.first().cloned(),
    };
    let Some(template) = template else {
        let st = editor_state_handle();
        let _ = helix_js::dap::resolve(
            &st,
            request_id,
            DapOutcome::err(format!("没有名为 {:?} 的调试模板", params.get("name"))),
        );
        return;
    };
    // 模板参数 + 调用方覆盖
    let mut args: std::collections::HashMap<String, Value> = template
        .args
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    args.insert("cwd".into(), json!(helix_stdx::env::current_working_dir()));
    if let Some(extra) = params.get("args").and_then(|v| v.as_object()) {
        for (k, v) in extra {
            args.insert(k.clone(), v.clone());
        }
    }
    let args = Value::Object(args.into_iter().collect());
    let start_id = editor.debug_adapters.reserve();
    let public_id = editor.debug_adapters.public_id(start_id);
    let config_for_job = config.clone();
    let kind_for_job = kind.clone();
    let adapter_name = config.name.clone();

    let job = Box::pin(async move {
        let spawned =
            helix_dap::registry::Registry::spawn_raw(start_id, None, &config_for_job).await;
        let (client, receiver) = match spawned {
            Ok(v) => v,
            Err(e) => {
                let st = editor_state_handle();
                let _ = helix_js::dap::resolve(
                    &st,
                    request_id,
                    DapOutcome::err(format!("启动 adapter 失败: {e}")),
                );
                return Err(anyhow!("spawn_raw: {e}"));
            }
        };
        let handle = client.handle();
        let caps = match handle
            .request::<dap::requests::Initialize>(helix_dap::initialize_arguments(&adapter_name))
            .await
        {
            Ok(caps) => caps,
            Err(e) => {
                let st = editor_state_handle();
                let _ = helix_js::dap::resolve(
                    &st,
                    request_id,
                    DapOutcome::err(format!("initialize 失败: {e}")),
                );
                return Err(anyhow!("initialize: {e}"));
            }
        };
        let started = if kind_for_job == "launch" {
            handle.call::<dap::requests::Launch>(args).await
        } else {
            handle.call::<dap::requests::Attach>(args).await
        };

        let call: Callback = Callback::EditorCompositor(Box::new(
            move |editor: &mut Editor, _compositor: &mut Compositor, mut js| {
                let inserted = editor.debug_adapters.insert_client(
                    start_id,
                    client,
                    receiver,
                    &config_for_job,
                    true,
                );
                let outcome = match inserted {
                    Ok(_) => {
                        if let Some(c) = editor.debug_adapters.get_client_mut(start_id) {
                            c.caps = Some(caps);
                        }
                        match started {
                            Ok(_) => {
                                editor.emit_dap_event(
                                    "dap-session",
                                    json!({
                                        "id": public_id,
                                        "kind": "created",
                                        "name": adapter_name,
                                        "state": "running",
                                    }),
                                );
                                DapOutcome::json(json!({
                                    "id": public_id,
                                    "name": adapter_name,
                                    "language": language,
                                    "state": "running",
                                }))
                            }
                            Err(e) => DapOutcome::err(format!("{} 请求失败: {e}", kind_for_job)),
                        }
                    }
                    Err(e) => DapOutcome::err(format!("插入会话失败: {e}")),
                };
                refresh_dap_snapshots(editor, js.as_deref_mut());
                // `HostState` 是 Rc 内部的，clone 一次比借来借去省事（Promise 兑现要 &HostState）
                let st = match js.as_deref() {
                    Some(host) => std::rc::Rc::clone(host.state_rc()),
                    None => editor_state_handle(),
                };
                let _ = helix_js::dap::resolve(&st, request_id, outcome);
            },
        ));
        Ok(call)
    });
    jobs.callback(job);
}

// ============================ `:` 命令（与键位同一实现） ============================
//
// 为什么要有：DAP 动作原先**只有键位**（`<space>G b` 之类），`:` 补全里一个都看不到 ——
// 用户的原话就是"完全找不到怎么进入"。这里把常用的几个接成 typed 命令，
// 实现一律走 `DapIntent`（与键位路径完全同一条代码）。

/// `:debug-toggle-breakpoint`
pub fn typed_toggle_breakpoint(cx: &mut crate::compositor::Context) {
    let Some((path, line)) = current_path_line(cx.editor) else {
        cx.editor
            .set_error("Can't set breakpoint: document has no path");
        return;
    };
    let set = cx.editor.breakpoints.toggle(path.clone(), line);
    push_breakpoints(cx.editor, path);
    cx.editor.set_status(if set {
        format!("Breakpoint set: {}", line + 1)
    } else {
        format!("Breakpoint cleared: {}", line + 1)
    });
}

/// `:debug-continue` / `:debug-next` / `:debug-step-in` / `:debug-step-out` / `:debug-terminate`
pub fn typed_request(cx: &mut crate::compositor::Context, kind: &str) {
    if cx.editor.debug_adapters.get_active_client().is_none() {
        cx.editor.set_error("没有活动调试会话（先 :debug-start）");
        return;
    }
    cx.editor.dap_intents.push(DapIntent::Request {
        kind: kind.to_string(),
        params: json!({}),
    });
}

/// `:debug-variables`（弹层列作用域/变量）
pub fn typed_variables(cx: &mut crate::compositor::Context) {
    let Some(debugger) = cx.editor.debug_adapters.get_active_client() else {
        cx.editor.set_error("没有活动调试会话（先 :debug-start）");
        return;
    };
    let (Some(thread_id), Some(frame)) = (debugger.thread_id, debugger.active_frame) else {
        cx.editor
            .set_status("Cannot access variables while target is running.");
        return;
    };
    let Some(frame_id) = debugger
        .stack_frames
        .get(&thread_id)
        .and_then(|frames| frames.get(frame))
        .map(|f| f.id)
    else {
        cx.editor
            .set_error(format!("Failed to get stack frame for thread {thread_id}"));
        return;
    };
    cx.editor
        .dap_intents
        .push(DapIntent::Variables { frame_id });
}

/// `:debug-sessions`（多会话切换）
///
/// Picker **在回调里构造**：`Picker` 的回调不是 `Send`，先建好再搬进 job 会编译不过
/// （`theme` / LSP code action 那些 typed 命令也是这么写的）。
pub fn typed_sessions(cx: &mut crate::compositor::Context) {
    let ids = cx.editor.debug_adapters.session_list();
    if ids.is_empty() {
        cx.editor.set_error("没有调试会话（先 :debug-start）");
        return;
    }
    let active = cx.editor.debug_adapters.active_id();
    let items: Vec<(DebugAdapterId, String)> = ids
        .into_iter()
        .map(|(public, id)| {
            let client = cx.editor.debug_adapters.get_client(id);
            let name = client
                .and_then(|c| c.config.as_ref().map(|cfg| cfg.name.clone()))
                .unwrap_or_else(|| "adapter".to_string());
            let state = if client.map(|c| c.thread_id.is_some()).unwrap_or(false) {
                "stopped"
            } else {
                "running"
            };
            let mark = if Some(id) == active { "▸" } else { " " };
            (id, format!("{mark} #{public} {name} ({state})"))
        })
        .collect();

    let job = Box::pin(async move {
        let call: Callback = Callback::EditorCompositor(Box::new(
            move |_editor: &mut Editor, compositor: &mut Compositor, mut _js| {
                let columns = [ui::PickerColumn::new(
                    "session",
                    |item: &(DebugAdapterId, String), _| item.1.as_str().into(),
                )];
                let picker = Picker::new(columns, 0, items, (), move |cx, (id, label), _action| {
                    cx.editor.debug_adapters.set_active_client(*id);
                    cx.editor
                        .set_status(format!("Active debug session: {}", label.trim()));
                });
                compositor.push(Box::new(picker));
            },
        ));
        Ok(call)
    });
    cx.jobs.callback(job);
}

/// 把一个 stdout/stderr 逐行灌进流式通道（构建输出）。
async fn stream_lines<R>(reader: R, category: &'static str)
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncBufReadExt;
    let mut lines = tokio::io::BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        crate::dap_output::push(category, format!("[build] {line}"));
    }
}

#[cfg(test)]
mod js_debug_probe_tests {
    use super::find_js_debug_server_in;

    /// 按 VS Code 扩展目录结构探测；多版本取最近修改。
    #[test]
    fn finds_dap_debug_server_in_vscode_extensions() {
        let root = std::env::temp_dir().join(format!("hx-jsdebug-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let ext = root.join(".vscode/extensions/ms-vscode.js-debug-9.9.9/src");
        std::fs::create_dir_all(&ext).unwrap();
        let server = ext.join("dapDebugServer.js");
        std::fs::write(&server, "// fake").unwrap();

        let got = find_js_debug_server_in(&root).expect("应探测到 dapDebugServer.js");
        assert_eq!(got, server);

        // 没有扩展目录 ⇒ None（不 panic）
        let empty = root.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(find_js_debug_server_in(&empty).is_none());

        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod build_tool_hint_tests {
    #[test]
    fn build_tool_hint_names_the_tool() {
        assert_eq!(
            super::build_tool("meson setup build && meson compile -C build"),
            "meson"
        );
        assert_eq!(super::build_tool("cargo build"), "cargo");
        let hint = super::toolchain_hint("meson compile -C build", "No such file or directory");
        assert!(hint.contains("meson"), "{hint}");
    }
}
