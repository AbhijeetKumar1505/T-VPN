#!/usr/bin/env node
// Render the Homebrew formula and winget manifests for a release.
//
//   node packaging/render.mjs --version 0.1.0 --repo AbhijeetKumar1505/T-VPN \
//        --checksums dist/SHA256SUMS [--winget-id Owner.tvpn] [--out dist/packaging]
//
// Placeholders: {{VERSION}} {{REPO}} {{REPO_OWNER}} {{REPO_OWNER_LOWER}} {{WINGET_ID}} {{DATE}}
//               {{SHA256:<asset>}} {{SHA256_UPPER:<asset>}}

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

function arg(name, fallback) {
  const i = process.argv.indexOf(`--${name}`);
  if (i !== -1 && process.argv[i + 1]) return process.argv[i + 1];
  if (fallback !== undefined) return fallback;
  console.error(`missing --${name}`);
  process.exit(2);
}

const version = arg("version").replace(/^v/, "");
const repo = arg("repo");
const owner = repo.split("/")[0];
const wingetId = arg("winget-id", `${owner}.tvpn`);
const out = resolve(arg("out", "dist/packaging"));
const sums = new Map(
  readFileSync(resolve(arg("checksums")), "utf8")
    .split(/\r?\n/)
    .filter(Boolean)
    .map((line) => {
      const [hash, file] = line.trim().split(/\s+\*?/);
      return [file, hash.toLowerCase()];
    })
);

function render(template) {
  return template.replace(/\{\{([A-Z0-9_]+)(?::([^}]+))?\}\}/g, (_, key, asset) => {
    switch (key) {
      case "VERSION": return version;
      case "REPO": return repo;
      case "REPO_OWNER": return owner;
      case "REPO_OWNER_LOWER": return owner.toLowerCase();
      case "WINGET_ID": return wingetId;
      case "DATE": return new Date().toISOString().slice(0, 10);
      case "SHA256":
      case "SHA256_UPPER": {
        const h = sums.get(asset);
        if (!h) throw new Error(`no checksum for ${asset} in SHA256SUMS`);
        return key === "SHA256_UPPER" ? h.toUpperCase() : h;
      }
      default: throw new Error(`unknown placeholder ${key}`);
    }
  });
}

const jobs = [
  ["homebrew/tvpn.rb.tmpl", "homebrew/tvpn.rb"],
  ["winget/tvpn.yaml.tmpl", `winget/${wingetId}.yaml`],
  ["winget/tvpn.installer.yaml.tmpl", `winget/${wingetId}.installer.yaml`],
  ["winget/tvpn.locale.en-US.yaml.tmpl", `winget/${wingetId}.locale.en-US.yaml`],
];
for (const [src, dst] of jobs) {
  const target = join(out, dst);
  mkdirSync(dirname(target), { recursive: true });
  writeFileSync(target, render(readFileSync(join(here, src), "utf8")));
  console.log(`rendered ${target}`);
}
