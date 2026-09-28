//! Binary media protocol for the POC.
//!
//! Every media packet is a single binary WebSocket frame. The relay forwards
//! these frames verbatim; it never parses the payload. Only the desktop client
//! (producer) and the Activity (consumer) understand the header.
//!
//! Wire format (little-endian), fixed 16-byte header followed by payload:
//!
//! ```text
//! offset  field         type   notes
//! 0       magic         u8     always 0xD5
//! 1       version       u8     protocol version (currently 1)
//! 2       media_type    u8     0 = video, 1 = audio
//! 3       flags         u8     bit0 = KEYFRAME, bit1 = CONFIG
//! 4       codec         u8     0 = H264, 1 = OPUS, 2 = AV1 (future)
//! 5       reserved      u8     always 0
//! 6       reserved2     u16    always 0 (alignment / future use)
//! 8       sequence      u32    monotonically increasing per stream
//! 12      timestamp_ms  u32    presentation timestamp, relative, ms
//! 16      payload...    bytes  encoded media bytes (implicit length)
//! ```
//!
//! Payload length is implicit: the WebSocket frame already delimits the packet,
//! so `payload.len() == frame.len() - HEADER_LEN`.

use std::convert::TryFrom;

/// Sentinel byte at the start of every media packet.
pub const MAGIC: u8 = 0xD5;

/// Current protocol version.
pub const VERSION: u8 = 1;

/// Size of the fixed binary header in bytes.
pub const HEADER_LEN: usize = 16;

/// Bit flags carried in the header `flags` byte.
pub mod flags {
    /// Packet carries a keyframe (IDR).
    pub const KEYFRAME: u8 = 0b0000_0001;
    /// Packet carries codec configuration (e.g. H.264 SPS/PPS / avcC).
    pub const CONFIG: u8 = 0b0000_0010;
}

/// Kind of media in a packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MediaType {
    Video = 0,
    Audio = 1,
}

impl TryFrom<u8> for MediaType {
    type Error = ProtocolError;
    fn try_from(v: u8) -> Result<Self, Self::Error> {
        match v {
            0 => Ok(MediaType::Video),
            1 => Ok(MediaType::Audio),
            other => Err(ProtocolError::InvalidMediaType(other)),
        }
    }
}

/// Codec identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Codec {
    H264 = 0,
    Opus = 1,
    Av1 = 2,
}

impl TryFrom<u8> for Codec {
    type Error = ProtocolError;
    fn try_from(v: u8) -> Result<Self, Self::Error> {
        match v {
            0 => Ok(Codec::H264),
            1 => Ok(Codec::Opus),
            2 => Ok(Codec::Av1),
            other => Err(ProtocolError::InvalidCodec(other)),
        }
    }
}

/// Errors produced when decoding a media packet.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("buffer too short: got {got} bytes, need at least {need}")]
    TooShort { got: usize, need: usize },
    #[error("bad magic byte: got {0:#x}, expected {MAGIC:#x}")]
    BadMagic(u8),
    #[error("unsupported version: {0}")]
    UnsupportedVersion(u8),
    #[error("invalid media_type: {0}")]
    InvalidMediaType(u8),
    #[error("invalid codec: {0}")]
    InvalidCodec(u8),
}

/// A decoded view over a media packet: parsed header plus a borrow of the payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaPacket<'a> {
    pub media_type: MediaType,
    pub codec: Codec,
    pub flags: u8,
    pub sequence: u32,
    pub timestamp_ms: u32,
    pub payload: &'a [u8],
}

impl<'a> MediaPacket<'a> {
    /// Returns true if the KEYFRAME flag is set.
    pub fn is_keyframe(&self) -> bool {
        self.flags & flags::KEYFRAME != 0
    }

    /// Returns true if the CONFIG flag is set.
    pub fn is_config(&self) -> bool {
        self.flags & flags::CONFIG != 0
    }

