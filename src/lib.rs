//! Shared runtime support for schema-derived triad daemons.
//!
//! `triad-runtime` owns generic mechanics around generated Signal/Nexus/SEMA
//! interfaces. Component crates keep their schema-generated nouns and domain
//! algorithms; this crate supplies reusable runtime infrastructure that is the
//! same for every component.

#![forbid(unsafe_code)]

pub mod trace;

pub use trace::{
    TraceError, TraceEventFrame, TraceFrame, TraceLog, TraceSocketListener, TraceSocketPath,
};
