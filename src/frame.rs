use std::io::{Read, Write};

use thiserror::Error;

const LENGTH_PREFIX_BYTE_COUNT: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MaximumFrameLength {
    bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameBody {
    bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LengthPrefixedCodec {
    maximum_body_length: MaximumFrameLength,
}

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("frame IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("frame body is too large: {found} bytes")]
    BodyTooLarge { found: usize },
}

impl MaximumFrameLength {
    pub const fn new(bytes: usize) -> Self {
        Self { bytes }
    }

    pub const fn maximum_for_u32_prefix() -> Self {
        Self::new(u32::MAX as usize)
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn accepts(&self, length: usize) -> bool {
        length <= self.bytes
    }
}

impl FrameBody {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            bytes: bytes.into(),
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

impl Default for LengthPrefixedCodec {
    fn default() -> Self {
        Self::new(MaximumFrameLength::maximum_for_u32_prefix())
    }
}

impl LengthPrefixedCodec {
    pub const fn new(maximum_body_length: MaximumFrameLength) -> Self {
        Self {
            maximum_body_length,
        }
    }

    pub fn maximum_body_length(&self) -> MaximumFrameLength {
        self.maximum_body_length
    }

    pub fn encode_body(&self, body: &FrameBody) -> Result<Vec<u8>, FrameError> {
        self.validate_length(body.len())?;
        let length = u32::try_from(body.len())
            .map_err(|_| FrameError::BodyTooLarge { found: body.len() })?;
        let mut frame = Vec::with_capacity(LENGTH_PREFIX_BYTE_COUNT + body.len());
        frame.extend_from_slice(&length.to_be_bytes());
        frame.extend_from_slice(body.bytes());
        Ok(frame)
    }

    pub fn write_body(&self, writer: &mut impl Write, body: &FrameBody) -> Result<(), FrameError> {
        writer.write_all(&self.encode_body(body)?)?;
        Ok(())
    }

    pub fn read_body(&self, reader: &mut impl Read) -> Result<FrameBody, FrameError> {
        let mut length_bytes = [0_u8; LENGTH_PREFIX_BYTE_COUNT];
        reader.read_exact(&mut length_bytes)?;
        let length = u32::from_be_bytes(length_bytes) as usize;
        self.validate_length(length)?;
        let mut body = vec![0_u8; length];
        reader.read_exact(&mut body)?;
        Ok(FrameBody::new(body))
    }

    fn validate_length(&self, length: usize) -> Result<(), FrameError> {
        if self.maximum_body_length.accepts(length) {
            Ok(())
        } else {
            Err(FrameError::BodyTooLarge { found: length })
        }
    }
}
