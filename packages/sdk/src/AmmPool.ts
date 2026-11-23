/**
 * AmmPool — typed wrapper for the Soroban AMM contract — Issue #104
 *
 * Provides human-readable error decoding, a `simulate` helper that composes
 * `simulate_swap` + `get_amount_out`, and typed wrappers for every public
 * AMM function.
 */

import {
  Account,
  Contract,
  Networks,
  TransactionBuilder,
  rpc as StellarRpc,
  nativeToScVal,
  scValToNative,
  xdr,
  Address,
} from "@stellar/stellar-sdk";
import type {
  NetworkConfig,
  Signer,
  PoolInfo,
  SwapParams,
  SwapSimulation,
  AddLiquidityParams,
  RemoveLiquidityParams,
  LiquidityResult,
  FlashLoanParams,
} from "./types.js";
import { AmmErrors, AmmErrorNames } from "./types.js";
import type { AmmErrorCode, AmmErrorKey } from "./types.js";
import { simulateRead } from "./internal/simulate.js";

// ── Helpers ────────────────────────────────────────────────────────────────────

function i128(value: bigint): xdr.ScVal {
  return nativeToScVal(value, { type: "i128" });
}

function u64(value: bigint): xdr.ScVal {
  return nativeToScVal(value, { type: "u64" });
}

function addr(address: string): xdr.ScVal {
  return nativeToScVal(Address.fromString(address));
}

/**
 * A contract-returned `AmmError`, decoded from the numeric discriminant Soroban
 * reports. Carries the discriminant and its symbolic name so callers can branch
 * on the specific failure rather than string-matching the message.
 */
export class AmmContractError extends Error {
  /** Numeric discriminant, matching `AmmError` in contracts/amm/src/lib.rs. */
  readonly code: AmmErrorCode;
  /** Symbolic variant name, e.g. `"Paused"`. */
  readonly name: AmmErrorKey;
  /** Unmodified message reported by the RPC server. */
  readonly rawMessage: string;

  constructor(code: AmmErrorCode, rawMessage: string) {
    super(`AMM error: ${AmmErrors[code]}`);
    this.code = code;
    this.name = AmmErrorNames[code];
    this.rawMessage = rawMessage;
  }
}

/**
 * Matches the numeric-coded form Soroban RPC uses to report a contract-returned
 * error, e.g. `Error(Contract, #6)`. Whitespace is tolerated because the exact
 * spacing varies between RPC versions and SDK error wrappers.
 */
