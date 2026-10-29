use crate::editor::DapIntent;
use crate::editor::{Action, Breakpoint};
use crate::{align_view, Align, Editor};
use anyhow::bail;
use dap::requests::DisconnectArguments;
use dap::requests::ThreadsArguments;
use helix_core::Selection;
use helix_dap::{
    self as dap, registry::DebugAdapterId, Client, ConnectionType, Payload, Request, ThreadId,
};
use log::{error, warn};
use serde_json::{json, Value};
use std::fmt::Write;

#[macro_export]
macro_rules! debugger {
    ($editor:expr) => {{
        let Some(debugger) = $editor.debug_adapters.get_active_client_mut() else {
            return;
        };
        debugger
    }};
}

// general utils:
pub fn dap_pos_to_pos(doc: &helix_core::Rope, line: usize, column: usize) -> Option<usize> {
    // 1-indexing to 0 indexing
    let line = doc.try_line_to_char(line - 1).ok()?;
    let pos = line + column.saturating_sub(1);
    // TODO: this is probably utf-16 offsets
    Some(pos)
}

pub async fn select_thread_id(editor: &mut Editor, thread_id: ThreadId, force: bool) {
    let debugger = debugger!(editor);

    if !force && debugger.thread_id.is_some() {
        return;
    }

    debugger.thread_id = Some(thread_id);
    fetch_stack_trace(debugger, thread_id).await;

    let frame = debugger.stack_frames[&thread_id].first().cloned();
    if let Some(frame) = &frame {
        jump_to_stack_frame(editor, frame);
    }
}

pub async fn fetch_stack_trace(debugger: &mut Client, thread_id: ThreadId) {
    let (frames, _) = match debugger.stack_trace(thread_id).await {
        Ok(frames) => frames,
        Err(_) => return,
    };
    debugger.stack_frames.insert(thread_id, frames);
    debugger.active_frame = Some(0);
}

pub fn jump_to_stack_frame(editor: &mut Editor, frame: &helix_dap::StackFrame) {
    let path = if let Some(helix_dap::Source {
        path: Some(ref path),
        ..
    }) = frame.source
    {
        path.clone()
    } else {
        return;
    };

    if let Err(e) = editor.open(&path, Action::Replace) {
        editor.set_error(format!("Unable to jump to stack frame: {}", e));
        return;
    }

    let (view, doc) = current!(editor);
    // 记下来：会话结束时把"我们打开、且用户没改过"的文档关掉
    let opened_id = doc.id();
    if !editor.debug_docs_before.contains(&opened_id) {
        editor.debug_opened_docs.insert(opened_id);
    }

    let text_end = doc.text().len_chars().saturating_sub(1);
    let start = dap_pos_to_pos(doc.text(), frame.line, frame.column).unwrap_or(0);
    let end = frame
        .end_line
        .and_then(|end_line| dap_pos_to_pos(doc.text(), end_line, frame.end_column.unwrap_or(0)))
        .unwrap_or(start);

    let selection = Selection::single(start.min(text_end), end.min(text_end));
    doc.set_selection(view.id, selection);
    align_view(doc, view, Align::Center);
}

/// 断点能力检查：条件断点 / 命中条件 / 日志断点。
///
/// 抽出来是因为有**两条路径**都要查（命令的同步路径、`initialized` 的重放路径），
/// 而错误文案必须一致 —— 用户看到的该是"这个 adapter 不支持 X"，不是"设断点失败"。
pub fn check_breakpoint_caps(
    caps: Option<&dap::DebuggerCapabilities>,
    breakpoints: &[Breakpoint],
) -> Result<(), anyhow::Error> {
    let Some(caps) = caps else { return Ok(()) };
    if breakpoints.iter().any(|b| b.condition.is_some())
        && !caps.supports_conditional_breakpoints.unwrap_or_default()
    {
        bail!("Can't edit breakpoint: debugger does not support conditional breakpoints")
    }
    if breakpoints.iter().any(|b| b.hit_condition.is_some())
        && !caps
            .supports_hit_conditional_breakpoints
            .unwrap_or_default()
    {
        bail!("Can't edit breakpoint: debugger does not support hit conditional breakpoints")
    }
    if breakpoints.iter().any(|b| b.log_message.is_some())
        && !caps.supports_log_points.unwrap_or_default()
    {
        bail!("Can't edit breakpoint: debugger does not support logpoints")
    }
    Ok(())
}

