import { readFile, writeFile } from "node:fs/promises";
const [js, css, icon] = await Promise.all([
  readFile(new URL("../portable/app.js", import.meta.url), "utf8"),
  readFile(new URL("../portable/app.css", import.meta.url), "utf8"),
  readFile(new URL("../public/app-icon.png", import.meta.url)),
]);
// Escape HTML raw-text terminators without changing the JavaScript string value.
const script = js.replace(/<\/script/gi, "<\\/script");
const style = css.replace(/<\/style/gi, "<\\/style");
const html = `<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>PendingCrew</title><link rel="icon" type="image/png" href="data:image/png;base64,${icon.toString("base64")}"><style>body{margin:0}${style}</style></head><body><div id="root"></div><script>${script}</script></body></html>`;
const output = new URL("../PendingCrew-Demo.html", import.meta.url);
await writeFile(output, html);
console.log(`Single HTML ready: ${output.pathname} (${Buffer.byteLength(html)} bytes)`);
