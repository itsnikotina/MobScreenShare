// Screen Share POC — Activity viewer.
//
// Connects to the media relay over WebSocket, parses the 16-byte binary media
// header (see core/src/protocol.rs), and decodes H.264 access units with
// WebCodecs, painting them to a canvas.
//
// Runs in two environments:
//  * Standalone browser (local testing): connects directly to the relay URL.
//  * Discord Activity: all traffic must go through the proxy, so the relay is
//    reached via a same-origin `/.proxy/...` path that maps to the relay's wss.
//
// The header layout (little-endian) mirrors the Rust side exactly:
//   0 magic(0xD5) 1 version 2 media_type 3 flags 4 codec 5 rsv 6..8 rsv
//   8 sequence(u32) 12 timestamp_ms(u32) 16.. payload

const MAGIC = 0xd5;
const HEADER_LEN = 16;
const FLAG_KEYFRAME = 0x01;
const FLAG_CONFIG = 0x02;

const CFG = window.POC_CONFIG || {};
const ROOM = CFG.ROOM || "test-room";
const STREAM = CFG.STREAM || "test-stream";
const RELAY_PROXY_PREFIX = CFG.RELAY_PROXY_PREFIX || "/relay";

// ---- diagnostics state ----
const stats = {
  packets: 0,
  bytes: 0,
  frames: 0,
  lastSeq: null,
  maxFrame: 0,
  reconnects: 0,
};

const el = (id) => document.getElementById(id);

function humanBytes(n) {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(2)} MB`;
}

function log(msg) {
  const box = el("log");
  const line = `[${new Date().toLocaleTimeString()}] ${msg}\n`;
  box.textContent = (line + box.textContent).slice(0, 4000);
  // eslint-disable-next-line no-console
  console.log("[poc]", msg);
}

function setState(name, cls) {
  const s = el("s-state");
  s.textContent = name;
  s.className = `state ${cls}`;
}

function renderStats() {
  el("s-packets").textContent = stats.packets;
  el("s-bytes").textContent = humanBytes(stats.bytes);
  el("s-frames").textContent = stats.frames;
  el("s-seq").textContent = stats.lastSeq ?? "—";
  el("s-maxframe").textContent = humanBytes(stats.maxFrame);
  el("s-reconnects").textContent = stats.reconnects;
}

// ---- video decode ----
const canvas = el("video");
const ctx = canvas.getContext("2d");
let decoder = null;
let decoderConfigured = false;

function ensureDecoder() {
  if (decoder && decoder.state !== "closed") return decoder;
  decoder = new VideoDecoder({
    output: (frame) => {
      stats.frames++;
      if (canvas.width !== frame.displayWidth) {
        canvas.width = frame.displayWidth;
        canvas.height = frame.displayHeight;
        el("s-res").textContent = `${frame.displayWidth}x${frame.displayHeight}`;
      }
      ctx.drawImage(frame, 0, 0);
      frame.close();
      renderStats();
    },
    error: (e) => {
      el("s-decoder").textContent = "error";
      log(`decoder error: ${e.message}`);
      // Recover: drop the codec and re-sync on the next keyframe instead of
      // hammering a closed codec.
      decoderConfigured = false;
      if (decoder && decoder.state !== "closed") {
        try {
          decoder.close();
        } catch {
          /* ignore */
        }
      }
      decoder = null;
    },
  });
  decoderConfigured = false;
  return decoder;
}

// Try hardware first, then software; resolves to a supported config or null.
async function pickConfig() {
  const base = { codec: "avc1.42e01f", optimizeForLatency: true };
  const candidates = [
    base,
    { ...base, hardwareAcceleration: "prefer-software" },
    { ...base, hardwareAcceleration: "prefer-hardware" },
  ];
  for (const cfg of candidates) {
    try {
      if (typeof VideoDecoder.isConfigSupported === "function") {
        const res = await VideoDecoder.isConfigSupported(cfg);
        if (res && res.supported) return res.config || cfg;
      } else {
        return cfg;
      }
    } catch (e) {
      log(`isConfigSupported failed: ${e.message}`);
    }
  }
  return null;
}

let configuring = false;
async function configureDecoder() {
  if (configuring) return;
  configuring = true;
  try {
    const cfg = await pickConfig();
    if (!cfg) {
      el("s-decoder").textContent = "unsupported";
      log("no supported H.264 decoder config in this environment");
      return;
    }
    const dec = ensureDecoder();
    dec.configure(cfg);
    decoderConfigured = true;
    el("s-decoder").textContent = "configured";
    log(
      `decoder configured (${cfg.codec}, hw=${cfg.hardwareAcceleration || "default"})`
    );
  } catch (e) {
    el("s-decoder").textContent = "error";
    log(`configure failed: ${e.message}`);
  } finally {
    configuring = false;
  }
}

function handlePacket(buf) {
  const view = new DataView(buf);
  if (view.byteLength < HEADER_LEN || view.getUint8(0) !== MAGIC) {
    return; // not our packet
  }
  const flags = view.getUint8(3);
  const sequence = view.getUint32(8, true);
  const timestampMs = view.getUint32(12, true);
  const payload = new Uint8Array(buf, HEADER_LEN);

  stats.packets++;
  stats.bytes += buf.byteLength;
  stats.lastSeq = sequence;
  if (buf.byteLength > stats.maxFrame) stats.maxFrame = buf.byteLength;

  const isKey = (flags & FLAG_KEYFRAME) !== 0;
  const isConfig = (flags & FLAG_CONFIG) !== 0;

  if (!decoderConfigured) {
    // Wait for a keyframe (which carries SPS/PPS in Annex-B) before decoding.
    if (!isKey && !isConfig) {
      renderStats();
      return;
    }
    // Configuration is async; kick it off and drop this packet. The next
    // keyframe will be decoded once the codec is ready.
    configureDecoder();
    renderStats();
    return;
  }

  // Codec configured but a decoder error may have torn it down; guard state.
  if (!decoder || decoder.state !== "configured") {
    if (decoder && decoder.state === "closed") decoderConfigured = false;
    renderStats();
    return;
  }

  // After a decoder error we must resume on a keyframe, not a delta.
  if (!isKey && stats.frames === 0) {
    renderStats();
    return;
  }

  try {
    const chunk = new EncodedVideoChunk({
      type: isKey ? "key" : "delta",
      timestamp: timestampMs * 1000, // microseconds
      data: payload,
    });
    decoder.decode(chunk);
  } catch (e) {
    log(`decode threw: ${e.message}`);
    decoderConfigured = false;
  }
  renderStats();
}

// ---- transport ----
function buildRelayUrl() {
  // Query overrides for flexible local testing: ?relay=ws://host:port
  const params = new URLSearchParams(location.search);
  const override = params.get("relay");
  if (override) return `${override.replace(/\/$/, "")}/watch/${ROOM}/${STREAM}`;

  const inDiscord = location.hostname.endsWith("discordsays.com");
  if (inDiscord) {
    // Through the Discord proxy: same-origin path. The `/.proxy` prefix is
    // stripped by Discord and the request is forwarded to the URL-mapping
    // target you configured for RELAY_PROXY_PREFIX (which must be a `wss`
    // target). Result: wss://{clientId}.discordsays.com/.proxy/relay/watch/...
    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    return `${proto}//${location.host}/.proxy${RELAY_PROXY_PREFIX}/watch/${ROOM}/${STREAM}`;
  }
  // Local dev on GitHub Pages or file server: talk to localhost relay.
  return `ws://127.0.0.1:9000/watch/${ROOM}/${STREAM}`;
}

