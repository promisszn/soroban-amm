import assert from "node:assert/strict";
import test from "node:test";
import { PoolIndexer } from "./indexer.js";

function makeIndexer() {
  return new PoolIndexer();
}

test("getTwal returns null when there is no price history in the window", () => {
  const indexer = makeIndexer();
  assert.equal(indexer.getTwal("pool-x", 3600), null);
});

test("getTwal returns a time-weighted value that differs from the naive mean for unevenly spaced points", () => {
  const indexer = makeIndexer();
  const now = Date.now();
  // Price held at 1.0 for a long stretch, then briefly spikes to 2.0 just before "now".
  indexer.recordPrice({ poolId: "pool-x", timestamp: now - 3000, price: 1.0, feeBps: 30 });
  indexer.recordPrice({ poolId: "pool-x", timestamp: now - 100, price: 2.0, feeBps: 30 });

  const twal = indexer.getTwal("pool-x", 3600);
  const naiveMean = (1.0 + 2.0) / 2;

  assert.notEqual(twal, null);
  assert.notEqual(twal, naiveMean);
  // The 1.0 price dominates the window (2900ms vs 100ms), so twal should be close to 1.0.
  assert.ok(twal! < naiveMean);
});

test("getTwal ignores price points outside the requested window", () => {
  const indexer = makeIndexer();
  const now = Date.now();
  indexer.recordPrice({ poolId: "pool-x", timestamp: now - 7200 * 1000, price: 5.0, feeBps: 30 });
  indexer.recordPrice({ poolId: "pool-x", timestamp: now - 10, price: 1.0, feeBps: 30 });

  const twal = indexer.getTwal("pool-x", 60);
  assert.equal(twal, 1.0);
});

test("getTwal is scoped to the given poolId", () => {
  const indexer = makeIndexer();
  const now = Date.now();
  indexer.recordPrice({ poolId: "pool-a", timestamp: now - 10, price: 1.0, feeBps: 30 });
  assert.equal(indexer.getTwal("pool-b", 3600), null);
});
