use std::{
    fmt::{Display, Formatter},
    fs,
    io::{Read, Write},
    os::unix::fs::PermissionsExt,
    os::unix::net::UnixStream,
};

use tempfile::TempDir;
use thiserror::Error;
use triad_runtime::{
    DaemonRuntime, ListenerPollInterval, ListenerSocket, MultiListenerDaemon, MultiListenerRuntime,
    RequestErrorLog, SingleListenerDaemon, SocketMode,
};

#[derive(Debug, Error)]
enum TestRuntimeError {
    #[error("test runtime request failed")]
    RequestFailed,
}

#[derive(Debug, Default)]
struct TestRuntime {
    events: Vec<&'static str>,
    fail_next_request: bool,
    handled_stream_count: u8,
    stop_after_first_stream: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TestListener {
    Ordinary,
    Meta,
}

impl TestRuntime {
    fn fail_next_request(mut self) -> Self {
        self.fail_next_request = true;
        self
    }

    fn stop_after_first_stream(mut self) -> Self {
        self.stop_after_first_stream = true;
        self
    }

    fn events(&self) -> &[&'static str] {
        &self.events
    }
}

impl TestListener {
    fn response_offset(self) -> u8 {
        match self {
            Self::Ordinary => 10,
            Self::Meta => 20,
        }
    }
}

impl Display for TestListener {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ordinary => formatter.write_str("ordinary"),
            Self::Meta => formatter.write_str("meta"),
        }
    }
}

impl DaemonRuntime for TestRuntime {
    type RequestError = TestRuntimeError;
    type StartError = TestRuntimeError;
    type StopError = TestRuntimeError;

    fn start(&mut self) -> Result<(), Self::StartError> {
        self.events.push("start");
        Ok(())
    }

    fn stop(&mut self) -> Result<(), Self::StopError> {
        self.events.push("stop");
        Ok(())
    }

    fn handle_stream(&mut self, mut stream: UnixStream) -> Result<(), Self::RequestError> {
        self.events.push("handle");
        if self.fail_next_request {
            self.fail_next_request = false;
            return Err(TestRuntimeError::RequestFailed);
        }
        let mut byte = [0_u8; 1];
        stream.read_exact(&mut byte).expect("read request byte");
        stream
            .write_all(&[byte[0].saturating_add(1)])
            .expect("write response byte");
        Ok(())
    }
}

impl MultiListenerRuntime for TestRuntime {
    type Listener = TestListener;
    type RequestError = TestRuntimeError;
    type StartError = TestRuntimeError;
    type StopError = TestRuntimeError;

    fn start(&mut self) -> Result<(), Self::StartError> {
        self.events.push("start");
        Ok(())
    }

    fn should_continue(&self) -> bool {
        !self.stop_after_first_stream || self.handled_stream_count == 0
    }

    fn stop(&mut self) -> Result<(), Self::StopError> {
        self.events.push("stop");
        Ok(())
    }

    fn handle_stream(
        &mut self,
        listener: Self::Listener,
        mut stream: UnixStream,
    ) -> Result<(), Self::RequestError> {
        self.events.push(match listener {
            TestListener::Ordinary => "ordinary",
            TestListener::Meta => "meta",
        });
        if self.fail_next_request {
            self.fail_next_request = false;
            return Err(TestRuntimeError::RequestFailed);
        }
        self.handled_stream_count = self.handled_stream_count.saturating_add(1);
        let mut byte = [0_u8; 1];
        stream.read_exact(&mut byte).expect("read request byte");
        let response = byte[0]
            .saturating_add(listener.response_offset())
            .saturating_add(self.handled_stream_count);
        stream.write_all(&[response]).expect("write response byte");
        Ok(())
    }
}

struct TestClient {
    socket_path: std::path::PathBuf,
    request_byte: u8,
}

impl TestClient {
    fn new(socket_path: impl Into<std::path::PathBuf>, request_byte: u8) -> Self {
        Self {
            socket_path: socket_path.into(),
            request_byte,
        }
    }

    fn spawn(self) -> std::thread::JoinHandle<[u8; 1]> {
        std::thread::spawn(move || self.run())
    }

    fn run(self) -> [u8; 1] {
        let mut stream = UnixStream::connect(self.socket_path).expect("connect client");
        stream
            .write_all(&[self.request_byte])
            .expect("write request");
        let mut response = [0_u8; 1];
        stream.read_exact(&mut response).expect("read response");
        response
    }
}

