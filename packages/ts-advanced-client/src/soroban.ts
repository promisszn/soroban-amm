import type { Transaction, TransactionSubmitter } from './core.js'

// -------------------------------------------------------------------------------------------------------
// RPC interface
//
// Narrow on purpose: the `@soroban-amm/sdk` write path (and `@stellar/stellar-sdk`'s `rpc.Server`
// behind a thin adapter) implements it, and tests implement it with a fake. This package does not
// duplicate the SDK's transaction building.
// -------------------------------------------------------------------------------------------------------

export interface SorobanSimulation {
  /** Set when the simulation failed; the transaction is not sent. */
  error?: string
  /** Resource fee the simulation asks for, in stroops. */
  minResourceFee?: bigint
  /** Set when RPC reports that the footprint must be restored before invocation. */
  restoreRequired?: boolean
}

export type SorobanSendStatus = 'PENDING' | 'DUPLICATE' | 'TRY_AGAIN_LATER' | 'ERROR'

export interface SorobanSendResponse {
  status: SorobanSendStatus
  hash: string
  errorMessage?: string
}

export type SorobanTransactionStatus = 'NOT_FOUND' | 'SUCCESS' | 'FAILED'

export interface SorobanTransactionResponse<TReturn = unknown> {
  status: SorobanTransactionStatus
  returnValue?: TReturn
}

export interface SorobanRpc<TTransaction extends Transaction, TReturn = unknown> {
  simulateTransaction(tx: TTransaction): Promise<SorobanSimulation>
  /** Applies the simulation's footprint and resource fee to the transaction. */
  assembleTransaction(
    tx: TTransaction,
    simulation: SorobanSimulation,
  ): TTransaction | Promise<TTransaction>
  /** Builds a restoreFootprint transaction for a restore simulation. */
  createRestoreTransaction?(tx: TTransaction, simulation: SorobanSimulation): Promise<TTransaction>
  /** Refreshes the source sequence after the restore transaction is confirmed. */
  refreshTransaction?(tx: TTransaction): Promise<TTransaction>
  sendTransaction(tx: TTransaction): Promise<SorobanSendResponse>
  getTransaction(hash: string): Promise<SorobanTransactionResponse<TReturn>>
}

export interface SorobanSigner<TTransaction extends Transaction> {
  sign(tx: TTransaction): Promise<TTransaction>
}

// -------------------------------------------------------------------------------------------------------
// Errors and result
// -------------------------------------------------------------------------------------------------------

export type SorobanSubmissionErrorCode =
  | 'SIMULATION_FAILED'
  | 'SEND_REJECTED'
  | 'RESUBMIT_EXHAUSTED'
  | 'TRANSACTION_FAILED'
  | 'POLL_TIMEOUT'

export class SorobanSubmissionError extends Error {
  constructor(
    readonly code: SorobanSubmissionErrorCode,
    message: string,
    readonly hash?: string,
  ) {
    super(message)
    this.name = 'SorobanSubmissionError'
  }
}

export interface SorobanSubmissionResult<TReturn = unknown> {
  hash: string
  status: 'SUCCESS'
  returnValue?: TReturn
}

// -------------------------------------------------------------------------------------------------------
// Submitter
// -------------------------------------------------------------------------------------------------------

export interface SorobanSubmitterOptions<TTransaction extends Transaction, TReturn = unknown> {
  rpc: SorobanRpc<TTransaction, TReturn>
  signer: SorobanSigner<TTransaction>
  /** How long to poll for a final status after the network accepted the transaction. Default 30 000. */
  timeoutMs?: number
  /** Delay between polls and between resubmissions. Default 1 000. */
  pollIntervalMs?: number
  /** How many times to resubmit after `TRY_AGAIN_LATER`. Default 3. */
  maxResubmits?: number
  /** Injectable for tests. */
  sleep?: (ms: number) => Promise<void>
  /** Injectable clock for the polling deadline, in milliseconds. */
  now?: () => number
}

const defaultSleep = (ms: number): Promise<void> =>
  new Promise((resolve) => setTimeout(resolve, ms))

/**
 * Simulate, assemble, sign, send, then poll until `SUCCESS` or `FAILED`.
 * Resubmits on `TRY_AGAIN_LATER`. Every failure is a `SorobanSubmissionError`
 * with a distinct `code`.
 */
