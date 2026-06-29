import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Tauri expects a fixed dev port and serves the built assets from dist/.
export default defineConfig({
  plugins: [svelte()],
  // Relative asset paths so the bundle loads inside Tauri's webview protocol
  // (absolute /assets/ paths render as a blank white window).
  base: "./",
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: "es2021",
    outDir: "dist",
    emptyOutDir: true,
  },
});
