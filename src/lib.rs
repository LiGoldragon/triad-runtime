//! Shared runtime support for schema-derived triad daemons.
//!
//! `triad-runtime` owns generic mechanics around generated Signal/Nexus/SEMA
//! interfaces. Component crates keep their schema-generated nouns and domain
//! algorithms; this crate supplies reusable runtime infrastructure that is the
//! same for every component.

#![forbid(unsafe_code)]

pub mod argument;
pub mod daemon;
pub mod frame;
pub mod runner;
pub mod trace;

pub use argument::{
    ArgumentError, ComponentArgument, ComponentCommand, InlineNota, NotaFile, SignalFile,
};
pub use daemon::{
    BoundSingleListenerDaemon, DaemonRuntime, ListenerError, RequestErrorLog, SingleListenerDaemon,
    SingleListenerDaemonError,
};
pub use frame::{FrameBody, FrameError, LengthPrefixedCodec, MaximumFrameLength};
pub use runner::{
    ContinuationBudget, ContinuationExhausted, ContinuationLimit, NextStep, Runner, RunnerEngines,
};
pub use trace::{
    TraceClient, TraceError, TraceEventFrame, TraceFrame, TraceLog, TraceSocketListener,
    TraceSocketPath,
};
