//! Component-agnostic process-edge runtime for emitted daemons.
//!
//! The emitted daemon module (schema-rust-next `RustEmissionTarget::Daemon`)
//! reads its socket layout through [`DaemonConfiguration`] and turns its top
//! `Result` into a process exit code through [`ExitReport`]. Both surfaces are
//! deliberately free of component meaning: the trait names the uniform socket
//! accessors every triad daemon configuration exposes, and the exit reporter
//! owns only the process name it prints with.

use std::{fmt::Display, path::Path, process::ExitCode};

use crate::SocketMode;

/// The uniform socket-and-storage surface the emitted daemon reads from a
/// component's configuration object.
///
/// A component's hand-written `Configuration` implements this so the emitted
/// `Daemon::run` can bind the working socket, the optional owner-only meta
/// socket, open the database, and wire the optional testing-trace socket
/// without the emitter naming component-specific accessor methods. Paths stay
/// borrowed from the configuration; the optional meta and trace slots are
/// `None` when the component does not expose those tiers.
pub trait DaemonConfiguration {
    /// The peer-callable working signal socket path. Required — every triad
    /// daemon binds at least this listener.
    fn socket_path(&self) -> &Path;

    /// The owner-only meta-signal socket path, when the component runs a
    /// second meta listener tier. `None` for single-listener daemons.
    fn meta_socket_path(&self) -> Option<&Path> {
        None
    }

    /// The durable database path the engine opens at startup.
    fn database_path(&self) -> &Path;

    /// The testing-trace Unix socket path, when the component was launched
    /// with trace transport configured. `None` disables trace collection.
    fn trace_socket_path(&self) -> Option<&Path> {
        None
    }

    /// The owner-only file mode applied to the meta socket, when a meta
    /// listener tier is bound. `None` leaves the meta socket at the default
    /// umask-derived mode; components that need owner-only authority return a
    /// concrete [`SocketMode`].
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
