# tvpn

[![CI](https://github.com/AbhijeetKumar1505/T-VPN/actions/workflows/ci.yml/badge.svg)](https://github.com/AbhijeetKumar1505/T-VPN/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/AbhijeetKumar1505/T-VPN?sort=semver)](https://github.com/AbhijeetKumar1505/T-VPN/releases/latest)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

**An on-demand WireGuard VPN for your terminal.** Connect when you need it,
gone when you don't: no background service, no GUI, one native binary written
in Rust. The same binary also runs your own VPN server.

```console
$ tvpn up work
tvpn 0.1.0 · profile work
  interface  tvpn0 (10.8.0.2/32)
  endpoint   vpn.example.com:51820 (203.0.113.7:51820)
  routes     all traffic (0.0.0.0/0, ::/0)
  dns        1.1.1.1, 1.0.0.1
✔ Connected (handshake after 0.2s)
  ● connected · ↓12.4 MiB  ↑1.1 MiB · handshake 9s ago · up 2m 14s  Ctrl+C to disconnect
```

- **On demand.** `tvpn up` runs in the foreground with a live status line.
  Ctrl+C, closing the window, or `tvpn down` from another terminal
  disconnects and restores your routes and DNS exactly as they were.
- **Works in every shell.** cmd, PowerShell, Windows Terminal, bash, zsh and
  fish, on Windows, Linux, macOS and WSL. Completions are included.
- **Speaks standard WireGuard.** Profiles are ordinary `.conf` files, so
  configs from any WireGuard provider (Mullvad, Proton, IVPN, your router,
  wg-easy, ...) work as-is.
- **Your own server in three commands.** `tvpn server init`, `add-peer --qr`,
  `up`. Configs it generates also work in the official WireGuard apps for
  iOS and Android.
- **Safe by default.** Crash-safe cleanup, no hook scripts from imported
  configs, keys stored with owner-only permissions, and an anti-spoofing
  check on every incoming packet.

## Install

| Channel | Command |
|---|---|
| **Shell script** (Linux, macOS, WSL) | `curl -fsSL https://raw.githubusercontent.com/AbhijeetKumar1505/T-VPN/main/install.sh \| sh` |
| **PowerShell** (Windows) | `irm https://raw.githubusercontent.com/AbhijeetKumar1505/T-VPN/main/install.ps1 \| iex` |
| **cmd.exe** (Windows) | `powershell -NoProfile -ExecutionPolicy Bypass -c "irm https://raw.githubusercontent.com/AbhijeetKumar1505/T-VPN/main/install.ps1 \| iex"` |
| **Homebrew** (macOS, Linux) | `brew tap abhijeetkumar1505/tvpn https://github.com/AbhijeetKumar1505/T-VPN && brew install tvpn` |
| **npm** (any OS) | `npm install -g tvpn` |
| **pip / pipx / uv** (any OS) | `pipx install tvpn` · `uv tool install tvpn` · `pip install tvpn` |
| **winget** (Windows) | `winget install AbhijeetKumar1505.tvpn` |
| **Cargo** | `cargo install tvpn` |

Prebuilt binaries for Linux (x86_64/arm64, static), macOS (Intel/Apple
Silicon) and Windows (x64/arm64) are attached to every
[GitHub release](https://github.com/AbhijeetKumar1505/T-VPN/releases) with a `SHA256SUMS`
file. The script installers verify the checksum before installing.

Then check your system:

```sh
tvpn doctor
```

## Use it as a client

```sh
# 1. Import a WireGuard config from your provider or server
tvpn import ~/Downloads/us-east.conf --name work

# 2. Connect (asks for sudo/admin if needed). Ctrl+C disconnects.
tvpn up work

# From another terminal:
tvpn status          # live status (exit code 3 = not connected)
tvpn status --json   # for scripts
tvpn down            # disconnect
```

You can also connect straight from a file without importing it:
`tvpn up ./wg0.conf`. With a single profile, `tvpn up` is enough.

| Flag | Effect |
|---|---|
| `--no-dns` | leave system DNS untouched |
| `--interface NAME` | name of the tunnel interface (default `tvpn0`, `tvpn` on Windows) |
| `-q, --quiet` | no banner or status line |
| `--no-sudo` | never re-run through sudo automatically |

## Run your own server

On any Linux machine with a public IP (a $5 VPS is plenty):

```sh
tvpn server init --endpoint vpn.example.com   # keys, subnet 10.8.0.0/24, UDP 51820
tvpn server add-peer laptop                   # prints a client config
tvpn server add-peer phone --qr               # QR code for the WireGuard mobile app
tvpn server up                                # serve in the foreground (Ctrl+C stops)
```

Remember to allow UDP 51820 in your firewall or cloud security group.

While the server runs, tvpn enables IP forwarding, adds NAT (masquerade) and
firewall-accept rules with `iptables`, and removes all of them again on
shutdown. Peers added or removed while it runs are applied live
(`tvpn server reload`) without dropping existing sessions.

On the client, import the generated config (`tvpn import laptop.conf`) or use
`tvpn server add-peer laptop --import` when both run on the same machine.

| Command | |
|---|---|
| `tvpn server init [--endpoint HOST] [--port N] [--subnet CIDR] [--ipv6] [--dns IPs]` | create the server config |
| `tvpn server add-peer NAME [--qr] [-o FILE] [--allowed-ips CIDRS]` | add a client |
| `tvpn server peers` | list clients (with live status when running) |
| `tvpn server show-peer NAME [--qr]` | print a client config again |
| `tvpn server remove-peer NAME` | revoke a client |
| `tvpn server up [--no-nat] [--no-firewall] [--wan IFACE]` | run the server |
| `tvpn server down` / `tvpn server reload` | stop it / apply peer changes |

Server mode is fully automated on Linux. On macOS and Windows it runs too,
but without NAT: peers can reach the server machine itself, not the internet
through it.

## All commands

```text
tvpn up [PROFILE|FILE]     connect (foreground)        tvpn genkey     private key (like wg genkey)
tvpn down                  disconnect                  tvpn pubkey     public key from stdin
tvpn status [--json]       connection status           tvpn genpsk     pre-shared key
tvpn import FILE [--name]  save a WireGuard config     tvpn keypair    private + public key
tvpn list                  list profiles               tvpn doctor     check this system
tvpn show NAME [--qr]      show a profile              tvpn completions SHELL
tvpn remove NAME           delete a profile            tvpn server ... run your own server
```

Use `-v`, `-vv`, `-vvv` for more logging, `--no-color` (or `NO_COLOR=1`) for
plain output.

### Shell completions

```sh
tvpn completions bash > ~/.local/share/bash-completion/completions/tvpn
tvpn completions zsh  > "${fpath[1]}/_tvpn"
tvpn completions fish > ~/.config/fish/completions/tvpn.fish
tvpn completions powershell | Out-String | Invoke-Expression   # add to $PROFILE to keep it
```

## Platform notes

**Windows.** Creating the tunnel adapter needs an elevated terminal ("Run as
administrator"), or `sudo tvpn up` with Sudo for Windows enabled. tvpn uses
the [Wintun](https://www.wintun.net) driver from the WireGuard project.
Release zips, winget and npm ship `wintun.dll` next to `tvpn.exe`. Otherwise
it is downloaded once and verified against a pinned SHA-256. DNS servers are
set on the tunnel adapter, and the adapter gets a low interface metric so
Windows prefers it.

**Linux.** `tvpn up` re-runs itself through `sudo` when needed. Profiles are
still read from *your* home directory. DNS goes through `systemd-resolved`
(`resolvectl`) when it is in use; otherwise `/etc/resolv.conf` is replaced
while connected and restored afterwards (symlinks are preserved). Needs
`/dev/net/tun`; containers need `--cap-add NET_ADMIN --device /dev/net/tun`.

**macOS.** Uses the built-in `utun` driver. DNS is set per network service
with `networksetup` and restored on disconnect.

**WSL2.** Works like Linux, but the tunnel only affects the WSL VM, not
Windows apps. Use the Windows build for that.

## How it works

```
 apps ──► tvpn0 (TUN) ──► engine ──► UDP ──► WireGuard server
                           │  boringtun (Noise IK, ChaCha20-Poly1305)
                           │  cryptokey routing: AllowedIPs ⇄ peers
                           └─ timers: handshakes, rekeying, keepalives, retries
```

- The data plane is Cloudflare's [boringtun](https://github.com/cloudflare/boringtun)
  WireGuard implementation, driven by a small multi-peer engine. The same
  engine serves client and server roles.
- Full-tunnel routing adds `0.0.0.0/1` + `128.0.0.0/1` (and `::/1` +
  `8000::/1`) through the tunnel, plus a host route to the endpoint via your
  normal gateway. Your existing default route is never modified.
- Every system change (route, DNS, sysctl, firewall rule) is journaled to a
  state file *before* it is made. A normal exit undoes them. If the process is
  killed, `tvpn down` replays the journal to clean up.
- A running session answers `status`, `down` and `reload` on a localhost
  control port. Changing state requires a token that only root/admin can read.

### Files and environment

| | Linux / macOS | Windows |
|---|---|---|
| Profiles | `~/.config/tvpn/profiles/*.conf` | `%APPDATA%\tvpn\profiles\*.conf` |
| Server | `~/.config/tvpn/server/` | `%APPDATA%\tvpn\server\` |
| Session state | `/run/tvpn` (`/var/run/tvpn` on macOS) | `%ProgramData%\tvpn\run` |

`TVPN_CONFIG_DIR` and `TVPN_RUNTIME_DIR` override these locations.
`TVPN_WINTUN_DLL` points to a specific `wintun.dll`. `TVPN_LOG` sets a log
filter (e.g. `debug`).

## Limitations and roadmap

- No kill switch yet: if the tunnel drops, traffic is not blocked while tvpn
  reconnects. Planned: nftables/pf/WFP-based kill switch.
- Windows: other adapters' DNS servers can still be queried in parallel
  (Windows "smart multi-homed name resolution"). Planned: WFP DNS leak
  protection.
- NAT for server mode is automated on Linux only.
- `PreUp`/`PostUp` hooks in imported configs are intentionally ignored.

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md). In short: `cargo test` runs unit tests
and an in-process client↔server WireGuard test (no root), and
`tests/e2e/netns.sh` runs real tunnels through Linux network namespaces.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at
your option. WireGuard is a registered trademark of Jason A. Donenfeld. tvpn
is not affiliated with the WireGuard project.
