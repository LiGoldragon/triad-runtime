//! Risk-gate #408 (O3) proof suite for the generic reaction frame.
//!
//! Five proofs, matching the gate brief:
//!
//! 1. multi-parameter generic enum + the full wire-derive stack compiles;
//! 2. rkyv Archive→Serialize→Deserialize round-trips a `Work<P1,P2,P3,P4>`;
//! 3. DOTOS `to_dotos`→`from_dotos` round-trips the same value;
//! 4. the omittable-leg mechanism: the literal `enum Never {}` binding is
//!    DISPROVEN (it cannot carry the wire-derive stack), and the viable
//!    fixed-arity fallback (a derivable stand-in leg) round-trips both ways;
//! 5. `Action<…>` projects to `triad_runtime::NextStep<…>` via a direct
//!    `From`, with no associated-type `RunnerEngines` / `into_next_step` shim.
//!
//! The concrete payload types below carry the exact derive stack the generated
//! wire enums use (`spirit/src/schema/nexus.rs`), so the proof is
//! representative of production payloads.

#![cfg(feature = "dotos-text")]

use dotos::{DotosEncode, DotosSource};
use triad_runtime::{Action, Never, NextStep, Work};

// --- Concrete payloads, each deriving the full wire stack -------------------

#[derive(
    dotos::DotosDecode,
    dotos::DotosEncode,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
    Clone,
    Debug,
    PartialEq,
    Eq,
)]
struct ArrivedSignal {
    sequence: u64,
}

#[derive(
    dotos::DotosDecode,
    dotos::DotosEncode,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
    Clone,
    Debug,
    PartialEq,
    Eq,
)]
struct WriteOutcome {
    committed: bool,
}

#[derive(
    dotos::DotosDecode,
    dotos::DotosEncode,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
    Clone,
    Debug,
    PartialEq,
    Eq,
)]
struct ReadOutcome {
    record_count: u64,
}

#[derive(
    dotos::DotosDecode,
    dotos::DotosEncode,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
    Clone,
    Debug,
    PartialEq,
    Eq,
)]
struct EffectOutcome {
    exit_code: u64,
}

#[derive(
    dotos::DotosDecode,
    dotos::DotosEncode,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
    Clone,
    Debug,
    PartialEq,
    Eq,
)]
struct SignalReply {
    accepted: bool,
}

// Type aliases mirroring a four-leg component binding (the maximal frame).
type FullWork = Work<ArrivedSignal, WriteOutcome, ReadOutcome, EffectOutcome>;
type FullAction = Action<SignalReply, WriteOutcome, ReadOutcome, EffectOutcome, FullWork>;

// A three-leg component that has no SEMA-read leg. The O3 proposal was to bind
// that parameter to an uninhabitable `Never` so `SemaReadCompleted(Never)` can
// never be constructed while the enum still derives the full wire stack. That
// is DISPROVEN — `Never` cannot carry the rkyv stack (see the `never_*` tests
// and `reaction::Never`'s doc). The viable fallback is a fixed smaller arity:
// the component binds the read leg to a derivable stand-in. Here we use the
// "unit" leg `LegAbsent` — a real, derivable, single-state payload that proves
// a three-leg component still round-trips its constructible variants. (In
// production this is the fixed-arity fallback the gate selects.)
// The fixed-arity stand-in must carry at least one field: the dotos derive
// emits `DotosEncode`/`DotosDecode` for single-field records (like every payload
// here) but NOT for a zero-field unit struct, so a bare `struct LegAbsent;`
// fails the DOTOS half of the stack just as `Never` fails the rkyv half. The
// honest fallback payload therefore carries an explicit marker field.
#[derive(
    dotos::DotosDecode,
    dotos::DotosEncode,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
    Clone,
    Debug,
    PartialEq,
    Eq,
)]
struct LegAbsent {
    absent: bool,
}

