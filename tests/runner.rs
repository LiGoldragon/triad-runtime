use std::cell::RefCell;

use triad_runtime::{
    ContinuationBudget, ContinuationExhausted, ContinuationLimit, NextStep, NexusEffectCommand,
    NexusWork, Runner, RunnerEngines, SemaReadInput, SemaWriteInput,
};

#[derive(Clone, Debug, Eq, PartialEq)]
enum TestWork {
    ReplyNow,
    Write,
    AfterWrite,
    Read,
    AfterRead,
    Effect,
    AfterEffect,
    Continue,
    Loop,
    DelayedEffect,
    AfterDelayedEffect,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TestReply {
    Done,
    Exhausted(ContinuationExhausted),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TestSemaWrite {
    label: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TestSemaRead {
    label: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TestEffect {
    label: &'static str,
    delayed: bool,
}

#[derive(Debug, Default)]
struct TestEngines {
    actions: RefCell<Vec<&'static str>>,
}

impl TestSemaWrite {
    fn new(label: &'static str) -> Self {
        Self { label }
    }
}

impl TestSemaRead {
    fn new(label: &'static str) -> Self {
        Self { label }
    }
}

impl TestEffect {
    fn new(label: &'static str) -> Self {
        Self {
            label,
            delayed: false,
        }
    }

    fn delayed(label: &'static str) -> Self {
        Self {
            label,
            delayed: true,
        }
    }
}

impl NexusWork for TestWork {}

impl SemaWriteInput for TestSemaWrite {}

impl SemaReadInput for TestSemaRead {}

impl NexusEffectCommand for TestEffect {}

impl TestEngines {
    fn cloned_actions(&self) -> Vec<&'static str> {
        self.actions.borrow().clone()
    }

    fn push_action(&mut self, action: &'static str) {
        self.actions.borrow_mut().push(action);
    }

    fn push_shared_action(&self, action: &'static str) {
        self.actions.borrow_mut().push(action);
    }
}

impl RunnerEngines for TestEngines {
    type Reply = TestReply;
    type SemaRead = TestSemaRead;
    type SemaWrite = TestSemaWrite;
    type Effect = TestEffect;
    type Work = TestWork;

    fn decide_next_step(
        &mut self,
        work: Self::Work,
    ) -> NextStep<Self::Reply, Self::SemaWrite, Self::SemaRead, Self::Effect, Self::Work> {
        match work {
            TestWork::ReplyNow => NextStep::Reply(TestReply::Done),
            TestWork::Write => NextStep::SemaWrite(TestSemaWrite::new("write")),
            TestWork::AfterWrite | TestWork::Read => NextStep::SemaRead(TestSemaRead::new("read")),
            TestWork::AfterRead | TestWork::Effect => {
                NextStep::RunEffect(TestEffect::new("effect"))
            }
            TestWork::DelayedEffect => NextStep::RunEffect(TestEffect::delayed("delayed-effect")),
            TestWork::AfterEffect | TestWork::Continue => NextStep::Continue(TestWork::ReplyNow),
            TestWork::AfterDelayedEffect => NextStep::Reply(TestReply::Done),
            TestWork::Loop => NextStep::Continue(TestWork::Loop),
        }
    }

    async fn apply_sema_write(&mut self, write: Self::SemaWrite) -> Self::Work {
        self.push_action(write.label);
        TestWork::AfterWrite
    }

    async fn observe_sema_read(&mut self, read: Self::SemaRead) -> Self::Work {
        self.push_shared_action(read.label);
        TestWork::AfterRead
    }

    async fn run_effect(&mut self, effect: Self::Effect) -> Self::Work {
        if effect.delayed {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            self.push_action(effect.label);
            return TestWork::AfterDelayedEffect;
        }
        self.push_action(effect.label);
        TestWork::AfterEffect
    }

    fn budget_exhausted_reply(&self, exhausted: ContinuationExhausted) -> Self::Reply {
        self.push_shared_action("exhausted");
        TestReply::Exhausted(exhausted)
    }
}

#[tokio::test]
async fn runner_returns_direct_reply_without_spending_budget() {
    let runner = Runner::new(ContinuationLimit::new(0));
    let mut engines = TestEngines::default();

    let reply = runner.drive(&mut engines, TestWork::ReplyNow).await;

    assert_eq!(reply, TestReply::Done);
    assert!(engines.cloned_actions().is_empty());
}

#[tokio::test]
async fn runner_drives_all_non_reply_paths_until_reply() {
    let runner = Runner::new(ContinuationLimit::new(4));
    let mut engines = TestEngines::default();

    let reply = runner.drive(&mut engines, TestWork::Write).await;

    assert_eq!(reply, TestReply::Done);
    assert_eq!(engines.cloned_actions(), ["write", "read", "effect"]);
}

#[tokio::test]
async fn runner_accepts_each_non_reply_entry_shape() {
    let runner = Runner::new(ContinuationLimit::new(3));
    let mut read_engines = TestEngines::default();
    let mut effect_engines = TestEngines::default();
    let mut continue_engines = TestEngines::default();

    let read_reply = runner.drive(&mut read_engines, TestWork::Read).await;
    let effect_reply = runner.drive(&mut effect_engines, TestWork::Effect).await;
    let continue_reply = runner
        .drive(&mut continue_engines, TestWork::Continue)
        .await;

    assert_eq!(read_reply, TestReply::Done);
    assert_eq!(effect_reply, TestReply::Done);
    assert_eq!(continue_reply, TestReply::Done);
    assert_eq!(read_engines.cloned_actions(), ["read", "effect"]);
    assert_eq!(effect_engines.cloned_actions(), ["effect"]);
    assert!(continue_engines.cloned_actions().is_empty());
}

#[tokio::test]
async fn runner_stops_before_dispatching_action_past_budget() {
    let runner = Runner::new(ContinuationLimit::new(2));
    let mut engines = TestEngines::default();

    let reply = runner.drive(&mut engines, TestWork::Write).await;

    let TestReply::Exhausted(exhausted) = reply else {
        panic!("expected budget exhaustion reply");
    };
    assert_eq!(exhausted.limit(), ContinuationLimit::new(2));
    assert_eq!(exhausted.completed_step_count(), 2);
    assert_eq!(exhausted.attempted_step_count(), 3);
    assert_eq!(engines.cloned_actions(), ["write", "read", "exhausted"]);
}

#[tokio::test]
async fn runner_exhausts_continue_loop_without_plane_dispatch() {
    let runner = Runner::new(ContinuationLimit::new(2));
    let mut engines = TestEngines::default();

    let reply = runner.drive(&mut engines, TestWork::Loop).await;

    let TestReply::Exhausted(exhausted) = reply else {
        panic!("expected budget exhaustion reply");
    };
    assert_eq!(exhausted.limit(), ContinuationLimit::new(2));
    assert_eq!(exhausted.completed_step_count(), 2);
    assert_eq!(exhausted.attempted_step_count(), 3);
    assert_eq!(engines.cloned_actions(), ["exhausted"]);
}

#[tokio::test]
async fn runner_awaits_effect_continuation_before_replying() {
    let runner = Runner::new(ContinuationLimit::new(1));
    let mut engines = TestEngines::default();

    let reply = runner.drive(&mut engines, TestWork::DelayedEffect).await;

    assert_eq!(reply, TestReply::Done);
    assert_eq!(engines.cloned_actions(), ["delayed-effect"]);
}

#[test]
fn continuation_budget_reports_remaining_and_exhausted_counts() {
    let mut budget = ContinuationBudget::new(ContinuationLimit::new(1));

    assert_eq!(budget.remaining_step_count(), 1);
    assert!(budget.spend_next_step().is_ok());
    assert_eq!(budget.completed_step_count(), 1);
    assert_eq!(budget.remaining_step_count(), 0);

    let exhausted = budget
        .spend_next_step()
        .expect_err("second step exhausts one-step budget");

    assert_eq!(exhausted.limit(), ContinuationLimit::new(1));
    assert_eq!(exhausted.completed_step_count(), 1);
    assert_eq!(exhausted.attempted_step_count(), 2);
}
