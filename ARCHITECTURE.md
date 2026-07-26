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

## Direction

`triad-runtime` provides the reusable runtime nouns schema-emitted daemons compose. The Nexus schema is the engine's **feature catalog**: every internal feature — any computation, any filtering or condition on results, any conditional write — is declared as a Nexus verb and object in the component schema, never as inline hidden logic in this runtime library. The runner drives already-declared typed actions; it must not hide new component capability behind generic runtime code. Per Spirit record `z6qu`.

Async task-backed daemon execution is the new target shape. `AsyncSingleListenerDaemon` and `AsyncMultiListenerDaemon` are the current target shells; see `NON_IDEAL_AGENTS.md` for legacy listener debt that must not shape new work.

The per-component runner GLUE (`NexusEngine::execute`, which constructs a `Runner` and awaits `Runner::drive`) is **not owned by `triad-runtime`**. That glue is schema-emitted by `schema-rust` into each component crate. `triad-runtime` supplies the loop; the schema supplies the per-component entry.

Cross-host transport is a tailnet-bound TCP listener in this crate, reusing the length-prefixed frame codec, with peer identity as a typed closed sum distinguishing kernel-vouched Unix-socket peers from tailnet TCP peers. Ssh-forwarded sockets are rejected as the transport shape. Per Spirit `rj9y` (Decision, High).

Component binaries share the single-argument rule through `ComponentCommand`. Daemons use `signal_file_argument()` and accept only a signal-encoded/rkyv file path; inline NOTA text and `.nota` paths are rejected before component-specific decoding. Per Spirit record `pjvv`.

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

The codec exposes both synchronous `Read` / `Write` and Tokio async
`AsyncRead` / `AsyncWrite` methods. The async methods are the async task-backed
daemon path: a generated listener can stay on Tokio IO without reimplementing
the length-prefix parser or blocking a runtime worker thread.

## Async Runtime

`async_runtime.rs` is the Kameo/Tokio substrate for the next daemon shell. The
first reusable primitive is request admission:

- `RequestConcurrencyLimit` names the bounded concurrency dial;
- `RequestPermitPool` owns a Tokio semaphore;
- `RequestPermit` holds one live request slot until the request driver drops
  it;
- `RequestGate` is a data-bearing Kameo actor that accepts permit requests and
  delegates the actual wait through `Context::spawn`.

The delegated wait is load-bearing. If all permits are held, the permit request
may wait, but the `RequestGate` mailbox remains available for status,
shutdown, or future control messages. This is the runtime pattern downstream
schema emitters should copy for storage and child-process effects: accept the
typed message, update actor state, return a delegated typed reply, and let the
slow wait happen outside the actor handler.

`AsyncSingleListenerDaemon` is the first async task-backed listener shell. It binds a
Tokio Unix listener, starts a `RequestGate`, and turns each accepted socket into
an `AcceptedConnection`: the Tokio stream, `ConnectionContext` read through
`SO_PEERCRED`, and the held `RequestPermit`. The component implements
`AsyncConnectionRuntime` on a data-bearing runtime object; that runtime handles
`AcceptedConnection` asynchronously. The listener shell owns socket preparation,
stale socket removal, socket-file cleanup, request admission, and request-error
logging.

`AsyncMultiListenerDaemon` is the async task-backed ordinary/meta listener shell. It
binds a list of `AsyncListenerSocket<Listener>` values, applies each socket's
optional mode, and starts one accept task per listener. Each listener task owns
its own `RequestGate`, so a slow or concurrency-limited ordinary request cannot
block meta admission. Accepted streams become `AcceptedConnection` values and
are passed to a shared data-bearing `AsyncMultiConnectionRuntime` together with
the listener identity. Request failures are logged per listener and do not stop
the accept loops; listener task failures remain fatal to the daemon.

`BindingSurface` exposes both `socket_mode()` for the working socket and
`meta_socket_mode()` for the meta socket. The default for each is `None`, so
older components keep their umask-derived behavior, while private daemon
surfaces can ask the generated binder to apply `0600`/`0660` modes through
`AsyncListenerSocket` instead of reintroducing local bind/chmod code.

The per-listener gate choice is deliberate. A single global gate would re-create
the old "one concern blocks another" bug at the runtime layer: if the ordinary
socket holds the only permit, the meta socket would wait even though its own
listener task and authority plane are separately admitted. Components that need
a cross-listener global budget can add that as component policy inside their
runtime; the shared shell's default backpressure boundary is the listener
concern.

`TcpListenerDaemon` (`src/tcp.rs`) is the cross-host sibling of the async Unix
shells. It binds whatever TCP socket address the caller configures — the
runtime does not know what a tailnet is; tailnet-only ingress is the deployed
bind address — starts the same `RequestGate` admission actor, and turns each
accepted stream into an `AcceptedConnection<TcpStream>` whose context carries
the peer's remote address. Frames go through `LengthPrefixedCodec` unchanged.
Socket-file concepts do not apply: there is no mode to chmod and no stale path
to remove, and cleanup is dropping the bound listener. Ssh-forwarded sockets
are rejected as the cross-host transport shape; the TCP listener with typed
peer identity is the transport.