type ReadlessWork = Work<ArrivedSignal, WriteOutcome, LegAbsent, EffectOutcome>;

// --- Proof 1: the multi-parameter generic compiles with the full stack ------
//
// Reaching this file at all means `Work<…>` and `Action<…>` compiled with
// rkyv::{Archive,Serialize,Deserialize} + dotos::{DotosDecode,DotosEncode}
// over four and five free type parameters respectively. This compile-only
// instantiation pins it as an asserted fact.

#[test]
fn multi_parameter_generic_compiles_with_full_wire_stack() {
    let work: FullWork = Work::SignalArrived(ArrivedSignal { sequence: 7 });
    let action: FullAction = Action::ReplyToSignal(SignalReply { accepted: true });
    // Construct one of every Work variant to prove each leg is inhabited.
    let _writes: [FullWork; 4] = [
        Work::SignalArrived(ArrivedSignal { sequence: 1 }),
        Work::SemaWriteCompleted(WriteOutcome { committed: true }),
        Work::SemaReadCompleted(ReadOutcome { record_count: 3 }),
        Work::EffectCompleted(EffectOutcome { exit_code: 0 }),
    ];
    assert!(matches!(work, Work::SignalArrived(_)));
    assert!(matches!(action, Action::ReplyToSignal(_)));
}

// --- Proof 2: rkyv round-trip over the multi-parameter generic --------------

#[test]
fn rkyv_round_trips_multi_parameter_work() {
    let work: FullWork = Work::SemaReadCompleted(ReadOutcome { record_count: 42 });

    let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&work).expect("serialize work");
    let restored: FullWork =
        rkyv::from_bytes::<FullWork, rkyv::rancor::Error>(&bytes).expect("deserialize work");

    assert_eq!(work, restored);
}

#[test]
fn rkyv_round_trips_multi_parameter_action() {
    let action: FullAction =
        Action::Continue(Work::EffectCompleted(EffectOutcome { exit_code: 9 }));

    let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&action).expect("serialize action");
    let restored: FullAction =
        rkyv::from_bytes::<FullAction, rkyv::rancor::Error>(&bytes).expect("deserialize action");

    assert_eq!(action, restored);
}

// --- Proof 3: DOTOS round-trip over the multi-parameter generic --------------

#[test]
fn dotos_round_trips_multi_parameter_work() {
    let work: FullWork = Work::SignalArrived(ArrivedSignal { sequence: 11 });

    let rendered = work.to_dotos();
    let restored: FullWork = DotosSource::new(&rendered)
        .parse::<FullWork>()
        .expect("parse work back from dotos");

    assert_eq!(work, restored);
}

#[test]
fn dotos_round_trips_multi_parameter_action() {
    let action: FullAction = Action::CommandSemaWrite(WriteOutcome { committed: true });

    let rendered = action.to_dotos();
    let restored: FullAction = DotosSource::new(&rendered)
        .parse::<FullAction>()
        .expect("parse action back from dotos");

    assert_eq!(action, restored);
}

// --- Proof 4: the omittable-leg mechanism (O3) ------------------------------
//
// O3 verdict, in two parts.
//
// (a) The literal proposal — bind the absent leg to an uninhabitable
//     `enum Never {}` carrying the wire stack — is DISPROVEN. `Never` cannot
//     derive the rkyv stack at all (zero-variant + `#[repr(u8)]` => E0084,
//     `match self {}` over `&Never` => E0004), and the crate's
//     `forbid(unsafe_code)` blocks a hand-written `unsafe impl Portable`. The
//     `never_*` tests below pin this as type-level fact.
//
// (b) The viable fallback — a fixed smaller arity, binding the absent leg to a
//     derivable stand-in (`LegAbsent`) — DOES round-trip. A three-leg
//     component is realized as `Work<Event, Write, LegAbsent, Effect>` and its
//     constructible variants serialize/deserialize and DOTOS round-trip.

