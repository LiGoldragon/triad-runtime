# INTENT — triad-runtime

`triad-runtime` holds shared runtime mechanics for schema-derived
Signal/Nexus/SEMA component daemons.

The runtime crate is separate from schema emission. `schema-rust-next` emits
component-specific nouns and traits; `triad-runtime` provides reusable runtime
objects those generated surfaces can use at run time.

The first live scope is trace transport. Component crates own their generated
trace event type and actor hook logic. `triad-runtime` owns the event log,
length-prefixed binary frame mechanics, and Unix trace socket listener.

Machines communicate through rkyv archives. `triad-runtime` does not own NOTA
parsing; text projection stays at CLI and human-facing edges.

Backpressure and deeper runtime-control machinery are deferred future runtime
work. The current production slice is trace substrate extraction only.
