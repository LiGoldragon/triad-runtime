//! Async task-backed runtime substrate for generated triad daemons.
//!
//! This module is the replacement direction for the old synchronous listener
//! and thread-worker shell. It intentionally starts with the load-bearing
//! primitive every async daemon needs first: request admission that applies
//! backpressure without blocking an actor mailbox.

use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::future::Future;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kameo::actor::{Actor, ActorRef, Spawn};
use kameo::error::Infallible;
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use thiserror::Error;
use tokio::net::{UnixListener as TokioUnixListener, UnixStream as TokioUnixStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::{ConnectionContext, RequestErrorLog, SocketMode};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestConcurrencyLimit {
    count: usize,
}

#[derive(Clone)]
pub struct RequestPermitPool {
    limit: RequestConcurrencyLimit,
    semaphore: Arc<Semaphore>,
}

pub struct RequestPermit {
    limit: RequestConcurrencyLimit,
    _permit: OwnedSemaphorePermit,
}

pub struct AcceptedConnection {
    stream: TokioUnixStream,
    context: ConnectionContext,
    _permit: RequestPermit,
}

#[derive(Debug, Error)]
pub enum RequestPermitError {
    #[error("request gate is closed")]
    GateClosed,
}

#[derive(Debug, Error)]
pub enum AsyncListenerError {
    #[error("async listener IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("request gate error: {detail}")]
    RequestGate { detail: String },

    #[error("async listener index {index} is out of range for {listener_count} listener(s)")]
    ListenerIndexOutOfRange { index: usize, listener_count: usize },

    #[error("async listener task error: {detail}")]
    ListenerTask { detail: String },
}

#[derive(Debug)]
pub enum AsyncSingleListenerDaemonError<RuntimeError> {
    Listener(AsyncListenerError),
    Start(RuntimeError),
    Stop(RuntimeError),
}

#[derive(Debug)]
pub enum AsyncMultiListenerDaemonError<RuntimeError> {
    Listener(AsyncListenerError),
    Start(RuntimeError),
    Stop(RuntimeError),
}

#[derive(Debug)]
pub struct RequestGate {
    pool: RequestPermitPool,
    accepted_request_count: u64,
}

pub struct AcquireRequestPermit {
    request_name: String,
    #[cfg(test)]
    admission_observer: Option<RequestAdmissionObserver>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestGateStatusRequest {
    observer_name: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, kameo::Reply)]
pub struct RequestGateStatus {
    limit: RequestConcurrencyLimit,
    available_permit_count: usize,
    accepted_request_count: u64,
}

pub trait AsyncConnectionRuntime: Send + Sync + 'static {
    type Error: Display + Send + Sync + 'static;

    fn start(&self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async { Ok(()) }
    }

    fn stop(&self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async { Ok(()) }
    }

    fn handle_connection(
        &self,
        connection: AcceptedConnection,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

pub trait AsyncMultiConnectionRuntime: Send + Sync + 'static {
    type Listener: Clone + Display + Send + Sync + 'static;
    type Error: Display + Send + Sync + 'static;

    fn start(&self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async { Ok(()) }
    }

    fn stop(&self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async { Ok(()) }
    }

    fn handle_connection(
        &self,
        listener: Self::Listener,
        connection: AcceptedConnection,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

pub struct AsyncSingleListenerDaemon<Runtime> {
    socket_path: PathBuf,
    socket_mode: Option<SocketMode>,
    concurrency_limit: RequestConcurrencyLimit,
    request_error_log: RequestErrorLog,
    runtime: Arc<Runtime>,
}

pub struct BoundAsyncSingleListenerDaemon<Runtime> {
    _socket_file: AsyncSocketFile,
    listener: TokioUnixListener,
    request_gate: ActorRef<RequestGate>,
    request_error_log: RequestErrorLog,
    runtime: Arc<Runtime>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AsyncListenerSocket<Listener> {
    listener: Listener,
    socket_path: PathBuf,
    socket_mode: Option<SocketMode>,
}

pub struct AsyncMultiListenerDaemon<Runtime>
where
    Runtime: AsyncMultiConnectionRuntime,
{
    listener_sockets: Vec<AsyncListenerSocket<Runtime::Listener>>,
    concurrency_limit: RequestConcurrencyLimit,
    request_error_log: RequestErrorLog,
    runtime: Arc<Runtime>,
}

pub struct BoundAsyncMultiListenerDaemon<Runtime>
where
    Runtime: AsyncMultiConnectionRuntime,
{
    listeners: Vec<BoundAsyncListener<Runtime::Listener>>,
    request_error_log: RequestErrorLog,
    runtime: Arc<Runtime>,
}

struct StoppedAsyncMultiListenerDaemon<Runtime> {
    request_gates: Vec<ActorRef<RequestGate>>,
    _request_error_log: RequestErrorLog,
    runtime: Arc<Runtime>,
}

struct AsyncSocketPath<'path> {
    path: &'path Path,
    socket_mode: Option<SocketMode>,
}

struct AsyncSocketFile {
    path: PathBuf,
}

struct BoundAsyncListener<Listener> {
    listener: Listener,
    _socket_file: AsyncSocketFile,
    unix_listener: TokioUnixListener,
    request_gate: ActorRef<RequestGate>,
}

struct AsyncListenerTask<Listener, Runtime>
where
    Runtime: AsyncMultiConnectionRuntime<Listener = Listener>,
{
    listener: BoundAsyncListener<Listener>,
    request_error_log: RequestErrorLog,
    runtime: Arc<Runtime>,
}

#[cfg(test)]
struct RequestAdmissionObserver {
    sender: Option<tokio::sync::oneshot::Sender<RequestAdmission>>,
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
struct RequestAdmission {
    request_name: String,
    accepted_request_count: u64,
}

impl RequestConcurrencyLimit {
    pub const fn new(count: usize) -> Self {
        if count == 0 {
            Self::one()
        } else {
            Self { count }
        }
    }

    pub const fn one() -> Self {
        Self::new(1)
    }

    pub fn count(self) -> usize {
        self.count
    }
}

impl Debug for RequestPermitPool {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RequestPermitPool")
            .field("limit", &self.limit)
            .field("available_permit_count", &self.available_permit_count())
            .finish()
    }
}

impl RequestPermitPool {
    pub fn new(limit: RequestConcurrencyLimit) -> Self {
        Self {
            limit,
            semaphore: Arc::new(Semaphore::new(limit.count())),
        }
    }

    pub fn limit(&self) -> RequestConcurrencyLimit {
        self.limit
    }

    pub fn available_permit_count(&self) -> usize {
        self.semaphore.available_permits()
    }

    pub async fn acquire(&self) -> Result<RequestPermit, RequestPermitError> {
        self.semaphore
            .clone()
            .acquire_owned()
            .await
            .map(|permit| RequestPermit::new(self.limit, permit))
            .map_err(|_| RequestPermitError::GateClosed)
    }
}

impl Debug for RequestPermit {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RequestPermit")
            .field("limit", &self.limit)
            .finish_non_exhaustive()
    }
}

impl RequestPermit {
    fn new(limit: RequestConcurrencyLimit, permit: OwnedSemaphorePermit) -> Self {
        Self {
            limit,
            _permit: permit,
        }
    }

    pub fn limit(&self) -> RequestConcurrencyLimit {
        self.limit
    }
}

impl AcceptedConnection {
    fn new(stream: TokioUnixStream, context: ConnectionContext, permit: RequestPermit) -> Self {
        Self {
            stream,
            context,
            _permit: permit,
        }
    }

    pub fn context(&self) -> &ConnectionContext {
        &self.context
    }

    pub fn stream_mut(&mut self) -> &mut TokioUnixStream {
        &mut self.stream
    }

    /// Consume the accepted connection into its Tokio stream and peer context.
    ///
    /// Stream-aware generated daemons use this to split the stream, keep an
    /// owned writer half for subscription events, and still classify the first
    /// request by the kernel-vouched peer credentials.
    pub fn into_parts(self) -> (TokioUnixStream, ConnectionContext) {
        (self.stream, self.context)
    }
}

impl RequestGate {
    pub fn new(limit: RequestConcurrencyLimit) -> Self {
        Self {
            pool: RequestPermitPool::new(limit),
            accepted_request_count: 0,
        }
    }

    pub async fn start(self) -> ActorRef<Self> {
        let actor = Self::spawn(self);
        actor.wait_for_startup().await;
        actor
    }

    fn status(&self) -> RequestGateStatus {
        RequestGateStatus::new(
            self.pool.limit(),
            self.pool.available_permit_count(),
            self.accepted_request_count,
        )
    }

    fn accept_request(&mut self, message: &mut AcquireRequestPermit) {
        self.accepted_request_count = self.accepted_request_count.saturating_add(1);
        message.record_admission(self.accepted_request_count);
    }
}

impl Actor for RequestGate {
    type Args = Self;
    type Error = Infallible;

    async fn on_start(
        gate: Self::Args,
        _actor_reference: ActorRef<Self>,
    ) -> Result<Self, Self::Error> {
        Ok(gate)
    }
}

impl AcquireRequestPermit {
    pub fn new(request_name: impl Into<String>) -> Self {
        Self {
            request_name: request_name.into(),
            #[cfg(test)]
            admission_observer: None,
        }
    }

    pub fn request_name(&self) -> &str {
        &self.request_name
    }

    #[cfg(test)]
    fn with_admission_observer(
        request_name: impl Into<String>,
        admission_observer: RequestAdmissionObserver,
    ) -> Self {
        Self {
            request_name: request_name.into(),
            admission_observer: Some(admission_observer),
        }
    }

    fn record_admission(&mut self, accepted_request_count: u64) {
        #[cfg(test)]
        if let Some(observer) = self.admission_observer.take() {
            observer.record(RequestAdmission::new(
                self.request_name.clone(),
                accepted_request_count,
            ));
        }
        let _ = accepted_request_count;
    }
}

impl RequestGateStatusRequest {
    pub fn new(observer_name: impl Into<String>) -> Self {
        Self {
            observer_name: observer_name.into(),
        }
    }

    pub fn observer_name(&self) -> &str {
        &self.observer_name
    }
}

impl RequestGateStatus {
    pub const fn new(
        limit: RequestConcurrencyLimit,
        available_permit_count: usize,
        accepted_request_count: u64,
    ) -> Self {
        Self {
            limit,
            available_permit_count,
            accepted_request_count,
        }
    }

    pub fn limit(self) -> RequestConcurrencyLimit {
        self.limit
    }

    pub fn available_permit_count(self) -> usize {
        self.available_permit_count
    }

    pub fn accepted_request_count(self) -> u64 {
        self.accepted_request_count
    }
}

impl<Listener> AsyncListenerSocket<Listener> {
    pub fn new(listener: Listener, socket_path: impl Into<PathBuf>) -> Self {
        Self {
            listener,
            socket_path: socket_path.into(),
            socket_mode: None,
        }
    }

    pub fn with_socket_mode(mut self, socket_mode: SocketMode) -> Self {
        self.socket_mode = Some(socket_mode);
        self
    }

    pub fn listener(&self) -> &Listener {
        &self.listener
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn socket_mode(&self) -> Option<SocketMode> {
        self.socket_mode
    }
}

impl Message<AcquireRequestPermit> for RequestGate {
    type Reply = DelegatedReply<Result<RequestPermit, RequestPermitError>>;

    async fn handle(
        &mut self,
        mut message: AcquireRequestPermit,
        context: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.accept_request(&mut message);
        let pool = self.pool.clone();
        context.spawn(async move { pool.acquire().await })
    }
}

impl Message<RequestGateStatusRequest> for RequestGate {
    type Reply = RequestGateStatus;

    async fn handle(
        &mut self,
        _message: RequestGateStatusRequest,
        _context: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.status()
    }
}

impl<Runtime> AsyncMultiListenerDaemon<Runtime>
where
    Runtime: AsyncMultiConnectionRuntime,
{
    pub fn new(
        listener_sockets: impl IntoIterator<Item = AsyncListenerSocket<Runtime::Listener>>,
        runtime: Runtime,
        request_error_log: RequestErrorLog,
    ) -> Self {
        Self {
            listener_sockets: listener_sockets.into_iter().collect(),
            concurrency_limit: RequestConcurrencyLimit::one(),
            request_error_log,
            runtime: Arc::new(runtime),
        }
    }

    pub fn with_concurrency_limit(mut self, concurrency_limit: RequestConcurrencyLimit) -> Self {
        self.concurrency_limit = concurrency_limit;
        self
    }

    pub fn listener_sockets(&self) -> &[AsyncListenerSocket<Runtime::Listener>] {
        &self.listener_sockets
    }

    pub async fn bind(self) -> Result<BoundAsyncMultiListenerDaemon<Runtime>, AsyncListenerError> {
        let mut listeners = Vec::new();
        for listener_socket in self.listener_sockets {
            let socket_path = listener_socket.socket_path;
            let unix_listener =
                AsyncSocketPath::new(&socket_path, listener_socket.socket_mode).bind()?;
            let request_gate = RequestGate::new(self.concurrency_limit).start().await;
            listeners.push(BoundAsyncListener::new(
                listener_socket.listener,
                socket_path,
                unix_listener,
                request_gate,
            ));
        }
        Ok(BoundAsyncMultiListenerDaemon {
            listeners,
            request_error_log: self.request_error_log,
            runtime: self.runtime,
        })
    }

    pub async fn run(self) -> Result<(), AsyncMultiListenerDaemonError<Runtime::Error>> {
        let daemon = self
            .bind()
            .await
            .map_err(AsyncMultiListenerDaemonError::Listener)?;
        daemon.run().await
    }
}

impl<Runtime> AsyncSingleListenerDaemon<Runtime>
where
    Runtime: AsyncConnectionRuntime,
{
    pub fn new(
        socket_path: impl Into<PathBuf>,
        runtime: Runtime,
        request_error_log: RequestErrorLog,
    ) -> Self {
        Self {
            socket_path: socket_path.into(),
            socket_mode: None,
            concurrency_limit: RequestConcurrencyLimit::one(),
            request_error_log,
            runtime: Arc::new(runtime),
        }
    }

    pub fn with_socket_mode(mut self, socket_mode: SocketMode) -> Self {
        self.socket_mode = Some(socket_mode);
        self
    }

    pub fn with_concurrency_limit(mut self, concurrency_limit: RequestConcurrencyLimit) -> Self {
        self.concurrency_limit = concurrency_limit;
        self
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub async fn bind(self) -> Result<BoundAsyncSingleListenerDaemon<Runtime>, AsyncListenerError> {
        let listener = AsyncSocketPath::new(&self.socket_path, self.socket_mode).bind()?;
        let request_gate = RequestGate::new(self.concurrency_limit).start().await;
        Ok(BoundAsyncSingleListenerDaemon {
            _socket_file: AsyncSocketFile::new(self.socket_path),
            listener,
            request_gate,
            request_error_log: self.request_error_log,
            runtime: self.runtime,
        })
    }

    pub async fn run(self) -> Result<(), AsyncSingleListenerDaemonError<Runtime::Error>> {
        let daemon = self
            .bind()
            .await
            .map_err(AsyncSingleListenerDaemonError::Listener)?;
        daemon.run().await
    }
}

impl<Runtime> BoundAsyncMultiListenerDaemon<Runtime>
where
    Runtime: AsyncMultiConnectionRuntime,
{
    pub fn runtime(&self) -> &Runtime {
        self.runtime.as_ref()
    }

    pub fn listener_count(&self) -> usize {
        self.listeners.len()
    }

    pub async fn start(&self) -> Result<(), Runtime::Error> {
        self.runtime.start().await
    }

    pub async fn stop(&self) -> Result<(), Runtime::Error> {
        for listener in &self.listeners {
            listener.request_gate().stop_gracefully().await.ok();
        }
        for listener in &self.listeners {
            listener.request_gate().wait_for_shutdown().await;
        }
        self.runtime.stop().await
    }

    pub async fn serve_next_connection_at(&self, index: usize) -> Result<(), AsyncListenerError> {
        let Some(listener) = self.listeners.get(index) else {
            return Err(AsyncListenerError::ListenerIndexOutOfRange {
                index,
                listener_count: self.listeners.len(),
            });
        };
        let connection = self
            .accepted_connection(listener, listener.accept_connection().await?)
            .await?;
        self.spawn_connection(listener.listener().clone(), connection);
        Ok(())
    }

    pub async fn run(self) -> Result<(), AsyncMultiListenerDaemonError<Runtime::Error>> {
        self.start()
            .await
            .map_err(AsyncMultiListenerDaemonError::Start)?;
        let (daemon, serve_result) = self.serve_connections().await;
        let stop_result = daemon.stop().await;
        match (serve_result, stop_result) {
            (Err(error), _) => Err(AsyncMultiListenerDaemonError::Listener(error)),
            (Ok(()), Err(error)) => Err(AsyncMultiListenerDaemonError::Stop(error)),
            (Ok(()), Ok(())) => Ok(()),
        }
    }

    async fn serve_connections(
        self,
    ) -> (
        StoppedAsyncMultiListenerDaemon<Runtime>,
        Result<(), AsyncListenerError>,
    ) {
        let request_error_log = self.request_error_log.clone();
        let runtime = self.runtime.clone();
        let request_gates = self
            .listeners
            .iter()
            .map(|listener| listener.request_gate().clone())
            .collect::<Vec<_>>();
        let mut tasks = tokio::task::JoinSet::new();

        for listener in self.listeners {
            let listener_task =
                AsyncListenerTask::new(listener, request_error_log.clone(), runtime.clone());
            tasks.spawn(async move { listener_task.serve_connections().await });
        }

        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tasks.abort_all();
                    let daemon = StoppedAsyncMultiListenerDaemon::new(
                        request_gates,
                        request_error_log,
                        runtime,
                    );
                    return (daemon, Err(error));
                }
                Err(error) => {
                    tasks.abort_all();
                    let daemon = StoppedAsyncMultiListenerDaemon::new(
                        request_gates,
                        request_error_log,
                        runtime,
                    );
                    return (
                        daemon,
                        Err(AsyncListenerError::ListenerTask {
                            detail: error.to_string(),
                        }),
                    );
                }
            }
        }

        (
            StoppedAsyncMultiListenerDaemon::new(request_gates, request_error_log, runtime),
            Ok(()),
        )
    }

    async fn accepted_connection(
        &self,
        listener: &BoundAsyncListener<Runtime::Listener>,
        stream: TokioUnixStream,
    ) -> Result<AcceptedConnection, AsyncListenerError> {
        let context = ConnectionContext::from_tokio_stream(&stream)?;
        let permit = self.acquire_permit(listener).await?;
        Ok(AcceptedConnection::new(stream, context, permit))
    }

    async fn acquire_permit(
        &self,
        listener: &BoundAsyncListener<Runtime::Listener>,
    ) -> Result<RequestPermit, AsyncListenerError> {
        listener
            .request_gate()
            .ask(AcquireRequestPermit::new("accepted-connection"))
            .await
            .map_err(|error| AsyncListenerError::RequestGate {
                detail: error.to_string(),
            })
    }

    fn spawn_connection(&self, listener: Runtime::Listener, connection: AcceptedConnection) {
        let runtime = self.runtime.clone();
        let request_error_log = self.request_error_log.clone();
        tokio::spawn(async move {
            if let Err(error) = runtime
                .handle_connection(listener.clone(), connection)
                .await
            {
                request_error_log.report_for_listener(&listener, &error);
            }
        });
    }
}

impl<Runtime> BoundAsyncSingleListenerDaemon<Runtime>
where
    Runtime: AsyncConnectionRuntime,
{
    pub fn runtime(&self) -> &Runtime {
        self.runtime.as_ref()
    }

    pub async fn start(&self) -> Result<(), Runtime::Error> {
        self.runtime.start().await
    }

    pub async fn stop(&self) -> Result<(), Runtime::Error> {
        self.request_gate.stop_gracefully().await.ok();
        self.request_gate.wait_for_shutdown().await;
        self.runtime.stop().await
    }

    pub async fn serve_next_connection(&self) -> Result<(), AsyncListenerError> {
        let (stream, _) = self.listener.accept().await?;
        let connection = self.accepted_connection(stream).await?;
        self.spawn_connection(connection);
        Ok(())
    }

    pub async fn serve_connections(&self) -> Result<(), AsyncListenerError> {
        loop {
            self.serve_next_connection().await?;
        }
    }

    pub async fn run(self) -> Result<(), AsyncSingleListenerDaemonError<Runtime::Error>> {
        self.start()
            .await
            .map_err(AsyncSingleListenerDaemonError::Start)?;
        let serve_result = self.serve_connections().await;
        let stop_result = self.stop().await;
        match (serve_result, stop_result) {
            (Err(error), _) => Err(AsyncSingleListenerDaemonError::Listener(error)),
            (Ok(()), Err(error)) => Err(AsyncSingleListenerDaemonError::Stop(error)),
            (Ok(()), Ok(())) => Ok(()),
        }
    }

    async fn accepted_connection(
        &self,
        stream: TokioUnixStream,
    ) -> Result<AcceptedConnection, AsyncListenerError> {
        let context = ConnectionContext::from_tokio_stream(&stream)?;
        let permit = self.acquire_permit().await?;
        Ok(AcceptedConnection::new(stream, context, permit))
    }

    async fn acquire_permit(&self) -> Result<RequestPermit, AsyncListenerError> {
        self.request_gate
            .ask(AcquireRequestPermit::new("accepted-connection"))
            .await
            .map_err(|error| AsyncListenerError::RequestGate {
                detail: error.to_string(),
            })
    }

    fn spawn_connection(&self, connection: AcceptedConnection) {
        let runtime = self.runtime.clone();
        let request_error_log = self.request_error_log.clone();
        tokio::spawn(async move {
            if let Err(error) = runtime.handle_connection(connection).await {
                request_error_log.report(&error);
            }
        });
    }
}

impl<Runtime> StoppedAsyncMultiListenerDaemon<Runtime> {
    fn new(
        request_gates: Vec<ActorRef<RequestGate>>,
        request_error_log: RequestErrorLog,
        runtime: Arc<Runtime>,
    ) -> Self {
        Self {
            request_gates,
            _request_error_log: request_error_log,
            runtime,
        }
    }
}

impl<Runtime> StoppedAsyncMultiListenerDaemon<Runtime>
where
    Runtime: AsyncMultiConnectionRuntime,
{
    async fn stop(&self) -> Result<(), Runtime::Error> {
        for request_gate in &self.request_gates {
            request_gate.stop_gracefully().await.ok();
        }
        for request_gate in &self.request_gates {
            request_gate.wait_for_shutdown().await;
        }
        self.runtime.stop().await
    }
}

impl<Listener> BoundAsyncListener<Listener> {
    fn new(
        listener: Listener,
        socket_path: PathBuf,
        unix_listener: TokioUnixListener,
        request_gate: ActorRef<RequestGate>,
    ) -> Self {
        Self {
            listener,
            _socket_file: AsyncSocketFile::new(socket_path),
            unix_listener,
            request_gate,
        }
    }

    fn listener(&self) -> &Listener {
        &self.listener
    }

    fn request_gate(&self) -> &ActorRef<RequestGate> {
        &self.request_gate
    }

    async fn accept_connection(&self) -> Result<TokioUnixStream, AsyncListenerError> {
        let (stream, _) = self.unix_listener.accept().await?;
        Ok(stream)
    }
}

impl<Listener, Runtime> AsyncListenerTask<Listener, Runtime>
where
    Listener: Clone + Display + Send + Sync + 'static,
    Runtime: AsyncMultiConnectionRuntime<Listener = Listener>,
{
    fn new(
        listener: BoundAsyncListener<Listener>,
        request_error_log: RequestErrorLog,
        runtime: Arc<Runtime>,
    ) -> Self {
        Self {
            listener,
            request_error_log,
            runtime,
        }
    }

    async fn serve_connections(self) -> Result<(), AsyncListenerError> {
        loop {
            self.serve_next_connection().await?;
        }
    }

    async fn serve_next_connection(&self) -> Result<(), AsyncListenerError> {
        let listener = self.listener.listener().clone();
        let stream = self.listener.accept_connection().await?;
        let connection = self.accepted_connection(stream).await?;
        self.spawn_connection(listener, connection);
        Ok(())
    }

    async fn accepted_connection(
        &self,
        stream: TokioUnixStream,
    ) -> Result<AcceptedConnection, AsyncListenerError> {
        let context = ConnectionContext::from_tokio_stream(&stream)?;
        let permit = self.acquire_permit().await?;
        Ok(AcceptedConnection::new(stream, context, permit))
    }

    async fn acquire_permit(&self) -> Result<RequestPermit, AsyncListenerError> {
        self.listener
            .request_gate()
            .ask(AcquireRequestPermit::new("accepted-connection"))
            .await
            .map_err(|error| AsyncListenerError::RequestGate {
                detail: error.to_string(),
            })
    }

    fn spawn_connection(&self, listener: Listener, connection: AcceptedConnection) {
        let runtime = self.runtime.clone();
        let request_error_log = self.request_error_log.clone();
        tokio::spawn(async move {
            if let Err(error) = runtime
                .handle_connection(listener.clone(), connection)
                .await
            {
                request_error_log.report_for_listener(&listener, &error);
            }
        });
    }
}

impl<RuntimeError> std::fmt::Display for AsyncSingleListenerDaemonError<RuntimeError>
where
    RuntimeError: Display,
{
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Listener(error) => write!(formatter, "{error}"),
            Self::Start(error) => write!(formatter, "async daemon runtime start error: {error}"),
            Self::Stop(error) => write!(formatter, "async daemon runtime stop error: {error}"),
        }
    }
}