`AcceptedConnection<Stream>` and `AsyncConnectionRuntime<Stream>` are generic
over the transport stream with `tokio::net::UnixStream` as the default, so the
Unix shells and their consumers keep the bare names while the TCP shell
requires `AsyncConnectionRuntime<TcpStream>`. One runtime object may implement
both instantiations when a component serves both transports.

## Argument Runtime

`ComponentCommand` owns the process-edge single-argument rule. It accepts an
argv slice, verifies exactly one component argument, and classifies it as a
`ComponentArgument`:

- `InlineNota` — inline text for a CLI/user edge;
- `NotaFile` — an existing path read as NOTA text by the component;
- `SignalFile` — an existing non-`.nota` path read as a signal-encoded binary
  by a daemon or batch edge.

The runtime deliberately does not parse NOTA. It removes duplicated argument
counting and path/text classification while leaving schema-specific parsing
to each component crate. Daemon entrypoints call `signal_file_argument()`:
inline text and `.nota` paths are rejected before the component tries to load
its typed binary startup record.

## Runner Runtime

`Runner` owns the shared recursive Nexus loop. Component code does not
hand-write the cycle from a Nexus action into SEMA writes, SEMA reads, effects,
or another Nexus work item. Instead, generated glue projects the component's
typed action enum into `NextStep`, and async `Runner::drive` dispatches the fixed
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
plus the async effect handler and budget-exhausted reply. SEMA writes, SEMA
reads, and effects are awaited runner steps so database work, actor messages,
and child processes are not disguised as synchronous callbacks.

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

### Runner ownership and the emitted daemon entry

The ownership line is exact. `triad-runtime` owns the generic recursive runner
and nothing component-specific: `Runner`, async `Runner::drive`, the `NextStep`
five-outcome enum, the `RunnerEngines` role trait, and the typed
`ContinuationLimit` / `ContinuationBudget` / `ContinuationExhausted` budget.

The per-component runner GLUE is NOT owned by `triad-runtime`. The default
`NexusEngine::execute` method — which constructs a `Runner` from the component's
`continuation_limit`, wraps the component engine in a `RunnerEngines` adapter,
awaits `Runner::drive`, and projects the reply back into a `NexusAction` — is
schema-emitted by `schema-rust` into each component crate. In `spirit` that
glue is the generated `src/schema/nexus.rs`. `triad-runtime` provides the loop;
the schema provides each component's typed entry into the loop.

The old shorthand `triad_main!` is now realized as a source-visible emitted
daemon module, not as a proc-macro invocation. `schema-rust` emits
`src/schema/daemon.rs` when a component build declares a `NexusDaemonShape`.
That module owns `DaemonCommand`, `ComponentDaemon`, the
`GeneratedDaemonRuntime` decode -> execute -> encode spine, single/multi
listener selection, `DaemonError`, and `DaemonEntry::run_to_exit_code`.
`triad-runtime` does NOT emit that module; it supplies the reusable process,
listener, runner, frame, streaming, and exit-report objects the emitted module
uses.

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

`MultiListenerDaemon` is the legacy synchronous ordinary/meta shell. It binds a
list of `ListenerSocket<Listener>` values, applies each socket's optional
`SocketMode`, sets listeners nonblocking, and polls them in one synchronous
loop. See `NON_IDEAL_AGENTS.md` for its migration debt and restrictions on new
schema-emitted daemon work.

`MultiListenerRuntime::should_continue` is the stop boundary for supervised
components. The default keeps serving forever; a component runtime that owns a
supervision or shutdown signal can return false, causing the shared stream loop
to exit cleanly before `stop` is called. This keeps graceful shutdown in the
shared daemon shell instead of forcing every supervised component to fork its
own polling loop.

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
subscriptions, issues tokens, accepts already-minted tokens from a
schema-declared open-subscription effect, unregisters tokens, and publishes
matching events through caller-supplied filter and delivery closures.

`SubscriptionEventEpochAuthority` is the single application-owned authority
for subscription-event epochs. It reserves a non-cloneable, one-use
`SubscriptionEventEpoch` for each publisher. The application persists
`next_epoch()` and restores it with `restore(...)`; it is responsible for
persisting a reservation before its publisher is used after a crash boundary
and for never running two authorities for the same stream identity.
`SubscriptionEventPublisher<Contract, Input, Output, Event>` consumes that
reservation, privately owns its acceptor-lane sequence, and produces
`signal_frame::BoundStreamingFrame<Contract, Input, Output, Event>` values
whose body is `StreamingFrameBody::SubscriptionEvent`. `Contract` implements
`signal_frame::WireContract`, so the producer derives the nonzero contract and
revision header and callers cannot inject an unbound or wrong-contract header.
The publisher remains generic over the schema-generated request, reply, and
event roots; it never knows component event variants. `publish` returns
`Result<BoundStreamingFrame<...>, SubscriptionPublishError>`; maximum sequence
and epoch values are issued once, then remain typed exhaustion errors without
wrapping or mutation.

