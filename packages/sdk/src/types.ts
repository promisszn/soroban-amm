/**
 * Shared TypeScript types for the Soroban AMM SDK — Issue #104
 */

/** Network configuration for the SDK. */
export interface NetworkConfig {
  rpcUrl: string;
  networkPassphrase: string;
  contractId: string;
}

/** Full pool state returned by `get_info`. */
export interface PoolInfo {
  tokenA: string;
  tokenB: string;
  reserveA: bigint;
  reserveB: bigint;
  totalShares: bigint;
  feeBps: bigint;
  protocolFeeBps: bigint;
  feeRecipient: string | null;
  flashLoanFeeBps: bigint;
  isPaused: boolean;
  name: string | null;
}

/** Input for a swap call. */
export interface SwapParams {
  /** Caller / trader address (public key). */
  trader: string;
  /** Address of the token being sent in. */
  tokenIn: string;
  /** Amount to send in (in token's smallest unit). */
  amountIn: bigint;
  /** Minimum amount of the output token to accept (slippage guard). */
  minAmountOut: bigint;
  /** Unix timestamp deadline — transaction must execute before this. */
  deadline: bigint;
}

/** Result of a swap simulation. */
export interface SwapSimulation {
  amountIn: bigint;
  amountOut: bigint;
  priceImpactBps: number;
  feeAmount: bigint;
}

/** Input for adding liquidity. */
export interface AddLiquidityParams {
  provider: string;
  amountA: bigint;
  amountB: bigint;
  minShares: bigint;
  deadline: bigint;
}

/** Input for removing liquidity. */
export interface RemoveLiquidityParams {
  provider: string;
  shares: bigint;
  minAmountA: bigint;
  minAmountB: bigint;
  deadline: bigint;
}

/** Result of liquidity operations. */
export interface LiquidityResult {
  amountA: bigint;
  amountB: bigint;
  shares: bigint;
}

/** Flash loan parameters. */
export interface FlashLoanParams {
  receiver: string;
  tokenA: bigint;
  tokenB: bigint;
}

/** Well-known AMM error strings surfaced by the contract. */
export const AmmErrors = {
  PAUSED: "contract is paused",
  DEADLINE_PASSED: "deadline passed",
  SLIPPAGE: "slippage",
  ZERO_LIQUIDITY: "zero liquidity",
  INSUFFICIENT_LIQUIDITY: "insufficient liquidity",
  INVALID_TOKEN: "invalid token",
  NOT_ADMIN: "not admin",
} as const;

export type AmmErrorKey = keyof typeof AmmErrors;
