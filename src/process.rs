//! Component-agnostic process-edge runtime for emitted daemons.
//!
//! The emitted daemon module (schema-rust `RustEmissionTarget::Daemon`)
//! reads its socket layout through [`BindingSurface`] and turns its top
//! `Result` into a process exit code through [`ExitReport`]. Both surfaces are
//! deliberately free of component meaning: the trait names the uniform socket
//! accessors every triad daemon configuration exposes, and the exit reporter
//! owns only the process name it prints with.

use std::{
    env,
    fmt::{Display, Formatter},
    fs,
    net::SocketAddr,
    os::unix::{fs::FileTypeExt, net::UnixStream},
    path::{Component, Path, PathBuf},
    process::ExitCode,
};

use crate::{RequestConcurrencyLimit, SocketMode};

/// The kernel-vouched `SO_PEERCRED` credential triple of a Unix-socket peer.
///
/// The credentials are obtained through rustix's safe
/// [`rustix::net::sockopt::socket_peercred`] wrapper — no raw `getsockopt` and
/// no `unsafe` in this crate, so `triad-runtime` keeps `unsafe_code = "forbid"`.
/// The standard library's own `UnixStream::peer_cred` is still unstable on the
/// stable toolchain (`peer_credentials_unix_socket`), which is why the safe
/// rustix wrapper carries this instead. `rustix` exposes a real `Pid`, so the
/// peer process identifier is carried as a plain `i32` instead of a fake
/// optional value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnixCredentials {
    user_id: u32,
    group_id: u32,
    process_id: i32,
}

impl UnixCredentials {
    /// Construct credentials from explicit values. The `from_stream` paths are
    /// the production source; this constructor exists so tests and in-process
    /// callers can build credentials without a real socket.
    pub const fn new(user_id: u32, group_id: u32, process_id: i32) -> Self {
        Self {
            user_id,
            group_id,
            process_id,
        }
    }

    /// Read the kernel-vouched peer credentials of an accepted stream.
    ///
    /// Wraps [`rustix::net::sockopt::socket_peercred`], which performs the
    /// `getsockopt(SO_PEERCRED)` query internally; an operating-system failure to
    /// read the credentials surfaces as a [`std::io::Error`] the emitted spine
    /// lifts into its typed daemon error.
    pub fn from_stream(stream: &UnixStream) -> std::io::Result<Self> {
        let credentials = rustix::net::sockopt::socket_peercred(stream)?;
        Ok(Self {
            user_id: credentials.uid.as_raw(),
            group_id: credentials.gid.as_raw(),
            process_id: credentials.pid.as_raw_pid(),
        })
    }

    /// Read the kernel-vouched peer credentials of an accepted Tokio Unix
    /// stream. Async listener daemons use this path before they hand the stream
    /// into an asynchronous request driver.
    pub fn from_tokio_stream(stream: &tokio::net::UnixStream) -> std::io::Result<Self> {
        let credentials = rustix::net::sockopt::socket_peercred(stream)?;
        Ok(Self {
            user_id: credentials.uid.as_raw(),
            group_id: credentials.gid.as_raw(),
            process_id: credentials.pid.as_raw_pid(),
        })
    }

    /// The peer's Unix user identifier (`uid`).
    pub fn user_id(&self) -> u32 {
        self.user_id
    }

    /// The peer's Unix group identifier (`gid`).
    pub fn group_id(&self) -> u32 {
        self.group_id
    }

    /// The peer's process identifier (`pid`).
    pub fn process_id(&self) -> i32 {
        self.process_id
    }
}

/// The transport-level identity of an accepted peer — a closed sum.
///
/// A Unix-socket peer is kernel-vouched: the operating system supplies the
/// `SO_PEERCRED` uid/gid/pid triple and no payload claim can forge it. A TCP
/// peer carries only its remote socket address; the runtime asserts nothing
/// stronger, and any further trust (a tailnet boundary, mutual authentication)
/// is the deployment's and the component's concern. The sum is closed on
/// purpose: ssh-forwarded sockets are rejected as a transport shape, so no
/// third "forwarded" identity exists.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeerIdentity {
    /// A same-host Unix-socket peer with kernel-vouched credentials.
    Unix(UnixCredentials),
    /// A TCP peer known only by its remote socket address.
    Tcp(SocketAddr),
}