The 0.8 migration removes the unbound publisher API. Consumers add their
generated contract marker as the first publisher type parameter and pass the
event `WireRoute` to `new`; raw `ShortHeader` construction and unbound
`StreamingFrame` output have no compatibility path. Consumers now obtain the
second `new` argument from an application-owned
`SubscriptionEventEpochAuthority`, handle `SubscriptionEventEpochError` at
reservation and `SubscriptionPublishError` from `publish`, and restore only
the persisted next epoch with `SubscriptionEventEpochAuthority::restore`.
Restoring stale state or running concurrent authorities can reintroduce duplicate
epochs and remains the application's persistence and single-authority
responsibility. Event identifier fields are read through `session_epoch()`,
`lane()`, and `sequence()`.

The runtime does not own stream policy. Schema declares which operations open
streams and which event variants belong to streams; generated code exposes the
typed frame aliases; component code supplies filter semantics and writes frames
to the subscriber connection.

## Process Runtime

`process.rs` owns the component-agnostic process edge the emitted daemon
module (`schema-rust` `RustEmissionTarget::Daemon`) reads.

`BindingSurface` is the uniform socket-and-storage surface a component's
hand-written `Configuration` implements: `socket_path` (the required working
listener), `meta_socket_path` (the optional meta tier),
`database_path`, `trace_socket_path`, and `meta_socket_mode`. The emitted
`Daemon::run` binds listeners and opens the engine by reading these accessors,
so the emitter never names component-specific configuration methods. The
optional tiers default to `None`, so a single-listener daemon implements only
`socket_path` + `database_path`.

`ExitReport` owns the process name and turns a daemon's top-level `Result`
into a `std::process::ExitCode` through `from_result`: `Ok` exits success,
`Err` prints `"<process_name>: <error>"` to stderr and exits failure. This is
the component-agnostic `fn main` tail the emitted daemon calls, so the
exit-mapping verb lives on a real noun (the process name) rather than as a
free function re-emitted into every component.

`ConnectionContext` is the trust-boundary carrier for accepted streams. Inside
it, `PeerIdentity` is a closed sum with exactly two variants: a Unix-socket
peer carrying kernel-vouched `UnixCredentials` (the `SO_PEERCRED` uid/gid/pid
triple, read with rustix's safe `socket_peercred` wrapper), and a TCP peer
carrying only its remote socket address. No accessor pretends a TCP peer has
Unix credentials — `unix_credentials()` and `tcp_address()` both return
`Option`, and the credential accessors live on `UnixCredentials` itself. The
schema-rust emitted daemon module reads the context from each accepted
stream and passes it into the component working-input hook; components that
mint provenance classify owner/non-owner/internal/remote origins from the
typed peer identity instead of trusting payload fields. `triad-runtime` keeps
the context type and safe credential reader; the actual `handle_working_input`
hook signature and wiring are emitted by schema-rust.

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
- `src/process.rs` — component-agnostic `BindingSurface` socket surface
  and `ExitReport` process-exit mapping for the emitted daemon module.
- `src/daemon.rs` — reusable single-listener Unix daemon runner and lifecycle
  trait, plus multi-listener ordinary/meta daemon shell.
- `src/runner.rs` — generic recursive Nexus runner and typed continuation
  budget.
- `src/role.rs` — reusable role traits implemented by generated component
  roots.
- `src/streaming.rs` — reusable subscription token registry and typed
  `signal-frame` subscription-event publisher.
- `src/tcp.rs` — async task-backed TCP listener shell for cross-host
  transport.
- `src/trace.rs` — generic trace log, frame, socket path, listener, client,
  and error.
- `tests/argument.rs` — single-argument and argument-kind witnesses.
- `tests/frame.rs` — generic length-prefix codec witnesses.
- `tests/daemon.rs` — Unix listener preparation, lifecycle, and request-error
  isolation witnesses.
- `tests/process.rs` — binding surface, peer-identity, and exit-report
  witnesses.
- `tests/runner.rs` — shared runner loop and budget witnesses.
- `tests/streaming.rs` — token issuance, registry filtering, event sequence,
  and `signal-frame` streaming-frame witnesses.
- `tests/tcp.rs` — loopback TCP frame round-trip, remote-address peer
  identity, and listener-drop cleanup witnesses.
- `tests/trace.rs` — rkyv frame and Unix socket witnesses using a local event
  type.
