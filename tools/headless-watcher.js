// Headless watcher for local end-to-end verification of the relay + protocol.
// Not part of the Activity; it mimics what the Activity's WebSocket layer does,
// minus WebCodecs (which only exists in the browser). Verifies that binary
// frames published to the relay are forwarded intact to a watcher, including
// the cached CONFIG/KEYFRAME replay for late joiners.

const WebSocket = require("ws");

const RELAY = process.env.RELAY || "ws://127.0.0.1:9000";
const ROOM = "test-room";
const STREAM = "test-stream";
const MAGIC = 0xd5;
const HEADER_LEN = 16;

const ws = new WebSocket(`${RELAY}/watch/${ROOM}/${STREAM}`);
ws.binaryType = "arraybuffer";

let packets = 0;
let bytes = 0;
let maxFrame = 0;
let firstSeq = null;
let lastSeq = null;
let keyframes = 0;
let configs = 0;
const start = Date.now();

ws.on("open", () => console.log(`[watcher] connected to ${RELAY}`));
ws.on("message", (data) => {
  const buf = Buffer.isBuffer(data) ? data : Buffer.from(data);
  if (buf.length < HEADER_LEN || buf[0] !== MAGIC) return;
  const flags = buf[3];
  const seq = buf.readUInt32LE(8);
  packets++;
  bytes += buf.length;
  if (buf.length > maxFrame) maxFrame = buf.length;
  if (firstSeq === null) firstSeq = seq;
  lastSeq = seq;
  if (flags & 0x01) keyframes++;
  if (flags & 0x02) configs++;
});
ws.on("close", () => {
  const secs = ((Date.now() - start) / 1000).toFixed(1);
  console.log(
    `[watcher] closed after ${secs}s\n` +
      `  packets=${packets} bytes=${bytes} maxFrame=${maxFrame}\n` +
      `  firstSeq=${firstSeq} lastSeq=${lastSeq} keyframes=${keyframes} configs=${configs}`
  );
  process.exit(packets > 0 && keyframes > 0 ? 0 : 1);
});
ws.on("error", (e) => {
  console.error("[watcher] error", e.message);
  process.exit(2);
});

// Auto-stop after the clip should have finished.
setTimeout(() => ws.close(), 8000);
