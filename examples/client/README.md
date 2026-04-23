# Soroban AMM TypeScript Client Example

This example shows how to interact with a deployed AMM contract from a JavaScript or TypeScript application.

It demonstrates:

- connecting to the Stellar testnet RPC
- reading pool state with `get_info()`
- quoting a swap with `get_amount_out()`
- executing a swap with `swap()`
- reading LP share balance with `shares_of()`

## Install

```sh
npm install
```

## Configure

Set the required environment variables before running the example:

```sh
AMM_CONTRACT_ID=<deployed AMM contract id>
SOURCE_SECRET=<secret key for the transaction source and default trader>
TOKEN_IN_CONTRACT_ID=<token A or token B contract id>
SWAP_AMOUNT_IN=100000
SWAP_MIN_OUT=0
```

Optional environment variables:

```sh
STELLAR_RPC_URL=https://soroban-testnet.stellar.org
STELLAR_NETWORK_PASSPHRASE="Test SDF Network ; September 2015"
LP_PROVIDER_ADDRESS=<defaults to SOURCE_SECRET public key>
```

The account behind `SOURCE_SECRET` is used as the transaction source and swap trader. It must exist on testnet, have enough XLM to pay transaction fees, and hold the input token being swapped.

## Build

```sh
npm run build
```

## Run

```sh
npm start
```

The example simulates read-only calls first, then submits a signed `swap()` transaction.
