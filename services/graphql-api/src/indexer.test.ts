import assert from "node:assert/strict";
import test from "node:test";
import { InvalidMetricError, InvalidThresholdError, PoolIndexer } from "./indexer.js";

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

test("setAlertConfig rejects an unrecognized metric and does not store it", () => {
  const indexer = makeIndexer();
  assert.throws(
    () => indexer.setAlertConfig({ poolId: "pool-a", metric: "not_a_real_metric" as never, thresholdValue: 10 }),
    InvalidMetricError,
  );
  assert.deepEqual(indexer.getAlertConfigs("pool-a"), []);
});

test("setAlertConfig rejects a negative threshold", () => {
  const indexer = makeIndexer();
  assert.throws(
    () => indexer.setAlertConfig({ poolId: "pool-a", metric: "tvl", thresholdValue: -5 }),
    InvalidThresholdError,
  );
  assert.deepEqual(indexer.getAlertConfigs("pool-a"), []);
});

test("setAlertConfig accepts each valid metric and getAlertConfigs returns it", () => {
  const indexer = makeIndexer();
  indexer.setAlertConfig({ poolId: "pool-a", metric: "price_deviation", thresholdBps: 100 });
  indexer.setAlertConfig({ poolId: "pool-a", metric: "tvl", thresholdValue: 500 });
  indexer.setAlertConfig({ poolId: "pool-a", metric: "volume24h", thresholdValue: 1000 });

  const configs = indexer.getAlertConfigs("pool-a");
  assert.equal(configs.length, 3);
  assert.ok(configs.some((c) => c.metric === "price_deviation" && c.thresholdBps === 100));
  assert.ok(configs.some((c) => c.metric === "tvl" && c.thresholdValue === 500));
  assert.ok(configs.some((c) => c.metric === "volume24h" && c.thresholdValue === 1000));
});

test("setAlertConfig requires the correct threshold field for the given metric", () => {
  const indexer = makeIndexer();
  assert.throws(
    () => indexer.setAlertConfig({ poolId: "pool-a", metric: "price_deviation" }),
    InvalidThresholdError,
  );
  assert.throws(
    () => indexer.setAlertConfig({ poolId: "pool-a", metric: "tvl" }),
    InvalidThresholdError,
  );
});
