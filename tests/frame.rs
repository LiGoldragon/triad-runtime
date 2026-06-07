use std::io::Cursor;

use tokio::io::AsyncWriteExt;
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

#[tokio::test]
async fn async_codec_round_trips_without_blocking_io_traits() {
    let codec = LengthPrefixedCodec::default();
    let body = FrameBody::new([0xA1, 0xA2, 0xA3]);
    let (mut client, mut server) = tokio::io::duplex(64);

    codec
        .write_body_async(&mut client, &body)
        .await
        .expect("async write");
    drop(client);

    let decoded = codec
        .read_body_async(&mut server)
        .await
        .expect("async read");

    assert_eq!(decoded, body);
}

#[tokio::test]
async fn async_codec_rejects_read_length_above_configured_limit() {
    let codec = LengthPrefixedCodec::new(MaximumFrameLength::new(2));
    let (mut client, mut server) = tokio::io::duplex(64);

    client
        .write_all(&[0, 0, 0, 3, 1, 2, 3])
        .await
        .expect("write oversized frame");
    drop(client);

    let error = codec
        .read_body_async(&mut server)
        .await
        .expect_err("oversized async frame should be rejected");

    assert!(matches!(error, FrameError::BodyTooLarge { found: 3 }));
}
