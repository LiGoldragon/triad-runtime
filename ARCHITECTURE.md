# ARCHITECTURE — triad-runtime

## Purpose

`triad-runtime` is the shared runtime library for schema-derived component
daemons. It owns generic mechanics around Signal/Nexus/SEMA interfaces while
each component crate owns its generated schema nouns and domain algorithms.

The readability rule is the boundary rule: schema names the interface;
generated Rust names the objects and traits; handwritten component code should
mostly match typed input, decide, call the next typed interface, and return
typed output. `triad-runtime` supports that path without becoming the owner of
component-specific meaning.

## Frame Runtime

`LengthPrefixedCodec` owns the generic binary envelope used by runtime
transports: a four-byte big-endian body length followed by exactly that many
payload bytes. `FrameBody` is intentionally just bytes. The codec does not
know about schema roots, trace events, signal frames, NOTA, or rkyv archive
layout; those belong to the caller.

This replaces the old pattern where trace transport, signal transport, and
schema-emitted frame tests each hand-rolled the same prefix logic. Components
configure a maximum body length by constructing `MaximumFrameLength`; the
default accepts the full `u32` prefix range.

## Argument Runtime

`ComponentCommand` owns the process-edge single-argument rule. It accepts an
argv slice, verifies exactly one component argument, and classifies it as a
`ComponentArgument`:

- `InlineNota` — inline text for a CLI/user edge;
- `NotaFile` — an existing path read as NOTA text by the component;
- `SignalFile` — an existing path read as a signal-encoded binary by a
  daemon or batch edge.

The runtime deliberately does not parse NOTA. It removes duplicated argument
counting and path/text classification while leaving schema-specific parsing
to each component crate.

## Runner Runtime

`Runner` owns the shared recursive Nexus loop. Component code does not
hand-write the cycle from a Nexus action into SEMA writes, SEMA reads, effects,
or another Nexus work item. Instead, generated glue projects the component's
typed action enum into `NextStep`, and `Runner::drive` dispatches the fixed
five-outcome shape:

- `Reply` exits to Signal;
- `SemaWrite` applies storage and re-enters with write completion;
- `SemaRead` observes storage and re-enters with read completion;
- `RunEffect` performs a component effect and re-enters with effect
  completion;
- `Continue` re-enters Nexus directly.

`RunnerEngines` is the adapter surface between generated/component code and
the library loop. It is deliberately typed over each component's generated
payloads; the runtime does not erase plane identity or invent component
meaning. The adapter is generated glue, not an author-written fourth engine.
Component authors still implement the real Signal, Nexus, and SEMA behavior
plus the effect handler and budget-exhausted reply.

The fixed five-outcome loop is mechanics, not feature vocabulary. If a
component adds a computed operation, a result filter, a conditional write, or a
similar internal engine feature, that feature is first declared in the
component's Nexus schema as a verb/object. The runtime may then drive the
resulting typed action or effect, but the feature remains visible in the
component schema instead of disappearing into shared library code.

`ContinuationLimit`, `ContinuationBudget`, and `ContinuationExhausted` make the
recursion limit typed and testable. A final `Reply` is always allowed, but once
the limit is exhausted the runner refuses to dispatch another storage,
effect, or continuation step and asks the component for a typed error reply.
The default limit is 32 non-reply steps.

`role.rs` names the reusable engine roles as traits. Generated component roots
such as `NexusWork`, `CommandSemaWrite`, `CommandSemaRead`, and
`CommandEffect` implement `NexusWork`, `SemaWriteInput`, `SemaReadInput`, and
`NexusEffectCommand` respectively. The concrete enum variants remain
component-specific; the reusable name is the trait/interface the runtime sees.

## Daemon Runtime

`SingleListenerDaemon` owns the reusable single-listener daemon shell. It
prepares the Unix socket path, creates the parent directory, removes any stale
socket file, binds the listener, starts the component runtime, and then serves
incoming streams. A request-level error is logged with `RequestErrorLog` and
does not stop the daemon; listener, startup, and shutdown errors remain fatal.

`DaemonRuntime` is the component boundary. A component implements it on a
data-bearing runtime object that owns its engine state. The shared daemon
runner calls:

- `start` before the listener begins serving;
- `handle_stream` for each accepted Unix stream;
- `stop` when the accept loop exits.

The runtime crate deliberately does not know about generated Signal roots,
rkyv archives, NOTA, SEMA tables, trace configuration, or policy meaning. A
component's `handle_stream` method remains the place where generated
signal-frame transport meets the component engine.

