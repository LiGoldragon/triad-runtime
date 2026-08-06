#[test]
fn signal_frame_dependency_is_the_sole_exact_producer() {
    let manifest = include_str!("../Cargo.toml");
    let lockfile = include_str!("../Cargo.lock");
    let expected = "signal-frame = { git = \"https://github.com/LiGoldragon/signal-frame.git\", rev = \"8aa0bcaeb29fe9e461a11706a469638d2fd109ac\", default-features = false }";

    assert_eq!(manifest.matches("signal-frame =").count(), 1);
    assert!(manifest.contains(expected));
    assert!(lockfile.contains(
        "signal-frame.git?rev=8aa0bcaeb29fe9e461a11706a469638d2fd109ac#8aa0bcaeb29fe9e461a11706a469638d2fd109ac"
    ));
    assert!(lockfile.contains(
        "dotos.git?rev=80c7b17f7ad3cf547d2624c6a243e5de5f85c9f3#80c7b17f7ad3cf547d2624c6a243e5de5f85c9f3"
    ));
    assert!(!manifest.contains("branch ="));
    assert!(!lockfile.contains("?branch="));
    assert!(!manifest.contains("signal-frame.git\", path"));
}

#[test]
fn publisher_source_has_no_legacy_unbound_frame_path() {
    let source = include_str!("../src/streaming.rs");

    assert!(source.contains("BoundStreamingFrame"));
    assert!(source.contains("WireContract"));
    assert!(source.contains("WireRoute"));
    assert!(!source.contains("ShortHeader"));
    assert!(!source.contains(") -> StreamingFrame<"));
    assert!(!source.contains("StreamEventIdentifier::new"));
    assert!(!source.contains("ExchangeLane"));
}
