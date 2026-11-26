# Soroban AMM TypeScript client example

This example targets **Soroban Testnet** and uses the local `@soroban-amm/sdk` package rather than hand-encoding contract calls. It demonstrates the full lifecycle: reading pool state, quoting a swap with `simulateSwap`, applying slippage and deadline bounds, signing and submitting the swap via `AmmPool.swap`, and reading LP balances. Add-liquidity and remove-liquidity follow the same signed flow and are opt-in via environment variables.

## Install and build

From this directory:

```sh
npm install
npm run build
npm test
```

The SDK is consumed through the local workspace dependency `file:../../packages/sdk`, so the example stays aligned with the checked-in typed clients.

## Configure

Required variables:

| Variable | Description |
|---|---|
| `AMM_CONTRACT_ID` | Pool contract address |
| `TOKEN_IN_CONTRACT_ID` | Address of the token to swap in |
| `SOURCE_ADDRESS` | Public key (`G...`) of the trader/provider |
| `SIGNING_SECRET_KEY` | Secret key (`S...`) used to sign transactions |

Optional variables:

| Variable | Default | Description |
|---|---|---|
| `LP_TOKEN_CONTRACT_ID` | — | Read LP share balance after add-liquidity |
| `FACTORY_CONTRACT_ID` | — | Print all pools registered in the factory |
| `SWAP_AMOUNT_IN` | `100000` | Amount of `TOKEN_IN` to swap |
| `SLIPPAGE_BPS` | `50` | Maximum acceptable slippage in basis points |
| `DEADLINE_SECONDS` | `300` | Seconds from now until the transaction expires |
| `STELLAR_RPC_URL` | Soroban Testnet | RPC endpoint |
| `STELLAR_NETWORK_PASSPHRASE` | Testnet passphrase | Network identifier |
| `DRY_RUN` | — | Set to `1` to skip writes and only print reviewed parameters |
| `ADD_LIQUIDITY` | — | Set to `1` to also submit an add-liquidity transaction |
| `REMOVE_LIQUIDITY` | — | Set to `1` to also submit a remove-liquidity transaction (requires `LP_TOKEN_CONTRACT_ID`) |

A complete configured run:

```sh
AMM_CONTRACT_ID=<pool> \
TOKEN_IN_CONTRACT_ID=<token> \
SOURCE_ADDRESS=<G...> \
SIGNING_SECRET_KEY=<S...> \
LP_TOKEN_CONTRACT_ID=<lp-token> \
npm start
```

Set `DRY_RUN=1` to review parameters without submitting — useful when you do not have a funded testnet key available.

The example prints a readable contract error and links to `docs/error-codes.md` if an RPC or simulation call fails. It deliberately does not recreate `TransactionBuilder`, `nativeToScVal`, or raw XDR encoding.

## Signer adapters

`signer.ts` provides two shapes:

- **`KeypairSigner`** (`makeKeypairSigner`): reads `SIGNING_SECRET_KEY` from the environment and signs with a `Keypair`. Suitable for scripts, CI, and testnet use.
- **`SignerFunc`**: the raw function type the SDK's write methods accept — `(unsignedXdr: string) => Promise<string>`. Any async function that takes the base64-encoded unsigned transaction XDR and returns the signed XDR satisfies this interface, so HSM adapters, browser wallet bridges, and remote signers plug in without modification.

## Flow

1. `pool.getInfo()` — confirm the pair and read reserves.
2. `pool.simulateSwap(tokenIn, amountIn)` — quote the swap off-chain using the constant-product formula. No transaction is submitted.
3. Apply slippage: `minAmountOut = quote.amountOut * (10_000 - slippageBps) / 10_000`.
4. `pool.swap(params, signer)` — the SDK simulates the transaction (gets the Soroban footprint and auth entries), assembles the envelope, calls `signer.signTransaction` with the unsigned XDR, and submits the signed result. It polls until the transaction is confirmed or rejected, then returns the hash, ledger, and amount received in `returnValue`.
5. `pool.addLiquidity(params, signer)` — same signed lifecycle (opt-in via `ADD_LIQUIDITY=1`).
6. `pool.removeLiquidity(params, signer)` — same signed lifecycle (opt-in via `REMOVE_LIQUIDITY=1`).
