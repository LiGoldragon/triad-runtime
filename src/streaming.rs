use std::marker::PhantomData;

use signal_frame::{
    ExchangeLane, LaneSequence, SessionEpoch, ShortHeader, StreamEventIdentifier, StreamingFrame,
    StreamingFrameBody, SubscriptionTokenInner,
};

pub trait SubscriptionToken: Copy + Eq {
    fn from_inner(inner: SubscriptionTokenInner) -> Self;

    fn into_inner(self) -> SubscriptionTokenInner;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SubscriptionTokenIssuer {
    next_value: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubscriptionRegistry<Token, Filter>
where
    Token: SubscriptionToken,
{
    subscriptions: Vec<Subscription<Token, Filter>>,
    token_issuer: SubscriptionTokenIssuer,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Subscription<Token, Filter>
where
    Token: SubscriptionToken,
{
    token: Token,
    filter: Filter,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SubscriptionEventSequence {
    session_epoch: SessionEpoch,
    lane: ExchangeLane,
    next_sequence: LaneSequence,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubscriptionEventPublisher<RequestPayload, ReplyPayload, EventPayload> {
    short_header: ShortHeader,
    sequence: SubscriptionEventSequence,
    payload_types: PhantomData<(RequestPayload, ReplyPayload, EventPayload)>,
}

impl SubscriptionToken for SubscriptionTokenInner {
    fn from_inner(inner: SubscriptionTokenInner) -> Self {
        inner
    }

    fn into_inner(self) -> SubscriptionTokenInner {
        self
    }
}

impl Default for SubscriptionTokenIssuer {
    fn default() -> Self {
        Self::new(1)
    }
}

impl SubscriptionTokenIssuer {
    pub const fn new(first_value: u64) -> Self {
        Self {
            next_value: first_value,
        }
    }

    pub fn next_value(&self) -> u64 {
        self.next_value
    }

    pub fn issue<Token>(&mut self) -> Token
    where
        Token: SubscriptionToken,
    {
        let token = Token::from_inner(SubscriptionTokenInner::new(self.next_value));
        self.next_value = self.next_value.wrapping_add(1);
        token
    }
}

impl<Token, Filter> Default for SubscriptionRegistry<Token, Filter>
where
    Token: SubscriptionToken,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<Token, Filter> SubscriptionRegistry<Token, Filter>
where
    Token: SubscriptionToken,
{
    pub fn new() -> Self {
        Self::with_token_issuer(SubscriptionTokenIssuer::default())
    }

    pub fn with_token_issuer(token_issuer: SubscriptionTokenIssuer) -> Self {
        Self {
            subscriptions: Vec::new(),
            token_issuer,
        }
    }

    pub fn token_issuer(&self) -> SubscriptionTokenIssuer {
        self.token_issuer
    }

    pub fn register(&mut self, filter: Filter) -> Token {
        let token = self.token_issuer.issue();
        self.subscriptions.push(Subscription { token, filter });
        token
    }

    pub fn register_token(&mut self, token: Token, filter: Filter) {
        self.unregister(token);
        self.subscriptions.push(Subscription { token, filter });
    }

    pub fn unregister(&mut self, token: Token) -> bool {
        let before = self.subscriptions.len();
        self.subscriptions
            .retain(|subscription| subscription.token != token);
        self.subscriptions.len() != before
    }

    pub fn len(&self) -> usize {
        self.subscriptions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.subscriptions.is_empty()
    }

    pub fn publish_matching<Event>(
        &self,
        event: &Event,
        mut matches: impl FnMut(&Filter, &Event) -> bool,
        mut deliver: impl FnMut(Token, &Event),
    ) {
        for subscription in &self.subscriptions {
            if matches(&subscription.filter, event) {
                deliver(subscription.token, event);
            }
        }
    }
}

impl<Token, Filter> Subscription<Token, Filter>
where
    Token: SubscriptionToken,
{
    pub fn token(&self) -> Token {
        self.token
    }

    pub fn filter(&self) -> &Filter {
        &self.filter
    }
}

impl SubscriptionEventSequence {
    pub const fn new(
        session_epoch: SessionEpoch,
        lane: ExchangeLane,
        next_sequence: LaneSequence,
    ) -> Self {
        Self {
            session_epoch,
            lane,
            next_sequence,
        }
    }

    pub const fn acceptor(session_epoch: SessionEpoch) -> Self {
        Self::new(session_epoch, ExchangeLane::Acceptor, LaneSequence::first())
    }

    pub fn session_epoch(&self) -> SessionEpoch {
        self.session_epoch
    }

    pub fn lane(&self) -> ExchangeLane {
        self.lane
    }

    pub fn next_sequence(&self) -> LaneSequence {
        self.next_sequence
    }

    pub fn next_identifier(&mut self) -> StreamEventIdentifier {
        let identifier =
            StreamEventIdentifier::new(self.session_epoch, self.lane, self.next_sequence);
        self.next_sequence = self.next_sequence.next();
        identifier
    }
}

impl<RequestPayload, ReplyPayload, EventPayload>
    SubscriptionEventPublisher<RequestPayload, ReplyPayload, EventPayload>
{
    pub const fn new(short_header: ShortHeader, sequence: SubscriptionEventSequence) -> Self {
        Self {
            short_header,
            sequence,
            payload_types: PhantomData,
        }
    }

    pub const fn acceptor(short_header: ShortHeader, session_epoch: SessionEpoch) -> Self {
        Self::new(
            short_header,
            SubscriptionEventSequence::acceptor(session_epoch),
        )
    }

    pub fn short_header(&self) -> ShortHeader {
        self.short_header
    }

    pub fn sequence(&self) -> SubscriptionEventSequence {
        self.sequence
    }

    pub fn publish<Token>(
        &mut self,
        token: Token,
        event: EventPayload,
    ) -> StreamingFrame<RequestPayload, ReplyPayload, EventPayload>
    where
        Token: SubscriptionToken,
    {
        StreamingFrame::with_short_header(
            self.short_header,
            StreamingFrameBody::SubscriptionEvent {
                event_identifier: self.sequence.next_identifier(),
                token: token.into_inner(),
                event,
            },
        )
    }
}
