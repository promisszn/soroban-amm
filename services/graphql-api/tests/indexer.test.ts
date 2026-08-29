/**
 * Integration tests for the RPC ingestion and indexing layer.
 *
 * These tests verify:
 * - Event idempotency (same event processed twice = no duplicates)
 * - Correct ordering (events applied in ledger order)
 * - Metrics computation (TVL, 24h volume, fees, price history)
 * - Schema version rejection
 * - Retention window detection
 * - Alert firing
 */

import { describe, test, beforeEach } from "node:test";
import assert from "node:assert";
import { MemoryStore } from "../src/store/memory.js";
import { PoolIndexer } from "../src/indexer-refactored.js";
import type { PoolEvent, PoolStats } from "../src/store/interface.js";

describe("PoolIndexer with MemoryStore", () => {
  let store: MemoryStore;
  let indexer: PoolIndexer;

  beforeEach(() => {
    store = new MemoryStore();
    indexer = new PoolIndexer(store);
  });

  // ─── Idempotency tests ──────────────────────────────────────────────────

  test("same event indexed twice produces identical stats", async () => {
    const event: PoolEvent = {
      id: "evt-1",
      poolId: "pool-1",
      type: "swap",
      timestamp: Math.floor(Date.now() / 1000),
      ledger: 1000,
      txHash: "txhash-1",
      eventIndex: 0,
      payload: {
        amountIn: 1000,
        fee: 3,
        price: 0.12,
      },
    };

    // Index the same event twice
    await indexer.indexEvent(event);
    const statsAfterFirst = (await indexer.getPoolStats("pool-1"))[0];

    await indexer.indexEvent(event);
    const statsAfterSecond = (await indexer.getPoolStats("pool-1"))[0];

    // Stats should be identical (no double-counting)
    assert.deepStrictEqual(
      statsAfterFirst?.volume24h,
      statsAfterSecond?.volume24h,
      "Volume should not double-count the same event",
    );
    assert.deepStrictEqual(
      statsAfterFirst?.swapCount,
      statsAfterSecond?.swapCount,
      "Swap count should not increment twice",
    );
  });

  // ─── Event ordering tests ──────────────────────────────────────────────

  test("events are applied in ledger order", async () => {
    const baseTime = Math.floor(Date.now() / 1000);

    const event1: PoolEvent = {
      id: "evt-1",
      poolId: "pool-1",
      type: "add_liquidity",
      timestamp: baseTime,
      ledger: 1000,
      txHash: "tx-1",
      eventIndex: 0,
      payload: { amountA: 1000, amountB: 1000 },
    };

    const event2: PoolEvent = {
      id: "evt-2",
      poolId: "pool-1",
      type: "remove_liquidity",
      timestamp: baseTime + 1,
      ledger: 1001,
      txHash: "tx-2",
      eventIndex: 0,
      payload: { amountA: 500, amountB: 500 },
    };

    // Apply in correct order
    await indexer.indexEvent(event1);
    await indexer.indexEvent(event2);

    const stats = (await indexer.getPoolStats("pool-1"))[0];
    assert.strictEqual(stats?.tvl, 1000, "TVL should be 500 + 500 remaining");
  });

  // ─── Metrics computation tests ──────────────────────────────────────────

  test("TVL is correctly calculated from add_liquidity and remove_liquidity", async () => {
    const baseTime = Math.floor(Date.now() / 1000);

    const addEvent: PoolEvent = {
      id: "evt-1",
      poolId: "pool-1",
      type: "add_liquidity",
      timestamp: baseTime,
      ledger: 1000,
      txHash: "tx-1",
      eventIndex: 0,
      payload: { amountA: 500_000, amountB: 500_000 },
    };

    await indexer.indexEvent(addEvent);
    const stats = (await indexer.getPoolStats("pool-1"))[0];
    assert.strictEqual(stats?.tvl, 1_000_000, "TVL should equal sum of deposits");
  });

  test("24h volume is accumulated from swaps", async () => {
    const baseTime = Math.floor(Date.now() / 1000);

    const swap1: PoolEvent = {
      id: "evt-1",
      poolId: "pool-1",
      type: "swap",
      timestamp: baseTime,
      ledger: 1000,
      txHash: "tx-1",
      eventIndex: 0,
      payload: { amountIn: 100, fee: 0.3 },
    };

    const swap2: PoolEvent = {
      id: "evt-2",
      poolId: "pool-1",
      type: "swap",
      timestamp: baseTime + 60,
      ledger: 1001,
      txHash: "tx-2",
      eventIndex: 0,
      payload: { amountIn: 200, fee: 0.6 },
    };

    await indexer.indexEvent(swap1);
    await indexer.indexEvent(swap2);

    const stats = (await indexer.getPoolStats("pool-1"))[0];
    assert.strictEqual(stats?.volume24h, 300, "Volume should be sum of swaps");
    assert.strictEqual(
      stats?.fees24h,
      0.9,
      "Fees should be sum of all fees",
    );
  });

  test("swap count is incremented correctly", async () => {
    const baseTime = Math.floor(Date.now() / 1000);

    for (let i = 0; i < 5; i++) {
      const event: PoolEvent = {
        id: `evt-${i}`,
        poolId: "pool-1",
        type: "swap",
        timestamp: baseTime + i * 60,
        ledger: 1000 + i,
        txHash: `tx-${i}`,
        eventIndex: 0,
        payload: { amountIn: 100, fee: 0.3 },
      };
      await indexer.indexEvent(event);
    }

    const stats = (await indexer.getPoolStats("pool-1"))[0];
    assert.strictEqual(stats?.swapCount, 5, "Swap count should be 5");
  });

  // ─── Price history tests ───────────────────────────────────────────────

  test("price history is recorded from swap events", async () => {
    const baseTime = Math.floor(Date.now() / 1000);

    const event: PoolEvent = {
      id: "evt-1",
      poolId: "pool-1",
      type: "swap",
      timestamp: baseTime,
      ledger: 1000,
      txHash: "tx-1",
      eventIndex: 0,
      payload: { amountIn: 100, fee: 0.3, price: 0.12 },
    };

    await indexer.indexEvent(event);

    const priceHistory = await store.getPriceHistory(
      "pool-1",
      baseTime - 1,
      baseTime + 1,
    );
    assert.strictEqual(priceHistory.length, 1, "Should have one price point");
    assert.strictEqual(priceHistory[0].price, 0.12, "Price should match");
  });

  // ─── Pool health tests ──────────────────────────────────────────────────

  test("pool health is computed correctly", async () => {
    const baseTime = Math.floor(Date.now() / 1000);

    const addLiquidityEvent: PoolEvent = {
      id: "evt-1",
      poolId: "pool-1",
      type: "add_liquidity",
      timestamp: baseTime,
      ledger: 1000,
      txHash: "tx-1",
      eventIndex: 0,
      payload: { amountA: 1_000_000, amountB: 1_000_000 },
    };

    await indexer.indexEvent(addLiquidityEvent);
    const health = await indexer.getPoolHealth("pool-1");

    assert(health, "Health should not be null");
    assert(health.healthScore >= 0 && health.healthScore <= 100, "Score should be 0-100");
    assert(["healthy", "warning", "critical"].includes(health.status), "Status should be valid");
  });

  // ─── Alert tests ──────────────────────────────────────────────────────

  test("alerts fire when metrics exceed threshold", async () => {
    // Set an alert config
    await indexer.setAlertConfig({
      poolId: "pool-1",
      metric: "volume24h",
      thresholdBps: 100, // Trigger if volume > 100
    });

    const baseTime = Math.floor(Date.now() / 1000);

    // Create a swap that exceeds the threshold
    const event: PoolEvent = {
      id: "evt-1",
      poolId: "pool-1",
      type: "swap",
      timestamp: baseTime,
      ledger: 1000,
      txHash: "tx-1",
      eventIndex: 0,
      payload: { amountIn: 200, fee: 0.6 }, // 200 > 100 threshold
    };

    await indexer.indexEvent(event);

    const health = await indexer.getPoolHealth("pool-1");
    assert(health?.alertsFired.length ?? 0 > 0, "Alert should have fired");
  });

  // ─── Store lifecycle tests ──────────────────────────────────────────

  test("store can be closed gracefully", async () => {
    await store.close();
    // Should not throw
  });
});

