"""Offline conformance tests: example encodings vs. the SDK's contract ABI fixture.

These tests never touch the network. They intercept ``invoke`` on a stub client,
build arguments with the examples' own helpers, and check the number, order and
XDR type of every encoded argument against ``contract-abi.json``.
"""
from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import pytest
from stellar_sdk import Keypair, scval
from stellar_sdk.xdr import SCValType

import client as amm_client
import factory_client
import governance_client
import twap_client
from common import InvocationError, encode_unit_variant

REPO_ROOT = Path(__file__).resolve().parents[2]
ABI_PATH = REPO_ROOT / "packages" / "sdk" / "src" / "__fixtures__" / "contract-abi.json"

SOURCE = Keypair.random()
ADDR = SOURCE.public_key
CONTRACT = "CDUMMY" + "A" * 50

# Map ABI type names to the XDR SCValType the encoded argument must carry.
ABI_TO_XDR: dict[str, set[SCValType]] = {
    "Address": {SCValType.SCV_ADDRESS},
    "i128": {SCValType.SCV_I128},
    "u32": {SCValType.SCV_U32},
    "u64": {SCValType.SCV_U64},
    "Bytes": {SCValType.SCV_BYTES},
    "BytesN": {SCValType.SCV_BYTES},
    "bool": {SCValType.SCV_BOOL},
    "Symbol": {SCValType.SCV_SYMBOL},
    "String": {SCValType.SCV_STRING},
    "Option": {SCValType.SCV_VOID, SCValType.SCV_BYTES, SCValType.SCV_ADDRESS},
    "Vec": {SCValType.SCV_VEC},
    "Void": {SCValType.SCV_VOID},
    # Unit-variant enums are encoded as a one-element vector holding the variant symbol.
    "Vote": {SCValType.SCV_VEC},
    "ProposalKind": {SCValType.SCV_VEC},
}


def xdr_types_for(abi_type: str) -> set[SCValType] | None:
    """Allowed XDR types for an ABI type name; generics like ``Option<BytesN<32>>`` use their head."""
    return ABI_TO_XDR.get(abi_type.split("<", 1)[0])


def load_abi() -> dict[str, Any]:
    return json.loads(ABI_PATH.read_text())


def abi_function(abi: dict[str, Any], name: str, contract: str | None = None) -> dict[str, Any]:
    """Look up an entrypoint in the SDK fixture (``{"contracts": {name: {"entrypoints": [...]}}}``).

    Several contracts share entrypoint names (``upgrade``, ``pause``, ...), so pass
    ``contract`` unless the name is unique across the fixture.
    """
    matches = [
        (contract_name, fn)
        for contract_name, spec in abi["contracts"].items()
        if contract is None or contract_name == contract
        for fn in spec["entrypoints"]
        if fn["name"] == name
    ]
    if not matches:
        raise KeyError(f"ABI has no function {name!r}" + (f" in {contract!r}" if contract else ""))
    if len(matches) > 1:
        owners = ", ".join(owner for owner, _ in matches)
        raise KeyError(f"ABI function {name!r} is ambiguous ({owners}); pass contract=")
    return matches[0][1]


class _Captured(Exception):
    """Raised by the stub to stop before any network activity."""


class _Recorder:
    def __init__(self) -> None:
        self.calls: list[tuple[str, list[Any]]] = []

    def __call__(self, method: str, parameters: Any = None, **_: Any) -> Any:
        self.calls.append((method, list(parameters or [])))
        raise _Captured()


@pytest.fixture
def recorder(monkeypatch: pytest.MonkeyPatch) -> _Recorder:
    rec = _Recorder()
    # The examples take any object with ``invoke``; StubClient is that object.
    monkeypatch.setattr(StubClient, "invoke", rec, raising=False)
    return rec


def capture(recorder: _Recorder, fn: Any, *args: Any, **kwargs: Any) -> tuple[str, list[Any]]:
    """Call an example helper and return the single (method, encoded args) it produced."""
    recorder.calls.clear()
    with pytest.raises((_Captured, ValueError, Exception)):
        fn(*args, **kwargs)
    assert recorder.calls, f"{fn.__name__} made no contract invocation"
    return recorder.calls[-1]