export class SorobanSubmitter<TTransaction extends Transaction, TReturn = unknown>
  implements TransactionSubmitter<TTransaction, SorobanSubmissionResult<TReturn>>
{
  private readonly rpc: SorobanRpc<TTransaction, TReturn>
  private readonly signer: SorobanSigner<TTransaction>
  private readonly timeoutMs: number
  private readonly pollIntervalMs: number
  private readonly maxResubmits: number
  private readonly sleep: (ms: number) => Promise<void>
  private readonly now: () => number

  constructor(options: SorobanSubmitterOptions<TTransaction, TReturn>) {
    this.rpc = options.rpc
    this.signer = options.signer
    this.timeoutMs = options.timeoutMs ?? 30_000
    this.pollIntervalMs = options.pollIntervalMs ?? 1_000
    this.maxResubmits = options.maxResubmits ?? 3
    this.sleep = options.sleep ?? defaultSleep
    this.now = options.now ?? Date.now
  }

  async sendTransaction(tx: TTransaction): Promise<SorobanSubmissionResult<TReturn>> {
    let simulation = await this.rpc.simulateTransaction(tx)
    if (simulation.error !== undefined) {
      throw new SorobanSubmissionError(
        'SIMULATION_FAILED',
        `simulation failed: ${simulation.error}`,
      )
    }

    if (simulation.restoreRequired) {
      if (!this.rpc.createRestoreTransaction || !this.rpc.refreshTransaction) {
        throw new SorobanSubmissionError(
          'SIMULATION_FAILED',
          'simulation requires footprint restoration, but the RPC adapter does not support it',
        )
      }

      const restore = await this.rpc.createRestoreTransaction(tx, simulation)
      const signedRestore = await this.signer.sign(restore)
      const restoreHash = await this.submit(signedRestore)
      await this.poll(restoreHash)

      tx = await this.rpc.refreshTransaction(tx)
      simulation = await this.rpc.simulateTransaction(tx)
      if (simulation.error !== undefined) {
        throw new SorobanSubmissionError(
          'SIMULATION_FAILED',
          `simulation failed after footprint restoration: ${simulation.error}`,
        )
      }
      if (simulation.restoreRequired) {
        throw new SorobanSubmissionError(
          'SIMULATION_FAILED',
          'simulation still requires footprint restoration after the restore transaction',
        )
      }
    }

    const assembled = await this.rpc.assembleTransaction(tx, simulation)
    const signed = await this.signer.sign(assembled)
    const hash = await this.submit(signed)
    return this.poll(hash)
  }

  private async submit(signed: TTransaction): Promise<string> {
    for (let resubmits = 0; ; resubmits++) {
      const response = await this.rpc.sendTransaction(signed)
      switch (response.status) {
        case 'PENDING':
        case 'DUPLICATE':
          return response.hash
        case 'ERROR':
          throw new SorobanSubmissionError(
            'SEND_REJECTED',
            `network rejected the transaction: ${response.errorMessage ?? 'no reason given'}`,
            response.hash,
          )
        case 'TRY_AGAIN_LATER':
          if (resubmits >= this.maxResubmits) {
            throw new SorobanSubmissionError(
              'RESUBMIT_EXHAUSTED',
              `network asked to try again later ${String(resubmits + 1)} times`,
              response.hash,
            )
          }
          await this.sleep(this.pollIntervalMs)
          break
        default:
          throw new SorobanSubmissionError(
            'SEND_REJECTED',
            `unexpected sendTransaction status: ${String(response.status)}`,
            response.hash,
          )
      }
    }
  }

  private async poll(hash: string): Promise<SorobanSubmissionResult<TReturn>> {
    const deadline = this.now() + this.timeoutMs
    for (;;) {
      const response = await this.rpc.getTransaction(hash)
      if (response.status === 'SUCCESS') {
        return { hash, status: 'SUCCESS', returnValue: response.returnValue }
      }
      if (response.status === 'FAILED') {
        throw new SorobanSubmissionError('TRANSACTION_FAILED', `transaction ${hash} failed`, hash)
      }
      if (this.now() >= deadline) {
        throw new SorobanSubmissionError(
          'POLL_TIMEOUT',
          `transaction ${hash} was not final after ${String(this.timeoutMs)} ms`,
          hash,
        )
      }
      await this.sleep(this.pollIntervalMs)
    }
  }
}