impl PeerIdentity {
    /// The kernel-vouched credentials, when the peer is a Unix-socket peer.
    /// `None` for TCP peers — there is no credential to pretend to.
    pub fn unix_credentials(&self) -> Option<&UnixCredentials> {
        match self {
            Self::Unix(credentials) => Some(credentials),
            Self::Tcp(_) => None,
        }
    }

    /// The remote socket address, when the peer is a TCP peer. `None` for
    /// Unix-socket peers.
    pub fn tcp_address(&self) -> Option<SocketAddr> {
        match self {
            Self::Unix(_) => None,
            Self::Tcp(address) => Some(*address),
        }
    }
}

/// The per-connection trust context of an accepted stream.
///
/// The schema-rust emitted daemon module reads this once per accepted
/// working connection and threads it into the component's working-input hook,
/// so a component can mint an origin (owner vs non-owner local user vs internal
/// component instance vs remote host) from the transport trust boundary rather
/// than trusting payload claims. The carried [`PeerIdentity`] says exactly what
/// the transport vouches for; the component owns the trust policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectionContext {
    peer: PeerIdentity,
}

impl ConnectionContext {
    /// Read the kernel-vouched peer credentials of an accepted Unix stream.
    pub fn from_stream(stream: &UnixStream) -> std::io::Result<Self> {
        Ok(Self::from(UnixCredentials::from_stream(stream)?))
    }

    /// Read the kernel-vouched peer credentials of an accepted Tokio Unix
    /// stream. Async listener daemons use this path before they hand the stream
    /// into an asynchronous request driver.
    pub fn from_tokio_stream(stream: &tokio::net::UnixStream) -> std::io::Result<Self> {
        Ok(Self::from(UnixCredentials::from_tokio_stream(stream)?))
    }

    /// Read the remote address of a connected Tokio TCP stream. The TCP accept
    /// loop builds its context from the address the listener already returned;
    /// this path serves callers holding only the stream.
    pub fn from_tcp_stream(stream: &tokio::net::TcpStream) -> std::io::Result<Self> {
        Ok(Self::from(stream.peer_addr()?))
    }

    /// The transport-level peer identity.
    pub fn peer(&self) -> &PeerIdentity {
        &self.peer
    }

    /// The kernel-vouched credentials, when the peer is a Unix-socket peer.
    pub fn unix_credentials(&self) -> Option<&UnixCredentials> {
        self.peer.unix_credentials()
    }

    /// The remote socket address, when the peer is a TCP peer.
    pub fn tcp_address(&self) -> Option<SocketAddr> {
        self.peer.tcp_address()
    }
}

impl From<PeerIdentity> for ConnectionContext {
    fn from(peer: PeerIdentity) -> Self {
        Self { peer }
    }
}

impl From<UnixCredentials> for ConnectionContext {
    fn from(credentials: UnixCredentials) -> Self {
        Self::from(PeerIdentity::Unix(credentials))
    }
}

impl From<SocketAddr> for ConnectionContext {
    fn from(address: SocketAddr) -> Self {
        Self::from(PeerIdentity::Tcp(address))
    }
}

