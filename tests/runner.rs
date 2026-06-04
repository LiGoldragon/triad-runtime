use triad_runtime::{
    ContinuationBudget, ContinuationExhausted, ContinuationLimit, NextStep, Runner, RunnerEngines,
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
}

#[derive(Debug, Default)]
struct TestEngines {
    actions: Vec<&'static str>,
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
        Self { label }
    }
}

impl TestEngines {
    fn actions(&self) -> &[&'static str] {
        &self.actions
    }

    fn push_action(&mut self, action: &'static str) {
        self.actions.push(action);
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
            TestWork::AfterEffect | TestWork::Continue => NextStep::Continue(TestWork::ReplyNow),
            TestWork::Loop => NextStep::Continue(TestWork::Loop),
        }
    }

    fn apply_sema_write(&mut self, write: Self::SemaWrite) -> Self::Work {
        self.push_action(write.label);
        TestWork::AfterWrite
    }

    fn observe_sema_read(&mut self, read: Self::SemaRead) -> Self::Work {
        self.push_action(read.label);
        TestWork::AfterRead
    }

    fn run_effect(&mut self, effect: Self::Effect) -> Self::Work {
        self.push_action(effect.label);
        TestWork::AfterEffect
    }

    fn budget_exhausted_reply(&mut self, exhausted: ContinuationExhausted) -> Self::Reply {
        self.push_action("exhausted");
        TestReply::Exhausted(exhausted)
    }
}

#[test]
fn runner_returns_direct_reply_without_spending_budget() {
    let runner = Runner::new(ContinuationLimit::new(0));
    let mut engines = TestEngines::default();

    let reply = runner.drive(&mut engines, TestWork::ReplyNow);

    assert_eq!(reply, TestReply::Done);
    assert!(engines.actions().is_empty());
}

#[test]
fn runner_drives_all_non_reply_paths_until_reply() {
    let runner = Runner::new(ContinuationLimit::new(4));
    let mut engines = TestEngines::default();

    let reply = runner.drive(&mut engines, TestWork::Write);

    assert_eq!(reply, TestReply::Done);
    assert_eq!(engines.actions(), &["write", "read", "effect"]);
}

#[test]
fn runner_accepts_each_non_reply_entry_shape() {
    let runner = Runner::new(ContinuationLimit::new(3));
    let mut read_engines = TestEngines::default();
    let mut effect_engines = TestEngines::default();
    let mut continue_engines = TestEngines::default();

    let read_reply = runner.drive(&mut read_engines, TestWork::Read);
    let effect_reply = runner.drive(&mut effect_engines, TestWork::Effect);
    let continue_reply = runner.drive(&mut continue_engines, TestWork::Continue);

    assert_eq!(read_reply, TestReply::Done);
    assert_eq!(effect_reply, TestReply::Done);
    assert_eq!(continue_reply, TestReply::Done);
    assert_eq!(read_engines.actions(), &["read", "effect"]);
    assert_eq!(effect_engines.actions(), &["effect"]);
    assert!(continue_engines.actions().is_empty());
}

#[test]
fn runner_stops_before_dispatching_action_past_budget() {
    let runner = Runner::new(ContinuationLimit::new(2));
    let mut engines = TestEngines::default();

    let reply = runner.drive(&mut engines, TestWork::Write);

    let TestReply::Exhausted(exhausted) = reply else {
        panic!("expected budget exhaustion reply");
    };
    assert_eq!(exhausted.limit(), ContinuationLimit::new(2));
    assert_eq!(exhausted.completed_step_count(), 2);
    assert_eq!(exhausted.attempted_step_count(), 3);
    assert_eq!(engines.actions(), &["write", "read", "exhausted"]);
}

#[test]
fn runner_exhausts_continue_loop_without_plane_dispatch() {
    let runner = Runner::new(ContinuationLimit::new(2));
    let mut engines = TestEngines::default();

    let reply = runner.drive(&mut engines, TestWork::Loop);

    let TestReply::Exhausted(exhausted) = reply else {
        panic!("expected budget exhaustion reply");
    };
    assert_eq!(exhausted.limit(), ContinuationLimit::new(2));
    assert_eq!(exhausted.completed_step_count(), 2);
    assert_eq!(exhausted.attempted_step_count(), 3);
    assert_eq!(engines.actions(), &["exhausted"]);
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
