use std::num::{NonZeroU16, NonZeroU32};

use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};
use signal_frame::{
    BoundStreamingFrame, ContractBinding, ContractId, ExchangeLane, LaneSequence, RootCode,
    SessionEpoch, StreamingFrameBody, SubscriptionTokenInner, VariantCode, WireContract,
    WireRevision, WireRoute,
};
use triad_runtime::{
    SubscriptionEventPublisher, SubscriptionEventSequence, SubscriptionRegistry, SubscriptionToken,
    SubscriptionTokenIssuer,
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

#[test]
fn subscription_token_issuer_wraps_inner_tokens() {
    let mut issuer = SubscriptionTokenIssuer::new(9);

    let first: TestSubscriptionToken = issuer.issue();
    let second: TestSubscriptionToken = issuer.issue();

    assert_eq!(first.into_inner(), SubscriptionTokenInner::new(9));
    assert_eq!(second.into_inner(), SubscriptionTokenInner::new(10));
    assert_eq!(issuer.next_value(), 11);
}

#[test]
fn subscription_registry_registers_unregisters_and_delivers_matching_events() {
    let mut registry = SubscriptionRegistry::<TestSubscriptionToken, TestFilter>::new();
    let all = registry.register(TestFilter::All);
    let beta = registry.register(TestFilter::Label("beta"));
    let gamma = registry.register(TestFilter::Label("gamma"));

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
fn subscription_event_sequence_mints_monotonic_acceptor_identifiers() {
    let mut sequence = SubscriptionEventSequence::acceptor(SessionEpoch::new(7));

    let first = sequence.next_identifier();
    let second = sequence.next_identifier();

    assert_eq!(first.session_epoch(), SessionEpoch::new(7));
    assert_eq!(first.lane(), ExchangeLane::Acceptor);
    assert_eq!(first.sequence(), LaneSequence::first());
    assert_eq!(second.session_epoch(), SessionEpoch::new(7));
    assert_eq!(second.sequence(), LaneSequence::new(1));
    assert_eq!(sequence.next_sequence(), LaneSequence::new(2));
}

#[test]
fn subscription_event_publisher_builds_exactly_bound_streaming_events() {
    let token = TestSubscriptionToken(SubscriptionTokenInner::new(44));
    let mut publisher =
        SubscriptionEventPublisher::<TestContract, TestRequest, TestReply, TestEvent>::acceptor(
            EVENT_ROUTE,
            SessionEpoch::new(3),
        );

    let frame = publisher.publish(token, TestEvent::new("commit"));
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

    let next_frame = publisher.publish(token, TestEvent::new("next"));
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
