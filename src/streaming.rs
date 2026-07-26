use std::marker::PhantomData;

use signal_frame::{
    BoundStreamingFrame, LaneSequence, SessionEpoch, StreamEventIdentifier, StreamingFrameBody,
    SubscriptionTokenInner, WireContract, WireRoute,
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

#[derive(Debug, Eq, PartialEq)]
pub struct SubscriptionEventEpochAuthority {
    next_epoch: Option<SessionEpoch>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct SubscriptionEventEpoch {
    session_epoch: SessionEpoch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SubscriptionEventEpochError {
    #[error("subscription event epoch space is exhausted")]
    EpochExhausted,
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

impl SubscriptionEventEpochAuthority {
    /// Begins reserving epochs at `first_epoch`.
    pub const fn new(first_epoch: SessionEpoch) -> Self {
        Self {
            next_epoch: Some(first_epoch),
        }
    }

    /// Restores the persisted next epoch for this one application-owned authority.
    ///
    /// The application must persist [`Self::next_epoch`] and ensure that it has
    /// only one live authority for a stream identity. Persist the returned next
    /// epoch before using a newly reserved publisher after a crash boundary.
    pub const fn restore(next_epoch: Option<SessionEpoch>) -> Self {
        Self { next_epoch }
    }

    /// Returns the state that the application persists to restore this authority.
    pub const fn next_epoch(&self) -> Option<SessionEpoch> {
        self.next_epoch
    }

    /// Reserves one epoch for a single subscription-event publisher.
    pub fn reserve(&mut self) -> Result<SubscriptionEventEpoch, SubscriptionEventEpochError> {
        let Some(session_epoch) = self.next_epoch else {
            return Err(SubscriptionEventEpochError::EpochExhausted);
        };

        self.next_epoch = session_epoch.value().checked_add(1).map(SessionEpoch::new);
        Ok(SubscriptionEventEpoch { session_epoch })
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