/// 把某个文件的断点推给 adapter，并把响应回填进 `store`（**唯一**的推断点实现）。
///
/// 不并行推多个文件：回填要 `&mut store`，并行就得同时持有多份可变借用（不可能）。
/// 断点规模是几十条，"按文件串行"完全够，而且顺序稳定（`paths()` 是 BTreeMap 序）。
pub async fn sync_breakpoints(
    debugger: &dap::Client,
    store: &mut helix_dap::breakpoints::BreakpointStore,
    path: &std::path::Path,
) -> Result<(), anyhow::Error> {
    check_breakpoint_caps(debugger.caps.as_ref(), &store.for_path(path))?;
    let source_breakpoints = store.source_breakpoints(path);
    let response = debugger
        .set_breakpoints(path.to_path_buf(), source_breakpoints)
        .await?;
    if let Some(breakpoints) = response {
        store.apply_response(path, &breakpoints);
    }
    Ok(())
}

impl Editor {
    pub async fn handle_debugger_message(
        &mut self,
        id: DebugAdapterId,
        payload: helix_dap::Payload,
    ) -> bool {
        use helix_dap::{events, Event};

        let adapter_id = id;
        match payload {
            Payload::Event(event) => {
                let event = match Event::parse(&event.event, event.body) {
                    Ok(event) => event,
                    Err(dap::Error::Unhandled) => {
                        log::info!("Discarding unknown DAP event '{}'", event.event);
                        return false;
                    }
                    Err(err) => {
                        log::warn!("Discarding invalid DAP event '{}': {err}", event.event);
                        return false;
                    }
                };
                match event {
                    Event::Stopped(events::StoppedBody {
                        thread_id,
                        description,
                        text,
                        reason,
                        all_threads_stopped,
                        ..
                    }) => {
                        // "跑到光标"的临时断点：命中就摘掉（不留进用户的断点表）
                        // 注意：要在取 `&mut debugger` **之前**做 —— `self` 的其它字段
                        // 和 `debug_adapters` 的可变借用不能同时活着。
                        if let Some((path, row)) = self.debug_temp_breakpoint.take() {
                            self.breakpoints.remove(&path, row);
                            self.dap_intents
                                .push(DapIntent::SyncBreakpoints(path.clone()));
                            self.set_status(format!(
                                "已到光标：{}:{}（临时断点已移除）",
                                path.display(),
                                row + 1
                            ));
                        }
                        let debugger = match self.debug_adapters.get_client_mut(id) {
                            Some(debugger) => debugger,
                            None => return false,
                        };

                        let all_threads_stopped = all_threads_stopped.unwrap_or_default();

                        if all_threads_stopped {
                            if let Ok(response) = debugger
                                .request::<dap::requests::Threads>(Some(ThreadsArguments {}))
                                .await
                            {
                                for thread in response.threads {
                                    fetch_stack_trace(debugger, thread.id).await;
                                }
                            }
                        } else if let Some(tid) = thread_id {
                            debugger.thread_states.insert(tid, reason.clone()); // TODO: dap uses "type" || "reason"
                            fetch_stack_trace(debugger, tid).await;
                        }

                        // gdb 式：记下"停在哪"（这是借用 `debugger` 的**最后一处** —— 之后
                        // 才能 `select_thread_id(self, …)` 与移动光标）
                        let stop_point = debugger.current_stack_frame().and_then(|f| {
                            let path = f.source.as_ref()?.path.as_ref()?.clone();
                            Some((f.name.clone(), path, f.line))
                        });

                        let scope = match thread_id {
                            Some(id) => format!("Thread {}", id),
                            None => "Target".to_owned(),
                        };

                        // 借用结束：选线程（会动 self）
                        if all_threads_stopped {
                            select_thread_id(self, thread_id.unwrap_or_default(), false).await;
                        } else if let Some(tid) = thread_id {
                            select_thread_id(self, tid, false).await;
                        }

                        let text_for_event = text.clone();
                        let mut status = format!("{} stopped because of {}", scope, reason);
                        if let Some(desc) = description {
                            write!(status, " {}", desc).unwrap();
                        }
                        if let Some(text) = text {
                            write!(status, " {}", text).unwrap();
                        }
                        if all_threads_stopped {
                            status.push_str(" (all threads stopped)");
                        }

                        // 状态栏："函数 — Thread N  file:line"（比只报 reason 有用），
                        // 并把光标/视口跟到停止行 —— gdb 单步后你总看得见自己在哪
                        if let Some((name, path, row)) = stop_point {
                            status =
                                format!("{}  —  {}  {}:{}", name, scope, path.display(), row + 1);
                            let (view, doc) = crate::current!(self);
                            if doc.path().map(|p| p.to_path_buf()).as_deref()
                                == Some(path.as_path())
                            {
                                let text = doc.text().slice(..);
                                let last = text.len_lines().saturating_sub(1);
                                let line_start = text.line_to_char(row.min(last));
                                let view_id = view.id;
                                doc.set_selection(view_id, Selection::point(line_start));
                                align_view(doc, view, Align::Center);
                            }
                        }
                        self.set_status(status);
                        // 事件放在栈帧抓取**之后**（上面那段已经 await 完）：插件收到事件立刻
                        // `stack_frames()` 才拿得到新帧。
                        let public = self.debug_adapters.public_id(id).unwrap_or(0);
                        self.emit_dap_event(
                            "dap-stopped",
                            json!({
                                "id": public,
                                "reason": reason,
                                "thread_id": thread_id.map(|t| t.to_string().parse::<i64>().unwrap_or(0)),
                                "text": text_for_event,
                                "all_threads_stopped": all_threads_stopped,
                            }),
                        );
                    }
                    Event::Continued(events::ContinuedBody { thread_id, .. }) => {
                        let debugger = match self.debug_adapters.get_client_mut(id) {
                            Some(debugger) => debugger,
                            None => return false,
                        };

                        debugger
                            .thread_states
                            .insert(thread_id, "running".to_owned());
                        if debugger.thread_id == Some(thread_id) {
                            debugger.resume_application();
                        }
                        let public = self.debug_adapters.public_id(id).unwrap_or(0);
                        self.emit_dap_event(
                            "dap-continued",
                            json!({
                                "id": public,
                                "thread_id": thread_id.to_string().parse::<i64>().unwrap_or(0),
                            }),
                        );
                    }
                    Event::Thread(thread) => {
                        self.set_status(format!("Thread {}: {}", thread.thread_id, thread.reason));
                        let public = self.debug_adapters.public_id(id).unwrap_or(0);
                        self.emit_dap_event(
                            "dap-thread",
                            json!({
                                "id": public,
                                "reason": thread.reason,
                                "thread_id": thread.thread_id.to_string().parse::<i64>().unwrap_or(0),
                            }),
                        );
                        let debugger = match self.debug_adapters.get_client_mut(id) {
                            Some(debugger) => debugger,
                            None => return false,
                        };

                        debugger.thread_id = Some(thread.thread_id);
                        // set the stack frame for the thread
                    }
                    Event::Breakpoint(events::BreakpointBody { reason, breakpoint }) => {
                        match &reason[..] {
                            "new" => {
                                if let Some(source) = breakpoint.source {
                                    let Some(path) = source.path else {
                                        warn!("DAP breakpoint event missing source path");
                                        return false;
                                    };
                                    let Some(line) = breakpoint.line else {
                                        warn!("DAP breakpoint event missing line");
                                        return false;
                                    };
                                    let path_for_event = path.to_string_lossy().to_string();
                                    self.breakpoints.set(
                                        path,
                                        Breakpoint {
                                            id: breakpoint.id,
                                            verified: breakpoint.verified,
                                            message: breakpoint.message.clone(),
                                            line: line.saturating_sub(1),
                                            column: breakpoint.column,
                                            ..Default::default()
                                        },
                                    );
                                    if let Some(message) = &breakpoint.message {
                                        self.set_status(format!("Breakpoint: {}", message));
                                    }
                                    let public = self.debug_adapters.public_id(id).unwrap_or(0);
                                    self.emit_dap_event(
                                        "dap-breakpoint",
                                        json!({
                                            "id": public,
                                            "path": path_for_event,
                                            "line": line.saturating_sub(1),
                                            "verified": breakpoint.verified,
                                            "message": breakpoint.message,
                                        }),
                                    );
                                }
                            }
                            "changed" => {
                                let Some(id) = breakpoint.id else {
                                    warn!("DAP breakpoint event missing id");
                                    return false;
                                };
                                if let Some(source) = &breakpoint.source {
                                    if source.path.is_none() {
                                        warn!("DAP breakpoint event missing source path");
                                    }
                                }
                                if breakpoint.line.is_none() {
                                    warn!("DAP breakpoint event missing line");
                                }
                                // 按 id 回填 + 必要时按新行号重排（`apply_changed` 内部保证键值同步）
                                if self
                                    .breakpoints
                                    .apply_changed(
                                        id,
                                        breakpoint.verified,
                                        breakpoint.message.clone(),
                                        breakpoint.line.map(|l| l.saturating_sub(1)),
                                        breakpoint.column,
                                    )
                                    .is_none()
                                {
                                    warn!("DAP breakpoint {} not found in store", id);
                                }
                                if let Some(message) = &breakpoint.message {
                                    self.set_status(format!("Breakpoint: {}", message));
                                }
                            }
                            "removed" => {
                                let Some(id) = breakpoint.id else {
                                    warn!("DAP breakpoint event missing id");
                                    return false;
                                };
                                match self.breakpoints.remove_by_id(id) {
                                    Some((path, line)) => {
                                        let public =
                                            self.debug_adapters.public_id(adapter_id).unwrap_or(0);
                                        self.emit_dap_event(
                                            "dap-breakpoint",
                                            json!({
                                                "id": public,
                                                "path": path.to_string_lossy(),
                                                "line": line,
                                                "verified": false,
                                                "removed": true,
                                            }),
                                        );
                                    }
                                    None => warn!("DAP breakpoint {} not found in store", id),
                                }
                            }
                            reason => {
                                warn!("Unknown breakpoint event: {}", reason);
                            }
                        }
                    }
                    Event::Output(events::OutputBody {
                        category, output, ..
                    }) => {
                        let category_for_event = category.clone();
                        let prefix = match category {
                            Some(category) => {
                                if &category == "telemetry" {
                                    return false;
                                }
                                format!("Debug ({}):", category)
                            }
                            None => "Debug:".to_owned(),
                        };

                        log::info!("{}", output);
                        self.set_status(format!("{} {}", prefix, output));
                        let public = self.debug_adapters.public_id(id).unwrap_or(0);
                        self.emit_dap_event(
                            "dap-output",
                            json!({
                                "id": public,
                                "category": category_for_event,
                                "output": output,
                            }),
                        );
                    }
                    Event::ProgressStart(body) => {
                        let status = {
                            let debugger = match self.debug_adapters.get_client_mut(id) {
                                Some(debugger) => debugger,
                                None => return false,
                            };

                            debugger.progress_start(body.clone())
                        };

                        self.set_status(status);
                        let public = self.debug_adapters.public_id(id).unwrap_or(0);
                        self.emit_dap_event(
                            "dap-progress",
                            json!({
                                "id": public,
                                "kind": "start",
                                "text": body.title,
                                "percentage": body.percentage,
                            }),
                        );
                    }
                    Event::ProgressUpdate(body) => {
                        let status = {
                            let debugger = match self.debug_adapters.get_client_mut(id) {
                                Some(debugger) => debugger,
                                None => return false,
                            };

                            debugger.progress_update(body.clone())
                        };

                        if let Some(status) = status {
                            self.set_status(status);
                        }
                        let public = self.debug_adapters.public_id(id).unwrap_or(0);
                        self.emit_dap_event(
                            "dap-progress",
                            json!({
                                "id": public,
                                "kind": "update",
                                "text": body.message,
                                "percentage": body.percentage,
                            }),
                        );
                    }
                    Event::ProgressEnd(body) => {
                        let status = {
                            let debugger = match self.debug_adapters.get_client_mut(id) {
                                Some(debugger) => debugger,
                                None => return false,
                            };

                            debugger.progress_end(body.clone())
                        };

                        if let Some(status) = status {
                            self.set_status(status);
                        }
                        let public = self.debug_adapters.public_id(id).unwrap_or(0);
                        self.emit_dap_event(
                            "dap-progress",
                            json!({
                                "id": public,
                                "kind": "end",
                                "text": body.message,
                            }),
                        );
                    }
                    Event::Initialized(_) => {
                        self.set_status("Debugger initialized...");
                        // 把已有断点全量推给 adapter。`debug_adapters`（不可变借）与
                        // `breakpoints`（可变借）是**不同字段** ⇒ 同时借是合法的，不用先克隆客户端。
                        let paths = self.breakpoints.paths();
                        log::debug!("dap: initialized -> 重放 {} 个文件的断点", paths.len());
                        let mut failure = None;
                        for path in paths {
                            let Some(debugger) = self.debug_adapters.get_client(id) else {
                                break;
                            };
                            if let Err(err) =
                                sync_breakpoints(debugger, &mut self.breakpoints, &path).await
                            {
                                failure = Some(err);
                                break;
                            }
                        }
                        if let Some(err) = failure {
                            self.set_error(format!("Failed to set breakpoints: {}", err));
                        }

                        let Some(debugger) = self.debug_adapters.get_client(id) else {
                            return false;
                        };
                        if let Err(err) = debugger.configuration_done().await {
                            self.set_error(format!("Debugger configuration failed: {}", err));
                        } else {
                            self.set_status("Debugged application started");
                            // 会话此刻才算真正开始：记下**已经开着**的文档，
                            // 结束时只关"这次会话新打开的"（见 `close_debug_opened_docs`）
                            self.debug_docs_before = self.documents.keys().copied().collect();
                            self.debug_opened_docs.clear();
                        }

                        self.debug_adapters.set_active_client(id);
                    }
                    Event::Terminated(terminated) => {
                        // 跑到光标没命中就结束了 ⇒ 临时断点也要摘掉（别留在用户断点表里）
                        if let Some((path, row)) = self.debug_temp_breakpoint.take() {
                            self.breakpoints.remove(&path, row);
                            self.dap_intents.push(DapIntent::SyncBreakpoints(path));
                        }
                        // 先发事件再取可变借用：`public_id` 只读 registry，顺序反了会撞借用检查
                        let public = self.debug_adapters.public_id(adapter_id).unwrap_or(0);
                        let restart_for_event = terminated.as_ref().and_then(|t| t.restart.clone());
                        self.emit_dap_event(
                            "dap-terminated",
                            json!({ "id": public, "restart": restart_for_event }),
                        );
                        self.close_debug_opened_docs();
                        let debugger = match self.debug_adapters.get_client_mut(id) {
                            Some(debugger) => debugger,
                            None => return false,
                        };

                        let restart_arg = if let Some(terminated) = terminated {
                            terminated.restart
                        } else {
                            None
                        };

                        let restart_bool = restart_arg
                            .as_ref()
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let disconnect_args = Some(DisconnectArguments {
                            restart: Some(restart_bool),
                            terminate_debuggee: None,
                            suspend_debuggee: None,
                        });

                        if let Err(err) = debugger.disconnect(disconnect_args).await {
                            self.set_error(format!(
                                "Cannot disconnect debugger upon terminated event receival {:?}",
                                err
                            ));
                            return false;
                        }

                        match restart_arg {
                            Some(Value::Bool(false)) | None => {
                                self.debug_adapters.remove_client(id);
                                self.debug_adapters.unset_active_client();
                                self.set_status(
                                    "Terminated debugging session and disconnected debugger.",
                                );

                                // 全部标回"未确认"：UI 上 ● → ◯（会话断了，断点还在，但不再有效）
                                self.breakpoints.invalidate_all();
                            }
                            Some(val) => {
                                log::info!("Attempting to restart debug session.");
                                let connection_type = match debugger.connection_type() {
                                    Some(connection_type) => connection_type,
                                    None => {
                                        self.set_error("No starting request found, to be used in restarting the debugging session.");
                                        return false;
                                    }
                                };

                                let relaunch_resp = if let ConnectionType::Launch = connection_type
                                {
                                    debugger.launch(val).await
                                } else {
                                    debugger.attach(val).await
                                };

                                if let Err(err) = relaunch_resp {
                                    self.set_error(format!(
                                        "Failed to restart debugging session: {:?}",
                                        err
                                    ));
                                }
                            }
                        }
                    }
                    Event::Exited(resp) => {
                        if let Some((path, row)) = self.debug_temp_breakpoint.take() {
                            self.breakpoints.remove(&path, row);
                            self.dap_intents.push(DapIntent::SyncBreakpoints(path));
                        }
                        let exit_code = resp.exit_code;
                        let public = self.debug_adapters.public_id(id).unwrap_or(0);
                        self.emit_dap_event(
                            "dap-exited",
                            json!({ "id": public, "exit_code": exit_code }),
                        );
                        if exit_code != 0 {
                            self.set_error(format!(
                                "Debuggee failed to exit successfully (exit code: {exit_code})."
                            ));
                        }
                    }
                    ev => {
                        log::warn!("Unhandled event {:?}", ev);
                        return false; // return early to skip render
                    }
                }
            }
            Payload::Response(_) => unreachable!(),
            Payload::Request(request) => {
                let reply = match Request::parse(&request.command, request.arguments) {
                    Ok(Request::RunInTerminal(arguments)) => {
                        let config = self.config();
                        let Some(config) = config.terminal.as_ref() else {
                            self.set_error("No external terminal defined");
                            return true;
                        };

                        let process = match std::process::Command::new(&config.command)
                            .args(&config.args)
                            .args(&arguments.args)
                            .spawn()
                        {
                            Ok(process) => process,
                            Err(err) => {
                                self.set_error(format!(
                                    "Error starting external terminal: {}",
                                    err
                                ));
                                return true;
                            }
                        };

                        Ok(json!(dap::requests::RunInTerminalResponse {
                            process_id: Some(process.id()),
                            shell_process_id: None,
                        }))
                    }
                    Ok(Request::StartDebugging(arguments)) => {
                        let debugger = match self.debug_adapters.get_client_mut(id) {
                            Some(debugger) => debugger,
                            None => {
                                self.set_error("No active debugger found.");
                                return true;
                            }
                        };
                        // Currently we only support starting a child debugger if the parent is using the TCP transport
                        let socket = match debugger.socket {
                            Some(socket) => socket,
                            None => {
                                self.set_error("Child debugger can only be started if the parent debugger is using TCP transport.");
                                return true;
                            }
                        };

                        let config = match debugger.config.clone() {
                            Some(config) => config,
                            None => {
                                error!("No configuration found for the debugger.");
                                return true;
                            }
                        };

                        let result = self
                            .debug_adapters
                            .start_client(Some(socket), &config)
                            .await;

                        let client_id = match result {
                            Ok(child) => child,
                            Err(err) => {
                                self.set_error(format!(
                                    "Failed to create child debugger: {:?}",
                                    err
                                ));
                                return true;
                            }
                        };

                        let client = match self.debug_adapters.get_client_mut(client_id) {
                            Some(child) => child,
                            None => {
                                self.set_error("Failed to get child debugger.");
                                return true;
                            }
                        };

                        let relaunch_resp = if let ConnectionType::Launch = arguments.request {
                            client.launch(arguments.configuration).await
                        } else {
                            client.attach(arguments.configuration).await
                        };
                        if let Err(err) = relaunch_resp {
                            self.set_error(format!("Failed to start debugging session: {:?}", err));
                            return true;
                        }

                        Ok(json!({
                            "success": true,
                        }))
                    }
                    Err(err) => Err(err),
                };

                if let Some(debugger) = self.debug_adapters.get_client_mut(id) {
                    debugger
                        .reply(request.seq, &request.command, reply)
                        .await
                        .ok();
                }
            }
        }
        true
    }
}
