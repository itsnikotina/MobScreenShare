// Tiny static file server for local testing of the Activity viewer.
const http = require("http");
const fs = require("fs");
const path = require("path");

const root = path.resolve(__dirname, "..", "activity");
const port = process.env.PORT || 8080;

http
  .createServer((req, res) => {
    let u = req.url.split("?")[0];
    if (u === "/") u = "/index.html";
    const fp = path.join(root, u);
    fs.readFile(fp, (err, data) => {
      if (err) {
        res.writeHead(404);
        res.end("not found");
        return;
      }
      const type = u.endsWith(".js")
        ? "text/javascript"
        : u.endsWith(".html")
        ? "text/html"
        : "application/octet-stream";
      res.writeHead(200, { "content-type": type });
      res.end(data);
    });
  })
  .listen(port, () => console.log(`static server on http://127.0.0.1:${port}`));