const CONTRACT_ERROR_PATTERN = /Error\s*\(\s*Contract\s*,\s*#(\d+)\s*\)/;

/**
 * Decode a simulation or RPC failure into a friendly, typed AMM error.
 *
 * Soroban reports contract errors as `Error(Contract, #N)` where `N` is the
 * `AmmError` discriminant — never as descriptive English text, which is why the
 * previous substring match against phrases like "contract is paused" could never
 * fire. We parse the discriminant and look it up in {@link AmmErrors}, falling
 * back to the raw message when no discriminant is present (host errors, network
 * failures) or when the discriminant is one this SDK does not know.
 */
export function decodeError(err: unknown): Error {
  const msg = err instanceof Error ? err.message : String(err);
  const match = CONTRACT_ERROR_PATTERN.exec(msg);
  if (match) {
    const code = Number(match[1]);
    if (code in AmmErrors) {
      return new AmmContractError(code as AmmErrorCode, msg);
    }
    return new Error(`AMM error: unknown contract error #${code}: ${msg}`);
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
    return simulateRead(
      this.server,
      this.contract,
      this.networkPassphrase,
      method,
      args,
      decodeError
    );
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
      admin: native.admin ? String(native.admin) : null,
      isPaused: Boolean(native.is_paused),
      name: native.name ? String(native.name) : null,
    };
  }

  /** Return protocol fees accrued but not yet withdrawn — read-only. */
  async getAccruedFees(): Promise<{ accruedA: bigint; accruedB: bigint }> {
    const raw = await this.simulate("get_accrued_fees");
    const native = scValToNative(raw) as [unknown, unknown];
    return {
      accruedA: BigInt(String(native[0] ?? 0)),
      accruedB: BigInt(String(native[1] ?? 0)),
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

  // ── Signed write methods ───────────────────────────────────────────────────

  /**
   * Build, assemble, sign, and submit a `swap` call.
   *
   * The `signer` callback receives the base64-encoded unsigned transaction XDR
   * and must return the base64-encoded signed XDR. This keeps key material out
   * of the SDK: pass a {@link KeypairSigner} for an in-process secret key, or
   * a remote/HSM implementation for production.
   *
   * Returns the amount of output token received.
   */
  async swap(params: SwapParams, signer: Signer): Promise<bigint> {
    const args = [
      addr(params.trader),
      addr(params.tokenIn),
      i128(params.amountIn),
      i128(params.minAmountOut),
      u64(params.deadline),
    ];
    const raw = await this.submitSigned("swap", args, params.trader, signer);
    return BigInt(String(scValToNative(raw) ?? 0));
  }

  /**
   * Build, assemble, sign, and submit an `add_liquidity` call.
   *
   * Returns the amounts deposited and LP shares minted.
   */
  async addLiquidity(params: AddLiquidityParams, signer: Signer): Promise<LiquidityResult> {
    const args = [
      addr(params.provider),
      i128(params.amountA),
      i128(params.amountB),
      i128(params.minShares),
      u64(params.deadline),
    ];
    const raw = await this.submitSigned("add_liquidity", args, params.provider, signer);
    const native = scValToNative(raw) as [unknown, unknown, unknown];
    return {
      amountA: BigInt(String(native[0] ?? 0)),
      amountB: BigInt(String(native[1] ?? 0)),
      shares: BigInt(String(native[2] ?? 0)),
    };
  }

  /**
   * Build, assemble, sign, and submit a `remove_liquidity` call.
   *
   * Returns the amounts of token A and token B returned, and shares burned.
   */
  async removeLiquidity(params: RemoveLiquidityParams, signer: Signer): Promise<LiquidityResult> {
    const args = [
      addr(params.provider),
      i128(params.shares),
      i128(params.minAmountA),
      i128(params.minAmountB),
      u64(params.deadline),
    ];
    const raw = await this.submitSigned("remove_liquidity", args, params.provider, signer);
    const native = scValToNative(raw) as [unknown, unknown, unknown];
    return {
      amountA: BigInt(String(native[0] ?? 0)),
      amountB: BigInt(String(native[1] ?? 0)),
      shares: BigInt(String(native[2] ?? 0)),
    };
  }

  // ── Internal signed-submit helper ──────────────────────────────────────────

  /**
   * Simulate, assemble (adds auth entries and footprint), sign via `signer`,
   * submit, and await the transaction.
   *
   * The source account sequence number is fetched from the RPC server so the
   * envelope is valid when sent. Auth entries are built for `sourceAddress`.
   */
  private async submitSigned(
    method: string,
    args: xdr.ScVal[],
    sourceAddress: string,
    signer: Signer
  ): Promise<xdr.ScVal> {
    const source = await this.server.getAccount(sourceAddress);
    const tx = new TransactionBuilder(source, {
      fee: "100",
      networkPassphrase: this.networkPassphrase,
    })
      .addOperation(this.contract.call(method, ...args))
      .setTimeout(30)
      .build();

    const simResult = await this.server.simulateTransaction(tx);
    if (StellarRpc.Api.isSimulationError(simResult)) {
      throw decodeError(simResult.error);
    }

    const assembled = StellarRpc.assembleTransaction(tx, simResult).build();
    const signedXdr = await signer(assembled.toXDR());
    const signed = TransactionBuilder.fromXDR(signedXdr, this.networkPassphrase);

    const sendResult = await this.server.sendTransaction(signed);
    if (sendResult.status === "ERROR") {
      throw decodeError(sendResult.errorResult?.toXDR() ?? "transaction submission failed");
    }

    // Poll until the transaction is confirmed or rejected.
    let getResult: StellarRpc.Api.GetTransactionResponse;
    do {
      await new Promise((resolve) => setTimeout(resolve, 1000));
      getResult = await this.server.getTransaction(sendResult.hash);
    } while (getResult.status === StellarRpc.Api.GetTransactionStatus.NOT_FOUND);

    if (getResult.status !== StellarRpc.Api.GetTransactionStatus.SUCCESS) {
      throw decodeError(`transaction ${sendResult.hash} failed: ${getResult.status}`);
    }

    const meta = getResult.resultMetaXdr;
    const ops = meta?.v3?.()?.sorobanMeta?.()?.returnValue?.();
    return ops ?? xdr.ScVal.scvVoid();
  }

  // ── Contract ID ────────────────────────────────────────────────────────────

  get contractId(): string {
    return this.contract.contractId();
  }
}