def assert_matches_abi(
    abi: dict[str, Any], method: str, args: list[Any], contract: str | None = None
) -> None:
    fn = abi_function(abi, method, contract)
    params = fn["inputs"]
    assert len(args) == len(params), (
        f"{method}: expected {len(params)} parameters, got {len(args)} "
        f"({[p['name'] for p in params]})"
    )
    for index, (param, arg) in enumerate(zip(params, args)):
        allowed = xdr_types_for(param["type"])
        assert allowed is not None, f"{method}.{param['name']}: unmapped ABI type {param['type']}"
        actual = arg.type if hasattr(arg, "type") else None
        assert actual in allowed, (
            f"{method}.{param['name']} (parameter {index}, ABI type {param['type']}): "
            f"encoded as {actual}, expected one of {sorted(t.name for t in allowed)}"
        )


# ---------- ABI self-checks ----------

def test_abi_fixture_is_present() -> None:
    assert ABI_PATH.exists(), f"ABI fixture missing: {ABI_PATH}"
    assert load_abi()["contracts"]["factory"]["entrypoints"]


def test_helper_scval_types_match_xdr_mapping() -> None:
    assert scval.to_vec([scval.to_symbol("For")]).type == SCValType.SCV_VEC
    assert encode_unit_variant("For").type == SCValType.SCV_VEC


# ---------- Factory ----------

def test_factory_create_pool_has_five_parameters_in_abi_order(recorder: _Recorder) -> None:
    abi = load_abi()
    method, args = capture(
        recorder, factory_client.create_pool, stub(), SOURCE, ADDR, ADDR, ADDR, 2
    )
    assert method == "create_pool"
    assert [p["name"] for p in abi_function(abi, "create_pool")["inputs"]] == [
        "caller", "token_a", "token_b", "fee_tier", "governance_wasm_hash"
    ]
    assert_matches_abi(abi, method, args)


def test_factory_create_pool_fee_tier_is_i128(recorder: _Recorder) -> None:
    _, args = capture(recorder, factory_client.create_pool, stub(), SOURCE, ADDR, ADDR, ADDR, 2)
    assert args[3].type == SCValType.SCV_I128
    assert args[3].i128.lo.uint64 == 2 and args[3].i128.hi.int64 == 0


def test_factory_create_pool_rejects_bps_before_network(recorder: _Recorder) -> None:
    recorder.calls.clear()
    with pytest.raises(ValueError):
        factory_client.create_pool(stub(), SOURCE, ADDR, ADDR, ADDR, 30)
    assert recorder.calls == []


def test_factory_create_pool_rejects_negative_tier(recorder: _Recorder) -> None:
    recorder.calls.clear()
    with pytest.raises(ValueError):
        factory_client.create_pool(stub(), SOURCE, ADDR, ADDR, ADDR, -1)
    assert recorder.calls == []


def test_factory_get_pool_matches_abi(recorder: _Recorder) -> None:
    abi = load_abi()
    method, args = capture(recorder, factory_client.get_pool, stub(), ADDR, ADDR)
    assert method == "get_pool"
    assert_matches_abi(abi, method, args)


def test_factory_all_pools_takes_no_arguments(recorder: _Recorder) -> None:
    abi = load_abi()
    method, args = capture(recorder, factory_client.all_pools, stub())
    assert method == "all_pools"
    assert args == []
    assert_matches_abi(abi, method, args, "factory")


# ---------- Governance ----------

def test_governance_vote_encodes_union_as_vector(recorder: _Recorder) -> None:
    method, args = capture(
        recorder, governance_client.vote, stub(), SOURCE, 1, "For"
    )
    assert method == "vote"
    assert args[2].type == SCValType.SCV_VEC
    assert len(args[2].vec.sc_vec) == 1
    assert args[2].vec.sc_vec[0].type == SCValType.SCV_SYMBOL
    assert args[2].vec.sc_vec[0].sym.sc_symbol == b"For"


@pytest.mark.parametrize("choice", ["For", "Against", "Abstain"])
def test_governance_vote_all_choices_match_abi(recorder: _Recorder, choice: str) -> None:
    abi = load_abi()
    method, args = capture(recorder, governance_client.vote, stub(), SOURCE, 1, choice)
    assert_matches_abi(abi, method, args)


def test_governance_vote_invalid_choice_raises_before_network(recorder: _Recorder) -> None:
    recorder.calls.clear()
    with pytest.raises(ValueError):
        governance_client.vote(stub(), SOURCE, 1, "Maybe")
    assert recorder.calls == []