let backoff = 500;
function connect() {
  const url = buildRelayUrl();
  el("target").textContent = url;
  setState("CONNECTING", "connecting");
  log(`connecting to ${url}`);

  let ws;
  try {
    ws = new WebSocket(url);
  } catch (e) {
    log(`WebSocket ctor failed: ${e.message}`);
    scheduleReconnect();
    return;
  }
  ws.binaryType = "arraybuffer";

  ws.onopen = () => {
    setState("CONNECTED", "connected");
    backoff = 500;
    log("websocket open");
  };
  ws.onmessage = (ev) => {
    if (ev.data instanceof ArrayBuffer) handlePacket(ev.data);
  };
  ws.onerror = () => {
    setState("ERROR", "error");
    log("websocket error");
  };
  ws.onclose = (ev) => {
    setState("RECONNECTING", "connecting");
    log(`websocket closed (code ${ev.code}); will reconnect`);
    // Reset decoder so the next session re-syncs on a fresh keyframe.
    decoderConfigured = false;
    if (decoder && decoder.state !== "closed") {
      try { decoder.close(); } catch { /* ignore */ }
    }
    scheduleReconnect();
  };
}

function scheduleReconnect() {
  stats.reconnects++;
  renderStats();
  setTimeout(connect, backoff);
  backoff = Math.min(backoff * 2, 8000);
}

async function main() {
  el("s-room").textContent = ROOM;
  el("s-stream").textContent = STREAM;

  if (typeof VideoDecoder === "undefined") {
    setState("ERROR", "error");
    log("WebCodecs VideoDecoder is not available in this environment");
    return;
  }

  // If loaded as a Discord Activity, initialize the SDK for identity/context.
  // Kept optional so the same build runs standalone in a browser.
  if (location.hostname.endsWith("discordsays.com")) {
    try {
      const mod = await import(
        "https://esm.sh/@discord/embedded-app-sdk@1.9.0"
      );
      const clientId =
        CFG.DISCORD_CLIENT_ID && CFG.DISCORD_CLIENT_ID !== "YOUR_CLIENT_ID_HERE"
          ? CFG.DISCORD_CLIENT_ID
          : new URLSearchParams(location.search).get("client_id");
      if (clientId) {
        const sdk = new mod.DiscordSDK(clientId);
        await sdk.ready();
        log(`Discord SDK ready (instance ${sdk.instanceId})`);
      } else {
        log("no client_id configured; skipping Discord SDK init");
      }
    } catch (e) {
      log(`Discord SDK init skipped: ${e.message}`);
    }
  }

  connect();
}

main();
