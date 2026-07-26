use signal_frame::SessionEpoch;
use triad_runtime::{SubscriptionEventEpochAuthority, SubscriptionEventEpochStore};

fn snapshot<Store>(authority: &SubscriptionEventEpochAuthority<Store>)
where
    Store: SubscriptionEventEpochStore,
{
    let _next = authority.next_epoch();
}

fn restore<Store>(next_epoch: Option<SessionEpoch>)
where
    Store: SubscriptionEventEpochStore,
{
    let _authority = SubscriptionEventEpochAuthority::<Store>::restore(next_epoch);
}

fn main() {
    let _authority = SubscriptionEventEpochAuthority::new(SessionEpoch::new(3));
}
