import { defineConfig } from "vite"
import solid from "vite-plugin-solid"
import { fileURLToPath } from "node:url"
export default defineConfig({
  plugins: [solid()],
  build: { lib: { entry: fileURLToPath(new URL("./src/gui-experimental.tsx", import.meta.url)), formats: ["cjs"], fileName: () => "gui-experimental.cjs" }, rollupOptions: { external: ["@opencode/plugin/gui", "@opencode/plugin/gui/main", "@opencode/client", "electron", "effect", "solid-js", "@local-voice/client"] } },
})
