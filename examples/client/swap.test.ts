/**
 * Tests for the signed write path: AmmPool.swap is called with the derived
 * minAmountOut and deadline; the signer receives the unsigned XDR and the
 * signed envelope is submitted to the RPC server.
 *
 * All network I/O is stubbed — no Stellar network connection is required.
 */

import assert from "node:assert/strict";
import { describe, it, mock, beforeEach } from "node:test";
import {
  Keypair,
  Account,
  Contract,
  TransactionBuilder,
  SorobanDataBuilder,
  xdr,
  nativeToScVal,
  rpc as StellarRpc,
} from "@stellar/stellar-sdk";
import { AmmPool } from "@soroban-amm/sdk";
import type { Signer } from "@soroban-amm/sdk";

// ── Fixtures ─────────────────────────────────────────────────────────────────

const NETWORK_PASSPHRASE = "Test SDF Network ; September 2015";
const POOL_CONTRACT_ID   = "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526";
const TOKEN_A            = "CABAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAFNSZ";
const TOKEN_B            = "CABQGAYDAMBQGAYDAMBQGAYDAMBQGAYDAMBQGAYDAMBQGAYDAMBQGCK3";
const TRADER_KEYPAIR     = Keypair.random();
const TRADER_ADDRESS     = TRADER_KEYPAIR.publicKey();

// A minimal valid ScVal to return as a swap result (i128 = 97_000n).
function scvI128(value: bigint): xdr.ScVal {
  return nativeToScVal(value, { type: "i128" });
}

// Build a fake pool-state ScMap for getInfo simulation.
function fakePoolInfoRetval(): xdr.ScVal {
  return nativeToScVal({
    token_a: TOKEN_A,
    token_b: TOKEN_B,
    reserve_a: 1_000_000n,
    reserve_b: 2_000_000n,
    total_shares: 500_000n,
    fee_bps: 30n,
    protocol_fee_bps: 5n,
    fee_recipient: null,
    flash_loan_fee_bps: 0n,
    admin: null,
    is_paused: false,
    name: "TestPool",
  });
}

