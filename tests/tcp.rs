use std::net::SocketAddr;
use std::sync::Arc;

use thiserror::Error;
use tokio::net::TcpStream;
use triad_runtime::{
    AcceptedConnection, AsyncConnectionRuntime, FrameBody, FrameError, LengthPrefixedCodec,
    PeerIdentity, RequestConcurrencyLimit, RequestErrorLog, TcpListenerDaemon,
};

#[derive(Debug, Error)]
enum TestRuntimeError {
    #[error("test frame error: {0}")]
    Frame(#[from] FrameError),
}

/// Reads one length-prefixed frame per connection, records the peer identity,
/// and replies with the reversed body through the same codec.
#[derive(Clone, Debug)]
struct ReversingFrameRuntime {
    codec: LengthPrefixedCodec,
    observed_peers: Arc<tokio::sync::Mutex<Vec<PeerIdentity>>>,
}

impl ReversingFrameRuntime {
    fn new() -> Self {
        Self {
            codec: LengthPrefixedCodec::default(),
            observed_peers: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        }
    }

    async fn observed_peers(&self) -> Vec<PeerIdentity> {
        self.observed_peers.lock().await.clone()
    }
}

impl AsyncConnectionRuntime<TcpStream> for ReversingFrameRuntime {
    type Error = TestRuntimeError;

    async fn handle_connection(
        &self,
        mut connection: AcceptedConnection<TcpStream>,
    ) -> Result<(), Self::Error> {
        self.observed_peers
            .lock()
            .await
            .push(*connection.context().peer());
        let body = self.codec.read_body_async(connection.stream_mut()).await?;
        let mut reversed = body.into_bytes();
        reversed.reverse();
        self.codec
            .write_body_async(connection.stream_mut(), &FrameBody::new(reversed))
            .await?;
        Ok(())
    }
}

struct FrameClient {
    address: SocketAddr,
    request: Vec<u8>,
}

struct FrameReply {
    client_address: SocketAddr,
    body: Vec<u8>,
}

impl FrameClient {
    fn new(address: SocketAddr, request: impl Into<Vec<u8>>) -> Self {
        Self {
            address,
            request: request.into(),
        }
    }

    async fn run(self) -> Result<FrameReply, TestRuntimeError> {
        let codec = LengthPrefixedCodec::default();
        let mut stream = TcpStream::connect(self.address)
            .await
            .map_err(FrameError::Io)?;
        let client_address = stream.local_addr().map_err(FrameError::Io)?;
        codec
            .write_body_async(&mut stream, &FrameBody::new(self.request))
            .await?;
        let body = codec.read_body_async(&mut stream).await?.into_bytes();
        Ok(FrameReply {
            client_address,
            body,
        })
    }
}

fn loopback_any_port() -> SocketAddr {
    "127.0.0.1:0".parse().expect("parse loopback address")
}

#[tokio::test]
async fn tcp_listener_round_trips_length_prefixed_frames() {
    let runtime = ReversingFrameRuntime::new();
    let daemon = TcpListenerDaemon::new(
        loopback_any_port(),
        runtime.clone(),
        RequestErrorLog::new("tcp-test"),
    )
    .with_concurrency_limit(RequestConcurrencyLimit::new(2))
    .bind()
    .await
    .expect("bind tcp listener");
    let address = daemon.local_address().expect("bound local address");

    let clients = [
        tokio::spawn(FrameClient::new(address, [1, 2, 3]).run()),
        tokio::spawn(FrameClient::new(address, *b"frame").run()),
    ];
    for _ in 0..clients.len() {
        daemon
            .serve_next_connection()
            .await
            .expect("serve next tcp connection");
    }

    let mut replies = Vec::new();
    for client in clients {
        replies.push(
            client
                .await
                .expect("client joins")
                .expect("client receives reply")
                .body,
        );
    }
    replies.sort();

    assert_eq!(replies, [vec![3, 2, 1], b"emarf".to_vec()]);
    daemon.stop().await.expect("stop tcp listener");
}

#[tokio::test]
async fn tcp_peer_identity_is_the_remote_address() {
    let runtime = ReversingFrameRuntime::new();
    let observed_runtime = runtime.clone();
    let daemon = TcpListenerDaemon::new(
        loopback_any_port(),
        runtime,
        RequestErrorLog::new("tcp-test"),
    )
    .bind()
    .await
    .expect("bind tcp listener");
    let address = daemon.local_address().expect("bound local address");

    let client = tokio::spawn(FrameClient::new(address, [9]).run());
    daemon
        .serve_next_connection()
        .await
        .expect("serve next tcp connection");
    let reply = client
        .await
        .expect("client joins")
        .expect("client receives reply");

    let observed_peers = observed_runtime.observed_peers().await;
    assert_eq!(observed_peers.len(), 1);
    let peer = observed_peers[0];
    assert_eq!(peer.tcp_address(), Some(reply.client_address));
    assert_eq!(
        peer.unix_credentials(),
        None,
        "a tcp peer never carries kernel-vouched unix credentials"
    );
    assert!(matches!(peer, PeerIdentity::Tcp(_)));
    daemon.stop().await.expect("stop tcp listener");
}

#[tokio::test]
async fn dropping_the_bound_listener_releases_the_address() {
    let first = TcpListenerDaemon::new(
        loopback_any_port(),
        ReversingFrameRuntime::new(),
        RequestErrorLog::new("tcp-test"),
    )
    .bind()
    .await
    .expect("bind first tcp listener");
    let address = first.local_address().expect("bound local address");

    drop(first);

    let second = TcpListenerDaemon::new(
        address,
        ReversingFrameRuntime::new(),
        RequestErrorLog::new("tcp-test"),
    )
    .bind()
    .await
    .expect("rebinding the dropped listener's address succeeds");
    assert_eq!(
        second.local_address().expect("bound local address"),
        address
    );
}
