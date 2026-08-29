/**
 * Abstract store interface for analytics data persistence.
 *
 * The indexer depends on this interface, not on concrete storage implementations.
 * This allows tests to use the fast in-memory store, while production deployments
 * can use a persistent store (SQLite) that survives restarts.
 */

export interface PoolEvent {
  id: string;
  poolId: string;
  type: PoolEventType;
  timestamp: number;
  ledger: number;
  txHash: string;
  eventIndex: number;
  payload: Record<string, unknown>;
}

export type PoolEventType =
  | "swap"
  | "add_liquidity"
  | "remove_liquidity"
  | "campaign_created"
  | "reward_distributed"
  | "fot_detected"
  | "price_upd"
  | "tick_crossed";

export interface PoolStats {
  poolId: string;
  tokenA: string;
  tokenB: string;
  tvl: number;
  volume24h: number;
  fees24h: number;
  swapCount: number;
  priceDeviationBps: number;
  lastUpdate: number;
}

export interface Position {
  id: string;
  poolId: string;
  owner: string;
  shares: number;
  valueUsd: number;
}

export interface PricePoint {
  poolId: string;
  timestamp: number;
  price: number;
  feeBps: number;
}

export interface HealthAlert {
  poolId: string;
  metric: string;
  threshold: number;
  currentValue: number;
  firedAt: number;
}

export interface AlertConfig {
  poolId: string;
  metric: string;
  thresholdBps: number;
}

/** Ingestion cursor tracking progress through RPC events. */
export interface IngestionCursor {
  /**
   * Last ledger sequence number successfully ingested.
   * Used to resume from where we left off.
   */
  ledger: number;
  /**
   * Last transaction hash processed at this ledger.
   * Used for idempotency (events keyed by (ledger, txHash, eventIndex)).
   */
  txHash: string;
  /**
   * Last event index processed in this transaction.
   * Used for idempotency.
   */
  eventIndex: number;
  /** Timestamp of last update, for monitoring lag. */
  updatedAt: number;
}

/**
 * Abstract store interface for all analytics data.
 *
 * Implementations must support both in-memory (for tests) and persistent
 * (SQLite, for production) backends transparently.
 */
export interface AnalyticsStore {
  // ─── Event storage ───────────────────────────────────────────────────────
  
  /**
   * Append an event to the store.
   * Must be idempotent: appending the same event twice by (ledger, txHash, eventIndex)
   * does not create a duplicate.
   */
  appendEvent(event: PoolEvent): Promise<void>;

  /**
   * Query events by pool and time range.
   */
  queryEvents(poolId: string, from: number, to: number, limit?: number): Promise<PoolEvent[]>;

  /**
   * Get the most recent events across all pools.
   */
  getRecentEvents(limit: number): Promise<PoolEvent[]>;

  // ─── Pool stats ──────────────────────────────────────────────────────────

  /**
   * Upsert pool statistics.
   * Creates if not exists, updates if already present.
   */
  upsertPoolStats(stats: PoolStats): Promise<void>;

  /**
   * Get pool statistics.
   */
  getPoolStats(poolId?: string): Promise<PoolStats[]>;

  // ─── Positions ───────────────────────────────────────────────────────────

  /**
   * Upsert a liquidity position.
   */
  upsertPosition(position: Position): Promise<void>;

  /**
   * Query positions by owner.
   */
  getPositions(owner?: string): Promise<Position[]>;

  // ─── Price history ──────────────────────────────────────────────────────

  /**
   * Record a price point.
   */
  recordPrice(point: PricePoint): Promise<void>;

  /**
   * Query price history for a pool over a time range.
   */
  getPriceHistory(poolId: string, from: number, to: number): Promise<PricePoint[]>;

  // ─── Alerts ─────────────────────────────────────────────────────────────

  /**
   * Set or update an alert configuration.
   */
  setAlertConfig(config: AlertConfig): Promise<AlertConfig>;

  /**
   * Remove an alert configuration.
   */
  removeAlertConfig(poolId: string, metric: string): Promise<boolean>;

  /**
   * Get alert configurations.
   */
  getAlertConfigs(poolId?: string): Promise<AlertConfig[]>;

  /**
   * Record a fired alert.
   */
  recordAlert(alert: HealthAlert): Promise<void>;

  /**
   * Get fired alerts.
   */
  getFiredAlerts(poolId?: string, limit?: number): Promise<HealthAlert[]>;

  // ─── Ingestion cursor ────────────────────────────────────────────────────

  /**
   * Get the current ingestion cursor (progress through RPC events).
   * Returns null if never ingested.
   */
  getCursor(): Promise<IngestionCursor | null>;

  /**
   * Update the ingestion cursor (called after successfully processing events).
   */
  setCursor(cursor: IngestionCursor): Promise<void>;

  // ─── Lifecycle ───────────────────────────────────────────────────────────

  /**
   * Close the store and release any resources (database connections, etc.).
   * Called during graceful shutdown.
   */
  close(): Promise<void>;
}
