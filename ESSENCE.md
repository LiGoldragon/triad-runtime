# ESSENCE - triad-runtime

`triad-runtime` exists so schema-derived daemons stay readable at
runtime.

> [The triad-engine readability principle: the system should be readable because types name the work, schema names the interface, generated Rust names the objects and traits, and handwritten code is mostly the real algorithm: match typed input, make the decision, call the next typed interface, return typed output.]

The schema names each component interface. `schema-rust` emits the
component-specific objects and traits. `triad-runtime` owns only the generic
runtime mechanics those generated surfaces reuse. Component crates keep the
generated nouns and handwritten algorithms.

The test for this crate: it should make generated interfaces easier to run
without smuggling component-specific meaning into the shared runtime.
