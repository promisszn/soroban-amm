/**
 * Contract-ABI conformance tests for the SDK's parameter builders — Issue #830.
 *
 * Each test asserts the exact length, order, XDR type discriminant and decoded
 * value of every argument, so a future divergence from the deployed contract
 * signature fails here rather than at the network boundary.
 */

import { describe, it, expect } from "vitest";
import { Networks, scValToNative, xdr } from "@stellar/stellar-sdk";

import { ConcentratedLiquidityClient } from "./cl.js";
import { RouterClient } from "./router.js";
import { TokenClient } from "./token.js";

const CONTRACT_ID = "CA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5SOAOPIFY6YQGAXE";
const ALICE = "GA5WUJ54Z23KILLCUOUNAKTPBVZWKMQVO4O6EQ5GHLAERIMLLHNCSKYH";
const BOB = "GAEQSCIJBEEQSCIJBEEQSCIJBEEQSCIJBEEQSCIJBEEQSCIJBEEQSH7S";
const TOKEN_A = "CAAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQC526";
const TOKEN_B = "CABAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAEAQCAIBAFNSZ";

const config = {
  rpcUrl: "https://rpc.example.invalid",
  networkPassphrase: Networks.TESTNET,
  contractId: CONTRACT_ID,
};

const cl = new ConcentratedLiquidityClient(config);
const router = new RouterClient(config);
const token = new TokenClient(config);

/** The XDR type discriminant name of an ScVal, e.g. "scvI128". */
function typeOf(v: xdr.ScVal): string {
  return v.switch().name;
}

/** Assert the ordered list of XDR type discriminants of an argument array. */
function expectTypes(args: xdr.ScVal[], types: string[]) {
  expect(args).toHaveLength(types.length);
  expect(args.map(typeOf)).toEqual(types);
}

describe("cl.swapParams", () => {
  // contracts/concentrated_liquidity/src/lib.rs:1437
  // (sender, zero_for_one: bool, amount_in: i128,
  //  sqrt_price_limit_x96: u128, min_amount_out: i128, deadline: u64)
  const args = cl.swapParams(ALICE, true, 1_000n, 79_228_162_514_264_337_593_543_950_336n, 950n, 1_700_000_000n);

  it("matches the contract's arity, order and types", () => {
    expectTypes(args, ["scvAddress", "scvBool", "scvI128", "scvU128", "scvI128", "scvU64"]);
  });

  it("encodes each value at the right position", () => {
    expect(scValToNative(args[0])).toBe(ALICE);
    expect(scValToNative(args[1])).toBe(true);
    expect(scValToNative(args[2])).toBe(1_000n);
    expect(scValToNative(args[3])).toBe(79_228_162_514_264_337_593_543_950_336n);
    expect(scValToNative(args[4])).toBe(950n);
    expect(scValToNative(args[5])).toBe(1_700_000_000n);
  });

  it("passes direction as a bool, never as a token address", () => {
    expect(typeOf(args[1])).toBe("scvBool");
    expect(scValToNative(cl.swapParams(ALICE, false, 1n, 0n, 0n, 0n)[1])).toBe(false);
  });

  it("encodes sqrt_price_limit_x96 as u128, not i128", () => {
    expect(typeOf(args[3])).toBe("scvU128");
  });
});

describe("cl.mintPositionParams", () => {
  // contracts/concentrated_liquidity/src/lib.rs:407
  // (provider, lower_tick: i32, upper_tick: i32,
  //  amount_a_desired: i128, amount_b_desired: i128, min_a: i128, min_b: i128)
  const args = cl.mintPositionParams(ALICE, -100, 100, 5_000n, 6_000n, 4_500n, 5_400n);

  it("matches the contract's arity, order and types", () => {
    expectTypes(args, [
      "scvAddress",
      "scvI32",
      "scvI32",
      "scvI128",
      "scvI128",
      "scvI128",
      "scvI128",
    ]);
  });

  it("encodes each value at the right position", () => {
    expect(scValToNative(args[0])).toBe(ALICE);
    expect(scValToNative(args[1])).toBe(-100);
    expect(scValToNative(args[2])).toBe(100);
    expect(scValToNative(args[3])).toBe(5_000n);
    expect(scValToNative(args[4])).toBe(6_000n);
    expect(scValToNative(args[5])).toBe(4_500n);
    expect(scValToNative(args[6])).toBe(5_400n);
  });

  it("emits no deadline argument", () => {
    // `mint_position` has no deadline guard; a trailing u64 would be an arity error.
    expect(args.map(typeOf)).not.toContain("scvU64");
  });
});

