#!/usr/bin/env node
// Assemble publishable npm packages from built release binaries.
//
//   node packaging/npm/build.mjs --version 0.1.0 --artifacts dist/bin --out dist/npm
//
// --artifacts must contain one directory per target triple holding the binary
// (and wintun.dll for Windows), e.g. dist/bin/x86_64-pc-windows-msvc/tvpn.exe.
// Targets without artifacts are skipped (and left out of optionalDependencies).
// Publish the platform packages first, then the `tvpn` package.

import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync, chmodSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "..", "..");

function arg(name, fallback) {
  const i = process.argv.indexOf(`--${name}`);
  if (i !== -1 && process.argv[i + 1]) return process.argv[i + 1];
  if (fallback !== undefined) return fallback;
  console.error(`missing --${name}`);
  process.exit(2);
}

const version = arg("version").replace(/^v/, "");
const artifacts = resolve(arg("artifacts"));
const out = resolve(arg("out", "dist/npm"));
const { targets } = JSON.parse(readFileSync(join(root, "packaging", "targets.json"), "utf8"));
const main = JSON.parse(readFileSync(join(here, "tvpn", "package.json"), "utf8"));

rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });

const built = [];
for (const t of targets) {
  const src = join(artifacts, t.triple);
  const exe = join(src, t.exe);
  if (!existsSync(exe)) {
    console.warn(`skip ${t.npm}: ${exe} not found`);
    continue;
  }
  const dir = join(out, t.npm);
  mkdirSync(join(dir, "bin"), { recursive: true });
  cpSync(exe, join(dir, "bin", t.exe));
  if (t.npm_os !== "win32") chmodSync(join(dir, "bin", t.exe), 0o755);
  const files = ["bin/"];
  if (existsSync(join(src, "wintun.dll"))) {
    cpSync(join(src, "wintun.dll"), join(dir, "bin", "wintun.dll"));
  }
  const pkg = {
    name: t.npm,
    version,
    description: `${main.description} (${t.triple} binary)`,
    license: main.license,
    repository: main.repository,
    homepage: main.homepage,
    os: [t.npm_os],
    cpu: [t.npm_cpu],
    files,
    preferUnplugged: true,
  };
  writeFileSync(join(dir, "package.json"), JSON.stringify(pkg, null, 2) + "\n");
  writeFileSync(join(dir, "README.md"), `# ${t.npm}\n\nThe \`${t.triple}\` binary for [tvpn](https://www.npmjs.com/package/tvpn). Install \`tvpn\` instead of this package.\n`);
  for (const lic of ["LICENSE-MIT", "LICENSE-APACHE"]) cpSync(join(root, lic), join(dir, lic));
  built.push(t.npm);
  console.log(`built ${t.npm}@${version}`);
}

if (built.length === 0) {
  console.error("no platform packages were built");
  process.exit(1);
}

const mainDir = join(out, "tvpn");
cpSync(join(here, "tvpn"), mainDir, { recursive: true });
main.version = version;
main.optionalDependencies = Object.fromEntries(built.map((name) => [name, version]));
writeFileSync(join(mainDir, "package.json"), JSON.stringify(main, null, 2) + "\n");
for (const f of ["README.md", "LICENSE-MIT", "LICENSE-APACHE"]) cpSync(join(root, f), join(mainDir, f));
console.log(`built tvpn@${version} with ${built.length} platform package(s)`);