impl<RuntimeError> Display for AsyncMultiListenerDaemonError<RuntimeError>
where
    RuntimeError: Display,
{
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Listener(error) => write!(formatter, "{error}"),
            Self::Start(error) => {
                write!(
                    formatter,
                    "async multi-listener runtime start error: {error}"
                )
            }
            Self::Stop(error) => {
                write!(
                    formatter,
                    "async multi-listener runtime stop error: {error}"
                )
            }
        }
    }
}

impl<RuntimeError> Error for AsyncSingleListenerDaemonError<RuntimeError>
where
    RuntimeError: Error + 'static,
{
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Listener(error) => Some(error),
            Self::Start(error) => Some(error),
            Self::Stop(error) => Some(error),
        }
    }
}

impl<RuntimeError> Error for AsyncMultiListenerDaemonError<RuntimeError>
where
    RuntimeError: Error + 'static,
{
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Listener(error) => Some(error),
            Self::Start(error) => Some(error),
            Self::Stop(error) => Some(error),
        }
    }
}

impl<'path> AsyncSocketPath<'path> {
    fn new(path: &'path Path, socket_mode: Option<SocketMode>) -> Self {
        Self { path, socket_mode }
    }

    fn bind(&self) -> Result<TokioUnixListener, AsyncListenerError> {
        self.prepare()?;
        let listener = TokioUnixListener::bind(self.path)?;
        self.apply_socket_mode()?;
        Ok(listener)
    }

