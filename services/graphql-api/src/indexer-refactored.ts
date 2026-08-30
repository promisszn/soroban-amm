/**
 * Analytics indexer — consumes pool events and computes metrics.
 *
 * This class depends on an AnalyticsStore interface, not a concrete
 * storage backend. This allows it to work with in-memory stores (for tests)
 * or persistent stores (SQLite, for production) transparently.
 *
 * The indexer computes and maintains:
 * - Pool statistics (TVL, 24h volume, fees)
 * - Price history
 * - Liquidity position state
 * - Health scores and alerts
 * - Fee accounting
 */

import type {
  AnalyticsStore,
  PoolEvent,
  PoolStats,
  PricePoint,
  HealthAlert,
  AlertConfig,
} from "./store/interface.js";

const RETENTION_MS = 30 * 24 * 60 * 60 * 1000; // 30 days
const VOLUME_WINDOW_MS = 24 * 60 * 60 * 1000; // 24 hours

export class PoolIndexer {
  constructor(private store: AnalyticsStore) {}

  /**
   * Process an incoming pool event.
   * Updates statistics and triggers alert checks.
   */
  async indexEvent(event: PoolEvent): Promise<void> {
    // Append to store (idempotent)
    await this.store.appendEvent(event);

    // Update pool statistics based on event type
    let stats = (await this.store.getPoolStats(event.poolId))[0] ?? {
      poolId: event.poolId,
      tokenA: String(event.payload["tokenA"] ?? ""),
      tokenB: String(event.payload["tokenB"] ?? ""),
      tvl: 0,
      volume24h: 0,
      fees24h: 0,
      swapCount: 0,
      priceDeviationBps: 0,
      lastUpdate: Date.now(),
    };

    switch (event.type) {
      case "swap":
        stats.swapCount += 1;
        stats.volume24h += Number(event.payload.amountIn ?? 0);
        stats.fees24h += Number(event.payload.fee ?? 0);
        const price = Number(event.payload.price ?? 0);
        if (price > 0) {
          await this.store.recordPrice({
            poolId: event.poolId,
            timestamp: event.timestamp,
            price,
            feeBps: 30,
          });
          stats.priceDeviationBps = await this.computePriceDeviation(
            event.poolId,
            price,
          );
        }
        break;

      case "add_liquidity":
        stats.tvl +=
          Number(event.payload.amountA ?? 0) +
          Number(event.payload.amountB ?? 0);
        break;

      case "remove_liquidity":
        stats.tvl -=
          Number(event.payload.amountA ?? 0) +
          Number(event.payload.amountB ?? 0);
        stats.tvl = Math.max(0, stats.tvl);
        break;

      case "price_upd":
        const newPrice = Number(event.payload.price ?? 0);
        if (newPrice > 0) {
          await this.store.recordPrice({
            poolId: event.poolId,
            timestamp: event.timestamp,
            price: newPrice,
            feeBps: Number(event.payload.feeBps ?? 30),
          });
          stats.priceDeviationBps = await this.computePriceDeviation(
            event.poolId,
            newPrice,
          );
        }
        break;
    }

    stats.lastUpdate = Date.now();
    await this.store.upsertPoolStats(stats);

    // Check for alert conditions
    await this.checkAlerts(event.poolId, stats);
  }

  /**
   * Get current pool statistics.
   */
  async getPoolStats(poolId?: string): Promise<PoolStats[]> {
    return this.store.getPoolStats(poolId);
  }

  /**
   * Get events with optional filtering.
   */
  async getEvents(poolId?: string, limit = 100): Promise<PoolEvent[]> {
    if (poolId) {
      const now = Date.now() / 1000;
      const from = (now - RETENTION_MS / 1000) | 0;
      return this.store.queryEvents(poolId, from, now, limit);
    }
    return this.store.getRecentEvents(limit);
  }

