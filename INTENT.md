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

Component binaries share the same single-argument rule through
`ComponentCommand` and `ComponentArgument`. The runtime enforces the exact-one
argument shape and classifies the edge as inline NOTA text, NOTA file, or
signal-encoded file; component crates still parse the schema-specific value.

The recursive Nexus runner is runtime-owned. Component code should not repeat a
hand-written action loop that applies storage, observes storage, runs effects,
continues, and checks a local budget. `Runner` owns that loop and the typed
continuation budget; generated glue projects each component's typed
`NexusAction` into the fixed `NextStep` shape. Component authors implement the
three plane engines, the effect handler, and the budget-exhausted reply. The
adapter that bundles those methods for `RunnerEngines` is generated.

The single-listener daemon runner is runtime-owned. Component code should not
repeat the Unix socket preparation and accept loop that every component daemon
needs before it reaches its typed Signal/Nexus/SEMA engines. `SingleListenerDaemon`
owns parent-directory creation, stale socket removal, listener binding,
request-error isolation, and the start/stop lifecycle calls around a
data-bearing component runtime. Component crates still own their typed
configuration object, engine construction, signal-frame transport, and domain
errors.

Backpressure and deeper runtime-control machinery are deferred future runtime
work. Multi-listener/meta-signal handoff and deployment concurrency are not
part of the current runner slice; concurrency is a runtime/deployment concern,
not public contract vocabulary. The current production slice is trace
substrate plus reusable frame, argument, runner, and single-listener daemon
edges.
