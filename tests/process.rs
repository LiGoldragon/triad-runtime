use std::{
    fmt::{Display, Formatter},
    path::{Path, PathBuf},
    process::ExitCode,
};

use triad_runtime::{DaemonConfiguration, ExitReport, SocketMode};

const OWNER_ONLY_SOCKET_MODE: u32 = 0o600;

struct TestConfiguration {
    socket_path: PathBuf,
    meta_socket_path: Option<PathBuf>,
    database_path: PathBuf,
    trace_socket_path: Option<PathBuf>,
}

impl TestConfiguration {
    fn single_listener() -> Self {
        Self {
            socket_path: PathBuf::from("/run/component/working.sock"),
            meta_socket_path: None,
            database_path: PathBuf::from("/var/component/state.sema"),
            trace_socket_path: None,
        }
    }

    fn multi_listener_with_trace() -> Self {
        Self {
            socket_path: PathBuf::from("/run/component/working.sock"),
            meta_socket_path: Some(PathBuf::from("/run/component/meta.sock")),
            database_path: PathBuf::from("/var/component/state.sema"),
            trace_socket_path: Some(PathBuf::from("/run/component/trace.sock")),
        }
    }
}

impl DaemonConfiguration for TestConfiguration {
    fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    fn meta_socket_path(&self) -> Option<&Path> {
        self.meta_socket_path.as_deref()
    }

    fn database_path(&self) -> &Path {
        &self.database_path
    }

    fn trace_socket_path(&self) -> Option<&Path> {
        self.trace_socket_path.as_deref()
    }

    fn meta_socket_mode(&self) -> Option<SocketMode> {
        self.meta_socket_path
            .as_ref()
            .map(|_| SocketMode::new(OWNER_ONLY_SOCKET_MODE))
    }
}

#[derive(Debug)]
struct TestDaemonError;

impl Display for TestDaemonError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("test daemon failed")
    }
}

#[test]
fn single_listener_configuration_exposes_only_the_working_tier() {
    let configuration = TestConfiguration::single_listener();

    assert_eq!(
        configuration.socket_path(),
        Path::new("/run/component/working.sock")
    );
    assert_eq!(configuration.meta_socket_path(), None);
    assert_eq!(
        configuration.database_path(),
        Path::new("/var/component/state.sema")
    );
    assert_eq!(configuration.trace_socket_path(), None);
    assert_eq!(configuration.meta_socket_mode(), None);
}

#[test]
fn multi_listener_configuration_exposes_meta_and_trace_tiers() {
    let configuration = TestConfiguration::multi_listener_with_trace();

    assert_eq!(
        configuration.meta_socket_path(),
        Some(Path::new("/run/component/meta.sock"))
    );
    assert_eq!(
        configuration.trace_socket_path(),
        Some(Path::new("/run/component/trace.sock"))
    );
    assert_eq!(
        configuration.meta_socket_mode(),
        Some(SocketMode::new(OWNER_ONLY_SOCKET_MODE))
    );
}

#[test]
fn exit_report_maps_ok_to_success() {
    let report = ExitReport::new("test-daemon");

    let exit_code = report.from_result::<TestDaemonError>(Ok(()));

    assert_eq!(format!("{exit_code:?}"), format!("{:?}", ExitCode::SUCCESS));
    assert_eq!(report.process_name(), "test-daemon");
}

#[test]
fn exit_report_maps_err_to_failure() {
    let report = ExitReport::new("test-daemon");

    let exit_code = report.from_result(Err(TestDaemonError));

    assert_eq!(format!("{exit_code:?}"), format!("{:?}", ExitCode::FAILURE));
}
