use std::marker::PhantomData;

use signal_frame::{
    BoundStreamingFrame, LaneSequence, SessionEpoch, StreamEventIdentifier, StreamingFrameBody,
    SubscriptionTokenInner, WireContract, WireRoute,
};

pub trait SubscriptionToken: Copy + Eq {
    fn from_inner(inner: SubscriptionTokenInner) -> Self;

    fn into_inner(self) -> SubscriptionTokenInner;
}

pub struct SubscriptionTokenIssuer {
    next_value: Option<u64>,
}

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

pub struct SubscriptionEventEpochAuthority<Store> {
    store: Store,
}

#[derive(Debug, Eq, PartialEq)]
pub struct SubscriptionEventEpoch {
    session_epoch: SessionEpoch,
}

pub struct SubscriptionEventEpochReservation<'authority> {
    authority: PhantomData<&'authority mut ()>,
}

pub trait SubscriptionEventEpochStore {
    type Error;

    /// Atomically advances durable reservation state before returning the epoch.
    ///
    /// A successful implementation must make the advance durable before it
    /// calls [`SubscriptionEventEpochReservation::commit`]. Skipping an epoch
    /// after a crash is safe; returning an epoch whose advance was not made
    /// durable can reuse an event identifier.
    fn reserve_next_epoch(
        &mut self,
        reservation: SubscriptionEventEpochReservation<'_>,
    ) -> Result<SubscriptionEventEpoch, SubscriptionEventEpochError<Self::Error>>;
}

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum SubscriptionEventEpochError<StoreError> {
    #[error("subscription event epoch space is exhausted")]
    EpochExhausted,
    #[error("subscription event epoch store rejected the reservation")]
    Store(StoreError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SubscriptionTokenError {
    #[error("subscription token space is exhausted")]
    TokenExhausted,
    #[error("minted subscription token collides with a live registration")]
    TokenCollision,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SubscriptionPublishError {
    #[error("subscription event sequence space is exhausted")]
    SequenceExhausted,
}

#[derive(Debug, Eq, PartialEq)]
struct SubscriptionEventSequence {
    session_epoch: SessionEpoch,
    next_sequence: Option<LaneSequence>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct SubscriptionEventPublisher<Contract, RequestPayload, ReplyPayload, EventPayload>
where
    Contract: WireContract,
{
    event_route: WireRoute,
    sequence: SubscriptionEventSequence,
    contract: PhantomData<fn() -> Contract>,
    request_payload: PhantomData<fn() -> RequestPayload>,
    reply_payload: PhantomData<fn() -> ReplyPayload>,
    event_payload: PhantomData<fn() -> EventPayload>,
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
        Self::new()
    }
}

impl SubscriptionTokenIssuer {
    /// Creates a fresh issuer whose first token has the established value `1`.
    pub const fn new() -> Self {
        Self {
            next_value: Some(1),
        }
    }

    #[cfg(test)]
    const fn from_next_value(next_value: u64) -> Self {
        Self {
            next_value: Some(next_value),
        }
    }

    pub fn mint<Token>(&mut self) -> Result<Token, SubscriptionTokenError>
    where
        Token: SubscriptionToken,
    {
        let Some(value) = self.next_value else {
            return Err(SubscriptionTokenError::TokenExhausted);
        };
        self.next_value = value.checked_add(1);
        Ok(Token::from_inner(SubscriptionTokenInner::new(value)))
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
        Self {
            subscriptions: Vec::new(),
            token_issuer: SubscriptionTokenIssuer::new(),
        }
    }

    pub fn register(&mut self, filter: Filter) -> Result<Token, SubscriptionTokenError> {
        let token = self.token_issuer.mint()?;
        if self
            .subscriptions
            .iter()
            .any(|subscription| subscription.token == token)
        {
            return Err(SubscriptionTokenError::TokenCollision);
        }
        self.subscriptions.push(Subscription { token, filter });
        Ok(token)
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

impl<'authority> SubscriptionEventEpochReservation<'authority> {
    /// Completes the store's reservation after its state advance is durable.
    pub fn commit(self, session_epoch: SessionEpoch) -> SubscriptionEventEpoch {
        SubscriptionEventEpoch { session_epoch }
    }
}

impl<Store> SubscriptionEventEpochAuthority<Store>
where
    Store: SubscriptionEventEpochStore,
{
    /// Takes ownership of the stream identity's single reservation store.
    pub const fn new(store: Store) -> Self {
        Self { store }
    }

    /// Reserves one epoch through the owning store for one publisher.
    pub fn reserve(
        &mut self,
    ) -> Result<SubscriptionEventEpoch, SubscriptionEventEpochError<Store::Error>> {
        self.store
            .reserve_next_epoch(SubscriptionEventEpochReservation {
                authority: PhantomData,
            })
    }
}

impl SubscriptionEventSequence {
    const fn new(session_epoch: SessionEpoch, next_sequence: Option<LaneSequence>) -> Self {
        Self {
            session_epoch,
            next_sequence,
        }
    }

    const fn acceptor(session_epoch: SessionEpoch) -> Self {
        Self::new(session_epoch, Some(LaneSequence::first()))
    }

    fn next_identifier(&mut self) -> Result<StreamEventIdentifier, SubscriptionPublishError> {
        let Some(next_sequence) = self.next_sequence else {
            return Err(SubscriptionPublishError::SequenceExhausted);
        };
        let identifier = StreamEventIdentifier::acceptor(self.session_epoch, next_sequence);
        self.next_sequence = next_sequence.value().checked_add(1).map(LaneSequence::new);
        Ok(identifier)
    }
}

impl<Contract, RequestPayload, ReplyPayload, EventPayload>
    SubscriptionEventPublisher<Contract, RequestPayload, ReplyPayload, EventPayload>
where
    Contract: WireContract,
{
    /// Creates the one publisher that owns `epoch`'s event sequence.
    pub const fn new(event_route: WireRoute, epoch: SubscriptionEventEpoch) -> Self {
        Self {
            event_route,
            sequence: SubscriptionEventSequence::acceptor(epoch.session_epoch),
            contract: PhantomData,
            request_payload: PhantomData,
            reply_payload: PhantomData,
            event_payload: PhantomData,
        }
    }

    pub fn event_route(&self) -> WireRoute {
        self.event_route
    }

    pub fn publish<Token>(
        &mut self,
        token: Token,
        event: EventPayload,
    ) -> Result<
        BoundStreamingFrame<Contract, RequestPayload, ReplyPayload, EventPayload>,
        SubscriptionPublishError,
    >
    where
        Token: SubscriptionToken,
    {
        Ok(BoundStreamingFrame::new(
            self.event_route,
            StreamingFrameBody::SubscriptionEvent {
                event_identifier: self.sequence.next_identifier()?,
                token: token.into_inner(),
                event,
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscription_token_issuer_issues_maximum_once_then_stays_exhausted() {
        let mut issuer = SubscriptionTokenIssuer::from_next_value(u64::MAX);

        assert_eq!(
            issuer.mint::<SubscriptionTokenInner>(),
            Ok(SubscriptionTokenInner::new(u64::MAX))
        );
        assert_eq!(
            issuer.mint::<SubscriptionTokenInner>(),
            Err(SubscriptionTokenError::TokenExhausted)
        );
        assert_eq!(
            issuer.mint::<SubscriptionTokenInner>(),
            Err(SubscriptionTokenError::TokenExhausted)
        );
    }

    #[test]
    fn subscription_registry_rejects_a_live_minted_token_collision() {
        let token = SubscriptionTokenInner::new(u64::MAX);
        let mut registry = SubscriptionRegistry {
            subscriptions: vec![Subscription { token, filter: () }],
            token_issuer: SubscriptionTokenIssuer::from_next_value(u64::MAX),
        };

        assert_eq!(
            registry.register(()),
            Err(SubscriptionTokenError::TokenCollision)
        );
        assert_eq!(registry.len(), 1);
        assert_eq!(
            registry.register(()),
            Err(SubscriptionTokenError::TokenExhausted)
        );
    }

    #[test]
    fn subscription_event_sequence_issues_maximum_once_then_stays_exhausted() {
        let mut sequence =
            SubscriptionEventSequence::new(SessionEpoch::new(7), Some(LaneSequence::new(u64::MAX)));

        let maximum = sequence.next_identifier().expect("issue maximum sequence");
        assert_eq!(maximum.session_epoch(), SessionEpoch::new(7));
        assert_eq!(maximum.sequence(), LaneSequence::new(u64::MAX));
        assert_eq!(sequence.next_sequence, None);
        assert_eq!(
            sequence.next_identifier(),
            Err(SubscriptionPublishError::SequenceExhausted)
        );
        assert_eq!(sequence.next_sequence, None);
    }
}