`MultiListenerDaemon` is the ordinary/meta daemon shell. It binds a list of
`ListenerSocket<Listener>` values, applies each socket's optional `SocketMode`,
sets listeners nonblocking, and passes accepted streams to one
`MultiListenerRuntime` object together with the listener identity. This keeps
the current engine-owner shape serial: two sockets do not imply two mutable
Nexus engines or a broad mutex around SEMA. Components still own their typed
ordinary/meta frame adapters; the runtime owns socket preparation, request-error
isolation, and lifecycle order.

This is the current production listener model, not the final streaming or
parallel scheduler model. A future transport scheduler may sit between the
listener set and the engine owner, but public contracts still do not declare
deployment parallelism.

## Streaming Runtime

`streaming.rs` owns reusable subscription mechanics above the `signal-frame`
wire kernel. `SubscriptionToken` is the bridge trait for generated
component-local token newtypes. `SubscriptionTokenIssuer` mints monotonically
increasing `signal_frame::SubscriptionTokenInner` values and wraps them in the
generated token type. `SubscriptionRegistry<Token, Filter>` stores live
subscriptions, issues tokens, unregisters tokens, and publishes matching
events through caller-supplied filter and delivery closures.

`SubscriptionEventSequence` owns `signal_frame::StreamEventIdentifier`
generation for the daemon/acceptor lane. `SubscriptionEventPublisher<Input,
Output, Event>` combines that sequence with a short header and produces real
`signal_frame::StreamingFrame<Input, Output, Event>` values whose body is
`StreamingFrameBody::SubscriptionEvent`. The publisher is generic over the
schema-generated request, reply, and event roots; it never knows component
event variants.

The runtime does not own stream policy. Schema declares which operations open
streams and which event variants belong to streams; generated code exposes the
typed frame aliases; component code supplies filter semantics and writes frames
to the subscriber connection.

## Trace Runtime

`TraceEventFrame` is the component boundary. A component's generated
`TraceEvent` implements the trait by archiving itself with rkyv. The runtime
never knows component-specific event variants.

`TraceLog<Event>` decides where events go:

- disabled sink;
- in-memory recording sink for tests;
- Unix socket sink for CLI-visible testing traces.

`TraceLog::record` is intentionally non-fatal and silent: tracing is
observability, not the runtime contract, and the default path does not print
string fallback logs from the runtime. `TraceLog::record_result` exposes the
fallible path for tests and callers that need to assert socket delivery.

`TraceFrame<Event>` owns the typed trace envelope and delegates the reusable
length-prefix mechanics to `LengthPrefixedCodec`. It writes a four-byte
big-endian archive length followed by the component-provided rkyv archive
bytes. `TraceSocketListener<Event>` binds a Unix socket, receives those
frames, decodes them through `TraceEventFrame`, and returns typed events.
Tests can either collect for a fixed time window or collect until an expected
event count arrives before a timeout.

`TraceClient<Event>` is the generic client-side trace surface. It is disabled
when no trace socket is configured, or it binds a `TraceSocketListener` and
collects typed `Event` values from the daemon. It only renders events through
`Display` at `print_events`, so trace data stays typed until the client/user
boundary. The component supplies that `Display` implementation; a NOTA-enabled
client can render the generated NOTA event without `triad-runtime` depending on
NOTA.

## Boundaries

`triad-runtime` owns reusable runtime infrastructure. It does not emit schema,
define component signal roots, parse NOTA, own component storage tables, or
decide component behavior.

Future extraction waves may add generic daemon command scaffolding, signal
transport, and trace-aware test harnesses. Those move here only when a second
component would otherwise copy the same mechanics. Backpressure, multi-listener
handoff, and deeper runtime-control machinery stay out of the current
implementation scope.

## Code Map

- `src/lib.rs` — crate surface.
- `src/argument.rs` — component process-edge argument classification.
- `src/frame.rs` — generic four-byte length-prefixed binary frame codec.
- `src/daemon.rs` — reusable single-listener Unix daemon runner and lifecycle
  trait, plus multi-listener ordinary/meta daemon shell.
- `src/runner.rs` — generic recursive Nexus runner and typed continuation
  budget.
- `src/role.rs` — reusable role traits implemented by generated component
  roots.
- `src/streaming.rs` — reusable subscription token registry and typed
  `signal-frame` subscription-event publisher.
- `src/trace.rs` — generic trace log, frame, socket path, listener, client,
  and error.
- `tests/argument.rs` — single-argument and argument-kind witnesses.
- `tests/frame.rs` — generic length-prefix codec witnesses.
- `tests/daemon.rs` — Unix listener preparation, lifecycle, and request-error
  isolation witnesses.
- `tests/runner.rs` — shared runner loop and budget witnesses.
- `tests/streaming.rs` — token issuance, registry filtering, event sequence,
  and `signal-frame` streaming-frame witnesses.
- `tests/trace.rs` — rkyv frame and Unix socket witnesses using a local event
  type.
