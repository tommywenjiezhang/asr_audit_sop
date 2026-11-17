// Dev server for the user-guide screenshots only: the real UI with a mocked backend and fictional data
// (docs/guide/demo). Not used by the app build. Started by docs/guide/capture.py.
import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

export default defineConfig({
  plugins: [svelte()],
  publicDir: "templates",
  clearScreen: false,
  server: { port: 1430, strictPort: true, watch: { ignored: ["**/src-tauri/**", "**/docs/guide/images/**"] } },
});
