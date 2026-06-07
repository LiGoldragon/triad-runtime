//! Shared runtime support for schema-derived triad daemons.
//!
//! `triad-runtime` owns generic mechanics around generated Signal/Nexus/SEMA
//! interfaces. Component crates keep their schema-generated nouns and domain
//! algorithms; this crate supplies reusable runtime infrastructure that is the
//! same for every component.

#![forbid(unsafe_code)]

pub mod actor_runtime;
pub mod argument;
pub mod daemon;
pub mod frame;
pub mod process;
pub mod role;
pub mod runner;
pub mod streaming;
pub mod trace;
pub mod workers;

pub use actor_runtime::{
    AcceptedConnection, AcquireRequestPermit, ActorBoundMultiListenerDaemon,
    ActorBoundSingleListenerDaemon, ActorConnectionRuntime, ActorListenerError,
    ActorListenerSocket, ActorMultiConnectionRuntime, ActorMultiListenerDaemon,
    ActorMultiListenerDaemonError, ActorSingleListenerDaemon, ActorSingleListenerDaemonError,
    RequestConcurrencyLimit, RequestGate, RequestGateStatus, RequestGateStatusRequest,
    RequestPermit, RequestPermitError, RequestPermitPool,
};
pub use argument::{
    ArgumentError, ComponentArgument, ComponentCommand, InlineNota, NotaFile, SignalFile,
};
pub use daemon::{
    BoundMultiListenerDaemon, BoundSingleListenerDaemon, DaemonRuntime, ListenerError,
    ListenerPollInterval, ListenerSocket, MultiListenerDaemon, MultiListenerDaemonError,
    MultiListenerRuntime, RequestErrorLog, SingleListenerDaemon, SingleListenerDaemonError,
    SocketMode,
};
pub use frame::{FrameBody, FrameError, LengthPrefixedCodec, MaximumFrameLength};
pub use process::{ConnectionContext, DaemonConfiguration, ExitReport};
pub use role::{
    NexusAction, NexusActionNextStep, NexusEffectCommand, NexusEffectResult, NexusWork,
    SemaReadInput, SemaReadOutput, SemaWriteInput, SemaWriteOutput,
};
pub use runner::{
    ContinuationBudget, ContinuationExhausted, ContinuationLimit, NextStep, Runner, RunnerEngines,
};
pub use streaming::{
    Subscription, SubscriptionEventPublisher, SubscriptionEventSequence, SubscriptionRegistry,
    SubscriptionToken, SubscriptionTokenIssuer,
};
pub use trace::{
    TraceClient, TraceError, TraceEventFrame, TraceFrame, TraceLog, TraceSocketListener,
    TraceSocketPath,
};
pub use workers::BoundedWorkers;
