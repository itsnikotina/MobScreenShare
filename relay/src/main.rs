//! Minimal media relay for the transport POC.
//!
//! Responsibilities (and nothing more):
//! * accept a streamer (publisher) connection and viewer (watcher) connections;
//! * associate each connection to a `room_id` / `stream_id`;
//! * forward binary frames from the publisher to every watcher of that stream;
//! * cache the most recent CONFIG and KEYFRAME packets so a viewer that joins
//!   mid-stream can start decoding immediately.
//!
//! It never decodes, transcodes, or stores media beyond the tiny keyframe cache.
//!
//! Transport: plain `ws://` by default. Set `RELAY_TLS_CERT` and `RELAY_TLS_KEY`
//! (PEM files) to serve `wss://` directly. For production behind Discord's proxy
//! you can either terminate TLS here or put the relay behind a TLS-terminating
//! reverse proxy (nginx/caddy/cloudflared) — the Discord URL mapping only needs
//! a `wss` target.
//!
//! Routing is by URL path (works through Discord's proxy via a `wss` URL
//! mapping). Two shapes are accepted:
//!
//! ```text
//! /publish/{room_id}/{stream_id}     the streamer
//! /watch/{room_id}/{stream_id}       a viewer
//! ```
//!
//! Auth is intentionally absent for the POC, but connections are keyed by
//! (room, stream), so adding a token check at accept time is a localized change.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

/// Bytes forwarded to a watcher, over an mpsc channel.
type WatcherTx = mpsc::UnboundedSender<Vec<u8>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Publish,
    Watch,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct StreamKey {
    room: String,
    stream: String,
}

/// Per-stream shared state.
#[derive(Default)]
struct StreamState {
    watchers: HashMap<u64, WatcherTx>,
    /// Last CONFIG packet seen (H.264 SPS/PPS), replayed to late joiners.
    last_config: Option<Vec<u8>>,
    /// Last KEYFRAME packet seen, replayed to late joiners.
    last_keyframe: Option<Vec<u8>>,
}

type Streams = Arc<Mutex<HashMap<StreamKey, Arc<Mutex<StreamState>>>>>;

// Protocol constants mirrored from stream-core (relay stays dependency-light and
// only peeks at the header to spot CONFIG/KEYFRAME for the join cache).
const MAGIC: u8 = 0xD5;
const HEADER_LEN: usize = 16;
const FLAG_KEYFRAME: u8 = 0b0000_0001;
const FLAG_CONFIG: u8 = 0b0000_0010;

fn parse_target(path: &str) -> Option<(Role, StreamKey)> {
    // Strip query string if present.
    let path = path.split('?').next().unwrap_or(path);
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    if parts.len() != 3 {
        return None;
    }
    let role = match parts[0] {
        "publish" => Role::Publish,
        "watch" => Role::Watch,
        _ => return None,
    };
    Some((
        role,
        StreamKey {
            room: parts[1].to_string(),
            stream: parts[2].to_string(),
        },
    ))
}

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let addr = std::env::var("RELAY_ADDR").unwrap_or_else(|_| "0.0.0.0:9000".to_string());
    let listener = TcpListener::bind(&addr)
        .await
        .expect("failed to bind relay address");

    // Optional TLS: enabled when both cert and key paths are provided.
    let tls_acceptor = load_tls_acceptor();
    log::info!(
        "relay listening on {addr} ({})",
        if tls_acceptor.is_some() { "wss/TLS" } else { "ws/plain" }
    );

    let streams: Streams = Arc::new(Mutex::new(HashMap::new()));
    let mut next_id: u64 = 0;

    while let Ok((tcp, peer)) = listener.accept().await {
        next_id += 1;
        let id = next_id;
        let streams = streams.clone();
        let acceptor = tls_acceptor.clone();
        tokio::spawn(async move {
            let result = match acceptor {
                Some(acc) => match acc.accept(tcp).await {
                    Ok(tls) => handle_connection(tls, peer, id, streams).await,
                    Err(e) => {
                        log::warn!("connection {id} ({peer}): TLS handshake failed: {e}");
                        return;
                    }
                },
                None => handle_connection(tcp, peer, id, streams).await,
            };
            if let Err(e) = result {
                log::warn!("connection {id} ({peer}) ended: {e}");
            }
        });
    }
}