describe("cl.burnPositionParams", () => {
  // contracts/concentrated_liquidity/src/lib.rs:1118
  // (provider, lower_tick: i32, upper_tick: i32, liquidity: i128)
  const args = cl.burnPositionParams(ALICE, -60, 60, 2_500n);

  it("matches the contract's arity, order and types", () => {
    expectTypes(args, ["scvAddress", "scvI32", "scvI32", "scvI128"]);
  });

  it("includes the liquidity amount to burn as the 4th argument", () => {
    expect(scValToNative(args[0])).toBe(ALICE);
    expect(scValToNative(args[1])).toBe(-60);
    expect(scValToNative(args[2])).toBe(60);
    expect(scValToNative(args[3])).toBe(2_500n);
  });
});

describe("router.swapExactInParams", () => {
  // contracts/router/src/lib.rs:34
  // (trader, path: Vec<Address>, amount_in: i128, min_amount_out: i128, deadline: u64)
  const path = [TOKEN_A, TOKEN_B];
  const args = router.swapExactInParams({
    trader: ALICE,
    path,
    amountIn: 1_000n,
    minAmountOut: 900n,
    deadline: 1_700_000_000n,
  });

  it("matches the contract's arity, order and types", () => {
    expectTypes(args, ["scvAddress", "scvVec", "scvI128", "scvI128", "scvU64"]);
  });

  it("passes the trader as the first argument", () => {
    expect(scValToNative(args[0])).toBe(ALICE);
  });

  it("encodes the path as a Vec of addresses", () => {
    expect(scValToNative(args[1])).toEqual(path);
  });

  it("encodes the remaining bounds and deadline", () => {
    expect(scValToNative(args[2])).toBe(1_000n);
    expect(scValToNative(args[3])).toBe(900n);
    expect(scValToNative(args[4])).toBe(1_700_000_000n);
  });
});

describe("router.swapExactOutParams", () => {
  // contracts/router/src/lib.rs:84
  // (trader, path: Vec<Address>, amount_out: i128, max_in: i128, deadline: u64)
  const path = [TOKEN_A, TOKEN_B];
  const args = router.swapExactOutParams({
    trader: BOB,
    path,
    amountOut: 500n,
    maxIn: 600n,
    deadline: 1_700_000_000n,
  });

  it("matches the contract's arity, order and types", () => {
    expectTypes(args, ["scvAddress", "scvVec", "scvI128", "scvI128", "scvU64"]);
  });

  it("encodes each value at the right position", () => {
    expect(scValToNative(args[0])).toBe(BOB);
    expect(scValToNative(args[1])).toEqual(path);
    expect(scValToNative(args[2])).toBe(500n);
    expect(scValToNative(args[3])).toBe(600n);
    expect(scValToNative(args[4])).toBe(1_700_000_000n);
  });

  it("supports multi-hop paths", () => {
    const threeHop = [TOKEN_A, TOKEN_B, TOKEN_A];
    const multi = router.swapExactOutParams({
      trader: BOB,
      path: threeHop,
      amountOut: 1n,
      maxIn: 2n,
      deadline: 3n,
    });
    expect(scValToNative(multi[1])).toEqual(threeHop);
  });
});

describe("router.getAmountOutPath argument order", () => {
  // contracts/router/src/lib.rs:155 — (path: Vec<Address>, amount_in: i128)
  it("sends the path first and the amount second", async () => {
    const client = new RouterClient(config);
    const server = (client as unknown as { server: Record<string, unknown> }).server;

    let invokeArgs: xdr.ScVal[] = [];
    server.simulateTransaction = async (tx: {
      operations: Array<{ func: xdr.HostFunction }>;
    }) => {
      invokeArgs = tx.operations[0].func.invokeContract().args();
      return { result: { retval: xdr.ScVal.scvI128(new xdr.Int128Parts({ hi: xdr.Int64.fromString("0"), lo: xdr.Uint64.fromString("7") })) } };
    };

    await client.getAmountOutPath([TOKEN_A, TOKEN_B], 1_000n);

    expectTypes(invokeArgs, ["scvVec", "scvI128"]);
    expect(scValToNative(invokeArgs[0])).toEqual([TOKEN_A, TOKEN_B]);
    expect(scValToNative(invokeArgs[1])).toBe(1_000n);
  });
});

describe("token.approveParams", () => {
  // contracts/token/src/lib.rs:156 — (from, spender, amount: i128)
  const args = token.approveParams(ALICE, BOB, 1_000n);

  it("matches the contract's arity, order and types", () => {
    expectTypes(args, ["scvAddress", "scvAddress", "scvI128"]);
  });

  it("encodes each value at the right position", () => {
    expect(scValToNative(args[0])).toBe(ALICE);
    expect(scValToNative(args[1])).toBe(BOB);
    expect(scValToNative(args[2])).toBe(1_000n);
  });

  it("emits no live_until_ledger argument", () => {
    // This contract's `approve` stores allowances without an expiry ledger, so
    // it takes 3 arguments — a trailing u32 would be rejected for arity.
    expect(args.map(typeOf)).not.toContain("scvU32");
  });
});
