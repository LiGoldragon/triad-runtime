# INTENT — triad-runtime

`triad-runtime` holds shared runtime mechanics for schema-derived
Signal/Nexus/SEMA component daemons.

The triad-engine readability principle is load-bearing here:
[The triad-engine readability principle: the system should be readable because types name the work, schema names the interface, generated Rust names the objects and traits, and handwritten code is mostly the real algorithm: match typed input, make the decision, call the next typed interface, return typed output.]

The nexus schema is the engine's FEATURE CATALOG. Every internal feature —
any computation, any filtering or condition on results, any conditional
write — is declared as a Nexus verb + object in the schema, never as inline
hidden logic, so the engine's complete internal-feature surface is visible in
one place. Feature visibility is the main reason the Nexus interface exists;
the runner executes those declared Nexus verbs. (psyche 2026-06-05, record z6qu)

The runtime crate is separate from schema emission. `schema-rust-next` emits
component-specific nouns and traits; `triad-runtime` provides reusable runtime
objects those generated surfaces can use at run time.

Async task-backed daemon execution is the new target shape. `triad-runtime` should
provide Kameo/Tokio runtime nouns that schema-emitted daemons reuse, and those
nouns must keep actor mailboxes available while requests wait on admission,
storage, child processes, or other effects. Long-running waits are delegated
through actor-aware tasks and typed replies; they are not synchronous work hidden
inside an actor handler.
The emitted daemon should depend on runtime-owned listener and admission nouns,
not generate socket accept loops itself: accepted connections enter as typed
`AcceptedConnection` values carrying the Tokio stream, kernel-vouched peer
credentials, and the held request permit.
The emitted daemon should also apply runtime-owned socket preparation uniformly:
the configuration surface exposes an optional working socket mode alongside
the optional meta socket mode, and the generated binder passes those modes into
the async listener sockets instead of each component hand-writing chmod logic.
Multi-listener async daemons should admit requests per listener concern, not
through one global gate that lets an ordinary request starve a meta request.
Each bound ordinary/meta socket owns its own `RequestGate`; the component
runtime may still be shared, but waiting on one listener's concurrency budget
does not block another listener's accept loop.
Reusable engine-role names are expressed as runtime traits when concrete
component variants differ. A component still owns its generated `NexusAction`,
`NexusWork`, `SemaWriteInput`, and sibling enums, but shared runtime code speaks
through `triad-runtime` role traits rather than treating one component's enum
name as the generic concept.

The first live scope is trace transport. Component crates own their generated
trace event type and actor hook logic. `triad-runtime` owns the event log,
length-prefixed binary frame mechanics, and Unix trace socket listener.
Client-side trace collection also belongs to this shared runtime surface: a
component CLI should instantiate a generic typed trace client and render events
only at the user-facing display edge, rather than hand-writing trace listener
logic per component. The component's generated trace type decides the display
surface; text clients should normally render the generated NOTA value, while
the runtime stays generic over the event noun.

Machines communicate through rkyv archives. `triad-runtime` does not own NOTA
parsing; text projection stays at CLI and human-facing edges.
The default `TraceLog::record` path is silent on delivery failure; callers
that need proof use the fallible `record_result` method. Trace transport should
not create runtime string fallback logs before the client display boundary.

Shared runtime byte mechanics live here before each component hand-rolls them:
`LengthPrefixedCodec` owns the four-byte big-endian length-prefix envelope
used by trace and signal transports. The payload remains caller-owned binary
data; the codec never interprets schema, NOTA, or rkyv.
The same codec owns synchronous and Tokio async IO methods so async task-backed
listeners do not duplicate frame parsing or fall back to blocking `Read` /
`Write` in async request drivers.

Streaming subscription mechanics are runtime-owned once schema exposes the
stream. `signal-frame` owns the low-level wire kernel; generated schemas own
typed `Input`, `Output`, and event payloads; `triad-runtime` owns reusable
subscription token issuance, live-subscription registries, stream event
sequence IDs, and publishers that construct real
`signal_frame::StreamingFrameBody::SubscriptionEvent` frames. Component code
supplies filter policy and delivery IO; it should not reimplement token
counters or event-frame construction per daemon.

Component binaries share the same single-argument rule through
`ComponentCommand` and `ComponentArgument`. The runtime enforces the exact-one
argument shape, while the caller chooses the edge-specific classifier. Text
clients use the NOTA classifier. Daemons use `signal_file_argument()` and accept
only a signal-encoded/rkyv file path; inline NOTA text and `.nota` paths are
rejected before component-specific decoding. Daemons do not understand NOTA
startup or configuration text; deploy/bootstrap tools encode typed data before
the daemon receives it. (psyche 2026-06-07, record pjvv)

