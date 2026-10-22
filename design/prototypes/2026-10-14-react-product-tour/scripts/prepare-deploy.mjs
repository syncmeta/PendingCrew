import { cp, mkdir, readdir, rm, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
const root = fileURLToPath(new URL("../", import.meta.url));
const site = path.join(root, "deploy/site");
await rm(site, { recursive: true, force: true });
await mkdir(site, { recursive: true });
await cp(path.join(root, "dist"), path.join(site, "crewdemo"), {
  recursive: true,
});
async function removeMaps(dir) {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const file = path.join(dir, entry.name);
    if (entry.isDirectory()) await removeMaps(file);
    else if (entry.name.endsWith(".map")) await rm(file);
  }
}
await removeMaps(site);
await writeFile(
  path.join(site, "_headers"),
  "/crewdemo/*\n  X-Content-Type-Options: nosniff\n  Referrer-Policy: strict-origin-when-cross-origin\n",
);
console.log("Prepared runtime assets under /crewdemo/ (source maps excluded).");
