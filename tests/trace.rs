use std::{fmt, os::unix::net::UnixStream, time::Duration};

use rkyv::{Archive, Deserialize, Serialize};
use tempfile::TempDir;
use triad_runtime::{
    TraceClient, TraceError, TraceEventFrame, TraceFrame, TraceLog, TraceSocketListener,
};

#[derive(Archive, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ExampleTraceEvent {
    name: String,
}

#[derive(Archive, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct CounterTraceEvent {
    count: u32,
}

impl ExampleTraceEvent {
    fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl CounterTraceEvent {
    fn new(count: u32) -> Self {
        Self { count }
    }
}

impl fmt::Display for ExampleTraceEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.name)
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

impl TraceEventFrame for CounterTraceEvent {
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
    log.record_result(ExampleTraceEvent::new("NexusEntered"))
        .expect("disabled trace sink is infallible");

    assert!(log.events().is_empty());
}

#[test]
fn record_result_reports_missing_socket_listener() {
    let directory = TempDir::new().expect("tempdir");
    let socket_path = directory.path().join("missing.sock");
    let log = TraceLog::socket(&socket_path);

    let error = log
        .record_result(ExampleTraceEvent::new("SignalAdmitted"))
        .expect_err("missing listener should be reported by fallible trace path");

    assert!(matches!(error, TraceError::Io(_)));
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
fn trace_frame_is_length_prefixed_binary_not_display_text() {
    let event = ExampleTraceEvent::new("SemaWriteApplied");
    let frame = TraceFrame::new(event.clone())
        .to_bytes()
        .expect("frame bytes");
    let archive_length = u32::from_be_bytes(frame[..4].try_into().expect("length prefix")) as usize;

    assert_eq!(
        archive_length,
        frame.len() - 4,
        "the frame is a four-byte length prefix followed by that many archive bytes"
    );
    assert_ne!(
        &frame[4..],
        event.to_string().as_bytes(),
        "the wire carries the rkyv archive, not the Display text"
    );

    let decoded = ExampleTraceEvent::from_trace_archive(&frame[4..]).expect("decode archive");
    assert_eq!(decoded, event);
}

#[test]
fn trace_runtime_is_generic_over_distinct_event_shapes() {
    let event = CounterTraceEvent::new(4);
    let frame = TraceFrame::new(event.clone())
        .to_bytes()
        .expect("frame bytes");
    let decoded = CounterTraceEvent::from_trace_archive(&frame[4..]).expect("decode archive");

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

#[test]
fn socket_listener_collects_until_expected_count() {
    let directory = TempDir::new().expect("tempdir");
    let socket_path = directory.path().join("trace.sock");
    let listener = TraceSocketListener::<ExampleTraceEvent>::bind(&socket_path).expect("bind");
    let log = TraceLog::socket(&socket_path);

    log.record_result(ExampleTraceEvent::new("SignalTriaged"))
        .expect("write first trace event");
    log.record_result(ExampleTraceEvent::new("NexusEntered"))
        .expect("write second trace event");

    assert_eq!(
        listener
            .collect_until_count(2, Duration::from_millis(100))
            .expect("collect until count"),
        vec![
            ExampleTraceEvent::new("SignalTriaged"),
            ExampleTraceEvent::new("NexusEntered"),
        ]
    );
}

#[test]
fn socket_listener_collect_until_count_returns_partial_on_timeout() {
    let directory = TempDir::new().expect("tempdir");
    let socket_path = directory.path().join("trace.sock");
    let listener = TraceSocketListener::<ExampleTraceEvent>::bind(&socket_path).expect("bind");
    let log = TraceLog::socket(&socket_path);

    log.record_result(ExampleTraceEvent::new("SignalTriaged"))
        .expect("write trace event");

    assert_eq!(
        listener
            .collect_until_count(2, Duration::from_millis(20))
            .expect("collect until timeout"),
        vec![ExampleTraceEvent::new("SignalTriaged")]
    );
}

#[test]
fn trace_client_disabled_collects_no_events() {
    let client = TraceClient::<ExampleTraceEvent>::disabled();

    assert!(client.events().expect("disabled client events").is_empty());
}

#[test]
fn trace_client_prints_typed_events_at_display_boundary() {
    let directory = TempDir::new().expect("tempdir");
    let socket_path = directory.path().join("trace.sock");
    let client = TraceClient::<ExampleTraceEvent>::listen(&socket_path, Duration::from_millis(100))
        .expect("trace client");
    let log = TraceLog::socket(&socket_path);
    let mut output = Vec::new();

    log.record_result(ExampleTraceEvent::new("SignalTriaged"))
        .expect("write first trace event");
    log.record_result(ExampleTraceEvent::new("NexusEntered"))
        .expect("write second trace event");

    client
        .print_events(&mut output)
        .expect("print trace events");

    assert_eq!(
        String::from_utf8(output).expect("trace output is UTF-8"),
        "SignalTriaged\nNexusEntered\n"
    );
}
