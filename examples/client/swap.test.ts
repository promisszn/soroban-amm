import assert from "node:assert/strict";
import { it } from "node:test";
import {
  Account,
  Contract,
  Keypair,
  Transaction,
  TransactionBuilder,
  SorobanDataBuilder,
  nativeToScVal,
  xdr,
} from "@stellar/stellar-sdk";
import { AmmPool } from "@soroban-amm/sdk";
import { makeKeypairSigner } from "./signer.js";

const networkPassphrase = "Test SDF Network ; September 2015";
const contractId = "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526";
const tokenIn = "CABAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAFNSZ";
const keypair = Keypair.random();
const trader = keypair.publicKey();
const deadline = 9_999_999_999n;

function loadSigner(secret: string | undefined) {
  const original = process.env.SIGNING_SECRET_KEY;
  try {
    if (secret === undefined) delete process.env.SIGNING_SECRET_KEY;
    else process.env.SIGNING_SECRET_KEY = secret;
    return makeKeypairSigner();
  } finally {
    if (original === undefined) delete process.env.SIGNING_SECRET_KEY;
    else process.env.SIGNING_SECRET_KEY = original;
  }
}

it("rejects missing and malformed example signing keys", () => {
  assert.throws(() => loadSigner(undefined), /SIGNING_SECRET_KEY is required/);
  assert.throws(() => loadSigner("invalid"), /not a valid Stellar secret key/);
});

it("signs the envelope for the requested network", async () => {
  const signer = loadSigner(keypair.secret());
  assert.equal(await signer.publicKey(), trader);
  const unsigned = new TransactionBuilder(new Account(trader, "100"), {
    fee: "100", networkPassphrase,
  }).addOperation(new Contract(contractId).call("swap")).setTimeout(30).build();
  const signed = TransactionBuilder.fromXDR(
    await signer.signTransaction(unsigned.toXDR(), networkPassphrase), networkPassphrase,
  );
  assert.equal(signed.signatures.length, 1);
  assert.ok(keypair.verify(signed.hash(), signed.signatures[0].signature));
});

it("signs and submits swap, add and remove once and decodes their ABI results", async () => {
  const submitted: Transaction[] = [];
  const retvals = [
    nativeToScVal(97_000n, { type: "i128" }),
    nativeToScVal(250n, { type: "i128" }),
    xdr.ScVal.scvVec([nativeToScVal(63n, { type: "i128" }), nativeToScVal(101n, { type: "i128" })]),
  ];
  let sequence = 100;
  const server = {
    getAccount: async (address: string) => {
      assert.equal(address, trader);
      return new Account(address, String(sequence++));
    },
    simulateTransaction: async () => ({
      _parsed: true, latestLedger: 99, minResourceFee: "0", transactionData: new SorobanDataBuilder(),
      result: { retval: retvals[submitted.length], auth: [] },
    }),
    prepareTransaction: async (tx: Transaction) => tx,
    sendTransaction: async (tx: Transaction) => {
      const parsed = TransactionBuilder.fromXDR(tx.toXDR(), networkPassphrase);
      assert.ok(parsed instanceof Transaction);
      assert.equal(parsed.source, trader);
      assert.equal(parsed.signatures.length, 1);
      assert.ok(keypair.verify(parsed.hash(), parsed.signatures[0].signature));
      submitted.push(parsed);
      return { status: "PENDING", hash: String(submitted.length) };
    },
    getTransaction: async (hash: string) => ({
      status: "SUCCESS", ledger: 100, returnValue: retvals[Number(hash) - 1],
    }),
  };
  const pool = new AmmPool({ rpcUrl: "https://stub", networkPassphrase, contractId });
  (pool as unknown as { server: typeof server }).server = server;
  const signer = loadSigner(keypair.secret());

  assert.deepEqual(await pool.swap(
    { trader, tokenIn, amountIn: 100_000n, minAmountOut: 97_000n, deadline }, signer,
  ), { hash: "1", ledger: 100, returnValue: 97_000n });
  assert.deepEqual(await pool.addLiquidity(
    { provider: trader, amountA: 100n, amountB: 200n, minShares: 1n, deadline }, signer,
  ), { hash: "2", ledger: 100, returnValue: 250n });
  assert.deepEqual(await pool.removeLiquidity(
    { provider: trader, shares: 200n, minAmountA: 1n, minAmountB: 1n, deadline }, signer,
  ), { hash: "3", ledger: 100, returnValue: [63n, 101n] });

  const args = submitted.map(tx => {
    const op = tx.operations[0];
    if (op.type !== "invokeHostFunction" || op.func.type !== "hostFunctionTypeInvokeContract") throw new Error("expected contract invocation");
    return op.func.invokeContract;
  });
  assert.deepEqual(args.map(arg => String(arg.functionName)), ["swap", "add_liquidity", "remove_liquidity"]);
  assert.equal(args[0].args[3].toXDR("base64"), nativeToScVal(97_000n, { type: "i128" }).toXDR("base64"));
  assert.equal(args[0].args[4].toXDR("base64"), nativeToScVal(deadline, { type: "u64" }).toXDR("base64"));
});