The recursive Nexus runner is runtime-owned. Component code should not repeat a
hand-written action loop that applies storage, observes storage, runs effects,
continues, and checks a local budget. `Runner` owns that loop and the typed
continuation budget; generated glue projects each component's typed
`NexusAction` into the fixed `NextStep` shape. Component authors implement the
three plane engines, the async effect handler, and the budget-exhausted reply.
SEMA writes, SEMA reads, and effects are awaited runner steps, because storage
and external effects can be actor messages, database IO, or child-process work.
The adapter that bundles those methods for `RunnerEngines` is generated.

The ownership boundary for the runner is precise, so a reader is not misled
about who owns what. `triad-runtime` owns ONLY the generic recursive runner
machinery: `Runner`, async `Runner::drive`, the `NextStep` five-outcome enum, the
`RunnerEngines` role trait, and the typed `ContinuationLimit` /
`ContinuationBudget` / `ContinuationExhausted` budget. The per-component runner
GLUE — the default `NexusEngine::execute` method that constructs a `Runner`,
builds the component's `RunnerEngines` adapter, awaits `Runner::drive`, and wraps
the reply back into a `NexusAction` — is NOT owned by `triad-runtime`. That glue
is schema-emitted by `schema-rust-next` into each component crate (in `spirit`
it lives in the generated `src/schema/nexus.rs`). `triad-runtime` supplies the
loop; the schema supplies the per-component entry into the loop.

The old shorthand `triad_main!` is realized as a source-visible emitted daemon
module, not a proc macro. `schema-rust-next` emits `src/schema/daemon.rs` when a
component build declares a `NexusDaemonShape`; that module owns `DaemonCommand`,
`ComponentDaemon`, the decode → execute → encode spine, single/multi listener
selection, `DaemonError`, and `DaemonEntry::run_to_exit_code`. `triad-runtime`
does not emit that module. It supplies the reusable process, listener, runner,
frame, streaming, and exit-report objects the emitted module uses.
The runner does not own component feature vocabulary. Per intent record
`gvaz`, computations, result filters, conditional writes, and similar internal
engine features are declared as Nexus schema verbs/objects in the component
crate. `triad-runtime` only drives the already-declared typed actions and
effects; it must not hide new component capability behind generic runtime code.

The single-listener daemon runner is runtime-owned. Component code should not
repeat the Unix socket preparation and accept loop that every component daemon
needs before it reaches its typed Signal/Nexus/SEMA engines. `SingleListenerDaemon`
owns parent-directory creation, stale socket removal, listener binding,
request-error isolation, and the start/stop lifecycle calls around a
data-bearing component runtime. Component crates still own their typed
configuration object, engine construction, signal-frame transport, and domain
errors.

The async task-backed multi-listener daemon shell is runtime-owned for ordinary +
meta signal daemons. `AsyncMultiListenerDaemon` binds multiple Unix sockets,
applies per-socket modes, isolates request errors, and routes accepted
connections through one data-bearing runtime object with a listener identity.
The listener accept loops are independent Tokio tasks, and each listener owns
its own admission gate. The one runtime owner remains the component boundary for
generated Nexus execution and SEMA single-writer semantics; components supply
the typed ordinary/meta frame bridges and the generated Nexus/SEMA behavior.
Socket-file cleanup also belongs to the bound daemon shell: once a bound
single- or multi-listener daemon is dropped, its Unix socket paths are removed
so supervised components release their ingress paths after shutdown.
The older synchronous `MultiListenerDaemon` remains in the crate only as a
migration surface for consumers not yet moved to async task-backed generated daemon
code; new schema-emitted daemon work should target the async runtime nouns.

Accepted-connection trust context is runtime-owned and emitter-wired.
`ConnectionContext` carries kernel-vouched `SO_PEERCRED` uid/gid/pid for a Unix
socket stream, using rustix's safe `socket_peercred` wrapper so runtime code
keeps `unsafe_code = "forbid"`. `schema-rust-next` emits the working-input hook
that receives this context; components decide what it means for provenance or
authority. The runtime owns the credential noun and reader, not the
component-specific trust policy.

Deeper runtime-control machinery is deferred future runtime work. Deployment
concurrency is a runtime concern, not public contract vocabulary. The current
production slice is trace substrate plus reusable frame, argument, runner,
async task-backed single-listener daemon, async task-backed multi-listener ordinary/meta
daemon edges, legacy synchronous daemon compatibility, and typed streaming
subscription mechanics.
