use crate::NextStep;

pub trait NexusWork {}

pub trait SemaWriteInput {}

pub trait SemaWriteOutput {}

pub trait SemaReadInput {}

pub trait SemaReadOutput {}

pub trait NexusEffectCommand {}

pub trait NexusEffectResult {}

pub trait NexusAction: Sized {
    type Reply;
    type SemaWrite: SemaWriteInput;
    type SemaRead: SemaReadInput;
    type Effect: NexusEffectCommand;
    type Work: NexusWork;

    fn into_next_step(self) -> NexusActionNextStep<Self>;
}

pub type NexusActionNextStep<Action> = NextStep<
    <Action as NexusAction>::Reply,
    <Action as NexusAction>::SemaWrite,
    <Action as NexusAction>::SemaRead,
    <Action as NexusAction>::Effect,
    <Action as NexusAction>::Work,
>;

impl SemaWriteInput for std::convert::Infallible {}

impl SemaWriteOutput for std::convert::Infallible {}

impl SemaReadInput for std::convert::Infallible {}

impl SemaReadOutput for std::convert::Infallible {}

impl NexusEffectCommand for std::convert::Infallible {}

impl NexusEffectResult for std::convert::Infallible {}
