/**
 * @soroban-amm/sdk
 *
 * Typed TypeScript SDK for all five Soroban AMM contracts.
 *
 * ```ts
 * import { AmmPool, TokenClient, FactoryClient, GovernanceClient, ConcentratedLiquidityClient } from "@soroban-amm/sdk";
 *
 * const pool = new AmmPool({ rpcUrl, networkPassphrase, contractId });
 * const info = await pool.getInfo();
 * const quote = await pool.simulateSwap(tokenIn, amountIn);
 *
 * const factory = new FactoryClient({ rpcUrl, networkPassphrase, contractId: factoryId });
 * const pools = await factory.allPools();
 * ```
 */

export { AmmPool, AmmContractError, decodeError } from "./AmmPool.js";
export { RouterClient } from "./router.js";
export type { SwapExactInInput, SwapExactOutInput } from "./router.js";
export { SIMULATION_SOURCE_ACCOUNT } from "./internal/simulate.js";
export { TokenClient } from "./token.js";
export { FactoryClient } from "./factory.js";
export type { CreatePoolResult } from "./factory.js";
export { GovernanceClient } from "./governance.js";
export type { GovernanceParams, Proposal, ProposalStatus, VoteChoice, VoteRecord } from "./governance.js";
export { ConcentratedLiquidityClient } from "./cl.js";
export type { Position, ClPoolState, PositionQuote, PriceImpactEstimate } from "./cl.js";
export * from "./types.js";
