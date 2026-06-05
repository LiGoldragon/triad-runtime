use std::{
    error::Error,
    fmt::{Debug, Display, Formatter},
    fs,
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
};

use thiserror::Error;

pub trait DaemonRuntime {
    type StartError;
    type StopError;
    type RequestError: Display;

    fn start(&mut self) -> Result<(), Self::StartError>;

    fn stop(&mut self) -> Result<(), Self::StopError>;

    fn handle_stream(&mut self, stream: UnixStream) -> Result<(), Self::RequestError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestErrorLog {
    process_name: String,
}

#[derive(Debug, Error)]
pub enum ListenerError {
    #[error("listener IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug)]
pub enum SingleListenerDaemonError<StartError, StopError> {
    Listener(ListenerError),
    Start(StartError),
    Stop(StopError),
}

pub struct SingleListenerDaemon<Runtime> {
    socket_path: PathBuf,
    request_error_log: RequestErrorLog,
    runtime: Runtime,
}

pub struct BoundSingleListenerDaemon<Runtime> {
    listener: UnixListener,
    request_error_log: RequestErrorLog,
    runtime: Runtime,
}

impl RequestErrorLog {
    pub fn new(process_name: impl Into<String>) -> Self {
        Self {
            process_name: process_name.into(),
        }
    }

    pub fn process_name(&self) -> &str {
        &self.process_name
    }

    pub fn report<RequestError>(&self, error: &RequestError)
    where
        RequestError: Display,
    {
        eprintln!("{}: {error}", self.process_name);
    }
}

impl<Runtime> SingleListenerDaemon<Runtime> {
    pub fn new(
        socket_path: impl Into<PathBuf>,
        runtime: Runtime,
        request_error_log: RequestErrorLog,
    ) -> Self {
        Self {
            socket_path: socket_path.into(),
            request_error_log,
            runtime,
        }
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn bind(self) -> Result<BoundSingleListenerDaemon<Runtime>, ListenerError> {
        BoundSocketPath::new(&self.socket_path).prepare()?;
        let listener = UnixListener::bind(&self.socket_path)?;
        Ok(BoundSingleListenerDaemon {
            listener,
            request_error_log: self.request_error_log,
            runtime: self.runtime,
        })
    }
}

impl<Runtime> SingleListenerDaemon<Runtime>
where
    Runtime: DaemonRuntime,
{
    pub fn run(
        self,
    ) -> Result<(), SingleListenerDaemonError<Runtime::StartError, Runtime::StopError>> {
        let mut daemon = self.bind().map_err(SingleListenerDaemonError::Listener)?;
        daemon.start().map_err(SingleListenerDaemonError::Start)?;
        let accept_result = daemon.serve_streams();
        let stop_result = daemon.stop();
        match (accept_result, stop_result) {
            (Err(error), _) => Err(SingleListenerDaemonError::Listener(error)),
            (Ok(()), Err(error)) => Err(SingleListenerDaemonError::Stop(error)),
            (Ok(()), Ok(())) => Ok(()),
        }
    }
}

impl<Runtime> BoundSingleListenerDaemon<Runtime> {
    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    pub fn into_runtime(self) -> Runtime {
        self.runtime
    }
}

impl<Runtime> BoundSingleListenerDaemon<Runtime>
where
    Runtime: DaemonRuntime,
{
    pub fn start(&mut self) -> Result<(), Runtime::StartError> {
        self.runtime.start()
    }

    pub fn stop(&mut self) -> Result<(), Runtime::StopError> {
        self.runtime.stop()
    }

    pub fn serve_next_stream(&mut self) -> Result<(), ListenerError> {
        let (stream, _) = self.listener.accept()?;
        if let Err(error) = self.runtime.handle_stream(stream) {
            self.request_error_log.report(&error);
        }
        Ok(())
    }

    pub fn serve_streams(&mut self) -> Result<(), ListenerError> {
        for stream in self.listener.incoming() {
            match stream {
                Ok(stream) => {
                    if let Err(error) = self.runtime.handle_stream(stream) {
                        self.request_error_log.report(&error);
                    }
                }
                Err(error) => return Err(ListenerError::Io(error)),
            }
        }
        Ok(())
    }
}

impl<StartError, StopError> Display for SingleListenerDaemonError<StartError, StopError>
where
    StartError: Display,
    StopError: Display,
{
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Listener(error) => write!(formatter, "{error}"),
            Self::Start(error) => write!(formatter, "daemon runtime start error: {error}"),
            Self::Stop(error) => write!(formatter, "daemon runtime stop error: {error}"),
        }
    }
}

impl<StartError, StopError> Error for SingleListenerDaemonError<StartError, StopError>
where
    StartError: Error + 'static,
    StopError: Error + 'static,
{
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Listener(error) => Some(error),
            Self::Start(error) => Some(error),
            Self::Stop(error) => Some(error),
        }
    }
}

impl<StartError, StopError> From<ListenerError>
    for SingleListenerDaemonError<StartError, StopError>
{
    fn from(error: ListenerError) -> Self {
        Self::Listener(error)
    }
}

struct BoundSocketPath<'path> {
    path: &'path Path,
}

impl<'path> BoundSocketPath<'path> {
    fn new(path: &'path Path) -> Self {
        Self { path }
    }

    fn prepare(&self) -> Result<(), ListenerError> {
        self.create_parent_directory()?;
        self.remove_stale_socket()
    }

    fn create_parent_directory(&self) -> Result<(), ListenerError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    fn remove_stale_socket(&self) -> Result<(), ListenerError> {
        match fs::remove_file(self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(ListenerError::Io(error)),
        }
    }
}
