import test from "node:test";
import assert from "node:assert";
import { RpcIngester } from "./ingest/rpc.js";
import { xdr, scValToNative } from "@stellar/stellar-sdk";

// Helper to encode sample XDR fixtures for tests
function encodeFixture(topicSymbol: string, schemaVersion: number, payload: unknown, traderOrProvider?: string): { topic: string[]; value: string } {
  const topics: string[] = [];
  const symVal = xdr.scValToNative(xdr.ScVal.scvSymbol(topicSymbol)) ? xdr.ScVal.scvSymbol(topicSymbol) : xdr.ScVal.scvSymbol(topicSymbol);
  topics.push(symVal.toXDR("base64"));
  if (traderOrProvider) {
    // arbitrary address scVal for topic[1] or similar
    const addr = xdr.ScVal.scvAddress(xdr.ScAddress.scAddressTypeAccountId(xdr.PublicKey.fromAccountId("GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF")));
    topics.push(addr.toXDR("base64"));
  } else {
    // schema version in topic[1] or data tuple
    const sv = xdr.ScVal.scvU32(schemaVersion);
    topics.push(sv.toXDR("base64"));
  }

  // Versioned envelope data: (schemaVersion, payload)
  const versionScVal = xdr.ScVal.scvU32(schemaVersion);
  let payloadScVal: xdr.ScVal;
  if (Array.isArray(payload)) {
    const vec = payload.map((item) => {
      if (typeof item === "number") return xdr.ScVal.scvI64(xdr.UnsignedHyper.fromString(String(item)));
      if (typeof item === "string") return xdr.ScVal.scvString(item);
      return xdr.ScVal.scvVoid();
    });
    payloadScVal = xdr.ScVal.scvVec(vec);
  } else {
    payloadScVal = xdr.ScVal.scvVoid();
  }

  const envelope = xdr.ScVal.scvVec([versionScVal, payloadScVal]);
  return {
    topic: topics,
    value: envelope.toXDR("base64"),
  };
}

test("1. With START_LEDGER unset, the first getEvents request contains a positive startLedger", async () => {
  let capturedBody: any = null;
  const originalFetch = global.fetch;

  (global as any).fetch = async (url: string, init: any) => {
    const body = JSON.parse(init.body);
    capturedBody = body;
    if (body.method === "getLatestLedger") {
      return {
        ok: true,
        json: async () => ({ jsonrpc: "2.0", id: "1", result: { sequence: 12345, oldestLedger: 2345 } }),
      };
    }
    return {
      ok: true,
      json: async () => ({
        jsonrpc: "2.0",
        id: "1",
        result: { events: [], latestLedger: 12345 },
      }),
    };
  };

  try {
    const ingester = new RpcIngester(
      { rpcUrl: "https://rpc.test", contractIds: ["C_POOL"] },
      async () => {},
      async () => {},
    );

    // Trigger poll manually by starting and stopping quickly or calling private method
    await (ingester as any)._poll();

    assert.ok(capturedBody, "Fetch should have been called");
    assert.strictEqual(capturedBody.method, "getEvents");
    assert.ok(capturedBody.params.startLedger > 0, `startLedger should be positive, got ${capturedBody.params.startLedger}`);
  } finally {
    global.fetch = originalFetch;
  }
});

test("2. A real swap event fixture (AMM) decodes to a PoolEvent of type swap with correct amounts and schema version", async () => {
  const fixture = encodeFixture("swap", 1, ["GC_IN", 1000, "GC_OUT", 950, null], "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF");
  const rawEvent = {
    type: "contract",
    ledger: 100,
    ledgerClosedAt: new Date().toISOString(),
    contractId: "C_POOL",
    id: "evt-1",
    pagingToken: "100-0",
    topic: fixture.topic,
    value: fixture.value,
    inSuccessfulContractInvocation: true,
    txHash: "tx-abc",
  };

  let decodedEvent: any = null;
  const ingester = new RpcIngester(
    { rpcUrl: "https://rpc.test", contractIds: ["C_POOL"] },
    async (ev) => { decodedEvent = ev; },
    async () => {},
  );

  const result = (ingester as any)._decodeEvent(rawEvent, "C_POOL");
  assert.notStrictEqual(result, null);
  assert.strictEqual(result.type, "swap");
  assert.strictEqual(result.payload.amountIn, 1000);
  assert.strictEqual(result.payload.amountOut, 950);
  assert.strictEqual(result.txHash, "tx-abc");
});

test("3. Fixtures for add_liquidity and remove_liquidity decode correctly", async () => {
  const addFixture = encodeFixture("add_liquidity", 1, [5000, 5000, 1000], "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF");
  const rawAdd = {
    type: "contract",
    ledger: 101,
    ledgerClosedAt: new Date().toISOString(),
    contractId: "C_POOL",
    id: "evt-add",
    pagingToken: "101-0",
    topic: addFixture.topic,
    value: addFixture.value,
    inSuccessfulContractInvocation: true,
  };

  const ingester = new RpcIngester(
    { rpcUrl: "https://rpc.test", contractIds: ["C_POOL"] },
    async () => {},
    async () => {},
  );

  const decodedAdd = (ingester as any)._decodeEvent(rawAdd, "C_POOL");
  assert.notStrictEqual(decodedAdd, null);
  assert.strictEqual(decodedAdd.type, "add_liquidity");
  assert.strictEqual(decodedAdd.payload.amountA, 5000);
  assert.strictEqual(decodedAdd.payload.amountB, 5000);

  const rmFixture = encodeFixture("remove_liquidity", 1, ["G_PROV", 500, 2500, 2500]);
  const rawRm = {
    type: "contract",
    ledger: 102,
    ledgerClosedAt: new Date().toISOString(),
    contractId: "C_POOL",
    id: "evt-rm",
    pagingToken: "102-0",
    topic: rmFixture.topic,
    value: rmFixture.value,
    inSuccessfulContractInvocation: true,
  };

  const decodedRm = (ingester as any)._decodeEvent(rawRm, "C_POOL");
  assert.notStrictEqual(decodedRm, null);
  assert.strictEqual(decodedRm.type, "remove_liquidity");
  assert.strictEqual(decodedRm.payload.amountA, 2500);
});

