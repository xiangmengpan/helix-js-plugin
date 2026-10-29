use crate::{
    registry::DebugAdapterId,
    requests::{DisconnectArguments, TerminateArguments},
    transport::{Payload, Request, Response, Transport},
    Error, ProgressMap, ProgressState, Result,
};
use helix_core::syntax::config::{DebugAdapterConfig, DebuggerQuirks};
use helix_dap_types::*;

use serde_json::Value;

use anyhow::anyhow;
use std::{
    collections::HashMap,
    future::Future,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
    sync::Arc,
};
use tokio::{
    io::{AsyncBufRead, AsyncWrite, BufReader, BufWriter},
    net::TcpStream,
    process::{Child, Command},
    sync::mpsc::{channel, unbounded_channel, UnboundedReceiver, UnboundedSender},
    time,
};

#[derive(Debug)]
pub struct Client {
    id: DebugAdapterId,
    _process: Option<Child>,
    server_tx: UnboundedSender<Payload>,
    request_counter: Arc<AtomicU64>,
    connection_type: Option<ConnectionType>,
    starting_request_args: Option<Value>,
    /// 起会话时用的构建命令（`r` 重启时先重建，见 `remember_build`）
    starting_build: Option<String>,
    /// The socket address of the debugger, if using TCP transport.
    pub socket: Option<SocketAddr>,
    pub caps: Option<DebuggerCapabilities>,
    // thread_id -> frames
    pub stack_frames: HashMap<ThreadId, Vec<StackFrame>>,
    pub thread_states: ThreadStates,
    pub progress: ProgressMap,
    pub thread_id: Option<ThreadId>,
    /// Currently active frame for the current thread.
    pub active_frame: Option<usize>,
    pub quirks: DebuggerQuirks,
    /// The config which was used to start this debugger.
    pub config: Option<DebugAdapterConfig>,
}

impl Client {
    // Spawn a process and communicate with it by either TCP or stdio
    // The returned stream includes the Client ID so consumers can differentiate between multiple clients
    pub async fn process(
        transport: &str,
        command: &str,
        args: Vec<&str>,
        port_arg: Option<&str>,
        id: DebugAdapterId,
    ) -> Result<(Self, UnboundedReceiver<(DebugAdapterId, Payload)>)> {
        if command.is_empty() {
            // 说清"怎么修"，而不是只甩一句 Command not provided（真实现象：node 的
            // js-debug-dap 模板不带 command，用户按了 `:debug-start` 只看到这句，
            // 完全不知道要去 languages.toml 里配 argv）。
            return Result::Err(Error::Other(anyhow!(
                "这个语言的调试器没配 command：请在 languages.toml 的 `[language.debugger]` \
                 里补 `command` 和 `args`（例如 js-debug-dap 需要 `node` + dapDebugServer.js \
                 的路径），或在项目里用 `.helix/launch.toml` 显式指定；\
                 详见 docs/api/dap.md「语言模板 / 零配置」"
            )));
        }
        match (transport, port_arg) {
            ("tcp", Some(port_arg)) => Self::tcp_process(command, args, port_arg, id).await,
            ("stdio", _) => Self::stdio(command, args, id),
            _ => Result::Err(Error::Other(anyhow!("Incorrect transport {}", transport))),
        }
    }

    pub fn streams(
        rx: Box<dyn AsyncBufRead + Unpin + Send>,
        tx: Box<dyn AsyncWrite + Unpin + Send>,
        err: Option<Box<dyn AsyncBufRead + Unpin + Send>>,
        id: DebugAdapterId,
        process: Option<Child>,
    ) -> Result<(Self, UnboundedReceiver<(DebugAdapterId, Payload)>)> {
        let (server_rx, server_tx) = Transport::start(rx, tx, err, id);
        let (client_tx, client_rx) = unbounded_channel();

        let client = Self {
            id,
            _process: process,
            server_tx,
            request_counter: Arc::new(AtomicU64::new(0)),
            caps: None,
            connection_type: None,
            starting_request_args: None,
            starting_build: None,
            socket: None,
            stack_frames: HashMap::new(),
            thread_states: HashMap::new(),
            progress: HashMap::new(),
            thread_id: None,
            active_frame: None,
            quirks: DebuggerQuirks::default(),
            config: None,
        };

        tokio::spawn(Self::recv(id, server_rx, client_tx));

        Ok((client, client_rx))
    }

