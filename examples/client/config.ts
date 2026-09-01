/** Pure parsing/validation for this example's numeric env vars. */

export function parseAmountIn(raw: string | undefined): bigint {
  const value = raw ?? "100000";
  let amount: bigint;
  try {
    amount = BigInt(value);
  } catch {
    throw new Error(`SWAP_AMOUNT_IN must be an integer, got "${value}"`);
  }
  if (amount <= 0n) {
    throw new Error(`SWAP_AMOUNT_IN must be a positive integer, got "${value}"`);
  }
  return amount;
}

export function parseDeadlineSeconds(raw: string | undefined): number {
  const value = raw ?? "300";
  const seconds = Number(value);
  if (!Number.isFinite(seconds) || seconds <= 0) {
    throw new Error(`DEADLINE_SECONDS must be a positive number, got "${value}"`);
  }
  return seconds;
}

export function parseSlippageBps(raw: string | undefined): bigint {
  const value = raw ?? "50";
  let bps: bigint;
  try {
    bps = BigInt(value);
  } catch {
    throw new Error(`SLIPPAGE_BPS must be an integer in [0, 10000), got "${value}"`);
  }
  if (bps < 0n || bps >= 10_000n) {
    throw new Error(`SLIPPAGE_BPS must be an integer in [0, 10000), got "${value}"`);
  }
  return bps;
}
