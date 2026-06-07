use crate::{NexusEffectCommand, NexusWork, SemaReadInput, SemaWriteInput};

use std::future::Future;

const DEFAULT_CONTINUATION_LIMIT_COUNT: u32 = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContinuationLimit {
    count: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContinuationBudget {
    limit: ContinuationLimit,
    completed_step_count: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContinuationExhausted {
    limit: ContinuationLimit,
    completed_step_count: u32,
    attempted_step_count: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NextStep<Reply, SemaWrite, SemaRead, Effect, Work> {
    Reply(Reply),
    SemaWrite(SemaWrite),
    SemaRead(SemaRead),
    RunEffect(Effect),
    Continue(Work),
}

pub trait RunnerEngines {
    type Reply;
    type SemaWrite: SemaWriteInput;
    type SemaRead: SemaReadInput;
    type Effect: NexusEffectCommand;
    type Work: NexusWork;

    fn decide_next_step(&mut self, work: Self::Work) -> RunnerNextStep<Self>
    where
        Self: Sized;

    fn apply_sema_write(
        &mut self,
        write: Self::SemaWrite,
    ) -> impl Future<Output = Self::Work> + Send + '_;

    fn observe_sema_read(
        &mut self,
        read: Self::SemaRead,
    ) -> impl Future<Output = Self::Work> + Send + '_;

    fn run_effect(&mut self, effect: Self::Effect) -> impl Future<Output = Self::Work> + Send + '_;

    fn budget_exhausted_reply(&self, exhausted: ContinuationExhausted) -> Self::Reply;
}

pub type RunnerNextStep<Engines> = NextStep<
    <Engines as RunnerEngines>::Reply,
    <Engines as RunnerEngines>::SemaWrite,
    <Engines as RunnerEngines>::SemaRead,
    <Engines as RunnerEngines>::Effect,
    <Engines as RunnerEngines>::Work,
>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Runner {
    continuation_limit: ContinuationLimit,
}

impl Default for ContinuationLimit {
    fn default() -> Self {
        Self::new(DEFAULT_CONTINUATION_LIMIT_COUNT)
    }
}

impl ContinuationLimit {
    pub const fn new(count: u32) -> Self {
        Self { count }
    }

    pub fn count(&self) -> u32 {
        self.count
    }

    pub fn budget(&self) -> ContinuationBudget {
        ContinuationBudget::new(*self)
    }
}

impl ContinuationBudget {
    pub const fn new(limit: ContinuationLimit) -> Self {
        Self {
            limit,
            completed_step_count: 0,
        }
    }

    pub fn limit(&self) -> ContinuationLimit {
        self.limit
    }

    pub fn completed_step_count(&self) -> u32 {
        self.completed_step_count
    }

    pub fn remaining_step_count(&self) -> u32 {
        self.limit.count().saturating_sub(self.completed_step_count)
    }

    pub fn spend_next_step(&mut self) -> Result<(), ContinuationExhausted> {
        let attempted_step_count = self.completed_step_count.saturating_add(1);
        if self.completed_step_count >= self.limit.count() {
            Err(ContinuationExhausted {
                limit: self.limit,
                completed_step_count: self.completed_step_count,
                attempted_step_count,
            })
        } else {
            self.completed_step_count = attempted_step_count;
            Ok(())
        }
    }
}

impl ContinuationExhausted {
    pub fn limit(&self) -> ContinuationLimit {
        self.limit
    }

    pub fn completed_step_count(&self) -> u32 {
        self.completed_step_count
    }

    pub fn attempted_step_count(&self) -> u32 {
        self.attempted_step_count
    }
}

impl Default for Runner {
    fn default() -> Self {
        Self::new(ContinuationLimit::default())
    }
}

impl Runner {
    pub const fn new(continuation_limit: ContinuationLimit) -> Self {
        Self { continuation_limit }
    }

    pub fn continuation_limit(&self) -> ContinuationLimit {
        self.continuation_limit
    }

    pub async fn drive<Engines>(
        &self,
        engines: &mut Engines,
        first_work: Engines::Work,
    ) -> Engines::Reply
    where
        Engines: RunnerEngines,
    {
        let mut work = first_work;
        let mut budget = self.continuation_limit.budget();

        loop {
            match engines.decide_next_step(work) {
                NextStep::Reply(reply) => return reply,
                NextStep::SemaWrite(write) => {
                    if let Err(exhausted) = budget.spend_next_step() {
                        return engines.budget_exhausted_reply(exhausted);
                    }
                    work = engines.apply_sema_write(write).await;
                }
                NextStep::SemaRead(read) => {
                    if let Err(exhausted) = budget.spend_next_step() {
                        return engines.budget_exhausted_reply(exhausted);
                    }
                    work = engines.observe_sema_read(read).await;
                }
                NextStep::RunEffect(effect) => {
                    if let Err(exhausted) = budget.spend_next_step() {
                        return engines.budget_exhausted_reply(exhausted);
                    }
                    work = engines.run_effect(effect).await;
                }
                NextStep::Continue(next_work) => {
                    if let Err(exhausted) = budget.spend_next_step() {
                        return engines.budget_exhausted_reply(exhausted);
                    }
                    work = next_work;
                }
            }
        }
    }
}
