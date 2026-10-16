import { defineConfig } from "vite";
export default defineConfig({
  build: {
    outDir: "lib",
    sourcemap: true,
    lib: {
      entry: "src/index.ts",
      formats: ["es"],
      fileName: "pendingcrew-tour",
      cssFileName: "pendingcrew-tour",
    },
    rollupOptions: { external: ["react", "react-dom", "react/jsx-runtime"] },
  },
});
