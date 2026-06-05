use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
};

use tempfile::TempDir;
use thiserror::Error;
use triad_runtime::{DaemonRuntime, RequestErrorLog, SingleListenerDaemon};

#[derive(Debug, Error)]
enum TestRuntimeError {
    #[error("test runtime request failed")]
    RequestFailed,
}

#[derive(Debug, Default)]
struct TestRuntime {
    events: Vec<&'static str>,
    fail_next_request: bool,
}

impl TestRuntime {
    fn fail_next_request(mut self) -> Self {
        self.fail_next_request = true;
        self
    }

    fn events(&self) -> &[&'static str] {
        &self.events
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
