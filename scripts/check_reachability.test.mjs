// Tests for the reachability guard (issue #1168).
// Run with: node --test scripts/check_reachability.test.mjs

import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  PACKAGES,
  findOrphanedModules,
  findProblems,
  importsOf,
  reachableModules,
} from "./check_reachability.mjs";

/** Builds a throwaway package tree and returns its root plus a cleanup fn. */
function fixture(files) {
  const root = mkdtempSync(join(tmpdir(), "reachability-"));
  for (const [rel, contents] of Object.entries(files)) {
    const full = join(root, rel);
    mkdirSync(join(full, ".."), { recursive: true });
    writeFileSync(full, contents);
  }
  return { root, cleanup: () => rmSync(root, { recursive: true, force: true }) };
}

const PKG = { name: "demo", entry: "pkg/src/index.ts" };

test("passes when every module is reachable from the entrypoint", () => {
  const { root, cleanup } = fixture({
    "pkg/src/index.ts": 'export { a } from "./a.js";\n',
    "pkg/src/a.ts": 'import { b } from "./nested/b.js";\nexport const a = b;\n',
    "pkg/src/nested/b.ts": "export const b = 1;\n",
  });
  try {
    assert.deepEqual(findOrphanedModules(PKG, root), []);
  } finally {
    cleanup();
  }
});

test("fails on a deliberately orphaned module in a test fixture", () => {
  const { root, cleanup } = fixture({
    "pkg/src/index.ts": 'export { a } from "./a.js";\n',
    "pkg/src/a.ts": "export const a = 1;\n",
    // Nothing imports this one.
    "pkg/src/orphan.ts": "export const orphan = 1;\n",
  });
  try {
    const problems = findOrphanedModules(PKG, root);
    assert.equal(problems.length, 1);
    assert.match(problems[0], /orphan\.ts is not reachable/);
  } finally {
    cleanup();
  }
});

test("ignores test, test-support stub and declaration files", () => {
  const { root, cleanup } = fixture({
    "pkg/src/index.ts": 'export { a } from "./a.js";\n',
    "pkg/src/a.ts": "export const a = 1;\n",
    "pkg/src/a.test.ts": 'import { a } from "./a.js";\n',
    "pkg/src/dom-stub.js": "export function installFakeDocument() {}\n",
    "pkg/src/api.stub.ts": "export const fakeApi = {};\n",
    "pkg/src/types.d.ts": "export type T = number;\n",
  });
  try {
    assert.deepEqual(findOrphanedModules(PKG, root), []);
  } finally {
    cleanup();
  }
});

test("findProblems scans multiple packages and reports orphans per package", () => {
  const packages = [
    { name: "alpha", entry: "packages/alpha/src/index.ts" },
    { name: "beta", entry: "services/beta/src/index.ts" },
  ];
  const { root, cleanup } = fixture({
    "packages/alpha/src/index.ts": 'export { a } from "./a.js";\n',
    "packages/alpha/src/a.ts": "export const a = 1;\n",
    "services/beta/src/index.ts": 'export { b } from "./b.js";\n',
    "services/beta/src/b.ts": "export const b = 2;\n",
    "services/beta/src/orphan.ts": "export const dead = 3;\n",
    "services/beta/src/dom-stub.js": "export const stub = {};\n",
  });
  try {
    const problems = findProblems(root, packages);
    assert.equal(problems.length, 1);
    assert.match(problems[0], /^beta: services\/beta\/src\/orphan\.ts is not reachable/);
  } finally {
    cleanup();
  }
});

test("reports a missing entrypoint instead of throwing", () => {
  const { root, cleanup } = fixture({ "pkg/src/a.ts": "export const a = 1;\n" });
  try {
    const problems = findOrphanedModules(PKG, root);
    assert.equal(problems.length, 1);
    assert.match(problems[0], /entrypoint .* does not exist/);
  } finally {
    cleanup();
  }
});

test("follows type-only imports and re-exports", () => {
  const { root, cleanup } = fixture({
    "pkg/src/index.ts": 'export type { T } from "./types.js";\nexport * from "./impl.js";\n',
    "pkg/src/types.ts": "export type T = number;\n",
    "pkg/src/impl.ts": 'import type { T } from "./types.js";\nexport const x: T = 1;\n',
  });
  try {
    assert.deepEqual(findOrphanedModules(PKG, root), []);
  } finally {
    cleanup();
  }
});

test("importsOf reads static, type-only and dynamic specifiers", () => {
  const { root, cleanup } = fixture({
    "pkg/src/index.ts":
      'import { a } from "./a.js";\n' +
      'import type { T } from "./t.js";\n' +
      'export { b } from "./b.js";\n' +
      'const c = await import("./c.js");\n',
    "pkg/src/a.ts": "export const a = 1;\n",
    "pkg/src/t.ts": "export type T = number;\n",
    "pkg/src/b.ts": "export const b = 1;\n",
    "pkg/src/c.ts": "export const c = 1;\n",
  });
  try {
    const specs = importsOf(join(root, "pkg/src/index.ts")).sort();
    assert.deepEqual(specs, ["./a.js", "./b.js", "./c.js", "./t.js"]);
  } finally {
    cleanup();
  }
});

test("reachableModules includes the entrypoint itself", () => {
  const { root, cleanup } = fixture({
    "pkg/src/index.ts": 'export { a } from "./a.js";\n',
    "pkg/src/a.ts": "export const a = 1;\n",
  });
  try {
    const reached = reachableModules(join(root, "pkg/src/index.ts"));
    assert.equal(reached.size, 2);
  } finally {
    cleanup();
  }
});

test("the real package graph is fully reachable", () => {
  assert.deepEqual(findProblems(), []);
});

test("the guard is configured for every checked package and service", () => {
  const names = PACKAGES.map((p) => p.name);
  for (const expected of [
    "sdk",
    "ui-components",
    "ts-advanced-client",
    "graphql-api",
    "webhook-streamer",
    "health-dashboard",
  ]) {
    assert.ok(names.includes(expected), `expected PACKAGES to include ${expected}`);
  }
});