    pub async fn tcp(
        addr: std::net::SocketAddr,
        id: DebugAdapterId,
    ) -> Result<(Self, UnboundedReceiver<(DebugAdapterId, Payload)>)> {
        let stream = TcpStream::connect(addr).await?;
        let (rx, tx) = stream.into_split();
        Self::streams(Box::new(BufReader::new(rx)), Box::new(tx), None, id, None)
    }

    pub fn stdio(
        cmd: &str,
        args: Vec<&str>,
        id: DebugAdapterId,
    ) -> Result<(Self, UnboundedReceiver<(DebugAdapterId, Payload)>)> {
        // Resolve path to the binary
        let cmd = helix_stdx::env::which(cmd)?;

        let mut command = Command::new(cmd);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // make sure the process is reaped on drop
            .kill_on_drop(true);

        // **让 adapter 自成会话**（unix）。
        //
        // 为什么必须有:debugpy 会把被调试进程塞进 tty 的**前台进程组**
        // （`tcsetpgrp`）—— 之后我们（编辑器）变成后台进程组，读 tty 会拿 EIO/SIGTTIN
        // ⇒ 输入流彻底断掉（实测：python 命中后所有按键失效、CPU 空闲、DAP 日志正常）。
        // `setsid` 之后 adapter 没有控制终端，改不了我们的 tty。
        #[cfg(unix)]
        unsafe {
            command.pre_exec(|| {
                // 新会话 ⇒ 脱离控制终端（失败也不致命：至少不影响 DAP 通信）
                libc::setsid();
                Ok(())
            });
        }

        let process = command.spawn();

        let mut process = process?;

        // TODO: do we need bufreader/writer here? or do we use async wrappers on unblock?
        let writer = BufWriter::new(process.stdin.take().expect("Failed to open stdin"));
        let reader = BufReader::new(process.stdout.take().expect("Failed to open stdout"));
        let stderr = BufReader::new(process.stderr.take().expect("Failed to open stderr"));

        Self::streams(
            Box::new(reader),
            Box::new(writer),
            Some(Box::new(stderr)),
            id,
            Some(process),
        )
    }

