# Polterware Dispatch

Dispatch delegates a task to a coding-agent CLI (Codex, Claude Code, Cursor, Grok, OpenCode, Pi, Gemini, Qwen, Copilot, Kimi, Antigravity, or any ACP agent you add), routed by difficulty and quota, and keeps each run's transcript, routing decision, usage, and worktree. Agents start runs with `dispatch run` or the Dispatch skill; the desktop app follows, merges, and tunes them. It is part of the [Polterware](https://github.com/polterware/polterware) ecosystem and works on plain folders without it.

## Layout

- `go-core`: the `dispatch` binary, its core, and the CLI (module `github.com/polterware/dispatch/go-core`). `dispatch serve` is the only process that opens `dispatch.db`.
- `crates/dispatch-core`: the core in Rust, which will replace `go-core`.
- `app`: the desktop app in GPUI (`dispatch-app`), on [polterware/ui](https://github.com/polterware/ui). It reads the core over Connect-JSON and never opens SQLite.
- `schemas`: the `dispatch.v1` contracts and `polterware/local/v1/dispatch_host.proto`, the `DispatchHostService` Polterware serves to Dispatch. Polterware vendors these files; change them here.
- `plugins/opencode-gui`: the OpenCode Desktop plugin (Solid), built into the file `go-core` embeds.
- `tooling`: the checks, the model catalog (`models.mjs`), the release bundle (`release/bundle.sh`), and the Sparkle updater (`updater`).
- `updates/dispatch.xml`: the Sparkle appcast the app checks daily.

## Run

```bash
(cd go-core && go build -o ../target/dispatch ./runtimes/cli)
./target/dispatch serve &
cargo run -p dispatch-app
```

The app attaches to the core of `$PWDISPATCH_DATA_DIR`, else `~/Library/Application Support/Polterware Dispatch`, and never starts one: any `dispatch` command starts it, and `dispatch stop` stops it. `crates/dispatch-core` compiles the protos with `protoc` (`PROTOC` names it when it is not on `PATH`).

## Checks

```bash
bash tooling/verify.sh
```

This runs `buf lint`, `go vet` and `go test` on `go-core`, the plugin tests, `cargo fmt --check`, `clippy`, and `cargo test`, the plugin's typecheck and build check, and the boundary check.

## Release

`bash tooling/release/bundle.sh <version>` builds `Dispatch.app` for macOS arm64, with its ZIP, DMG, checksums, and signed appcast. Pushing a `dispatch-v<version>` tag runs the release workflow, which attaches them to a draft GitHub release. Builds are signed ad hoc and not notarized.

## License

Apache-2.0. Third-party code and data keep their own licenses; see [NOTICE](NOTICE).
