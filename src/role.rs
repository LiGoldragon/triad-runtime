use crate::NextStep;

pub trait NexusWork {}

pub trait MemoryWriteInput {}

pub trait MemoryWriteOutput {}

pub trait MemoryReadInput {}

pub trait MemoryReadOutput {}

pub trait NexusEffectCommand {}

pub trait NexusEffectResult {}

pub trait NexusAction: Sized {
    type Reply;
    type MemoryWrite: MemoryWriteInput;
    type MemoryRead: MemoryReadInput;
    type Effect: NexusEffectCommand;
    type Work: NexusWork;

    fn into_next_step(self) -> NexusActionNextStep<Self>;
}

pub type NexusActionNextStep<Action> = NextStep<
    <Action as NexusAction>::Reply,
    <Action as NexusAction>::MemoryWrite,
    <Action as NexusAction>::MemoryRead,
    <Action as NexusAction>::Effect,
    <Action as NexusAction>::Work,
>;

impl MemoryWriteInput for std::convert::Infallible {}

impl MemoryWriteOutput for std::convert::Infallible {}

impl MemoryReadInput for std::convert::Infallible {}

impl MemoryReadOutput for std::convert::Infallible {}

impl NexusEffectCommand for std::convert::Infallible {}

impl NexusEffectResult for std::convert::Infallible {}
