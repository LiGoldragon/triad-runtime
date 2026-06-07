//! Actor-native runtime substrate for generated triad daemons.
//!
//! This module is the replacement direction for the old synchronous listener
//! and thread-worker shell. It intentionally starts with the load-bearing
//! primitive every actor daemon needs first: request admission that applies
//! backpressure without blocking an actor mailbox.

use std::fmt::{Debug, Formatter};
use std::sync::Arc;

use kameo::actor::{Actor, ActorRef, Spawn};
use kameo::error::Infallible;
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use thiserror::Error;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestConcurrencyLimit {
    count: usize,
}

#[derive(Clone)]
pub struct RequestPermitPool {
    limit: RequestConcurrencyLimit,
    semaphore: Arc<Semaphore>,
}

pub struct RequestPermit {
    limit: RequestConcurrencyLimit,
    _permit: OwnedSemaphorePermit,
}

#[derive(Debug, Error)]
pub enum RequestPermitError {
    #[error("request gate is closed")]
    GateClosed,
}

#[derive(Debug)]
pub struct RequestGate {
    pool: RequestPermitPool,
    accepted_request_count: u64,
}

pub struct AcquireRequestPermit {
    request_name: String,
    #[cfg(test)]
    admission_observer: Option<RequestAdmissionObserver>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestGateStatusRequest {
    observer_name: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, kameo::Reply)]
pub struct RequestGateStatus {
    limit: RequestConcurrencyLimit,
    available_permit_count: usize,
    accepted_request_count: u64,
}

#[cfg(test)]
struct RequestAdmissionObserver {
    sender: Option<tokio::sync::oneshot::Sender<RequestAdmission>>,
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
struct RequestAdmission {
    request_name: String,
    accepted_request_count: u64,
}

impl RequestConcurrencyLimit {
    pub const fn new(count: usize) -> Self {
        if count == 0 {
            Self::one()
        } else {
            Self { count }
        }
    }

    pub const fn one() -> Self {
        Self::new(1)
    }

    pub fn count(self) -> usize {
        self.count
    }
}

impl Debug for RequestPermitPool {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RequestPermitPool")
            .field("limit", &self.limit)
            .field("available_permit_count", &self.available_permit_count())
            .finish()
    }
}

impl RequestPermitPool {
    pub fn new(limit: RequestConcurrencyLimit) -> Self {
        Self {
            limit,
            semaphore: Arc::new(Semaphore::new(limit.count())),
        }
    }

    pub fn limit(&self) -> RequestConcurrencyLimit {
        self.limit
    }

    pub fn available_permit_count(&self) -> usize {
        self.semaphore.available_permits()
    }

    pub async fn acquire(&self) -> Result<RequestPermit, RequestPermitError> {
        self.semaphore
            .clone()
            .acquire_owned()
            .await
            .map(|permit| RequestPermit::new(self.limit, permit))
            .map_err(|_| RequestPermitError::GateClosed)
    }
}

impl Debug for RequestPermit {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RequestPermit")
            .field("limit", &self.limit)
            .finish_non_exhaustive()
    }
}

impl RequestPermit {
    fn new(limit: RequestConcurrencyLimit, permit: OwnedSemaphorePermit) -> Self {
        Self {
            limit,
            _permit: permit,
        }
    }

    pub fn limit(&self) -> RequestConcurrencyLimit {
        self.limit
    }
}

impl RequestGate {
    pub fn new(limit: RequestConcurrencyLimit) -> Self {
        Self {
            pool: RequestPermitPool::new(limit),
            accepted_request_count: 0,
        }
    }

    pub async fn start(self) -> ActorRef<Self> {
        let actor = Self::spawn(self);
        actor.wait_for_startup().await;
        actor
    }

    fn status(&self) -> RequestGateStatus {
        RequestGateStatus::new(
            self.pool.limit(),
            self.pool.available_permit_count(),
            self.accepted_request_count,
        )
    }

    fn accept_request(&mut self, message: &mut AcquireRequestPermit) {
        self.accepted_request_count = self.accepted_request_count.saturating_add(1);
        message.record_admission(self.accepted_request_count);
    }
}

impl Actor for RequestGate {
    type Args = Self;
    type Error = Infallible;

    async fn on_start(
        gate: Self::Args,
        _actor_reference: ActorRef<Self>,
    ) -> Result<Self, Self::Error> {
        Ok(gate)
    }
}

impl AcquireRequestPermit {
    pub fn new(request_name: impl Into<String>) -> Self {
        Self {
            request_name: request_name.into(),
            #[cfg(test)]
            admission_observer: None,
        }
    }

    pub fn request_name(&self) -> &str {
        &self.request_name
    }

    #[cfg(test)]
    fn with_admission_observer(
        request_name: impl Into<String>,
        admission_observer: RequestAdmissionObserver,
    ) -> Self {
        Self {
            request_name: request_name.into(),
            admission_observer: Some(admission_observer),
        }
    }

    fn record_admission(&mut self, accepted_request_count: u64) {
        #[cfg(test)]
        if let Some(observer) = self.admission_observer.take() {
            observer.record(RequestAdmission::new(
                self.request_name.clone(),
                accepted_request_count,
            ));
        }
        let _ = accepted_request_count;
    }
}

impl RequestGateStatusRequest {
    pub fn new(observer_name: impl Into<String>) -> Self {
        Self {
            observer_name: observer_name.into(),
        }
    }

