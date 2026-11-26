//! Keeps `SdkAmmError` in lock-step with the pool contract's `AmmError`.
//!
//! Two layers:
//!
//! * **Drift** — both enums are `#[contracterror]`, so each carries its
//!   contract-spec entry (`spec_xdr()`): the exact `(name, value)` list that is
//!   embedded in the WASM and published in `docs/abi.json`. The test parses
//!   both entries and requires the case lists to be identical. Adding a variant
//!   to `AmmError` (say `#31`) without mirroring it in `SdkAmmError` — or
//!   renaming / renumbering one — fails here, with no hardcoded code range to
//!   keep up to date. Every contract case is additionally decoded through
//!   `SdkAmmError::try_from(soroban_sdk::Error)`, the conversion the generated
//!   client uses, so a spec match that fails to decode would also fail.
//!
//! * **Client error path** — a stub pool returns each contract error code
//!   through a real cross-contract invocation, and the call is made through the
//!   public `AmmPoolSdk` wrapper (which funnels every `try_*` result through
//!   `unwrap_contract_result`). Each code must come back as
//!   `Err(SdkAmmError::…)` rather than panicking with "failed to decode
//!   contract result", which is what codes 25-30 did before the SDK mirrored
//!   them.

use soroban_amm_sdk::client::AmmPoolSdk;
use soroban_amm_sdk::types::SdkAmmError;
use soroban_sdk::{
    contract, contractimpl, symbol_short,
    testutils::Address as _,
    xdr::{Limits, ReadXdr, ScSpecEntry},
    Address, Env, Error,
};

/// `(name, value)` for every case of a `#[contracterror]` spec entry.
fn error_cases(spec_xdr: &[u8]) -> Vec<(String, u32)> {
    match ScSpecEntry::from_xdr(spec_xdr, Limits::none()).expect("valid contract spec XDR") {
        ScSpecEntry::UdtErrorEnumV0(e) => e
            .cases
            .iter()
            .map(|c| (c.name.to_utf8_string_lossy(), c.value))
            .collect(),
        other => panic!("expected an error-enum spec entry, got {other:?}"),
    }
}

#[test]
fn sdk_amm_error_spec_matches_contract_amm_error() {
    let contract_cases = error_cases(&amm::AmmError::spec_xdr());
    let sdk_cases = error_cases(&SdkAmmError::spec_xdr());

    // Sanity: guard against the parse silently yielding nothing.
    assert!(
        contract_cases.len() >= 30,
        "AmmError spec unexpectedly small"
    );

    let missing: Vec<_> = contract_cases
        .iter()
        .filter(|c| !sdk_cases.contains(c))
        .collect();
    let extra: Vec<_> = sdk_cases
        .iter()
        .filter(|c| !contract_cases.contains(c))
        .collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "SdkAmmError has drifted from amm::AmmError\n  \
         in AmmError but not SdkAmmError: {missing:?}\n  \
         in SdkAmmError but not AmmError: {extra:?}"
    );
}

#[test]
fn every_contract_error_code_decodes_to_the_same_sdk_variant() {
    for (name, code) in error_cases(&amm::AmmError::spec_xdr()) {
        let err = Error::from_contract_error(code);
        let contract = amm::AmmError::try_from(err)
            .unwrap_or_else(|_| panic!("AmmError does not decode its own code {code}"));
        let sdk = SdkAmmError::try_from(err)
            .unwrap_or_else(|_| panic!("SdkAmmError cannot decode AmmError::{name} (#{code})"));
        assert_eq!(sdk as u32, code);
        assert_eq!(format!("{sdk:?}"), format!("{contract:?}"));
        assert_eq!(format!("{sdk:?}"), name);
    }
}

// ── Client error path ─────────────────────────────────────────────────────────

/// Minimal stand-in for a pool whose `swap` always fails with a configurable
/// contract error code.
#[contract]
pub struct RejectingPool;

#[contractimpl]
impl RejectingPool {
    pub fn set_code(env: Env, code: u32) {
        env.storage().instance().set(&symbol_short!("code"), &code);
    }

    pub fn swap(
        env: Env,
        _trader: Address,
        _token_in: Address,
        _amount_in: i128,
        _min_out: i128,
        _deadline: u64,
    ) -> Result<i128, Error> {
        let code: u32 = env
            .storage()
            .instance()
            .get(&symbol_short!("code"))
            .unwrap();
        Err(Error::from_contract_error(code))
    }
}

#[test]
fn sdk_client_surfaces_codes_25_through_30_as_typed_errors() {
    let env = Env::default();
    let pool = env.register_contract(None, RejectingPool);
    let stub = RejectingPoolClient::new(&env, &pool);
    let sdk = AmmPoolSdk::new(&env, &pool);
    let trader = Address::generate(&env);
    let token_in = Address::generate(&env);

    let cases = [
        (25, SdkAmmError::QuorumNotReached),
        (26, SdkAmmError::InvalidQuorum),
        (27, SdkAmmError::EmergencyDelayNotElapsed),
        (28, SdkAmmError::InvalidRecipient),
        (29, SdkAmmError::MultisigGuardActive),
        (30, SdkAmmError::InvalidEmergencyDelay),
    ];
    for (code, expected) in cases {
        stub.set_code(&code);
        assert_eq!(
            sdk.execute_swap(&trader, &token_in, 1_000, 0, u64::MAX),
            Err(expected),
            "code {code}"
        );
    }
}

#[test]
fn sdk_client_surfaces_every_contract_error_code_as_a_typed_error() {
    let env = Env::default();
    let pool = env.register_contract(None, RejectingPool);
    let stub = RejectingPoolClient::new(&env, &pool);
    let sdk = AmmPoolSdk::new(&env, &pool);
    let trader = Address::generate(&env);
    let token_in = Address::generate(&env);

    for (name, code) in error_cases(&amm::AmmError::spec_xdr()) {
        stub.set_code(&code);
        let err = sdk
            .execute_swap(&trader, &token_in, 1_000, 0, u64::MAX)
            .expect_err("stub pool always fails");
        assert_eq!(err as u32, code, "AmmError::{name}");
        assert_eq!(format!("{err:?}"), name);
    }
}