def test_governance_propose_update_fee_kind_is_vector(recorder: _Recorder) -> None:
    abi = load_abi()
    method, args = capture(recorder, governance_client.propose, stub(), SOURCE, 50)
    assert method == "propose"
    assert args[1].type == SCValType.SCV_VEC
    assert_matches_abi(abi, method, args)


def test_governance_get_proposal_matches_abi(recorder: _Recorder) -> None:
    abi = load_abi()
    method, args = capture(recorder, governance_client.get_proposal, stub(), 3)
    assert_matches_abi(abi, method, args)


def test_governance_proposal_status_matches_abi(recorder: _Recorder) -> None:
    abi = load_abi()
    method, args = capture(recorder, governance_client.proposal_status, stub(), 3)
    assert_matches_abi(abi, method, args)


# ---------- AMM pool ----------

def test_amm_swap_matches_abi(recorder: _Recorder) -> None:
    abi = load_abi()
    recorder.calls.clear()
    with pytest.raises(InvocationError):
        amm_client.swap(stub(), SOURCE, ADDR, ADDR, 100, 0, 300)
    method, args = recorder.calls[-1]
    assert method == "swap"
    assert_matches_abi(abi, method, args, "amm")


def test_amm_get_amount_out_matches_abi(recorder: _Recorder) -> None:
    abi = load_abi()
    method, args = capture(recorder, amm_client.simulate_contract_call, stub(), "get_amount_out", amm_client.scval.to_address(ADDR), amm_client.scval.to_int128(10))
    assert_matches_abi(abi, method, args)


# ---------- TWAP ----------

def test_twap_save_snapshot_matches_abi(recorder: _Recorder) -> None:
    abi = load_abi()
    recorder.calls.clear()
    with pytest.raises(InvocationError):
        twap_client.save_snapshot(stub(), SOURCE, ADDR)
    method, args = recorder.calls[-1]
    assert method == "save_snapshot"
    assert_matches_abi(abi, method, args, "twap_consumer")


def test_twap_get_twap_price_matches_abi(recorder: _Recorder) -> None:
    abi = load_abi()
    method, args = capture(recorder, twap_client.get_twap_price, stub(), ADDR, 60)
    assert method == "get_twap_price"
    assert_matches_abi(abi, method, args)


def test_twap_get_twap_both_matches_abi(recorder: _Recorder) -> None:
    abi = load_abi()
    method, args = capture(recorder, twap_client.get_twap_both, stub(), ADDR, 60)
    assert_matches_abi(abi, method, args)


def test_twap_validate_price_matches_abi(recorder: _Recorder) -> None:
    abi = load_abi()
    method, args = capture(
        recorder, twap_client.validate_price_against_twap, stub(), ADDR, 60, 1000, 500
    )
    assert method == "validate_price_against_twap"
    assert_matches_abi(abi, method, args)


def test_twap_get_tracked_pools_takes_no_arguments(recorder: _Recorder) -> None:
    abi = load_abi()
    method, args = capture(recorder, twap_client.get_tracked_pools, stub())
    assert args == []
    assert_matches_abi(abi, method, args, "twap_consumer")


# ---------- Negative: a mutated call shape must be detected ----------

def test_detects_missing_parameter() -> None:
    abi = load_abi()
    with pytest.raises(AssertionError, match="expected 5 parameters, got 4"):
        assert_matches_abi(
            abi,
            "create_pool",
            [scval.to_address(ADDR), scval.to_address(ADDR), scval.to_address(ADDR), scval.to_int128(2)],
        )


def test_detects_wrong_xdr_type() -> None:
    abi = load_abi()
    with pytest.raises(AssertionError, match="fee_tier"):
        assert_matches_abi(
            abi,
            "create_pool",
            [
                scval.to_address(ADDR),
                scval.to_address(ADDR),
                scval.to_address(ADDR),
                scval.to_uint32(2),
                scval.to_void(),
            ],
        )


def stub() -> Any:
    """A StubClient typed as ``Any`` so it can stand in for ``ContractClient``."""
    return StubClient()


class StubClient:
    """Minimal stand-in for ContractClient; ``invoke`` is patched by the recorder."""

    def __init__(self) -> None:
        self.server = _StubServer()


class _StubServer:
    def close(self) -> None:
        return None
