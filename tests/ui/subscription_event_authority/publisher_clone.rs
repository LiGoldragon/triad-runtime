use signal_frame::{ContractBinding, ContractId, WireContract, WireRevision};
use std::num::{NonZeroU16, NonZeroU32};
use triad_runtime::SubscriptionEventPublisher;

struct Contract;

impl WireContract for Contract {
    const BINDING: ContractBinding = ContractBinding::new(
        ContractId::new(NonZeroU32::new(1).unwrap()),
        WireRevision::new(NonZeroU16::new(1).unwrap()),
    );
}

fn duplicate(publisher: SubscriptionEventPublisher<Contract, (), (), ()>) {
    let _duplicate = publisher.clone();
}

fn main() {}
