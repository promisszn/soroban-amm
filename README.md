# Soroban AMM

A constant-product Automated Market Maker (AMM) built as a Soroban smart contract on the Stellar blockchain. It implements the classic `x * y = k` bonding curve model — the same design used by Uniswap v2 — providing decentralized token swaps and liquidity provisioning.

---

## Table of Contents

- [Overview](#overview)
- [Architecture](#architecture)
- [Contracts](#contracts)
  - [AMM Pool Contract](#amm-pool-contract)
  - [LP Token Contract](#lp-token-contract)
  - [Factory Contract](#factory-contract)
  - [TWAP Consumer Contract](#twap-consumer-contract)
- [Math & Formulas](#math--formulas)
- [Getting Started](#getting-started)
  - [Prerequisites](#prerequisites)
  - [Build](#build)
  - [Test](#test)
- [Usage](#usage)
  - [Deploy via Factory](#deploy-via-factory)
  - [Deploy Manually](#deploy-manually)
  - [Add Liquidity](#add-liquidity)
  - [Swap Tokens](#swap-tokens)
  - [Remove Liquidity](#remove-liquidity)
  - [Query the Pool](#query-the-pool)
  - [Use the TWAP Oracle](#use-the-twap-oracle)
  - [TypeScript Client Example](#typescript-client-example)
  - [Python Client Example](#python-client-example)
- [Contributing](#contributing)
- [Security](#security)
- [License](#license)

---

## Overview

This AMM lets users:

- **Provide liquidity** — deposit two tokens into a pool and receive LP (Liquidity Provider) tokens representing their share.
- **Swap tokens** — exchange one pool token for the other at a price determined by the constant-product formula.
- **Redeem liquidity** — burn LP tokens to withdraw a proportional share of the pool's reserves.

All operations include slippage protection parameters. Fees are configurable in basis points at deployment and are distributed to liquidity providers by growing the pool reserves.

---

## Architecture

The project is a Cargo workspace with four contracts:

```
soroban-amm/
├── Cargo.toml                  # Workspace root
└── contracts/
    ├── amm/                    # Core AMM pool contract
    │   └── src/lib.rs
    ├── token/                  # SEP-41 LP token contract
    │   └── src/lib.rs
    ├── factory/                # Pool factory contract
    │   └── src/lib.rs
    └── twap_consumer/          # Example TWAP consumer integration contract
        └── src/lib.rs
```

The AMM contract depends on the token contract. When liquidity is added or removed, the AMM calls the LP token contract to mint or burn shares on behalf of the provider. The factory contract depends on both AMM and token: it deploys and initialises them as a pair when a new pool is created.

---

## Contracts

---

## Storage Layout

### AMM Pool Contract

| Key | Storage Tier | Type | Description |
|---|---|---|---|
| `TokenA` | Instance | `Address` | First pool asset |
| `TokenB` | Instance | `Address` | Second pool asset |
| `LpToken` | Instance | `Address` | LP token contract |
| `ReserveA` | Instance | `i128` | Current TokenA reserves |
| `ReserveB` | Instance | `i128` | Current TokenB reserves |
| `TotalShares` | Instance | `i128` | Total LP shares issued |
| `FeeBps` | Instance | `i128` | Swap fee in basis points |

### LP Token Contract

| Key | Storage Tier | Type | Description |
|---|---|---|---|
| `Admin` | Instance | `Address` | Contract administrator (the AMM pool) |
| `Name` | Instance | `String` | Token name |
| `Symbol` | Instance | `String` | Token symbol |
| `Decimals` | Instance | `u32` | Token decimal places |
| `TotalSupply` | Instance | `i128` | Total shares in circulation |
| `Balance(Address)` | Persistent | `i128` | Individual user share balance |
| `Allowance(Address, Address)` | Persistent | `i128` | Third-party spending allowance |

---

## Upgrade Considerations

- **Storage Immutability**: Critical setup parameters (e.g., `TokenA`, `TokenB`, `LpToken`) are immutable after `initialize`.
- **Breaking Changes**: Modifying `DataKey` variants or data types constitutes a breaking change. Since Soroban storage is keyed by the enum's binary representation, any restructuring requires a new deployment or a careful migration strategy.
- **State Migration**: Upgrading logic while preserving state is possible via contract code upgrades, but changing storage tiers (e.g., Instance to Persistent) requires manual data relocation.

---

## Public Interface

| Key | Type | Description |
|---|---|---|
| `TokenA` | `Address` | First pool asset |
| `TokenB` | `Address` | Second pool asset |
| `LpToken` | `Address` | LP token contract |
| `ReserveA` | `i128` | Pool's current balance of TokenA |
| `ReserveB` | `i128` | Pool's current balance of TokenB |
| `TotalShares` | `i128` | Total LP shares outstanding |
| `Shares(Address)` | `i128` | LP shares held by a specific provider |
| `FeeBps` | `i128` | Swap fee in basis points (e.g. `30` = 0.30%) |
| `Paused` | `bool` | Emergency circuit breaker state |
| `FlashLoanFeeBps` | `i128` | Flash-loan fee in basis points; defaults to `FeeBps` |
### AMM Pool Contract

Located in [contracts/amm/src/lib.rs](contracts/amm/src/lib.rs).

| Function | Description |
|---|---|
| `initialize(token_a, token_b, lp_token, fee_bps)` | One-time pool setup |
| `pause(admin)` | Pause state-changing pool operations; requires `admin` auth |
| `unpause(admin)` | Resume state-changing pool operations; requires `admin` auth |
| `is_paused() → bool` | Read the current pause state |
| `initialize_with_flash_loan_fee(token_a, token_b, lp_token, fee_bps, flash_loan_fee_bps)` | One-time pool setup with a distinct flash-loan fee |
| `flash_loan(receiver, token, amount, data) -> fee` | Borrow pool reserves and repay within the receiver callback |
| `add_liquidity(provider, amount_a, amount_b, min_shares) → shares` | Deposit tokens, receive LP shares |
| `remove_liquidity(provider, shares, min_a, min_b) → (a, b)` | Burn LP shares, withdraw tokens |
| `swap(trader, token_in, amount_in, min_out) → amount_out` | Exchange tokens |
| `get_amount_out(token_in, amount_in) → amount_out` | Quote a swap without executing it |
| `get_info() → PoolInfo` | Read pool state (reserves, fee, shares) |
| `shares_of(provider) → shares` | Read an LP's share balance |

### Factory Contract

Located in [contracts/factory/src/lib.rs](contracts/factory/src/lib.rs).

A single-entry-point contract for creating and discovering AMM pools. The factory deploys a new AMM pool and its paired LP token in one transaction, enforces uniqueness per token pair, and maintains a registry of all pools it has deployed.

#### Storage

| Key | Type | Description |
|---|---|---|
| `Admin` | `Address` | Factory administrator; set as AMM fee recipient |
| `AmmWasmHash` | `BytesN<32>` | WASM hash of the AMM pool contract |
| `TokenWasmHash` | `BytesN<32>` | WASM hash of the LP token contract |
| `Pool(Address, Address)` | `Address` | Normalised token pair → pool address |
| `AllPools` | `Vec<Address>` | Ordered list of all deployed pool addresses |
| `PoolCount` | `u64` | Monotonic counter used to derive deploy salts |

#### Public Interface

| Function | Description |
|---|---|
| `initialize(admin, amm_wasm_hash, token_wasm_hash)` | One-time factory setup |
| `create_pool(token_a, token_b, fee_bps) → Address` | Deploy a new AMM + LP token pair; panics on duplicate |
| `get_pool(token_a, token_b) → Option<Address>` | Look up an existing pool (order-independent) |
| `all_pools() → Vec<Address>` | List every pool deployed by this factory |

#### Notes

- Token pair order is **normalised** at creation time (smaller address stored first). `get_pool` accepts either order.
- `create_pool` panics with `"pool already exists"` if a pool for the pair is already registered.
- The factory admin is set as the AMM's `fee_recipient`; protocol fees start at 0 bps and can be enabled later.

---

### TWAP Consumer Contract

Located in [contracts/twap_consumer/src/lib.rs](contracts/twap_consumer/src/lib.rs).

An example integration contract that reads the AMM cumulative oracle and computes a windowed TWAP for token A.

| Function | Description |
|---|---|
| `save_snapshot(pool)` | Stores `(cum_a, cum_b, pool_ts)` under `Snapshot(pool, pool_ts)` |
| `get_twap_price(pool, window_seconds) -> i128` | Returns `(cum_a_now - cum_a_then) / window_seconds`, where `cum_a_then` comes from the snapshot at `now_ts - window_seconds` |

This contract is intentionally simple and intended as integration documentation for downstream builders.

---

### LP Token Contract
#### Flash Loan Receiver Interface

Borrowers must implement a callback contract with this interface:

```rust
pub trait FlashLoanReceiver {
    fn on_flash_loan(env: Env, token: Address, amount: i128, fee: i128, data: Bytes) -> bool;
}
```

During `flash_loan`, the AMM transfers `amount` of `token` to `receiver`, invokes `on_flash_loan`, and then checks that the pool's token balance increased by at least `fee`. If the receiver does not return `amount + fee` before the callback finishes, the transaction reverts.

### LP Token Contract

Located in [contracts/token/src/lib.rs](contracts/token/src/lib.rs).

| Function | Description |
|---|---|
| `initialize(admin, name, symbol, decimals)` | One-time token setup |
| `mint(to, amount)` | Mint tokens — admin only |
| `burn(from, amount)` | Burn tokens — admin only |
| `transfer(from, to, amount)` | Transfer between accounts |
| `transfer_from(spender, from, to, amount)` | Spend an approved allowance |
| `approve(from, spender, amount)` | Approve a spender |
| `balance(id) → i128` | Read account balance |
| `allowance(from, spender) → i128` | Read spending allowance |
| `total_supply() → i128` | Read total tokens minted |

---

## Math & Formulas

### Constant-Product Invariant

Every swap must satisfy:

```
reserve_a * reserve_b = k   (constant)
```

### Swap Output

Fees are deducted from the input before applying the formula:

```
amount_in_with_fee = amount_in * (10_000 - fee_bps)

amount_out = (amount_in_with_fee * reserve_out)
           / (reserve_in * 10_000 + amount_in_with_fee)
```

### Initial LP Shares (First Deposit)

Uses the geometric mean of the deposited amounts:

```
shares = sqrt(amount_a * amount_b)
```

### Subsequent LP Shares

Uses the lesser of the two proportional contributions to prevent imbalanced deposits:

```
shares = min(
    amount_a * total_shares / reserve_a,
    amount_b * total_shares / reserve_b
)
```

### Liquidity Removal

Proportional to pool ownership at the time of withdrawal:

```
out_a = shares * reserve_a / total_shares
out_b = shares * reserve_b / total_shares
```

---

## Getting Started

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (stable toolchain)
- `wasm32-unknown-unknown` compilation target:
  ```sh
  rustup target add wasm32-unknown-unknown
  ```
- [Stellar CLI](https://developers.stellar.org/docs/tools/stellar-cli) (`stellar`) for deployment:
  ```sh
  cargo install --locked stellar-cli --features opt
  ```

### Setup

1. **Clone the repository:**

   ```sh
   git clone https://github.com/your-org/soroban-amm.git
   cd soroban-amm
   ```

2. **Verify the toolchain and target are installed:**

   ```sh
   rustup show                          # confirm stable toolchain is active
   rustup target list --installed       # should include wasm32-unknown-unknown
   ```

   If the WASM target is missing:

   ```sh
   rustup target add wasm32-unknown-unknown
   ```

3. **Configure the Stellar CLI for your target network** (testnet shown):

   ```sh
   stellar network add testnet \
     --rpc-url https://soroban-testnet.stellar.org \
     --network-passphrase "Test SDF Network ; September 2015"
   ```

4. **Create or import an account identity:**

   ```sh
   # Generate a new keypair and fund it via Friendbot
   stellar keys generate --default-seed mykey
   stellar keys fund mykey --network testnet
   ```

   Or import an existing secret key:

   ```sh
   stellar keys add mykey --secret-key
   # paste your secret key when prompted
   ```

5. **Confirm everything is wired up:**

   ```sh
   stellar keys address mykey           # should print your public key
   ```

You are now ready to build, test, and deploy.

### Build

Build all contracts as optimised WASM binaries:

```sh
cargo wasm
```

`wasm` is a Cargo alias defined in [.cargo/config.toml](.cargo/config.toml) that expands to:

```sh
cargo build --release --target wasm32-unknown-unknown
```

Output files:

```
target/wasm32-unknown-unknown/release/amm.wasm
target/wasm32-unknown-unknown/release/token.wasm
```

### Test

The AMM and token contract tests run without pre-built WASM:

```sh
cargo test -p amm -p token
```

The factory tests embed compiled WASM at compile time, so build WASM first:

```sh
cargo build --release --target wasm32-unknown-unknown
cargo test -p factory
```

Or run the full suite in one go:

```sh
cargo build --release --target wasm32-unknown-unknown && cargo test
```

For a real-network smoke test on Stellar testnet, run the end-to-end script:

```sh
scripts/e2e.sh
```

The script deploys fresh contracts, funds a test account, adds liquidity, swaps, removes liquidity, and exits non-zero on any failed assertion.

---

## Usage

### Automated Deployment

The fastest way to deploy a full AMM environment (Token A, Token B, LP Token, and AMM Pool) to testnet is using the provided deployment script:

```sh
./scripts/deploy.sh [network]
```

- **network**: Optional target network (defaults to `testnet`).
- The script builds contracts, generates/funds a deployer account, deploys all contracts, and initialises them.
- Deployed contract IDs are printed to the console and saved to `.soroban-amm.deploy.env`.

### ABI Schema

A machine-readable JSON schema of all public contract functions, parameters, and events is available at [docs/abi.json](docs/abi.json).

### Development

The project includes a `Makefile` to simplify common development tasks:

- `make build`: Build contracts for production (`wasm32-unknown-unknown`)
- `make test`: Run all contract unit tests
- `make fmt`: Format code using `cargo fmt`
- `make lint`: Run `clippy` with warnings treated as errors
- `make check`: Run formatting, linting, and tests in sequence
- `make deploy`: Deploy contracts to testnet via `scripts/deploy.sh`
- `make e2e`: Run full end-to-end integration tests
- `make clean`: Remove build artifacts

### Deploy via Factory

The factory is the recommended way to create pools. It deploys and initialises the AMM pool and its LP token in a single transaction, and registers the pool in its on-chain registry.

**1. Upload the contract WASM blobs:**

```sh
stellar contract upload \
  --wasm target/wasm32-unknown-unknown/release/amm.wasm \
  --network testnet --source <YOUR_KEY>
# → prints AMM_WASM_HASH

stellar contract upload \
  --wasm target/wasm32-unknown-unknown/release/token.wasm \
  --network testnet --source <YOUR_KEY>
# → prints TOKEN_WASM_HASH
```

**2. Deploy the factory:**

```sh
stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/factory.wasm \
  --network testnet --source <YOUR_KEY>
# → prints FACTORY_CONTRACT_ID
```

**3. Initialise the factory:**

```sh
stellar contract invoke \
  --id <FACTORY_CONTRACT_ID> \
  --network testnet --source <YOUR_KEY> \
  -- initialize \
  --admin <YOUR_ADDRESS> \
  --amm_wasm_hash <AMM_WASM_HASH> \
  --token_wasm_hash <TOKEN_WASM_HASH>
```

**4. Create a pool (deploys AMM + LP token, registers the pair):**

```sh
stellar contract invoke \
  --id <FACTORY_CONTRACT_ID> \
  --network testnet --source <YOUR_KEY> \
  -- create_pool \
  --token_a <TOKEN_A_CONTRACT_ID> \
  --token_b <TOKEN_B_CONTRACT_ID> \
  --fee_bps 30
# → prints the new POOL_CONTRACT_ID
```

**5. Look up an existing pool:**

```sh
stellar contract invoke \
  --id <FACTORY_CONTRACT_ID> \
  -- get_pool \
  --token_a <TOKEN_A_CONTRACT_ID> \
  --token_b <TOKEN_B_CONTRACT_ID>

stellar contract invoke --id <FACTORY_CONTRACT_ID> -- all_pools
```

---

### Deploy Manually

Deploy the LP token contract first, then the AMM pool. The AMM contract address becomes the LP token's admin.

```sh
# Deploy the LP token
stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/token.wasm \
  --network testnet \
  --source <YOUR_KEY>

# Deploy the AMM pool
stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/amm.wasm \
  --network testnet \
  --source <YOUR_KEY>
```

Initialize the LP token (admin = AMM contract address):

```sh
stellar contract invoke \
  --id <LP_TOKEN_CONTRACT_ID> \
  --network testnet \
  --source <YOUR_KEY> \
  -- initialize \
  --admin <AMM_CONTRACT_ID> \
  --name "Pool LP Token" \
  --symbol "AMMLP" \
  --decimals 7
```

Initialize the AMM pool (fee of 30 bps = 0.30%):

```sh
stellar contract invoke \
  --id <AMM_CONTRACT_ID> \
  --network testnet \
  --source <YOUR_KEY> \
  -- initialize \
  --token_a <TOKEN_A_CONTRACT_ID> \
  --token_b <TOKEN_B_CONTRACT_ID> \
  --lp_token <LP_TOKEN_CONTRACT_ID> \
  --fee_bps 30 \
  --fee_recipient <FEE_RECIPIENT_ADDRESS> \
  --protocol_fee_bps 0
```

### Add Liquidity

```sh
stellar contract invoke \
  --id <AMM_CONTRACT_ID> \
  --network testnet \
  --source <YOUR_KEY> \
  -- add_liquidity \
  --provider <PROVIDER_ADDRESS> \
  --amount_a 1000000 \
  --amount_b 2000000 \
  --min_shares 0
```

`min_shares` is the minimum LP tokens you are willing to accept. Set to `0` to skip slippage protection during initial seeding.

### Swap Tokens

```sh
stellar contract invoke \
  --id <AMM_CONTRACT_ID> \
  --network testnet \
  --source <YOUR_KEY> \
  -- swap \
  --trader <TRADER_ADDRESS> \
  --token_in <TOKEN_A_CONTRACT_ID> \
  --amount_in 100000 \
  --min_out 0
```

Use `get_amount_out` first to compute an appropriate `min_out`.

### Remove Liquidity

```sh
stellar contract invoke \
  --id <AMM_CONTRACT_ID> \
  --network testnet \
  --source <YOUR_KEY> \
  -- remove_liquidity \
  --provider <PROVIDER_ADDRESS> \
  --shares <LP_SHARE_AMOUNT> \
  --min_a 0 \
  --min_b 0
```

### Query the Pool

```sh
# Full pool info
stellar contract invoke --id <AMM_CONTRACT_ID> -- get_info

# Quote a swap
stellar contract invoke --id <AMM_CONTRACT_ID> \
  -- get_amount_out \
  --token_in <TOKEN_A_CONTRACT_ID> \
  --amount_in 100000

# LP share balance
stellar contract invoke --id <AMM_CONTRACT_ID> \
  -- shares_of --provider <PROVIDER_ADDRESS>
```

### Use the TWAP Oracle

The AMM exposes cumulative price state with `get_price_cumulative()`. The example consumer contract shows one way to turn that into a fixed-window TWAP.

1. Deploy `twap_consumer.wasm`.
2. Save a snapshot (for example every minute):

```sh
stellar contract invoke \
  --id <TWAP_CONSUMER_CONTRACT_ID> \
  --network testnet --source <YOUR_KEY> \
  -- save_snapshot \
  --pool <AMM_CONTRACT_ID>
```

3. After `window_seconds` has elapsed, read TWAP:

```sh
stellar contract invoke \
  --id <TWAP_CONSUMER_CONTRACT_ID> \
  --network testnet --source <YOUR_KEY> \
  -- get_twap_price \
  --pool <AMM_CONTRACT_ID> \
  --window_seconds 60
```

Notes:

- `window_seconds` must be greater than 0.
- `save_snapshot` must have been called exactly at `now_ts - window_seconds` (matching the pool timestamp used by `get_price_cumulative`).
- Returned TWAP is scaled the same way as AMM spot price (`1_000_000` scale factor).

### TypeScript Client Example

A standalone TypeScript client is available in [examples/client](examples/client). It demonstrates connecting to Stellar testnet RPC, reading `get_info()`, quoting with `get_amount_out()`, executing `swap()`, and reading LP shares with `shares_of()`.

```sh
cd examples/client
npm install
npm run build
npm start
```

### Python Client Example

A standalone Python client is available in [examples/python](examples/python). It demonstrates the same flow using `py-stellar-base` (`stellar-sdk`): connect to Stellar testnet RPC, read `get_info()`, quote with `get_amount_out()`, execute `swap()`, and read LP shares with `shares_of()`.

```sh
cd examples/python
python3 -m venv .venv
. .venv/bin/activate
pip install -r requirements.txt
python client.py
```

---

## Contributing

Contributions are welcome. Please follow the guidelines below to keep the codebase consistent and review cycles short.

### Reporting Issues

- Search existing issues before opening a new one.
- Include the Rust / `soroban-sdk` version, the steps to reproduce, and the expected vs. actual behavior.
- For security vulnerabilities, **do not open a public issue** — see [SECURITY.md](SECURITY.md) for the responsible disclosure process.

### Development Workflow

1. **Fork** the repository and create a branch from `main`:

   ```sh
   git checkout -b feat/my-feature
   ```

   Branch naming conventions:
   | Prefix | Use for |
   |---|---|
   | `feat/` | New features |
   | `fix/` | Bug fixes |
   | `refactor/` | Code restructuring without behavior change |
   | `test/` | Adding or improving tests |
   | `docs/` | Documentation only |
   | `chore/` | Build scripts, tooling, dependencies |

2. **Make your changes**, then ensure the build and tests pass:

   ```sh
   cargo build --release --target wasm32-unknown-unknown
   cargo test
   ```

3. **Write tests** for any new behavior. All public functions should have at least one test. Tests live alongside the implementation in `src/lib.rs` under a `#[cfg(test)]` module.

4. **Keep commits focused.** One logical change per commit. Use the [Conventional Commits](https://www.conventionalcommits.org/) format:

   ```
   feat: add time-weighted average price accumulator
   fix: prevent zero-share mint on initial deposit
   test: cover swap with maximum fee setting
   ```

5. **Open a Pull Request** against `main`. In the PR description:
   - Explain _what_ changed and _why_.
   - Reference any related issues with `Closes #<issue>` or `Related to #<issue>`.
   - If the change affects contract behavior, include before/after output or test coverage evidence.

### Code Style

- An [`.editorconfig`](.editorconfig) at the workspace root defines shared formatting rules (UTF-8, LF line endings, 4-space indentation, trailing-whitespace trimming). Most editors apply it automatically; install the [EditorConfig plugin](https://editorconfig.org/#download) if yours does not.
- A [`rustfmt.toml`](rustfmt.toml) at the workspace root defines Rust formatting rules. It enforces:
  - **Edition**: 2021
  - **Max width**: 100 columns
  - **Indentation**: 4 spaces
  - **Line endings**: Unix (LF)
  - **Import grouping**: Standard library, external crates, then crate-local modules
- Run `cargo fmt` before committing to automatically apply these rules.
- Run `cargo clippy -- -D warnings` and resolve any warnings before opening a PR.
- Prefer explicit arithmetic with overflow checks over silent wrapping. The release profile already enables `overflow-checks = true`.
- Avoid unsafe code. There is no reason to use `unsafe` in a Soroban contract.
- Do not add dependencies without discussion. The contract binary size and attack surface matter.

### Pull Request Checklist

Before requesting review, confirm:

- [ ] `cargo fmt` has been run
- [ ] `cargo clippy -- -D warnings` passes
- [ ] `cargo test` passes
- [ ] New behavior is covered by tests
- [ ] Public interface changes are reflected in this README
- [ ] Commit messages follow the Conventional Commits format

### Versioning

This project follows [Semantic Versioning](https://semver.org/). Breaking changes to the on-chain interface (function signatures, storage layout, error codes) constitute a major version bump.

---

## Security

Please do not open public issues for security vulnerabilities. See [SECURITY.md](SECURITY.md) for the full vulnerability disclosure policy, supported versions, and how to reach the maintainers privately.

---

## License

This project is licensed under the [MIT License](LICENSE).
