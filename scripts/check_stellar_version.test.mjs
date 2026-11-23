// Tests for the Stellar CLI workflow version checker.
// Run with: node --test scripts/check_stellar_version.test.mjs

import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import {
  checkStellarVersions,
  findHardcodedCliVersions,
  findStellarCliPins,
  readPinnedVersion,
} from "./check_stellar_version.mjs";

function makeFixture(t, versionContent, workflows = {}) {
  const root = mkdtempSync(join(tmpdir(), "stellar-version-test-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  if (versionContent !== null) {
    writeFileSync(join(root, ".stellar-version"), versionContent, "utf8");
  }
  const wfDir = join(root, ".github", "workflows");
  mkdirSync(wfDir, { recursive: true });
  for (const [name, content] of Object.entries(workflows)) {
    writeFileSync(join(wfDir, name), content, "utf8");
  }
  return root;
}

test("current repository tree passes with zero version drift", () => {
  const result = checkStellarVersions();
  assert.deepEqual(result.errors, []);
  assert.equal(result.version, readPinnedVersion());
  assert.ok(result.checked >= 3, `expected at least 3 workflow pins, got ${result.checked}`);
});

test("matching workflow pins across multiple workflows pass", (t) => {
  const root = makeFixture(t, "27.1.0\n", {
    "ci.yml": "steps:\n  - uses: stellar/stellar-cli@v27.1.0\n",
    "smoke-test.yml": "steps:\n  - uses: 'stellar/stellar-cli@v27.1.0' # pinned\n",
  });
  const result = checkStellarVersions(root);
  assert.deepEqual(result.errors, []);
  assert.equal(result.version, "27.1.0");
  assert.equal(result.checked, 2);
});

test("mismatched workflow pin fails naming file, line, found ref, and expected version", (t) => {
  const root = makeFixture(t, "27.1.0\n", {
    "ci.yml": "steps:\n  - uses: stellar/stellar-cli@v27.1.0\n",
    "smoke-test.yml": "jobs:\n  smoke:\n    steps:\n      - uses: stellar/stellar-cli@v25.1.0\n",
  });
  const result = checkStellarVersions(root);
  assert.equal(result.errors.length, 1);
  assert.match(result.errors[0], /\.github\/workflows\/smoke-test\.yml:4:/);
  assert.match(result.errors[0], /@v25\.1\.0/);
  assert.match(result.errors[0], /v27\.1\.0/);
});

test("unpinned branch ref like @main fails", (t) => {
  const root = makeFixture(t, "27.1.0\n", {
    "ci.yml": "steps:\n  - uses: stellar/stellar-cli@main\n",
  });
  const result = checkStellarVersions(root);
  assert.equal(result.errors.length, 1);
  assert.match(result.errors[0], /@main/);
  assert.match(result.errors[0], /v27\.1\.0/);
});

test("hardcoded second CLI version check in workflow fails", (t) => {
  const root = makeFixture(t, "27.1.0\n", {
    "release.yml": [
      "steps:",
      "  - uses: stellar/stellar-cli@v27.1.0",
      '  - run: test "$(cat .stellar-version)" = "27.1.0"',
    ].join("\n"),
  });
  assert.equal(findHardcodedCliVersions(
    '  - run: test "$(cat .stellar-version)" = "27.1.0"\n',
  ).length, 1);
  const result = checkStellarVersions(root);
  assert.equal(result.errors.length, 1);
  assert.match(result.errors[0], /\.github\/workflows\/release\.yml:3:/);
  assert.match(result.errors[0], /hardcodes a second Stellar CLI version/);
});

test("commented-out stellar-cli pins are ignored", (t) => {
  const yaml = [
    "# - uses: stellar/stellar-cli@v23.0.0",
    "steps:",
    "  - uses: stellar/stellar-cli@v27.1.0 # matches .stellar-version",
  ].join("\n");
  assert.deepEqual(findStellarCliPins(yaml), [{ line: 3, ref: "v27.1.0" }]);
  const root = makeFixture(t, "27.1.0\n", { "ci.yml": yaml });
  const result = checkStellarVersions(root);
  assert.deepEqual(result.errors, []);
  assert.equal(result.checked, 1);
});

test("missing or invalid .stellar-version fails", (t) => {
  const missingRoot = makeFixture(t, null, {
    "ci.yml": "steps:\n  - uses: stellar/stellar-cli@v27.1.0\n",
  });
  assert.match(checkStellarVersions(missingRoot).errors[0], /\.stellar-version is missing/);

  const badRoot = makeFixture(t, "latest\n", {
    "ci.yml": "steps:\n  - uses: stellar/stellar-cli@v27.1.0\n",
  });
  assert.match(checkStellarVersions(badRoot).errors[0], /expected semver/);
});

test("zero workflow pins fails rather than silently passing", (t) => {
  const root = makeFixture(t, "27.1.0\n", {
    "ci.yml": "steps:\n  - uses: actions/checkout@v7\n",
  });
  const result = checkStellarVersions(root);
  assert.equal(result.checked, 0);
  assert.match(result.errors[0], /found no stellar\/stellar-cli/);
});

test("unquoted .stellar-version comparison and reversed-flag cargo install fail", (t) => {
  const root = makeFixture(t, "27.1.0\n", {
    "release.yml": [
      "steps:",
      "  - uses: stellar/stellar-cli@v27.1.0",
      '  - run: test "$(cat .stellar-version)" = 27.1.0',
      "  - run: cargo install --locked --version 27.1.0 stellar-cli",
    ].join("\n"),
  });
  const result = checkStellarVersions(root);
  assert.equal(result.errors.length, 2);
  assert.match(result.errors[0], /\.github\/workflows\/release\.yml:3:/);
  assert.match(result.errors[1], /\.github\/workflows\/release\.yml:4:/);
});
