import { describe, expect, it } from "vitest";
import { Account, Address, Networks, StrKey, xdr } from "@stellar/stellar-sdk";
import { FactoryClient } from "./factory.js";
import { SIMULATION_SOURCE_ACCOUNT } from "./internal/simulate.js";

const poolAddress = StrKey.encodeContract(Buffer.alloc(32, 1));
const governanceAddress = StrKey.encodeContract(Buffer.alloc(32, 2));
const tokenA = StrKey.encodeContract(Buffer.alloc(32, 3));
const tokenB = StrKey.encodeContract(Buffer.alloc(32, 4));
const signer = {
  publicKey: () => SIMULATION_SOURCE_ACCOUNT,
  signTransaction: (envelope: string) => envelope,
};
const txHash = "confirmed-hash";
const ledger = 42;

function clientWithReturn(returnValue: xdr.ScVal) {
  const client = new FactoryClient({
    rpcUrl: "https://rpc.example.invalid",
    contractId: StrKey.encodeContract(Buffer.alloc(32, 5)),
    networkPassphrase: Networks.TESTNET,
  });
  // Stub the transport. The SDK still executes and decodes the confirmed XDR.
  const { server } = client as unknown as { server: Record<string, unknown> };
  server.getAccount = () => new Account(SIMULATION_SOURCE_ACCOUNT, "0");
  server.simulateTransaction = () => ({ transactionData: {}, minResourceFee: "0" });
  server.prepareTransaction = <T>(transaction: T) => transaction;
  server.sendTransaction = () => ({ status: "PENDING", hash: txHash });
  server.getTransaction = () => ({ status: "SUCCESS", ledger, returnValue });
  return client;
}

describe("FactoryClient.createPool", () => {
  it.each([null, governanceAddress])("decodes the confirmed tuple with governance %s", async (governance) => {
    const result = xdr.ScVal.scvVec([
      Address.fromString(poolAddress).toScVal(),
      governance ? Address.fromString(governance).toScVal() : xdr.ScVal.scvVoid(),
    ]);
    const client = clientWithReturn(result);

    await expect(client.createPool(SIMULATION_SOURCE_ACCOUNT, tokenA, tokenB, 0n, signer))
      .resolves.toEqual({
        hash: txHash,
        ledger,
        returnValue: { poolAddress, governanceAddress: governance },
      });
  });

  it("rejects a malformed tuple", async () => {
    const client = clientWithReturn(xdr.ScVal.scvVec([Address.fromString(poolAddress).toScVal()]));

    await expect(client.createPool(SIMULATION_SOURCE_ACCOUNT, tokenA, tokenB, 0n, signer))
      .rejects.toThrow(/CreatePoolResult/);
  });

  it("continues to return a bare address for createClPool", async () => {
    const client = clientWithReturn(Address.fromString(poolAddress).toScVal());

    await expect(client.createClPool(SIMULATION_SOURCE_ACCOUNT, tokenA, tokenB, 30n, 0, signer))
      .resolves.toEqual({ hash: txHash, ledger, returnValue: poolAddress });
  });
});
