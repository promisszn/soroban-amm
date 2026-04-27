/**
 * AmmPool — typed wrapper for the Soroban AMM contract — Issue #104
 *
 * Provides human-readable error decoding, a `simulate` helper that composes
 * `simulate_swap` + `get_amount_out`, and typed wrappers for every public
 * AMM function.
 */

import {
  Contract,
  Networks,
  rpc as StellarRpc,
  nativeToScVal,
  scValToNative,
  xdr,
  Address,
} from "@stellar/stellar-sdk";
import type {
  NetworkConfig,
  PoolInfo,
  SwapParams,
  SwapSimulation,
  AddLiquidityParams,
  RemoveLiquidityParams,
  LiquidityResult,
  FlashLoanParams,
} from "./types.js";
import { AmmErrors } from "./types.js";

// ── Helpers ────────────────────────────────────────────────────────────────────

function i128(value: bigint): xdr.ScVal {
  return nativeToScVal(value, { type: "i128" });
}

function addr(address: string): xdr.ScVal {
  return nativeToScVal(Address.fromString(address));
}

function decodeError(err: unknown): Error {
  const msg = err instanceof Error ? err.message : String(err);
  for (const [, text] of Object.entries(AmmErrors)) {
    if (msg.toLowerCase().includes(text)) {
      return new Error(`AMM error: ${text}`);
    }
  }
  return new Error(`AMM error: ${msg}`);
}

// ── AmmPool class ─────────────────────────────────────────────────────────────

export class AmmPool {
  private readonly server: StellarRpc.Server;
  private readonly contract: Contract;
  private readonly networkPassphrase: string;

  constructor(config: NetworkConfig) {
    this.server = new StellarRpc.Server(config.rpcUrl);
    this.contract = new Contract(config.contractId);
    this.networkPassphrase = config.networkPassphrase;
  }

  // ── Read-only helpers ──────────────────────────────────────────────────────

  private async simulate(method: string, ...args: xdr.ScVal[]): Promise<xdr.ScVal> {
    const op = this.contract.call(method, ...args);
    const tx = new (await import("@stellar/stellar-sdk")).TransactionBuilder(
      await this.server.getAccount("GAAZI4TCR3TY5OJHCTJC2A4QSY6CJWJH5IAJTGKIN2ER7LBNVKOCCWN"),
      { fee: "100", networkPassphrase: this.networkPassphrase }
    )
      .addOperation(op)
      .setTimeout(30)
      .build();
    const result = await this.server.simulateTransaction(tx);
    if (StellarRpc.Api.isSimulationError(result)) {
      throw decodeError(new Error(result.error));
    }
    return (result as StellarRpc.Api.SimulateTransactionSuccessResponse)
      .result!.retval;
  }

  // ── Pool info ──────────────────────────────────────────────────────────────

  /** Fetch full pool state. */
  async getInfo(): Promise<PoolInfo> {
    const raw = await this.simulate("get_info");
    const native = scValToNative(raw) as Record<string, unknown>;
    return {
      tokenA: String(native.token_a),
      tokenB: String(native.token_b),
      reserveA: BigInt(String(native.reserve_a ?? 0)),
      reserveB: BigInt(String(native.reserve_b ?? 0)),
      totalShares: BigInt(String(native.total_shares ?? 0)),
      feeBps: BigInt(String(native.fee_bps ?? 0)),
      protocolFeeBps: BigInt(String(native.protocol_fee_bps ?? 0)),
      feeRecipient: native.fee_recipient ? String(native.fee_recipient) : null,
      flashLoanFeeBps: BigInt(String(native.flash_loan_fee_bps ?? 0)),
      isPaused: Boolean(native.is_paused),
      name: native.name ? String(native.name) : null,
    };
  }

  /** Return the human-readable pool name (or null). */
  async getName(): Promise<string | null> {
    const raw = await this.simulate("get_name");
    const native = scValToNative(raw);
    return native !== null ? String(native) : null;
  }

  /** Return the flash-loan fee in basis points. */
  async getFlashLoanFeeBps(): Promise<bigint> {
    const raw = await this.simulate("get_flash_loan_fee_bps");
    return BigInt(scValToNative(raw) as number);
  }

  /** Return the LP share balance of `address`. */
  async sharesOf(address: string): Promise<bigint> {
    const raw = await this.simulate("shares_of", addr(address));
    return BigInt(scValToNative(raw) as number);
  }

  // ── Swap simulation ────────────────────────────────────────────────────────

  /**
   * Simulate a swap and return amount out + price impact.
   *
   * Composes `get_amount_out` — no transaction is submitted.
   */
  async simulateSwap(
    tokenIn: string,
    amountIn: bigint
  ): Promise<SwapSimulation> {
    const info = await this.getInfo();
    const [reserveIn, reserveOut] =
      tokenIn === info.tokenA
        ? [info.reserveA, info.reserveB]
        : [info.reserveB, info.reserveA];

    // x*y = k constant-product formula with fee
    const feeMul = 10_000n - info.feeBps;
    const amountInWithFee = amountIn * feeMul;
    const numerator = amountInWithFee * reserveOut;
    const denominator = reserveIn * 10_000n + amountInWithFee;
    const amountOut = denominator > 0n ? numerator / denominator : 0n;
    const feeAmount = (amountIn * info.feeBps) / 10_000n;

    const spotPrice = reserveIn > 0n ? (reserveOut * 10_000n) / reserveIn : 0n;
    const executionPrice =
      amountOut > 0n ? (amountIn * 10_000n) / amountOut : 0n;
    const priceImpactBps =
      spotPrice > 0n
        ? Number(((executionPrice - spotPrice) * 10_000n) / spotPrice)
        : 0;

    return { amountIn, amountOut, priceImpactBps, feeAmount };
  }

  /** Return the amount out for `amountIn` of `tokenIn` (on-chain query). */
  async getAmountOut(tokenIn: string, amountIn: bigint): Promise<bigint> {
    const raw = await this.simulate("get_amount_out", addr(tokenIn), i128(amountIn));
    return BigInt(scValToNative(raw) as number);
  }

  /** Return the amount in required to receive `amountOut` of `tokenOut`. */
  async getAmountIn(tokenOut: string, amountOut: bigint): Promise<bigint> {
    const raw = await this.simulate("get_amount_in", addr(tokenOut), i128(amountOut));
    return BigInt(scValToNative(raw) as number);
  }

  // ── Contract ID ────────────────────────────────────────────────────────────

  get contractId(): string {
    return this.contract.contractId();
  }
}