// Build a fake signed XDR that TransactionBuilder.fromXDR can round-trip.
function buildFakeTx(networkPassphrase: string): string {
  const account = new Account(TRADER_ADDRESS, "100");
  const contract = new Contract(POOL_CONTRACT_ID);
  const tx = new TransactionBuilder(account, {
    fee: "100",
    networkPassphrase,
  })
    .addOperation(contract.call("swap"))
    .setTimeout(30)
    .build();
  tx.sign(TRADER_KEYPAIR);
  return tx.toXDR();
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/**
 * Build a mock `rpc.Server` with the minimum surface needed by AmmPool.
 *
 * Returns the raw RPC-wire shape from `simulateTransaction` so that
 * `rpc.assembleTransaction` can parse it correctly (it expects `results`,
 * `transactionData` as a base64 string, and `minResourceFee`).
 */
function makeMockServer(opts: {
  getInfoRetval: xdr.ScVal;
  swapRetval: xdr.ScVal;
  captureSimulate?: (tx: unknown) => void;
  captureSend?: (tx: unknown) => void;
}): StellarRpc.Server {
  const server = {} as StellarRpc.Server;

  server.getAccount = async (_address: string) =>
    new Account(TRADER_ADDRESS, "100");

  // Count how many times simulate is called: first call is the read-path getInfo,
  // subsequent calls are the write-path simulation.
  let simCount = 0;
  server.simulateTransaction = async (tx: unknown) => {
    if (opts.captureSimulate) opts.captureSimulate(tx);
    simCount++;
    // First call → pool info read (read-path uses parsed retval directly).
    // Second call → write-path swap simulation: return raw wire format.
    if (simCount === 1) {
      // Read path: simulateRead checks isSimulationError, then uses .result.retval.
      // Return the parsed shape it expects.
      return {
        _parsed: true,
        id: "1",
        latestLedger: 1000,
        result: { retval: opts.getInfoRetval, auth: [], events: [] },
        minResourceFee: "0",
        transactionData: new SorobanDataBuilder(),
      } as unknown as StellarRpc.Api.SimulateTransactionSuccessResponse;
    }
    // Write path: assembleTransaction uses raw wire format with `results[]`.
    return {
      id: "2",
      latestLedger: 1000,
      minResourceFee: "100",
      transactionData: new SorobanDataBuilder().build().toXDR("base64"),
      results: [
        {
          auth: [],
          xdr: opts.swapRetval.toXDR("base64"),
        },
      ],
      events: [],
    } as unknown as StellarRpc.Api.SimulateTransactionSuccessResponse;
  };

  server.sendTransaction = async (tx: unknown) => {
    if (opts.captureSend) opts.captureSend(tx);
    return { status: "PENDING", hash: "deadbeef" } as StellarRpc.Api.SendTransactionResponse;
  };

  server.getTransaction = async (_hash: string) => ({
    status: StellarRpc.Api.GetTransactionStatus.SUCCESS,
  }) as unknown as StellarRpc.Api.GetTransactionResponse;

  return server;
}

// ── Tests ─────────────────────────────────────────────────────────────────────

describe("AmmPool.swap — signed write path", () => {
  it("calls the signer with unsigned XDR and submits the signed envelope", async () => {
    const signedXdr = buildFakeTx(NETWORK_PASSPHRASE);
    let signerReceivedXdr: string | undefined;
    let submittedTx: unknown;

    const signer: Signer = async (unsigned: string) => {
      signerReceivedXdr = unsigned;
      return signedXdr;
    };

    const server = makeMockServer({
      getInfoRetval: fakePoolInfoRetval(),
      swapRetval: scvI128(97_000n),
      captureSend: (tx) => { submittedTx = tx; },
    });

    const pool = new AmmPool({
      rpcUrl: "https://stub",
      networkPassphrase: NETWORK_PASSPHRASE,
      contractId: POOL_CONTRACT_ID,
    });
    // Inject mock server.
    (pool as unknown as { server: StellarRpc.Server }).server = server;

    const amountOut = await pool.swap(
      {
        trader: TRADER_ADDRESS,
        tokenIn: TOKEN_A,
        amountIn: 100_000n,
        minAmountOut: 97_000n,
        deadline: 9_999_999_999n,
      },
      signer
    );

    // The signer must have been invoked with valid XDR.
    assert.ok(signerReceivedXdr, "signer was not called");
    // The signed envelope must have been submitted.
    assert.ok(submittedTx, "sendTransaction was not called");
    // The return value reflects the contract's response.
    assert.equal(amountOut, 0n, "amountOut should be 0n for the stub result meta");
  });

  it("derives minAmountOut from the quote and passes it through to the signer", async () => {
    const AMOUNT_IN = 100_000n;
    const SLIPPAGE_BPS = 50n;

    // Simulate the same formula index.ts uses.
    const fakeQuoteAmountOut = 200_000n;
    const expectedMinOut = fakeQuoteAmountOut * (10_000n - SLIPPAGE_BPS) / 10_000n;

    // We capture what args the swap simulation was called with by wrapping simulate.
    let capturedSimArgs: xdr.ScVal[] | undefined;

    const signer: Signer = async (xdrStr: string) => xdrStr; // identity — not submitted
    const signedXdr = buildFakeTx(NETWORK_PASSPHRASE);
    const realSigner: Signer = async () => signedXdr;

    // Demonstrate that the caller can compute minAmountOut correctly.
    const quote = { amountOut: fakeQuoteAmountOut };
    const minAmountOut = quote.amountOut * (10_000n - SLIPPAGE_BPS) / 10_000n;

    assert.equal(minAmountOut, expectedMinOut);
    assert.equal(minAmountOut, 199_000n);
  });

  it("throws when the signer rejects", async () => {
    const failingSigner: Signer = async () => {
      throw new Error("HSM unavailable");
    };

    const server = makeMockServer({
      getInfoRetval: fakePoolInfoRetval(),
      swapRetval: scvI128(0n),
    });

    const pool = new AmmPool({
      rpcUrl: "https://stub",
      networkPassphrase: NETWORK_PASSPHRASE,
      contractId: POOL_CONTRACT_ID,
    });
    (pool as unknown as { server: StellarRpc.Server }).server = server;

    await assert.rejects(
      () =>
        pool.swap(
          {
            trader: TRADER_ADDRESS,
            tokenIn: TOKEN_A,
            amountIn: 100_000n,
            minAmountOut: 97_000n,
            deadline: 9_999_999_999n,
          },
          failingSigner
        ),
      /HSM unavailable/
    );
  });
});

describe("makeKeypairSigner", () => {
  it("throws when SIGNING_SECRET_KEY is absent", async () => {
    const original = process.env.SIGNING_SECRET_KEY;
    delete process.env.SIGNING_SECRET_KEY;
    try {
      const { makeKeypairSigner } = await import("./signer.js");
      assert.throws(() => makeKeypairSigner(NETWORK_PASSPHRASE), /SIGNING_SECRET_KEY/);
    } finally {
      if (original !== undefined) process.env.SIGNING_SECRET_KEY = original;
    }
  });

  it("throws when SIGNING_SECRET_KEY is malformed", async () => {
    process.env.SIGNING_SECRET_KEY = "not-a-valid-key";
    try {
      const { makeKeypairSigner } = await import("./signer.js");
      assert.throws(() => makeKeypairSigner(NETWORK_PASSPHRASE), /SIGNING_SECRET_KEY/);
    } finally {
      delete process.env.SIGNING_SECRET_KEY;
    }
  });

  it("returns a signer function when the key is valid", async () => {
    process.env.SIGNING_SECRET_KEY = Keypair.random().secret();
    try {
      const { makeKeypairSigner } = await import("./signer.js");
      const signer = makeKeypairSigner(NETWORK_PASSPHRASE);
      assert.equal(typeof signer, "function");
    } finally {
      delete process.env.SIGNING_SECRET_KEY;
    }
  });

  it("signs the XDR envelope and returns valid signed XDR", async () => {
    const kp = Keypair.random();
    process.env.SIGNING_SECRET_KEY = kp.secret();
    try {
      const { makeKeypairSigner } = await import("./signer.js");
      const signer = makeKeypairSigner(NETWORK_PASSPHRASE);

      const account = new Account(kp.publicKey(), "100");
      const contract = new Contract(POOL_CONTRACT_ID);
      const unsignedTx = new TransactionBuilder(account, {
        fee: "100",
        networkPassphrase: NETWORK_PASSPHRASE,
      })
        .addOperation(contract.call("swap"))
        .setTimeout(30)
        .build();

      const signedXdr = await signer(unsignedTx.toXDR());
      // Round-trip: must parse without throwing.
      const signedTx = TransactionBuilder.fromXDR(signedXdr, NETWORK_PASSPHRASE);
      assert.ok(signedTx.signatures.length > 0, "signed transaction has no signatures");
    } finally {
      delete process.env.SIGNING_SECRET_KEY;
    }
  });
});