#[test]
fn single_listener_daemon_binds_socket_and_serves_one_stream() {
    let directory = TempDir::new().expect("tempdir");
    let socket_path = directory.path().join("nested").join("component.sock");
    let runtime = TestRuntime::default();
    let mut daemon =
        SingleListenerDaemon::new(&socket_path, runtime, RequestErrorLog::new("test-daemon"))
            .bind()
            .expect("bind listener");
    daemon.start().expect("start runtime");

    let client_path = socket_path.clone();
    let client = std::thread::spawn(move || {
        let mut stream = UnixStream::connect(client_path).expect("connect client");
        stream.write_all(&[41]).expect("write request");
        let mut response = [0_u8; 1];
        stream.read_exact(&mut response).expect("read response");
        response
    });

    daemon.serve_next_stream().expect("serve one stream");
    daemon.stop().expect("stop runtime");

    assert_eq!(client.join().expect("client completes"), [42]);
    assert_eq!(daemon.runtime().events(), ["start", "handle", "stop"]);
}

#[test]
fn request_errors_are_logged_without_stopping_the_listener() {
    let directory = TempDir::new().expect("tempdir");
    let socket_path = directory.path().join("component.sock");
    let runtime = TestRuntime::default().fail_next_request();
    let mut daemon =
        SingleListenerDaemon::new(&socket_path, runtime, RequestErrorLog::new("test-daemon"))
            .bind()
            .expect("bind listener");

    let failing_client_path = socket_path.clone();
    let failing_client = std::thread::spawn(move || {
        let _stream = UnixStream::connect(failing_client_path).expect("connect failing client");
    });
    daemon
        .serve_next_stream()
        .expect("request error does not stop listener");
    failing_client.join().expect("failing client completes");

    let successful_client_path = socket_path.clone();
    let successful_client = std::thread::spawn(move || {
        let mut stream = UnixStream::connect(successful_client_path).expect("connect client");
        stream.write_all(&[9]).expect("write request");
        let mut response = [0_u8; 1];
        stream.read_exact(&mut response).expect("read response");
        response
    });
    daemon
        .serve_next_stream()
        .expect("listener continues after request error");

    assert_eq!(successful_client.join().expect("client completes"), [10]);
    assert_eq!(daemon.runtime().events(), ["handle", "handle"]);
}

#[test]
fn multi_listener_daemon_routes_two_sockets_through_one_runtime_owner() {
    let directory = TempDir::new().expect("tempdir");
    let ordinary_socket_path = directory.path().join("ordinary.sock");
    let meta_socket_path = directory.path().join("meta.sock");
    let runtime = TestRuntime::default();
    let listener_sockets = [
        ListenerSocket::new(TestListener::Ordinary, &ordinary_socket_path),
        ListenerSocket::new(TestListener::Meta, &meta_socket_path),
    ];
    let mut daemon = MultiListenerDaemon::new(
        listener_sockets,
        runtime,
        RequestErrorLog::new("test-daemon"),
    )
    .with_listener_poll_interval(ListenerPollInterval::from_millis(1))
    .bind()
    .expect("bind listeners");
    daemon.start().expect("start runtime");

    let ordinary_client = TestClient::new(&ordinary_socket_path, 1).spawn();
    daemon
        .serve_next_stream()
        .expect("serve ordinary stream through shared runtime");

    let meta_client = TestClient::new(&meta_socket_path, 1).spawn();
    daemon
        .serve_next_stream()
        .expect("serve meta stream through shared runtime");
    daemon.stop().expect("stop runtime");

    assert_eq!(ordinary_client.join().expect("ordinary completes"), [12]);
    assert_eq!(meta_client.join().expect("meta completes"), [23]);
    assert_eq!(
        daemon.runtime().events(),
        ["start", "ordinary", "meta", "stop"]
    );
}

#[test]
fn multi_listener_runtime_can_stop_the_stream_loop_after_a_request() {
    let directory = TempDir::new().expect("tempdir");
    let ordinary_socket_path = directory.path().join("ordinary.sock");
    let meta_socket_path = directory.path().join("meta.sock");
    let runtime = TestRuntime::default().stop_after_first_stream();
    let listener_sockets = [
        ListenerSocket::new(TestListener::Ordinary, &ordinary_socket_path),
        ListenerSocket::new(TestListener::Meta, &meta_socket_path),
    ];
    let mut daemon = MultiListenerDaemon::new(
        listener_sockets,
        runtime,
        RequestErrorLog::new("test-daemon"),
    )
    .with_listener_poll_interval(ListenerPollInterval::from_millis(1))
    .bind()
    .expect("bind listeners");
    daemon.start().expect("start runtime");

    let ordinary_client = TestClient::new(&ordinary_socket_path, 4).spawn();
    daemon
        .serve_streams()
        .expect("runtime stop request exits stream loop");
    daemon.stop().expect("stop runtime");

    assert_eq!(ordinary_client.join().expect("ordinary completes"), [15]);
    assert_eq!(daemon.runtime().events(), ["start", "ordinary", "stop"]);
}

