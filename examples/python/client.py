#!/usr/bin/env python3
import argparse

import os
import sys

from stellar_sdk import Keypair, Network, scval
from stellar_sdk.contract import ContractClient
from common import format_json, parse_i128, required_env, simulate_contract_call, submit_contract_call

I128_MIN = -(2**127)
I128_MAX = 2**127 - 1


def main() -> int:
    argparse.ArgumentParser(description="Soroban AMM integration example").parse_args()
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


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:
        print(exc, file=sys.stderr)
        raise SystemExit(1) from exc
