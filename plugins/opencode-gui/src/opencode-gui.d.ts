// The slice of OpenCode's GUI plugin SDK this plugin uses. The app provides
// the module at runtime; its full types live in @opencode/plugin/gui.
declare module "@opencode/plugin/gui" {
  type Registry<T> = { readonly id: string; readonly __item?: T };
  export const Plugin: { define<T>(definition: T): T };
  export const Command: Registry<unknown>;
  export const Panel: Registry<unknown>;
  export const SettingsPage: Registry<unknown>;
  export const ComposerPill: Registry<unknown>;
  export const MenuItem: Registry<unknown>;
  export const Style: Registry<string>;
}

declare module "*/rpc.js" {
  export const Dispatch: { id: string; methods: Record<string, unknown>; events: Record<string, unknown> };
}
