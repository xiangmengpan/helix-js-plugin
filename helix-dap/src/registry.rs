use crate::{Client, Payload, Result, StackFrame};
use helix_core::syntax::config::DebugAdapterConfig;
use slotmap::SlotMap;
use std::fmt;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_stream::wrappers::UnboundedReceiverStream;

/// The registry manages and owns multiple debugger clients.
///
/// **多会话**：`inner` 是 `SlotMap<DebugAdapterId, Option<Client>>` —— `Option` 存在的理由是
/// **id 必须在 spawn 之前就确定**（事件流按 id 打标），而 `Client` 要 `await` 完 spawn +
/// `initialize` 才能构造出来。所以流程是：`reserve()` 占坑 → `spawn_client()`（async，不借
/// registry）→ `insert_client()` 填坑。
///
/// 旧的实现把 spawn+initialize 用 `futures_executor::block_on` 塞在同步路径上，等于在 UI 线程
/// 等 adapter 启进程 —— 那条路已经删掉（见 `docs/superpowers/plans/2026-10-25-dap-redesign.md`）。
pub struct Registry {
    inner: SlotMap<DebugAdapterId, Option<Client>>,
    /// 对外的**稳定数字 id**（`helix.dap.sessions()` 的 `id`）：slotmap 的键不能跨进程/跨槽
    /// 传递，而 JS 侧要一个能写进快照、再传回来的整数。`reserve()` 时分配，永不复用。
    public_ids: std::collections::HashMap<DebugAdapterId, u64>,
    next_public_id: u64,
    /// 命令默认操作的会话（`dap_launch` 新起的会话会成为它）
    current_client_id: Option<DebugAdapterId>,
    /// A stream of incoming messages from all debuggers
    pub incoming:
        futures_util::stream::SelectAll<UnboundedReceiverStream<(DebugAdapterId, Payload)>>,
}

impl Registry {
    pub fn new() -> Self {
        Self {
            inner: SlotMap::with_key(),
            public_ids: std::collections::HashMap::new(),
            next_public_id: 1,
            current_client_id: None,
            incoming: futures_util::stream::SelectAll::new(),
        }
    }

    /// 占一个 id。**spawn 之前**就要拿到它（事件流按 id 打标）。
    pub fn reserve(&mut self) -> DebugAdapterId {
        let id = self.inner.insert(None);
        let public = self.next_public_id;
        self.next_public_id += 1;
        self.public_ids.insert(id, public);
        id
    }

    /// 对外数字 id（`sessions()` 快照里的 `id`）。
    pub fn public_id(&self, id: DebugAdapterId) -> Option<u64> {
        self.public_ids.get(&id).copied()
    }

    /// 按对外数字 id 找会话（JS 侧把 `id` 传回来时用）。
    pub fn by_public_id(&self, public: u64) -> Option<DebugAdapterId> {
        self.public_ids
            .iter()
            .find(|(_, p)| **p == public)
            .map(|(id, _)| *id)
    }

    /// 已就绪会话的 (对外 id, 内部 id)，按对外 id 升序（= 创建顺序）。
    pub fn session_list(&self) -> Vec<(u64, DebugAdapterId)> {
        let mut out: Vec<(u64, DebugAdapterId)> = self
            .session_ids()
            .into_iter()
            .filter_map(|id| self.public_id(id).map(|p| (p, id)))
            .collect();
        out.sort_by_key(|(p, _)| *p);
        out
    }

    /// 只起传输（**不** initialize）：**不碰 `&mut self`**，可在异步上下文里 `await` 而不借住 registry。
    ///
    /// 给"命令层去阻塞"的路径用：命令先 `reserve()` 拿 id，`jobs.callback` 里 `spawn_raw().await`
    /// 再用 [`Client::handle`] 自己发 `initialize`（响应走句柄的回包通道，不经过 registry），
    /// 最后由回调把客户端插进 registry —— 这样 UI 线程一次都不等 adapter。
    pub async fn spawn_raw(
        id: DebugAdapterId,
        socket: Option<std::net::SocketAddr>,
        config: &DebugAdapterConfig,
    ) -> Result<(Client, UnboundedReceiver<(DebugAdapterId, Payload)>)> {
        match socket {
            Some(socket) => Client::tcp(socket, id).await,
            None => {
                Client::process(
                    &config.transport,
                    &config.command,
                    config.args.iter().map(|arg| arg.as_str()).collect(),
                    config.port_arg.as_deref(),
                    id,
                )
                .await
            }
        }
    }