    fn prepare(&self) -> Result<(), AsyncListenerError> {
        self.create_parent_directory()?;
        self.remove_stale_socket()
    }

    fn create_parent_directory(&self) -> Result<(), AsyncListenerError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    fn remove_stale_socket(&self) -> Result<(), AsyncListenerError> {
        match std::fs::remove_file(self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(AsyncListenerError::Io(error)),
        }
    }

    fn apply_socket_mode(&self) -> Result<(), AsyncListenerError> {
        if let Some(socket_mode) = self.socket_mode {
            std::fs::set_permissions(
                self.path,
                std::fs::Permissions::from_mode(socket_mode.bits()),
            )?;
        }
        Ok(())
    }
}

impl AsyncSocketFile {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for AsyncSocketFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
impl RequestAdmissionObserver {
    fn new(sender: tokio::sync::oneshot::Sender<RequestAdmission>) -> Self {
        Self {
            sender: Some(sender),
        }
    }

    fn record(mut self, admission: RequestAdmission) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(admission);
        }
    }
}

#[cfg(test)]
impl RequestAdmission {
    fn new(request_name: impl Into<String>, accepted_request_count: u64) -> Self {
        Self {
            request_name: request_name.into(),
            accepted_request_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn request_concurrency_limit_clamps_zero_to_one() {
        let limit = RequestConcurrencyLimit::new(0);
        let gate = RequestGate::new(limit).start().await;

        let status = gate
            .ask(RequestGateStatusRequest::new("test"))
            .await
            .expect("status");

        assert_eq!(status.limit(), RequestConcurrencyLimit::one());
        assert_eq!(status.available_permit_count(), 1);

        gate.stop_gracefully().await.expect("stop gate");
        gate.wait_for_shutdown().await;
    }

    #[tokio::test]
    async fn request_gate_admits_only_the_configured_concurrency() {
        let gate = RequestGate::new(RequestConcurrencyLimit::new(2))
            .start()
            .await;

        let first = gate
            .ask(AcquireRequestPermit::new("first"))
            .await
            .expect("first permit");
        let second = gate
            .ask(AcquireRequestPermit::new("second"))
            .await
            .expect("second permit");

        let gate_for_third = gate.clone();
        let third =
            tokio::spawn(
                async move { gate_for_third.ask(AcquireRequestPermit::new("third")).await },
            );

        tokio::time::sleep(Duration::from_millis(25)).await;
        assert!(!third.is_finished());

        drop(first);

        let third = tokio::time::timeout(Duration::from_millis(200), third)
            .await
            .expect("third permit unblocks")
            .expect("third task joins")
            .expect("third permit succeeds");

        assert_eq!(third.limit(), RequestConcurrencyLimit::new(2));
        drop(second);
        drop(third);

        let status = gate
            .ask(RequestGateStatusRequest::new("test"))
            .await
            .expect("status");
        assert_eq!(status.available_permit_count(), 2);
        assert_eq!(status.accepted_request_count(), 3);

        gate.stop_gracefully().await.expect("stop gate");
        gate.wait_for_shutdown().await;
    }

    #[tokio::test]
    async fn request_gate_mailbox_stays_available_while_permit_waits() {
        let gate = RequestGate::new(RequestConcurrencyLimit::one())
            .start()
            .await;
        let held = gate
            .ask(AcquireRequestPermit::new("held"))
            .await
            .expect("held permit");

        let (admission_sender, admission_receiver) = tokio::sync::oneshot::channel();
        let waiting_request = AcquireRequestPermit::with_admission_observer(
            "waiting",
            RequestAdmissionObserver::new(admission_sender),
        );
        let gate_for_waiting_request = gate.clone();
        let waiting_permit =
            tokio::spawn(async move { gate_for_waiting_request.ask(waiting_request).await });

        let admission = tokio::time::timeout(Duration::from_millis(200), admission_receiver)
            .await
            .expect("waiting request reaches actor")
            .expect("admission observed");
        assert_eq!(admission.request_name, "waiting");
        assert_eq!(admission.accepted_request_count, 2);

        let status = tokio::time::timeout(
            Duration::from_millis(200),
            gate.ask(RequestGateStatusRequest::new("probe")),
        )
        .await
        .expect("status reply is not blocked behind waiting permit")
        .expect("status ask");
        assert_eq!(status.available_permit_count(), 0);
        assert_eq!(status.accepted_request_count(), 2);

        drop(held);

        let waiting = tokio::time::timeout(Duration::from_millis(200), waiting_permit)
            .await
            .expect("waiting permit unblocks")
            .expect("waiting task joins")
            .expect("waiting permit succeeds");
        assert_eq!(waiting.limit(), RequestConcurrencyLimit::one());
        drop(waiting);

        gate.stop_gracefully().await.expect("stop gate");
        gate.wait_for_shutdown().await;
    }

    #[derive(Clone, Debug)]
    struct CountingConnectionRuntime {
        live_request_count: Arc<AtomicUsize>,
        peak_request_count: Arc<AtomicUsize>,
        delay: Duration,
    }

    #[derive(Debug, Error)]
    enum CountingConnectionError {
        #[error("connection IO error: {0}")]
        Io(#[from] std::io::Error),

        #[error("test release channel closed")]
        ReleaseClosed,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum AsyncRuntimeTestListener {
        Ordinary,
        Meta,
    }

    #[derive(Clone, Debug)]
    struct RoutingMultiConnectionRuntime {
        events: Arc<tokio::sync::Mutex<Vec<AsyncRuntimeTestListener>>>,
    }

    #[derive(Debug)]
    struct BlockingMultiConnectionRuntime {
        ordinary_started_sender: tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        ordinary_release_receiver: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }

    impl CountingConnectionRuntime {
        fn new(delay: Duration) -> Self {
            Self {
                live_request_count: Arc::new(AtomicUsize::new(0)),
                peak_request_count: Arc::new(AtomicUsize::new(0)),
                delay,
            }
        }

        fn peak_request_count(&self) -> usize {
            self.peak_request_count.load(Ordering::SeqCst)
        }
    }

    impl AsyncRuntimeTestListener {
        fn response_offset(self) -> u8 {
            match self {
                Self::Ordinary => 10,
                Self::Meta => 20,
            }
        }
    }

    impl Display for AsyncRuntimeTestListener {
        fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Ordinary => formatter.write_str("ordinary"),
                Self::Meta => formatter.write_str("meta"),
            }
        }
    }

    impl RoutingMultiConnectionRuntime {
        fn new() -> Self {
            Self {
                events: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            }
        }

        async fn events(&self) -> Vec<AsyncRuntimeTestListener> {
            self.events.lock().await.clone()
        }
    }

    impl BlockingMultiConnectionRuntime {
        fn new(
            ordinary_started_sender: tokio::sync::oneshot::Sender<()>,
            ordinary_release_receiver: tokio::sync::oneshot::Receiver<()>,
        ) -> Self {
            Self {
                ordinary_started_sender: tokio::sync::Mutex::new(Some(ordinary_started_sender)),
                ordinary_release_receiver: tokio::sync::Mutex::new(Some(ordinary_release_receiver)),
            }
        }

        async fn wait_for_release(&self) -> Result<(), CountingConnectionError> {
            let release_receiver = self
                .ordinary_release_receiver
                .lock()
                .await
                .take()
                .expect("ordinary release receiver is present");
            release_receiver
                .await
                .map_err(|_| CountingConnectionError::ReleaseClosed)
        }

        async fn record_ordinary_start(&self) {
            if let Some(sender) = self.ordinary_started_sender.lock().await.take() {
                let _ = sender.send(());
            }
        }
    }

    impl AsyncConnectionRuntime for CountingConnectionRuntime {
        type Error = CountingConnectionError;

        async fn handle_connection(
            &self,
            mut connection: AcceptedConnection,
        ) -> Result<(), Self::Error> {
            let now = self.live_request_count.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak_request_count.fetch_max(now, Ordering::SeqCst);

            let mut byte = [0_u8; 1];
            connection.stream_mut().read_exact(&mut byte).await?;
            tokio::time::sleep(self.delay).await;
            connection
                .stream_mut()
                .write_all(&[byte[0].saturating_add(1)])
                .await?;
            self.live_request_count.fetch_sub(1, Ordering::SeqCst);
            Ok(())
        }
    }

    impl AsyncMultiConnectionRuntime for RoutingMultiConnectionRuntime {
        type Listener = AsyncRuntimeTestListener;
        type Error = CountingConnectionError;

        async fn handle_connection(
            &self,
            listener: Self::Listener,
            mut connection: AcceptedConnection,
        ) -> Result<(), Self::Error> {
            self.events.lock().await.push(listener);
            let mut byte = [0_u8; 1];
            connection.stream_mut().read_exact(&mut byte).await?;
            connection
                .stream_mut()
                .write_all(&[byte[0].saturating_add(listener.response_offset())])
                .await?;
            Ok(())
        }
    }

    impl AsyncMultiConnectionRuntime for BlockingMultiConnectionRuntime {
        type Listener = AsyncRuntimeTestListener;
        type Error = CountingConnectionError;

        async fn handle_connection(
            &self,
            listener: Self::Listener,
            mut connection: AcceptedConnection,
        ) -> Result<(), Self::Error> {
            let mut byte = [0_u8; 1];
            connection.stream_mut().read_exact(&mut byte).await?;

            match listener {
                AsyncRuntimeTestListener::Ordinary => {
                    self.record_ordinary_start().await;
                    self.wait_for_release().await?;
                }
                AsyncRuntimeTestListener::Meta => {}
            }

            connection
                .stream_mut()
                .write_all(&[byte[0].saturating_add(listener.response_offset())])
                .await?;
            Ok(())
        }
    }

    struct AsyncRuntimeTestClient {
        socket_path: PathBuf,
        request_byte: u8,
    }

    impl AsyncRuntimeTestClient {
        fn new(socket_path: impl Into<PathBuf>, request_byte: u8) -> Self {
            Self {
                socket_path: socket_path.into(),
                request_byte,
            }
        }

        async fn run(self) -> std::io::Result<[u8; 1]> {
            let mut stream = TokioUnixStream::connect(self.socket_path).await?;
            stream.write_all(&[self.request_byte]).await?;
            let mut response = [0_u8; 1];
            stream.read_exact(&mut response).await?;
            Ok(response)
        }
    }

    #[tokio::test]
    async fn async_single_listener_daemon_serves_connections_under_limit() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let socket_path = directory.path().join("async-daemon.sock");
        let runtime = CountingConnectionRuntime::new(Duration::from_millis(25));
        let daemon = AsyncSingleListenerDaemon::new(
            &socket_path,
            runtime.clone(),
            RequestErrorLog::new("async-test"),
        )
        .with_concurrency_limit(RequestConcurrencyLimit::new(2))
        .bind()
        .await
        .expect("bind async listener");

        let clients = (0..4_u8)
            .map(|byte| {
                let socket_path = socket_path.clone();
                tokio::spawn(
                    async move { AsyncRuntimeTestClient::new(socket_path, byte).run().await },
                )
            })
            .collect::<Vec<_>>();

        for _ in 0..4 {
            daemon
                .serve_next_connection()
                .await
                .expect("serve next connection");
        }

        let mut responses = Vec::new();
        for client in clients {
            responses.push(
                client
                    .await
                    .expect("client joins")
                    .expect("client receives response")[0],
            );
        }
        responses.sort_unstable();

        assert_eq!(responses, [1, 2, 3, 4]);
        assert!(runtime.peak_request_count() <= 2);
        daemon.stop().await.expect("stop async listener");
    }

    #[tokio::test]
    async fn async_single_listener_daemon_removes_socket_path_on_drop() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let socket_path = directory.path().join("async-daemon.sock");
        {
            let runtime = CountingConnectionRuntime::new(Duration::from_millis(1));
            let _daemon = AsyncSingleListenerDaemon::new(
                &socket_path,
                runtime,
                RequestErrorLog::new("async-test"),
            )
            .bind()
            .await
            .expect("bind async listener");

            assert!(socket_path.exists());
        }

        assert!(!socket_path.exists());
    }

