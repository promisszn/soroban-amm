from __future__ import annotations

from typing import Any, cast
from unittest.mock import Mock

import pytest
from stellar_sdk import scval, xdr
from stellar_sdk.address import Address

import client as amm_client

TRADER = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
TOKEN_IN = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
NOW = 1_700_000_000


def invoke_swap(monkeypatch: pytest.MonkeyPatch, deadline_seconds: int = 300) -> tuple[Any, ...]:
    submitted = Mock(return_value=42)
    monkeypatch.setattr(amm_client, "submit_contract_call", submitted)
    monkeypatch.setattr(amm_client.time, "time", lambda: NOW)

    result = amm_client.swap(Mock(), Mock(), TRADER, TOKEN_IN, 100_000, 50, deadline_seconds)

    assert result == 42
    return submitted.call_args.args


def test_swap_submits_five_contract_arguments_in_signature_order(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    args = invoke_swap(monkeypatch)

    assert args[2] == "swap"
    contract_args = args[3:]
    assert len(contract_args) == 5
    native_args = [scval.to_native(value) for value in contract_args]
    assert [
        cast(Address, native_args[0]).address,
        cast(Address, native_args[1]).address,
        *native_args[2:],
    ] == [
        TRADER,
        TOKEN_IN,
        100_000,
        50,
        NOW + 300,
    ]


def test_swap_deadline_is_encoded_as_uint64(monkeypatch: pytest.MonkeyPatch) -> None:
    args = invoke_swap(monkeypatch)

    assert args[-1].type == xdr.SCValType.SCV_U64


def test_swap_deadline_is_now_plus_configured_window(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    args = invoke_swap(monkeypatch, deadline_seconds=300)

    assert scval.to_native(args[-1]) == NOW + 300


def test_default_swap_deadline_seconds_is_documented_default(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("SWAP_DEADLINE_SECONDS", raising=False)

    assert amm_client.get_swap_deadline_seconds() == 300
    assert amm_client.DEFAULT_SWAP_DEADLINE_SECONDS == 300


def test_zero_swap_deadline_window_still_submits(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    args = invoke_swap(monkeypatch, deadline_seconds=0)

    assert scval.to_native(args[-1]) == NOW


def test_negative_swap_deadline_window_fails_before_submission(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    submitted = Mock()
    monkeypatch.setattr(amm_client, "submit_contract_call", submitted)

    with pytest.raises(ValueError, match="SWAP_DEADLINE_SECONDS"):
        amm_client.swap(Mock(), Mock(), TRADER, TOKEN_IN, 100_000, 50, -1)

    submitted.assert_not_called()


def test_configured_swap_deadline_seconds_is_read_from_environment(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("SWAP_DEADLINE_SECONDS", "17")

    assert amm_client.get_swap_deadline_seconds() == 17
