import { createServer } from "vite";
import { spawn } from "node:child_process";
const server = await createServer({
  server: { host: "127.0.0.1", port: 0, strictPort: true },
});
await server.listen();
const port = server.httpServer.address().port;
function run(file) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [file], {
      stdio: "inherit",
      env: { ...process.env, TOUR_TEST_URL: `http://127.0.0.1:${port}` },
    });
    child.on("error", reject);
    child.on("exit", (code) =>
      code === 0 ? resolve() : reject(Error(`${file} exited with ${code}`)),
    );
  });
}
try {
  await run("verify.cjs");
  await run("verify-overview.cjs");
  await run("verify-features.cjs");
  await run("verify-copy.cjs");
  await run("verify-groups.cjs");
  await run("verify-library.cjs");
  await run("verify-data.cjs");
  await run("verify-react.cjs");
} finally {
  await server.close();
}
