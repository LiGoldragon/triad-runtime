//! The shared reaction frame — `Work` and `Action` as direct-parameter
//! generics, declared once and bound per component.
//!
//! This is the compile-prototype for risk gate #408 (O3). The schema-emitted
//! `NexusWork` / `NexusAction` enums (one fixed shape per component) are
//! replaced by two generic enums declared here once. Each component binds the
//! type parameters to its own payloads.
//!
//! Gate verdict: the inhabited multi-parameter frame proves out fully, but the
//! O3 omittable-leg proposal — bind an absent leg to an uninhabitable
//! `enum Never {}` carrying the wire stack — is DISPROVEN; see `Never`'s doc
//! for the exact derive failure. Fewer-leg components fall back to a fixed
//! smaller arity binding the absent leg to a derivable stand-in.
//!
//! The derive stack is taken character-for-character from the generated wire
//! enums in `spirit/src/schema/nexus.rs` — the types this frame replaces:
//!
//! ```ignore
//! #[cfg_attr(feature = "dotos-text", derive(dotos::DotosDecode, dotos::DotosEncode))]
//! #[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Debug, PartialEq, Eq)]
//! ```
//!
//! No `#[rkyv(...)]` / `#[archive(...)]` bound attributes, no `omit_bounds`,
//! no explicit `where`: the rkyv 0.8 derive and the dotos derive each
//! synthesise their own per-parameter bounds, so the bare stack composes over
//! a multi-parameter generic enum without help. See the module test suite for
//! the proof.

use crate::NextStep;

/// Work the reaction frame can be handed: a typed event or the completion of a
/// commanded side effect. Each parameter is bound by the component to its own
/// payload. A component with fewer legs binds an absent leg to a derivable
/// stand-in payload (the fixed-arity fallback); binding it to the uninhabitable
/// `Never` does not compile under the wire-derive stack — see `Never`.
#[cfg_attr(feature = "dotos-text", derive(dotos::DotosDecode, dotos::DotosEncode))]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum Work<Event, Write, Read, Effect> {
    SignalArrived(Event),
    SemaWriteCompleted(Write),
    SemaReadCompleted(Read),
    EffectCompleted(Effect),
}

/// The decision the reaction frame emits: reply, command a SEMA write/read,
/// command a side effect, or continue with further work. The `Continuation`
/// parameter is the component's own `Work` instantiation, kept as a free
/// parameter so the frame stays acyclic at the type level.
#[cfg_attr(feature = "dotos-text", derive(dotos::DotosDecode, dotos::DotosEncode))]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum Action<Reply, Write, Read, Effect, Continuation> {
    ReplyToSignal(Reply),
    CommandSemaWrite(Write),
    CommandSemaRead(Read),
    CommandEffect(Effect),
    Continue(Continuation),
}

/// The uninhabitable leg marker (O3's omittable-leg mechanism). A component
/// that does not have a given leg binds that leg's payload parameter to a type
/// that carries no value, so the corresponding `Work`/`Action` variant can
/// never be constructed.
///
/// # The risk-gate #408 (O3) finding — the bare `enum Never {}` does NOT work
///
/// The natural O3 shape is `enum Never {}` carrying the same wire-derive stack
/// as every payload, so it is a drop-in binding for any leg parameter. **It
/// does not compile.** The `rkyv` 0.8 derive unconditionally emits `#[repr(u8)]`
/// on the archived tag/enum and a `match self {}` in its `Serialize` /
/// `Deserialize` bodies; a zero-variant enum rejects both
/// (`E0084 unsupported representation for zero-variant enum`, and
/// `E0004 non-exhaustive patterns: &Never is non-empty` because references are
/// always considered inhabited). This is asserted as a `compile_fail` doctest:
///
/// ```compile_fail
/// #[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Clone, Debug, PartialEq, Eq)]
/// pub enum Never {}
/// ```
///
/// A hand-written stack impl is also unavailable here: `rkyv::Portable` and
/// `bytecheck::CheckBytes` are `unsafe trait`s, and this crate is
/// `#![forbid(unsafe_code)]`, so the only route to those impls is the derive
/// that just failed.
///
/// Therefore the maximal-frame-with-uninhabitable-leg design as literally
/// proposed (bind an absent leg to `enum Never {}`) is **disproven** by this
/// gate. The inhabited multi-parameter frame (every other proof) holds; the
/// omittable leg falls back to fixed arities, or to a future derivable
/// uninhabitable stand-in that the schema stack would have to grow.
///
/// `Never` is retained as the *concept* marker so the absent-leg binding has a
/// name in component signatures, but it deliberately carries **only** the
/// derives a zero-variant enum tolerates — not the wire-derive stack. A `Work`
/// or `Action` whose leg is bound to this `Never` therefore cannot itself
/// derive the wire stack; that is the disproof, made type-level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Never {}

/// The runner seam, proven without the associated-type `NexusAction` shim.
///
/// `triad_runtime::NextStep<Reply, SemaWrite, SemaRead, Effect, Work>` and
/// `Action<Reply, Write, Read, Effect, Continuation>` are structurally the same
/// five-way sum, so the projection is a direct, total `From` — no
/// `RunnerEngines::*` associated types, no `into_next_step` trait method. The
/// runner can be driven straight off `Action` by converting at the seam.
impl<Reply, Write, Read, Effect, Continuation>
    From<Action<Reply, Write, Read, Effect, Continuation>>
    for NextStep<Reply, Write, Read, Effect, Continuation>
{
    fn from(action: Action<Reply, Write, Read, Effect, Continuation>) -> Self {
        match action {
            Action::ReplyToSignal(reply) => NextStep::Reply(reply),
            Action::CommandSemaWrite(write) => NextStep::SemaWrite(write),
            Action::CommandSemaRead(read) => NextStep::SemaRead(read),
            Action::CommandEffect(effect) => NextStep::RunEffect(effect),
            Action::Continue(work) => NextStep::Continue(work),
        }
    }
}
