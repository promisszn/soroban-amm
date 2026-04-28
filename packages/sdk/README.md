# @soroban-amm/sdk

TypeScript/JavaScript SDK for the Soroban AMM contract — Issue #104.

## Installation

```bash
npm install @soroban-amm/sdk @stellar/stellar-sdk
```

## Usage

```ts
import { AmmPool } from "@soroban-amm/sdk";

const pool = new AmmPool({
  rpcUrl: "https://soroban-testnet.stellar.org",
  networkPassphrase: "Test SDF Network ; September 2015",
  contractId: "C...",
});

// Fetch full pool state
const info = await pool.getInfo();
console.log(info.reserveA, info.reserveB, info.feeBps);

// Get pool name (Issue #105 metadata)
const name = await pool.getName();

// Get flash-loan fee (Issue #102 public getter)
const flashFee = await pool.getFlashLoanFeeBps();

// Simulate a swap off-chain
const quote = await pool.simulateSwap(info.tokenA, 1_000_000n);
console.log(`Out: ${quote.amountOut}, price impact: ${quote.priceImpactBps} bps`);

// On-chain quote
const out = await pool.getAmountOut(info.tokenA, 1_000_000n);

// LP share balance
const shares = await pool.sharesOf("G...");
```

## Exported types

| Type | Description |
|---|---|
| `PoolInfo` | Full pool state from `get_info` |
| `SwapSimulation` | Result of `simulateSwap` (off-chain) |
| `SwapParams` | Parameters for a swap transaction |
| `AddLiquidityParams` | Parameters for adding liquidity |
| `RemoveLiquidityParams` | Parameters for removing liquidity |
| `LiquidityResult` | Amounts returned from liquidity ops |
| `FlashLoanParams` | Flash loan parameters |
| `NetworkConfig` | RPC + contract configuration |
| `AmmErrors` | Well-known AMM error strings |

## Building

```bash
npm run build   # tsc → dist/
npm test        # vitest
```
