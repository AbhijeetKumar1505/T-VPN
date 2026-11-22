# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.0]

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
