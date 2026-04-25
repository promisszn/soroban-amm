#!/usr/bin/env python3

import json
import os
import re
import sys
from typing import Any

from stellar_sdk import Keypair, Network, scval
from stellar_sdk.address import Address
from stellar_sdk.contract import ContractClient

I128_MIN = -(2**127)
I128_MAX = 2**127 - 1


def main() -> int:
    rpc_url = os.getenv("STELLAR_RPC_URL", "https://soroban-testnet.stellar.org")
    network_passphrase = os.getenv(
        "STELLAR_NETWORK_PASSPHRASE",
        Network.TESTNET_NETWORK_PASSPHRASE,
    )

    amm_contract_id = required_env("AMM_CONTRACT_ID")
    source_secret = required_env("SOURCE_SECRET")
    token_in_contract_id = required_env("TOKEN_IN_CONTRACT_ID")

    swap_amount_in = parse_i128(os.getenv("SWAP_AMOUNT_IN", "100000"))
    swap_min_out = parse_i128(os.getenv("SWAP_MIN_OUT", "0"))

    source_keypair = Keypair.from_secret(source_secret)
    trader_address = source_keypair.public_key
    lp_provider_address = os.getenv("LP_PROVIDER_ADDRESS", trader_address)

    client = ContractClient(
        contract_id=amm_contract_id,
        rpc_url=rpc_url,
        network_passphrase=network_passphrase,
    )

    print(f"Connected to {rpc_url}")
    print(f"AMM contract: {amm_contract_id}")

    pool_info = simulate_contract_call(client, "get_info")
    print("Pool info:")
    print(format_json(pool_info))

    quoted_amount_out = simulate_contract_call(
        client,
        "get_amount_out",
        scval.to_address(token_in_contract_id),
        scval.to_int128(swap_amount_in),
    )
    print("Quote:")
    print(
        format_json(
            {
                "token_in_contract_id": token_in_contract_id,
                "amount_in": swap_amount_in,
                "amount_out": quoted_amount_out,
            }
        )
    )

    lp_shares = simulate_contract_call(
        client,
        "shares_of",
        scval.to_address(lp_provider_address),
    )
    print("LP share balance:")
    print(
        format_json(
            {
                "provider": lp_provider_address,
                "shares": lp_shares,
            }
        )
    )

    swap_result = submit_contract_call(
        client,
        source_keypair,
        "swap",
        scval.to_address(trader_address),
        scval.to_address(token_in_contract_id),
        scval.to_int128(swap_amount_in),
        scval.to_int128(swap_min_out),
    )
    print("Swap submitted:")
    print(
        format_json(
            {
                "trader": trader_address,
                "amount_out": swap_result,
            }
        )
    )

    client.server.close()
    return 0


def simulate_contract_call(
    client: ContractClient,
    method: str,
    *parameters: Any,
) -> Any:
    return client.invoke(
        method,
        parameters=list(parameters),
        parse_result_xdr_fn=scval.to_native,
    ).result()


def submit_contract_call(
    client: ContractClient,
    source_keypair: Keypair,
    method: str,
    *parameters: Any,
) -> Any:
    assembled = client.invoke(
        method,
        parameters=list(parameters),
        source=source_keypair.public_key,
        signer=source_keypair,
        parse_result_xdr_fn=scval.to_native,
    )
    assembled.sign_auth_entries(source_keypair)
    return assembled.sign_and_submit()


def required_env(name: str) -> str:
    value = os.getenv(name)
    if not value:
        raise ValueError(f"Missing required environment variable: {name}")
    return value


def parse_i128(value: str) -> int:
    if not re.fullmatch(r"-?\d+", value):
        raise ValueError(f'Expected an integer i128 value, got "{value}"')

    parsed = int(value)
    if parsed < I128_MIN or parsed > I128_MAX:
        raise ValueError(f"Value {value} is outside the i128 range")
    return parsed


def format_json(value: Any) -> str:
    return json.dumps(normalize_for_json(value), indent=2)


def normalize_for_json(value: Any) -> Any:
    if isinstance(value, Address):
        return value.address

    if isinstance(value, bytes):
        return value.hex()

    if isinstance(value, list):
        return [normalize_for_json(entry) for entry in value]

    if isinstance(value, dict):
        return {
            str(normalize_for_json(key)): normalize_for_json(entry_value)
            for key, entry_value in value.items()
        }

    return value


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:
        print(exc, file=sys.stderr)
        raise SystemExit(1) from exc
