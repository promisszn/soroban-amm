# Authorization Tests for Concentrated Liquidity Contract

## Summary

Added comprehensive authorization tests for all 11 privileged admin functions in the `concentrated_liquidity` contract. Each test verifies that the function requires proper authorization through three test cases:

1. **No authorization** → `Error(Auth, InvalidAction)`
2. **Stranger's authorization only** → `Error(Auth, InvalidAction)`  
3. **Correct signer's authorization** → Success

For functions that accept an `admin` parameter, additional tests verify that passing a stranger's address as the admin argument fails with `ClError::Unauthorized`.

## Mutation Check Coverage

Each `require_auth()` call is now guarded by at least one test that will fail if that specific authorization check is removed:

| Function | require_auth() Location | Test Name | Line in lib.rs |
|----------|------------------------|-----------|----------------|
| `set_oracle` | `admin.require_auth()` | `test_set_oracle_requires_admin_auth` | ~413 |
| `set_position_nft` | `admin.require_auth()` | `test_set_position_nft_requires_admin_auth` | ~447 |
| `set_max_oracle_deviation_bps` | `admin.require_auth()` | `test_set_max_oracle_deviation_bps_requires_admin_auth` | ~529 |
| `pause` | `admin.require_auth()` | `test_pause_requires_admin_auth` | ~556 |
| `unpause` | `admin.require_auth()` | `test_unpause_requires_admin_auth` | ~569 |
| `propose_admin` | `admin.require_auth()` | `test_propose_admin_requires_admin_auth` | ~581 |
| `accept_admin` | `new_admin.require_auth()` | `test_accept_admin_requires_nominee_auth` | ~604 |
| `upgrade` | `admin.require_auth()` | `test_upgrade_requires_admin_auth`, `test_upgrade_rejects_stranger_as_admin` | ~627 |
| `set_protocol_fee` | `admin.require_auth()` | `test_set_protocol_fee_requires_admin_auth` | ~645 |
| `withdraw_protocol_fees` | `admin.require_auth()` | `test_withdraw_protocol_fees_requires_admin_auth` | ~667 |
| `set_oracle_cardinality` | `admin.require_auth()` | `test_set_oracle_cardinality_requires_admin_auth` | ~3471 |

## Test Coverage Details

### Functions with admin parameter verification:
The following tests include both authorization checks AND verification that a stranger passed as the `admin` argument is rejected:

- `set_oracle`
- `set_position_nft`
- `set_max_oracle_deviation_bps`
- `propose_admin`
- `set_protocol_fee`
- `withdraw_protocol_fees`
- `set_oracle_cardinality`

### Functions with dual verification:
- `upgrade` has two separate tests: one for authorization and one explicitly for rejecting a stranger as admin
- `accept_admin` verifies both that a stranger cannot accept AND that the nominee needs proper authorization

## Files Modified

1. **Created**: `contracts/concentrated_liquidity/src/auth_tests.rs` (new file)
   - 11 test functions covering all admin operations
   - Helper functions for authorization testing
   - Fixture setup for test isolation

2. **Modified**: `contracts/concentrated_liquidity/src/lib.rs`
   - Added `#[cfg(test)] mod auth_tests;` module declaration

## Test Characteristics

- **Total test cases**: 11 main test functions
- **Authorization scenarios per function**: 3 (no auth, stranger auth, correct auth)
- **Additional Unauthorized checks**: 9 functions test stranger-as-admin rejection
- **Total assertions**: ~35+ individual checks
- **Expected runtime**: < 5 seconds (all tests use minimal fixtures)

## Verification Commands

### Run only authorization tests:
```bash
cargo test -p concentrated_liquidity auth_tests
```

### Run all concentrated_liquidity tests:
```bash
cargo test -p concentrated_liquidity
```

### Verify mutation detection (example):
To verify that removing a `require_auth()` call causes tests to fail, comment out any single `admin.require_auth()` or `new_admin.require_auth()` line in `lib.rs` and run:
```bash
cargo test -p concentrated_liquidity auth_tests
```

The corresponding test(s) should fail with an assertion about missing authorization.

## Notes

- All tests use `mock_all_auths()` for fixture setup (initializing the pool, minting tokens)
- Authorization checks use `set_auths(&[])` and `mock_auths(&[MockAuth {...}])` to precisely control which signatures are present
- Tests catch panics to verify `Error(Auth, InvalidAction)` is returned in unauthorized cases
- The pattern follows the established convention from `contracts/amm/src/auth_tests.rs`
