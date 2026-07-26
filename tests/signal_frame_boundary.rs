#[test]
fn signal_frame_dependency_is_one_exact_zero_point_four_producer() {
    let manifest = include_str!("../Cargo.toml");
    let expected = "signal-frame = { git = \"https://github.com/LiGoldragon/signal-frame.git\", rev = \"0786fbe8caf27552afcdd5deb85bc82ec6088337\" }";

    assert_eq!(manifest.matches("signal-frame =").count(), 1);
    assert!(manifest.contains(expected));
    assert!(!manifest.contains("signal-frame.git\", branch"));
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
