/**
 * GraphQL resolvers, kept apart from `index.ts` so tests can build a server
 * from them without starting one (`index.ts` listens on import).
 */
import { GraphQLError } from "graphql";
import { PubSub } from "graphql-subscriptions";
import {
  defaultIndexer,
  InvalidComparisonError,
  InvalidMetricError,
  InvalidThresholdError,
  type AlertConfig,
  type AlertMetric,
} from "./indexer.js";
import { asSeconds } from "./time.js";
import type { AnalyticsStore } from "./store/interface.js";
import type { PoolIndexer } from "./indexer.js";

export interface GraphQLContext {
  [key: string]: unknown;
  indexer: PoolIndexer;
  store?: AnalyticsStore;
}

function activeIndexer(context?: Partial<GraphQLContext>): PoolIndexer {
  return context?.indexer ?? defaultIndexer;
}

export const pubsub = new PubSub();

export const resolvers = {
  Query: {
    poolStats: (_: unknown, { poolId }: { poolId?: string }, context?: GraphQLContext) =>
      activeIndexer(context).getPoolStats(poolId),
    poolEvents: (
      _: unknown,
      { poolId, limit }: { poolId?: string; limit?: number },
      context?: GraphQLContext,
    ) => {
      try {
        return activeIndexer(context).getEvents(poolId, limit ?? 100);
      } catch (error) {
        if (error instanceof RangeError) {
          throw new GraphQLError(error.message, {
            extensions: { code: "BAD_USER_INPUT" },
          });
        }
        throw error;
      }
    },
    positions: (_: unknown, { owner }: { owner?: string }, context?: GraphQLContext) =>
      activeIndexer(context).getPositions(owner),
    priceHistory: (
      _: unknown,
      { poolId, from, to }: { poolId: string; from?: number; to?: number },
      context?: GraphQLContext,
    ) =>
      activeIndexer(context).getPriceHistory(
        poolId,
        from === undefined ? undefined : asSeconds(from),
        to === undefined ? undefined : asSeconds(to),
      ),
    twal: (
      _: unknown,
      { poolId, windowSeconds }: { poolId: string; windowSeconds: number },
      context?: GraphQLContext,
    ) => {
      if (windowSeconds <= 0) {
        throw new GraphQLError("windowSeconds must be greater than 0", {
          extensions: { code: "BAD_USER_INPUT" },
        });
      }
      return activeIndexer(context).getTwal(poolId, windowSeconds);
    },
    poolHealth: (_: unknown, { poolId }: { poolId: string }, context?: GraphQLContext) =>
      activeIndexer(context).getPoolHealth(poolId),
    alertConfigs: (_: unknown, { poolId }: { poolId?: string }, context?: GraphQLContext) =>
      activeIndexer(context).getAlertConfigs(poolId),
  },
  Mutation: {
    setAlertConfig: (
      _: unknown,
      {
        poolId,
        metric,
        comparison,
        thresholdBps,
        thresholdValue,
      }: {
        poolId: string;
        metric: string;
        comparison?: string;
        thresholdBps?: number;
        thresholdValue?: number;
      },
      context?: GraphQLContext,
    ): AlertConfig | Promise<AlertConfig> => {
      const indexer = activeIndexer(context);
      try {
        const config = indexer.validateAlertConfig({
          poolId,
          metric: metric as AlertMetric,
          comparison: comparison as "above" | "below" | undefined,
          thresholdBps,
          thresholdValue,
        });
        // Durable first: a store failure must leave the in-memory indexer
        // untouched, otherwise memory and the store diverge permanently.
        return context?.store
          ? context.store.setAlertConfig(config).then(() => indexer.applyAlertConfig(config))
          : indexer.applyAlertConfig(config);
      } catch (error) {
        if (error instanceof InvalidMetricError || error instanceof InvalidThresholdError ||
            error instanceof InvalidComparisonError) {
          throw new GraphQLError(error.message, {
            extensions: { code: "BAD_USER_INPUT" },
          });
        }
        throw error;
      }
    },
    removeAlertConfig: (
      _: unknown,
      { poolId, metric }: { poolId: string; metric: string },
      context?: GraphQLContext,
    ): boolean | Promise<boolean> => {
      const indexer = activeIndexer(context);
      if (!context?.store) return indexer.removeAlertConfig(poolId, metric);
      return context.store.removeAlertConfig(poolId, metric).then((stored) => {
        const removed = indexer.removeAlertConfig(poolId, metric);
        return removed || stored;
      });
    },
  },
  PoolEvent: {
    payload: (parent: { payload: Record<string, unknown> }) =>
      JSON.stringify(parent.payload),
  },
  Subscription: {
    poolEvent: {
      subscribe: (_: unknown, { poolId }: { poolId?: string }) =>
        pubsub.asyncIterableIterator(poolId ? `EVENT:${poolId}` : "EVENT:ALL"),
    },
  },
};
