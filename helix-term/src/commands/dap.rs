use super::{Context, Editor};
use crate::{
    compositor::{self, Compositor},
    job::{Callback, Jobs},
    ui::{self, overlay::overlaid, Picker, Popup, Prompt, PromptEvent, Text},
};
use dap::{StackFrame, Thread, ThreadStates};
use helix_core::syntax::config::{DebugConfigCompletion, DebugTemplate};
use helix_dap::{self as dap, requests::TerminateArguments};
use helix_view::editor::Breakpoint;

use serde_json::{json, to_value, Value};
use tui::text::Spans;

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
fn push_breakpoints(editor: &mut Editor, path: PathBuf) {
    editor.dap_intents.push(DapIntent::SyncBreakpoints(path));
}

/// 冲刷 [`Editor::dap_intents`]：把每个意图变成"异步请求 + 主线程回填"的 job。
///
/// **事件循环专用**（`application.rs::pump_term_events`）—— 那是唯一同时握着 `&mut Editor`
/// 与 `&mut Jobs` 的地方。命令与 Picker 回调只负责 `push`，一次都不等 adapter。
pub fn dispatch_dap_intents(editor: &mut Editor, jobs: &mut Jobs) {
    for intent in std::mem::take(&mut editor.dap_intents) {
        dispatch_one(editor, jobs, intent);
    }
}

fn dispatch_one(editor: &mut Editor, jobs: &mut Jobs, intent: DapIntent) {
    match intent {
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
            let scope_style = editor.theme.get("ui.linenr.selected");
            let type_style = editor.theme.get("ui.text");
            let text_style = editor.theme.get("ui.text.focus");
            let job = Box::pin(async move {
                let scopes = handle.scopes(frame_id).await?;
                let mut lines = Vec::new();
                for scope in &scopes {
                    lines.push(Spans::from(tui::text::Span::styled(
                        format!("▸ {}", scope.name),
                        scope_style,
                    )));
                    if let Ok(vars) = handle.variables(scope.variables_reference).await {
                        lines.reserve(vars.len());
                        for var in vars {
                            let mut spans = Vec::with_capacity(5);
                            spans.push(tui::text::Span::styled(var.name.to_owned(), text_style));
                            if let Some(ty) = var.ty {
                                spans.push(tui::text::Span::raw(": "));
                                spans.push(tui::text::Span::styled(ty, type_style));
                            }
                            spans.push(tui::text::Span::raw(" = "));
                            spans.push(tui::text::Span::styled(var.value.to_owned(), text_style));
                            lines.push(Spans::from(spans));
                        }
                    }
                }
                let contents = Text::from(tui::text::Text::from(lines));
                let call: Callback =
                    Callback::EditorCompositor(Box::new(move |_editor, compositor, mut _js| {
                        let popup = Popup::new("dap-variables", contents);
                        compositor.replace_or_push("dap-variables", popup);
                    }));
                Ok(call)
            });
            jobs.callback(job);
        }
    }
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
fn start_session_with(
    cx: &mut compositor::Context,
    debugger_cfg: helix_core::syntax::config::DebugAdapterConfig,
    request: String,
    args: Value,
    socket: Option<std::net::SocketAddr>,
) -> Result<(), anyhow::Error> {
    // Refuse to spawn a debug adapter in workspace trust restricted mode.
    let workspace = doc!(cx.editor).workspace_root().to_path_buf();
    if !cx
        .editor
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

    let id = cx.editor.debug_adapters.reserve();
    let adapter_name = debugger_cfg.name.clone();
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
        // launch / attach：响应体当前没有消费方（与旧实现一致），但错误要能在状态栏看到
        let sent = match request.as_str() {
            "launch" => handle.call::<dap::requests::Launch>(args).await.map(|_| ()),
            _ => handle.call::<dap::requests::Attach>(args).await.map(|_| ()),
        };
        let launch_error = sent.err().map(|e| e.to_string());

        Ok(Callback::EditorCompositor(Box::new(
            move |editor: &mut Editor, _compositor: &mut Compositor, mut _js| {
                match editor
                    .debug_adapters
                    .insert_client(id, client, receiver, &debugger_cfg, true)
                {
                    Ok(_) => {
                        if let Some(client) = editor.debug_adapters.get_client_mut(id) {
                            client.caps = Some(caps);
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
                        match launch_error {
                            Some(err) => editor.set_error(format!(
                                "Debug session started, but {} failed: {err}",
                                request
                            )),
                            None => editor.set_status("Debugged application started"),
                        }
                    }
                    Err(err) => editor.set_error(format!("Failed to attach debugger: {err}")),
                }
            },
        )))
    });
    cx.jobs.callback(callback);
    // TODO: either await "initialized" or buffer commands until event is received
    Ok(())
}

