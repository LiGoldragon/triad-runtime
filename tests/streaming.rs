use std::{
    cell::RefCell,
    num::{NonZeroU16, NonZeroU32},
    rc::Rc,
};

use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};
use signal_frame::{
    BoundStreamingFrame, ContractBinding, ContractId, ExchangeLane, LaneSequence, RootCode,
    SessionEpoch, StreamingFrameBody, SubscriptionTokenInner, VariantCode, WireContract,
    WireRevision, WireRoute,
};
use triad_runtime::{
    SubscriptionEventEpochAuthority, SubscriptionEventEpochError,
    SubscriptionEventEpochReservation, SubscriptionEventEpochStore, SubscriptionEventPublisher,
    SubscriptionRegistry, SubscriptionToken, SubscriptionTokenIssuer,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TestSubscriptionToken(SubscriptionTokenInner);

#[derive(Clone, Debug, Eq, PartialEq)]
enum TestFilter {
    All,
    Label(&'static str),
}

#[derive(Archive, RkyvSerialize, RkyvDeserialize, Clone, Debug, Eq, PartialEq)]
struct TestRequest {
    label: String,
}

#[derive(Archive, RkyvSerialize, RkyvDeserialize, Clone, Debug, Eq, PartialEq)]
struct TestReply {
    accepted: bool,
}

#[derive(Archive, RkyvSerialize, RkyvDeserialize, Clone, Debug, Eq, PartialEq)]
struct TestEvent {
    label: String,
}

struct TestContract;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TestEpochStoreError {
    Unavailable,
}

struct TestEpochStore {
    state: Rc<RefCell<Option<SessionEpoch>>>,
    fail_next_reservation: bool,
}

impl WireContract for TestContract {
    const BINDING: ContractBinding = ContractBinding::new(
        ContractId::new(NonZeroU32::new(0x1020_3040).unwrap()),
        WireRevision::new(NonZeroU16::new(7).unwrap()),
    );
}

const EVENT_ROUTE: WireRoute = WireRoute::new(RootCode::new(0x91), VariantCode::new(0x2a));

impl SubscriptionToken for TestSubscriptionToken {
    fn from_inner(inner: SubscriptionTokenInner) -> Self {
        Self(inner)
    }

    fn into_inner(self) -> SubscriptionTokenInner {
        self.0
    }
}

impl TestEvent {
    fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
        }
    }
}

impl TestFilter {
    fn matches(&self, event: &TestEvent) -> bool {
        match self {
            Self::All => true,
            Self::Label(label) => event.label == *label,
        }
    }
}

impl TestEpochStore {
    fn new(state: Rc<RefCell<Option<SessionEpoch>>>) -> Self {
        Self {
            state,
            fail_next_reservation: false,
        }
    }

    fn failing(state: Rc<RefCell<Option<SessionEpoch>>>) -> Self {
        Self {
            state,
            fail_next_reservation: true,
        }
    }
}

impl SubscriptionEventEpochStore for TestEpochStore {
    type Error = TestEpochStoreError;

    fn reserve_next_epoch(
        &mut self,
        reservation: SubscriptionEventEpochReservation<'_>,
    ) -> Result<triad_runtime::SubscriptionEventEpoch, SubscriptionEventEpochError<Self::Error>>
    {
        if self.fail_next_reservation {
            self.fail_next_reservation = false;
            return Err(SubscriptionEventEpochError::Store(
                TestEpochStoreError::Unavailable,
            ));
        }

        let mut state = self.state.borrow_mut();
        let Some(epoch) = *state else {
            return Err(SubscriptionEventEpochError::EpochExhausted);
        };
        *state = epoch.value().checked_add(1).map(SessionEpoch::new);
        Ok(reservation.commit(epoch))
    }
}

fn epoch_state(first_epoch: u64) -> Rc<RefCell<Option<SessionEpoch>>> {
    Rc::new(RefCell::new(Some(SessionEpoch::new(first_epoch))))
}

#[test]
fn subscription_token_issuer_wraps_inner_tokens() {
    let mut issuer = SubscriptionTokenIssuer::new();

    let first: TestSubscriptionToken = issuer.mint().expect("mint first token");
    let second: TestSubscriptionToken = issuer.mint().expect("mint second token");

    assert_eq!(first.into_inner(), SubscriptionTokenInner::new(1));
    assert_eq!(second.into_inner(), SubscriptionTokenInner::new(2));
}

#[test]
fn subscription_registry_registers_unregisters_and_delivers_matching_events() {
    let mut registry = SubscriptionRegistry::<TestSubscriptionToken, TestFilter>::new();
    let all = registry.register(TestFilter::All).expect("register all");
    let beta = registry
        .register(TestFilter::Label("beta"))
        .expect("register beta");
    let gamma = registry
        .register(TestFilter::Label("gamma"))
        .expect("register gamma");

    assert_eq!(registry.len(), 3);
    assert!(!registry.is_empty());

    let event = TestEvent::new("beta");
    let mut delivered = Vec::new();
    registry.publish_matching(&event, TestFilter::matches, |token, delivered_event| {
        delivered.push((token, delivered_event.clone()));
    });

    assert_eq!(
        delivered,
        vec![
            (all, TestEvent::new("beta")),
            (beta, TestEvent::new("beta"))
        ]
    );

    assert!(registry.unregister(beta));
    assert!(!registry.unregister(beta));

    let mut after_unregister = Vec::new();
    registry.publish_matching(&event, TestFilter::matches, |token, _event| {
        after_unregister.push(token);
    });
    assert_eq!(after_unregister, vec![all]);
    assert_eq!(registry.len(), 2);
    assert!(registry.unregister(all));
    assert!(registry.unregister(gamma));
    assert!(registry.is_empty());
}

