# Soroban AMM TypeScript client example

This example targets **Soroban Testnet** and uses the local `@soroban-amm/sdk` package rather than hand-encoding contract calls. It demonstrates the safe order for reading pool state, quoting a swap, applying slippage and deadline bounds, reviewing liquidity operations, and reading LP balances.

## Install and build

From this directory:

```sh
npm install
npm run build
npm test
```

The SDK is consumed through the local workspace dependency `file:../../packages/sdk`, so the example stays aligned with the checked-in typed clients.

## Configure

Required variables are `AMM_CONTRACT_ID`, `TOKEN_IN_CONTRACT_ID`, and `SOURCE_ADDRESS`. `SOURCE_ADDRESS` is the public key that will act as trader/provider when a wallet adapter submits transactions. Optional variables are `LP_TOKEN_CONTRACT_ID`, `FACTORY_CONTRACT_ID`, `SWAP_AMOUNT_IN` (default `100000`), `SLIPPAGE_BPS` (default `50`), `DEADLINE_SECONDS` (default `300`), `STELLAR_RPC_URL` (default Soroban Testnet), and `STELLAR_NETWORK_PASSPHRASE` (default Testnet passphrase).

A complete configured run is:

```sh
AMM_CONTRACT_ID=<pool> TOKEN_IN_CONTRACT_ID=<token> SOURCE_ADDRESS=<G...> LP_TOKEN_CONTRACT_ID=<lp-token> npm start
```

The example prints a readable contract error and links to `docs/error-codes.md` if an RPC or simulation call fails. It deliberately does not recreate `TransactionBuilder`, `nativeToScVal`, or raw XDR encoding.

## SDK limitation and follow-up

The current SDK exposes typed pool simulation, pool information, share reads, token reads, and parameter types, but it does not yet expose signed `AmmPool` methods for swap, add-liquidity, or remove-liquidity submission. The example therefore prints the reviewed execution parameters and delegates those writes to the application’s wallet adapter rather than silently reaching back to the raw Stellar SDK. A follow-up should add typed signed lifecycle methods to `packages/sdk` before these three operations can be automated here.
