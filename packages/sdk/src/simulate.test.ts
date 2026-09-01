/**
 * Tests for the shared read-only simulation helper — Issue #831.
 *
 * Covers the two defects the helper was introduced to fix:
 *   1. the simulation source account must be a valid 56-character strkey, and
 *   2. no client may fetch it over the network via `server.getAccount(...)`.
 */

import { describe, it, expect, vi } from "vitest";
import { Networks, StrKey, xdr } from "@stellar/stellar-sdk";

import { AmmPool } from "./AmmPool.js";
import { TokenClient } from "./token.js";
import { FactoryClient } from "./factory.js";
import { GovernanceClient } from "./governance.js";
import { ConcentratedLiquidityClient } from "./cl.js";
import { RouterClient } from "./router.js";
import { SIMULATION_SOURCE_ACCOUNT } from "./internal/simulate.js";

const CONTRACT_ID = "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5SOAOPIFY6YQGAXE";

const config = {
  rpcUrl: "https://rpc.example.invalid",
  networkPassphrase: Networks.TESTNET,
  contractId: CONTRACT_ID,
};

/** A successful simulation response returning the u32 value 1. */
function successResponse() {
  return {
    result: { retval: xdr.ScVal.scvU32(1) },
    // `isSimulationError` checks for an `error` field; omitting it means success.
  };
}

describe("SIMULATION_SOURCE_ACCOUNT", () => {
  it("is a syntactically valid 56-character Stellar strkey", () => {
    expect(SIMULATION_SOURCE_ACCOUNT).toHaveLength(56);
    expect(StrKey.isValidEd25519PublicKey(SIMULATION_SOURCE_ACCOUNT)).toBe(true);
  });

  it("is not the old 55-character malformed constant", () => {
    const malformed = "GAAZI4TCR3TY5OJHCTJC2A4QSY6CJWJH5IAJTGKIN2ER7LBNVKOCCWN";
    expect(malformed).toHaveLength(55);
    expect(StrKey.isValidEd25519PublicKey(malformed)).toBe(false);
    expect(SIMULATION_SOURCE_ACCOUNT).not.toBe(malformed);
  });
});

describe("read-only clients never fetch a dummy source account", () => {
  const clients: Array<[string, () => { server: unknown }, (c: never) => Promise<unknown>]> = [
    ["AmmPool", () => new AmmPool(config) as never, (c: never) => (c as AmmPool).getName()],
    ["TokenClient", () => new TokenClient(config) as never, (c: never) => (c as TokenClient).decimals()],
    ["FactoryClient", () => new FactoryClient(config) as never, (c: never) => (c as FactoryClient).poolCount()],
    ["GovernanceClient", () => new GovernanceClient(config) as never, (c: never) => (c as GovernanceClient).proposalCount()],
    [
      "ConcentratedLiquidityClient",
      () => new ConcentratedLiquidityClient(config) as never,
      (c: never) => (c as ConcentratedLiquidityClient).currentTick(),
    ],
    ["RouterClient", () => new RouterClient(config) as never, (c: never) => (c as RouterClient).getFactory()],
  ];

  for (const [name, make, call] of clients) {
    it(`${name}.simulate() calls simulateTransaction without getAccount`, async () => {
      const client = make() as never;
      // Reach into the private `server` the client built in its constructor.
      const server = (client as unknown as { server: Record<string, unknown> }).server;

      const getAccount = vi.fn(async () => {
        throw new Error("getAccount must not be called during read-only simulation");
      });
      const simulateTransaction = vi.fn(async () => successResponse());
      server.getAccount = getAccount;
      server.simulateTransaction = simulateTransaction;

      await call(client);

      expect(getAccount).not.toHaveBeenCalled();
      expect(simulateTransaction).toHaveBeenCalledTimes(1);
    });
  }

  it("builds the envelope against the valid local dummy account", async () => {
    const client = new TokenClient(config);
    const server = (client as unknown as { server: Record<string, unknown> }).server;

    let sourceAccount = "";
    server.getAccount = vi.fn(async () => {
      throw new Error("getAccount must not be called");
    });
    server.simulateTransaction = vi.fn(async (tx: { source: string }) => {
      sourceAccount = tx.source;
      return successResponse();
    });

    await client.decimals();

    expect(sourceAccount).toBe(SIMULATION_SOURCE_ACCOUNT);
  });
});
