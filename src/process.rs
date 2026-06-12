//! Component-agnostic process-edge runtime for emitted daemons.
//!
//! The emitted daemon module (schema-rust-next `RustEmissionTarget::Daemon`)
//! reads its socket layout through [`BindingSurface`] and turns its top
//! `Result` into a process exit code through [`ExitReport`]. Both surfaces are
//! deliberately free of component meaning: the trait names the uniform socket
//! accessors every triad daemon configuration exposes, and the exit reporter
//! owns only the process name it prints with.

use std::{
    fmt::Display, net::SocketAddr, os::unix::net::UnixStream, path::Path, process::ExitCode,
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
/// The schema-rust-next emitted daemon module reads this once per accepted
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