    #[tokio::test]
    async fn async_multi_listener_daemon_routes_connections_by_socket() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let ordinary_socket_path = directory.path().join("ordinary.sock");
        let meta_socket_path = directory.path().join("meta.sock");
        let runtime = RoutingMultiConnectionRuntime::new();
        let observed_runtime = runtime.clone();
        let listener_sockets = [
            AsyncListenerSocket::new(AsyncRuntimeTestListener::Ordinary, &ordinary_socket_path),
            AsyncListenerSocket::new(AsyncRuntimeTestListener::Meta, &meta_socket_path),
        ];
        let daemon = AsyncMultiListenerDaemon::new(
            listener_sockets,
            runtime,
            RequestErrorLog::new("async-test"),
        )
        .with_concurrency_limit(RequestConcurrencyLimit::new(2))
        .bind()
        .await
        .expect("bind actor multi listener");

        let ordinary_client =
            tokio::spawn(AsyncRuntimeTestClient::new(&ordinary_socket_path, 1).run());
        daemon
            .serve_next_connection_at(0)
            .await
            .expect("serve ordinary connection");

        let meta_client = tokio::spawn(AsyncRuntimeTestClient::new(&meta_socket_path, 1).run());
        daemon
            .serve_next_connection_at(1)
            .await
            .expect("serve meta connection");

