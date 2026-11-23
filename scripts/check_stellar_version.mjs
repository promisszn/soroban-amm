#!/usr/bin/env node
// Fails when any workflow's `stellar/stellar-cli@vX` pin drifts from
// `.stellar-version`, or when a workflow hardcodes a second CLI version (#1353).
//
// `.stellar-version` is the single source of truth for the Stellar CLI pin:
// Dockerfile reads it directly, and ci.yml, smoke-test.yml, and release.yml
// must pin the matching `stellar/stellar-cli@v<version>` action tag.
//
//   node scripts/check_stellar_version.mjs

import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const SEMVER_RE = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/;
const ACTION_RE = /\buses:\s*['"]?stellar\/stellar-cli@([^\s'"#]+)['"]?/;
const HARDCODED_VERSION_RES = [
  /\.stellar-version[^#\n]*==?\s*["']?v?\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?["']?/,
  /\.stellar-version[^#\n]*["']v?\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?["']/,
  /\bstellar-cli\b[^#\n]*--version(?:=|\s+)["']?v?\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?["']?/,
  /--version(?:=|\s+)["']?v?\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?["']?[^#\n]*\bstellar-cli\b/,
];

/** Reads and validates `.stellar-version` from `root`. */
export function readPinnedVersion(root = ROOT) {
  const versionFile = join(root, ".stellar-version");
  if (!existsSync(versionFile)) {
    throw new Error(".stellar-version is missing");
  }
  const version = readFileSync(versionFile, "utf8").trim();
  if (!SEMVER_RE.test(version)) {
    throw new Error(`.stellar-version: expected semver like "27.1.0", found "${version}"`);
  }
  return version;
}

/** Finds every non-comment `uses: stellar/stellar-cli@<ref>` entry with 1-based line numbers. */
export function findStellarCliPins(yamlText) {
  const pins = [];
  const lines = yamlText.split(/\r?\n/);
  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const match = ACTION_RE.exec(line);
    if (match) {
      pins.push({ line: i + 1, ref: match[1] });
    }
  }
  return pins;
}

/** Finds workflow lines that hardcode a literal Stellar CLI version check or install. */
export function findHardcodedCliVersions(yamlText) {
  const hits = [];
  const lines = yamlText.split(/\r?\n/);
  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    if (HARDCODED_VERSION_RES.some((re) => re.test(line))) {
      hits.push({ line: i + 1, text: trimmed });
    }
  }
  return hits;
}

/** Checks every `.github/workflows/*.yml` file against `.stellar-version`. */
export function checkStellarVersions(root = ROOT) {
  let version;
  try {
    version = readPinnedVersion(root);
  } catch (error) {
    return {
      version: null,
      checked: 0,
      errors: [error instanceof Error ? error.message : String(error)],
    };
  }

  const expectedRef = `v${version}`;
  const workflowsDir = join(root, ".github", "workflows");
  if (!existsSync(workflowsDir)) {
    return { version, checked: 0, errors: [".github/workflows directory is missing"] };
  }

  const errors = [];
  let checked = 0;
  const files = readdirSync(workflowsDir)
    .filter((name) => /\.ya?ml$/.test(name))
    .sort();

  for (const name of files) {
    const text = readFileSync(join(workflowsDir, name), "utf8");
    for (const { line, ref } of findStellarCliPins(text)) {
      checked += 1;
      if (ref !== expectedRef) {
        errors.push(
          `.github/workflows/${name}:${line}: pins stellar/stellar-cli@${ref}, expected ${expectedRef} (.stellar-version)`,
        );
      }
    }
    for (const { line, text: lineText } of findHardcodedCliVersions(text)) {
      errors.push(
        `.github/workflows/${name}:${line}: hardcodes a second Stellar CLI version (${lineText}); use scripts/check_stellar_version.mjs`,
      );
    }
  }

  if (checked === 0) {
    errors.push(".github/workflows: found no stellar/stellar-cli@<ref> action pins");
  }

  return { version, checked, errors };
}

function main() {
  const { version, checked, errors } = checkStellarVersions();
  if (errors.length > 0) {
    console.error(
      `Stellar CLI version drift${version ? ` from .stellar-version (${version})` : ""}:`,
    );
    for (const err of errors) console.error(`  - ${err}`);
    console.error(
      "\nUpdate .stellar-version and every .github/workflows/*.yml stellar/stellar-cli@vX pin together.",
    );
    process.exit(1);
  }
  console.log(`OK: Stellar CLI v${version} (.stellar-version); ${checked} workflow pin(s) match.`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main();
}
