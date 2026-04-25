# Soroban AMM Python Client Example

This example shows how to interact with a deployed AMM contract from Python using `py-stellar-base` (`stellar-sdk`).

It demonstrates:

- connecting to the Stellar testnet RPC
- reading pool state with `get_info()`
- quoting a swap with `get_amount_out()`
- executing a swap with `swap()`
- reading LP share balance with `shares_of()`

## Install

```sh
python3 -m venv .venv
. .venv/bin/activate
pip install -r requirements.txt
```

## Configure

Set the required environment variables before running the example:

```sh
export AMM_CONTRACT_ID=<deployed AMM contract id>
export SOURCE_SECRET=<secret key for the transaction source and trader>
export TOKEN_IN_CONTRACT_ID=<token A or token B contract id>
export SWAP_AMOUNT_IN=100000
export SWAP_MIN_OUT=0
```

Optional environment variables:

```sh
export STELLAR_RPC_URL=https://soroban-testnet.stellar.org
export STELLAR_NETWORK_PASSPHRASE="Test SDF Network ; September 2015"
export LP_PROVIDER_ADDRESS=<defaults to SOURCE_SECRET public key>
```

The account behind `SOURCE_SECRET` must exist on testnet, have enough XLM to pay fees, and hold the input token being swapped.

## Run

```sh
python client.py
```

The example simulates the read-only calls first, then signs and submits a `swap()` transaction.
