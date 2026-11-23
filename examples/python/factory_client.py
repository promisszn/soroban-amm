#!/usr/bin/env python3
import argparse
import os
import sys

from stellar_sdk import Keypair, Network, scval
from stellar_sdk.address import Address
from stellar_sdk.contract import ContractClient

from common import (
    ConfigError,
    format_json,
    required_env,
    simulate_contract_call,
    submit_contract_call,
)

# Fee tiers accepted by Factory::create_pool (contracts/factory/src/lib.rs).
# Any other value is rejected on-chain with FactoryError::InvalidFeeBps.
FEE_TIERS: dict[int, int] = {0: 1, 1: 5, 2: 30, 3: 100}  # tier -> fee in bps
FEE_TIER_TABLE = "\n".join(f"  tier {t} = {bps} bps" for t, bps in FEE_TIERS.items())
BPS_TO_TIER: dict[int, int] = {bps: t for t, bps in FEE_TIERS.items()}


def resolve_fee_tier() -> int:
    """Read the fee tier from FEE_TIER, or map a legacy FEE_BPS value to a tier."""
    tier_raw = os.getenv("FEE_TIER")
    if tier_raw is not None and tier_raw != "":
        return validate_fee_tier(tier_raw)

    bps_raw = os.getenv("FEE_BPS")
    if bps_raw is not None and bps_raw != "":
        try:
            bps = int(bps_raw)
        except ValueError as exc:
            raise ConfigError(f"FEE_BPS must be one of 1, 5, 30, 100; got {bps_raw!r}") from exc
        if bps not in BPS_TO_TIER:
            raise ConfigError(
                f"FEE_BPS={bps} does not map to a factory fee tier. "
                f"Use FEE_TIER instead. Valid tiers:\n{FEE_TIER_TABLE}"
            )
        tier = BPS_TO_TIER[bps]
        print(f"Note: FEE_BPS={bps} is deprecated; mapped to FEE_TIER={tier}. Prefer FEE_TIER.")
        return tier

    return validate_fee_tier("2")


def validate_fee_tier(raw: str | int) -> int:
    """Parse and validate a fee tier in 0..=3, raising ConfigError otherwise."""
    try:
        tier = int(raw)
    except (TypeError, ValueError) as exc:
        raise ConfigError(f"FEE_TIER must be an integer 0-3; got {raw!r}") from exc
    if tier not in FEE_TIERS:
        raise ConfigError(f"Invalid fee tier {tier}. Valid tiers:\n{FEE_TIER_TABLE}")
    return tier


def main(argv: list[str] | None = None) -> int:
    # Explicit empty-by-default argv, not the implicit sys.argv[1:] default,
    # so main() is safely callable as a plain function (e.g. from tests running
    # under pytest) without this no-op parser choking on the caller's own CLI
    # flags and exiting the whole process with SystemExit(2).
    argparse.ArgumentParser(description="Soroban AMM integration example").parse_args(argv if argv is not None else [])
    rpc_url = os.getenv("STELLAR_RPC_URL", "https://soroban-testnet.stellar.org")
    network_passphrase = os.getenv(
        "STELLAR_NETWORK_PASSPHRASE",
        Network.TESTNET_NETWORK_PASSPHRASE,
    )

    factory_contract_id = required_env("FACTORY_CONTRACT_ID")
    source_secret = required_env("SOURCE_SECRET")
    token_a_contract_id = required_env("TOKEN_A_CONTRACT_ID")
    token_b_contract_id = required_env("TOKEN_B_CONTRACT_ID")
    fee_tier = resolve_fee_tier()

    source_keypair = Keypair.from_secret(source_secret)

    client = ContractClient(
        contract_id=factory_contract_id,
        rpc_url=rpc_url,
        network_passphrase=network_passphrase,
    )

    print(f"Connected to {rpc_url}")
    print(f"Factory contract: {factory_contract_id}")

    # 1. get_pool (read-only query)
    print("\n1. Querying if pool exists for pair...")
    pool_address = get_pool(client, token_a_contract_id, token_b_contract_id)
    print(f"Pool address: {pool_address}")

    # 2. create_pool if it doesn't exist
    if not pool_address:
        print(f"\n2. Creating pool for pair with fee_tier={fee_tier} ({FEE_TIERS[fee_tier]} bps)...")
        try:
            pool_address, governance_address = create_pool(
                client, source_keypair, source_keypair.public_key, token_a_contract_id, token_b_contract_id, fee_tier
            )
            print(f"Pool created successfully at: {pool_address}")
            if governance_address:
                print(f"Governance contract deployed at: {governance_address}")
            else:
                print("No governance contract was deployed (governance_wasm_hash not supplied).")
        except Exception as e:
            print(f"Failed to create pool (might already exist): {e}")
            client.server.close()
            return 1
    else:
        print("\n2. Pool already exists, skipping create_pool.")

    # 3. all_pools
    print("\n3. Listing all pools deployed by this factory...")
    pools = all_pools(client)
    print(format_json(pools))

    client.server.close()
    return 0


def create_pool(
    client: ContractClient,
    source_kp: Keypair,
    caller: str,
    token_a: str,
    token_b: str,
    fee_tier: int,
    governance_wasm_hash: bytes | None = None,
) -> tuple[str, str | None]:
    # Validate client-side so an out-of-range tier fails before any network call.
    validate_fee_tier(fee_tier)
    result = submit_contract_call(
        client,
        source_kp,
        "create_pool",
        scval.to_address(caller),
        scval.to_address(token_a),
        scval.to_address(token_b),
        scval.to_int128(fee_tier),
        scval.to_bytes(governance_wasm_hash) if governance_wasm_hash is not None else scval.to_void(),
    )

    pool_result, governance_result = result

    pool_address = pool_result.address if isinstance(pool_result, Address) else str(pool_result)
    governance_address: str | None = None
    if isinstance(governance_result, Address):
        governance_address = governance_result.address
    elif governance_result is not None:
        governance_address = str(governance_result)

    return pool_address, governance_address


def get_pool(client: ContractClient, token_a: str, token_b: str) -> str | None:
    result = simulate_contract_call(
        client,
        "get_pool",
        scval.to_address(token_a),
        scval.to_address(token_b),
    )
    if not result:
        return None
    if isinstance(result, Address):
        return result.address
    return str(result)


def all_pools(client: ContractClient) -> list[str]:
    result = simulate_contract_call(client, "all_pools")
    if not result:
        return []
    if isinstance(result, list):
        return [r.address if isinstance(r, Address) else str(r) for r in result]
    return [str(result)]


if __name__ == "__main__":
    try:
        raise SystemExit(main(sys.argv[1:]))
    except Exception as exc:
        print(exc, file=sys.stderr)
        raise SystemExit(1) from exc