test("4. An event whose data version is higher than supported is rejected. Version equal to supported is accepted", async () => {
  const newerFixture = encodeFixture("swap", 2, [100, 200]);
  const rawNewer = {
    type: "contract",
    ledger: 103,
    ledgerClosedAt: new Date().toISOString(),
    contractId: "C_POOL",
    id: "evt-new",
    pagingToken: "103-0",
    topic: newerFixture.topic,
    value: newerFixture.value,
    inSuccessfulContractInvocation: true,
  };

  const ingester = new RpcIngester(
    { rpcUrl: "https://rpc.test", contractIds: ["C_POOL"] },
    async () => {},
    async () => {},
  );

  const resNewer = (ingester as any)._decodeEvent(rawNewer, "C_POOL");
  assert.strictEqual(resNewer, null, "Newer schema version should be rejected");

  const validFixture = encodeFixture("swap", 1, [100, 200]);
  const rawValid = {
    type: "contract",
    ledger: 104,
    ledgerClosedAt: new Date().toISOString(),
    contractId: "C_POOL",
    id: "evt-valid",
    pagingToken: "104-0",
    topic: validFixture.topic,
    value: validFixture.value,
    inSuccessfulContractInvocation: true,
  };

  const resValid = (ingester as any)._decodeEvent(rawValid, "C_POOL");
  assert.notStrictEqual(resValid, null, "Supported schema version should be accepted");
});

test("5. Two contracts polled in one cycle each advance their own cursor", async () => {
  const requests: any[] = [];
  const originalFetch = global.fetch;

  (global as any).fetch = async (url: string, init: any) => {
    const body = JSON.parse(init.body);
    requests.push(body);
    const contractId = body.params.filters[0].contractIds[0];
    const cursor = body.params.pagination.cursor;

    let events: any[] = [];
    if (!cursor) {
      const fixture = encodeFixture("swap", 1, ["A", 10, "B", 10]);
      events = [
        {
          type: "contract",
          ledger: 200,
          ledgerClosedAt: new Date().toISOString(),
          contractId,
          id: `evt-${contractId}-1`,
          pagingToken: `${contractId}-token-1`,
          topic: fixture.topic,
          value: fixture.value,
          inSuccessfulContractInvocation: true,
        },
      ];
    }
    return {
      ok: true,
      json: async () => ({
        jsonrpc: "2.0",
        id: "1",
        result: { events, latestLedger: 250 },
      }),
    };
  };

  try {
    const ingester = new RpcIngester(
      { rpcUrl: "https://rpc.test", contractIds: ["POOL_1", "POOL_2"], startLedger: 100 },
      async () => {},
      async () => {},
    );

    await (ingester as any)._poll();
    await (ingester as any)._poll(); // second poll should send respective cursors

    const cursors = (ingester as any).contractCursors;
    assert.strictEqual(cursors.get("POOL_1"), "POOL_1-token-1");
    assert.strictEqual(cursors.get("POOL_2"), "POOL_2-token-1");
  } finally {
    global.fetch = originalFetch;
  }
});

test("6. Replaying the same page twice produces no duplicate store entries (idempotent MemoryStore)", async () => {
  const { MemoryStore } = await import("./store/memory.js");
  const store = new MemoryStore();

  const event = {
    id: "evt-dup",
    poolId: "POOL_1",
    type: "swap" as const,
    timestamp: 123456,
    ledger: 100,
    txHash: "tx-1",
    eventIndex: 0,
    payload: { amountIn: 100 },
  };

  await store.appendEvent(event);
  await store.appendEvent(event); // replay

  const recent = await store.getRecentEvents(10);
  assert.strictEqual(recent.length, 1);
});

test("7. Retention check detects ledger outside window", async () => {
  let errorCaught: Error | null = null;
  const originalFetch = global.fetch;

  (global as any).fetch = async () => {
    return {
      ok: true,
      json: async () => ({
        jsonrpc: "2.0",
        id: "1",
        result: {
          events: [],
          latestLedger: 50000,
          oldestLedger: 40000,
        },
      }),
    };
  };

  try {
    const ingester = new RpcIngester(
      { rpcUrl: "https://rpc.test", contractIds: ["POOL_1"], startLedger: 1000 },
      async () => {},
      async (err) => { errorCaught = err; },
    );

    await (ingester as any)._poll();
    assert.notStrictEqual(errorCaught, null);
    assert.ok(errorCaught!.message.includes("retention window"));
  } finally {
    global.fetch = originalFetch;
  }
});

test("8. Malformed event topics or XDR gracefully handled", async () => {
  const ingester = new RpcIngester(
    { rpcUrl: "https://rpc.test", contractIds: ["POOL_1"] },
    async () => {},
    async () => {},
  );

  const rawBad = {
    type: "contract",
    ledger: 100,
    ledgerClosedAt: new Date().toISOString(),
    contractId: "POOL_1",
    id: "bad-evt",
    pagingToken: "100-0",
    topic: ["invalid-base85-or-xdr"],
    value: "invalid-value",
    inSuccessfulContractInvocation: true,
  };

  const res = (ingester as any)._decodeEvent(rawBad, "POOL_1");
  assert.strictEqual(res, null);
});