    pub fn observer_name(&self) -> &str {
        &self.observer_name
    }
}

impl RequestGateStatus {
    pub const fn new(
        limit: RequestConcurrencyLimit,
        available_permit_count: usize,
        accepted_request_count: u64,
    ) -> Self {
        Self {
            limit,
            available_permit_count,
            accepted_request_count,
        }
    }

    pub fn limit(self) -> RequestConcurrencyLimit {
        self.limit
    }

    pub fn available_permit_count(self) -> usize {
        self.available_permit_count
    }

    pub fn accepted_request_count(self) -> u64 {
        self.accepted_request_count
    }
}

impl Message<AcquireRequestPermit> for RequestGate {
    type Reply = DelegatedReply<Result<RequestPermit, RequestPermitError>>;

    async fn handle(
        &mut self,
        mut message: AcquireRequestPermit,
        context: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.accept_request(&mut message);
        let pool = self.pool.clone();
        context.spawn(async move { pool.acquire().await })
    }
}

impl Message<RequestGateStatusRequest> for RequestGate {
    type Reply = RequestGateStatus;

    async fn handle(
        &mut self,
        _message: RequestGateStatusRequest,
        _context: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.status()
    }
}

#[cfg(test)]
impl RequestAdmissionObserver {
    fn new(sender: tokio::sync::oneshot::Sender<RequestAdmission>) -> Self {
        Self {
            sender: Some(sender),
        }
    }

    fn record(mut self, admission: RequestAdmission) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(admission);
        }
    }
}

#[cfg(test)]
impl RequestAdmission {
    fn new(request_name: impl Into<String>, accepted_request_count: u64) -> Self {
        Self {
            request_name: request_name.into(),
            accepted_request_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn request_concurrency_limit_clamps_zero_to_one() {
        let limit = RequestConcurrencyLimit::new(0);
        let gate = RequestGate::new(limit).start().await;

        let status = gate
            .ask(RequestGateStatusRequest::new("test"))
            .await
            .expect("status");

        assert_eq!(status.limit(), RequestConcurrencyLimit::one());
        assert_eq!(status.available_permit_count(), 1);

        gate.stop_gracefully().await.expect("stop gate");
        gate.wait_for_shutdown().await;
    }

    #[tokio::test]
    async fn request_gate_admits_only_the_configured_concurrency() {
        let gate = RequestGate::new(RequestConcurrencyLimit::new(2))
            .start()
            .await;

        let first = gate
            .ask(AcquireRequestPermit::new("first"))
            .await
            .expect("first permit");
        let second = gate
            .ask(AcquireRequestPermit::new("second"))
            .await
            .expect("second permit");

        let gate_for_third = gate.clone();
        let third =
            tokio::spawn(
                async move { gate_for_third.ask(AcquireRequestPermit::new("third")).await },
            );

        tokio::time::sleep(Duration::from_millis(25)).await;
        assert!(!third.is_finished());

        drop(first);

        let third = tokio::time::timeout(Duration::from_millis(200), third)
            .await
            .expect("third permit unblocks")
            .expect("third task joins")
            .expect("third permit succeeds");

        assert_eq!(third.limit(), RequestConcurrencyLimit::new(2));
        drop(second);
        drop(third);

        let status = gate
            .ask(RequestGateStatusRequest::new("test"))
            .await
            .expect("status");
        assert_eq!(status.available_permit_count(), 2);
        assert_eq!(status.accepted_request_count(), 3);

        gate.stop_gracefully().await.expect("stop gate");
        gate.wait_for_shutdown().await;
    }

    #[tokio::test]
    async fn request_gate_mailbox_stays_available_while_permit_waits() {
        let gate = RequestGate::new(RequestConcurrencyLimit::one())
            .start()
            .await;
        let held = gate
            .ask(AcquireRequestPermit::new("held"))
            .await
            .expect("held permit");

        let (admission_sender, admission_receiver) = tokio::sync::oneshot::channel();
        let waiting_request = AcquireRequestPermit::with_admission_observer(
            "waiting",
            RequestAdmissionObserver::new(admission_sender),
        );
        let gate_for_waiting_request = gate.clone();
        let waiting_permit =
            tokio::spawn(async move { gate_for_waiting_request.ask(waiting_request).await });

        let admission = tokio::time::timeout(Duration::from_millis(200), admission_receiver)
            .await
            .expect("waiting request reaches actor")
            .expect("admission observed");
        assert_eq!(admission.request_name, "waiting");
        assert_eq!(admission.accepted_request_count, 2);

        let status = tokio::time::timeout(
            Duration::from_millis(200),
            gate.ask(RequestGateStatusRequest::new("probe")),
        )
        .await
        .expect("status reply is not blocked behind waiting permit")
        .expect("status ask");
        assert_eq!(status.available_permit_count(), 0);
        assert_eq!(status.accepted_request_count(), 2);

        drop(held);

        let waiting = tokio::time::timeout(Duration::from_millis(200), waiting_permit)
            .await
            .expect("waiting permit unblocks")
            .expect("waiting task joins")
            .expect("waiting permit succeeds");
        assert_eq!(waiting.limit(), RequestConcurrencyLimit::one());
        drop(waiting);

        gate.stop_gracefully().await.expect("stop gate");
        gate.wait_for_shutdown().await;
    }
}
