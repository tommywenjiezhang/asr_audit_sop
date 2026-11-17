import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// templates/ is served as static files so "sop-images/imageN.png" resolves in previews/editor.
export default defineConfig({
  plugins: [svelte()],
  publicDir: "templates",
  clearScreen: false,
  server: { port: 1420, strictPort: true, watch: { ignored: ["**/src-tauri/**"] } },
  // dist-web (not dist): dist\ holds release packages (portable ZIP) and must not be emptied by frontend builds
  build: { target: "es2022", outDir: "dist-web", emptyOutDir: true },
});
