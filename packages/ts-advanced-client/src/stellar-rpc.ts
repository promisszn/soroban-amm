import { BASE_FEE, Operation, TransactionBuilder, rpc } from '@stellar/stellar-sdk'
import type { rpc as StellarRpc, Transaction, xdr } from '@stellar/stellar-sdk'
import type {
  SorobanRpc,
  SorobanSendResponse,
  SorobanSimulation,
  SorobanTransactionResponse,
} from './soroban.js'

export interface SorobanRpcAdapterOptions {
  server: StellarRpc.Server
  /** Multiplier applied to the base and resource fees for restore transactions. */
  feeMultiplier?: number
  /** Timeout for restore transactions, in seconds. */
  timeoutSeconds?: number
}

/**
 * Adapts stellar-sdk's RPC server to SorobanSubmitter.
 */
export class SorobanRpcAdapter implements SorobanRpc<Transaction, xdr.ScVal> {
  private readonly server: StellarRpc.Server
  private readonly feeMultiplier: number
  private readonly timeoutSeconds: number
  private readonly simulationResponses = new WeakMap<
    SorobanSimulation,
    StellarRpc.Api.SimulateTransactionResponse
  >()

  constructor(options: SorobanRpcAdapterOptions) {
    this.server = options.server
    this.feeMultiplier = options.feeMultiplier ?? 1.15
    this.timeoutSeconds = options.timeoutSeconds ?? 60
  }

  async simulateTransaction(tx: Transaction): Promise<SorobanSimulation> {
    const response = await this.server.simulateTransaction(tx)

    if (rpc.Api.isSimulationError(response)) {
      return { error: response.error }
    }
    if (!rpc.Api.isSimulationSuccess(response)) {
      return { error: 'RPC returned an unrecognized simulation response' }
    }

    const simulation: SorobanSimulation = {
      minResourceFee: BigInt(response.minResourceFee),
      restoreRequired: rpc.Api.isSimulationRestore(response),
    }
    this.simulationResponses.set(simulation, response)
    return simulation
  }

  async assembleTransaction(
    tx: Transaction,
    simulation: SorobanSimulation,
  ): Promise<Transaction> {
    const response = this.getSimulationResponse(simulation)
    return rpc.assembleTransaction(tx, response).build()
  }

  async createRestoreTransaction(
    tx: Transaction,
    simulation: SorobanSimulation,
  ): Promise<Transaction> {
    const response = this.getSimulationResponse(simulation)
    if (!rpc.Api.isSimulationRestore(response)) {
      throw new TypeError('Simulation does not contain a restore preamble')
    }

    const account = await this.server.getAccount(tx.source)
    const fee = Math.ceil(
      (Number(BASE_FEE) + Number(response.restorePreamble.minResourceFee)) *
        this.feeMultiplier,
    )
    return new TransactionBuilder(account, {
      fee: String(fee),
      networkPassphrase: tx.networkPassphrase,
    })
      .addOperation(Operation.restoreFootprint({}))
      .setSorobanData(response.restorePreamble.transactionData.build())
      .setTimeout(this.timeoutSeconds)
      .build()
  }

  async refreshTransaction(tx: Transaction): Promise<Transaction> {
    const account = await this.server.getAccount(tx.source)
    const builder = TransactionBuilder.cloneFrom(tx, {
      fee: tx.fee,
      networkPassphrase: tx.networkPassphrase,
    })
    builder.source = account
    return builder.build()
  }

  async sendTransaction(tx: Transaction): Promise<SorobanSendResponse> {
    const response = await this.server.sendTransaction(tx)
    return {
      status: response.status,
      hash: response.hash,
      ...(response.errorResult
        ? { errorMessage: response.errorResult.toXDR('base64') }
        : {}),
    }
  }

  async getTransaction(hash: string): Promise<SorobanTransactionResponse<xdr.ScVal>> {
    const response = await this.server.getTransaction(hash)
    const status = response.status
    switch (status) {
      case rpc.Api.GetTransactionStatus.SUCCESS:
        return { status: 'SUCCESS', returnValue: response.returnValue }
      case rpc.Api.GetTransactionStatus.FAILED:
        return { status: 'FAILED' }
      case rpc.Api.GetTransactionStatus.NOT_FOUND:
        return { status: 'NOT_FOUND' }
      default:
        throw new TypeError(`Unexpected transaction status: ${String(status)}`)
    }
  }

  private getSimulationResponse(
    simulation: SorobanSimulation,
  ): StellarRpc.Api.SimulateTransactionResponse {
    const response = this.simulationResponses.get(simulation)
    if (!response) {
      throw new TypeError('Simulation was not produced by this SorobanRpcAdapter')
    }
    return response
  }
}
