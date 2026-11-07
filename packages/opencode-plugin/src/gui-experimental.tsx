import { Plugin } from "@opencode/plugin/gui"
import { main } from "./gui/main.ts"

// GUI process setup is defined through the documented Plugin.define setup/main entry points.
export default Plugin.define({ id: "local-voice", main, setup() { return () => {} } })
