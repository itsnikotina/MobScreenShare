//! Cross-platform abstractions for capture, encoding, and audio.
//!
//! These traits define the seams between the platform-agnostic core and the
//! per-OS backends (Windows: WGC/WASAPI/D3D11; future Linux: PipeWire/Wayland).
//! **No concrete implementation lives here** — the POC does not do real capture,
//! encoding, or audio. The traits exist so the real product can be built without
//! reshaping the core.

use crate::protocol::Codec;

/// An abstract, platform-independent video frame.
///
/// Backends decide the concrete pixel storage (a Windows backend may wrap an
/// `ID3D11Texture2D`; a Linux backend a DMA-BUF). The core only needs the
/// metadata below to drive an encoder.
#[derive(Debug, Clone, Copy)]
pub struct VideoFrameInfo {
    pub width: u32,
    pub height: u32,
    pub timestamp_ms: u32,
}

/// Captures video frames from a screen or window.
///
/// Windows: implemented over Windows Graphics Capture.
/// Linux (future): implemented over PipeWire / Wayland / X11.
pub trait VideoCapture: Send {
    /// Human-readable backend name, for diagnostics.
    fn backend_name(&self) -> &str;
    // Real methods (start/stop/next_frame) are intentionally omitted in the POC.
}

/// Encodes video frames into a compressed bitstream.
///
/// Windows: NVENC / AMF / QSV via Media Foundation, software fallback.
/// Linux (future): NVENC / VAAPI, software fallback.
pub trait VideoEncoder: Send {
    fn codec(&self) -> Codec;
    fn backend_name(&self) -> &str;
    // Real methods (configure/encode/force_keyframe) are omitted in the POC.
}

/// Captures system / per-application audio. Never a microphone.
///
/// Windows: WASAPI process loopback.
/// Linux (future): PipeWire.
pub trait AudioCapture: Send {
    fn backend_name(&self) -> &str;
    // Real methods are omitted in the POC.
}
