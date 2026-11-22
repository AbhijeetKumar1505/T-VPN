# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.2] - 2026-11-22

### Fixed
- `tvpn up` without any imported profile now explains where WireGuard configs
  come from (a VPN provider or `tvpn server add-peer`) and how to import one,
  instead of pointing at `tvpn import --help`. `tvpn import --help` now has
  examples too.
- `tvpn server init` rejects an empty, URL-shaped or `host:port` `--endpoint`
  up front instead of saving a server config that later breaks `add-peer`.

### Changed
- README: the client quick start explains that tvpn needs a WireGuard config
  and that profile names like `work` are labels you choose.

## [0.1.1] - 2026-11-22

### Changed
- Dependency updates: `sha2` 0.11, `zip` 9 and `base64` 0.23 (`sha2` and `zip`
  are used to verify and unpack the Wintun driver on Windows).
- The Homebrew formula is now updated through a pull request or a deploy key,
  because `main` is protected by a repository ruleset.

### Internal
- Repository rulesets: pull requests, required CI checks and automatic Copilot
  code review on `main`; release tags can't be moved or deleted.
- `.github/copilot-instructions.md` guides automated code review.
- The MSRV CI job pins its toolchain through an input, so Dependabot can't
  mistake the Rust version for an action version.

## [0.1.0] - 2026-11-22

### Added
- `tvpn up` / `down` / `status`: on-demand WireGuard client that runs in the
  foreground with a live status line and disconnects cleanly on Ctrl+C,
  console close, SIGTERM or `tvpn down` from another terminal.
- Full-tunnel and split-tunnel routing on Linux, macOS and Windows, with an
  endpoint bypass route so the tunnel never routes its own traffic.
- DNS handling: systemd-resolved, `/etc/resolv.conf` (symlink-safe, WSL-safe),
  macOS `networksetup`, Windows adapter DNS. Original settings are restored.
- Crash recovery: every system change is journaled; `tvpn down` reverts the
  changes of a session that was killed.
- Profiles are standard WireGuard `.conf` files (`import`, `list`, `show`,
  `remove`), compatible with any WireGuard provider.
- Self-hosted server: `tvpn server init | add-peer | remove-peer | peers |
  show-peer | up | down | reload`, with IP forwarding, iptables NAT and
  firewall rules on Linux, QR codes for the WireGuard mobile apps, and live
  peer reload without dropping sessions.
- Key tools compatible with `wg`: `genkey`, `pubkey`, `genpsk`, plus `keypair`.
- `tvpn doctor` system check and shell completions for bash, zsh, fish,
  PowerShell and elvish.
- Windows: Wintun driver located next to `tvpn.exe`, or downloaded once and
  verified against a pinned SHA-256.
- Installers: npm, pip/pipx/uv (maturin), Homebrew, winget, `install.sh`,
  `install.ps1`, `cargo install`.
