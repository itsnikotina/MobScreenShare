//! Windows test client for the transport POC.
//!
//! It does **no** real screen capture. It reads a canned H.264 Annex-B file,
//! splits it into access units (one decodable picture each), and streams them to
//! the relay using the same `stream-core` transport + protocol the real product
//! will use.
//!
//! Usage:
//! ```text
//! test-client [RELAY_WS_URL] [PATH_TO_H264] [--loop]
//! ```
//! Defaults: `ws://127.0.0.1:9000/publish/test-room/test-stream`, `assets/test.h264`.

use std::time::Duration;

use async_trait::async_trait;
use stream_core::diagnostics::Diagnostics;
use stream_core::protocol::{flags, Codec, MediaType};
use stream_core::streaming::{run_sender, EncodedUnit, MediaSource};
use stream_core::transport::{Transport, WebSocketTransport};

/// Splits a raw H.264 Annex-B buffer into access units and yields them at ~fps.
struct H264FileSource {
    access_units: Vec<AccessUnit>,
    index: usize,
    looping: bool,
    frame_interval: Duration,
    timestamp_ms: u32,
}

struct AccessUnit {
    data: Vec<u8>,
    keyframe: bool,
    config: bool,
}

impl H264FileSource {
    fn new(bytes: &[u8], fps: u32, looping: bool) -> Self {
        Self {
            access_units: split_access_units(bytes),
            index: 0,
            looping,
            frame_interval: Duration::from_millis((1000 / fps.max(1)) as u64),
            timestamp_ms: 0,
        }
    }
}

#[async_trait]
impl MediaSource for H264FileSource {
    async fn next_unit(&mut self) -> Option<EncodedUnit> {
        if self.index >= self.access_units.len() {
            if self.looping && !self.access_units.is_empty() {
                self.index = 0;
            } else {
                return None;
            }
        }
        // Pace output to roughly match the frame rate.
        tokio::time::sleep(self.frame_interval).await;

        let au = &self.access_units[self.index];
        self.index += 1;
        let mut f = 0u8;
        if au.keyframe {
            f |= flags::KEYFRAME;
        }
        if au.config {
            f |= flags::CONFIG;
        }
        let ts = self.timestamp_ms;
        self.timestamp_ms = self
            .timestamp_ms
            .wrapping_add(self.frame_interval.as_millis() as u32);
        Some(EncodedUnit {
            media_type: MediaType::Video,
            codec: Codec::H264,
            flags: f,
            timestamp_ms: ts,
            data: au.data.clone(),
        })
    }
}

/// NAL unit type (lower 5 bits of the byte after the start code).
fn nal_type(nal_first_byte: u8) -> u8 {
    nal_first_byte & 0x1F
}

/// Split an Annex-B stream into access units. Each access unit keeps its start
/// codes so a WebCodecs decoder in Annex-B mode can consume it directly. An
/// access unit is flushed when a new VCL NAL (slice) begins after the current
/// unit already contains one.
fn split_access_units(bytes: &[u8]) -> Vec<AccessUnit> {
    let nals = split_nals(bytes);
    let mut units: Vec<AccessUnit> = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    let mut cur_has_vcl = false;
    let mut cur_keyframe = false;
    let mut cur_config = false;

    for (start_code_len, nal) in nals {
        let t = nal_type(nal[start_code_len]);
        let is_vcl = matches!(t, 1..=5);
        if is_vcl && cur_has_vcl {
            units.push(AccessUnit {
                data: std::mem::take(&mut cur),
                keyframe: cur_keyframe,
                config: cur_config,
            });
            cur_has_vcl = false;
            cur_keyframe = false;
            cur_config = false;
        }
        if t == 7 {
            cur_config = true; // SPS
        }
        if t == 5 {
            cur_keyframe = true; // IDR slice
        }
        if is_vcl {
            cur_has_vcl = true;
        }
        cur.extend_from_slice(nal);
    }
    if !cur.is_empty() {
        units.push(AccessUnit {
            data: cur,
            keyframe: cur_keyframe,
            config: cur_config,
        });
    }
    units
}

