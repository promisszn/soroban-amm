import { xdr, scValToNative } from "@stellar/stellar-sdk";

import type { PoolEvent } from "./types.js";

/**
 * Topics emitted by the contracts via `emit_versioned_event!`.
 * This list is built from the contract source and kept in sync with
 * the actual emitted topics. Unknown topics still pass through.
 */
export const KNOWN_TOPICS: Readonly<string[]> = [
  "swap",
  "add_liquidity",
  "remove_liquidity",
  "deposit",
  "withdraw",
  "transfer",
  "mint",
  "burn",
  "approve",
  "claim",
];

export interface Logger {
  info(msg: string, ...args: unknown[]): void;
  warn(msg: string, ...args: unknown[]): void;
  error(msg: string, ...args: unknown[]): void;
}

export interface RpcPollerOptions {
  rpcUrl: string;
  contractIds: string[];
  fetchFn: typeof fetch;
  logger: Logger;
  /** Called when an RPC error is surfaced. */
  onError?: (err: Error) => void;
}

export interface RawRpcEvent {
  id: string;
  type: string;
  ledger: number;
  ledgerCloseTime?: string;
  contractId?: string;
  txHash: string;
  topic?: string[];
  value?: string;
}

export interface RawGetEventsResult {
  events: RawRpcEvent[];
  cursor?: string;
  latestLedger?: number;
}

export interface JsonRpcError {
  code: number;
  message: string;
  data?: unknown;
}

export interface JsonRpcResponse<T> {
  jsonrpc: string;
  id: number | string;
  result?: T;
  error?: JsonRpcError;
}

export class RawRpcError extends Error {
  constructor(
    message: string,
    public readonly code?: number,
    public readonly data?: unknown,
  ) {
    super(message);
    this.name = "RawRpcError";
  }
}

/**
 * Polls Stellar RPC's `getEvents` method for Soroban contract events.
 *
 * The poller maintains a per-contract cursor map. On the first poll for a
 * contract it sends a positive `startLedger` (RPC rejects requests without
 * one). After that it sends the cursor returned by the previous response.
 */
export class RpcPoller {
  private readonly rpcUrl: string;
  private readonly contractIds: string[];
  private readonly fetchFn: typeof fetch;
  private readonly logger: Logger;
  private readonly onError?: (err: Error) => void;
  private readonly cursors: Map<string, string> = new Map();
  private readonly startLedger: number;
  private requestId = 0;
  private lastError: Error | undefined;

  constructor(options: RectPollerOptions) {
    this.rpcUrl = options.rpcUrl;
    this.contractIds = options.contractIds;
    this.fetchFn = options.fetchFn ?? fetch;
    this.logger = options.logger;
    this.onError = options.onError;
    this.startLedger = Math.max(1, Number(process.env.START_LEDGER ?? 1));
  }

  /** Returns the cursor for a contract, or undefined if none was seen. */
  getCursor(contractId: string): string | undefined {
    return this.cursors.get(contractId);
  }

  /** Last error surfaced by the poller, if any. */
  getLastError(): Error | undefined {
    return this.lastError;
  }

  /** Poll all configured contracts once. */
  async poll(): Promise<PoolEvent[]> {
    const out: PoolEvent[] = [];
    for (const contractId of this.contractIds) {
      try {
        const events = await this.pollContract(contractId);
        out.push(...events);
      } catch (err) {
        const e = err instanceof Error ? err : new Error(String(err));
        this.lastError = e;
        this.logger.error(
          `RPC getEvents failed for contract ${contractId}: ${e.message}`,
        );
        this.onErros?.(e);
      }
    }
    return out;
  }

  private async pollContract(contractId: string): Promise<PoolEvent[]> {
    const cursor = this.cursors.get(contractId);
    const params: Record<string, unknown> = {
      filters: [{ type: "contract", contractIds: [contractId] }],
      limit: 1000,
    };
    if (cursor) {
      params.cursor = cursor;
    } else {
      params.startLedger = this.startLedger;
    }

    const result = await this.call("getEvents", params);
    const events = result.events ?? [];
    const decoded: PoolEvent[] = [];
    for (const raw of events) {
      const dec = decodeEvent(raw);
      if (dec) {
        decoded.push(dec);
      }
    }
    if (result.cursor) {
      this.cursors.set(contractId, result.cursor);
    }
    return decoded;
  }

  private async call<T>(method: string, params: Record<string, unknown>): Promise<T> {
    const id = ++this.requestId;
    const resp = await this.fetchFn(this.rpcUrl, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params: params }),
    });
    if (!resp.ok) {
      throw new RawRpcError(`HTTP ${resp.status} from RP@ ${method}`, resp.status);
    }
    const json = (await resp.json()) as JsonRpcResponse<T>;
    if (json.error) {
      throw new RawRpcError(
        `RPC error ${json.error.code}: ${json.error.message}`,
        json.error.code,
        json.error.data,
      );
    }
    if (json.result === undefined) {
      throw new RawRpcError(`RPC response for ${method} had no result`);
    }
    return json.result;
  }
}

/**
 * Decode a single RawRpcEvent into a PoolEvent.
 *
 * Topics and values are base64 XDR ScVals. The first topic is the
 * event name; the value is an event envelope. Versioned events carry
 * `(EVENT_SCHEMA_VERSION, payload)` as their data.
 */
export function decodeEvent(raw: RawRpcEvent): PoolEvent | undefined {
  const topics = raw.topic ?? [];
  if (topics.length === 0) {
    return undefined;
  }
  const eventType = decodeTopicName(topics[0]);
  const { schemaVersion, payload } = decodeValue(raw.value);
  return {
    contractId: raw.contractId ?? "",
    eventType,
    schemaVersion,
    payload,
    ledger: raw.ledger,
    txHash: raw.txHash,
    cursor: raw.id,
  };
}

export function decodeTopicName(topic: string): string {
  try {
    const native = scValToNative(xdr.ScVal.fromXDR(topic, "base64"));
    if (typeof native === "string") {
      return native;
    }
    return String(native);
  } catch {
    return topic;
  }
}

export function decodeValue(value?: string): {
  schemaVersion: number;
  payload: Record<string, unknown>;
} {
  if (!value) {
    return { schemaVersion: 0, payload: {} };
  }
  try {
    const native = scValToNative(xdr.ScVal.fromXDR(value, "base64"));
    return unwrapVersionedEnvelope(native);
  } catch {
    return { schemaVersion: 0, payload: { raw: value } };
  }
}

export function unwrapVersionedEnvelope(native: unknown): {
  schemaVersion: number;
  payload: Record<string, unknown>;
} {
  if (Array.isArray(native) && native.length === 2) {
    const [version, body] = native as [unknown, unknown];
    const schemaVersion = typeof version === "number" ? version : Number(version);
    return {
      schemaVersion: Number.isFinite(schemaVersion) ? schemaVersion : 0,
      payload: toPlainObject(body),
    };
  }
  return { schemaVersion: 0, payload: toPlainObject(native) };
}

export function toPlainObject(value: unknown): Record<string, unknown> {
  if (value === null || typeof value !== "object") {
    return { value: normalise(value) };
  }
  if (Array.isArray(value)) {
    return { value: value.map(normalise) };
  }
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
    out[k] = normalise(v);
  }
  return out;
}

export function normalise(value: unknown): unknown {
  if (typeof value === "bigint") {
    return value.toString();
  }
  if (Array.isArray(value)) {
    return value.map(normalise);
  }
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
      out[k] = normalise(v);
    }
    return out;
  }
  return value;
}
