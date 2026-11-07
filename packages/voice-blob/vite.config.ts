import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

export default defineConfig({
  root: "demo",
  plugins: [solid()],
  build: { outDir: "../dist", emptyOutDir: true },
});
