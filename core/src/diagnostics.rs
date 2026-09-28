//! Lightweight, thread-safe counters for the POC.
//!
//! Tracks the metrics the prompt asks for: packets/bytes sent & received,
//! last sequence and timestamp, error and reconnect counts.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Default)]
struct Inner {
    packets_sent: AtomicU64,
    packets_received: AtomicU64,
    bytes_sent: AtomicU64,
    bytes_received: AtomicU64,
    last_sequence: AtomicU32,
    last_timestamp_ms: AtomicU32,
    errors: AtomicU64,
    reconnects: AtomicU64,
}

/// Cheap-to-clone handle to a shared set of counters.
#[derive(Clone, Default)]
pub struct Diagnostics {
    inner: Arc<Inner>,
}

/// Immutable snapshot of the counters at one moment.
#[derive(Debug, Clone, Copy)]
pub struct DiagnosticsSnapshot {
    pub packets_sent: u64,
    pub packets_received: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub last_sequence: u32,
    pub last_timestamp_ms: u32,
    pub errors: u64,
    pub reconnects: u64,
}

impl Diagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_sent(&self, bytes: usize, sequence: u32, timestamp_ms: u32) {
        self.inner.packets_sent.fetch_add(1, Ordering::Relaxed);
        self.inner
            .bytes_sent
            .fetch_add(bytes as u64, Ordering::Relaxed);
        self.inner.last_sequence.store(sequence, Ordering::Relaxed);
        self.inner
            .last_timestamp_ms
            .store(timestamp_ms, Ordering::Relaxed);
    }

    pub fn record_received(&self, bytes: usize) {
        self.inner.packets_received.fetch_add(1, Ordering::Relaxed);
        self.inner
            .bytes_received
            .fetch_add(bytes as u64, Ordering::Relaxed);
    }

    pub fn record_error(&self) {
        self.inner.errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_reconnect(&self) {
        self.inner.reconnects.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> DiagnosticsSnapshot {
        DiagnosticsSnapshot {
            packets_sent: self.inner.packets_sent.load(Ordering::Relaxed),
            packets_received: self.inner.packets_received.load(Ordering::Relaxed),
            bytes_sent: self.inner.bytes_sent.load(Ordering::Relaxed),
            bytes_received: self.inner.bytes_received.load(Ordering::Relaxed),
            last_sequence: self.inner.last_sequence.load(Ordering::Relaxed),
            last_timestamp_ms: self.inner.last_timestamp_ms.load(Ordering::Relaxed),
            errors: self.inner.errors.load(Ordering::Relaxed),
            reconnects: self.inner.reconnects.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_accumulate() {
        let d = Diagnostics::new();
        d.record_sent(100, 1, 33);
        d.record_sent(200, 2, 66);
        d.record_received(50);
        let s = d.snapshot();
        assert_eq!(s.packets_sent, 2);
        assert_eq!(s.bytes_sent, 300);
        assert_eq!(s.packets_received, 1);
        assert_eq!(s.bytes_received, 50);
        assert_eq!(s.last_sequence, 2);
        assert_eq!(s.last_timestamp_ms, 66);
    }
}