/// `:debug-start [模板名] [参数…]`：走 `languages.toml` 的模板（保留原有的参数提示流程）。
pub fn dap_start_impl(
    cx: &mut compositor::Context,
    name: Option<&str>,
    socket: Option<std::net::SocketAddr>,
    params: Option<Vec<std::borrow::Cow<str>>>,
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

    // TODO: avoid refetching all of this... pass a config in
    let template = match name {
        Some(name) => config.templates.iter().find(|t| t.name == name).cloned(),
        None => config.templates.first().cloned(),
    }
    .ok_or_else(|| anyhow!("No debug config with given name"))?;

    let mut args: HashMap<&str, Value> = if let Some(params) = params.as_ref() {
        let preprocessed_params = prepare_dap_params(&template, params);
        template
            .args
            .iter()
            .map(|(k, v)| (k.as_str(), map_value(v, &preprocessed_params)))
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

    start_session_with(cx, config, template.request.clone(), args, socket)
}

/// 用一条 `launch.toml` / `launch.json` 配置起会话（参数已做过变量替换）。
pub fn start_with_launch_config(
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
    start_session_with(cx, debugger_cfg, cfg.request, cfg.args, socket)
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
    label: String,
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

    let entries: Vec<LaunchEntry> = configs
        .into_iter()
        .map(|cfg| {
            let label = format!(
                "{:<12} {:<7} {:<9} {}",
                cfg.name,
                cfg.request,
                cfg.source.as_str(),
                cfg.language.clone().unwrap_or_else(|| "-".into())
            );
            LaunchEntry { label, cfg }
        })
        .collect();

    let columns = [ui::PickerColumn::new(
        "配置 · 请求 · 来源 · 语言",
        |item: &LaunchEntry, _| item.label.as_str().into(),
    )];

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
            if let Err(err) = start_with_launch_config(cx, cfg, None) {
                cx.editor.set_error(err.to_string());
            }
        },
    ))))
}

pub fn dap_restart(cx: &mut Context) {
    let debugger = match cx.editor.debug_adapters.get_active_client() {
        Some(debugger) => debugger,
        None => {
            cx.editor.set_error("Debugger is not running");
            return;
        }
    };
    if !debugger
        .capabilities()
        .supports_restart_request
        .unwrap_or(false)
    {
        cx.editor
            .set_error("Debugger does not support session restarts");
        return;
    }
    if debugger.starting_request_args().is_none() {
        cx.editor
            .set_error("No arguments found with which to restart the sessions");
        return;
    }

    dap_callback(
        cx.jobs,
        debugger.restart(),
        |editor, _compositor, _resp: ()| editor.set_status("Debugging session restarted"),
    );
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
        dap_callback(cx.jobs, request, |editor, _compositor, _response: ()| {
            // editor.set_error(format!("Failed to disconnect: {}", e));
            editor.debug_adapters.unset_active_client();
        });
    } else {
        cx.editor.debug_adapters.unset_active_client();
    }
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

