import { readFileSync } from "node:fs";
import { defineConfig } from "vite";

const tauriConfig = JSON.parse(
  readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"),
) as { version: string };

export default defineConfig({
  define: {
    "import.meta.env.VITE_CLOUDLEDGER_APP_VERSION": JSON.stringify(tauriConfig.version),
  },
  server: {
    port: 1420,
    strictPort: true,
    host: "127.0.0.1"
  },
  clearScreen: false,
  build: {
    outDir: "dist",
    emptyOutDir: true
  }
});
