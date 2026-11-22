# Security policy

tvpn handles private keys and changes system network settings, so security
reports are taken seriously.

## Reporting a vulnerability

Please **do not open a public issue**. Use GitHub's private vulnerability
reporting ("Security" tab → "Report a vulnerability") on this repository.
Include the version (`tvpn --version`), OS, and steps to reproduce. You should
get an acknowledgement within a few days.

## Supported versions

Only the latest release receives security fixes.

## Security model (summary)

- The WireGuard protocol implementation is Cloudflare's `boringtun`; tvpn does
  not implement its own cryptography.
- Profiles and server configs are written with owner-only permissions (0600 on
  Unix). Private keys are never logged; `tvpn show` hides them unless
  `--reveal` is given.
- tvpn never executes `PreUp`/`PostUp`/`PreDown`/`PostDown` hooks from imported
  configs. They are reported and ignored.
- A running session listens on `127.0.0.1` only. `status` is readable by local
  users; `down` and `reload` require a random token stored in a root/admin-only
  file.
- Incoming packets are only accepted from addresses inside the sending peer's
  `AllowedIPs` (cryptokey routing).
- On Windows the Wintun driver is either shipped next to the binary or
  downloaded over HTTPS and verified against a pinned SHA-256 before use.
