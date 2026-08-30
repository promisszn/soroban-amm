// ── Regression tests for per-contract cursor tracking (issue: shared cursor drops events) ──

import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { HorizonPoller } from "./horizon-poller.js";
import type { HorizonEvent, PoolEvent } from "./types.js";

function makeRawEvent(contractId: string, pagingToken: string): HorizonEvent {
  return {
    id: `${contractId}-${pagingToken}`,
    type: "contract",
    ledger: Number(pagingToken),
    ledgerClosedAt: new Date().toISOString(),
    contractId,
    topic: ["swap"],
    value: JSON.stringify({}),
    pagingToken,
  };
}

/**
 * Mock Horizon /events endpoint: for each contract, holds a single event
 * whose pagingToken is returned only when the request's cursor is strictly
 * less than it — mirroring Horizon's "cursor is a global ledger position"
 * pagination semantics.
 */
async function startHorizonMock(
  eventsByContract: Record<string, HorizonEvent>,
): Promise<{ url: string; close: () => Promise<void> }> {
  const server: Server = createServer((req, res) => {
    const reqUrl = new URL(req.url ?? "/", "http://localhost");
    const contractId = reqUrl.searchParams.get("contract_id") ?? "";
    const cursor = Number(reqUrl.searchParams.get("cursor") ?? "0");
    const event = eventsByContract[contractId];

    const records =
      event && Number(event.pagingToken) > cursor ? [event] : [];

    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({ _embedded: { records } }));
  });

  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address() as AddressInfo;

  return {
    url: `http://127.0.0.1:${port}`,
    close: () =>
      new Promise<void>((resolve) => {
        server.closeAllConnections?.();
        server.close(() => resolve());
      }),
  };
}

describe("HorizonPoller cursors", () => {
  it("tracks independent cursors per contract, exposed via cursors()", async () => {
    const poller = new HorizonPoller(
      { horizonUrl: "http://example.invalid", contractIds: ["A", "B"], startCursor: "0" },
      async () => {},
    );
    const cursors = poller.cursors();
    assert.equal(cursors.get("A"), "0");
    assert.equal(cursors.get("B"), "0");
  });

  it("does not skip a second contract's older-paging-token event after the first contract advances (regression)", async () => {
    const mock = await startHorizonMock({
      A: makeRawEvent("A", "100"),
      B: makeRawEvent("B", "50"),
    });
    try {
      const delivered: PoolEvent[] = [];
      const poller = new HorizonPoller(
        {
          horizonUrl: mock.url,
          contractIds: ["A", "B"],
          startCursor: "0",
        },
        async (event) => {
          delivered.push(event);
        },
      );

      // Access the private polling method directly to run exactly one cycle.
      await (poller as unknown as { _poll(): Promise<void> })._poll();

      const contractIds = delivered.map((e) => e.contractId).sort();
      assert.deepEqual(
        contractIds,
        ["A", "B"],
        "contract B's event (paging token 50) must not be dropped just because contract A advanced past it",
      );

      const cursors = poller.cursors();
      assert.equal(cursors.get("A"), "100");
      assert.equal(cursors.get("B"), "50");
    } finally {
      await mock.close();
    }
  });

  it("advancing one contract's cursor does not change another contract's stored cursor", async () => {
    const mock = await startHorizonMock({
      A: makeRawEvent("A", "100"),
      // B has no event yet, so its cursor should remain untouched.
    });
    try {
      const poller = new HorizonPoller(
        { horizonUrl: mock.url, contractIds: ["A", "B"], startCursor: "0" },
        async () => {},
      );

      await (poller as unknown as { _poll(): Promise<void> })._poll();

      const cursors = poller.cursors();
      assert.equal(cursors.get("A"), "100");
      assert.equal(cursors.get("B"), "0");
    } finally {
      await mock.close();
    }
  });

  it("still pages forward correctly for a single-contract poller (common case unchanged)", async () => {
    const mock = await startHorizonMock({ A: makeRawEvent("A", "42") });
    try {
      const delivered: PoolEvent[] = [];
      const poller = new HorizonPoller(
        { horizonUrl: mock.url, contractIds: ["A"], startCursor: "0" },
        async (event) => {
          delivered.push(event);
        },
      );

      await (poller as unknown as { _poll(): Promise<void> })._poll();
      assert.equal(delivered.length, 1);
      assert.equal(poller.cursors().get("A"), "42");

      // A second poll with no new events should not redeliver.
      await (poller as unknown as { _poll(): Promise<void> })._poll();
      assert.equal(delivered.length, 1);
    } finally {
      await mock.close();
    }
  });
});
