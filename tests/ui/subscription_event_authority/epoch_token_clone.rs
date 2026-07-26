use signal_frame::SessionEpoch;
use triad_runtime::SubscriptionEventEpochAuthority;

fn main() {
    let mut authority = SubscriptionEventEpochAuthority::new(SessionEpoch::new(3));
    let reservation = authority.reserve().unwrap();
    let _duplicate = reservation.clone();
}
