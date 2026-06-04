use std::io::Cursor;

use triad_runtime::{FrameBody, FrameError, LengthPrefixedCodec, MaximumFrameLength};

#[test]
fn codec_writes_four_byte_big_endian_length_prefix() {
    let codec = LengthPrefixedCodec::default();
    let body = FrameBody::new([0x51, 0x52, 0x53]);

    let frame = codec.encode_body(&body).expect("encoded frame");

    assert_eq!(&frame[..4], &[0, 0, 0, 3]);
    assert_eq!(&frame[4..], body.bytes());
}

#[test]
fn codec_reads_body_from_length_prefixed_bytes() {
    let codec = LengthPrefixedCodec::default();
    let mut frame = Cursor::new([0, 0, 0, 2, 0x8A, 0x8B]);

    let body = codec.read_body(&mut frame).expect("decoded body");

    assert_eq!(body.bytes(), &[0x8A, 0x8B]);
}

#[test]
fn codec_rejects_body_above_configured_limit() {
    let codec = LengthPrefixedCodec::new(MaximumFrameLength::new(2));
    let body = FrameBody::new([1, 2, 3]);

    let error = codec
        .encode_body(&body)
        .expect_err("oversized body should be rejected");

    assert!(matches!(error, FrameError::BodyTooLarge { found: 3 }));
}

#[test]
fn codec_rejects_read_length_above_configured_limit() {
    let codec = LengthPrefixedCodec::new(MaximumFrameLength::new(2));
    let mut frame = Cursor::new([0, 0, 0, 3, 1, 2, 3]);

    let error = codec
        .read_body(&mut frame)
        .expect_err("oversized frame should be rejected");

    assert!(matches!(error, FrameError::BodyTooLarge { found: 3 }));
}
