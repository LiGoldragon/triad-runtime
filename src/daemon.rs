use std::{
    error::Error,
    fmt::{Debug, Display, Formatter},
    fs,
    os::unix::fs::PermissionsExt,
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    thread,
    time::Duration,
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

pub trait MultiListenerRuntime {
    type Listener: Clone + Display;
    type StartError;
    type StopError;
    type RequestError: Display;

    fn should_continue(&self) -> bool {
        true
    }

    fn start(&mut self) -> Result<(), Self::StartError>;

    fn stop(&mut self) -> Result<(), Self::StopError>;

    fn handle_stream(
        &mut self,
        listener: Self::Listener,
        stream: UnixStream,
    ) -> Result<(), Self::RequestError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestErrorLog {
    process_name: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SocketMode {
    bits: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ListenerPollInterval {
    duration: Duration,
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

#[derive(Debug)]
pub enum MultiListenerDaemonError<StartError, StopError> {
    Listener(ListenerError),
    Start(StartError),
    Stop(StopError),
}

pub struct SingleListenerDaemon<Runtime> {
    socket_path: PathBuf,
    socket_mode: Option<SocketMode>,
    request_error_log: RequestErrorLog,
    runtime: Runtime,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListenerSocket<Listener> {
    listener: Listener,
    socket_path: PathBuf,
    socket_mode: Option<SocketMode>,
}

pub struct MultiListenerDaemon<Runtime>
where
    Runtime: MultiListenerRuntime,
{
    listener_sockets: Vec<ListenerSocket<Runtime::Listener>>,
    listener_poll_interval: ListenerPollInterval,
    request_error_log: RequestErrorLog,
    runtime: Runtime,
}

pub struct BoundSingleListenerDaemon<Runtime> {
    listener: UnixListener,
    request_error_log: RequestErrorLog,
    runtime: Runtime,
}

pub struct BoundMultiListenerDaemon<Runtime>
where
    Runtime: MultiListenerRuntime,
{
    listeners: Vec<BoundListener<Runtime::Listener>>,
    listener_poll_interval: ListenerPollInterval,
    request_error_log: RequestErrorLog,
    runtime: Runtime,
}

struct BoundListener<Listener> {
    listener: Listener,
    unix_listener: UnixListener,
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

    pub fn report_for_listener<Listener, RequestError>(
        &self,
        listener: &Listener,
        error: &RequestError,
    ) where
        Listener: Display,
        RequestError: Display,
    {
        eprintln!("{}[{listener}]: {error}", self.process_name);
    }
}

impl SocketMode {
    pub const fn new(bits: u32) -> Self {
        Self { bits }
    }

    pub fn bits(self) -> u32 {
        self.bits
    }
}

impl Default for ListenerPollInterval {
    fn default() -> Self {
        Self::from_millis(10)
    }
}

impl ListenerPollInterval {
    pub const fn new(duration: Duration) -> Self {
        Self { duration }
    }

    pub const fn from_millis(milliseconds: u64) -> Self {
        Self::new(Duration::from_millis(milliseconds))
    }

    pub fn duration(self) -> Duration {
        self.duration
    }
}

impl<Listener> ListenerSocket<Listener> {
    pub fn new(listener: Listener, socket_path: impl Into<PathBuf>) -> Self {
        Self {
            listener,
            socket_path: socket_path.into(),
            socket_mode: None,
        }
    }

    pub fn with_socket_mode(mut self, socket_mode: SocketMode) -> Self {
        self.socket_mode = Some(socket_mode);
        self
    }

    pub fn listener(&self) -> &Listener {
        &self.listener
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn socket_mode(&self) -> Option<SocketMode> {
        self.socket_mode
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
            socket_mode: None,
            request_error_log,
            runtime,
        }
    }

    pub fn with_socket_mode(mut self, socket_mode: SocketMode) -> Self {
        self.socket_mode = Some(socket_mode);
        self
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn bind(self) -> Result<BoundSingleListenerDaemon<Runtime>, ListenerError> {
        let listener = BoundSocketPath::new(&self.socket_path, self.socket_mode).bind_listener()?;
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

impl<Runtime> MultiListenerDaemon<Runtime>
where
    Runtime: MultiListenerRuntime,
{
    pub fn new(
        listener_sockets: impl IntoIterator<Item = ListenerSocket<Runtime::Listener>>,
        runtime: Runtime,
        request_error_log: RequestErrorLog,
    ) -> Self {
        Self {
            listener_sockets: listener_sockets.into_iter().collect(),
            listener_poll_interval: ListenerPollInterval::default(),
            request_error_log,
            runtime,
        }
    }

    pub fn with_listener_poll_interval(
        mut self,
        listener_poll_interval: ListenerPollInterval,
    ) -> Self {
        self.listener_poll_interval = listener_poll_interval;
        self
    }

    pub fn listener_sockets(&self) -> &[ListenerSocket<Runtime::Listener>] {
        &self.listener_sockets
    }

    pub fn bind(self) -> Result<BoundMultiListenerDaemon<Runtime>, ListenerError> {
        let mut listeners = Vec::new();
        for listener_socket in self.listener_sockets {
            let unix_listener =
                BoundSocketPath::new(&listener_socket.socket_path, listener_socket.socket_mode)
                    .bind_listener()?;
            unix_listener.set_nonblocking(true)?;
            listeners.push(BoundListener::new(listener_socket.listener, unix_listener));
        }
        Ok(BoundMultiListenerDaemon {
            listeners,
            listener_poll_interval: self.listener_poll_interval,
            request_error_log: self.request_error_log,
            runtime: self.runtime,
        })
    }

    pub fn run(
        self,
    ) -> Result<(), MultiListenerDaemonError<Runtime::StartError, Runtime::StopError>> {
        let mut daemon = self.bind().map_err(MultiListenerDaemonError::Listener)?;
        daemon.start().map_err(MultiListenerDaemonError::Start)?;
        let accept_result = daemon.serve_streams();
        let stop_result = daemon.stop();
        match (accept_result, stop_result) {
            (Err(error), _) => Err(MultiListenerDaemonError::Listener(error)),
            (Ok(()), Err(error)) => Err(MultiListenerDaemonError::Stop(error)),
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

impl<Runtime> BoundMultiListenerDaemon<Runtime>
where
    Runtime: MultiListenerRuntime,
{
    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    pub fn into_runtime(self) -> Runtime {
        self.runtime
    }

    pub fn start(&mut self) -> Result<(), Runtime::StartError> {
        self.runtime.start()
    }

    pub fn stop(&mut self) -> Result<(), Runtime::StopError> {
        self.runtime.stop()
    }

    pub fn try_serve_next_stream(&mut self) -> Result<bool, ListenerError> {
        for index in 0..self.listeners.len() {
            let listener = self.listeners[index].listener().clone();
            if let Some(stream) = self.listeners[index].accept_next_stream()? {
                if let Err(error) = self.runtime.handle_stream(listener.clone(), stream) {
                    self.request_error_log
                        .report_for_listener(&listener, &error);
                }
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn serve_next_stream(&mut self) -> Result<(), ListenerError> {
        while self.runtime.should_continue() {
            if self.try_serve_next_stream()? {
                return Ok(());
            }
            thread::sleep(self.listener_poll_interval.duration());
        }
        Ok(())
    }

    pub fn serve_streams(&mut self) -> Result<(), ListenerError> {
        while self.runtime.should_continue() {
            self.serve_next_stream()?;
        }
        Ok(())
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

impl<Listener> BoundListener<Listener> {
    fn new(listener: Listener, unix_listener: UnixListener) -> Self {
        Self {
            listener,
            unix_listener,
        }
    }

    fn listener(&self) -> &Listener {
        &self.listener
    }

    fn accept_next_stream(&self) -> Result<Option<UnixStream>, ListenerError> {
        match self.unix_listener.accept() {
            Ok((stream, _)) => Ok(Some(stream)),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(ListenerError::Io(error)),
        }
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

impl<StartError, StopError> Display for MultiListenerDaemonError<StartError, StopError>
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

impl<StartError, StopError> Error for MultiListenerDaemonError<StartError, StopError>
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

impl<StartError, StopError> From<ListenerError>
    for MultiListenerDaemonError<StartError, StopError>
{
    fn from(error: ListenerError) -> Self {
        Self::Listener(error)
    }
}

struct BoundSocketPath<'path> {
    path: &'path Path,
    socket_mode: Option<SocketMode>,
}

impl<'path> BoundSocketPath<'path> {
    fn new(path: &'path Path, socket_mode: Option<SocketMode>) -> Self {
        Self { path, socket_mode }
    }

    fn bind_listener(&self) -> Result<UnixListener, ListenerError> {
        self.prepare()?;
        let listener = UnixListener::bind(self.path)?;
        self.apply_socket_mode()?;
        Ok(listener)
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

    fn apply_socket_mode(&self) -> Result<(), ListenerError> {
        if let Some(socket_mode) = self.socket_mode {
            fs::set_permissions(self.path, fs::Permissions::from_mode(socket_mode.bits()))?;
        }
        Ok(())
    }
}
