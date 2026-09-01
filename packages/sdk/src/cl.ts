/**
 * ConcentratedLiquidityClient — typed client for the tick-based CL AMM contract.
 *
 * Covers the public interface of contracts/concentrated_liquidity/src/lib.rs.
 */

import {
  Contract,
  rpc as StellarRpc,
  nativeToScVal,
  scValToNative,
  xdr,
  Address,
} from "@stellar/stellar-sdk";
import type { NetworkConfig } from "./types.js";
import { simulateRead } from "./internal/simulate.js";

// ── Helpers ────────────────────────────────────────────────────────────────────

function addr(address: string): xdr.ScVal {
  return nativeToScVal(Address.fromString(address));
}

function i128(value: bigint): xdr.ScVal {
  return nativeToScVal(value, { type: "i128" });
}

function i32(value: number): xdr.ScVal {
  return nativeToScVal(value, { type: "i32" });
}

function u64(value: bigint): xdr.ScVal {
  return nativeToScVal(value, { type: "u64" });
}

function u128(value: bigint): xdr.ScVal {
  return nativeToScVal(value, { type: "u128" });
}

// ── Types ──────────────────────────────────────────────────────────────────────

/** A liquidity position returned by `get_position`. */
export interface Position {
  lowerTick: number;
  upperTick: number;
  liquidity: bigint;
  feeGrowthInsideA: bigint;
  feeGrowthInsideB: bigint;
  tokensOwedA: bigint;
  tokensOwedB: bigint;
}

/** Full pool state returned by `get_pool_state`. */
export interface ClPoolState {
  sqrtPrice: bigint;
  currentTick: number;
  activeLiquidity: bigint;
  tickSpacing: number;
}

/** Quote returned by `quote_position`. */
export interface PositionQuote {
  amountA: bigint;
  amountB: bigint;
  liquidity: bigint;
}

/** Detailed quote returned by `estimate_price_impact`. */
export interface PriceImpactEstimate {
  amountIn: bigint;
  amountInAfterFee: bigint;
  amountOut: bigint;
  feeAmount: bigint;
  spotPriceBefore: bigint;
  effectivePrice: bigint;
  priceImpactBps: bigint;
  sqrtPriceBefore: bigint;
  sqrtPriceAfter: bigint;
  tickBefore: number;
  tickAfter: number;
  activeLiquidityBefore: bigint;
  activeLiquidityAfter: bigint;
}

// ── ConcentratedLiquidityClient ───────────────────────────────────────────────

export class ConcentratedLiquidityClient {
  private readonly server: StellarRpc.Server;
  private readonly contract: Contract;
  private readonly networkPassphrase: string;

  constructor(config: NetworkConfig) {
    this.server = new StellarRpc.Server(config.rpcUrl);
    this.contract = new Contract(config.contractId);
    this.networkPassphrase = config.networkPassphrase;
  }

  get contractId(): string {
    return this.contract.contractId();
  }

  private async simulate(method: string, ...args: xdr.ScVal[]): Promise<xdr.ScVal> {
    return simulateRead(this.server, this.contract, this.networkPassphrase, method, args);
  }

  // ── Read-only methods ──────────────────────────────────────────────────────

  /** Returns the full pool state (sqrt price, current tick, active liquidity, tick spacing). */
  async getPoolState(): Promise<ClPoolState> {
    const raw = await this.simulate("get_pool_state");
    const native = scValToNative(raw) as Record<string, unknown>;
    return {
      sqrtPrice: BigInt(String(native.sqrt_price ?? 0)),
      currentTick: Number(native.current_tick ?? 0),
      activeLiquidity: BigInt(String(native.active_liquidity ?? 0)),
      tickSpacing: Number(native.tick_spacing ?? 1),
    };
  }

  /** Returns the current active tick. */
  async currentTick(): Promise<number> {
    const raw = await this.simulate("current_tick");
    return Number(scValToNative(raw));
  }

  /** Returns the current active liquidity across the current tick. */
  async activeLiquidity(): Promise<bigint> {
    const raw = await this.simulate("active_liquidity");
    return BigInt(String(scValToNative(raw)));
  }

  /**
   * Returns the tick cumulative and its timestamp `(tick_cumulative, last_ts)`.
   *
   * Used by the TWAP consumer via `save_cl_snapshot`.
   */
  async getTickCumulative(): Promise<{ tickCumulative: bigint; lastTimestamp: bigint }> {
    const raw = await this.simulate("get_tick_cumulative");
    const native = scValToNative(raw) as [unknown, unknown];
    return {
      tickCumulative: BigInt(String(native[0] ?? 0)),
      lastTimestamp: BigInt(String(native[1] ?? 0)),
    };
  }

