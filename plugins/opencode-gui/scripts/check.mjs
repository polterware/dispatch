// Builds the GUI into a temporary file and fails when the committed bundle
// differs, so the dispatch binary never embeds a stale one.
import { execFileSync } from "node:child_process"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { fileURLToPath } from "node:url"

const here = path.dirname(fileURLToPath(import.meta.url))
const pkg = path.join(here, "..")
const committed = path.join(pkg, "../../crates/dispatch-core/src/setup/opencode/gui-experimental.cjs")
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "dispatch-gui-"))
const out = path.join(tmp, "gui-experimental.cjs")
try {
  execFileSync("pnpm", ["exec", "vite", "build", "--logLevel", "error"], { cwd: pkg, stdio: "inherit", env: { ...process.env, DISPATCH_GUI_OUT: out } })
  if (!fs.readFileSync(out).equals(fs.readFileSync(committed))) {
    console.error(`${path.relative(process.cwd(), committed)} is stale: run pnpm --dir plugins/opencode-gui build`)
    process.exit(1)
  }
  console.log("dispatch-opencode bundle OK")
} finally {
  fs.rmSync(tmp, { recursive: true, force: true })
}
