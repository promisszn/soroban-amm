import { Account, Address, Contract, Keypair, SorobanRpc, TransactionBuilder, scValToNative, xdr } from '@stellar/stellar-sdk';

/**
 * Typed Client for the Soroban AMM router contract.
 *
 * @remarks
 * - Read methods simulate and do not require a signer.
 * - Write methods take a source account (a keypair) and return the submitted transaction result.
 * - [128] values are ``bigint` throughout.
 */

export interface RouterClientOptions {
  rpcUrl: string;
  networkPassphrase: string;
  contractId: string;
}

/**
 * Input arguments for `this.swapExactIn`(`)`.
 * See contracts/router/src/lib.rs:16 "swap_exact_in"
 */
export interface SwapExactInInput {
  path: string[];
  amountIn: bigint;
  amountOutMin: bigint;
  to: string;
  deadlineSeconds?: number;
}

/**
 * Input arguments for `this.swapExactOut()`,
 * See contracts/router/src/lib.rs:22 "swap_exact_out"
 */
export interface SwapExactOutInput {
  path: string[];
  amountOut: bigint;
  amountInMax: bigint;
  to: string;
  deadlineSeconds?: number;
}

/** Converts an array of Stellar public keys to a ScVal vector of addresses. */
function addressVecToScVal(addresses: string[]): xdr.ScVal {
  return xdr.ScVal.scvVec(addresses.map(a => Address.fromString(a).toScVal()));
}

/** Converts i128 value to `ScVal`. */
function i128ToScVal(value: bigint): xdr.ScVal {
  const lo: bigint = BigInt.asUintN(64, value);
  const hi: bigint = BigInt.asUintN(64, value >> 64n);
  return xdr.ScVal.scvI128(
    new xdr.Int128Parts({
      lo: xdr.Uint64.fromString(lo.toString()),
      hi: xdr.Uint64.fromString(hi.toString()),
    })
  );
}

/** Converts u64 value to `ScVal`. */
function u64ToScVal(value: bigint): xdr.ScVal {
  return xdr.ScVal.scvU64(xdr.Uint64.fromString(value.toString()));
}

/**
 * Client for the Soroban AMM router contract.
 *
 * Usage:
 * ```tr
 * const client = new RouterClient({ rpcUrl, networkPassphrase, contractId });
 * const amountOut = await client.getAmountOutPath(1000n, [path]);
 * `token`)
 */
export class RouterClient {
  private rpcUrl: string;
  private networkPassphrase: string;
  private contractId: string;
  private server: SorobanRpc.Server;
  private contract: Contract;
  private static dummyAccount = new Account(
    'GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWWF',
    '0'
  );

  constructor(options: RouterClientOptions) {
    this.rpcUrl = options.rpcUrl;
    this.networkPassphrase = options.networkPassphrase;
    this.contractId = options.contractId;
    this.server = new SorobanRpc.Server(options.rpcUrl, { allowHttp: true });
    this.contract = new Contract(options.contractId);
  }

  /**
   * Return the factory address used by the router.
   * See contracts/router/src/lib.rs:8 "get_factory"
   */
  async getFactory(): Promise<string> {
    return this.simulateRead('get_factory', []);
  }

  /**
   * Return the output amount for a given input amount and path.
   * See contracts/router/src/lib.rs:12 "get_amount_out_path"
   */
  async getAmountOutPath(amountIn: bigint, path: string[]): Promise<bigint> {
    return this.simulateRead(
      'get_amount_out_path',
      [i128ToScVal(amountIn), addressVecToScVal(path)]
    );
  }

  /**
   * Swap tokens with an exact input amount.
   * `srcInput` contains `deadlineSeconds` optionally, defaulting to ledger time + 300.
   * See contracts/router/src/lib.rs:16 "swap_exact_in"
   */
  async swapExactIn(input: SwapExactInInput, source: Keypair, fee?: string): Promise<SorobanRpc.SendTransactionResponse> {
    const deadline = await this.resolveDeadline(input.deadlineSeconds);
    return this.submitContractCall(
      'swap_exact_in',
      [
        addressVecToScVal(input.path),
        i128ToScVal(input.amountIn),
        i128ToScVal(input.amountOutMin),
        Address.fromString(input.to).toScVal(),
        u64ToScVal(deadline),
      ],
      source,
      fee
    );
  }

  /**
   * Swap tokens with an exact output amount.
   * `srcInput` contains `deadlineSeconds` optionally, defaulting to ledger time + 300.
   * See contracts/router/src/lib.rs:22 "swap_exact_out"
   */
  async swapExactOut(input: SwapExactOutInput, source: Keypair, fee?: string): Promise<SorobanRpc.SendTransactionResponse> {
    const deadline = await this.resolveDeadline(input.deadlineSeconds);
    return this.submitContractCall(
      'swap_exact_out',
      [
        addressVecToScVal(input.path),
        i128ToScVal(input.amountOut),
        i128ToScVal(input.amountInMax),
        Address.fromString(input.to).toScVal(),
        u64ToScVal(deadline),
      ],
      source,
      fee
    );
  }

  private async simulateRead(method: string, params: xdr.ScVal[]): Promise<any> {
    const op = this.contract.call(method, ...params);
    const tx = new TransactionBuilder(
      RouterClient.dummyAccount,
      { fee: '100', networkPassphrase: this.networkPassphrase }
    )
      .addOperation(op)
      .setTimeout(0)
      .build();
    const sim = await this.server.simulateTransaction(tx);
    if ((sim as any).error) {
      throw this.decodeError((sim as any).error);
    }
    const retval = (sim as any).result?.retval ?? (sim as any).result;
    if (!retval) throw new Error('No result from simulation');
    return scValToNative(retval as xdr.ScVal);
  }

  private async {private submitContractCall(
    method: string,
    params: xdr.ScVal[],
    source: Keypair,
    fee?: string
  ): Promise<SorobanRpc.SendTransactionResponse> {
    if (!source) throw new Error('Source account required');
    const publicKey = source.publicKey();
    const account = await this.server.getAccount(publicKey);
    const op = this.contract.call(method, ...params);
    const tx = new TransactionBuilder(account, {
      fee: fee ?? '100',
      networkPassphrase: this.networkPassphrase
    })
      .addOperation(op)
      .setTimeout(0)
      .build();

    // Simulate first to catch contract reverts.
    const sim = await this.server.simulateTransaction(tx);
    if ((sim as any).error) {
      throw this.decodeError((sim as any).error);
    }

    tx.sign(source);
    const sendResponse = await this.server.sendTransaction(tx);
    if (sendResponse.status === 'ERROR') {
      throw this.decodeError(sendResponse.errorResult ?? sendResponse);
    }
    return sendResponse;
  }

  private async resolveDeadline(deadlineSeconds?: number): Promise<bigint> {
    if (deadlineSeconds !== undefined) {
      return BigInt(deadlineSeconds);
    }
    const latest = await this.server.getLatestLedger();
    return BigInt(latest.timestamp) + 300n;
  }

  private decodeError(error: unknown): Error {
    const message = (error as { message?: unknown })?.message;
    if (typeof message === 'string') {
      return new Error(message);
    }
    return new Error(String(error));
  }
}
