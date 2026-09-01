from __future__ import annotations


import pytest

from common import (
    ConfigError,
    I128_MAX,
    I128_MIN,
    decode_scval,
    encode_address,
    encode_bool,
    encode_i128,
    encode_string,
    normalize_for_json,
    parse_i128,
    required_env,
)


def test_parse_i128_accepts_minimum() -> None:
    assert parse_i128(str(I128_MIN)) == I128_MIN


def test_parse_i128_accepts_maximum() -> None:
    assert parse_i128(str(I128_MAX)) == I128_MAX


@pytest.mark.parametrize("value", [str(I128_MIN - 1), str(I128_MAX + 1), "", "abc", "1.2"])
def test_parse_i128_rejects_invalid_values(value: str) -> None:
    with pytest.raises(ValueError):
        parse_i128(value)


def test_required_env_names_missing_variable(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("AMM_CONTRACT_ID", raising=False)
    with pytest.raises(ConfigError, match="AMM_CONTRACT_ID"):
        required_env("AMM_CONTRACT_ID")


def test_required_env_returns_value(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("AMM_CONTRACT_ID", "CABC")
    assert required_env("AMM_CONTRACT_ID") == "CABC"


def test_i128_scval_round_trip() -> None:
    value = -12345678901234567890
    assert decode_scval(encode_i128(value)) == value


def test_i128_scval_round_trip_at_maximum() -> None:
    assert decode_scval(encode_i128(I128_MAX)) == I128_MAX


def test_address_scval_round_trip() -> None:
    address = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF"
    assert normalize_for_json(decode_scval(encode_address(address))) == address


def test_bool_scval_round_trip() -> None:
    assert decode_scval(encode_bool(True)) is True


def test_string_scval_round_trip() -> None:
    assert decode_scval(encode_string("Soroban AMM")) == "Soroban AMM"


def test_normalize_bytes() -> None:
    assert normalize_for_json(b"\x00\xff") == "00ff"


def test_normalize_nested_values() -> None:
    assert normalize_for_json({"amount": 3, "items": [b"a"]}) == {"amount": 3, "items": ["61"]}