  /** Returns the position for `owner` between `lowerTick` and `upperTick`. */
  async getPosition(owner: string, lowerTick: number, upperTick: number): Promise<Position> {
    const raw = await this.simulate("get_position", addr(owner), i32(lowerTick), i32(upperTick));
    const native = scValToNative(raw) as Record<string, unknown>;
    const owed = native.tokens_owed as [unknown, unknown];
    return {
      lowerTick: Number(native.lower_tick ?? lowerTick),
      upperTick: Number(native.upper_tick ?? upperTick),
      liquidity: BigInt(String(native.liquidity ?? 0)),
      feeGrowthInsideA: BigInt(String(native.fee_growth_inside_a ?? 0)),
      feeGrowthInsideB: BigInt(String(native.fee_growth_inside_b ?? 0)),
      tokensOwedA: BigInt(String(owed?.[0] ?? 0)),
      tokensOwedB: BigInt(String(owed?.[1] ?? 0)),
    };
  }

  /**
   * Quotes how much token A and token B are required — and how much liquidity
   * would be minted — for a position between `lowerTick` and `upperTick` given
   * desired deposit amounts.
   */
  async quotePosition(
    lowerTick: number,
    upperTick: number,
    amountA: bigint,
    amountB: bigint
  ): Promise<PositionQuote> {
    const raw = await this.simulate(
      "quote_position",
      i32(lowerTick),
      i32(upperTick),
      i128(amountA),
      i128(amountB)
    );
    const native = scValToNative(raw) as [unknown, unknown, unknown];
    return {
      amountA: BigInt(String(native[0] ?? 0)),
      amountB: BigInt(String(native[1] ?? 0)),
      liquidity: BigInt(String(native[2] ?? 0)),
    };
  }

  /**
   * Estimates swap output and price impact for a concentrated-liquidity swap.
   *
   * The contract walks initialized ticks with the same math used by `swap`, so
   * this is suitable for frontends, slippage previews, and route comparison.
   */
  async estimatePriceImpact(
    zeroForOne: boolean,
    amountIn: bigint,
    sqrtPriceLimit: bigint
  ): Promise<PriceImpactEstimate> {
    const raw = await this.simulate(
      "estimate_price_impact",
      nativeToScVal(zeroForOne),
      i128(amountIn),
      nativeToScVal(sqrtPriceLimit, { type: "u128" })
    );
    const native = scValToNative(raw) as Record<string, unknown>;
    return {
      amountIn: BigInt(String(native.amount_in ?? 0)),
      amountInAfterFee: BigInt(String(native.amount_in_after_fee ?? 0)),
      amountOut: BigInt(String(native.amount_out ?? 0)),
      feeAmount: BigInt(String(native.fee_amount ?? 0)),
      spotPriceBefore: BigInt(String(native.spot_price_before ?? 0)),
      effectivePrice: BigInt(String(native.effective_price ?? 0)),
      priceImpactBps: BigInt(String(native.price_impact_bps ?? 0)),
      sqrtPriceBefore: BigInt(String(native.sqrt_price_before ?? 0)),
      sqrtPriceAfter: BigInt(String(native.sqrt_price_after ?? 0)),
      tickBefore: Number(native.tick_before ?? 0),
      tickAfter: Number(native.tick_after ?? 0),
      activeLiquidityBefore: BigInt(String(native.active_liquidity_before ?? 0)),
      activeLiquidityAfter: BigInt(String(native.active_liquidity_after ?? 0)),
    };
  }

  /**
   * Returns the fee growth inside the tick range `[lowerTick, upperTick]` for
   * both tokens as `(fee_growth_inside_a, fee_growth_inside_b)`.
   */
  async feeGrowthInside(
    lowerTick: number,
    upperTick: number
  ): Promise<{ feeGrowthA: bigint; feeGrowthB: bigint }> {
    const raw = await this.simulate("fee_growth_inside", i32(lowerTick), i32(upperTick));
    const native = scValToNative(raw) as [unknown, unknown];
    return {
      feeGrowthA: BigInt(String(native[0] ?? 0)),
      feeGrowthB: BigInt(String(native[1] ?? 0)),
    };
  }

  /**
   * Converts a tick index to the corresponding price ratio scaled by 1_000_000.
   * Price = 1.0001^tick, returned as (price * 1_000_000).
   */
  async tickToPrice(tick: number): Promise<bigint> {
    const raw = await this.simulate("tick_to_price", i32(tick));
    return BigInt(String(scValToNative(raw)));
  }