    /// `spawn_raw` + `initialize`（给**已经在异步上下文**的调用方：`StartDebugging` 事件）。
    pub async fn spawn_client(
        id: DebugAdapterId,
        socket: Option<std::net::SocketAddr>,
        config: &DebugAdapterConfig,
    ) -> Result<(Client, UnboundedReceiver<(DebugAdapterId, Payload)>)> {
        let (mut client, receiver) = Self::spawn_raw(id, socket, config).await?;
        client.initialize(config.name.clone()).await?;
        Ok((client, receiver))
    }

    /// 把起好的客户端放进预留的槽：接上事件流、写回 config/quirks、必要时设为当前会话。
    pub fn insert_client(
        &mut self,
        id: DebugAdapterId,
        mut client: Client,
        receiver: UnboundedReceiver<(DebugAdapterId, Payload)>,
        config: &DebugAdapterConfig,
        make_current: bool,
    ) -> Result<DebugAdapterId> {
        client.config = Some(config.clone());
        client.quirks = config.quirks.clone();
        let Some(slot) = self.inner.get_mut(id) else {
            // 预留的槽被移掉了（并发关闭）⇒ 别把事件流挂上去
            return Result::Err(crate::Error::Other(anyhow::anyhow!(
                "debug adapter slot {id} disappeared"
            )));
        };
        *slot = Some(client);
        self.incoming.push(UnboundedReceiverStream::new(receiver));
        if make_current || self.current_client_id.is_none() {
            self.current_client_id = Some(id);
        }
        Ok(id)
    }

    /// 预留 + spawn + 插入。给**已经在异步上下文**的调用方用（`StartDebugging` 事件）。
    ///
    /// 同步命令（`:debug-launch`）不用它：那条路是 `reserve()` → `jobs.callback` 里
    /// `spawn_client().await` → 回调里 `insert_client()`，这样 UI 线程一次都不阻塞。
    pub async fn start_client(
        &mut self,
        socket: Option<std::net::SocketAddr>,
        config: &DebugAdapterConfig,
    ) -> Result<DebugAdapterId> {
        let id = self.reserve();
        match Self::spawn_client(id, socket, config).await {
            Ok((client, receiver)) => self.insert_client(id, client, receiver, config, true),
            Err(err) => {
                self.inner.remove(id);
                Result::Err(err)
            }
        }
    }

    pub fn remove_client(&mut self, id: DebugAdapterId) {
        self.inner.remove(id);
        self.public_ids.remove(&id);
        if self.current_client_id == Some(id) {
            self.current_client_id = None;
        }
    }

    pub fn get_client(&self, id: DebugAdapterId) -> Option<&Client> {
        self.inner.get(id)?.as_ref()
    }

    pub fn get_client_mut(&mut self, id: DebugAdapterId) -> Option<&mut Client> {
        self.inner.get_mut(id)?.as_mut()
    }

    pub fn get_active_client(&self) -> Option<&Client> {
        self.current_client_id.and_then(|id| self.get_client(id))
    }

    pub fn get_active_client_mut(&mut self) -> Option<&mut Client> {
        self.current_client_id
            .and_then(|id| self.get_client_mut(id))
    }

    pub fn set_active_client(&mut self, id: DebugAdapterId) {
        // 槽里可能还是 `None`（spawn 中）：只认已经填好的会话
        if self.get_client(id).is_some() {
            self.current_client_id = Some(id);
        } else {
            self.current_client_id = None;
        }
    }

    pub fn unset_active_client(&mut self) {
        self.current_client_id = None;
    }

    /// 所有**已就绪**的会话 id（`None` 的预留槽不算），按创建顺序。
    pub fn session_ids(&self) -> Vec<DebugAdapterId> {
        self.inner
            .iter()
            .filter_map(|(id, client)| client.as_ref().map(|_| id))
            .collect()
    }

    pub fn active_id(&self) -> Option<DebugAdapterId> {
        self.current_client_id
    }

    pub fn current_stack_frame(&self) -> Option<&StackFrame> {
        self.get_active_client()
            .and_then(|debugger| debugger.current_stack_frame())
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

slotmap::new_key_type! {
    pub struct DebugAdapterId;
}

impl fmt::Display for DebugAdapterId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}
