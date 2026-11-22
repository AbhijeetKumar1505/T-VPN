#!/usr/bin/env node
// Launcher for the native tvpn binary, which npm installs from the matching
// platform package (tvpn-<platform>-<arch>) listed in optionalDependencies.
"use strict";

const { spawnSync } = require("child_process");

const PACKAGES = {
  "linux-x64": "tvpn-linux-x64",
  "linux-arm64": "tvpn-linux-arm64",
  "darwin-x64": "tvpn-darwin-x64",
  "darwin-arm64": "tvpn-darwin-arm64",
  "win32-x64": "tvpn-win32-x64",
  "win32-arm64": "tvpn-win32-arm64",
};

function binaryPath() {
  if (process.env.TVPN_BINARY) return process.env.TVPN_BINARY;
  const key = `${process.platform}-${process.arch}`;
  const pkg = PACKAGES[key];
  if (!pkg) {
    throw new Error(`tvpn: no prebuilt binary for ${key}. Build from source with: cargo install tvpn`);
  }
  const exe = process.platform === "win32" ? "tvpn.exe" : "tvpn";
  try {
    return require.resolve(`${pkg}/bin/${exe}`);
  } catch (_) {
    throw new Error(
      `tvpn: the platform package "${pkg}" is missing. ` +
        `Reinstall without --no-optional/--omit=optional (e.g. npm install -g tvpn).`
    );
  }
}

let bin;
try {
  bin = binaryPath();
} catch (e) {
  console.error(e.message);
  process.exit(1);
}

// Let the native process handle Ctrl+C itself (it disconnects cleanly);
// the launcher just waits for it to exit.
for (const sig of ["SIGINT", "SIGTERM", "SIGHUP", "SIGBREAK"]) {
  try {
    process.on(sig, () => {});
  } catch (_) {
    /* signal not supported on this platform */
  }
}

const result = spawnSync(bin, process.argv.slice(2), { stdio: "inherit", windowsHide: false });
if (result.error) {
  console.error(`tvpn: failed to start ${bin}: ${result.error.message}`);
  process.exit(1);
}
process.exit(result.status === null ? 1 : result.status);
