use triad_runtime::SubscriptionTokenIssuer;

fn main() {
    let issuer = SubscriptionTokenIssuer::new();
    let _snapshot = issuer.next_value();
    let _duplicate = issuer.clone();
}
