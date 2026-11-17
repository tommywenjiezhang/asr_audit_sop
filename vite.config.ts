import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// templates/ is served as static files so "sop-images/imageN.png" resolves in previews/editor.
export default defineConfig({
  plugins: [svelte()],
  publicDir: "templates",
  clearScreen: false,
  server: { port: 1420, strictPort: true, watch: { ignored: ["**/src-tauri/**"] } },
  build: { target: "es2022", outDir: "dist", emptyOutDir: true },
});