/// Build a rustls acceptor if `RELAY_TLS_CERT` and `RELAY_TLS_KEY` are set.
fn load_tls_acceptor() -> Option<tokio_rustls::TlsAcceptor> {
    let cert_path = std::env::var("RELAY_TLS_CERT").ok()?;
    let key_path = std::env::var("RELAY_TLS_KEY").ok()?;

    let cert_pem = std::fs::read(&cert_path)
        .unwrap_or_else(|e| panic!("failed to read RELAY_TLS_CERT {cert_path}: {e}"));
    let key_pem = std::fs::read(&key_path)
        .unwrap_or_else(|e| panic!("failed to read RELAY_TLS_KEY {key_path}: {e}"));

    let certs: Vec<_> = rustls_pemfile::certs(&mut &cert_pem[..])
        .collect::<Result<_, _>>()
        .expect("invalid certificate PEM");
    let key = rustls_pemfile::private_key(&mut &key_pem[..])
        .expect("invalid key PEM")
        .expect("no private key found in RELAY_TLS_KEY");

    let config = tokio_rustls::rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("failed to build TLS config");
    Some(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
}

async fn handle_connection<S>(
    stream: S,
    peer: SocketAddr,
    id: u64,
    streams: Streams,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    // Capture the request path during the handshake to learn role + stream key.
    let mut target: Option<(Role, StreamKey)> = None;
    let ws = tokio_tungstenite::accept_hdr_async(stream, |req: &Request, resp: Response| {
        target = parse_target(req.uri().path());
        Ok(resp)
    })
    .await?;

    let (role, key) = match target {
        Some(t) => t,
        None => {
            log::warn!("connection {id} ({peer}): bad path, closing");
            return Ok(());
        }
    };
    log::info!("connection {id} ({peer}): {role:?} room={} stream={}", key.room, key.stream);

    let state = {
        let mut map = streams.lock().await;
        map.entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(StreamState::default())))
            .clone()
    };

    match role {
        Role::Publish => handle_publisher(ws, id, key, state, streams).await,
        Role::Watch => handle_watcher(ws, id, state).await,
    }
}

async fn handle_publisher<S>(
    mut ws: WebSocketStream<S>,
    id: u64,
    key: StreamKey,
    state: Arc<Mutex<StreamState>>,
    streams: Streams,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    while let Some(msg) = ws.next().await {
        match msg? {
            Message::Binary(bytes) => {
                update_cache(&state, &bytes).await;
                fan_out(&state, &bytes).await;
            }
            Message::Close(_) => break,
            Message::Ping(p) => ws.send(Message::Pong(p)).await?,
            _ => {}
        }
    }
    log::info!("publisher {id} disconnected; clearing stream {}/{}", key.room, key.stream);
    // Drop the stream entry so watchers see the source is gone.
    streams.lock().await.remove(&key);
    Ok(())
}

async fn handle_watcher<S>(
    mut ws: WebSocketStream<S>,
    id: u64,
    state: Arc<Mutex<StreamState>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();

    // Register and replay cached CONFIG + KEYFRAME so decoding can start now.
    {
        let mut s = state.lock().await;
        if let Some(cfg) = &s.last_config {
            let _ = tx.send(cfg.clone());
        }
        if let Some(kf) = &s.last_keyframe {
            let _ = tx.send(kf.clone());
        }
        s.watchers.insert(id, tx);
    }

    let result = loop {
        tokio::select! {
            outbound = rx.recv() => match outbound {
                Some(bytes) => {
                    if let Err(e) = ws.send(Message::Binary(bytes)).await {
                        break Err(e.into());
                    }
                }
                None => break Ok(()),
            },
            inbound = ws.next() => match inbound {
                Some(Ok(Message::Close(_))) | None => break Ok(()),
                Some(Ok(Message::Ping(p))) => { let _ = ws.send(Message::Pong(p)).await; }
                Some(Ok(_)) => {}
                Some(Err(e)) => break Err(e.into()),
            },
        }
    };

    state.lock().await.watchers.remove(&id);
    log::info!("watcher {id} disconnected");
    result
}

/// Peek at the header and cache CONFIG / KEYFRAME packets for late joiners.
async fn update_cache(state: &Arc<Mutex<StreamState>>, bytes: &[u8]) {
    if bytes.len() < HEADER_LEN || bytes[0] != MAGIC {
        return;
    }
    let flags = bytes[3];
    if flags & FLAG_CONFIG != 0 {
        state.lock().await.last_config = Some(bytes.to_vec());
    }
    if flags & FLAG_KEYFRAME != 0 {
        state.lock().await.last_keyframe = Some(bytes.to_vec());
    }
}

/// Forward a frame to every watcher. Slow watchers whose channel is closed are
/// pruned; the unbounded channel means a slow socket cannot block the publisher.
async fn fan_out(state: &Arc<Mutex<StreamState>>, bytes: &[u8]) {
    let mut s = state.lock().await;
    let mut dead = Vec::new();
    for (wid, tx) in s.watchers.iter() {
        if tx.send(bytes.to_vec()).is_err() {
            dead.push(*wid);
        }
    }
    for wid in dead {
        s.watchers.remove(&wid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_publish_path() {
        let (role, key) = parse_target("/publish/test-room/test-stream").unwrap();
        assert_eq!(role, Role::Publish);
        assert_eq!(key.room, "test-room");
        assert_eq!(key.stream, "test-stream");
    }

    #[test]
    fn parses_watch_path_with_query() {
        let (role, key) = parse_target("/watch/r1/s1?token=abc").unwrap();
        assert_eq!(role, Role::Watch);
        assert_eq!(key.room, "r1");
        assert_eq!(key.stream, "s1");
    }

    #[test]
    fn rejects_unknown_role() {
        assert!(parse_target("/bogus/r/s").is_none());
    }

    #[test]
    fn rejects_wrong_segment_count() {
        assert!(parse_target("/publish/only-room").is_none());
        assert!(parse_target("/publish/a/b/c").is_none());
    }
}
