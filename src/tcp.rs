//! Async task-backed TCP listener shell for cross-host component transport.
//!
//! The runtime binds whatever socket address the caller configures; it does
//! not know what a tailnet is. Deployments that want tailnet-only ingress
//! configure a tailnet-bound address — the trust boundary is the bind address
//! plus the carried [`PeerIdentity`](crate::PeerIdentity), never a payload
//! claim. There is no socket file: TCP has no modes to apply and no stale
//! path to remove, and cleanup is dropping the bound listener.

use std::net::SocketAddr;
use std::sync::Arc;

use kameo::actor::ActorRef;
use tokio::net::{TcpListener as TokioTcpListener, TcpStream as TokioTcpStream};

use crate::async_runtime::{
    AcceptedConnection, AcquireRequestPermit, AsyncConnectionRuntime, AsyncListenerError,
    AsyncSingleListenerDaemonError, RequestConcurrencyLimit, RequestGate, RequestPermit,
};
use crate::{ConnectionContext, RequestErrorLog};

/// The unbound TCP listener daemon shell: a configured socket address, an
/// admission budget, and the data-bearing component runtime that handles
/// accepted connections.
pub struct TcpListenerDaemon<Runtime> {
    socket_address: SocketAddr,
    concurrency_limit: RequestConcurrencyLimit,
    request_error_log: RequestErrorLog,
    runtime: Arc<Runtime>,
}

/// The bound TCP listener daemon: the live Tokio listener, its admission
/// gate, and the shared runtime. Dropping this value closes the listener —
/// that is the whole TCP cleanup story.
pub struct BoundTcpListenerDaemon<Runtime> {
    listener: TokioTcpListener,
    request_gate: ActorRef<RequestGate>,
    request_error_log: RequestErrorLog,
    runtime: Arc<Runtime>,
}

impl<Runtime> TcpListenerDaemon<Runtime>
where
    Runtime: AsyncConnectionRuntime<TokioTcpStream>,
{
    pub fn new(
        socket_address: SocketAddr,
        runtime: Runtime,
        request_error_log: RequestErrorLog,
    ) -> Self {
        Self {
            socket_address,
            concurrency_limit: RequestConcurrencyLimit::one(),
            request_error_log,
            runtime: Arc::new(runtime),
        }
    }

    pub fn with_concurrency_limit(mut self, concurrency_limit: RequestConcurrencyLimit) -> Self {
        self.concurrency_limit = concurrency_limit;
        self
    }

    pub fn socket_address(&self) -> SocketAddr {
        self.socket_address
    }

    pub async fn bind(self) -> Result<BoundTcpListenerDaemon<Runtime>, AsyncListenerError> {
        let listener = TokioTcpListener::bind(self.socket_address).await?;
        let request_gate = RequestGate::new(self.concurrency_limit).start().await;
        Ok(BoundTcpListenerDaemon {
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

impl<Runtime> BoundTcpListenerDaemon<Runtime>
where
    Runtime: AsyncConnectionRuntime<TokioTcpStream>,
{
    pub fn runtime(&self) -> &Runtime {
        self.runtime.as_ref()
    }

    /// The address the listener actually bound. Callers that configure port
    /// zero read the operating-system-assigned port here.
    pub fn local_address(&self) -> Result<SocketAddr, AsyncListenerError> {
        Ok(self.listener.local_addr()?)
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
        let (stream, remote_address) = self.listener.accept().await?;
        let permit = self.acquire_permit().await?;
        let connection =
            AcceptedConnection::new(stream, ConnectionContext::from(remote_address), permit);
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

    async fn acquire_permit(&self) -> Result<RequestPermit, AsyncListenerError> {
        self.request_gate
            .ask(AcquireRequestPermit::new("accepted-connection"))
            .await
            .map_err(|error| AsyncListenerError::RequestGate {
                detail: error.to_string(),
            })
    }

    fn spawn_connection(&self, connection: AcceptedConnection<TokioTcpStream>) {
        let runtime = self.runtime.clone();
        let request_error_log = self.request_error_log.clone();
        tokio::spawn(async move {
            if let Err(error) = runtime.handle_connection(connection).await {
                request_error_log.report(&error);
            }
        });
    }
}