    async fn get_port() -> Option<u16> {
        Some(
            tokio::net::TcpListener::bind(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                0,
            ))
            .await
            .ok()?
            .local_addr()
            .ok()?
            .port(),
        )
    }

    pub fn starting_request_args(&self) -> Option<&Value> {
        self.starting_request_args.as_ref()
    }

    /// 记录"这条会话是怎么起的"（客户端重启要原样重放）。
    ///
    /// `launch`/`attach` 走 `DapHandle::call` 时**不会**设置这两个字段（只有 `Client::launch`
    /// 那条自用路径会）——而我们的命令路径正是用 handle ⇒ 必须显式补记，否则 `r`（重启）
    /// 会误判成"不是本编辑器起的会话"。
    /// 记住"起这个会话时的构建命令"（`:debug-start` 的自动构建/显式 build）。
    /// `r` 重启时用它重建产物 —— 否则改了代码按 `r` 跑的还是旧二进制。
    pub fn remember_build(&mut self, build: Option<String>) {
        self.starting_build = build;
    }

    pub fn starting_build(&self) -> Option<String> {
        self.starting_build.clone()
    }

    pub fn remember_start(&mut self, request: &str, args: &Value) {
        self.starting_request_args = Some(args.clone());
        self.connection_type = Some(match request {
            "attach" => ConnectionType::Attach,
            _ => ConnectionType::Launch,
        });
    }

    pub async fn tcp_process(
        cmd: &str,
        args: Vec<&str>,
        port_format: &str,
        id: DebugAdapterId,
    ) -> Result<(Self, UnboundedReceiver<(DebugAdapterId, Payload)>)> {
        let port = Self::get_port().await.unwrap();

        let process = Command::new(cmd)
            .args(args)
            .args(port_format.replace("{}", &port.to_string()).split(' '))
            // silence messages
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // Do not kill debug adapter when leaving, it should exit automatically
            .spawn()?;

        // Wait for adapter to become ready for connection
        time::sleep(time::Duration::from_millis(500)).await;
        let socket = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), port);
        let stream = TcpStream::connect(socket).await?;

        let (rx, tx) = stream.into_split();
        let mut result = Self::streams(
            Box::new(BufReader::new(rx)),
            Box::new(tx),
            None,
            id,
            Some(process),
        );

        // Set the socket address for the client
        if let Ok((client, _)) = &mut result {
            client.socket = Some(socket);
        }

        result
    }

    async fn recv(
        id: DebugAdapterId,
        mut server_rx: UnboundedReceiver<Payload>,
        client_tx: UnboundedSender<(DebugAdapterId, Payload)>,
    ) {
        while let Some(msg) = server_rx.recv().await {
            match msg {
                Payload::Event(ev) => {
                    client_tx
                        .send((id, Payload::Event(ev)))
                        .expect("Failed to send");
                }
                Payload::Response(res) => {
                    // 防御:transport 已经不再转发"无主响应",这里跳过而不是 `unreachable!()`
                    // —— 前者丢一条消息,后者掉整条事件流。
                    log::warn!(
                        "Dropping orphan DAP response for request #{}",
                        res.request_seq
                    );
                }
                Payload::Request(req) => {
                    client_tx
                        .send((id, Payload::Request(req)))
                        .expect("Failed to send");
                }
            }
        }
    }

    pub fn id(&self) -> DebugAdapterId {
        self.id
    }

    pub fn connection_type(&self) -> Option<ConnectionType> {
        self.connection_type
    }

    fn next_request_id(&self) -> u64 {
        // > The `seq` for the first message sent by a client or debug adapter
        // > is 1, and for each subsequent message is 1 greater than the
        // > previous message sent by that actor
        // <https://microsoft.github.io/debug-adapter-protocol/specification#Base_Protocol_ProtocolMessage>
        self.request_counter.fetch_add(1, Ordering::Relaxed) + 1
    }

    // Internal, called by specific DAP commands when resuming
    pub fn resume_application(&mut self) {
        if let Some(thread_id) = self.thread_id {
            self.thread_states.insert(thread_id, "running".to_string());
            self.stack_frames.remove(&thread_id);
        }
        self.active_frame = None;
        self.thread_id = None;
    }

    /// 请求用的**廉价句柄**：`Clone` 一次 = 两个 `Arc`/`Rc` 克隆 + 一个 `Copy` 的 id。
    ///
    /// 为什么要它：命令层是**同步**的，而 `Client` 里带着子进程与可变会话状态，搬不进
    /// `jobs.callback` 的异步任务；句柄只带 `server_tx` + 序号计数器，`'static` 且 `Clone`，
    /// 于是"请求丢后台、结果回主线程"成为可能（`call` 的 20s 超时语义完全不变）。
    pub fn handle(&self) -> DapHandle {
        DapHandle {
            id: self.id,
            server_tx: self.server_tx.clone(),
            request_counter: Arc::clone(&self.request_counter),
        }
    }

    /// Execute a RPC request on the debugger.
    pub fn call<R: helix_dap_types::Request>(
        &self,
        arguments: R::Arguments,
    ) -> impl Future<Output = Result<Value>>
    where
        R::Arguments: serde::Serialize,
    {
        call_impl::<R>(self.server_tx.clone(), self.next_request_id(), arguments)
    }

    pub async fn request<R: helix_dap_types::Request>(
        &self,
        params: R::Arguments,
    ) -> Result<R::Result>
    where
        R::Arguments: serde::Serialize,
    {
        request_impl::<R>(self.server_tx.clone(), self.next_request_id(), params).await
    }

    pub fn reply(
        &self,
        request_seq: u64,
        command: &str,
        result: core::result::Result<Value, Error>,
    ) -> impl Future<Output = Result<()>> {
        reply_impl(
            self.server_tx.clone(),
            request_seq,
            command.to_string(),
            result,
        )
    }

    pub fn capabilities(&self) -> &DebuggerCapabilities {
        self.caps.as_ref().expect("debugger not yet initialized!")
    }

    pub fn progress_start(&mut self, event: events::ProgressStartBody) -> String {
        let status = ProgressState::new(event.title, event.message, event.percentage);
        let status_line = status.status_line();
        self.progress.insert(event.progress_id, status);
        status_line
    }

    pub fn progress_update(&mut self, event: events::ProgressUpdateBody) -> Option<String> {
        let status = self.progress.get_mut(&event.progress_id)?;
        status.update(event.message, event.percentage);
        Some(status.status_line())
    }

    pub fn progress_end(&mut self, event: events::ProgressEndBody) -> Option<String> {
        let status = self.progress.remove(&event.progress_id)?;
        Some(status.end_status_line(event.message.as_deref()))
    }

    pub async fn initialize(&mut self, adapter_id: String) -> Result<()> {
        let args = initialize_arguments(&adapter_id);
        let response = self.request::<requests::Initialize>(args).await?;
        self.caps = Some(response);

        Ok(())
    }

    pub fn disconnect(
        &mut self,
        args: Option<DisconnectArguments>,
    ) -> impl Future<Output = Result<Value>> {
        self.connection_type = None;
        self.call::<requests::Disconnect>(args)
    }

    pub fn terminate(
        &mut self,
        args: Option<TerminateArguments>,
    ) -> impl Future<Output = Result<Value>> {
        self.connection_type = None;
        self.call::<requests::Terminate>(args)
    }

    pub fn launch(&mut self, args: serde_json::Value) -> impl Future<Output = Result<Value>> {
        self.connection_type = Some(ConnectionType::Launch);
        self.starting_request_args = Some(args.clone());
        self.call::<requests::Launch>(args)
    }

    pub fn attach(&mut self, args: serde_json::Value) -> impl Future<Output = Result<Value>> {
        self.connection_type = Some(ConnectionType::Attach);
        self.starting_request_args = Some(args.clone());
        self.call::<requests::Attach>(args)
    }

    pub fn restart(&self) -> impl Future<Output = Result<Value>> {
        let args = if let Some(args) = &self.starting_request_args {
            args.clone()
        } else {
            Value::Null
        };
        self.call::<requests::Restart>(args)
    }

    pub async fn set_breakpoints(
        &self,
        file: PathBuf,
        breakpoints: Vec<SourceBreakpoint>,
    ) -> Result<Option<Vec<Breakpoint>>> {
        let args = requests::SetBreakpointsArguments {
            source: Source {
                path: Some(file),
                name: None,
                source_reference: None,
                presentation_hint: None,
                origin: None,
                sources: None,
                adapter_data: None,
                checksums: None,
            },
            breakpoints: Some(breakpoints),
            source_modified: Some(false),
        };

        let response = self.request::<requests::SetBreakpoints>(args).await?;

        Ok(response.breakpoints)
    }

    pub async fn configuration_done(&self) -> Result<()> {
        if !self
            .caps
            .as_ref()
            .and_then(|caps| caps.supports_configuration_done_request)
            .unwrap_or(false)
        {
            return Ok(());
        }

        self.call::<requests::ConfigurationDone>(Some(requests::ConfigurationDoneArguments {}))
            .await?;

        Ok(())
    }

    pub fn continue_thread(&self, thread_id: ThreadId) -> impl Future<Output = Result<Value>> {
        let args = requests::ContinueArguments { thread_id };

        self.call::<requests::Continue>(args)
    }

    pub async fn stack_trace(
        &self,
        thread_id: ThreadId,
    ) -> Result<(Vec<StackFrame>, Option<usize>)> {
        let args = requests::StackTraceArguments {
            thread_id,
            start_frame: None,
            levels: None,
            format: None,
        };

        let response = self.request::<requests::StackTrace>(args).await?;
        let mut frames = response.stack_frames;
        for frame in &mut frames {
            frame.name = clean_symbol(&frame.name);
        }
        Ok((frames, response.total_frames))
    }

    pub fn threads(&self) -> impl Future<Output = Result<Value>> {
        self.call::<requests::Threads>(Some(requests::ThreadsArguments {}))
    }

    pub async fn scopes(&self, frame_id: usize) -> Result<Vec<Scope>> {
        let args = requests::ScopesArguments { frame_id };

        let response = self.request::<requests::Scopes>(args).await?;
        Ok(response.scopes)
    }

    pub async fn variables(&self, variables_reference: usize) -> Result<Vec<Variable>> {
        self.variables_paged(variables_reference, None, None).await
    }

    /// 分页取变量（`start`/`count`）。巨大数组（`indexedVariables` 上万）靠它按需加载，
    /// 而不是一次全塞进树里。
    pub async fn variables_paged(
        &self,
        variables_reference: usize,
        start: Option<usize>,
        count: Option<usize>,
    ) -> Result<Vec<Variable>> {
        let args = requests::VariablesArguments {
            variables_reference,
            filter: None,
            start,
            count,
            format: None,
        };

        let response = self.request::<requests::Variables>(args).await?;
        Ok(response.variables)
    }

    pub fn step_in(&self, thread_id: ThreadId) -> impl Future<Output = Result<Value>> {
        let args = requests::StepInArguments {
            thread_id,
            target_id: None,
            granularity: None,
        };

        self.call::<requests::StepIn>(args)
    }

    pub fn step_out(&self, thread_id: ThreadId) -> impl Future<Output = Result<Value>> {
        let args = requests::StepOutArguments {
            thread_id,
            granularity: None,
        };

        self.call::<requests::StepOut>(args)
    }

    pub fn next(&self, thread_id: ThreadId) -> impl Future<Output = Result<Value>> {
        let args = requests::NextArguments {
            thread_id,
            granularity: None,
        };

        self.call::<requests::Next>(args)
    }

    pub fn pause(&self, thread_id: ThreadId) -> impl Future<Output = Result<Value>> {
        let args = requests::PauseArguments { thread_id };

        self.call::<requests::Pause>(args)
    }

    pub async fn eval(
        &self,
        expression: String,
        frame_id: Option<usize>,
    ) -> Result<requests::EvaluateResponse> {
        let args = requests::EvaluateArguments {
            expression,
            frame_id,
            context: None,
            format: None,
        };

        self.request::<requests::Evaluate>(args).await
    }

    pub fn set_exception_breakpoints(
        &self,
        filters: Vec<String>,
    ) -> impl Future<Output = Result<Value>> {
        let args = requests::SetExceptionBreakpointsArguments { filters };

        self.call::<requests::SetExceptionBreakpoints>(args)
    }

    /// 会话是否处于**暂停**（有活动栈帧）。gdb 式按键只在暂停时接管 —— 运行中按 `n`
    /// 应该是编辑器的"下一个匹配"，不是"单步"。
    pub fn is_paused(&self) -> bool {
        self.active_frame.is_some()
    }

    pub fn current_stack_frame(&self) -> Option<&StackFrame> {
        self.stack_frames
            .get(&self.thread_id?)?
            .get(self.active_frame?)
    }
}

