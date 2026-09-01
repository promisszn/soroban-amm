// ── Shared types for the webhook-streamer service (issue #306) ──────────────

/** Raw Soroban contract event as returned by Horizon's /events endpoint. */
export interface HorizonEvent {
  id: string;
  type: string;
  ledger: number;
  ledgerClosedAt: string;
  contractId: string;
  topic: string[];
  value: string;
  pagingToken: string;
}

/** Normalised pool event forwarded to webhooks. */
export interface PoolEvent {
  id: string;
  contractId: string;
  eventType: string;
  ledger: number;
  timestamp: string;
  payload: Record<string, unknown>;
}

/** A registered webhook subscription. */
export interface WebhookSubscription {
  id: string;
  url: string;
  /** Filter by contract ID; undefined = all contracts. */
  contractId?: string;
  /** Filter by event type (e.g. "swap", "mint_pos"); undefined = all types. */
  eventType?: string;
  /** Shared secret sent in X-Webhook-Secret header for HMAC verification. */
  secret?: string;
  createdAt: number;
}

/**
 * Why a delivery attempt failed.
 *
 * `timeout` is deliberately distinct from `network` so operators can tell a
 * slow subscriber from a broken one (issue #720).
 */
export type FailureKind = "timeout" | "network" | "http" | "circuit_open";

/** Outcome of one individual HTTP attempt within a delivery. */
export interface AttemptOutcome {
  /** 0-based attempt index. */
  attempt: number;
  success: boolean;
  statusCode?: number;
  failureKind?: FailureKind;
  error?: string;
  /** Delay waited *before* this attempt was made, in ms. */
  delayMs: number;
  attemptedAt: number;
}

/** Result of a single webhook delivery attempt. */
export interface DeliveryResult {
  subscriptionId: string;
  url: string;
  success: boolean;
  statusCode?: number;
  error?: string;
  attemptedAt: number;
  /** Set when the delivery failed; distinguishes a timeout from other failures. */
  failureKind?: FailureKind;
  /** True when the request was abandoned at the configured timeout. */
  timedOut?: boolean;
  /** Every attempt made, in order. Length is 1 when no retry occurred. */
  attempts?: AttemptOutcome[];
  /** True when the delivery was abandoned and written to the dead-letter queue. */
  deadLettered?: boolean;
  /** Identifier of the dead-letter entry, when one was created. */
  deadLetterId?: string;
}

/** A permanently-failed delivery, retained for inspection and replay. */
export interface DeadLetter {
  id: string;
  event: PoolEvent;
  subscription: WebhookSubscription;
  attempts: AttemptOutcome[];
  /** When the delivery was finally abandoned. */
  failedAt: number;
  /** Human-readable summary of the terminal failure. */
  reason: string;
}

/** Circuit-breaker states, following the standard closed → open → half-open cycle. */
export type CircuitState = "closed" | "open" | "half_open";

/** Point-in-time view of one subscription's circuit. */
export interface CircuitSnapshot {
  subscriptionId: string;
  state: CircuitState;
  consecutiveFailures: number;
  /** Epoch ms at which an open circuit may next be probed. */
  nextProbeAt?: number;
}

/** Aggregate delivery counters exposed via GET /metrics. */
export interface MetricsSnapshot {
  attempted: number;
  succeeded: number;
  failed: number;
  timedOut: number;
  deadLettered: number;
  retries: number;
  shortCircuited: number;
  circuits: CircuitSnapshot[];
}
