// Runtime configuration for the Activity viewer.
//
// Edit these values for your deployment. This file is plain JS (no build step)
// so it works directly on GitHub Pages. It is loaded before main.js.
//
// - DISCORD_CLIENT_ID: your application's OAuth2 client id (from the Dev Portal).
//   Only needed so the Embedded App SDK can initialize inside Discord. The POC
//   does not require authentication to view the test stream.
// - RELAY_PROXY_PREFIX: the URL-mapping prefix you configured in the Dev Portal
//   that targets your relay's host (see README). When running inside Discord the
//   viewer connects to `wss://{clientId}.discordsays.com/.proxy{PREFIX}/watch/...`.
// - ROOM / STREAM: fixed identifiers for the POC.
window.POC_CONFIG = {
  DISCORD_CLIENT_ID: "YOUR_CLIENT_ID_HERE",
  RELAY_PROXY_PREFIX: "/relay",
  ROOM: "test-room",
  STREAM: "test-stream",
};