  /**
   * Compute the current health score for a pool.
   */
  async getPoolHealth(poolId: string) {
    const stats = (await this.store.getPoolStats(poolId))[0];
    if (!stats) return null;

    // TVL score: 0-100. Pools with TVL > 1M score near 100.
    const tvlScore = Math.min(100, (stats.tvl / 1_000_000) * 100);

    // Volume score: volume/TVL ratio. Healthy is 5–20% daily turnover.
    const volumeRatio = stats.tvl > 0 ? stats.volume24h / stats.tvl : 0;
    const volumeScore = Math.min(100, volumeRatio * 500); // 20% ratio = 100 pts

    // Fee efficiency: fees relative to TVL. Healthy is 0.01–0.1% daily.
    const feeRatio = stats.tvl > 0 ? stats.fees24h / stats.tvl : 0;
    const feeEfficiencyScore = Math.min(100, feeRatio * 100_000); // 0.1% = 100 pts

    // Price deviation penalty: high deviation reduces score.
    const deviationPenalty = Math.min(100, stats.priceDeviationBps / 10);

    const healthScore = Math.max(
      0,
      tvlScore * 0.4 +
        volumeScore * 0.35 +
        feeEfficiencyScore * 0.25 -
        deviationPenalty * 0.5,
    );

    const status: "healthy" | "warning" | "critical" =
      healthScore >= 70
        ? "healthy"
        : healthScore >= 40
          ? "warning"
          : "critical";

    const alertsFired = await this.store.getFiredAlerts(poolId, 10);

    return {
      poolId,
      healthScore: Math.round(healthScore * 10) / 10,
      tvlScore: Math.round(tvlScore * 10) / 10,
      volumeScore: Math.round(volumeScore * 10) / 10,
      feeEfficiencyScore: Math.round(feeEfficiencyScore * 10) / 10,
      priceDeviationBps: stats.priceDeviationBps,
      status,
      alertsFired,
    };
  }

  /**
   * Set an alert configuration.
   */
  async setAlertConfig(config: AlertConfig): Promise<AlertConfig> {
    return this.store.setAlertConfig(config);
  }

  /**
   * Remove an alert configuration.
   */
  async removeAlertConfig(poolId: string, metric: string): Promise<boolean> {
    return this.store.removeAlertConfig(poolId, metric);
  }

  /**
   * Get alert configurations.
   */
  async getAlertConfigs(poolId?: string): Promise<AlertConfig[]> {
    return this.store.getAlertConfigs(poolId);
  }

  /**
   * Compute price deviation (TWAP vs spot).
   */
  private async computePriceDeviation(
    poolId: string,
    currentPrice: number,
  ): Promise<number> {
    const oneHourAgo = Math.floor(
      (Date.now() - 60 * 60 * 1000) / 1000,
    );
    const history = await this.store.getPriceHistory(
      poolId,
      oneHourAgo,
      Math.floor(Date.now() / 1000),
    );

    if (history.length < 2) return 0;

    const twap =
      history.reduce((sum: number, p: PricePoint) => sum + p.price, 0) /
      history.length;
    if (twap === 0) return 0;

    return Math.round(
      (Math.abs(currentPrice - twap) / twap) * 10_000,
    );
  }

  /**
   * Check alert conditions for a pool.
   */
  private async checkAlerts(
    poolId: string,
    stats: PoolStats,
  ): Promise<void> {
    const configs = await this.store.getAlertConfigs(poolId);

    for (const cfg of configs) {
      let currentValue = 0;

      if (cfg.metric === "price_deviation") {
        currentValue = stats.priceDeviationBps;
      } else if (cfg.metric === "tvl") {
        currentValue = stats.tvl;
      } else if (cfg.metric === "volume24h") {
        currentValue = stats.volume24h;
      }

      if (currentValue > cfg.thresholdBps) {
        const alert: HealthAlert = {
          poolId,
          metric: cfg.metric,
          threshold: cfg.thresholdBps,
          currentValue,
          firedAt: Date.now(),
        };

        await this.store.recordAlert(alert);
      }
    }
  }
}
