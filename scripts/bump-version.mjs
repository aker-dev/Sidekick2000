#!/usr/bin/env node
// Bump the version in package.json, src-tauri/tauri.conf.json, and src-tauri/Cargo.toml
// in lockstep so a release tag always matches a single, consistent version.
//
// Usage:
//   node scripts/bump-version.mjs 0.2.0
//   npm run version:bump -- 0.2.0

import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "..");

const next = process.argv[2];
if (!next || !/^\d+\.\d+\.\d+(-[\w.]+)?$/.test(next)) {
  console.error("usage: bump-version.mjs <semver>  e.g. 0.2.0");
  process.exit(1);
}

function patch(path, transform) {
  const full = resolve(root, path);
  const before = readFileSync(full, "utf8");
  const after = transform(before);
  if (before === after) {
    console.error(`! no change in ${path} — version pattern not found`);
    process.exit(2);
  }
  writeFileSync(full, after);
  console.log(`  ${path}`);
}

console.log(`bumping to ${next}`);

patch("package.json", (s) =>
  s.replace(/("version"\s*:\s*)"[^"]+"/, `$1"${next}"`),
);
patch("src-tauri/tauri.conf.json", (s) =>
  s.replace(/("version"\s*:\s*)"[^"]+"/, `$1"${next}"`),
);
patch("src-tauri/Cargo.toml", (s) =>
  s.replace(/^version\s*=\s*"[^"]+"/m, `version = "${next}"`),
);

console.log("done. next steps:");
console.log(`  git commit -am "release: v${next}"`);
console.log(`  git tag v${next}`);
console.log(`  git push origin main --tags`);
