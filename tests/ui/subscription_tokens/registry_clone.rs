use signal_frame::SubscriptionTokenInner;
use triad_runtime::{SubscriptionRegistry, SubscriptionToken};

#[derive(Clone, Copy, Eq, PartialEq)]
struct Token(SubscriptionTokenInner);

impl SubscriptionToken for Token {
    fn from_inner(inner: SubscriptionTokenInner) -> Self {
        Self(inner)
    }

    fn into_inner(self) -> SubscriptionTokenInner {
        self.0
    }
}

fn main() {
    let registry = SubscriptionRegistry::<Token, ()>::new();
    let _issuer_snapshot = registry.token_issuer();
    let _duplicate = registry.clone();
}
