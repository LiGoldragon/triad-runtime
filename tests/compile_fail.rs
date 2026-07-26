#[test]
fn subscription_event_authority_types_cannot_be_cloned() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/ui/subscription_event_authority/*.rs");
}