/// `initialize` 请求的载荷（`Client::initialize` 与"后台起会话"路径**共用**这一份）。
///
/// 关键字段：`lines_start_at_one` / `columns_start_at_one` —— 它们决定断点行号能不能 +1，
/// 以及 `DapBreakpoint.line` 回填时要不要 -1（`BreakpointStore::apply_response`）。
/// 线上名是 `linesStartAt1`（DAP 规范），与 Rust 字段名不同。
pub fn initialize_arguments(adapter_id: &str) -> requests::InitializeArguments {
    requests::InitializeArguments {
        client_id: Some("hx".to_owned()),
        client_name: Some("helix".to_owned()),
        adapter_id: adapter_id.to_owned(),
        locale: Some("en-us".to_owned()),
        lines_start_at_one: Some(true),
        columns_start_at_one: Some(true),
        path_format: Some("path".to_owned()),
        supports_variable_type: Some(true),
        supports_variable_paging: Some(true),
        supports_run_in_terminal_request: Some(true),
        supports_memory_references: Some(false),
        supports_progress_reporting: Some(true),
        supports_invalidated_event: Some(false),
    }
}

/// 请求句柄（见 [`Client::handle`]）。
#[derive(Clone)]
pub struct DapHandle {
    id: DebugAdapterId,
    server_tx: UnboundedSender<Payload>,
    request_counter: Arc<AtomicU64>,
}