#[test]
fn subscription_registry_can_register_an_already_minted_token() {
    let mut registry = SubscriptionRegistry::<TestSubscriptionToken, TestFilter>::new();
    let token = TestSubscriptionToken(SubscriptionTokenInner::new(88));

    registry.register_token(token, TestFilter::Label("alpha"));

    let mut delivered = Vec::new();
    registry.publish_matching(
        &TestEvent::new("alpha"),
        TestFilter::matches,
        |token, _event| {
            delivered.push(token);
        },
    );
    assert_eq!(delivered, vec![token]);

    registry.register_token(token, TestFilter::Label("beta"));

    let mut after_replacement = Vec::new();
    registry.publish_matching(
        &TestEvent::new("alpha"),
        TestFilter::matches,
        |token, _event| {
            after_replacement.push(token);
        },
    );
    assert!(after_replacement.is_empty());
    assert_eq!(registry.len(), 1);
}

#[test]
fn subscription_event_authority_reserves_distinct_epochs_and_publishers_start_sequences() {
    let token = TestSubscriptionToken(SubscriptionTokenInner::new(44));
    let mut authority = SubscriptionEventEpochAuthority::new(TestEpochStore::new(epoch_state(7)));
    let mut first =
        SubscriptionEventPublisher::<TestContract, TestRequest, TestReply, TestEvent>::new(
            EVENT_ROUTE,
            authority.reserve().expect("reserve first epoch"),
        );
    let mut second =
        SubscriptionEventPublisher::<TestContract, TestRequest, TestReply, TestEvent>::new(
            EVENT_ROUTE,
            authority.reserve().expect("reserve second epoch"),
        );

    let first = first
        .publish(token, TestEvent::new("first"))
        .expect("publish first");
    let second = second
        .publish(token, TestEvent::new("second"))
        .expect("publish second");
    let StreamingFrameBody::SubscriptionEvent {
        event_identifier: first_identifier,
        ..
    } = first.into_body()
    else {
        panic!("expected subscription event");
    };
    let StreamingFrameBody::SubscriptionEvent {
        event_identifier: second_identifier,
        ..
    } = second.into_body()
    else {
        panic!("expected subscription event");
    };

    assert_eq!(first_identifier.session_epoch(), SessionEpoch::new(7));
    assert_eq!(second_identifier.session_epoch(), SessionEpoch::new(8));
    assert_eq!(first_identifier.lane(), ExchangeLane::Acceptor);
    assert_eq!(second_identifier.lane(), ExchangeLane::Acceptor);
    assert_eq!(first_identifier.sequence(), LaneSequence::first());
    assert_eq!(second_identifier.sequence(), LaneSequence::first());
}

#[test]
fn subscription_event_publisher_builds_exactly_bound_streaming_events() {
    let token = TestSubscriptionToken(SubscriptionTokenInner::new(44));
    let mut authority = SubscriptionEventEpochAuthority::new(TestEpochStore::new(epoch_state(3)));
    let mut publisher =
        SubscriptionEventPublisher::<TestContract, TestRequest, TestReply, TestEvent>::new(
            EVENT_ROUTE,
            authority.reserve().expect("reserve epoch"),
        );

    let frame = publisher
        .publish(token, TestEvent::new("commit"))
        .expect("publish event");
    let _: &BoundStreamingFrame<TestContract, TestRequest, TestReply, TestEvent> = &frame;
    let bytes = frame.encode_length_prefixed().expect("encode frame");
    let decoded = BoundStreamingFrame::<TestContract, TestRequest, TestReply, TestEvent>::
        decode_length_prefixed(&bytes)
        .expect("decode frame");

    assert_eq!(decoded.short_header().binding(), TestContract::BINDING);
    assert_eq!(decoded.short_header().route(), EVENT_ROUTE);
    match decoded.into_body() {
        StreamingFrameBody::SubscriptionEvent {
            event_identifier,
            token: decoded_token,
            event,
        } => {
            assert_eq!(event_identifier.session_epoch(), SessionEpoch::new(3));
            assert_eq!(event_identifier.lane(), ExchangeLane::Acceptor);
            assert_eq!(event_identifier.sequence(), LaneSequence::first());
            assert_eq!(decoded_token, SubscriptionTokenInner::new(44));
            assert_eq!(event, TestEvent::new("commit"));
        }
        _ => panic!("expected subscription event"),
    }

    let next_frame = publisher
        .publish(token, TestEvent::new("next"))
        .expect("publish next event");
    match next_frame.into_body() {
        StreamingFrameBody::SubscriptionEvent {
            event_identifier, ..
        } => {
            assert_eq!(event_identifier.sequence(), LaneSequence::new(1));
        }
        _ => panic!("expected subscription event"),
    }
}