  /**
   * Returns oracle tick cumulative values for an array of historical timestamps.
   * Returns one `i64` per requested timestamp.
   */
  async observe(timestamps: bigint[]): Promise<bigint[]> {
    const tsVec = nativeToScVal(timestamps.map((t) => nativeToScVal(t, { type: "u64" })));
    const raw = await this.simulate("observe", tsVec);
    const native = scValToNative(raw) as unknown[];
    return (native ?? []).map((v) => BigInt(String(v)));
  }

  /** Returns whether the pool is paused. */
  async isPaused(): Promise<boolean> {
    const raw = await this.simulate("is_paused");
    return Boolean(scValToNative(raw));
  }

  // ── Write-method parameter types ───────────────────────────────────────────

  /**
   * Parameters for `mint_position`.
   *
   * Mirrors `ConcentratedLiquidity::mint_position` —
   * contracts/concentrated_liquidity/src/lib.rs:407
   * `(provider: Address, lower_tick: i32, upper_tick: i32,
   *   amount_a_desired: i128, amount_b_desired: i128, min_a: i128, min_b: i128)`
   *
   * Note there is no `deadline` argument: `mint_position` has no deadline
   * guard. `minA`/`minB` are independent per-token slippage floors, not a
   * single combined minimum-liquidity bound.
   */
  mintPositionParams(
    provider: string,
    lowerTick: number,
    upperTick: number,
    amountADesired: bigint,
    amountBDesired: bigint,
    minA: bigint,
    minB: bigint
  ): xdr.ScVal[] {
    return [
      addr(provider),
      i32(lowerTick),
      i32(upperTick),
      i128(amountADesired),
      i128(amountBDesired),
      i128(minA),
      i128(minB),
    ];
  }

  /**
   * Parameters for `modify_position`.
   *
   * Mirrors `ConcentratedLiquidity::modify_position` —
   * contracts/concentrated_liquidity/src/lib.rs:540
   * `(provider: Address, lower_tick: i32, upper_tick: i32,
   *   liquidity_delta: i128, min_a: i128, min_b: i128, deadline: u64)`
   *
   * Returns `(amount_a, amount_b)`.
   */
  modifyPositionParams(
    provider: string,
    lowerTick: number,
    upperTick: number,
    liquidityDelta: bigint,
    minA: bigint,
    minB: bigint,
    deadline: bigint
  ): xdr.ScVal[] {
    return [addr(provider), i32(lowerTick), i32(upperTick), i128(liquidityDelta), i128(minA), i128(minB), u64(deadline)];
  }

  /**
   * Parameters for `burn_position`.
   *
   * Mirrors `ConcentratedLiquidity::burn_position` —
   * contracts/concentrated_liquidity/src/lib.rs:1118
   * `(provider: Address, lower_tick: i32, upper_tick: i32, liquidity: i128)`
   *
   * `liquidity` specifies how much of the position to burn and must be
   * positive; the contract rejects non-positive values with `ZeroLiquidity`.
   *
   * Returns `(amount_a, amount_b)` of tokens sent back to the provider.
   */
  burnPositionParams(
    provider: string,
    lowerTick: number,
    upperTick: number,
    liquidity: bigint
  ): xdr.ScVal[] {
    return [addr(provider), i32(lowerTick), i32(upperTick), i128(liquidity)];
  }

  /**
   * Parameters for `collect_fees`.
   *
   * Mirrors `ConcentratedLiquidity::collect_fees` —
   * contracts/concentrated_liquidity/src/lib.rs:1229
   * `(provider: Address, lower_tick: i32, upper_tick: i32)`
   *
   * Returns `(fee_a, fee_b)`.
   */
  collectFeesParams(provider: string, lowerTick: number, upperTick: number): xdr.ScVal[] {
    return [addr(provider), i32(lowerTick), i32(upperTick)];
  }

  /**
   * Parameters for `swap`.
   *
   * Mirrors `ConcentratedLiquidity::swap` —
   * contracts/concentrated_liquidity/src/lib.rs:1437
   * `(sender: Address, zero_for_one: bool, amount_in: i128,
   *   sqrt_price_limit_x96: u128, min_amount_out: i128, deadline: u64)`
   *
   * Direction is expressed as `zeroForOne` (true = swapping token A in for
   * token B out), not as an input-token address — the contract takes a `bool`
   * in that position. `sqrtPriceLimitX96` is an unsigned Q64.96 value and must
   * be encoded as `u128`, not `i128`.
   */
  swapParams(
    sender: string,
    zeroForOne: boolean,
    amountIn: bigint,
    sqrtPriceLimitX96: bigint,
    minAmountOut: bigint,
    deadline: bigint
  ): xdr.ScVal[] {
    return [
      addr(sender),
      nativeToScVal(zeroForOne, { type: "bool" }),
      i128(amountIn),
      u128(sqrtPriceLimitX96),
      i128(minAmountOut),
      u64(deadline),
    ];
  }
}