    /// Serialize header + payload into a new byte buffer suitable for a binary
    /// WebSocket frame.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(HEADER_LEN + self.payload.len());
        buf.push(MAGIC);
        buf.push(VERSION);
        buf.push(self.media_type as u8);
        buf.push(self.flags);
        buf.push(self.codec as u8);
        buf.push(0); // reserved
        buf.extend_from_slice(&0u16.to_le_bytes()); // reserved2
        buf.extend_from_slice(&self.sequence.to_le_bytes());
        buf.extend_from_slice(&self.timestamp_ms.to_le_bytes());
        buf.extend_from_slice(self.payload);
        buf
    }

    /// Parse a media packet from a byte buffer. The returned packet borrows the
    /// payload region of `buf`.
    pub fn decode(buf: &'a [u8]) -> Result<MediaPacket<'a>, ProtocolError> {
        if buf.len() < HEADER_LEN {
            return Err(ProtocolError::TooShort {
                got: buf.len(),
                need: HEADER_LEN,
            });
        }
        if buf[0] != MAGIC {
            return Err(ProtocolError::BadMagic(buf[0]));
        }
        if buf[1] != VERSION {
            return Err(ProtocolError::UnsupportedVersion(buf[1]));
        }
        let media_type = MediaType::try_from(buf[2])?;
        let flags = buf[3];
        let codec = Codec::try_from(buf[4])?;
        // buf[5], buf[6..8] are reserved and ignored.
        let sequence = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
        let timestamp_ms = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
        Ok(MediaPacket {
            media_type,
            codec,
            flags,
            sequence,
            timestamp_ms,
            payload: &buf[HEADER_LEN..],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(seq: u32, ts: u32, flags: u8) -> Vec<u8> {
        MediaPacket {
            media_type: MediaType::Video,
            codec: Codec::H264,
            flags,
            sequence: seq,
            timestamp_ms: ts,
            payload: &[1, 2, 3, 4, 5],
        }
        .encode()
    }

    #[test]
    fn roundtrip_preserves_all_fields() {
        let bytes = sample(42, 1000, flags::KEYFRAME | flags::CONFIG);
        let pkt = MediaPacket::decode(&bytes).unwrap();
        assert_eq!(pkt.media_type, MediaType::Video);
        assert_eq!(pkt.codec, Codec::H264);
        assert_eq!(pkt.sequence, 42);
        assert_eq!(pkt.timestamp_ms, 1000);
        assert!(pkt.is_keyframe());
        assert!(pkt.is_config());
        assert_eq!(pkt.payload, &[1, 2, 3, 4, 5]);
    }

    #[test]
    fn header_is_exactly_16_bytes() {
        let bytes = MediaPacket {
            media_type: MediaType::Video,
            codec: Codec::H264,
            flags: 0,
            sequence: 0,
            timestamp_ms: 0,
            payload: &[],
        }
        .encode();
        assert_eq!(bytes.len(), HEADER_LEN);
    }

    #[test]
    fn flags_default_off() {
        let bytes = sample(1, 1, 0);
        let pkt = MediaPacket::decode(&bytes).unwrap();
        assert!(!pkt.is_keyframe());
        assert!(!pkt.is_config());
    }

    #[test]
    fn sequence_and_timestamp_are_little_endian() {
        let bytes = sample(0x01020304, 0x0A0B0C0D, 0);
        // sequence occupies bytes 8..12
        assert_eq!(&bytes[8..12], &[0x04, 0x03, 0x02, 0x01]);
        // timestamp occupies bytes 12..16
        assert_eq!(&bytes[12..16], &[0x0D, 0x0C, 0x0B, 0x0A]);
    }

    #[test]
    fn rejects_short_buffer() {
        let err = MediaPacket::decode(&[0xD5, 1, 0]).unwrap_err();
        assert_eq!(err, ProtocolError::TooShort { got: 3, need: 16 });
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = sample(1, 1, 0);
        bytes[0] = 0x00;
        assert_eq!(MediaPacket::decode(&bytes).unwrap_err(), ProtocolError::BadMagic(0));
    }

    #[test]
    fn rejects_unknown_version() {
        let mut bytes = sample(1, 1, 0);
        bytes[1] = 99;
        assert_eq!(
            MediaPacket::decode(&bytes).unwrap_err(),
            ProtocolError::UnsupportedVersion(99)
        );
    }

    #[test]
    fn rejects_invalid_media_type() {
        let mut bytes = sample(1, 1, 0);
        bytes[2] = 5;
        assert_eq!(
            MediaPacket::decode(&bytes).unwrap_err(),
            ProtocolError::InvalidMediaType(5)
        );
    }

    #[test]
    fn empty_payload_roundtrips() {
        let bytes = MediaPacket {
            media_type: MediaType::Video,
            codec: Codec::H264,
            flags: flags::CONFIG,
            sequence: 7,
            timestamp_ms: 33,
            payload: &[],
        }
        .encode();
        let pkt = MediaPacket::decode(&bytes).unwrap();
        assert_eq!(pkt.payload.len(), 0);
        assert!(pkt.is_config());
    }
}
