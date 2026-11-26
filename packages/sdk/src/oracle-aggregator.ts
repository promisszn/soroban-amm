import { Address, nativeToScVal, scValToNative, xdr } from "@stellar/stellar-sdk";
import { ReadContractClient, address, u32 } from "./internal/contract-client.js";
import { requireBigInt, requireValue } from "./internal/decode.js";
import { simulateRead } from "./internal/simulate.js";
import { decodeProtocolContractError } from "./contract-errors.js";

/**
 * Decoded return value of `get_price` and `get_price_safe`.
 *
 * Fields match the contract's `AggregatedPrice` struct exactly:
 *   - `price: i128` → `bigint`
 *   - `confidence: u32` → `number`
 *
 * `confidenceBps` is a derived convenience alias for `confidence`; both names
 * refer to the same value and are kept in sync.
 */
export interface AggregatedPrice {
  price: bigint;
  /** Confidence interval in basis points, as returned by the contract. */
  confidence: number;
  /**
   * Alias for `confidence`. Provided for readability; always equals `confidence`.
   * @deprecated Use `confidence` to match the contract field name.
   */
  confidenceBps: number;
}

/** Encode a `PairKey { token_a, token_b }` as the contract's struct ScVal (scvMap). */
function pairKey(tokenA: string, tokenB: string): xdr.ScVal {
  return xdr.ScVal.scvMap([
    new xdr.ScMapEntry({
      key: xdr.ScVal.scvSymbol("token_a"),
      val: nativeToScVal(Address.fromString(tokenA)),
    }),
    new xdr.ScMapEntry({
      key: xdr.ScVal.scvSymbol("token_b"),
      val: nativeToScVal(Address.fromString(tokenB)),
    }),
  ]);
}

export class OracleAggregatorClient extends ReadContractClient {
  protected readonly contractKind = "oracle_aggregator" as const;

  private async simulate(method: string, ...args: xdr.ScVal[]): Promise<xdr.ScVal> {
    return simulateRead(
      this.server,
      this.contract,
      this.networkPassphrase,
      method,
      args,
      (message) => decodeProtocolContractError(this.contractKind, message)
    );
  }

  /** @internal Decode a raw native map from `get_price` or `get_price_safe`. */
  private decodeAggregatedPrice(native: Record<string, unknown>): AggregatedPrice {
    const price = requireBigInt(native, "price", "AggregatedPrice");
    const confidenceRaw = requireValue(native, "confidence", "AggregatedPrice");
    const confidence = Number(confidenceRaw);
    return { price, confidence, confidenceBps: confidence };
  }

  async getPrice(a: string, b: string): Promise<AggregatedPrice> {
    const raw = await this.simulate("get_price", address(a), address(b));
    return this.decodeAggregatedPrice(scValToNative(raw) as Record<string, unknown>);
  }

  async getPriceSafe(a: string, b: string): Promise<AggregatedPrice> {
    const raw = await this.simulate("get_price_safe", address(a), address(b));
    return this.decodeAggregatedPrice(scValToNative(raw) as Record<string, unknown>);
  }

  async getPriceDetailed(a: string, b: string): Promise<unknown> {
    const raw = await this.simulate("get_price_detailed", address(a), address(b));
    return scValToNative(raw);
  }

  /**
   * Parameters for `list_sources`.
   *
   * Encodes the three arguments the contract requires:
   *   1. `pair` — `PairKey { token_a, token_b }` as a struct map
   *   2. `cursor` — pagination cursor (u32)
   *   3. `limit` — maximum sources to return (u32)
   */
  listSourcesParams(tokenA: string, tokenB: string, cursor: number, limit: number): xdr.ScVal[] {
    return this.params(pairKey(tokenA, tokenB), u32(cursor), u32(limit));
  }

  /**
   * List oracle sources for a token pair.
   *
   * @param tokenA  - Address of the first token in the pair.
   * @param tokenB  - Address of the second token in the pair.
   * @param cursor  - Pagination cursor (u32); pass 0 to start from the beginning.
   * @param limit   - Maximum number of sources to return (u32).
   */
  async listSources(tokenA: string, tokenB: string, cursor: number, limit: number): Promise<unknown[]> {
    const raw = await this.simulate("list_sources", ...this.listSourcesParams(tokenA, tokenB, cursor, limit));
    return scValToNative(raw) as unknown[];
  }
}