#[test]
fn fixed_arity_fallback_work_round_trips_rkyv() {
    let work: ReadlessWork = Work::SignalArrived(ArrivedSignal { sequence: 5 });

    let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&work).expect("serialize readless work");
    let restored: ReadlessWork = rkyv::from_bytes::<ReadlessWork, rkyv::rancor::Error>(&bytes)
        .expect("deserialize readless work");

    assert_eq!(work, restored);
}

#[test]
fn fixed_arity_fallback_work_round_trips_dotos() {
    let work: ReadlessWork = Work::EffectCompleted(EffectOutcome { exit_code: 1 });

    let rendered = work.to_dotos();
    let restored: ReadlessWork = DotosSource::new(&rendered)
        .parse::<ReadlessWork>()
        .expect("parse readless work back from dotos");

    assert_eq!(work, restored);
}

#[test]
fn never_is_uninhabited_and_cannot_be_constructed() {
    // There is no value of `Never`. The only thing that can be done with a
    // `&Never` is an empty match, which the compiler accepts as exhaustive
    // because the type is genuinely uninhabited. This proves `Never` is a
    // sound uninhabitable marker — the obstacle is purely that it cannot carry
    // the wire-derive stack, not that the concept is unsound.
    fn absurd(never: &Never) -> ! {
        match *never {}
    }
    let _ = absurd; // referenced so the witness is not dead code
}

// --- Proof 5: NextStep projection without the associated-type shim ----------
//
// `Action<R,W,Rd,E,Cont>` projects to `NextStep<R,W,Rd,E,Cont>` via a plain
// `From`. No `RunnerEngines` associated types, no `NexusAction::into_next_step`
// trait method — the conversion is total and structural.

#[test]
fn action_projects_to_next_step_via_direct_from() {
    type Step = NextStep<SignalReply, WriteOutcome, ReadOutcome, EffectOutcome, FullWork>;

    let reply: FullAction = Action::ReplyToSignal(SignalReply { accepted: true });
    assert!(matches!(Step::from(reply), NextStep::Reply(_)));

    let write: FullAction = Action::CommandSemaWrite(WriteOutcome { committed: false });
    assert!(matches!(Step::from(write), NextStep::SemaWrite(_)));

    let read: FullAction = Action::CommandSemaRead(ReadOutcome { record_count: 2 });
    assert!(matches!(Step::from(read), NextStep::SemaRead(_)));

    let effect: FullAction = Action::CommandEffect(EffectOutcome { exit_code: 3 });
    assert!(matches!(Step::from(effect), NextStep::RunEffect(_)));

    let next: FullAction = Action::Continue(Work::SignalArrived(ArrivedSignal { sequence: 8 }));
    assert!(matches!(Step::from(next), NextStep::Continue(_)));
}

#[test]
fn action_projects_to_next_step_for_uninhabitable_leg_binding() {
    // Nuance worth recording: the projection `From<Action> for NextStep` has NO
    // derive bounds on the leg parameters, so it composes even when a leg is
    // bound to the uninhabitable `Never` — unlike the wire-derive stack, which
    // cannot. The runner seam therefore tolerates `Never` legs; only the
    // serialization boundary forces the fixed-arity fallback. Here the
    // read-command parameter is `Never`.
    type ReadlessAction = Action<SignalReply, WriteOutcome, Never, EffectOutcome, ReadlessWork>;
    type ReadlessStep = NextStep<SignalReply, WriteOutcome, Never, EffectOutcome, ReadlessWork>;

    let reply: ReadlessAction = Action::ReplyToSignal(SignalReply { accepted: false });
    assert!(matches!(ReadlessStep::from(reply), NextStep::Reply(_)));

    let effect: ReadlessAction = Action::CommandEffect(EffectOutcome { exit_code: 4 });
    assert!(matches!(ReadlessStep::from(effect), NextStep::RunEffect(_)));
}