/// The source that supplied a runtime path.
///
/// The value is carried for environment-derived paths so diagnostics can name
/// the exact variable and content that failed validation instead of losing the
/// bad input behind a synthesized default path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SocketPathSource {
    /// A component configuration field decoded from the daemon startup record.
    ConfigurationField { field: String },
    /// A built-in default path selected by the caller.
    Default { field: String },
    /// An explicit socket path override from an environment variable.
    EnvironmentOverride { variable: String, value: String },
    /// A default socket path derived from a runtime-directory environment
    /// variable such as `XDG_RUNTIME_DIR`.
    RuntimeDirectory {
        variable: String,
        value: String,
        field: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimePathErrorKind {
    Empty,
    Relative,
    ParentDirectory,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("runtime path {kind} for {path_source}: {value}")]
pub struct RuntimePathError {
    path_source: SocketPathSource,
    value: String,
    kind: RuntimePathErrorKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AbsoluteRuntimePath {
    path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeSocketPath {
    path: AbsoluteRuntimePath,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SocketPathSelection {
    socket_path: RuntimeSocketPath,
    source: SocketPathSource,
}

impl SocketPathSource {
    pub fn configuration_field(field: impl Into<String>) -> Self {
        Self::ConfigurationField {
            field: field.into(),
        }
    }

    pub fn default(field: impl Into<String>) -> Self {
        Self::Default {
            field: field.into(),
        }
    }

    pub fn environment_override(variable: impl Into<String>, value: impl Into<String>) -> Self {
        Self::EnvironmentOverride {
            variable: variable.into(),
            value: value.into(),
        }
    }

    pub fn runtime_directory(
        variable: impl Into<String>,
        value: impl Into<String>,
        field: impl Into<String>,
    ) -> Self {
        Self::RuntimeDirectory {
            variable: variable.into(),
            value: value.into(),
            field: field.into(),
        }
    }
}

impl Display for SocketPathSource {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConfigurationField { field } => write!(formatter, "configuration field {field}"),
            Self::Default { field } => write!(formatter, "default {field}"),
            Self::EnvironmentOverride { variable, value } => {
                write!(formatter, "environment override {variable}={value:?}")
            }
            Self::RuntimeDirectory {
                variable,
                value,
                field,
            } => write!(
                formatter,
                "runtime-directory default {field} from {variable}={value:?}"
            ),
        }
    }
}

impl Display for RuntimePathErrorKind {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => formatter.write_str("is empty"),
            Self::Relative => formatter.write_str("is relative"),
            Self::ParentDirectory => formatter.write_str("contains parent-directory traversal"),
        }
    }
}

impl RuntimePathError {
    fn new(source: SocketPathSource, path: &Path, kind: RuntimePathErrorKind) -> Self {
        Self {
            path_source: source,
            value: path.display().to_string(),
            kind,
        }
    }

    pub fn source(&self) -> &SocketPathSource {
        &self.path_source
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn kind(&self) -> RuntimePathErrorKind {
        self.kind
    }
}

impl AbsoluteRuntimePath {
    pub fn try_new(
        source: SocketPathSource,
        path: impl Into<PathBuf>,
    ) -> Result<Self, RuntimePathError> {
        let path = path.into();
        if path.as_os_str().is_empty() {
            return Err(RuntimePathError::new(
                source,
                &path,
                RuntimePathErrorKind::Empty,
            ));
        }
        if !path.is_absolute() {
            return Err(RuntimePathError::new(
                source,
                &path,
                RuntimePathErrorKind::Relative,
            ));
        }
        if path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(RuntimePathError::new(
                source,
                &path,
                RuntimePathErrorKind::ParentDirectory,
            ));
        }
        Ok(Self { path })
    }

    pub fn as_path(&self) -> &Path {
        &self.path
    }

    pub fn into_path_buf(self) -> PathBuf {
        self.path
    }
}

impl AsRef<Path> for AbsoluteRuntimePath {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

impl RuntimeSocketPath {
    pub fn try_new(
        source: SocketPathSource,
        path: impl Into<PathBuf>,
    ) -> Result<Self, RuntimePathError> {
        Ok(Self {
            path: AbsoluteRuntimePath::try_new(source, path)?,
        })
    }

    pub fn as_path(&self) -> &Path {
        self.path.as_path()
    }