impl std::fmt::Debug for DapHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DapHandle").field("id", &self.id).finish()
    }
}

impl DapHandle {
    pub fn id(&self) -> DebugAdapterId {
        self.id
    }

    fn next_request_id(&self) -> u64 {
        self.request_counter.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// 与 [`Client::call`] 同一实现（同一个 `server_tx`，序号共享）。
    pub fn call<R: helix_dap_types::Request>(
        &self,
        arguments: R::Arguments,
    ) -> impl Future<Output = Result<Value>>
    where
        R::Arguments: serde::Serialize,
    {
        call_impl::<R>(self.server_tx.clone(), self.next_request_id(), arguments)
    }

    pub async fn request<R: helix_dap_types::Request>(
        &self,
        params: R::Arguments,
    ) -> Result<R::Result>
    where
        R::Arguments: serde::Serialize,
    {
        request_impl::<R>(self.server_tx.clone(), self.next_request_id(), params).await
    }

    /// 任意 DAP 请求（命令 + 原始参数）：`helix.dap.request` 的逃生口。
    ///
    /// 原理上就是 `call_impl` 的无类型版 —— 新能力不必每次改核心。代价是对 adapter 乱回
    /// 零容错，所以文档里标注为**进阶用法**。
    pub fn call_raw(
        &self,
        command: &str,
        arguments: Option<Value>,
    ) -> impl Future<Output = Result<Value>> {
        call_raw_impl(
            self.server_tx.clone(),
            self.next_request_id(),
            command.to_string(),
            arguments,
        )
    }

    /// 只把请求塞进发送队列、**不等回包**（fire-and-forget，非阻塞）。
    ///
    /// 用途：`pause` / `continue` 这类"结果由 `stopped`/`continued` 事件告知"的请求 ——
    /// 以前为了拿一个没人看的响应，在 UI 线程上 `block_on` 等 adapter 回包。
    /// 回包会被 transport 当作"无主响应"丢掉（见 `transport.rs`）。
    pub fn send_only<R: helix_dap_types::Request>(&self, arguments: R::Arguments) -> Result<()>
    where
        R::Arguments: serde::Serialize,
    {
        let req = Request {
            back_ch: None,
            seq: self.next_request_id(),
            command: R::COMMAND.to_string(),
            arguments: Some(serde_json::to_value(arguments)?),
        };
        self.server_tx
            .send(Payload::Request(req))
            .map_err(|e| Error::Other(e.into()))
    }

    /// 取作用域（后台任务用句柄版；与 [`Client::scopes`] 同一载荷）。
    pub async fn scopes(&self, frame_id: usize) -> Result<Vec<Scope>> {
        let response = self
            .request::<requests::Scopes>(requests::ScopesArguments { frame_id })
            .await?;
        Ok(response.scopes)
    }

    /// 取变量（后台任务用句柄版）。
    pub async fn variables(&self, variables_reference: usize) -> Result<Vec<Variable>> {
        self.variables_paged(variables_reference, None, None).await
    }

    /// 分页取变量（`start`/`count`）——巨大数组按需加载。
    pub async fn variables_paged(
        &self,
        variables_reference: usize,
        start: Option<usize>,
        count: Option<usize>,
    ) -> Result<Vec<Variable>> {
        let args = requests::VariablesArguments {
            variables_reference,
            filter: None,
            start,
            count,
            format: None,
        };
        let response = self.request::<requests::Variables>(args).await?;
        Ok(response.variables)
    }

    /// 取线程栈帧（后台任务用句柄版）。
    pub async fn stack_trace(
        &self,
        thread_id: ThreadId,
    ) -> Result<(Vec<StackFrame>, Option<usize>)> {
        let args = requests::StackTraceArguments {
            thread_id,
            start_frame: None,
            levels: None,
            format: None,
        };
        let response = self.request::<requests::StackTrace>(args).await?;
        let mut frames = response.stack_frames;
        for frame in &mut frames {
            frame.name = clean_symbol(&frame.name);
        }
        Ok((frames, response.total_frames))
    }

    /// 设断点（与 [`Client::set_breakpoints`] 同一载荷；后台任务用句柄版）。
    pub async fn set_breakpoints(
        &self,
        file: PathBuf,
        breakpoints: Vec<SourceBreakpoint>,
    ) -> Result<Option<Vec<Breakpoint>>> {
        let args = requests::SetBreakpointsArguments {
            source: Source {
                path: Some(file),
                name: None,
                source_reference: None,
                presentation_hint: None,
                origin: None,
                sources: None,
                adapter_data: None,
                checksums: None,
            },
            breakpoints: Some(breakpoints),
            source_modified: Some(false),
        };

        let response = self.request::<requests::SetBreakpoints>(args).await?;

        Ok(response.breakpoints)
    }

    /// 回一个反向请求（`runInTerminal` 之类）。
    pub fn reply(
        &self,
        request_seq: u64,
        command: &str,
        result: core::result::Result<Value, Error>,
    ) -> impl Future<Output = Result<()>> {
        reply_impl(
            self.server_tx.clone(),
            request_seq,
            command.to_string(),
            result,
        )
    }
}

/// `Client::call` / `DapHandle::call` 的**唯一实现**。
///
/// 只吃自有的值（`server_tx`/`seq`）⇒ 返回的 future 不借用 `self`，因此可以 `'static` 地
/// 丢进 `jobs.callback`（命令层去 `block_on` 的关键；见 `helix-term/src/commands/dap.rs`）。
async fn call_impl<R: helix_dap_types::Request>(
    server_tx: UnboundedSender<Payload>,
    id: u64,
    arguments: R::Arguments,
) -> Result<Value>
where
    R::Arguments: serde::Serialize,
{
    {
        use std::time::Duration;
        use tokio::time::timeout;

        let arguments = Some(serde_json::to_value(arguments)?);

        // 协议面可视化：`-vv` 就能看到每条请求/响应。排查过「断点没生效」这类问题时，
        // 没有这个只能靠抓包——而 DAP 走 stdio，抓包成本很高。
        log::debug!("dap -> {} (#{id}) {:?}", R::COMMAND, arguments);
        // `:debug-log` 用：同样一份流水留最近 500 条在内存里（`-vv` 才落盘，来不及看）
        crate::transcript::record(
            "->",
            R::COMMAND,
            id,
            &arguments
                .as_ref()
                .map(|a| a.to_string())
                .unwrap_or_default(),
        );

        let (callback_tx, mut callback_rx) = channel(1);

        let req = Request {
            back_ch: Some(callback_tx),
            seq: id,
            command: R::COMMAND.to_string(),
            arguments,
        };

        server_tx
            .send(Payload::Request(req))
            .map_err(|e| Error::Other(e.into()))?;

        // TODO: specifiable timeout, delay other calls until initialize success
        let response = timeout(Duration::from_secs(20), callback_rx.recv())
            .await
            .map_err(|_| Error::Timeout(id))? // return Timeout
            .ok_or(Error::StreamClosed)??;

        if !response.success {
            let message = response
                .message
                .clone()
                .unwrap_or_else(|| "DAP request failed".to_string());
            log::debug!("dap <- {} (#{id}) 失败: {message}", R::COMMAND);
            crate::transcript::record("!!", R::COMMAND, id, &message);
            return Err(Error::Other(anyhow!(message)));
        }

        log::debug!("dap <- {} (#{id}) ok {:?}", R::COMMAND, response.body);
        crate::transcript::record(
            "<-",
            R::COMMAND,
            id,
            &response
                .body
                .as_ref()
                .map(|b| b.to_string())
                .unwrap_or_default(),
        );
        Ok(response.body.unwrap_or_default())
    }
}

async fn request_impl<R: helix_dap_types::Request>(
    server_tx: UnboundedSender<Payload>,
    id: u64,
    params: R::Arguments,
) -> Result<R::Result>
where
    R::Arguments: serde::Serialize,
{
    let json = call_impl::<R>(server_tx, id, params).await?;
    let response = serde_json::from_value(json)?;
    Ok(response)
}

async fn reply_impl(
    server_tx: UnboundedSender<Payload>,
    request_seq: u64,
    command: String,
    result: core::result::Result<Value, Error>,
) -> Result<()> {
    {
        let response = match result {
            Ok(result) => Response {
                request_seq,
                command,
                success: true,
                message: None,
                body: Some(result),
            },
            Err(error) => Response {
                request_seq,
                command,
                success: false,
                message: Some(error.to_string()),
                body: None,
            },
        };

        server_tx
            .send(Payload::Response(response))
            .map_err(|e| Error::Other(e.into()))?;

        Ok(())
    }
}

/// `DapHandle::call_raw` 的实现（无类型请求；与 `call_impl` 同一套回包/超时语义）。
async fn call_raw_impl(
    server_tx: UnboundedSender<Payload>,
    id: u64,
    command: String,
    arguments: Option<Value>,
) -> Result<Value> {
    use std::time::Duration;
    use tokio::time::timeout;

    let (callback_tx, mut callback_rx) = channel(1);
    let req = Request {
        back_ch: Some(callback_tx),
        seq: id,
        command,
        arguments,
    };
    server_tx
        .send(Payload::Request(req))
        .map_err(|e| Error::Other(e.into()))?;

    let response = timeout(Duration::from_secs(20), callback_rx.recv())
        .await
        .map_err(|_| Error::Timeout(id))?
        .ok_or(Error::StreamClosed)??;

    if !response.success {
        let message = response
            .message
            .clone()
            .unwrap_or_else(|| "DAP request failed".to_string());
        return Err(Error::Other(anyhow!(message)));
    }

    Ok(response.body.unwrap_or_default())
}

/// 把调试器给的符号名清理成**人能读**的样子。
///
/// 三件事：
/// 1. 去掉 lldb 加的修饰：`[inlined] ` 前缀、` at file.rs:12:3` 尾巴；
/// 2. Rust：`_RNv…`（v0）/ `_ZN…`（legacy）用 `rustc-demangle` 解，`{:#}` 还会去掉
///    `::h1a2b3c…` 那串 hash（lldb 自己只解得动 legacy，v0 原样返回 —— 这就是
///    "Rust 栈帧名很怪"的根因）；
/// 3. C/C++ 的动态符号版本尾巴 `@@GLIBC_2.34` / 单 `@` 版本也去掉。
///
/// 解不出来就**原样返回**（`objc_msgSend`、`??`、`0x1234` 之类不该被我们猜）。
pub fn clean_symbol(raw: &str) -> String {
    let mut s = raw.trim();
    if let Some(rest) = s.strip_prefix("[inlined] ") {
        s = rest;
    }
    // "foo at src/main.rs:12:3" → "foo"
    if let Some(idx) = s.find(" at ") {
        s = &s[..idx];
    }
    // 符号版本尾巴：`@@GLIBC_2.34`（默认版本）或 `@GLIBC_2.2.5`（单 @ 版本）
    let version = s.find("@@").or_else(|| {
        // 单 `@`：只在后面看起来像版本号时才切（别把 `foo@bar` 这种名字砍掉）
        s.match_indices('@')
            .find(|(i, _)| {
                s.get(i + 1..)
                    .is_some_and(|rest| rest.chars().next().is_some_and(|c| c.is_ascii_uppercase()))
            })
            .map(|(i, _)| i)
    });
    if let Some(idx) = version {
        if idx > 0 {
            s = &s[..idx];
        }
    }
    let demangled = if s.starts_with("_R") || s.starts_with("_ZN") {
        rustc_demangle::try_demangle(s)
            .ok()
            .map(|d| format!("{d:#}"))
    } else {
        None
    };
    demangled.unwrap_or_else(|| s.to_string())
}

#[cfg(test)]
mod clean_symbol_tests {
    use super::clean_symbol;