// TODO: both edit condition and edit log need to be stable: we might get new breakpoints from the debugger which can change offsets
pub fn dap_edit_condition(cx: &mut Context) {
    if let Some((path, line, breakpoint)) = get_breakpoint_at_current_line(cx.editor) {
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
                            // 条件写回 store（按 path+line 定位；下标在 store 里没有意义）
                            if let Some(bp) = cx.editor.breakpoints.get(&path, line).cloned() {
                                let mut bp = bp;
                                bp.condition = match input {
                                    "" => None,
                                    input => Some(input.to_owned()),
                                };
                                cx.editor.breakpoints.set(path.clone(), bp);
                            }
                            push_breakpoints(cx.editor, path.clone());
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
                            if let Some(bp) = cx.editor.breakpoints.get(&path, line).cloned() {
                                let mut bp = bp;
                                bp.log_message = match input {
                                    "" => None,
                                    input => Some(input.to_owned()),
                                };
                                cx.editor.breakpoints.set(path.clone(), bp);
                            }
                            push_breakpoints(cx.editor, path.clone());
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
            json!({
                "id": public,
                "name": name,
                "state": state,
                "active": active == Some(id),
                "thread": thread.and_then(|t| t.to_string().parse::<i64>().ok()),
                "frame": frame,
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

/// 读三级启动配置（`launch.toml` → `launch.json` → `languages.toml` 模板）。
///
/// 返回 `(配置列表, 告警)`。告警**只在用户主动开 Picker / 起会话时**显示 —— 这个函数也会被
/// 每帧的快照刷新调用，每帧弹告警等于刷屏。
pub fn load_launch_configs(editor: &Editor) -> (Vec<dap_config::LaunchConfig>, Vec<String>) {
    let Some(view) = editor.tree.try_get(editor.tree.focus) else {
        return (Vec::new(), Vec::new());
    };
    let Some(doc) = editor.documents.get(&view.doc) else {
        return (Vec::new(), Vec::new());
    };
    let workspace = doc.workspace_root().to_path_buf();
    let file = doc.path().map(ToOwned::to_owned);
    let language = doc.language_name().map(str::to_string);
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
    let ctx = dap_config::VarCtx {
        workspace: workspace.clone(),
        file: file.clone(),
        cwd: std::env::current_dir().unwrap_or_else(|_| workspace.clone()),
    };
    dap_config::load(&workspace, language.as_deref(), &templates, &ctx)
}

/// 按 `type`（= adapter 名）找 `[language.debugger]`：先看当前文档，再全语言搜。
///
/// `launch.json` 的 `type` 是 adapter 名（`lldb-dap` / `codelldb`），与"当前文档的语言"未必
/// 一致（比如在 `main.rs` 里跑一条 `type: "codelldb"` 的配置）。
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
        return current;
    };
    if current.as_ref().is_some_and(|c| c.name == want) {
        return current;
    }
    let loader = editor.syn_loader.load();
    let found = loader
        .language_configs()
        .filter_map(|cfg| cfg.debugger.clone())
        .find(|d| d.name == want);
    found.or(current)
}

/// 可用调试配置 → JSON（`configs()` 的数据源）。
///
/// P1 只有 `source: "lang"`（`languages.toml` 的 `[language.debugger]` 模板）；
/// `.helix/launch.toml` 与 `.vscode/launch.json` 是 P2。
pub fn dap_configs_json(editor: &Editor) -> String {
    // 每帧跑（刷新快照）⇒ 必须 panic-free 且不弹告警：这里只取条目，告警由 Picker 路径显示。
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
    Value::Array(items).to_string()
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

    let handle = match dap_client_handle(editor, &params) {
        Ok(handle) => handle,
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
            "set_breakpoint" | "remove_breakpoint" | "clear_breakpoints" => match breakpoint_push {
                Some((path, source_breakpoints)) => {
                    match handle.set_breakpoints(path, source_breakpoints).await {
                        Ok(_) => DapOutcome::unit(),
                        Err(e) => DapOutcome::err(format!("设置断点失败: {e}")),
                    }
                }
                None => DapOutcome::unit(),
            },
            "restart" => match handle.call::<dap::requests::Restart>(restart_args).await {
                Ok(_) => DapOutcome::unit(),
                Err(e) => DapOutcome::err(format!("restart 失败: {e}")),
            },
            _ => run_dap_request(&handle, &kind_for_job, &params_for_job).await,
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
            match handle.variables(reference as usize).await {
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