    pub fn into_path_buf(self) -> PathBuf {
        self.path.into_path_buf()
    }
}

impl AsRef<Path> for RuntimeSocketPath {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

impl SocketPathSelection {
    pub fn from_default(
        field: impl Into<String>,
        path: impl Into<PathBuf>,
    ) -> Result<Self, RuntimePathError> {
        let source = SocketPathSource::default(field);
        Self::from_source(source, path)
    }

    pub fn from_environment_override(
        variable: impl Into<String>,
    ) -> Result<Option<Self>, RuntimePathError> {
        let variable = variable.into();
        let Some(value) = env::var_os(&variable) else {
            return Ok(None);
        };
        let value = value.to_string_lossy().into_owned();
        let source = SocketPathSource::environment_override(variable, value.clone());
        Self::from_source(source, PathBuf::from(value)).map(Some)
    }

    pub fn from_runtime_directory(
        variable: impl Into<String>,
        field: impl Into<String>,
        socket_path_inside_runtime_directory: impl AsRef<Path>,
    ) -> Result<Self, RuntimePathError> {
        let variable = variable.into();
        let value = env::var_os(&variable)
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        let source = SocketPathSource::runtime_directory(variable, value.clone(), field);
        if value.is_empty() {
            return Err(RuntimePathError::new(
                source,
                &PathBuf::from(value),
                RuntimePathErrorKind::Empty,
            ));
        }
        let path = PathBuf::from(&value).join(socket_path_inside_runtime_directory);
        Self::from_source(source, path)
    }

    pub fn select_environment_override(
        self,
        variable: impl Into<String>,
    ) -> Result<Self, RuntimePathError> {
        Ok(Self::from_environment_override(variable)?.unwrap_or(self))
    }

    pub fn from_source(
        source: SocketPathSource,
        path: impl Into<PathBuf>,
    ) -> Result<Self, RuntimePathError> {
        Ok(Self {
            socket_path: RuntimeSocketPath::try_new(source.clone(), path)?,
            source,
        })
    }

    pub fn socket_path(&self) -> &RuntimeSocketPath {
        &self.socket_path
    }

    pub fn source(&self) -> &SocketPathSource {
        &self.source
    }

    pub fn as_path(&self) -> &Path {
        self.socket_path.as_path()
    }

