# ARCHITECTURE — triad-runtime

## Purpose

`triad-runtime` is the shared runtime library for schema-derived component
daemons. It owns generic mechanics around Signal/Nexus/SEMA interfaces while
each component crate owns its generated schema nouns and domain algorithms.

## Trace Runtime

The current library surface is `trace`.

`TraceEventFrame` is the component boundary. A component's generated
`TraceEvent` implements the trait by archiving itself with rkyv. The runtime
never knows component-specific event variants.

`TraceLog<Event>` decides where events go:

- disabled sink;
- in-memory recording sink for tests;
- Unix socket sink for CLI-visible testing traces.

`TraceLog::record` is intentionally non-fatal: tracing is observability, not
the runtime contract. `TraceLog::record_result` exposes the fallible path for
tests and callers that need to assert socket delivery.

`TraceFrame<Event>` owns the length-prefixed frame mechanics. It writes a
four-byte big-endian archive length followed by the component-provided rkyv
archive bytes. `TraceSocketListener<Event>` binds a Unix socket, receives
those frames, decodes them through `TraceEventFrame`, and returns typed events.
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
component would otherwise copy the same mechanics. Backpressure and deeper
runtime-control machinery stay out of the current implementation scope.

## Code Map

- `src/lib.rs` — crate surface.
- `src/trace.rs` — generic trace log, frame, socket path, listener, client,
  and error.
- `tests/trace.rs` — rkyv frame and Unix socket witnesses using a local event
  type.
