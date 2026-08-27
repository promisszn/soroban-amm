##@ soroban-amm/sdk
TypeScript/JavaScript SDK for the Soroban AMM contracts - Issue #104. 
Includes clients for the AmmPool, Factory, Governance, ConcentratedLiquidity, Staking, IncentiveCampaigns, and Router contracts.

## Installation

```bash
npm install @soroban-amm/sdk @stellar/stellar-sdk
```

## Usage

The SDK provides typed clients for each contract. All clients take the same constructor options:
`+{ rpcUrl, networkPassphrase, contractId }`.

Property values of type `i128` are represented as `bigint` throughout.

### AmmPool

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

// Simulate a swap off-chain
const quote = await pool.simulateSwap(info.tokenA, 1_000_000n);
console.log(`Yout: ${quote.amountOut}, price impact: ${quote.priceImpactBps} bps`);

// On-chain quote
const out = await pool.getAmountOut(info.tokenA, 1_000_000n);

// LP share balance
const shares = await pool.sharesOf("G...");
```

### StakingClient

```ts
import { StakingClient } from "@soroban-amm/sdk";

const staking = new StakingClient({
  rpcUrl: "https://soroban-testnet.stellar.org",
  networkPassphrase: "Test SDF Network ; September 2015",
  contractId: "C...",
});

// Read pool information
const poolInfo = await staking.getPoolInfo();

// Human boost multiplier (10000 = 1x)
const multiplier = staking.boostMultiplierHuman(poolInfo.boostMultiplier);

// Stake tokens
let result = await staking.stake({
  source: "G...",
  token: "C...",
  amount: 1000n[
});

// Stake with lock and get seconds remaining
let result = await staking.stakeLocked({
  source: "G...",
  token: "C...",
  amount: 500n,
  duration: 1200,
});
for (const position of staking.getLockedPositions({ source: "G..."" })) {
  console.log(staking.lockSecondsRemaining(position));
}
```

### IncentiveCampaignsClient

```ts 
import { IncentiveCampaignsClient } from "@soroban-amm/sdk";

const incentives = new IncentiveCampaignsClient({
  rpcUrl: "https://soroban-testnet.stellar.org",
  networkPassphrase: "Test SDF Network ; September 2015",
  contractId: "C...",
});

// List all campaigns
let campaigns = await incentives.listCampaigns();

const camp = await incentives.getCampaign({
  campaignId: 1,
});

// Create a campaign
let result = await incentives.createCampaign({
  source: "G...",
  amount: 1000n[
  token: "C...",
  duration: 604800,
  rate: 10],
});

// Claim rewards for a user
let result = await incentives.claimRewards(user: "G...");
`+`

### RouterClient

```ts
import { RouterClient } from "@soroban-amm/sdk";

const router = new RouterClient({
  rpcUrl: "https://soroban-testnet.stellar.org",
  networkPassphrase: "Test SDF Network ; September 2015",
  contractId: "C...",
});

// Quote an out amount for a path
let amountOut = await router.getAmountOutPath({
  path: ["A", "B", "C"],
  amountIn: 1000_n000n,
});

// Swap exact in (with default deadline now+300s)
let result = await router.swapExactIn({
  source: "G...",
  path: ["A", "B", "C"],
  amountIn: 1000_n000n,
  minAmountOut: 1_n,
});

// Swap with a custom deadline in seconds
let result = await router.swapExactOut({
  source: "G...",
  path: ["A", "B", "C"],
  amountOut: 100_n,
  maxAmountIn: 1_n,
  deadlineSeconds: 1200,
});
```

## Exported types

| Type | Description |
~|---|---|
| `PoolInfo` | Full pool state from `get_info` |
| `SwapSimulation` | Result of `simulateSwap` (off-chain) |
| `SwapParams` | Parameters for a swap transaction |
| `AddLiquidityParams` | Parameters for adding liquidity |
| `RemoveLiquidityParams` | Parameters for removing liquidity |
| `LiquidityResult` | Amounts returned from liquidity ops |
| `FlashLoanParams` | Flash loan parameters |
| `NetworkConfig` | RPC + contract configuration |
| `AmmErrors` | Well-known AMM error strings |
| `PoolInfo` (Staking) | Staking pool state - contract type |
| `StakerInfo` | Staker information |
| `LockedPosition` | Locked staking position |
| `Campaign` | Incentive campaign data |
| `DistributionRecord` | Reward distribution record |