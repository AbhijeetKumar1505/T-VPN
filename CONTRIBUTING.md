# Contributing to tvpn

Thanks for helping! Bug reports, docs fixes and pull requests are all welcome.

## Development setup

You need a stable Rust toolchain (see `rust-version` in `Cargo.toml`) and a C
compiler (for `ring`): MSVC Build Tools on Windows, Xcode command line tools on
macOS, `build-essential` on Linux.

```sh
cargo build
cargo test                       # unit tests + engine end-to-end tests (no root needed)
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

### Real-kernel end-to-end test (Linux)

`tests/e2e/netns.sh` builds throwaway network namespaces connected by veth
pairs and runs a real client and server through them: full-tunnel routing,
DNS rewrite/restore, server forwarding + NAT, live peer reload and crash
recovery. It needs no root where unprivileged user namespaces are allowed
(most distros, WSL2); otherwise run it with `sudo`.

```sh
cargo build && bash tests/e2e/netns.sh target/debug/tvpn
```

### Running locally

`tvpn up` needs root/administrator rights. To avoid touching your real config
while hacking, point tvpn at scratch directories:

```sh
export TVPN_CONFIG_DIR=/tmp/tvpn-dev/config TVPN_RUNTIME_DIR=/tmp/tvpn-dev/run
```

## Project layout

| Path | What |
|------|------|
| `src/engine.rs` | WireGuard data plane (peers, cryptokey routing, timers) on top of `boringtun` |
| `src/config.rs` | WireGuard `.conf` parser/writer |
| `src/netcfg/` | Routes, DNS, forwarding, NAT per OS, with a journal for undo |
| `src/control.rs` | Session state file + local control channel (`status`/`down`/`reload`) |
| `src/commands/` | CLI commands; `session.rs` is the shared foreground runtime |
| `src/server.rs` | Server config store, peer/IP allocation, client config generation |
| `packaging/` | npm, Homebrew, winget templates (rendered at release time) |
| `install.sh`, `install.ps1` | One-line installers |

## Pull requests

`main` is protected by a ruleset: every change lands through a pull request,
force-pushes and branch deletion are blocked, and these CI checks must pass:
Format, Clippy and Test (Linux, macOS, Windows), MSRV, the real-tunnel e2e test,
and the installer checks. GitHub Copilot reviews every pull request
automatically (guided by `.github/copilot-instructions.md`), and all review
conversations must be resolved before merging.

- Keep changes focused; add tests for behavior changes.
- Run `cargo fmt`, `cargo clippy` and `cargo test` before pushing.
- Update `CHANGELOG.md` under "Unreleased".

By contributing you agree that your contributions are dual-licensed under
MIT OR Apache-2.0, like the rest of the project.