        let ordinary_response = ordinary_client
            .await
            .expect("ordinary client joins")
            .expect("ordinary response");
        let meta_response = meta_client
            .await
            .expect("meta client joins")
            .expect("meta response");

        assert_eq!(ordinary_response, [11]);
        assert_eq!(meta_response, [21]);
        assert_eq!(
            observed_runtime.events().await,
            [
                AsyncRuntimeTestListener::Ordinary,
                AsyncRuntimeTestListener::Meta
            ]
        );
        daemon.stop().await.expect("stop actor multi listener");
    }

    #[tokio::test]
    async fn async_multi_listener_accepts_meta_while_ordinary_handler_waits() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let ordinary_socket_path = directory.path().join("ordinary.sock");
        let meta_socket_path = directory.path().join("meta.sock");
        let (ordinary_started_sender, ordinary_started_receiver) = tokio::sync::oneshot::channel();
        let (ordinary_release_sender, ordinary_release_receiver) = tokio::sync::oneshot::channel();
        let listener_sockets = [
            AsyncListenerSocket::new(AsyncRuntimeTestListener::Ordinary, &ordinary_socket_path),
            AsyncListenerSocket::new(AsyncRuntimeTestListener::Meta, &meta_socket_path),
        ];
        let daemon = AsyncMultiListenerDaemon::new(
            listener_sockets,
            BlockingMultiConnectionRuntime::new(ordinary_started_sender, ordinary_release_receiver),
            RequestErrorLog::new("async-test"),
        )
        .bind()
        .await
        .expect("bind actor multi listener");
        let server = tokio::spawn(daemon.run());

        let ordinary_client =
            tokio::spawn(AsyncRuntimeTestClient::new(&ordinary_socket_path, 7).run());
        tokio::time::timeout(Duration::from_millis(200), ordinary_started_receiver)
            .await
            .expect("ordinary handler starts")
            .expect("ordinary start signal");

        let meta_response = tokio::time::timeout(
            Duration::from_millis(200),
            AsyncRuntimeTestClient::new(&meta_socket_path, 3).run(),
        )
        .await
        .expect("meta completes while ordinary waits")
        .expect("meta response");
        assert_eq!(meta_response, [23]);

        ordinary_release_sender
            .send(())
            .expect("release ordinary handler");
        let ordinary_response = ordinary_client
            .await
            .expect("ordinary client joins")
            .expect("ordinary response");
        assert_eq!(ordinary_response, [17]);

        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn async_multi_listener_socket_modes_and_cleanup_apply_per_socket() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let ordinary_socket_path = directory.path().join("ordinary.sock");
        let meta_socket_path = directory.path().join("meta.sock");
        {
            let listener_sockets = [
                AsyncListenerSocket::new(AsyncRuntimeTestListener::Ordinary, &ordinary_socket_path)
                    .with_socket_mode(SocketMode::new(0o600)),
                AsyncListenerSocket::new(AsyncRuntimeTestListener::Meta, &meta_socket_path)
                    .with_socket_mode(SocketMode::new(0o660)),
            ];
            let _daemon = AsyncMultiListenerDaemon::new(
                listener_sockets,
                RoutingMultiConnectionRuntime::new(),
                RequestErrorLog::new("async-test"),
            )
            .bind()
            .await
            .expect("bind actor multi listener");

            let ordinary_mode = std::fs::metadata(&ordinary_socket_path)
                .expect("ordinary metadata")
                .permissions()
                .mode()
                & 0o777;
            let meta_mode = std::fs::metadata(&meta_socket_path)
                .expect("meta metadata")
                .permissions()
                .mode()
                & 0o777;

            assert_eq!(ordinary_mode, 0o600);
            assert_eq!(meta_mode, 0o660);
        }

        assert!(!ordinary_socket_path.exists());
        assert!(!meta_socket_path.exists());
    }
}
