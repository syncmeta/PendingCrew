import { build } from "esbuild";
import { mkdir, readFile, writeFile, copyFile } from "node:fs/promises";
await mkdir("portable", { recursive: true });
await build({
  entryPoints: ["src/main.tsx"],
  bundle: true,
  format: "iife",
  target: "es2022",
  outfile: "portable/app.js",
  minify: true,
  loader: { ".png": "dataurl" },
  define: { "process.env.NODE_ENV": '"production"' },
});
await writeFile(
  "portable/index.html",
  '<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>PendingCrew</title><link rel="icon" type="image/png" href="app-icon.png"><style>body{margin:0}</style><link rel="stylesheet" href="app.css"></head><body><div id="root"></div><script src="app.js"></script></body></html>',
);
await copyFile("public/app-icon.png", "portable/app-icon.png");
console.log("Portable React build ready; no module fetches required.");
const html = await readFile("dist/index.html", "utf8");
await writeFile(
  "dist/index.html",
  html.replace("</head>", "<style>body{margin:0}</style></head>"),
);
