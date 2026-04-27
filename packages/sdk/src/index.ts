/**
 * @soroban-amm/sdk — Issue #104
 *
 * Typed TypeScript SDK for the Soroban AMM contract.
 *
 * ```ts
 * import { AmmPool } from "@soroban-amm/sdk";
 * const pool = new AmmPool({ rpcUrl, networkPassphrase, contractId });
 * const info  = await pool.getInfo();
 * const quote = await pool.simulateSwap(tokenIn, amountIn);
 * ```
 */

export { AmmPool } from "./AmmPool.js";
export * from "./types.js";
