//! Actor-native runtime substrate for generated triad daemons.
//!
//! This module is the replacement direction for the old synchronous listener
//! and thread-worker shell. It intentionally starts with the load-bearing
//! primitive every actor daemon needs first: request admission that applies
//! backpressure without blocking an actor mailbox.

use std::error::Error;
use std::fmt::{Debug, Formatter};
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
pub enum ActorListenerError {
    #[error("actor listener IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("actor request gate error: {detail}")]
    RequestGate { detail: String },
}

#[derive(Debug)]
pub enum ActorSingleListenerDaemonError<RuntimeError> {
    Listener(ActorListenerError),
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

pub trait ActorConnectionRuntime: Send + Sync + 'static {
    type Error: std::fmt::Display + Send + Sync + 'static;

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

pub struct ActorSingleListenerDaemon<Runtime> {
    socket_path: PathBuf,
    socket_mode: Option<SocketMode>,
    concurrency_limit: RequestConcurrencyLimit,
    request_error_log: RequestErrorLog,
    runtime: Arc<Runtime>,
}

pub struct ActorBoundSingleListenerDaemon<Runtime> {
    _socket_file: ActorBoundSocketFile,
    listener: TokioUnixListener,
    request_gate: ActorRef<RequestGate>,
    request_error_log: RequestErrorLog,
    runtime: Arc<Runtime>,
}

struct ActorBoundSocketPath<'path> {
    path: &'path Path,
    socket_mode: Option<SocketMode>,
}

struct ActorBoundSocketFile {
    path: PathBuf,
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

impl<Runtime> ActorSingleListenerDaemon<Runtime>
where
    Runtime: ActorConnectionRuntime,
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

    pub async fn bind(self) -> Result<ActorBoundSingleListenerDaemon<Runtime>, ActorListenerError> {
        let listener = ActorBoundSocketPath::new(&self.socket_path, self.socket_mode).bind()?;
        let request_gate = RequestGate::new(self.concurrency_limit).start().await;
        Ok(ActorBoundSingleListenerDaemon {
            _socket_file: ActorBoundSocketFile::new(self.socket_path),
            listener,
            request_gate,
            request_error_log: self.request_error_log,
            runtime: self.runtime,
        })
    }

    pub async fn run(self) -> Result<(), ActorSingleListenerDaemonError<Runtime::Error>> {
        let daemon = self
            .bind()
            .await
            .map_err(ActorSingleListenerDaemonError::Listener)?;
        daemon.run().await
    }
}

impl<Runtime> ActorBoundSingleListenerDaemon<Runtime>
where
    Runtime: ActorConnectionRuntime,
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

    pub async fn serve_next_connection(&self) -> Result<(), ActorListenerError> {
        let (stream, _) = self.listener.accept().await?;
        let connection = self.accepted_connection(stream).await?;
        self.spawn_connection(connection);
        Ok(())
    }

    pub async fn serve_connections(&self) -> Result<(), ActorListenerError> {
        loop {
            self.serve_next_connection().await?;
        }
    }

    pub async fn run(self) -> Result<(), ActorSingleListenerDaemonError<Runtime::Error>> {
        self.start()
            .await
            .map_err(ActorSingleListenerDaemonError::Start)?;
        let serve_result = self.serve_connections().await;
        let stop_result = self.stop().await;
        match (serve_result, stop_result) {
            (Err(error), _) => Err(ActorSingleListenerDaemonError::Listener(error)),
            (Ok(()), Err(error)) => Err(ActorSingleListenerDaemonError::Stop(error)),
            (Ok(()), Ok(())) => Ok(()),
        }
    }

    async fn accepted_connection(
        &self,
        stream: TokioUnixStream,
    ) -> Result<AcceptedConnection, ActorListenerError> {
        let context = ConnectionContext::from_tokio_stream(&stream)?;
        let permit = self.acquire_permit().await?;
        Ok(AcceptedConnection::new(stream, context, permit))
    }

    async fn acquire_permit(&self) -> Result<RequestPermit, ActorListenerError> {
        self.request_gate
            .ask(AcquireRequestPermit::new("accepted-connection"))
            .await
            .map_err(|error| ActorListenerError::RequestGate {
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

impl<RuntimeError> std::fmt::Display for ActorSingleListenerDaemonError<RuntimeError>
where
    RuntimeError: std::fmt::Display,
{
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Listener(error) => write!(formatter, "{error}"),
            Self::Start(error) => write!(formatter, "actor daemon runtime start error: {error}"),
            Self::Stop(error) => write!(formatter, "actor daemon runtime stop error: {error}"),
        }
    }
}

impl<RuntimeError> Error for ActorSingleListenerDaemonError<RuntimeError>
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

impl<'path> ActorBoundSocketPath<'path> {
    fn new(path: &'path Path, socket_mode: Option<SocketMode>) -> Self {
        Self { path, socket_mode }
    }

    fn bind(&self) -> Result<TokioUnixListener, ActorListenerError> {
        self.prepare()?;
        let listener = TokioUnixListener::bind(self.path)?;
        self.apply_socket_mode()?;
        Ok(listener)
    }

    fn prepare(&self) -> Result<(), ActorListenerError> {
        self.create_parent_directory()?;
        self.remove_stale_socket()
    }

    fn create_parent_directory(&self) -> Result<(), ActorListenerError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    fn remove_stale_socket(&self) -> Result<(), ActorListenerError> {
        match std::fs::remove_file(self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(ActorListenerError::Io(error)),
        }
    }

    fn apply_socket_mode(&self) -> Result<(), ActorListenerError> {
        if let Some(socket_mode) = self.socket_mode {
            std::fs::set_permissions(
                self.path,
                std::fs::Permissions::from_mode(socket_mode.bits()),
            )?;
        }
        Ok(())
    }
}

impl ActorBoundSocketFile {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for ActorBoundSocketFile {
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

    impl ActorConnectionRuntime for CountingConnectionRuntime {
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

    struct ActorRuntimeTestClient {
        socket_path: PathBuf,
        request_byte: u8,
    }

    impl ActorRuntimeTestClient {
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
    async fn actor_single_listener_daemon_serves_connections_under_limit() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let socket_path = directory.path().join("actor-daemon.sock");
        let runtime = CountingConnectionRuntime::new(Duration::from_millis(25));
        let daemon = ActorSingleListenerDaemon::new(
            &socket_path,
            runtime.clone(),
            RequestErrorLog::new("actor-test"),
        )
        .with_concurrency_limit(RequestConcurrencyLimit::new(2))
        .bind()
        .await
        .expect("bind actor listener");

        let clients = (0..4_u8)
            .map(|byte| {
                let socket_path = socket_path.clone();
                tokio::spawn(
                    async move { ActorRuntimeTestClient::new(socket_path, byte).run().await },
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
        daemon.stop().await.expect("stop actor listener");
    }

    #[tokio::test]
    async fn actor_single_listener_daemon_removes_socket_path_on_drop() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let socket_path = directory.path().join("actor-daemon.sock");
        {
            let runtime = CountingConnectionRuntime::new(Duration::from_millis(1));
            let _daemon = ActorSingleListenerDaemon::new(
                &socket_path,
                runtime,
                RequestErrorLog::new("actor-test"),
            )
            .bind()
            .await
            .expect("bind actor listener");

            assert!(socket_path.exists());
        }

        assert!(!socket_path.exists());
    }
}