    #[test]
    fn demangles_rust_v0_and_drops_hash() {
        assert_eq!(clean_symbol("_RNvCs1TQt3UJmI7b_5rsdbg4main"), "rsdbg::main");
        // 带 hash 的 legacy 名字：`{:#}` 会把 `::h1234…` 去掉
        let legacy = "_ZN5rsdbg3add17h1234567890abcdefE";
        assert_eq!(clean_symbol(legacy), "rsdbg::add");
    }

    #[test]
    fn strips_lldb_decorations() {
        assert_eq!(clean_symbol("[inlined] foo at src/main.rs:12:3"), "foo");
        assert_eq!(clean_symbol("main at main.c:5"), "main");
    }

    #[test]
    fn strips_symbol_version_suffix() {
        assert_eq!(
            clean_symbol("__libc_start_main@@GLIBC_2.34"),
            "__libc_start_main"
        );
        assert_eq!(clean_symbol("foo@GLIBC_2.2.5"), "foo");
    }

    #[test]
    fn leaves_unknown_names_alone() {
        assert_eq!(clean_symbol("objc_msgSend"), "objc_msgSend");
        assert_eq!(clean_symbol("??"), "??");
        assert_eq!(clean_symbol("_RNvBROKEN"), "_RNvBROKEN");
    }
}
