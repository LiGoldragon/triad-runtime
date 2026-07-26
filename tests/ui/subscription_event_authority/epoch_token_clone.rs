use triad_runtime::SubscriptionEventEpoch;

fn duplicate(epoch: SubscriptionEventEpoch) {
    let _duplicate = epoch.clone();
}

fn main() {}
