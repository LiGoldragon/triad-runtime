use std::{
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use thiserror::Error;

const LENGTH_PREFIX_BYTE_COUNT: usize = 4;

pub trait TraceEventFrame: Clone + Send + 'static {
    fn to_trace_archive(&self) -> Result<Vec<u8>, TraceError>;

    fn from_trace_archive(archive: &[u8]) -> Result<Self, TraceError>
    where
        Self: Sized;
}

#[derive(Clone, Debug)]
pub struct TraceFrame<Event>
where
    Event: TraceEventFrame,
{
    event: Event,
}

#[derive(Clone, Debug)]
pub struct TraceLog<Event>
where
    Event: TraceEventFrame,
{
    destination: TraceDestination<Event>,
}

#[derive(Clone, Debug)]
enum TraceDestination<Event>
where
    Event: TraceEventFrame,
{
    Disabled,
    Recording(Arc<Mutex<Vec<Event>>>),
    Socket(TraceSocketPath),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceSocketPath {
    path: PathBuf,
}

pub struct TraceSocketListener<Event>
where
    Event: TraceEventFrame,
{
    listener: UnixListener,
    path: PathBuf,
    events: std::marker::PhantomData<Event>,
}

#[derive(Debug, Error)]
pub enum TraceError {
    #[error("trace IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("failed to encode trace event")]
    ArchiveEncode,

    #[error("failed to decode trace event")]
    ArchiveDecode,

    #[error("trace frame is too large: {found} bytes")]
    FrameTooLarge { found: usize },
}

impl<Event> Default for TraceLog<Event>
where
    Event: TraceEventFrame,
{
    fn default() -> Self {
        Self::recording()
    }
}

impl<Event> TraceFrame<Event>
where
    Event: TraceEventFrame,
{
    pub fn new(event: Event) -> Self {
        Self { event }
    }

    pub fn event(&self) -> &Event {
        &self.event
    }

    pub fn into_event(self) -> Event {
        self.event
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, TraceError> {
        let archive = self.event.to_trace_archive()?;
        let length = u32::try_from(archive.len()).map_err(|_| TraceError::FrameTooLarge {
            found: archive.len(),
        })?;
        let mut frame = Vec::with_capacity(LENGTH_PREFIX_BYTE_COUNT + archive.len());
        frame.extend_from_slice(&length.to_be_bytes());
        frame.extend_from_slice(&archive);
        Ok(frame)
    }

    pub fn write_to(&self, stream: &mut UnixStream) -> Result<(), TraceError> {
        stream.write_all(&self.to_bytes()?)?;
        Ok(())
    }

    pub fn read_from(stream: &mut UnixStream) -> Result<Self, TraceError> {
        let mut length_bytes = [0_u8; LENGTH_PREFIX_BYTE_COUNT];
        stream.read_exact(&mut length_bytes)?;
        let length = u32::from_be_bytes(length_bytes) as usize;
        let mut archive = vec![0_u8; length];
        stream.read_exact(&mut archive)?;
        let event = Event::from_trace_archive(&archive)?;
        Ok(Self::new(event))
    }
}

impl<Event> TraceLog<Event>
where
    Event: TraceEventFrame,
{
    pub fn disabled() -> Self {
        Self {
            destination: TraceDestination::Disabled,
        }
    }

    pub fn recording() -> Self {
        Self {
            destination: TraceDestination::Recording(Arc::new(Mutex::new(Vec::new()))),
        }
    }

    pub fn socket(path: impl Into<PathBuf>) -> Self {
        Self {
            destination: TraceDestination::Socket(TraceSocketPath::new(path)),
        }
    }

    pub fn events(&self) -> Vec<Event> {
        match &self.destination {
            TraceDestination::Recording(events) => events.lock().expect("trace event lock").clone(),
            TraceDestination::Disabled | TraceDestination::Socket(_) => Vec::new(),
        }
    }

    pub fn record(&self, event: Event) {
        if let Err(error) = self.record_result(event) {
            eprintln!("triad-runtime trace: {error}");
        }
    }

    pub fn record_result(&self, event: Event) -> Result<(), TraceError> {
        match &self.destination {
            TraceDestination::Disabled => Ok(()),
            TraceDestination::Recording(events) => {
                events.lock().expect("trace event lock").push(event);
                Ok(())
            }
            TraceDestination::Socket(path) => path.write_event(&event),
        }
    }
}

impl TraceSocketPath {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn as_path(&self) -> &Path {
        &self.path
    }

    pub fn write_event<Event>(&self, event: &Event) -> Result<(), TraceError>
    where
        Event: TraceEventFrame,
    {
        let mut stream = UnixStream::connect(&self.path)?;
        TraceFrame::new(event.clone()).write_to(&mut stream)
    }
}

impl<Event> TraceSocketListener<Event>
where
    Event: TraceEventFrame,
{
    pub fn bind(path: impl Into<PathBuf>) -> Result<Self, TraceError> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(TraceError::Io(error)),
        }
        let listener = UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            path,
            events: std::marker::PhantomData,
        })
    }

    pub fn collect_for(&self, duration: Duration) -> Result<Vec<Event>, TraceError> {
        let deadline = Instant::now() + duration;
        let mut events = Vec::new();
        while Instant::now() < deadline {
            self.collect_available_event(&mut events)?;
        }
        Ok(events)
    }

    pub fn collect_until_count(
        &self,
        expected_count: usize,
        timeout: Duration,
    ) -> Result<Vec<Event>, TraceError> {
        let deadline = Instant::now() + timeout;
        let mut events = Vec::new();
        while events.len() < expected_count && Instant::now() < deadline {
            self.collect_available_event(&mut events)?;
        }
        Ok(events)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn collect_available_event(&self, events: &mut Vec<Event>) -> Result<(), TraceError> {
        match self.listener.accept() {
            Ok((mut stream, _address)) => {
                events.push(TraceFrame::<Event>::read_from(&mut stream)?.into_event());
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(TraceError::Io(error)),
        }
        Ok(())
    }
}

impl<Event> Drop for TraceSocketListener<Event>
where
    Event: TraceEventFrame,
{
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