/// Split a buffer into NAL units including their leading start code. Returns
/// pairs of (start_code_length, slice_including_start_code).
fn split_nals(bytes: &[u8]) -> Vec<(usize, &[u8])> {
    let mut positions: Vec<(usize, usize)> = Vec::new(); // (offset, start_code_len)
    let mut i = 0;
    while i + 3 <= bytes.len() {
        if bytes[i] == 0 && bytes[i + 1] == 0 && bytes[i + 2] == 1 {
            positions.push((i, 3));
            i += 3;
        } else if i + 4 <= bytes.len()
            && bytes[i] == 0
            && bytes[i + 1] == 0
            && bytes[i + 2] == 0
            && bytes[i + 3] == 1
        {
            positions.push((i, 4));
            i += 4;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(positions.len());
    for idx in 0..positions.len() {
        let (start, sc_len) = positions[idx];
        let end = if idx + 1 < positions.len() {
            positions[idx + 1].0
        } else {
            bytes.len()
        };
        out.push((sc_len, &bytes[start..end]));
    }
    out
}

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut args = std::env::args().skip(1);
    let url = args
        .next()
        .unwrap_or_else(|| "ws://127.0.0.1:9000/publish/test-room/test-stream".to_string());
    let path = args.next().unwrap_or_else(|| "assets/test.h264".to_string());
    let looping = args.any(|a| a == "--loop");

    let bytes = std::fs::read(&path).unwrap_or_else(|e| {
        eprintln!("failed to read {path}: {e}");
        std::process::exit(1);
    });

    let source = H264FileSource::new(&bytes, 30, looping);
    log::info!(
        "loaded {} bytes -> {} access units from {path}",
        bytes.len(),
        source.access_units.len()
    );

    let diag = Diagnostics::new();
    log::info!("connecting to {url}");
    let mut transport = match WebSocketTransport::connect(&url).await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("connect failed: {e}");
            std::process::exit(1);
        }
    };
    log::info!("connected; streaming...");

    match run_sender(source, &mut transport, &diag).await {
        Ok(n) => {
            let s = diag.snapshot();
            log::info!(
                "done: {n} packets, {} bytes sent, last_seq={}, last_ts={}ms",
                s.bytes_sent,
                s.last_sequence,
                s.last_timestamp_ms
            );
        }
        Err(e) => {
            log::error!("sender error: {e}");
        }
    }
    let _ = transport.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    // 00 00 00 01 <SPS> | 00 00 00 01 <PPS> | 00 00 00 01 <IDR> | 00 00 01 <non-IDR>
    fn stream() -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&[0, 0, 0, 1, 0x67, 0xAA]); // SPS (type 7)
        v.extend_from_slice(&[0, 0, 0, 1, 0x68, 0xBB]); // PPS (type 8)
        v.extend_from_slice(&[0, 0, 0, 1, 0x65, 0xCC]); // IDR slice (type 5)
        v.extend_from_slice(&[0, 0, 1, 0x41, 0xDD]); // non-IDR slice (type 1)
        v
    }

    #[test]
    fn detects_start_codes() {
        let s = stream();
        let nals = split_nals(&s);
        assert_eq!(nals.len(), 4);
        assert_eq!(nals[0].0, 4); // 4-byte start code
        assert_eq!(nals[3].0, 3); // 3-byte start code
    }

    #[test]
    fn groups_first_au_as_keyframe_with_config() {
        let units = split_access_units(&stream());
        // SPS+PPS+IDR form the first access unit; the non-IDR slice is the second.
        assert_eq!(units.len(), 2);
        assert!(units[0].keyframe);
        assert!(units[0].config);
        assert!(!units[1].keyframe);
        assert!(!units[1].config);
    }

    #[test]
    fn first_au_contains_all_three_nals() {
        let units = split_access_units(&stream());
        // 3 NALs concatenated: SPS(6) + PPS(6) + IDR(6) = 18 bytes.
        assert_eq!(units[0].data.len(), 18);
    }
}