#[test]
fn subscription_event_authority_exhausts_after_issuing_maximum_epoch() {
    let token = TestSubscriptionToken(SubscriptionTokenInner::new(44));
    let state = epoch_state(u64::MAX);
    let mut authority = SubscriptionEventEpochAuthority::new(TestEpochStore::new(state.clone()));
    let mut publisher =
        SubscriptionEventPublisher::<TestContract, TestRequest, TestReply, TestEvent>::new(
            EVENT_ROUTE,
            authority.reserve().expect("reserve maximum epoch"),
        );

    let frame = publisher
        .publish(token, TestEvent::new("maximum epoch"))
        .expect("publish maximum epoch");
    let StreamingFrameBody::SubscriptionEvent {
        event_identifier, ..
    } = frame.into_body()
    else {
        panic!("expected subscription event");
    };
    assert_eq!(
        event_identifier.session_epoch(),
        SessionEpoch::new(u64::MAX)
    );
    assert_eq!(*state.borrow(), None);
    assert_eq!(
        authority.reserve(),
        Err(SubscriptionEventEpochError::EpochExhausted)
    );
    assert_eq!(
        authority.reserve(),
        Err(SubscriptionEventEpochError::EpochExhausted)
    );
    assert_eq!(*state.borrow(), None);
}

#[test]
fn subscription_event_authority_restart_reuses_the_same_store_state() {
    let token = TestSubscriptionToken(SubscriptionTokenInner::new(44));
    let persisted_state = epoch_state(3);
    let mut authority =
        SubscriptionEventEpochAuthority::new(TestEpochStore::new(persisted_state.clone()));
    let mut before_restart =
        SubscriptionEventPublisher::<TestContract, TestRequest, TestReply, TestEvent>::new(
            EVENT_ROUTE,
            authority.reserve().expect("reserve pre-restart epoch"),
        );
    drop(authority);
    let mut restarted =
        SubscriptionEventEpochAuthority::new(TestEpochStore::new(persisted_state.clone()));
    let mut after_restart =
        SubscriptionEventPublisher::<TestContract, TestRequest, TestReply, TestEvent>::new(
            EVENT_ROUTE,
            restarted.reserve().expect("reserve post-restart epoch"),
        );

    let before_restart = before_restart
        .publish(token, TestEvent::new("before restart"))
        .expect("publish before restart");
    let after_restart = after_restart
        .publish(token, TestEvent::new("after restart"))
        .expect("publish after restart");
    let StreamingFrameBody::SubscriptionEvent {
        event_identifier: before_restart_identifier,
        ..
    } = before_restart.into_body()
    else {
        panic!("expected subscription event");
    };
    let StreamingFrameBody::SubscriptionEvent {
        event_identifier: after_restart_identifier,
        ..
    } = after_restart.into_body()
    else {
        panic!("expected subscription event");
    };

    assert_eq!(
        before_restart_identifier.session_epoch(),
        SessionEpoch::new(3)
    );
    assert_eq!(
        after_restart_identifier.session_epoch(),
        SessionEpoch::new(4)
    );
    assert_eq!(before_restart_identifier.sequence(), LaneSequence::first());
    assert_eq!(after_restart_identifier.sequence(), LaneSequence::first());
}

#[test]
fn subscription_event_authority_propagates_typed_store_errors() {
    let state = epoch_state(9);
    let mut authority =
        SubscriptionEventEpochAuthority::new(TestEpochStore::failing(state.clone()));

    assert_eq!(
        authority.reserve(),
        Err(SubscriptionEventEpochError::Store(
            TestEpochStoreError::Unavailable
        ))
    );
    assert_eq!(*state.borrow(), Some(SessionEpoch::new(9)));

    authority
        .reserve()
        .expect("reserve after transient failure");
    assert_eq!(*state.borrow(), Some(SessionEpoch::new(10)));
}

#[test]
fn replacing_a_stale_registration_keeps_only_the_latest_filter() {
    let mut registry = SubscriptionRegistry::<TestSubscriptionToken, TestFilter>::new();
    let token = TestSubscriptionToken(SubscriptionTokenInner::new(88));
    registry.register_token(token, TestFilter::Label("stale"));
    registry.register_token(token, TestFilter::Label("current"));

    let mut stale_deliveries = Vec::new();
    registry.publish_matching(
        &TestEvent::new("stale"),
        TestFilter::matches,
        |token, _event| stale_deliveries.push(token),
    );
    assert!(stale_deliveries.is_empty());

    let mut current_deliveries = Vec::new();
    registry.publish_matching(
        &TestEvent::new("current"),
        TestFilter::matches,
        |token, _event| current_deliveries.push(token),
    );
    assert_eq!(current_deliveries, vec![token]);
    assert_eq!(registry.len(), 1);
}
