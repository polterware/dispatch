import { isBuiltin } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

const here = path.dirname(fileURLToPath(import.meta.url));

// The modules OpenCode Desktop provides to a plugin's GUI, each the app's own
// instance, as @opencode/plugin/gui/vite keeps them external.
const provided = new Set([
  "effect",
  "@opencode/plugin/gui",
  "@opencode/plugin/gui/main",
  "@opencode/plugin/rpc",
  "@opencode/schema/rpc",
  "@opencode/client",
  "@opencode/client/effect",
  "solid-js",
  "solid-js/web",
  "solid-js/store",
  "@opencode/plugin/gui/file",
  "@opencode/plugin/gui/review",
  "@opencode/plugin/gui/browser",
  "@opencode/client/solid",
]);

// One CommonJS file next to the plugin's server part, which dispatch-core embeds
// (tooling/sync-opencode-plugin.sh copies the plugin into go-core while it lives);
// DISPATCH_GUI_OUT writes it elsewhere (the check script).
const outFile = process.env.DISPATCH_GUI_OUT ?? path.join(here, "../../crates/dispatch-core/src/setup/opencode/gui-experimental.cjs");

export default defineConfig({
  plugins: [
    solid(),
    {
      name: "opencode-gui",
      enforce: "pre",
      resolveId(source) {
        if (provided.has(source) || source === "electron" || isBuiltin(source)) return { id: source, external: true };
        if (/^(effect|solid-js|@opencode)\//.test(source)) this.error(`${source} is not a module OpenCode provides to plugin GUIs`);
        return null;
      },
    },
  ],
  resolve: { conditions: ["browser"] },
  build: {
    outDir: path.dirname(outFile),
    emptyOutDir: false,
    minify: false,
    target: "es2022",
    lib: { entry: path.join(here, "src/gui.tsx"), formats: ["cjs"], fileName: () => path.basename(outFile) },
    rollupOptions: { output: { exports: "named", inlineDynamicImports: true } },
  },
});