#[test]
fn multi_listener_daemon_removes_socket_paths_on_drop() {
    let directory = TempDir::new().expect("tempdir");
    let ordinary_socket_path = directory.path().join("ordinary.sock");
    let meta_socket_path = directory.path().join("meta.sock");
    {
        let listener_sockets = [
            ListenerSocket::new(TestListener::Ordinary, &ordinary_socket_path),
            ListenerSocket::new(TestListener::Meta, &meta_socket_path),
        ];
        let _daemon = MultiListenerDaemon::new(
            listener_sockets,
            TestRuntime::default(),
            RequestErrorLog::new("test-daemon"),
        )
        .bind()
        .expect("bind listeners");

        assert!(ordinary_socket_path.exists());
        assert!(meta_socket_path.exists());
    }

    assert!(!ordinary_socket_path.exists());
    assert!(!meta_socket_path.exists());
}

#[test]
fn listener_bind_rejects_regular_file_at_socket_path_and_preserves_it() {
    let directory = TempDir::new().expect("tempdir");
    let socket_path = directory.path().join("component.sock");
    fs::write(&socket_path, "not a socket").expect("write sentinel regular file");

    let error = match SingleListenerDaemon::new(
        &socket_path,
        TestRuntime::default(),
        RequestErrorLog::new("test-daemon"),
    )
    .bind()
    {
        Ok(_) => panic!("regular file at socket path must be rejected"),
        Err(error) => error,
    };

    assert!(
        error.to_string().contains("regular file"),
        "error names the existing non-socket path kind: {error}"
    );
    assert_eq!(
        fs::read_to_string(&socket_path).expect("regular file is preserved"),
        "not a socket"
    );
}

#[test]
fn listener_bind_removes_stale_socket_file() {
    let directory = TempDir::new().expect("tempdir");
    let socket_path = directory.path().join("component.sock");
    let stale_listener =
        std::os::unix::net::UnixListener::bind(&socket_path).expect("bind stale socket file");
    drop(stale_listener);
    assert!(
        socket_path.exists(),
        "stale socket file remains after listener drop"
    );

    let _daemon = SingleListenerDaemon::new(
        &socket_path,
        TestRuntime::default(),
        RequestErrorLog::new("test-daemon"),
    )
    .bind()
    .expect("stale socket is removed before binding replacement listener");

    let file_type = fs::symlink_metadata(&socket_path)
        .expect("replacement socket metadata")
        .file_type();
    assert!(
        std::os::unix::fs::FileTypeExt::is_socket(&file_type),
        "replacement path is a unix socket"
    );
}

#[test]
fn listener_drop_preserves_non_socket_replacement() {
    let directory = TempDir::new().expect("tempdir");
    let socket_path = directory.path().join("component.sock");
    {
        let daemon = SingleListenerDaemon::new(
            &socket_path,
            TestRuntime::default(),
            RequestErrorLog::new("test-daemon"),
        )
        .bind()
        .expect("bind listener");
        fs::remove_file(&socket_path).expect("remove bound socket path");
        fs::write(&socket_path, "replacement").expect("write non-socket replacement");
        drop(daemon);
    }

    assert_eq!(
        fs::read_to_string(&socket_path).expect("replacement file is preserved"),
        "replacement"
    );
}

#[test]
fn multi_listener_socket_modes_are_applied_per_socket() {
    let directory = TempDir::new().expect("tempdir");
    let ordinary_socket_path = directory.path().join("ordinary.sock");
    let meta_socket_path = directory.path().join("meta.sock");
    let listener_sockets = [
        ListenerSocket::new(TestListener::Ordinary, &ordinary_socket_path)
            .with_socket_mode(SocketMode::new(0o600)),
        ListenerSocket::new(TestListener::Meta, &meta_socket_path)
            .with_socket_mode(SocketMode::new(0o660)),
    ];

    let _daemon = MultiListenerDaemon::new(
        listener_sockets,
        TestRuntime::default(),
        RequestErrorLog::new("test-daemon"),
    )
    .bind()
    .expect("bind listeners");

    let ordinary_mode = fs::metadata(&ordinary_socket_path)
        .expect("ordinary metadata")
        .permissions()
        .mode()
        & 0o777;
    let meta_mode = fs::metadata(&meta_socket_path)
        .expect("meta metadata")
        .permissions()
        .mode()
        & 0o777;

    assert_eq!(ordinary_mode, 0o600);
    assert_eq!(meta_mode, 0o660);
}
