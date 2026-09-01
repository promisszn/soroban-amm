/**
 * Per-subscription circuit breaker (issue #720).
 *
 * A permanently-dead endpoint should stop costing the service anything. After
 * `failureThreshold` consecutive failures the circuit opens and delivery is
 * skipped outright until `cooldownMs` has elapsed; the next attempt after that
 * is a half-open probe. A successful probe closes the circuit, a failed one
 * re-opens it and restarts the cooldown.
 *
 * The clock is injectable so cooldown behaviour can be tested without sleeping.
 */

import type { CircuitSnapshot, CircuitState } from "./types.js";

export const DEFAULT_FAILURE_THRESHOLD = 5;
export const DEFAULT_COOLDOWN_MS = 30_000;

export interface CircuitBreakerOptions {
  failureThreshold?: number;
  cooldownMs?: number;
  /** Injectable clock, defaults to Date.now. */
  now?: () => number;
}

interface CircuitEntry {
  state: CircuitState;
  consecutiveFailures: number;
  openedAt?: number;
}

export class CircuitBreaker {
  private circuits = new Map<string, CircuitEntry>();
  readonly failureThreshold: number;
  readonly cooldownMs: number;
  private readonly now: () => number;

  constructor(opts: CircuitBreakerOptions = {}) {
    this.failureThreshold = opts.failureThreshold ?? DEFAULT_FAILURE_THRESHOLD;
    this.cooldownMs = opts.cooldownMs ?? DEFAULT_COOLDOWN_MS;
    this.now = opts.now ?? Date.now;
  }

  private entry(id: string): CircuitEntry {
    let e = this.circuits.get(id);
    if (!e) {
      e = { state: "closed", consecutiveFailures: 0 };
      this.circuits.set(id, e);
    }
    return e;
  }

  /**
   * Whether a delivery may be attempted right now.
   *
   * Transitions an expired open circuit to half-open as a side effect, so the
   * caller's attempt becomes the probe.
   */
  canAttempt(id: string): boolean {
    const e = this.entry(id);
    if (e.state === "open") {
      const openedAt = e.openedAt ?? 0;
      if (this.now() - openedAt >= this.cooldownMs) {
        e.state = "half_open";
        return true;
      }
      return false;
    }
    return true;
  }

  /** Record a successful delivery: closes the circuit and clears the counter. */
  recordSuccess(id: string): void {
    const e = this.entry(id);
    e.state = "closed";
    e.consecutiveFailures = 0;
    delete e.openedAt;
  }

  /**
   * Record a failed delivery.
   *
   * A failure while half-open re-opens immediately — the endpoint is still
   * broken, so the cooldown restarts rather than allowing repeated probes.
   */
  recordFailure(id: string): void {
    const e = this.entry(id);
    e.consecutiveFailures += 1;

    if (e.state === "half_open") {
      e.state = "open";
      e.openedAt = this.now();
      return;
    }

    if (e.consecutiveFailures >= this.failureThreshold) {
      e.state = "open";
      e.openedAt = this.now();
    }
  }

  /** Current state of one circuit. */
  stateOf(id: string): CircuitState {
    return this.entry(id).state;
  }

  /** Snapshot of one circuit, for the management API. */
  snapshot(id: string): CircuitSnapshot {
    const e = this.entry(id);
    const snap: CircuitSnapshot = {
      subscriptionId: id,
      state: e.state,
      consecutiveFailures: e.consecutiveFailures,
    };
    if (e.state === "open" && e.openedAt !== undefined) {
      snap.nextProbeAt = e.openedAt + this.cooldownMs;
    }
    return snap;
  }

  /** Snapshots of every circuit this breaker has seen. */
  snapshots(): CircuitSnapshot[] {
    return [...this.circuits.keys()].map((id) => this.snapshot(id));
  }

  /** Forget a subscription's circuit, e.g. after it is unregistered. */
  forget(id: string): void {
    this.circuits.delete(id);
  }

  /** Drop all circuit state. */
  reset(): void {
    this.circuits.clear();
  }
}

export const defaultCircuitBreaker = new CircuitBreaker();
