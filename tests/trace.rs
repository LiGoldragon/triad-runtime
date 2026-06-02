use std::{os::unix::net::UnixStream, time::Duration};

use rkyv::{Archive, Deserialize, Serialize};
use tempfile::TempDir;
use triad_runtime::{TraceError, TraceEventFrame, TraceFrame, TraceLog, TraceSocketListener};

#[derive(Archive, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ExampleTraceEvent {
    name: String,
}

impl ExampleTraceEvent {
    fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl TraceEventFrame for ExampleTraceEvent {
    fn to_trace_archive(&self) -> Result<Vec<u8>, TraceError> {
        rkyv::to_bytes::<rkyv::rancor::Error>(self)
            .map(|archive| archive.to_vec())
            .map_err(|_| TraceError::ArchiveEncode)
    }

    fn from_trace_archive(archive: &[u8]) -> Result<Self, TraceError> {
        rkyv::from_bytes::<Self, rkyv::rancor::Error>(archive)
            .map_err(|_| TraceError::ArchiveDecode)
    }
}

#[test]
fn recording_log_stores_events_in_memory() {
    let log = TraceLog::recording();

    log.record(ExampleTraceEvent::new("SignalAdmitted"));
    log.record(ExampleTraceEvent::new("NexusEntered"));

    assert_eq!(
        log.events(),
        vec![
            ExampleTraceEvent::new("SignalAdmitted"),
            ExampleTraceEvent::new("NexusEntered"),
        ]
    );
}

#[test]
fn disabled_log_drops_events() {
    let log = TraceLog::disabled();

    log.record(ExampleTraceEvent::new("SignalAdmitted"));

    assert!(log.events().is_empty());
}

#[test]
fn trace_frame_writes_length_prefixed_binary_archive() {
    let (mut writer, mut reader) = UnixStream::pair().expect("socket pair");
    let event = ExampleTraceEvent::new("SemaWriteApplied");

    TraceFrame::new(event.clone())
        .write_to(&mut writer)
        .expect("write trace frame");

    let decoded = TraceFrame::<ExampleTraceEvent>::read_from(&mut reader)
        .expect("read trace frame")
        .into_event();
    assert_eq!(decoded, event);
}

#[test]
fn socket_listener_collects_events_written_by_log() {
    let directory = TempDir::new().expect("tempdir");
    let socket_path = directory.path().join("trace.sock");
    let listener = TraceSocketListener::<ExampleTraceEvent>::bind(&socket_path).expect("bind");
    let log = TraceLog::socket(&socket_path);

    log.record(ExampleTraceEvent::new("SignalTriaged"));

    assert_eq!(
        listener
            .collect_for(Duration::from_millis(100))
            .expect("collect"),
        vec![ExampleTraceEvent::new("SignalTriaged")]
    );
}
