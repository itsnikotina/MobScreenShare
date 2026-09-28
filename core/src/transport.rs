//! Transport abstraction.
//!
//! The core never talks to a concrete socket type. It sends and receives opaque
//! binary messages through the [`Transport`] trait. Today the only
//! implementation is [`WebSocketTransport`]; in the future a
//! `WebTransportTransport` or `WebRTCTransport` can be added without touching
//! capture, encoder, audio, protocol, or streaming code.

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

/// Errors that any transport implementation can surface.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("connection closed")]
    Closed,
    #[error("transport i/o error: {0}")]
    Io(String),
}

/// A bidirectional, message-oriented byte transport.
///
/// Implementations must preserve message boundaries: one `send` of N bytes must
/// arrive as one `recv` of N bytes. This is what lets the media protocol rely on
/// implicit payload length (one media packet == one message).
#[async_trait]
pub trait Transport: Send {
    /// Send one binary message.
    async fn send(&mut self, bytes: Vec<u8>) -> Result<(), TransportError>;

    /// Receive the next binary message. Returns `Ok(None)` on graceful close.
    async fn recv(&mut self) -> Result<Option<Vec<u8>>, TransportError>;

    /// Close the transport.
    async fn close(&mut self) -> Result<(), TransportError>;
}

/// WebSocket transport backed by `tokio-tungstenite`.
///
/// Binary WebSocket frames map 1:1 to messages. Text/ping/pong frames are
/// handled transparently: pings are answered by the library, text frames are
/// ignored (the media path is binary-only).
pub struct WebSocketTransport {
    ws: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
}

impl WebSocketTransport {
    /// Connect to a `ws://` or `wss://` URL.
    pub async fn connect(url: &str) -> Result<Self, TransportError> {
        let (ws, _resp) = tokio_tungstenite::connect_async(url)
            .await
            .map_err(|e| TransportError::Io(e.to_string()))?;
        Ok(Self { ws })
    }
}

#[async_trait]
impl Transport for WebSocketTransport {
    async fn send(&mut self, bytes: Vec<u8>) -> Result<(), TransportError> {
        self.ws
            .send(Message::Binary(bytes))
            .await
            .map_err(|e| TransportError::Io(e.to_string()))
    }

    async fn recv(&mut self) -> Result<Option<Vec<u8>>, TransportError> {
        while let Some(msg) = self.ws.next().await {
            match msg.map_err(|e| TransportError::Io(e.to_string()))? {
                Message::Binary(b) => return Ok(Some(b)),
                Message::Close(_) => return Ok(None),
                // Ignore text/ping/pong at the media layer.
                _ => continue,
            }
        }
        Ok(None)
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        self.ws
            .close(None)
            .await
            .map_err(|e| TransportError::Io(e.to_string()))
    }
}