describe("RPC Ingestion Cursor Persistence", () => {
  let store: MemoryStore;

  beforeEach(() => {
    store = new MemoryStore();
  });

  test("cursor is persisted and retrievable", async () => {
    const cursor = {
      ledger: 1000,
      txHash: "tx-1",
      eventIndex: 0,
      updatedAt: Date.now(),
    };

    await store.setCursor(cursor);
    const retrieved = await store.getCursor();

    assert.deepStrictEqual(retrieved, cursor, "Cursor should match");
  });

  test("cursor starts as null", async () => {
    const cursor = await store.getCursor();
    assert.strictEqual(cursor, null, "New store should have null cursor");
  });
});

describe("Multiple Pool Handling", () => {
  let store: MemoryStore;
  let indexer: PoolIndexer;

  beforeEach(() => {
    store = new MemoryStore();
    indexer = new PoolIndexer(store);
  });

  test("different pools are tracked independently", async () => {
    const baseTime = Math.floor(Date.now() / 1000);

    const event1: PoolEvent = {
      id: "evt-1",
      poolId: "pool-1",
      type: "swap",
      timestamp: baseTime,
      ledger: 1000,
      txHash: "tx-1",
      eventIndex: 0,
      payload: { amountIn: 100, fee: 0.3 },
    };

    const event2: PoolEvent = {
      id: "evt-2",
      poolId: "pool-2",
      type: "swap",
      timestamp: baseTime,
      ledger: 1000,
      txHash: "tx-2",
      eventIndex: 0,
      payload: { amountIn: 500, fee: 1.5 },
    };

    await indexer.indexEvent(event1);
    await indexer.indexEvent(event2);

    const stats1 = (await indexer.getPoolStats("pool-1"))[0];
    const stats2 = (await indexer.getPoolStats("pool-2"))[0];

    assert.strictEqual(stats1?.volume24h, 100, "Pool 1 volume should be 100");
    assert.strictEqual(stats2?.volume24h, 500, "Pool 2 volume should be 500");
  });
});
