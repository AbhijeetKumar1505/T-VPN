# Instructions for GitHub Copilot (code review and chat)

tvpn is an on-demand WireGuard VPN CLI in Rust: one binary that is both a
client (`tvpn up/down/status`) and a self-hosted server (`tvpn server ...`) on
Linux, macOS and Windows. The data plane is `boringtun`. TUN devices come from
`tun-rs`, routes from `route_manager`.

## Layout

- `src/engine.rs`: multi-peer WireGuard data plane (cryptokey routing, timers). Hot path.
- `src/config.rs`: WireGuard `.conf` parser/writer (must stay wg-quick compatible).
- `src/netcfg/`: routes, DNS, forwarding and NAT per OS. Every change is journaled in `Applied`.
- `src/control.rs`: session state file and the localhost control channel (`status`/`down`/`reload`).
- `src/commands/`: CLI commands. `session.rs` is the shared foreground runtime and teardown.
- `packaging/`, `install.sh`, `install.ps1`, `.github/workflows/`: distribution.

## What to check in reviews (highest priority first)

1. **Secrets.** Private and pre-shared keys must never be logged, printed by default, or included
   in errors. `Key`'s `Debug` is redacted, so keep it that way. Files with keys are written via
   `paths::write_private` (0600 on Unix).
2. **Untrusted input.** Imported configs must never cause command execution (`PostUp` etc. are
   ignored on purpose). The control channel accepts `down`/`reload` only with the token and
   only from loopback. Packets from a peer must stay restricted to its `AllowedIPs`.
3. **Reversible system changes.** Any new route, DNS, sysctl or firewall change must be
   recorded in `netcfg::Applied` as it is applied (and persisted), and undone in
   `netcfg::revert`, so that `tvpn down` can clean up after a crash. Flag changes that could
   leave a user's network broken after Ctrl+C, an error mid-setup, or `kill -9`.
4. **Cross-platform correctness.** Code must compile and behave on Linux, macOS and Windows.
   Check `#[cfg(...)]` gating, path handling, and that Windows (cmd.exe, PowerShell) and
   Unix shells both get sensible output. Use `console`/`ui` helpers, not raw ANSI codes.
5. **Hot path.** In `engine.rs`, avoid per-packet allocations, holding the peer `Mutex`
   across `.await`, and blocking calls. Socket errors that are transient (e.g. Windows
   `ConnectionReset` on UDP) must not kill the tunnel.
6. **Errors and UX.** Use `anyhow` with context that tells the user what to do next. User-facing
   messages are lowercase and concise. Exit codes: 0 ok, 1 error, 3 = not connected (`status`).

## Conventions

- MSRV is Rust 1.88 (edition 2024). Don't rely on newer std APIs.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test` must pass.
- Behavior changes need tests: unit tests next to the code, `tests/engine_e2e.rs` for the
  data plane, `tests/e2e/netns.sh` for real routing/DNS/NAT on Linux.
- `install.sh` must stay POSIX `sh` (no bashisms). `install.ps1` must work in Windows PowerShell 5.1.
- CI job names are required status checks in the `main` ruleset. Renaming a job in
  `.github/workflows/ci.yml` needs a matching ruleset update.
