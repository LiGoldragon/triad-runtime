use signal_frame::{
    ContractBinding, ContractId, RootCode, VariantCode, WireContract, WireRevision, WireRoute,
};
use std::num::{NonZeroU16, NonZeroU32};
use triad_runtime::{SubscriptionEventEpochAuthority, SubscriptionEventPublisher};

struct Contract;

impl WireContract for Contract {
    const BINDING: ContractBinding = ContractBinding::new(
        ContractId::new(NonZeroU32::new(1).unwrap()),
        WireRevision::new(NonZeroU16::new(1).unwrap()),
    );
}

fn main() {
    let mut authority = SubscriptionEventEpochAuthority::new(signal_frame::SessionEpoch::new(3));
    let publisher = SubscriptionEventPublisher::<Contract, (), (), ()>::new(
        WireRoute::new(RootCode::new(1), VariantCode::new(1)),
        authority.reserve().unwrap(),
    );
    let _duplicate = publisher.clone();
}
