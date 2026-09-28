//! Streaming session: pulls media packets from a [`MediaSource`] and pushes them
//! over a [`Transport`] using the binary [`protocol`](crate::protocol).
//!
//! This is the seam the real product will reuse: capture + encoder will simply
//! be a `MediaSource` implementation, and the transport can be swapped without
//! changing this loop.

use crate::diagnostics::Diagnostics;
use crate::protocol::{Codec, MediaPacket, MediaType};
use crate::transport::{Transport, TransportError};
use async_trait::async_trait;

/// One encoded media unit ready to be framed and sent.
#[derive(Debug, Clone)]
pub struct EncodedUnit {
    pub media_type: MediaType,
    pub codec: Codec,
    pub flags: u8,
    pub timestamp_ms: u32,
    pub data: Vec<u8>,
}

/// A source of encoded media units.
///
/// Returning `Ok(None)` ends the stream. The real product will implement this
/// on top of capture + encoder; the POC implements it from a canned H.264 file.
#[async_trait]
pub trait MediaSource: Send {
    async fn next_unit(&mut self) -> Option<EncodedUnit>;
}

/// Drives a [`MediaSource`] into a [`Transport`], assigning sequence numbers and
/// updating diagnostics. Returns the number of packets sent.
pub async fn run_sender<S, T>(
    mut source: S,
    transport: &mut T,
    diag: &Diagnostics,
) -> Result<u64, TransportError>
where
    S: MediaSource,
    T: Transport,
{
    let mut sequence: u32 = 0;
    while let Some(unit) = source.next_unit().await {
        let packet = MediaPacket {
            media_type: unit.media_type,
            codec: unit.codec,
            flags: unit.flags,
            sequence,
            timestamp_ms: unit.timestamp_ms,
            payload: &unit.data,
        };
        let bytes = packet.encode();
        let len = bytes.len();
        match transport.send(bytes).await {
            Ok(()) => diag.record_sent(len, sequence, unit.timestamp_ms),
            Err(e) => {
                diag.record_error();
                return Err(e);
            }
        }
        sequence = sequence.wrapping_add(1);
    }
    Ok(sequence as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::flags;
    use std::sync::{Arc, Mutex};

    struct VecSource {
        units: std::collections::VecDeque<EncodedUnit>,
    }

    #[async_trait]
    impl MediaSource for VecSource {
        async fn next_unit(&mut self) -> Option<EncodedUnit> {
            self.units.pop_front()
        }
    }

    /// Transport that records every message it "sends".
    #[derive(Clone, Default)]
    struct RecordingTransport {
        sent: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    #[async_trait]
    impl Transport for RecordingTransport {
        async fn send(&mut self, bytes: Vec<u8>) -> Result<(), TransportError> {
            self.sent.lock().unwrap().push(bytes);
            Ok(())
        }
        async fn recv(&mut self) -> Result<Option<Vec<u8>>, TransportError> {
            Ok(None)
        }
        async fn close(&mut self) -> Result<(), TransportError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn assigns_incrementing_sequences() {
        let units = (0..3)
            .map(|i| EncodedUnit {
                media_type: MediaType::Video,
                codec: Codec::H264,
                flags: if i == 0 { flags::KEYFRAME } else { 0 },
                timestamp_ms: i * 33,
                data: vec![i as u8; 4],
            })
            .collect::<std::collections::VecDeque<_>>();
        let source = VecSource { units };
        let mut transport = RecordingTransport::default();
        let diag = Diagnostics::new();

        let count = run_sender(source, &mut transport, &diag).await.unwrap();
        assert_eq!(count, 3);

        let sent = transport.sent.lock().unwrap();
        assert_eq!(sent.len(), 3);
        for (i, frame) in sent.iter().enumerate() {
            let pkt = MediaPacket::decode(frame).unwrap();
            assert_eq!(pkt.sequence, i as u32);
        }
        // First unit was a keyframe.
        assert!(MediaPacket::decode(&sent[0]).unwrap().is_keyframe());

        let snap = diag.snapshot();
        assert_eq!(snap.packets_sent, 3);
        assert_eq!(snap.last_sequence, 2);
    }
}
