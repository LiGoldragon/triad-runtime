# Skills — triad-runtime

Read the workspace Rust skills before editing this crate:

- `skills/rust-discipline.md`
- `skills/rust/methods.md`
- `skills/rust/errors.md`
- `skills/rust/storage-and-wire.md`
- `skills/rust/crate-layout.md`
- `skills/abstractions.md`

This crate is a runtime library, not an emitter. Keep component-specific
schema nouns in the component crate and expose generic runtime objects here.