    pub fn into_path_buf(self) -> PathBuf {
        self.socket_path.into_path_buf()
    }
}

impl AsRef<Path> for SocketPathSelection {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExistingRuntimePathKind {
    Socket,
    RegularFile,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeSocketFileError {
    #[error("invalid runtime socket path: {0}")]
    RuntimePath(#[from] RuntimePathError),

    #[error("runtime socket file IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("refuse to replace non-socket runtime path {} ({kind})", path.display())]
    NonSocketPath {
        path: PathBuf,
        kind: ExistingRuntimePathKind,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeSocketFile {
    path: PathBuf,
}

impl Display for ExistingRuntimePathKind {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Socket => formatter.write_str("socket"),
            Self::RegularFile => formatter.write_str("regular file"),
            Self::Directory => formatter.write_str("directory"),
            Self::Symlink => formatter.write_str("symlink"),
            Self::Other => formatter.write_str("other file type"),
        }
    }
}

impl ExistingRuntimePathKind {
    fn from_file_type(file_type: fs::FileType) -> Self {
        if file_type.is_socket() {
            Self::Socket
        } else if file_type.is_file() {
            Self::RegularFile
        } else if file_type.is_dir() {
            Self::Directory
        } else if file_type.is_symlink() {
            Self::Symlink
        } else {
            Self::Other
        }
    }

    fn is_socket(self) -> bool {
        matches!(self, Self::Socket)
    }
}

impl RuntimeSocketFile {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn prepare(&self) -> Result<(), RuntimeSocketFileError> {
        AbsoluteRuntimePath::try_new(
            SocketPathSource::default("listener socket path"),
            self.path.clone(),
        )?;
        self.create_parent_directory()?;
        self.remove_stale_socket()
    }

    pub fn remove_socket_if_current(&self) {
        let Ok(metadata) = fs::symlink_metadata(&self.path) else {
            return;
        };
        if ExistingRuntimePathKind::from_file_type(metadata.file_type()).is_socket() {
            let _ = fs::remove_file(&self.path);
        }
    }

    fn create_parent_directory(&self) -> Result<(), RuntimeSocketFileError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    fn remove_stale_socket(&self) -> Result<(), RuntimeSocketFileError> {
        match fs::symlink_metadata(&self.path) {
            Ok(metadata) => {
                let kind = ExistingRuntimePathKind::from_file_type(metadata.file_type());
                if kind.is_socket() {
                    fs::remove_file(&self.path)?;
                    Ok(())
                } else {
                    Err(RuntimeSocketFileError::NonSocketPath {
                        path: self.path.clone(),
                        kind,
                    })
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(RuntimeSocketFileError::Io(error)),
        }
    }
}

/// The uniform socket-and-storage surface the emitted daemon reads from a
/// component's configuration object.
///
/// A component's hand-written `Configuration` implements this so the emitted
/// `Daemon::run` can bind the working socket, the optional meta
/// socket, open the database, and wire the optional testing-trace socket
/// without the emitter naming component-specific accessor methods. Paths stay
/// borrowed from the configuration; the optional meta and trace slots are
/// `None` when the component does not expose those tiers.
pub trait BindingSurface {
    /// The peer-callable working signal socket path. Required — every triad
    /// daemon binds at least this listener.
    fn socket_path(&self) -> &Path;

    /// The file mode applied to the working signal socket. `None` leaves the
    /// socket at the default umask-derived mode; components with private
    /// working ingress return a concrete [`SocketMode`].
    fn socket_mode(&self) -> Option<SocketMode> {
        None
    }

    /// The request concurrency cap applied to each listener's admission gate.
    /// The default preserves single-request behavior for components that have
    /// not audited long-lived requests or parallel database access.
    fn request_concurrency_limit(&self) -> RequestConcurrencyLimit {
        RequestConcurrencyLimit::one()
    }

    /// The meta-signal socket path, when the component runs a second meta
    /// listener tier. `None` for single-listener daemons.
    fn meta_socket_path(&self) -> Option<&Path> {
        None
    }

    /// The owner-only upgrade socket path, when the component runs a third
    /// upgrade listener tier (the self-upgrade protocol). `None` for daemons
    /// without an upgrade tier.
    fn upgrade_socket_path(&self) -> Option<&Path> {
        None
    }

    /// The durable database path the engine opens at startup.
    fn database_path(&self) -> &Path;

    /// The testing-trace Unix socket path, when the component was launched
    /// with trace transport configured. `None` disables trace collection.
    fn trace_socket_path(&self) -> Option<&Path> {
        None
    }

    /// The file mode applied to the meta socket, when a meta listener tier is
    /// bound. `None` leaves the meta socket at the default umask-derived mode;
    /// components that need restricted meta authority return a concrete
    /// [`SocketMode`].
    fn meta_socket_mode(&self) -> Option<SocketMode> {
        None
    }
}

/// Turns a daemon's top-level `Result` into a process exit code, printing the
/// error to standard error prefixed with the process name.
///
/// This is the component-agnostic `fn main` tail the emitted daemon module
/// calls: `ExitReport::new(PROCESS_NAME).from_result(Daemon::…run())`. It owns
/// the process name it prints with, so the verb has a real noun to live on
/// rather than being a free function.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExitReport {
    process_name: &'static str,
}

impl ExitReport {
    pub const fn new(process_name: &'static str) -> Self {
        Self { process_name }
    }

    pub fn process_name(&self) -> &'static str {
        self.process_name
    }

    /// Report the outcome of a daemon run: `Ok` exits success, `Err` prints
    /// `"<process_name>: <error>"` to stderr and exits failure.
    pub fn from_result<Error>(&self, result: Result<(), Error>) -> ExitCode
    where
        Error: Display,
    {
        match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{}: {error}", self.process_name);
                ExitCode::FAILURE
            }
        }
    }
}
